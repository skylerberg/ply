//! The trusted computing base, as the CLI reads and reports it.

use crate::commands::common::plural;
use crate::config::Configuration;
use crate::db::{self, Database, DbConfig};
use ply_eval::host::{HostBinding, HostListing, HostRegistry, HostRow, HostRuntime};
use ply_host::tls;
use ply_span::{Diagnostic, Span};
use ply_ty::CheckOutput;
use ply_ty::ty::Footprint;
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::Arc;

/// The trusted computing base this binary was built with.
pub fn registry() -> HostRegistry {
    ply_host::registry()
}

fn registry_for(check: &CheckOutput, trace: Option<Arc<ply_host::trace::Trace>>) -> HostRegistry {
    let database = check
        .effects
        .values()
        .any(|e| e.name.as_str() == ply_host::db::EFFECT);
    match trace {
        Some(trace) => ply_host::registry_over(trace, database),
        None if database => ply_host::registry_with_database(),
        None => ply_host::registry(),
    }
}

/// What a run has bound, and what it *could* have bound.
pub struct Hosts {
    /// The facilities the bound handlers act on; the source of every [`HostRuntime`].
    host: Option<Arc<ply_host::Host>>,
    /// One binding serves the whole run.
    binding: Arc<HostBinding>,
    /// Every triple the registry resolves against this program, whether or not it is bound.
    listing: HostListing,
    db: Option<DbConfig>,
    /// Read by the `configuration` block, the banner and the digest; never re-derived from flags.
    config: Configuration,
    /// The `--db-schema` function, resolved against the program at start-up.
    schema: Option<db::schema::SchemaView>,
    /// Where this run's records go and on which channels, when the program records at all.
    observability: Option<Observability>,
    shutdown: Option<Shutdown>,
}

