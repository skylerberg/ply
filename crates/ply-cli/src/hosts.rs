//! The trusted computing base, as the CLI reads and reports it.

use crate::commands::common::plural;
use crate::config::Configuration;
use crate::db::{self, Database, DbConfig};
use ply_core::CheckOutput;
use ply_core::ty::Footprint;
use ply_eval::host::{HostBinding, HostListing, HostRegistry, HostRow, HostRuntime};
use ply_host::tls;
use ply_span::Diagnostic;
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
    /// The facilities the bound handlers act on, and the source of every [`HostRuntime`] this run
    /// hands to a machine.
    host: Option<Arc<ply_host::Host>>,
    /// Shared rather than owned because the machine takes it by `Arc`: one binding serves the whole
    /// run, and a run with two would have two answers to what it can do.
    binding: Arc<HostBinding>,
    /// Every triple the registry resolves against this program, whether or not it is bound.
    listing: HostListing,
    db: Option<DbConfig>,
    /// What the run resolved its configuration to, held for the same reason `db` is: the
    /// `configuration` block, the start-up banner and the digest all read it, and none of them may
    /// re-derive it from the command line.
    config: Configuration,
    /// The `--db-schema` function, resolved against the program at start-up.
    schema: Option<db::schema::SchemaView>,
    /// Where this run's records go and on which channels, when the program records at all.
    observability: Option<Observability>,
    shutdown: Option<Shutdown>,
}

impl Hosts {
    pub fn open(
        check: &CheckOutput,
        host: bool,
        credentials: &[tls::CredentialSpec],
        db: Option<DbConfig>,
        config: Configuration,
        trace: &crate::trace::TraceOptions,
        reach: Option<&Footprint>,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        Hosts::open_stopping(check, host, credentials, db, config, trace, reach, None)
    }

