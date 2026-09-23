//! The trusted computing base, as the CLI reads and reports it.

use crate::config::Configuration;
use crate::db::{self, Database, DbConfig};
use crate::payload::{count, diags_value, option, places_value, record, strings};
use crate::support::plural;
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostListing, HostOp, HostRegistry,
    HostRequest, HostResource, HostRow, HostRuntime, Linearity,
};
use ply_host::tls;
use ply_span::{Diagnostic, SourceMap, Span, Symbol};
use ply_ty::CheckOutput;
use ply_ty::ty::Footprint;
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::Arc;

/// The trusted computing base this binary was built with.
pub fn registry() -> HostRegistry {
    ply_host::registry()
}

/// One host operation and the handler that serves it, as a caller lends it to an entry.
pub type Lent = (HostOp, Arc<dyn HostHandler>);

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
        credentials: &crate::options::TlsOptions,
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
            None,
            Vec::new(),
        )
    }

    /// [`Hosts::open`] for a run that listens for a stop and is a process; only `ply run`, so
    /// ctrl-C ends no test and `process` is withheld from one. `lent` joins the registrations
    /// this binary compiles in, for an entry whose caller serves an effect of its own.
    #[allow(clippy::too_many_arguments)]
    pub fn open_stopping(
        check: &CheckOutput,
        host: bool,
        credentials: &crate::options::TlsOptions,
        roots: &[ply_host::fs::RootSpec],
        db: Option<DbConfig>,
        config: Configuration,
        trace: &crate::trace::TraceOptions,
        reach: Option<&Footprint>,
        shutdown: Option<Arc<ply_host::signal::Shutdown>>,
        process: Option<ply_host::process::ProcessHost>,
        lent: Vec<Lent>,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        if !host {
            let mut registry = registry_for(check, None);
            for (op, handler) in lent {
                registry.register(op, handler);
            }
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
        let material = tls::Credentials::load(&credentials.tls, &credentials.trust)?;
        // Loaded up front so a bad root is `E0454` before anything runs.
        let roots = ply_host::fs::Roots::load(roots, Span::DUMMY).map_err(|d| vec![d])?;
        // Opened only when a `db` operation can reach it, and probed now so an unreachable database
        // fails start-up rather than the first request.
        let mut facilities = match db.as_ref().filter(|_| reaches_db(check, reach)) {
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
        .traced(trace.open());
        if let Some(process) = process {
            facilities = facilities.with_process(process);
        }
        if let Some(shutdown) = shutdown {
            facilities = facilities.stopping_on(shutdown);
        }
        let facilities = Arc::new(facilities);
        let mut registry = facilities.registry();
        for (op, handler) in lent {
            registry.register(op, handler);
        }
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

    /// The code `process.exit` asked for, once the run is over.
    pub fn requested_exit(&self) -> Option<i32> {
        self.host.as_ref()?.process()?.requested_exit()
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
    pub roots: &'static str,
    pub roots_version: &'static str,
    /// `--trust` certificates joining the roots.
    pub trusted: usize,
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
        let reachable = listing
            .rows
            .iter()
            .any(|row| row.path == tls::HANDLER || row.path == tls::CONNECT_HANDLER);
        let configured = credentials.is_some_and(|c| !c.is_empty() || c.trusted() > 0);
        if !reachable && !configured {
            return None;
        }
        Some(Transport {
            library: tls::LIBRARY,
            version: tls::VERSION,
            provider: tls::PROVIDER,
            versions: &tls::VERSIONS,
            alpn: &tls::ALPN,
            roots: tls::ROOTS,
            roots_version: tls::ROOTS_VERSION,
            trusted: credentials.map_or(0, |c| c.trusted()),
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

    pub fn json(&self) -> Value {
        json!({
            "library": self.library,
            "version": self.version,
            "provider": self.provider,
            "versions": self.versions,
            "alpn": self.alpn,
            "roots": { "library": self.roots, "version": self.roots_version, "trusted": self.trusted },
            "credentials": self.credentials.iter().map(|c| json!({
                "name": c.name,
                "fingerprint": c.fingerprint,
                "certificates": c.certificates,
            })).collect::<Vec<_>>(),
        })
    }

    /// What the digest covers: the credential *names*, the provider and the library version.
    fn hash_into(&self, hasher: &mut blake3::Hasher) {
        for text in [
            self.library,
            self.version,
            self.provider,
            self.roots,
            self.roots_version,
        ] {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        hasher.update(&(self.trusted as u64).to_le_bytes());
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
                // A host row names a resource or none; nothing holds a label a caller fills.
                ply_ty::ty::Resource::Var(_) | ply_ty::ty::Resource::Singleton => None,
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
    fn of(
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

// --- What `ply hosts` is lent ------------------------------------------------

/// The effect `crates/ply-cli/ply/hosts.ply` declares. It is lent to that one entry and nowhere
/// else: what a run would bind is assembled here, and the program is handed what it says.
const EFFECT: &str = "tcb";

const PREVIEW: &str = "ply_cli::hosts::preview";

const OPEN: &str = "ply_cli::hosts::open";

/// The module the payload's constructors are declared in, as a program-wide name.
const PAYLOAD: &str = "hosts";

/// Assembled before the program is entered: a load and a backend are the compiler's work, and
/// the compiler is not something to re-enter from inside a running program.
/// What `ply hosts` is configured with, as plain data: the shell's parsed flags convert into
/// this.
#[derive(Clone, Debug)]
pub struct HostsOptions {
    pub path: std::path::PathBuf,
    pub host: bool,
    pub json: bool,
    pub digest: bool,
    pub tls: crate::options::TlsOptions,
    pub fs: Vec<ply_host::fs::RootSpec>,
    pub db: crate::db::DbOptions,
    pub config: crate::config::ConfigOptions,
    pub trace: crate::trace::TraceOptions,
    pub shutdown: crate::options::ShutdownOptions,
}

pub fn lent(args: &crate::hosts::HostsOptions) -> Vec<Lent> {
    let facility: Arc<dyn HostHandler> = Arc::new(Facility {
        assembled: Assembled::of(args),
    });
    vec![
        (registration("preview", PREVIEW), Arc::clone(&facility)),
        (registration("open", OPEN), facility),
    ]
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // The flags, the tree and the process environment are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // One binding serves the whole command, so a second perform reads the same one.
        linearity: Linearity::Repeatable,
        blocking: false,
        secrets: false,
        path,
    }
}

struct Facility {
    assembled: Assembled,
}

impl HostHandler for Facility {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let value = match req.op.op.as_str() {
            "preview" => self.assembled.preview(),
            "open" => self.assembled.binding(),
            other => return Err(unregistered(other, req.span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

/// The binding this invocation's flags define, resolved once and then only read.
struct Assembled {
    /// The `Stage` constructor the program matches on, by simple name.
    stage: &'static str,
    root: String,
    listing: HostListing,
    disclosures: Disclosures,
    digest: String,
    hermetic: bool,
    /// The refusal that stopped it, else the warnings the configuration raised.
    diagnostics: Vec<Diagnostic>,
    sources: SourceMap,
}

impl Assembled {
    fn of(args: &crate::hosts::HostsOptions) -> Assembled {
        let loaded = match crate::load::load(&args.path) {
            Ok(loaded) => loaded,
            Err(err) => {
                return Assembled::refused(
                    "NotLoaded",
                    String::new(),
                    err.diagnostics,
                    err.sources,
                );
            }
        };
        let root = loaded.root.display().to_string();
        match bind(args, &loaded) {
            Ok(bound) => Assembled {
                stage: "Bound",
                root,
                digest: digest_short(&bound.listing, &bound.disclosures),
                hermetic: bound.hermetic,
                listing: bound.listing,
                disclosures: bound.disclosures,
                diagnostics: bound.warnings,
                sources: loaded.sources,
            },
            Err((stage, diagnostics)) => {
                Assembled::refused(stage, root, diagnostics, loaded.sources)
            }
        }
    }

    fn refused(
        stage: &'static str,
        root: String,
        diagnostics: Vec<Diagnostic>,
        sources: SourceMap,
    ) -> Assembled {
        Assembled {
            stage,
            root,
            listing: HostListing::default(),
            disclosures: Disclosures::default(),
            digest: String::new(),
            hermetic: true,
            diagnostics,
            sources,
        }
    }

    fn preview(&self) -> PlyValue {
        let d = &self.disclosures;
        record(vec![
            ("stage", PlyValue::ctor(payload(self.stage), Vec::new())),
            ("root", PlyValue::str(&self.root)),
            ("handlers", count(self.listing.handlers)),
            (
                "rows",
                PlyValue::list(self.listing.rows.iter().map(row_value).collect()),
            ),
            ("digest", PlyValue::str(&self.digest)),
            (
                "transport",
                option(d.transport.as_ref().map(transport_value)),
            ),
            (
                "filesystem",
                option(d.filesystem.as_ref().map(filesystem_value)),
            ),
            ("database", option(d.database.as_ref().map(database_value))),
            (
                "configuration",
                option(d.configuration.as_ref().map(configuration_value)),
            ),
            (
                "observability",
                option(d.observability.as_ref().map(observability_value)),
            ),
            ("shutdown", option(d.shutdown.as_ref().map(shutdown_value))),
            ("diags", diags_value(&self.diagnostics)),
            ("places", places_value(&self.sources)),
        ])
    }

    fn binding(&self) -> PlyValue {
        record(vec![
            (
                "label",
                PlyValue::str(if self.hermetic { "hermetic" } else { "host" }),
            ),
            ("hermetic", PlyValue::Bool(self.hermetic)),
        ])
    }
}

/// What the host flags open, over a program that loaded.
struct Bound {
    listing: HostListing,
    disclosures: Disclosures,
    hermetic: bool,
    warnings: Vec<Diagnostic>,
}

/// The stage that refused, and why.
type Refusal = (&'static str, Vec<Diagnostic>);

fn bind(args: &crate::hosts::HostsOptions, loaded: &crate::load::Loaded) -> Result<Bound, Refusal> {
    // Whether or not `--host` was passed: a digest that moved with a flag would pin nothing.
    let trace = args.trace.open();
    let stopping = ply_host::signal::Shutdown::new(args.shutdown.bounds());
    let registry = registry_for(&loaded.check, Some(Arc::clone(&trace)));
    let listing = registry
        .preview(&loaded.check)
        .map_err(|diagnostics| ("NotResolved", diagnostics))?;
    let binding = if args.host {
        registry
            .bind(&loaded.check)
            .map_err(|diagnostics| ("NotResolved", diagnostics))?
    } else {
        HostBinding::hermetic_with(registry)
    };
    // Loaded even without `--host`: this command answers what a run trusts, and whether it starts.
    let credentials = tls::Credentials::load(&args.tls.tls, &args.tls.trust)
        .map_err(|diagnostics| ("NotBound", diagnostics))?;
    // Likewise, so an unresolvable root is `E0454` before the listing overstates what is reached.
    let roots = ply_host::fs::Roots::load(&args.fs, Span::DUMMY)
        .map_err(|diagnostic| ("NotBound", vec![diagnostic]))?;
    let db = args
        .db
        .resolve(args.host)
        .map_err(|diagnostics| ("NotBound", diagnostics))?;
    // Built only for a schema: this command runs nothing else.
    let constant = |name: &str| {
        let backend = crate::support::prover_backend(None, loaded)?;
        crate::support::enter_constant(backend.map(|(provider, _)| provider), name)
    };
    let schema = schema_view(&loaded.check, db.as_ref(), &constant)
        .map_err(|diagnostic| ("NotBound", vec![diagnostic]))?;
    let (configuration, warnings) =
        Configuration::open(&loaded.check, args.host, &args.config, &constant)
            .map_err(|diagnostics| ("NotBound", diagnostics))?;
    Ok(Bound {
        disclosures: Disclosures::of(
            &listing,
            Some(&credentials),
            Some(&roots),
            db,
            schema,
            Some(configuration),
            Some(&trace),
            args.trace.level_name(),
            Some(&stopping),
        ),
        listing,
        hermetic: binding.is_hermetic(),
        warnings,
    })
}

/// The `--db-schema` function as this command reports it: named, and evaluated for its shape.
fn schema_view(
    check: &CheckOutput,
    db: Option<&DbConfig>,
    constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
) -> Result<Option<db::schema::SchemaView>, Diagnostic> {
    let Some(name) = db.and_then(|c| c.schema.as_deref()) else {
        return Ok(None);
    };
    let resolved = db::schema::resolve(check, name)?;
    let name = resolved.as_str().to_string();
    let shape = crate::support::materialise_schema(&name, constant);
    Ok(Some(db::schema::SchemaView {
        name,
        shape,
        state: db::schema::State::Declared,
    }))
}

// --- The payload -------------------------------------------------------------

fn payload(ctor: &str) -> Symbol {
    Symbol::new(format!("{PAYLOAD}.{ctor}"))
}

fn row_value(row: &HostRow) -> PlyValue {
    record(vec![
        ("effect", PlyValue::str(row.effect.as_str())),
        ("op", PlyValue::str(row.op.as_str())),
        (
            "resource",
            option(match &row.resource {
                ply_ty::ty::Resource::Named(name) => Some(PlyValue::str(name.as_str())),
                ply_ty::ty::Resource::Var(_) | ply_ty::ty::Resource::Singleton => None,
            }),
        ),
        ("triple", PlyValue::str(row.to_string())),
        ("handler", PlyValue::str(row.path)),
        ("deterministic", PlyValue::Bool(row.deterministic)),
        ("linear", PlyValue::Bool(row.linearity.is_linear())),
        ("blocking", PlyValue::Bool(row.blocking)),
        ("secrets", PlyValue::Bool(row.secrets)),
        ("declared_nondet", PlyValue::Bool(row.declared_nondet)),
    ])
}

fn transport_value(transport: &Transport) -> PlyValue {
    record(vec![
        ("library", PlyValue::str(transport.library)),
        ("version", PlyValue::str(transport.version)),
        ("provider", PlyValue::str(transport.provider)),
        ("versions", strings(transport.versions.iter().copied())),
        ("alpn", strings(transport.alpn.iter().copied())),
        ("roots", PlyValue::str(transport.roots)),
        ("roots_version", PlyValue::str(transport.roots_version)),
        ("trusted", count(transport.trusted)),
        (
            "credentials",
            PlyValue::list(
                transport
                    .credentials
                    .iter()
                    .map(|c| {
                        record(vec![
                            ("name", PlyValue::str(&c.name)),
                            ("fingerprint", PlyValue::str(&c.fingerprint)),
                            ("certificates", count(c.certificates)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn filesystem_value(filesystem: &Filesystem) -> PlyValue {
    record(vec![(
        "roots",
        PlyValue::list(
            filesystem
                .roots
                .iter()
                .map(|r| {
                    record(vec![
                        ("name", PlyValue::str(&r.name)),
                        ("path", PlyValue::str(&r.path)),
                    ])
                })
                .collect(),
        ),
    )])
}

fn database_value(database: &Database) -> PlyValue {
    let config = database.config.as_ref();
    record(vec![
        ("live", PlyValue::Bool(database.is_live())),
        (
            "operations",
            strings(database.operations.iter().map(String::as_str)),
        ),
        (
            "url",
            option(config.map(|c| PlyValue::str(c.url.redacted()))),
        ),
        (
            "source",
            option(config.map(|c| PlyValue::str(c.source.as_str()))),
        ),
        (
            "pool",
            option(config.map(|c| {
                record(vec![
                    ("connections", PlyValue::Int(c.pool as i64)),
                    ("acquire_ms", PlyValue::Int(c.acquire_ms as i64)),
                    ("statement_ms", PlyValue::Int(c.statement_ms as i64)),
                    ("idle_txn_ms", PlyValue::Int(c.idle_txn_ms as i64)),
                    ("connect_ms", PlyValue::Int(c.connect_ms as i64)),
                    ("statement_cache", PlyValue::Int(c.statement_cache as i64)),
                ])
            })),
        ),
        ("scanner", PlyValue::str(db::SCANNER)),
        ("accepts", strings(db::ACCEPTED.split_whitespace())),
        (
            "server",
            option(database.server.as_ref().map(|s| {
                record(vec![
                    ("version", PlyValue::str(&s.version)),
                    ("database", PlyValue::str(&s.database)),
                    ("collation", PlyValue::str(&s.collation)),
                    ("encoding", PlyValue::str(&s.encoding)),
                ])
            })),
        ),
        (
            "schema",
            option(database.schema.as_ref().map(|s| {
                record(vec![
                    ("function", PlyValue::str(&s.name)),
                    ("tables", option(s.shape.map(|shape| count(shape.tables)))),
                    ("columns", option(s.shape.map(|shape| count(shape.columns)))),
                    ("state", PlyValue::str(s.state.as_str())),
                ])
            })),
        ),
    ])
}

fn configuration_value(configuration: &Configuration) -> PlyValue {
    let snapshot = &configuration.snapshot;
    let counts = snapshot.counts();
    record(vec![
        ("sets", count(snapshot.sets)),
        (
            "files",
            PlyValue::list(
                snapshot
                    .files
                    .iter()
                    .map(|p| PlyValue::str(p.display().to_string()))
                    .collect(),
            ),
        ),
        ("environment", count(snapshot.environment)),
        ("defaults", count(counts.default)),
        (
            "schema",
            option(configuration.schema.as_ref().map(|view| {
                record(vec![
                    ("function", PlyValue::str(&view.name)),
                    (
                        "keys",
                        PlyValue::list(
                            view.keys
                                .iter()
                                .map(|(name, shape)| {
                                    record(vec![
                                        ("name", PlyValue::str(name)),
                                        ("shape", PlyValue::str(shape.as_str())),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })),
        ),
        ("resolved", count(counts.keys)),
        ("secret", count(counts.secret)),
        (
            "keys",
            PlyValue::list(
                snapshot
                    .declared()
                    .map(|(name, resolved)| {
                        record(vec![
                            ("name", PlyValue::str(name)),
                            ("value", PlyValue::str(resolved.shown())),
                            ("source", PlyValue::str(resolved.source.as_str())),
                            (
                                "secret",
                                PlyValue::Bool(
                                    resolved.shape == Some(ply_host::config::Shape::Secret),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn observability_value(observability: &Observability) -> PlyValue {
    record(vec![
        ("sink", PlyValue::str(observability.sink)),
        ("destination", PlyValue::str(observability.destination)),
        ("level", option(observability.level.map(PlyValue::str))),
        (
            "channels",
            strings(observability.channels.iter().map(String::as_str)),
        ),
    ])
}

fn shutdown_value(shutdown: &Shutdown) -> PlyValue {
    record(vec![
        ("signals", strings(shutdown.names())),
        ("lead_ms", PlyValue::Int(shutdown.lead_ms as i64)),
        ("drain_ms", PlyValue::Int(shutdown.drain_ms as i64)),
    ])
}

#[cold]
fn unregistered(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        ply_span::codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` reached the binding, and nothing here serves it"),
    )
    .primary(span, "this perform reached `ply hosts`")
    .note("the registrations and the handler are written together; this is Ply's fault")
}