impl Hosts {
    /// The binding a run gets; `reach` is the row it enters, or `None` to let the binding decide.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        check: &CheckOutput,
        host: bool,
        credentials: &[tls::CredentialSpec],
        roots: &[ply_host::fs::RootSpec],
        db: Option<DbConfig>,
        config: Configuration,
        trace: &crate::trace::TraceOptions,
        reach: Option<&Footprint>,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        Hosts::open_stopping(
            check,
            host,
            credentials,
            roots,
            db,
            config,
            trace,
            reach,
            None,
        )
    }

    /// [`Hosts::open`] for a run that listens for a stop; only `ply run`, so ctrl-C ends no test.
    #[allow(clippy::too_many_arguments)]
    pub fn open_stopping(
        check: &CheckOutput,
        host: bool,
        credentials: &[tls::CredentialSpec],
        roots: &[ply_host::fs::RootSpec],
        db: Option<DbConfig>,
        config: Configuration,
        trace: &crate::trace::TraceOptions,
        reach: Option<&Footprint>,
        shutdown: Option<Arc<ply_host::signal::Shutdown>>,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        if !host {
            let registry = registry_for(check, None);
            return Ok(Hosts {
                host: None,
                binding: Arc::new(HostBinding::hermetic_with(registry)),
                listing: HostListing::default(),
                db: None,
                config: Configuration::default(),
                schema: None,
                observability: None,
                shutdown: None,
            });
        }
        let material = tls::Credentials::load(credentials)?;
        // Loaded up front so a bad root is `E0454` before anything runs.
        let roots = ply_host::fs::Roots::load(roots, Span::DUMMY).map_err(|d| vec![d])?;
        // Opened only when a `db` operation can reach it, and probed now so an unreachable database
        // fails start-up rather than the first request.
        let facilities = Arc::new(
            match db.as_ref().filter(|_| reaches_db(check, reach)) {
                Some(config) => {
                    let (url, bounds) = config.pool_config();
                    ply_host::Host::with_database(
                        material,
                        ply_host::db::PoolConfig {
                            url: url.expose().to_string(),
                            size: bounds.size,
                            acquire: bounds.acquire,
                            statement: bounds.statement,
                            idle_txn: bounds.idle_txn,
                            connect: bounds.connect,
                            statements: bounds.statements,
                        },
                    )
                    .map_err(|d| vec![d])?
                }
                None => ply_host::Host::with_credentials(material),
            }
            .configured(Arc::clone(&config.snapshot))
            .rooted(roots)
            .traced(trace.open()),
        );
        let facilities = match shutdown {
            Some(shutdown) => Arc::new(
                Arc::try_unwrap(facilities)
                    .unwrap_or_else(|_| unreachable!("the only `Arc` was just built"))
                    .stopping_on(shutdown),
            ),
            None => facilities,
        };
        let registry = facilities.registry();
        let binding = registry.bind(check)?;
        let listing = binding.listing().clone();
        let schema = db_schema(check, db.as_ref(), &listing, reach)?;
        let observability = Observability::of(&listing, facilities.tracing(), trace.level_name());
        let stopping = facilities
            .stop()
            .and_then(|shutdown| Shutdown::of(&listing, shutdown));
        Ok(Hosts {
            host: Some(facilities),
            binding: Arc::new(binding),
            listing,
            db,
            config,
            schema,
            observability,
            shutdown: stopping,
        })
    }

    /// The run's resolved configuration.
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }

    /// What the sink saw, for the line a stopping service prints.
    pub fn trace_counts(&self) -> Option<ply_host::trace::Counts> {
        self.host.as_ref().map(|host| host.tracing().counts())
    }

    /// The configuration a run was given, for the driver and for the report.
    pub fn db(&self) -> Option<&DbConfig> {
        self.db.as_ref()
    }

    /// The `database` block, or `None` when no database is in reach.
    pub fn database(&self) -> Option<Database> {
        Database::of(
            Database::operations_of(&self.listing),
            self.db.clone(),
            // Server facts come only from a live connection.
            None,
            self.schema.clone(),
        )
    }

    /// Whether this run reached a real database, so a green suite is not read as hermetic.
    pub fn is_live_database(&self) -> bool {
        self.database().is_some_and(|d| d.is_live())
    }

    /// Fill in the `--db-schema` function's table and column counts.
    pub fn describe_schema(&mut self, shape: Option<db::schema::Shape>) {
        if let Some(view) = &mut self.schema {
            view.shape = shape;
        }
    }

    /// The name `--db-schema` resolved to, for a command that wants to evaluate it.
    pub fn schema_function(&self) -> Option<&str> {
        self.schema.as_ref().map(|view| view.name.as_str())
    }

    /// The TLS stack and configured credentials, or `None` when neither exists.
    pub fn transport(&self) -> Option<Transport> {
        Transport::of(&self.listing, self.host.as_ref().map(|h| h.credentials()))
    }

    /// Every block the rows cannot carry; printed and hashed together.
    pub fn disclosures(&self) -> Disclosures {
        Disclosures {
            transport: self.transport(),
            filesystem: Filesystem::of(&self.listing, self.host.as_ref().map(|h| h.roots())),
            database: self.database(),
            configuration: Some(self.config.clone()).filter(Configuration::is_opened),
            observability: self.observability.clone(),
            shutdown: self.shutdown,
        }
    }

    /// [`Hosts::open`] against an explicit registry, for tests.
    pub fn bind(
        registry: HostRegistry,
        check: &CheckOutput,
        host: bool,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        Hosts::bind_with(registry, check, host, None)
    }

    /// [`Hosts::bind`] with a database configuration, for tests of the checks `open` runs.
    pub fn bind_with(
        registry: HostRegistry,
        check: &CheckOutput,
        host: bool,
        db: Option<DbConfig>,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        if !host {
            return Ok(Hosts {
                host: None,
                binding: Arc::new(HostBinding::hermetic_with(registry)),
                listing: HostListing::default(),
                db: None,
                config: Configuration::default(),
                schema: None,
                observability: None,
                shutdown: None,
            });
        }
        let binding = registry.bind(check)?;
        let listing = binding.listing().clone();
        let schema = db_schema(check, db.as_ref(), &listing, None)?;
        Ok(Hosts {
            host: None,
            binding: Arc::new(binding),
            listing,
            db,
            config: Configuration::default(),
            schema,
            observability: None,
            shutdown: None,
        })
    }

    /// Everything the registry resolves to, bound or not: what `ply hosts` prints and digests.
    pub fn preview(
        check: &CheckOutput,
        trace: Option<Arc<ply_host::trace::Trace>>,
    ) -> Result<HostListing, Vec<Diagnostic>> {
        registry_for(check, trace).preview(check)
    }

    /// A reactor for one machine, on the thread that will drive it.
    pub fn runtime(&self) -> Option<Rc<dyn HostRuntime>> {
        self.host.as_ref().map(|host| host.runtime())
    }

    /// The same thing, as something a worker thread can call for itself.
    pub fn runtime_factory(&self) -> Option<impl Fn() -> Rc<dyn HostRuntime> + Sync + use<>> {
        self.host
            .as_ref()
            .map(Arc::clone)
            .map(|host| move || host.runtime())
    }

    pub fn binding(&self) -> Arc<HostBinding> {
        Arc::clone(&self.binding)
    }

    pub fn listing(&self) -> &HostListing {
        &self.listing
    }

    pub fn is_hermetic(&self) -> bool {
        self.binding.is_hermetic()
    }

    pub fn label(&self) -> &'static str {
        if self.is_hermetic() {
            "hermetic"
        } else {
            "host"
        }
    }

    pub fn reaches(&self, footprint: &Footprint) -> bool {
        self.binding.reaches(footprint)
    }

    pub fn summary_json(&self) -> Value {
        let disclosures = self.disclosures();
        let mut summary = json!({
            "handlers": self.listing.handlers,
            "operations": self.listing.rows.len(),
            "digest": digest_short(&self.listing, &disclosures),
        });
        if let Some(transport) = &disclosures.transport {
            summary["transport"] = transport.json();
            // Counts, not diagnostics: a client speaking no TLS is attributable to no definition.
            summary["handshakes"] = handshakes_json(&self.handshakes());
        }
        if let Some(filesystem) = &disclosures.filesystem {
            summary["filesystem"] = filesystem.json();
        }
        if let Some(database) = &disclosures.database {
            summary["database"] = database.json();
        }
        summary
    }

    /// Handshakes this run completed and refused, with the reasons.
    pub fn handshakes(&self) -> tls::HandshakeCounts {
        self.host
            .as_ref()
            .map(|h| h.handshakes())
            .unwrap_or_default()
    }
}