    /// The same, for a run that listens for a stop.
    #[allow(clippy::too_many_arguments)]
    pub fn open_stopping(
        check: &CheckOutput,
        host: bool,
        credentials: &[tls::CredentialSpec],
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
        // Opened before the binding, and only when a `db` operation could actually reach it: the
        // pool is a thread and a set of sockets, and starting one for a program that never performs
        // a `db` operation would turn `--db` on an unrelated run into a connection failure.
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

    /// What the run resolved its configuration to: the `configuration` block, the banner's config
    /// line, and the digest's contribution.
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

    /// The `database` block, or `None` for a run with no database in reach — which is what keeps a
    /// W3 program's listing and digest what they were.
    pub fn database(&self) -> Option<Database> {
        Database::of(
            Database::operations_of(&self.listing),
            self.db.clone(),
            // The server's version, database name, collation and encoding come from a live
            // connection.
            None,
            self.schema.clone(),
        )
    }

    /// Whether this run reached a real database, which is the fact a report must carry so that a
    /// green suite is not read as a hermetic one.
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

    /// What the run has to say about TLS: the stack in the trusted computing base and the
    /// credentials it was configured with, or `None` when neither exists.
    pub fn transport(&self) -> Option<Transport> {
        Transport::of(&self.listing, self.host.as_ref().map(|h| h.credentials()))
    }

    /// Every block the rows cannot carry, together, because they are printed together and hashed
    /// together.
    pub fn disclosures(&self) -> Disclosures {
        Disclosures {
            transport: self.transport(),
            database: self.database(),
            configuration: Some(self.config.clone()).filter(Configuration::is_opened),
            observability: self.observability.clone(),
            shutdown: self.shutdown,
        }
    }

    /// [`open`] against an explicit registry, for a test that needs to control what is registered.
    #[cfg(test)]
    pub fn bind(
        registry: HostRegistry,
        check: &CheckOutput,
        host: bool,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        Hosts::bind_with(registry, check, host, None)
    }

    /// [`bind`], with a database configuration, for the tests that exercise the checks `open` runs
    /// between the binding and the first evaluation.
    #[cfg(test)]
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

    /// Everything the registry resolves to, bound or not: what `ply hosts` prints and what CI pins
    /// a digest of.
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

    /// `"hermetic"` or `"host"`.
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
            // Not a Ply diagnostic: a client that speaks no TLS is not the program's fault and is
            // attributable to no definition.
            summary["handshakes"] = handshakes_json(&self.handshakes());
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
        // The binding lists every `db` operation the *program* can reach, which is not the same
        // question.
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
    /// `tests` is every test the run reports on, paired with whether the raw classification called
    /// it region-isolated.
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

// --- transport --------------------------------------------------------------

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
    /// `Some` when this program can create a TLS listener, or when the run was configured with
    /// credentials.
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
    /// `None` for a program that never mentions `std.trace`, which is what keeps every W3 and W4
    /// corpus's block and digest where they were.
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
                ply_core::ty::Resource::Named(name) => Some(name.as_str().to_string()),
                ply_core::ty::Resource::Singleton => None,
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
    /// `SIGTERM` does not exist on Windows, so which signals a run listens for is a fact printed
    /// rather than a surprise a deployment discovers.
    signals: [Option<&'static str>; 2],
    lead_ms: u128,
    drain_ms: u128,
}

impl Shutdown {
    /// `None` for a program that never mentions `std.signal`, which is what keeps every W3 and W4
    /// corpus's block and digest where they were.
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

/// The blocks `ply hosts` prints under the table: facts about the trusted computing base that no
/// row can carry.
#[derive(Default)]
pub struct Disclosures {
    pub transport: Option<Transport>,
    pub database: Option<Database>,
    /// The run's configuration, when it opened any source.
    pub configuration: Option<Configuration>,
    pub observability: Option<Observability>,
    pub shutdown: Option<Shutdown>,
}

impl Disclosures {
    /// What a command that has no [`Hosts`] builds — `ply hosts` resolves the listing without
    /// binding, so it assembles these itself.
    #[allow(clippy::too_many_arguments)]
    pub fn of(
        listing: &HostListing,
        credentials: Option<&tls::Credentials>,
        db: Option<DbConfig>,
        schema: Option<db::schema::SchemaView>,
        configuration: Option<Configuration>,
        trace: Option<&Arc<ply_host::trace::Trace>>,
        level: &'static str,
        shutdown: Option<&Arc<ply_host::signal::Shutdown>>,
    ) -> Disclosures {
        Disclosures {
            transport: Transport::of(listing, credentials),
            database: Database::of(Database::operations_of(listing), db, None, schema),
            configuration: configuration.filter(Configuration::is_opened),
            observability: trace.and_then(|trace| Observability::of(listing, trace, level)),
            shutdown: shutdown.and_then(|shutdown| Shutdown::of(listing, shutdown)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.transport.is_none()
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
    if let Some(database) = &disclosures.database {
        hasher.update(DATABASE_DOMAIN);
        database.hash_into(&mut hasher);
    }
    // The schema function's name and every key's name and shape, and none of the resolved values: a
    // CI check that broke on a deployment's own configuration is a CI check people learn to ignore.
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

/// Domain-separated from the row digest so that a listing with an empty disclosure can never
/// collide with one that has none.
const DISCLOSURE_DOMAIN: &[u8] = b"ply.hosts.transport.v1\0";

/// Separates the database block from whatever precedes it, so that a transport-only listing and a
/// database-only listing cannot collide.
const DATABASE_DOMAIN: &[u8] = b"ply.hosts.database.v1\0";

/// Separates the configuration block from whatever precedes it, for the reason [`DATABASE_DOMAIN`]
/// exists.
const CONFIGURATION_DOMAIN: &[u8] = b"ply.hosts.configuration.v1\0";

/// Separates the observability and shutdown blocks, for the reason [`DATABASE_DOMAIN`] exists.
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

// --- `ply hosts` ------------------------------------------------------------

/// The row key and the atom, both, because the operation says *what* was bound and the atom is what
/// scheduling and isolation speak in.
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

/// Every line of `ply hosts --host`, without the indent, so the shape is testable without a
/// terminal.
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
        // Widths from the content rather than from a guess, so a long Rust path does not push the
        // flag columns out of alignment.
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

/// A bound listing with no rows is three different situations, and a reader who cannot tell them
/// apart will debug the wrong one.
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
        // Null for an operation declared without `[r]`: that is one singleton resource, not a
        // resource named "singleton".
        "resource": match &row.resource {
            ply_core::ty::Resource::Named(name) => json!(name.as_str()),
            ply_core::ty::Resource::Singleton => Value::Null,
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

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use ply_core::ty::Resource;
    use ply_eval::host::{
        Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime,
        Linearity,
    };
    use ply_span::{Symbol, codes};
    use std::sync::Arc;

    struct Never;

    impl HostHandler for Never {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(
                Diagnostic::error(codes::INTERNAL_ERROR, "a reporting test called a handler")
                    .primary(req.span, "here"),
            )
        }
    }

    pub(crate) fn op(
        effect: &str,
        name: &str,
        resource: HostResource,
        linearity: Linearity,
        blocking: bool,
        path: &'static str,
    ) -> HostOp {
        HostOp {
            effect: Symbol::new(effect),
            op: Symbol::new(name),
            resource,
            determinism: Determinism::Nondeterministic,
            linearity,
            blocking,
            secrets: false,
            path,
        }
    }

    /// The same registration, declared able to receive a credential.
    pub(crate) fn receives_secrets(mut op: HostOp) -> HostOp {
        op.secrets = true;
        op
    }

    pub(crate) fn named(label: &str) -> HostResource {
        HostResource::Only(Resource::Named(Symbol::new(label)))
    }

    /// The same registration, declared deterministic, which binds against an effect the program did
    /// not mark `nondet`.
    pub(crate) fn deterministic(mut op: HostOp) -> HostOp {
        op.determinism = Determinism::Deterministic;
        op
    }

    pub(crate) fn registry(ops: Vec<HostOp>) -> HostRegistry {
        let mut registry = HostRegistry::new();
        for op in ops {
            registry.register(op, Arc::new(Never));
        }
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{op, receives_secrets, registry};
    use super::*;
    use ply_core::ty::Resource;
    use ply_eval::host::{HostResource, Linearity};
    use ply_span::{SourceId, Symbol};

    const DB: &str = r#"
nondet effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

fn other(k: Int) -> Int / {db.read[orders]} = db.get[orders](k)

fn store(k: Int) -> Int / {db.write[orders]} = db.put[orders](k, 1)

fn stamp() -> Int / {clock.read} = clock.now()
"#;

    fn check(source: &str) -> CheckOutput {
        let module = ply_syntax::parse(SourceId(0), source).expect("the fixture parses");
        ply_core::check_module(&module).expect("the fixture typechecks")
    }

    fn full() -> HostRegistry {
        registry(vec![
            op(
                "clock",
                "now",
                HostResource::Only(Resource::Singleton),
                Linearity::Repeatable,
                false,
                "ply_host::clock::now",
            ),
            op(
                "db",
                "get",
                HostResource::Any,
                Linearity::AtMostOnce,
                true,
                "ply_host::postgres::read",
            ),
            op(
                "db",
                "put",
                HostResource::Only(Resource::Named(Symbol::new("orders"))),
                Linearity::AtMostOnce,
                true,
                "ply_host::postgres::write",
            ),
        ])
    }

    fn listing() -> HostListing {
        full().preview(&check(DB)).expect("the fixture binds")
    }

    /// One line per resolved triple, ascending, and an `Any` handler's resources spelled out rather
    /// than hidden behind a `*`.
    #[test]
    fn the_listing_names_every_resource_an_any_handler_got() {
        let listing = listing();
        let triples: Vec<String> = listing.rows.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            triples,
            [
                "clock.now",
                "db.get[orders]",
                "db.get[users]",
                "db.put[orders]"
            ]
        );
        assert_eq!(listing.handlers, 3);
        let text = listing_lines(&listing, &Disclosures::default()).join("\n");
        assert!(!text.contains('*'), "a resource was hidden:\n{text}");
    }

    /// The listing whole, rather than a claim per column.
    #[test]
    fn the_table_is_exactly_the_shape_the_contract_specifies() {
        let lines = listing_lines(&listing(), &Disclosures::default());
        let (rendered, digest) = lines.split_at(lines.len() - 1);
        assert_eq!(
            rendered.join("\n"),
            "\
3 host handlers · 4 operations · trusted computing base

OPERATION       ATOM              HANDLER                    DET  LINEAR        BLOCKING  SECRETS
clock.now       clock.read        ply_host::clock::now       no   repeatable    no        no
db.get[orders]  db.read[orders]   ply_host::postgres::read   no   at-most-once  yes       no
db.get[users]   db.read[users]    ply_host::postgres::read   no   at-most-once  yes       no
db.put[orders]  db.write[orders]  ply_host::postgres::write  no   at-most-once  yes       no
"
        );
        assert!(digest[0].starts_with("digest: b3:"), "{digest:?}");
    }

    /// The whole ambition of the listing is a one-line diff in a review, which requires two runs
    /// over one program to agree byte for byte.
    #[test]
    fn the_listing_and_its_digest_are_stable_across_runs() {
        let program = check(DB);
        let once = full().preview(&program).unwrap();
        let twice = full().preview(&program).unwrap();
        assert_eq!(
            listing_lines(&once, &Disclosures::default()),
            listing_lines(&twice, &Disclosures::default())
        );
        assert_eq!(once.digest_short(), twice.digest_short());
        assert_eq!(rows_json(&once), rows_json(&twice));
    }

    /// A handler that quietly became repeatable, or quietly stopped declaring itself blocking, is
    /// exactly the change worth a reviewer's attention.
    #[test]
    fn the_digest_moves_when_a_flag_alone_moves() {
        let program = check(DB);
        let base = full().preview(&program).unwrap().digest_short();

        let clock = |linearity, blocking| {
            registry(vec![op(
                "clock",
                "now",
                HostResource::Only(Resource::Singleton),
                linearity,
                blocking,
                "ply_host::clock::now",
            )])
            .preview(&program)
            .unwrap()
            .digest_short()
        };

        let one = clock(Linearity::Repeatable, false);
        let linear = clock(Linearity::AtMostOnce, false);
        let blocks = clock(Linearity::Repeatable, true);
        assert_ne!(one, base);
        assert_ne!(one, linear, "linearity alone must move the digest");
        assert_ne!(one, blocks, "blocking alone must move the digest");

        // The newest column, and the one whose value a reviewer most needs a diff for: a handler
        // that quietly became able to receive a credential is where the secret containment claim's claim stops
        // being enforceable.
        let secrets = registry(vec![receives_secrets(op(
            "clock",
            "now",
            HostResource::Only(Resource::Singleton),
            Linearity::Repeatable,
            false,
            "ply_host::clock::now",
        ))])
        .preview(&program)
        .unwrap();
        assert_ne!(
            one,
            secrets.digest_short(),
            "the secrets column alone must move the digest"
        );
        let text = listing_lines(&secrets, &Disclosures::default()).join("\n");
        assert!(text.contains("SECRETS"), "{text}");
        assert!(text.lines().any(|l| l.ends_with("yes")), "{text}");
        assert_eq!(row_json(&secrets.rows[0])["secrets"], true);
    }

    #[test]
    fn no_shipped_registration_declares_that_it_may_receive_a_credential() {
        let claiming: Vec<&str> = crate::hosts::registry()
            .ops()
            .chain(ply_host::registry_with_database().ops())
            .filter(|op| op.secrets)
            .map(|op| op.path)
            .collect();
        assert!(claiming.is_empty(), "{claiming:?}");
    }

    #[test]
    fn hermetic_says_so_and_still_reports_what_would_bind() {
        let lines = hermetic_lines(&listing());
        assert_eq!(lines[0], "hermetic — no host handler is bound");
        assert!(lines[2].contains("4 operations would bind"), "{lines:?}");
        assert!(lines[2].contains("--host"));
    }

    /// An empty listing is indistinguishable from a registry that failed to load, so neither form
    /// is allowed to print one and stop.
    #[test]
    fn an_empty_registry_says_it_is_empty_rather_than_printing_nothing() {
        let empty = HostRegistry::new().preview(&check(DB)).unwrap();
        assert!(hermetic_lines(&empty)[2].contains("no host handler is compiled"));
        assert!(
            listing_lines(&empty, &Disclosures::default())
                .iter()
                .any(|l| l.contains("no host handler is compiled"))
        );

        let idle = registry(vec![op(
            "db",
            "get",
            HostResource::Any,
            Linearity::AtMostOnce,
            true,
            "ply_host::postgres::read",
        )]);
        // A driver linked into a program that declares the effect and never queries is idle, not
        // wrong.
        let quiet =
            check("nondet effect db {\n  read get[r](key: Int) -> Int\n}\nfn f() -> Int = 1\n");
        let idle = idle.preview(&quiet).unwrap();
        assert!(idle.rows.is_empty());
        assert!(
            hermetic_lines(&idle)[2].contains("none serves an atom"),
            "{idle:?}"
        );
    }

    #[test]
    fn the_json_row_carries_the_declaration_side_of_the_determinism_pair() {
        let listing = listing();
        let rows = rows_json(&listing);
        let clock = &rows[0];
        assert_eq!(clock["triple"], "clock.now");
        assert_eq!(clock["atom"], "clock.read");
        assert_eq!(clock["resource"], Value::Null);
        assert_eq!(clock["linearity"], "repeatable");
        assert_eq!(clock["deterministic"], false);
        assert_eq!(clock["declared_nondet"], true);
        assert_eq!(rows[1]["resource"], "orders");
        assert_eq!(rows[1]["handler"], "ply_host::postgres::read");
        assert_eq!(rows[1]["blocking"], true);
    }

    /// The default is the point: nothing binds without the flag, and a hermetic binding reaches
    /// nothing whatever the registry holds.
    #[test]
    fn hermetic_is_the_default_and_reaches_nothing() {
        let program = check(DB);
        let hosts = Hosts::open(
            &program,
            false,
            &[],
            None,
            Configuration::default(),
            &crate::trace::TraceOptions::silent(),
            None,
        )
        .unwrap();
        assert!(hosts.is_hermetic());
        assert_eq!(hosts.label(), "hermetic");
        assert!(hosts.listing().is_empty());
        for def in program.defs.values() {
            assert!(!hosts.reaches(&def.footprint));
        }
    }

    /// A footprint that meets the binding is host-backed and therefore not isolated, however
    /// isolated its atoms would otherwise make it.
    #[test]
    fn a_host_backed_test_leaves_the_trivially_parallel_count() {
        let program = check(DB);
        let hosts = Hosts::bind(full(), &program, true).unwrap();
        assert_eq!(hosts.label(), "host");

        let reads = program
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == "lookup")
            .unwrap();
        let pure = Footprint::empty();
        assert!(hosts.reaches(&reads.footprint));
        assert!(!hosts.reaches(&pure));

        let counts = Counts::of(
            &hosts,
            [(&reads.footprint, true), (&pure, true), (&pure, false)],
        );
        assert_eq!(counts.total, 3);
        assert_eq!(counts.host, 1);
        assert_eq!(counts.isolated, 1);
        assert_eq!(counts.shared, 1);

        // The same corpus under a hermetic binding: the host column is empty and every other number
        // is what it was before W1.
        let hermetic = Hosts::open(
            &program,
            false,
            &[],
            None,
            Configuration::default(),
            &crate::trace::TraceOptions::silent(),
            None,
        )
        .unwrap();
        let counts = Counts::of(
            &hermetic,
            [(&reads.footprint, true), (&pure, true), (&pure, false)],
        );
        assert_eq!(counts.host, 0);
        assert_eq!(counts.isolated, 2);
        assert_eq!(counts.shared, 1);
    }

    // --- database -----------------------------------------------------------

    /// A registry whose `db.*` resolve to the postgres driver's paths, which is how a listing row
    /// is recognised as one.
    fn postgres(op_name: &'static str, path: &'static str) -> HostRegistry {
        registry(vec![op(
            "db",
            op_name,
            HostResource::Any,
            Linearity::AtMostOnce,
            true,
            path,
        )])
    }

    fn configured(schema: Option<&str>) -> DbConfig {
        crate::db::DbOptions {
            url: Some("postgres://ply:hunter2@127.0.0.1:5433/desk".to_string()),
            schema: schema.map(str::to_string),
            ..crate::db::DbOptions::default()
        }
        .resolve_with(true, &|_| None)
        .expect("the fixture URL parses")
        .expect("--host and a URL yield a configuration")
    }

    /// The check `--db` exists for: a program that reaches postgres and a run that named no
    /// database is a service that would discover it had nowhere to connect after accepting a
    /// request.
    #[test]
    fn reaching_postgres_with_no_database_configured_is_refused_before_anything_runs() {
        let program = check(DB);
        let Err(diagnostics) =
            Hosts::bind_with(postgres("get", "ply_host::db::query"), &program, true, None)
        else {
            panic!("a bound driver with no database must be E0431");
        };
        assert_eq!(diagnostics[0].code, ply_span::codes::DB_NOT_CONFIGURED);
        assert!(
            diagnostics[0]
                .notes
                .iter()
                .any(|n| n.contains("db.get[orders]")),
            "the reader is not told which operations bound: {:?}",
            diagnostics[0].notes
        );
    }

    /// The complement, and the reason the check keys on the *binding*: an HTTP-only program under
    /// `--host` binds no postgres handler and must not be made to name a database it will never
    /// open.
    #[test]
    fn a_program_that_reaches_no_database_needs_none_and_discloses_none() {
        let program = check(DB);
        let hosts = Hosts::bind(full(), &program, true).expect("net and clock bind without a URL");
        assert!(hosts.database().is_none());
        assert!(!hosts.is_live_database());
        assert!(hosts.disclosures().is_empty());
        assert_eq!(
            digest_short(hosts.listing(), &hosts.disclosures()),
            hosts.listing().digest_short(),
            "a program with no database in reach must hash what it hashed before W4"
        );
    }

    #[test]
    fn a_configured_run_says_it_reached_a_real_database_and_never_says_the_password() {
        let program = check(DB);
        let hosts = Hosts::bind_with(
            postgres("get", "ply_host::db::query"),
            &program,
            true,
            Some(configured(None)),
        )
        .expect("a bound driver with a database binds");
        assert!(hosts.is_live_database());

        let line = database_line(&hosts).expect("a live database is reported");
        assert!(
            line.contains("postgres://ply:****@127.0.0.1:5433/desk"),
            "{line}"
        );
        assert!(line.contains("configured by --db"), "{line}");
        assert!(!line.contains("hunter2"), "{line}");

        let text = listing_lines(hosts.listing(), &hosts.disclosures()).join("\n");
        assert!(
            text.contains("ply_host::db::scan · select insert"),
            "{text}"
        );
        assert!(text.contains("8 connections · acquire 5000ms"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(
            !serde_json::to_string(&hosts.summary_json())
                .unwrap()
                .contains("hunter2"),
            "the `--json` object carried the password"
        );
    }

    /// Transactions as handlers handles `db.rollback` in Ply, inside `transaction`, so a bound one would abort
    /// nothing and commit what the program meant to discard.
    #[test]
    fn a_bound_rollback_is_refused_as_a_defect_rather_than_listed() {
        let program = check(
            "nondet effect db {\n  write rollback[r](reason: Int) -> Int\n}\n\
             fn f() -> Int / {db.write[orders]} = db.rollback[orders](1)\n",
        );
        let Err(diagnostics) = Hosts::bind_with(
            postgres("rollback", "ply_host::db::abort"),
            &program,
            true,
            Some(configured(None)),
        ) else {
            panic!("a bound rollback must be refused as a defect");
        };
        assert_eq!(diagnostics[0].code, ply_span::codes::INTERNAL_ERROR);
        assert!(diagnostics[0].message.contains("db.rollback"));
    }

    #[test]
    fn a_db_schema_naming_nothing_is_refused_with_what_the_program_does_have() {
        let program = check(DB);
        let Err(diagnostics) = Hosts::bind_with(
            postgres("get", "ply_host::db::query"),
            &program,
            true,
            Some(configured(Some("desk.schema"))),
        ) else {
            panic!("a schema function that does not exist must be refused");
        };
        assert_eq!(diagnostics[0].code, ply_span::codes::DB_NOT_CONFIGURED);
        assert!(
            diagnostics[0].notes.iter().any(|n| n.contains("E0433")),
            "the reader is not told what dropping the flag costs: {:?}",
            diagnostics[0].notes
        );
    }

    // --- transport ----------------------------------------------------------

    /// A registry whose `net.listen_tls` resolves to the real TLS handler, so the listing carries
    /// the row `Transport::of` keys on.
    fn transport_only(transport: Transport) -> Disclosures {
        Disclosures {
            transport: Some(transport),
            ..Disclosures::default()
        }
    }

    fn tls_registry() -> HostRegistry {
        registry(vec![op(
            "net",
            "listen_tls",
            HostResource::Any,
            Linearity::AtMostOnce,
            false,
            tls::HANDLER,
        )])
    }

    const NET: &str = r#"
nondet effect net {
  write listen_tls[s](port: Int, credential: String) -> Int
}

fn serve() -> Int / {net.write[api]} = net.listen_tls[api](443, "api")
"#;

    fn tls_listing() -> HostListing {
        tls_registry()
            .preview(&check(NET))
            .expect("the fixture binds")
    }

    /// A program that cannot create a TLS listener says nothing about TLS, and its digest is what
    /// it was before W3 — which is the whole reason the block is conditional rather than always
    /// printed.
    #[test]
    fn a_plaintext_program_reports_no_transport_and_keeps_its_digest() {
        let listing = listing();
        assert!(Transport::of(&listing, None).is_none());
        assert_eq!(
            digest_short(&listing, &Disclosures::default()),
            listing.digest_short()
        );
        assert!(
            !listing_lines(&listing, &Disclosures::default())
                .join("\n")
                .contains("transport")
        );
    }

    #[test]
    fn a_program_that_can_listen_over_tls_discloses_the_stack_by_name() {
        let listing = tls_listing();
        let transport = Transport::of(&listing, None).expect("the tls handler is in the listing");
        assert_eq!(
            transport.lines(),
            [
                "",
                "transport",
                "tls  rustls 0.23.43 · provider ring · TLS 1.3, TLS 1.2 · alpn http/1.1",
                "",
                "credentials",
                "none — `net.listen_tls` is E0429 until `--tls NAME=CERT,KEY` names one",
            ]
        );
        let text = listing_lines(&listing, &transport_only(transport)).join("\n");
        assert!(text.contains(tls::HANDLER), "{text}");
        assert!(text.contains("alpn http/1.1"), "{text}");
    }

    /// The `--json` object carries the whole fingerprint; the table carries enough of it to
    /// recognise and not enough to push the columns out.
    #[test]
    fn a_credential_is_listed_by_name_and_fingerprint() {
        let transport = Transport {
            library: tls::LIBRARY,
            version: tls::VERSION,
            provider: tls::PROVIDER,
            versions: &tls::VERSIONS,
            alpn: &tls::ALPN,
            credentials: vec![CredentialView {
                name: "api".to_string(),
                fingerprint: "sha256:9f2c1a4e8b03c7d5e6f70819a2b3c4d5".to_string(),
                certificates: 2,
            }],
        };
        assert_eq!(
            transport.lines().last().unwrap(),
            "api  sha256:9f2c1a4e8b03…  2 certificates"
        );
        assert_eq!(
            transport.json()["credentials"][0]["fingerprint"],
            "sha256:9f2c1a4e8b03c7d5e6f70819a2b3c4d5",
            "the table abbreviates; the object must not"
        );
    }

    /// The trusted computing base listing: a CI check that broke on every certificate renewal is a CI check people learn
    /// to ignore.
    #[test]
    fn the_digest_survives_a_rotation_and_moves_when_a_credential_does() {
        let listing = tls_listing();
        let with = |credentials: Vec<CredentialView>| {
            digest_short(
                &listing,
                &transport_only(Transport {
                    library: tls::LIBRARY,
                    version: tls::VERSION,
                    provider: tls::PROVIDER,
                    versions: &tls::VERSIONS,
                    alpn: &tls::ALPN,
                    credentials,
                }),
            )
        };
        let credential = |fingerprint: &str| CredentialView {
            name: "api".to_string(),
            fingerprint: fingerprint.to_string(),
            certificates: 1,
        };

        let before = with(vec![credential("sha256:aaaa")]);
        assert_eq!(
            before,
            with(vec![credential("sha256:bbbb")]),
            "a renewed certificate is an operational fact, not a structural one"
        );
        assert_ne!(
            before,
            with(vec![
                credential("sha256:aaaa"),
                CredentialView {
                    name: "admin".to_string(),
                    fingerprint: "sha256:cccc".to_string(),
                    certificates: 1,
                }
            ]),
            "a second credential is a second thing the run can serve"
        );
        assert_ne!(before, with(Vec::new()));
        assert_ne!(
            before,
            listing.digest_short(),
            "a configured credential must not hash as if there were none"
        );
    }

    /// A handshake failure is not the program's fault and not attributable to any definition, so it
    /// is never a diagnostic — but silence would be wrong, so it is counted with its reason.
    #[test]
    fn refused_handshakes_are_counted_and_named_rather_than_raised() {
        assert!(handshake_lines(&tls::HandshakeCounts::default()).is_empty());
        let counts = tls::HandshakeCounts {
            completed: 7,
            refused: 3,
            reasons: vec![("no application protocol in common", 2), ("not tls", 1)],
        };
        assert_eq!(
            handshake_lines(&counts),
            [
                "handshakes: 7 completed, 3 refused",
                "  2 no application protocol in common",
                "  1 not tls",
            ]
        );
        let json = handshakes_json(&counts);
        assert_eq!(json["refused"], 3);
        assert_eq!(
            json["reasons"][0]["reason"],
            "no application protocol in common"
        );
    }
}
