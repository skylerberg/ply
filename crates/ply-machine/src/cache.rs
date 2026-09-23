//! The store `ply cache` reports on, as the CLI opens and works it.
//!
//! Opening, locking, sweeping and flushing a store is Rust's: a reader of the on-disk format
//! written in Ply would be a second implementation of it. What each subcommand *says* is the
//! program's, in `crates/ply-cli/ply/cache.ply`; this hands it the action's result as a value.

/// The cache subcommand, as plain data the machine reads; the shell's parsed flags convert into
/// this.
#[derive(Clone, Debug)]
pub enum CacheAction {
    Clear(CacheScope),
    Stats(CacheScope),
    Compact(CacheScope),
    Inspect(InspectOptions),
}

#[derive(Clone, Debug)]
pub struct CacheScope {
    pub path: std::path::PathBuf,
    pub json: bool,
}

#[derive(Clone, Debug)]
pub struct InspectOptions {
    pub query: String,
    pub path: std::path::PathBuf,
    pub json: bool,
}
use crate::hosts::Lent;
use crate::payload::{count, ctor, diags_value, option, record};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_store::{
    CacheStats, CachedDecl, Compaction, DeclBody, DefKind, FRONTEND_VERSION, FileSpan, Found,
    FoundDef, FoundTest, Outcome, PROVER_VERSION, RUNTIME_VERSION, Store,
};
use ply_ty::{Footprint, Type, print_scheme, print_type};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/cache.ply` declares. It is lent to that one entry and nowhere
/// else: four other commands work the same `Store`, and none of them is a program.
const EFFECT: &str = "store";

/// The module the payload's constructors are declared in, as a program-wide name.
const PAYLOAD: &str = "cache";

/// One registration per subcommand: the operation the program performs names what was done.
const OPERATIONS: [(&str, &str); 4] = [
    ("statistics", "ply_cli::cache::statistics"),
    ("matches", "ply_cli::cache::matches"),
    ("compacted", "ply_cli::cache::compacted"),
    ("cleared", "ply_cli::cache::cleared"),
];

/// The action runs here, before the program is entered: a handler is handed `&self`, and a store
/// that compacts or clears is worked with `&mut`.
pub fn lent(action: &CacheAction) -> Vec<Lent> {
    let done: Arc<dyn HostHandler> = Arc::new(Did::of(action));
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&done)))
        .collect()
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A cache on disk is not a function of program state.
        determinism: Determinism::Nondeterministic,
        // The action ran once already; a second perform reads the same answer.
        linearity: Linearity::Repeatable,
        blocking: false,
        secrets: false,
        path,
    }
}

/// The one action this invocation ran, and what it came back with.
enum Did {
    Statistics(Result<Stats, Refused>),
    Matches(Result<Matches, Refused>),
    Compacted(Result<Compacted, Refused>),
    Cleared(Result<Cleared, Refused>),
}

impl Did {
    fn of(action: &CacheAction) -> Did {
        match action {
            CacheAction::Stats(scope) => Did::Statistics(statistics(scope)),
            CacheAction::Compact(scope) => Did::Compacted(compacted(scope)),
            CacheAction::Clear(scope) => Did::Cleared(cleared(scope)),
            CacheAction::Inspect(args) => Did::Matches(matches(args)),
        }
    }

    fn op(&self) -> &'static str {
        match self {
            Did::Statistics(_) => "statistics",
            Did::Matches(_) => "matches",
            Did::Compacted(_) => "compacted",
            Did::Cleared(_) => "cleared",
        }
    }

    fn value(&self) -> PlyValue {
        match self {
            Did::Statistics(answer) => answered(answer, statistics_value),
            Did::Matches(answer) => answered(answer, matches_value),
            Did::Compacted(answer) => answered(answer, compacted_value),
            Did::Cleared(answer) => answered(answer, cleared_value),
        }
    }
}

impl HostHandler for Did {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        if req.op.op.as_str() != self.op() {
            return Err(unasked(req.op.op.as_str(), self.op(), req.span));
        }
        Ok(HostAnswer::Value(self.value()))
    }
}

// --- Opening -----------------------------------------------------------------

/// Why an action did not finish, as `cache.ply` reads it.
struct Refused {
    /// `None` when no store opened at all, which is the one refusal with no cache to name.
    directory: Option<String>,
    diags: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

/// A store that opened, with however long that took and whatever it degraded past.
struct Opened {
    store: Store,
    open_us: i64,
    warnings: Vec<Diagnostic>,
}

/// Opened once per invocation, with the migration notice folded into the store's own warnings.
fn open(scope: &CacheScope) -> Result<Opened, Refused> {
    let root = crate::load::project_root(&scope.path);
    let started = std::time::Instant::now();
    let mut store = match Store::open(&root) {
        Ok(store) => store,
        Err(e) => {
            return Err(Refused {
                directory: None,
                diags: vec![
                    Diagnostic::error(
                        codes::RUNTIME_ERROR,
                        format!("could not open a cache under `{}`: {e:#}", root.display()),
                    )
                    .primary(Span::DUMMY, "the cache directory is unusable")
                    .note("pass the directory the cache belongs to; the default is `.`"),
                ],
                warnings: Vec::new(),
            });
        }
    };
    let open_us = (started.elapsed().as_secs_f64() * 1_000_000.0).round() as i64;
    let mut warnings = store.take_warnings();
    let notice = crate::migrate::notice(&store, &warnings);
    warnings.extend(notice);
    Ok(Opened {
        store,
        open_us,
        warnings,
    })
}

fn stopped(dir: &Path, diagnostic: Diagnostic, warnings: Vec<Diagnostic>) -> Refused {
    Refused {
        directory: Some(dir.display().to_string()),
        diags: vec![diagnostic],
        warnings,
    }
}

// --- `ply cache stats` --------------------------------------------------------

struct Stats {
    directory: String,
    warnings: Vec<Diagnostic>,
    root: String,
    results_file: String,
    frontend_file: String,
    frontend_data_file: String,
    open_us: i64,
    counts: CacheStats,
}

fn statistics(scope: &CacheScope) -> Result<Stats, Refused> {
    let Opened {
        store,
        open_us,
        warnings,
    } = open(scope)?;
    Ok(Stats {
        directory: shown(store.dir()),
        warnings,
        root: shown(store.root()),
        results_file: shown(store.path()),
        frontend_file: shown(store.frontend_path()),
        frontend_data_file: shown(store.frontend_data_path()),
        open_us,
        counts: store.stats(),
    })
}

fn statistics_value(s: &Stats) -> PlyValue {
    record(vec![
        ("directory", PlyValue::str(&s.directory)),
        ("warnings", diags_value(&s.warnings)),
        ("runtime_version", PlyValue::str(RUNTIME_VERSION)),
        ("frontend_version", PlyValue::str(FRONTEND_VERSION)),
        ("prover_version", PlyValue::str(PROVER_VERSION)),
        ("root", PlyValue::str(&s.root)),
        ("results_file", PlyValue::str(&s.results_file)),
        ("frontend_file", PlyValue::str(&s.frontend_file)),
        ("frontend_data_file", PlyValue::str(&s.frontend_data_file)),
        ("open_us", PlyValue::Int(s.open_us)),
        ("results", count(s.counts.results)),
        ("definitions_seen", count(s.counts.definitions_seen)),
        ("results_bytes", size(s.counts.results_bytes)),
        ("obligations", count(s.counts.obligations)),
        ("reviews", count(s.counts.reviews)),
        ("sources", count(s.counts.sources)),
        ("defs", count(s.counts.defs)),
        ("decls", count(s.counts.decls)),
        ("bodies", count(s.counts.bodies)),
        ("index_bytes", size(s.counts.index_bytes)),
        ("data_bytes", size(s.counts.data_bytes)),
        ("garbage_bytes", option(s.counts.garbage_bytes.map(size))),
    ])
}

// --- `ply cache compact` ------------------------------------------------------

struct Compacted {
    directory: String,
    warnings: Vec<Diagnostic>,
    files_kept: usize,
    compaction: Compaction,
    results: usize,
}

fn compacted(scope: &CacheScope) -> Result<Compacted, Refused> {
    let Opened {
        mut store,
        mut warnings,
        ..
    } = open(scope)?;

    // Compaction drops what surviving files do not name, so a partial walk would delete silently.
    let keep = match crate::load::ply_files(store.root()) {
        // A shipped module has no file on disk, and its key is not a place under the root: it is
        // the whole of what names it, whatever the root of this run happens to be.
        Ok(mut keep) => {
            keep.extend(
                store
                    .source_keys()
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|p| crate::shelf::is_pseudo_path(p)),
            );
            keep
        }
        Err(e) => {
            let root = shown(store.root());
            return Err(stopped(
                store.dir(),
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("could not list the `.ply` files under `{root}`: {e}"),
                )
                .primary(Span::DUMMY, "nothing was dropped")
                .note("compaction needs to see every source file before it discards anything")
                .note("check the directory's permissions, then run it again"),
                warnings,
            ));
        }
    };

    let compaction = match store.compact(&keep) {
        Ok(compaction) => compaction,
        Err(e) => {
            let dir = shown(store.dir());
            return Err(stopped(
                store.dir(),
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("could not compact the cache at `{dir}`: {e:#}"),
                )
                .primary(Span::DUMMY, "the cache was left as it was")
                .note("check the directory's permissions, or run `ply cache clear` to start over"),
                warnings,
            ));
        }
    };
    warnings.extend(store.take_warnings());

    Ok(Compacted {
        directory: shown(store.dir()),
        warnings,
        files_kept: keep.len(),
        compaction,
        results: store.stats().results,
    })
}

fn compacted_value(c: &Compacted) -> PlyValue {
    let dropped = c.compaction.dropped;
    record(vec![
        ("directory", PlyValue::str(&c.directory)),
        ("warnings", diags_value(&c.warnings)),
        ("files_kept", count(c.files_kept)),
        (
            "dropped",
            record(vec![
                ("sources", count(dropped.sources)),
                ("defs", count(dropped.defs)),
                ("decls", count(dropped.decls)),
                ("bodies", count(dropped.bodies)),
            ]),
        ),
        ("bytes_before", size(c.compaction.bytes_before)),
        ("bytes_after", size(c.compaction.bytes_after)),
        ("results", count(c.results)),
    ])
}

// --- `ply cache clear` --------------------------------------------------------

struct Cleared {
    directory: String,
    warnings: Vec<Diagnostic>,
    cleared: usize,
}

fn cleared(scope: &CacheScope) -> Result<Cleared, Refused> {
    let Opened {
        mut store,
        warnings,
        ..
    } = open(scope)?;
    let before = store.len();

    if let Err(e) = store.clear() {
        let dir = shown(store.dir());
        return Err(stopped(
            store.dir(),
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not clear the cache at `{dir}`: {e:#}"),
            )
            .primary(Span::DUMMY, "the cache was left as it was")
            .note("check the directory's permissions, or delete it by hand"),
            warnings,
        ));
    }

    Ok(Cleared {
        directory: shown(store.dir()),
        warnings,
        cleared: before,
    })
}

fn cleared_value(c: &Cleared) -> PlyValue {
    record(vec![
        ("directory", PlyValue::str(&c.directory)),
        ("warnings", diags_value(&c.warnings)),
        ("cleared", count(c.cleared)),
    ])
}

// --- `ply cache inspect` ------------------------------------------------------

struct Matches {
    directory: String,
    warnings: Vec<Diagnostic>,
    entries: Vec<Entry>,
}

/// One cached name, gathered before anything is written so the two forms cannot disagree.
struct Entry {
    name: String,
    kind: Kind,
    hash: String,
    file: String,
    location: Option<String>,
    stale: bool,
    interface: Interface,
    footprint: Option<Vec<String>>,
    witness: Vec<(String, String)>,
    body: Option<(u32, usize)>,
    outcome: Option<Outcome>,
}

enum Kind {
    Def(DefKind),
    Test(bool),
}

/// `Unrecorded` is a half-pruned cache, or slots written for other names.
enum Interface {
    OfFn(String),
    OfType(usize, Vec<Variant>),
    OfEffect(bool, Vec<Operation>),
    OfTest(bool),
    Unrecorded,
}

struct Variant {
    name: String,
    fields: Vec<String>,
}

struct Operation {
    mode: &'static str,
    name: String,
    resource: bool,
    params: Vec<String>,
    ret: String,
}

fn matches(args: &InspectOptions) -> Result<Matches, Refused> {
    let scope = CacheScope {
        path: args.path.clone(),
        json: args.json,
    };
    let Opened {
        store, warnings, ..
    } = open(&scope)?;
    let found = store.lookup(&args.query);
    Ok(Matches {
        directory: shown(store.dir()),
        warnings,
        entries: found.iter().map(|f| entry_of(f, &store)).collect(),
    })
}

fn entry_of(found: &Found, store: &Store) -> Entry {
    match found {
        Found::Def(def) => def_entry(def, store),
        Found::Test(test) => test_entry(test, store),
    }
}

fn def_entry(def: &FoundDef, store: &Store) -> Entry {
    let (location, stale) = locate(store, &def.path, def.span);
    let mut witness = Vec::new();
    let mut footprint = None;

    let interface = match def.kind {
        DefKind::Fn => match store.def_of(def.hash, &def.name) {
            Some(cached) => {
                witness = witnessed(&cached.names);
                footprint = Some(atoms_of(&cached.footprint));
                Interface::OfFn(print_scheme(&cached.scheme))
            }
            None => Interface::Unrecorded,
        },
        DefKind::Type | DefKind::Effect => match store.decl_of(def.hash, &def.name) {
            Some(cached) => {
                witness = witnessed(&cached.names);
                declaration(&cached, &variant_names(store, def))
            }
            None => Interface::Unrecorded,
        },
    };

    Entry {
        name: def.name.to_string(),
        kind: Kind::Def(def.kind),
        hash: def.hash.to_hex(),
        file: shown(&def.path),
        location,
        stale,
        interface,
        footprint,
        witness,
        body: store.body(def.hash).map(|b| (b.encoding(), b.len())),
        outcome: None,
    }
}

fn test_entry(test: &FoundTest, store: &Store) -> Entry {
    let (location, stale) = locate(store, &test.path, test.span);
    Entry {
        name: test.name.clone(),
        kind: Kind::Test(test.nondet),
        hash: test.hash.to_hex(),
        file: shown(&test.path),
        location,
        stale,
        interface: Interface::OfTest(test.nondet),
        footprint: Some(atoms_of(&test.footprint)),
        witness: Vec::new(),
        body: store.body(test.hash).map(|b| (b.encoding(), b.len())),
        outcome: store.get(test.hash),
    }
}

fn matches_value(m: &Matches) -> PlyValue {
    record(vec![
        ("directory", PlyValue::str(&m.directory)),
        ("warnings", diags_value(&m.warnings)),
        (
            "entries",
            PlyValue::list(m.entries.iter().map(entry_value).collect()),
        ),
    ])
}

fn entry_value(e: &Entry) -> PlyValue {
    record(vec![
        ("name", PlyValue::str(&e.name)),
        ("kind", kind_value(&e.kind)),
        ("hash", PlyValue::str(&e.hash)),
        ("file", PlyValue::str(&e.file)),
        ("location", option(e.location.as_deref().map(PlyValue::str))),
        ("stale", PlyValue::Bool(e.stale)),
        ("interface", interface_value(&e.interface)),
        ("footprint", option(e.footprint.as_deref().map(texts))),
        (
            "witness",
            PlyValue::list(
                e.witness
                    .iter()
                    .map(|(name, hash)| {
                        record(vec![
                            ("name", PlyValue::str(name)),
                            ("hash", PlyValue::str(hash)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "body",
            option(e.body.map(|(encoding, bytes)| {
                record(vec![
                    ("encoding", PlyValue::Int(encoding as i64)),
                    ("bytes", count(bytes)),
                ])
            })),
        ),
        ("outcome", option(e.outcome.as_ref().map(outcome_value))),
    ])
}

fn kind_value(kind: &Kind) -> PlyValue {
    match kind {
        Kind::Def(DefKind::Fn) => ctor(PAYLOAD, "AFn", Vec::new()),
        Kind::Def(DefKind::Type) => ctor(PAYLOAD, "AType", Vec::new()),
        Kind::Def(DefKind::Effect) => ctor(PAYLOAD, "AnEffect", Vec::new()),
        Kind::Test(nondet) => ctor(PAYLOAD, "ATest", vec![PlyValue::Bool(*nondet)]),
    }
}

fn interface_value(interface: &Interface) -> PlyValue {
    match interface {
        Interface::OfFn(scheme) => ctor(PAYLOAD, "OfFn", vec![PlyValue::str(scheme)]),
        Interface::OfType(arity, variants) => ctor(
            PAYLOAD,
            "OfType",
            vec![
                count(*arity),
                PlyValue::list(
                    variants
                        .iter()
                        .map(|v| {
                            record(vec![
                                ("name", PlyValue::str(&v.name)),
                                ("fields", texts(&v.fields)),
                            ])
                        })
                        .collect(),
                ),
            ],
        ),
        Interface::OfEffect(nondet, ops) => ctor(
            PAYLOAD,
            "OfEffect",
            vec![
                PlyValue::Bool(*nondet),
                PlyValue::list(
                    ops.iter()
                        .map(|op| {
                            record(vec![
                                ("mode", PlyValue::str(op.mode)),
                                ("name", PlyValue::str(&op.name)),
                                ("resource", PlyValue::Bool(op.resource)),
                                ("params", texts(&op.params)),
                                ("ret", PlyValue::str(&op.ret)),
                            ])
                        })
                        .collect(),
                ),
            ],
        ),
        Interface::OfTest(nondet) => ctor(PAYLOAD, "OfTest", vec![PlyValue::Bool(*nondet)]),
        Interface::Unrecorded => ctor(PAYLOAD, "Unrecorded", Vec::new()),
    }
}

fn outcome_value(outcome: &Outcome) -> PlyValue {
    match outcome {
        Outcome::Pass => ctor(PAYLOAD, "Passed", Vec::new()),
        Outcome::Fail { message, .. } => ctor(PAYLOAD, "Failed", vec![PlyValue::str(message)]),
    }
}

/// Variant names are not in the hashed interface, so they come from the file's fingerprint.
fn variant_names(store: &Store, def: &FoundDef) -> Vec<Symbol> {
    let Some(fingerprint) = store.fingerprint(&def.path) else {
        return Vec::new();
    };
    fingerprint
        .defs
        .iter()
        .find(|entry| entry.hash == def.hash && entry.name == def.name)
        .map(|entry| entry.members.iter().map(|m| m.name.clone()).collect())
        .unwrap_or_default()
}

fn declaration(cached: &CachedDecl, variants: &[Symbol]) -> Interface {
    match &cached.body {
        DeclBody::Type { arity, ctors } => Interface::OfType(
            *arity,
            ctors
                .iter()
                .enumerate()
                .map(|(i, c)| Variant {
                    name: match variants.get(i) {
                        Some(name) => name.to_string(),
                        None => print_scheme(&c.scheme),
                    },
                    fields: printed(&c.fields),
                })
                .collect(),
        ),
        DeclBody::Effect { nondet, ops } => Interface::OfEffect(
            *nondet,
            ops.iter()
                .map(|op| Operation {
                    mode: op.mode.as_str(),
                    name: op.name.to_string(),
                    resource: op.resource_param,
                    params: printed(&op.params),
                    ret: print_type(&op.ret),
                })
                .collect(),
        ),
    }
}

/// A stored span is a byte range into the file as cached, valid only while those bytes are.
fn locate(store: &Store, path: &Path, span: FileSpan) -> (Option<String>, bool) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, true);
    };
    let unchanged = store
        .fingerprint(path)
        .is_some_and(|f| f.matches_bytes(text.as_bytes()));
    if !unchanged || span.start as usize > text.len() {
        return (None, true);
    }
    let mut sources = ply_span::SourceMap::new();
    let id = sources.add(path, text);
    let Some(file) = sources.get(id) else {
        return (None, true);
    };
    let (line, column) = file.line_col(span.start);
    (Some(format!("{}:{line}:{column}", path.display())), false)
}

// --- Small things -------------------------------------------------------------

/// `Ok(v)` or `Err(Refusal)`, as the program reads the operation's answer.
fn answered<T>(answer: &Result<T, Refused>, value: fn(&T) -> PlyValue) -> PlyValue {
    match answer {
        Ok(done) => PlyValue::ctor("Ok", vec![value(done)]),
        Err(why) => PlyValue::ctor("Err", vec![refusal_value(why)]),
    }
}

fn refusal_value(why: &Refused) -> PlyValue {
    record(vec![
        (
            "directory",
            option(why.directory.as_deref().map(PlyValue::str)),
        ),
        ("diags", diags_value(&why.diags)),
        ("warnings", diags_value(&why.warnings)),
    ])
}

fn witnessed(names: &[ply_store::NameRef]) -> Vec<(String, String)> {
    names
        .iter()
        .map(|n| (n.name.to_string(), n.hash.to_hex()))
        .collect()
}

/// Through the printer, exactly as `Footprint`'s own rendering goes: the atoms, which the report
/// then braces.
fn atoms_of(footprint: &Footprint) -> Vec<String> {
    ply_ty::Printer::new().atoms(&footprint.0)
}

fn printed(types: &[Type]) -> Vec<String> {
    types.iter().map(print_type).collect()
}

fn texts(items: &[String]) -> PlyValue {
    PlyValue::list(items.iter().map(PlyValue::str).collect())
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

/// A byte count the program reads as an `Int`; no cache comes near `i64`.
fn size(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}

#[cold]
fn unasked(op: &str, ran: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` reached the binding, and this run did `{EFFECT}.{ran}`"),
    )
    .primary(span, "this perform reached `ply cache`")
    .note("the subcommand and the operation are written together; this is Ply's fault")
}