/// Whether a `db` operation can reach the host boundary in this run.
fn reaches_db(check: &CheckOutput, reach: Option<&Footprint>) -> bool {
    let declared = check
        .effects
        .values()
        .any(|e| e.name.as_str() == ply_host::db::EFFECT);
    declared
        && reach.is_none_or(|reach| {
            reach
                .atoms()
                .any(|a| a.effect.as_str() == ply_host::db::EFFECT)
        })
}

/// The three checks that stand between a binding and the first evaluation, and the `--db-schema`
/// view they leave behind.
fn db_schema(
    check: &CheckOutput,
    config: Option<&DbConfig>,
    listing: &HostListing,
    reach: Option<&Footprint>,
) -> Result<Option<db::schema::SchemaView>, Vec<Diagnostic>> {
    if let Some(defect) = Database::rollback_bound(listing) {
        return Err(vec![defect]);
    }
    let operations = Database::operations_of(listing);
    let Some(config) = config else {
        // The binding lists what the program can reach, not what this run enters.
        let reached: Vec<String> = match reach {
            Some(reach) => reach
                .atoms()
                .filter(|a| a.effect.as_str() == ply_host::db::EFFECT)
                .map(|a| a.to_string())
                .collect(),
            None => operations.clone(),
        };
        if reached.is_empty() {
            return Ok(None);
        }
        return Err(vec![db::missing(&reached)]);
    };
    let Some(name) = &config.schema else {
        return Ok(None);
    };
    let resolved = db::schema::resolve(check, name).map_err(|d| vec![d])?;
    Ok(Some(db::schema::SchemaView {
        name: resolved.as_str().to_string(),
        shape: None,
        state: db::schema::State::Declared,
    }))
}

pub fn handshake_lines(counts: &tls::HandshakeCounts) -> Vec<String> {
    if counts.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "handshakes: {} completed, {} refused",
        counts.completed, counts.refused
    )];
    for (reason, n) in &counts.reasons {
        lines.push(format!("  {n} {reason}"));
    }
    lines
}

/// The one line that says a run reached a real database.
pub fn database_line(hosts: &Hosts) -> Option<String> {
    let database = hosts.database()?;
    if !database.is_live() {
        return None;
    }
    let config = database.config.as_ref()?;
    Some(format!(
        "database {} · {} {} · configured by {}",
        config.url.redacted(),
        database.operations.len(),
        plural(database.operations.len(), "operation"),
        config.source.as_str(),
    ))
}

pub fn handshakes_json(counts: &tls::HandshakeCounts) -> Value {
    json!({
        "completed": counts.completed,
        "refused": counts.refused,
        "reasons": counts.reasons.iter().map(|(reason, n)| json!({
            "reason": reason,
            "count": n,
        })).collect::<Vec<_>>(),
    })
}

/// What the test runner is told it may reach.
pub fn hosting<'a, F>(hosts: &Hosts, runtime: &'a Option<F>) -> ply_test::Hosting<'a>
where
    F: Fn() -> Rc<dyn HostRuntime> + Sync,
{
    let hosting = ply_test::Hosting::hermetic().with_binding(hosts.binding());
    match runtime {
        Some(factory) => hosting.with_runtime(factory),
        None => hosting,
    }
}

/// How the corpus splits once the binding is taken into account.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counts {
    pub total: usize,
    pub isolated: usize,
    pub shared: usize,
    pub host: usize,
}

impl Counts {
    /// `tests` pairs each footprint with whether it classified as region-isolated.
    pub fn of<'a>(hosts: &Hosts, tests: impl IntoIterator<Item = (&'a Footprint, bool)>) -> Counts {
        let mut counts = Counts::default();
        for (footprint, isolated) in tests {
            counts.total += 1;
            if hosts.reaches(footprint) {
                counts.host += 1;
            } else if isolated {
                counts.isolated += 1;
            } else {
                counts.shared += 1;
            }
        }
        counts
    }
}

/// The roots the run bound: what each `fs` label in a row actually names.
pub struct Filesystem {
    /// By name, ascending. Empty is reported, since an `fs` operation with no root is `E0451`.
    pub roots: Vec<RootView>,
}

pub struct RootView {
    pub name: String,
    /// Canonical, since that (not the path as written) is what confines the run.
    pub path: String,
}

impl Filesystem {
    /// `Some` when the program can perform an `fs` operation or the run bound a root.
    pub fn of(listing: &HostListing, roots: Option<&ply_host::fs::Roots>) -> Option<Filesystem> {
        let reachable = listing
            .rows
            .iter()
            .any(|row| row.path.starts_with("ply_host::fs::"));
        let configured = roots.is_some_and(|r| !r.is_empty());
        if !reachable && !configured {
            return None;
        }
        Some(Filesystem {
            roots: roots
                .into_iter()
                .flat_map(|r| r.listing())
                .map(|(name, path)| RootView {
                    name: name.to_string(),
                    path: path.display().to_string(),
                })
                .collect(),
        })
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![String::new(), "filesystem".to_string()];
        if self.roots.is_empty() {
            lines.push(
                "none — an `fs` operation is E0451 until `--fs NAME=PATH` binds its label"
                    .to_string(),
            );
            return lines;
        }
        let width = self
            .roots
            .iter()
            .map(|r| r.name.chars().count())
            .max()
            .unwrap_or(0);
        for root in &self.roots {
            lines.push(format!("{:width$}  {}", root.name, root.path));
        }
        lines
    }

    pub fn json(&self) -> Value {
        json!({
            "roots": self.roots.iter().map(|r| json!({
                "name": r.name,
                "path": r.path,
            })).collect::<Vec<_>>(),
        })
    }

    /// Names only, since paths differ per machine; repointing a root changes the listing, not this.
    fn hash_into(&self, hasher: &mut blake3::Hasher) {
        hasher.update(FILESYSTEM_DOMAIN);
        for root in &self.roots {
            hasher.update(root.name.as_bytes());
            hasher.update(b"\0");
        }
    }
}

/// The TLS stack in the trusted computing base, and the credentials the run was configured with.
pub struct Transport {
    pub library: &'static str,
    pub version: &'static str,
    pub provider: &'static str,
    pub versions: &'static [&'static str],
    pub alpn: &'static [&'static str],
    /// By name, ascending.
    pub credentials: Vec<CredentialView>,
}

pub struct CredentialView {
    pub name: String,
    pub fingerprint: String,
    pub certificates: usize,
}

impl Transport {
    /// `Some` when the program can create a TLS listener or the run was given credentials.
    pub fn of(listing: &HostListing, credentials: Option<&tls::Credentials>) -> Option<Transport> {
        let reachable = listing.rows.iter().any(|row| row.path == tls::HANDLER);
        let configured = credentials.is_some_and(|c| !c.is_empty());
        if !reachable && !configured {
            return None;
        }
        Some(Transport {
            library: tls::LIBRARY,
            version: tls::VERSION,
            provider: tls::PROVIDER,
            versions: &tls::VERSIONS,
            alpn: &tls::ALPN,
            credentials: credentials
                .into_iter()
                .flat_map(|c| c.iter())
                .map(|(name, credential)| CredentialView {
                    name: name.to_string(),
                    fingerprint: credential.fingerprint().to_string(),
                    certificates: credential.certificates(),
                })
                .collect(),
        })
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            String::new(),
            "transport".to_string(),
            format!(
                "tls  {} {} · provider {} · {} · alpn {}",
                self.library,
                self.version,
                self.provider,
                self.versions.join(", "),
                self.alpn.join(", "),
            ),
            String::new(),
            "credentials".to_string(),
        ];
        if self.credentials.is_empty() {
            lines.push(
                "none — `net.listen_tls` is E0429 until `--tls NAME=CERT,KEY` names one"
                    .to_string(),
            );
            return lines;
        }
        let width = self
            .credentials
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(0);
        for credential in &self.credentials {
            lines.push(format!(
                "{:width$}  {}  {} {}",
                credential.name,
                abbreviate(&credential.fingerprint),
                credential.certificates,
                plural(credential.certificates, "certificate"),
            ));
        }
        lines
    }

    pub fn json(&self) -> Value {
        json!({
            "library": self.library,
            "version": self.version,
            "provider": self.provider,
            "versions": self.versions,
            "alpn": self.alpn,
            "credentials": self.credentials.iter().map(|c| json!({
                "name": c.name,
                "fingerprint": c.fingerprint,
                "certificates": c.certificates,
            })).collect::<Vec<_>>(),
        })
    }

    /// What the digest covers: the credential *names*, the provider and the library version.
    fn hash_into(&self, hasher: &mut blake3::Hasher) {
        for text in [self.library, self.version, self.provider] {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        hasher.update(&(self.credentials.len() as u64).to_le_bytes());
        for credential in &self.credentials {
            hasher.update(&(credential.name.len() as u64).to_le_bytes());
            hasher.update(credential.name.as_bytes());
        }
    }
}

/// Where a run's records go and on which channels.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Observability {
    sink: &'static str,
    destination: &'static str,
    /// `None` for `ply_host::trace::discard`.
    level: Option<&'static str>,
    channels: Vec<String>,
}

impl Observability {
    /// `None` for a program that never mentions `std.trace`.
    fn of(
        listing: &HostListing,
        trace: &Arc<ply_host::trace::Trace>,
        level: &'static str,
    ) -> Option<Observability> {
        let mut channels: Vec<String> = listing
            .rows
            .iter()
            .filter(|row| row.effect.as_str() == ply_host::trace::EFFECT)
            .filter_map(|row| match &row.resource {
                ply_ty::ty::Resource::Named(name) => Some(name.as_str().to_string()),
                ply_ty::ty::Resource::Singleton => None,
            })
            .collect();
        channels.sort();
        channels.dedup();
        if channels.is_empty() {
            return None;
        }
        let sink = trace.sink_path();
        Some(Observability {
            sink,
            destination: trace.sink_destination(),
            level: (sink != ply_host::trace::DISCARD_PATH).then_some(level),
            channels,
        })
    }

    fn lines(&self) -> Vec<String> {
        vec![
            format!(
                "sink       {} → {}{}",
                self.sink,
                self.destination,
                self.level_suffix()
            ),
            format!("channels   {}", self.channels.join(" ")),
            "spans      per-task stack · closed at end_entry_point".to_string(),
        ]
    }

    /// The same three facts on one line, for the start-up banner.
    pub fn banner(&self) -> String {
        format!(
            "{} → {}{} · channels {}",
            self.sink,
            self.destination,
            self.level_suffix(),
            self.channels.join(" "),
        )
    }

    fn level_suffix(&self) -> String {
        match self.level {
            Some(level) => format!(" · level {level}"),
            None => String::new(),
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "sink": self.sink,
            "destination": self.destination,
            "level": self.level,
            "channels": self.channels,
        })
    }

    /// The sink's path, its level and the channel list.
    fn hash_into(&self, hasher: &mut blake3::Hasher) {
        for text in [self.sink, self.destination, self.level.unwrap_or("")] {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        hasher.update(&(self.channels.len() as u64).to_le_bytes());
        for channel in &self.channels {
            hasher.update(&(channel.len() as u64).to_le_bytes());
            hasher.update(channel.as_bytes());
        }
    }
}

/// What a `SIGINT` or a `SIGTERM` does to this run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shutdown {
    /// `SIGTERM` does not exist on Windows, so the signals listened for are printed.
    signals: [Option<&'static str>; 2],
    lead_ms: u128,
    drain_ms: u128,
}

impl Shutdown {
    /// `None` for a program that never mentions `std.signal`.
    fn of(listing: &HostListing, shutdown: &Arc<ply_host::signal::Shutdown>) -> Option<Shutdown> {
        if !listing
            .rows
            .iter()
            .any(|row| row.effect.as_str() == ply_host::signal::EFFECT)
        {
            return None;
        }
        let mut signals = [None, None];
        for (slot, signal) in signals.iter_mut().zip(shutdown.signals()) {
            *slot = Some(signal.name());
        }
        let bounds = shutdown.bounds();
        Some(Shutdown {
            signals,
            lead_ms: bounds.lead.as_millis(),
            drain_ms: bounds.drain.as_millis(),
        })
    }

    fn names(&self) -> Vec<&'static str> {
        self.signals.iter().flatten().copied().collect()
    }

    fn lines(&self) -> Vec<String> {
        vec![format!(
            "signals    {} · lead {}ms · drain {}ms · second signal exits 130/143",
            self.names().join(" "),
            self.lead_ms,
            self.drain_ms,
        )]
    }

    pub fn json(&self) -> Value {
        json!({
            "signals": self.names(),
            "lead_ms": self.lead_ms,
            "drain_ms": self.drain_ms,
        })
    }

    fn hash_into(&self, hasher: &mut blake3::Hasher) {
        for name in self.names() {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
        }
        hasher.update(&(self.lead_ms as u64).to_le_bytes());
        hasher.update(&(self.drain_ms as u64).to_le_bytes());
    }
}

/// The blocks `ply hosts` prints under the table: facts no row can carry.
#[derive(Default)]
pub struct Disclosures {
    pub transport: Option<Transport>,
    /// `None` when no root is bound and the program performs no `fs` operation.
    pub filesystem: Option<Filesystem>,
    pub database: Option<Database>,
    /// The run's configuration, when it opened any source.
    pub configuration: Option<Configuration>,
    pub observability: Option<Observability>,
    pub shutdown: Option<Shutdown>,
}

impl Disclosures {
    /// For `ply hosts`, which resolves the listing without binding a [`Hosts`].
    #[allow(clippy::too_many_arguments)]
    pub fn of(
        listing: &HostListing,
        credentials: Option<&tls::Credentials>,
        roots: Option<&ply_host::fs::Roots>,
        db: Option<DbConfig>,
        schema: Option<db::schema::SchemaView>,
        configuration: Option<Configuration>,
        trace: Option<&Arc<ply_host::trace::Trace>>,
        level: &'static str,
        shutdown: Option<&Arc<ply_host::signal::Shutdown>>,
    ) -> Disclosures {
        Disclosures {
            transport: Transport::of(listing, credentials),
            filesystem: Filesystem::of(listing, roots),
            database: Database::of(Database::operations_of(listing), db, None, schema),
            configuration: configuration.filter(Configuration::is_opened),
            observability: trace.and_then(|trace| Observability::of(listing, trace, level)),
            shutdown: shutdown.and_then(|shutdown| Shutdown::of(listing, shutdown)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.transport.is_none()
            && self.filesystem.is_none()
            && self.database.is_none()
            && self.observability.is_none()
            && self.shutdown.is_none()
            && !self
                .configuration
                .as_ref()
                .is_some_and(Configuration::is_pinned)
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(transport) = &self.transport {
            lines.extend(transport.lines());
        }
        if let Some(filesystem) = &self.filesystem {
            lines.extend(filesystem.lines());
        }
        if let Some(database) = &self.database {
            lines.extend(database.lines());
        }
        if let Some(configuration) = &self.configuration {
            lines.push(String::new());
            lines.push("configuration".to_string());
            lines.extend(configuration.lines());
        }
        if let Some(observability) = &self.observability {
            lines.push(String::new());
            lines.push("observability".to_string());
            lines.extend(observability.lines());
        }
        if let Some(shutdown) = &self.shutdown {
            lines.push(String::new());
            lines.push("shutdown".to_string());
            lines.extend(shutdown.lines());
        }
        lines
    }
}

/// The one line a CI check pins against the trusted computing base.
pub fn digest_short(listing: &HostListing, disclosures: &Disclosures) -> String {
    if disclosures.is_empty() {
        return listing.digest_short();
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(DISCLOSURE_DOMAIN);
    hasher.update(&listing.digest());
    if let Some(transport) = &disclosures.transport {
        transport.hash_into(&mut hasher);
    }
    if let Some(filesystem) = &disclosures.filesystem {
        filesystem.hash_into(&mut hasher);
    }
    if let Some(database) = &disclosures.database {
        hasher.update(DATABASE_DOMAIN);
        database.hash_into(&mut hasher);
    }
    // Names and shapes only, never resolved values: a deployment's own settings must not move it.
    if let Some(configuration) = disclosures.configuration.as_ref().filter(|c| c.is_pinned()) {
        hasher.update(CONFIGURATION_DOMAIN);
        configuration.digest_into(&mut |text| {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        });
    }
    if let Some(observability) = &disclosures.observability {
        hasher.update(OBSERVABILITY_DOMAIN);
        observability.hash_into(&mut hasher);
    }
    if let Some(shutdown) = &disclosures.shutdown {
        hasher.update(SHUTDOWN_DOMAIN);
        shutdown.hash_into(&mut hasher);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(15);
    out.push_str("b3:");
    for byte in &digest.as_bytes()[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Domain-separated so a listing with an empty disclosure cannot collide with one that has none.
const DISCLOSURE_DOMAIN: &[u8] = b"ply.hosts.transport.v1\0";

// One domain per block, so listings with different blocks cannot collide.
const FILESYSTEM_DOMAIN: &[u8] = b"ply.hosts.filesystem.v1\0";
const DATABASE_DOMAIN: &[u8] = b"ply.hosts.database.v1\0";
const CONFIGURATION_DOMAIN: &[u8] = b"ply.hosts.configuration.v1\0";
const OBSERVABILITY_DOMAIN: &[u8] = b"ply.hosts.observability.v1\0";
const SHUTDOWN_DOMAIN: &[u8] = b"ply.hosts.shutdown.v1\0";

/// A fingerprint short enough to sit in the table beside the name it belongs to.
fn abbreviate(fingerprint: &str) -> String {
    let (scheme, digits) = fingerprint.split_once(':').unwrap_or(("", fingerprint));
    let short: String = digits.chars().take(12).collect();
    let elided = if digits.chars().count() > 12 {
        "…"
    } else {
        ""
    };
    if scheme.is_empty() {
        format!("{short}{elided}")
    } else {
        format!("{scheme}:{short}{elided}")
    }
}

/// The operation says what was bound; the atom is what scheduling and isolation speak in.
const HEADERS: [&str; 7] = [
    "OPERATION",
    "ATOM",
    "HANDLER",
    "DET",
    "LINEAR",
    "BLOCKING",
    "SECRETS",
];

fn cells(row: &HostRow) -> [String; 7] {
    [
        row.to_string(),
        row.atom.to_string(),
        row.path.to_string(),
        yes_no(row.deterministic),
        row.linearity.as_str().to_string(),
        yes_no(row.blocking),
        yes_no(row.secrets),
    ]
}

fn yes_no(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_string()
}

/// Every line of `ply hosts --host`, unindented.
pub fn listing_lines(listing: &HostListing, disclosures: &Disclosures) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {} · {} {} · trusted computing base",
        listing.handlers,
        plural(listing.handlers, "host handler"),
        listing.rows.len(),
        plural(listing.rows.len(), "operation"),
    )];
    lines.push(String::new());

    if listing.rows.is_empty() {
        lines.push(empty_note(listing));
    } else {
        let rows: Vec<[String; 7]> = listing.rows.iter().map(cells).collect();
        let mut widths = HEADERS.map(str::len);
        for row in &rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let line = |cells: &[String; 7]| {
            let mut out = String::new();
            for (i, (cell, width)) in cells.iter().zip(widths).enumerate() {
                if i + 1 == cells.len() {
                    out.push_str(cell);
                } else {
                    out.push_str(&format!("{cell:<width$}  "));
                }
            }
            out
        };
        lines.push(line(&HEADERS.map(str::to_string)));
        lines.extend(rows.iter().map(line));
    }

    lines.extend(disclosures.lines());

    lines.push(String::new());
    lines.push(format!("digest: {}", digest_short(listing, disclosures)));
    lines
}

/// Why a bound listing has no rows.
fn empty_note(listing: &HostListing) -> String {
    if listing.handlers == 0 {
        "no host handler is compiled into this binary".to_string()
    } else {
        format!(
            "{} {} registered, and none serves an atom this program performs",
            listing.handlers,
            plural(listing.handlers, "handler")
        )
    }
}

/// What `ply hosts` says without `--host`.
pub fn hermetic_lines(listing: &HostListing) -> Vec<String> {
    let mut lines = vec![
        "hermetic — no host handler is bound".to_string(),
        String::new(),
    ];
    lines.push(if listing.rows.is_empty() {
        empty_note(listing)
    } else {
        format!(
            "{} {} would bind under `--host`; run `ply hosts --host` to list them",
            listing.rows.len(),
            plural(listing.rows.len(), "operation"),
        )
    });
    lines
}

pub fn row_json(row: &HostRow) -> Value {
    json!({
        "effect": row.effect.as_str(),
        "operation": row.op.as_str(),
        // Null for an operation declared without `[r]`: a singleton, not a resource named that.
        "resource": match &row.resource {
            ply_ty::ty::Resource::Named(name) => json!(name.as_str()),
            ply_ty::ty::Resource::Singleton => Value::Null,
        },
        "triple": row.to_string(),
        "atom": row.atom.to_string(),
        "handler": row.path,
        "deterministic": row.deterministic,
        "linearity": row.linearity.as_json(),
        "blocking": row.blocking,
        // Whether this operation may be handed a value containing a `Secret`.
        "secrets": row.secrets,
        // The other half of the pair E0423 checks.
        "declared_nondet": row.declared_nondet,
    })
}

pub fn rows_json(listing: &HostListing) -> Value {
    Value::Array(listing.rows.iter().map(row_json).collect())
}
