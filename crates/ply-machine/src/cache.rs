//! The store `ply cache` reports on, and the front-end cache every load reads and files, as the
//! CLI opens and works them.
//!
//! Opening, locking, sweeping and flushing a store is Rust's: a reader of the on-disk format
//! written in Ply would be a second implementation of it. What the front end files is the
//! compiler's own values, which this keeps and hands back without reading; what each subcommand
//! *says* is the program's, in `crates/ply-cli/ply/cache.ply`.

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
use crate::payload::{count, ctor, diags_value, field_of, option, record};
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_eval::{DefHash, Diagnostic, Span, Symbol, Value as PlyValue, codes};
use ply_store::{
    BODY_ENCODING, CacheStats, Compaction, ContentHash, DefBody, DefEntry, DefKind,
    FRONTEND_VERSION, FileSpan, Found, FoundDef, FoundTest, Member, Outcome, PROVER_VERSION,
    RUNTIME_VERSION, Slot, SourceFingerprint, Store, TestEntry,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/cache.ply` declares, lent to the `ply` program: `ply cache`
/// performs the four reports, and every load performs `known` and `file`.
const EFFECT: &str = "store";

/// The module the payload's constructors are declared in, as a program-wide name.
const PAYLOAD: &str = "cache";

/// One registration per operation: the operation the program performs names what was done.
const OPERATIONS: [(&str, &str); 6] = [
    ("statistics", "ply_machine::cache::statistics"),
    ("matches", "ply_machine::cache::matches"),
    ("compacted", "ply_machine::cache::compacted"),
    ("cleared", "ply_machine::cache::cleared"),
    ("known", "ply_machine::cache::known"),
    ("file", "ply_machine::cache::file"),
];

/// The ops and the one handler serving them: the action runs where the program asks for it, with
/// the path it names. A handler is handed `&self`, and a store that compacts, clears or files is
/// worked with `&mut`.
pub fn lent() -> Vec<Lent> {
    let done: Arc<dyn HostHandler> = Arc::new(Did);
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

/// The one handler: the action runs where the program asks for it.
struct Did;

impl HostHandler for Did {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let path = |value: &PlyValue| -> Result<PathBuf, Diagnostic> {
            Ok(PathBuf::from(value.as_str(span, "the project's path")?))
        };
        let value = match (req.op.op.as_str(), req.args) {
            ("statistics", [p]) => answered(
                &statistics(&CacheScope {
                    path: path(p)?,
                    json: false,
                }),
                statistics_value,
            ),
            ("matches", [query, p]) => answered(
                &matches(&InspectOptions {
                    query: query
                        .as_str(span, "the definition asked about")?
                        .to_string(),
                    path: path(p)?,
                    json: false,
                }),
                matches_value,
            ),
            ("compacted", [p]) => answered(
                &compacted(&CacheScope {
                    path: path(p)?,
                    json: false,
                }),
                compacted_value,
            ),
            ("cleared", [p]) => answered(
                &cleared(&CacheScope {
                    path: path(p)?,
                    json: false,
                }),
                cleared_value,
            ),
            ("known", [p, sources]) => known(&path(p)?, &sources_of(sources, span)?),
            ("file", [p, sources, filing, whole]) => diags_value(&file(
                &path(p)?,
                &sources_of(sources, span)?,
                filing,
                whole.as_bool(span, "whether the whole project was loaded")?,
                span,
            )?),
            (other, _) => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

fn unasked(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`store.{op}` reached the binding, and `ply` serves no such operation"),
    )
    .primary(span, "this is Ply's fault")
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

fn unopened(root: &Path, e: &dyn std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("could not open a cache under `{}`: {e:#}", root.display()),
    )
    .primary(Span::DUMMY, "the cache directory is unusable")
    .note("pass the directory the cache belongs to; the default is `.`")
}

/// Opened once per invocation, with the migration notice folded into the store's own warnings.
fn open(path: &Path) -> Result<Opened, Refused> {
    let root = crate::load::project_root(path);
    let started = std::time::Instant::now();
    let mut store = match Store::open(&root) {
        Ok(store) => store,
        Err(e) => {
            return Err(Refused {
                directory: None,
                diags: vec![unopened(&root, &e)],
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

// --- What a load reads and files ---------------------------------------------

/// The files a load read, as the program names them: its own under the project root, a
/// dependency's under its package's root, and a shipped module under its pseudo-path.
struct Sources {
    files: Vec<SourceFile>,
    /// Each dependency package's root and identity, which its files are keyed under.
    packages: Vec<(PathBuf, String)>,
}

struct SourceFile {
    path: PathBuf,
    text: Arc<[u8]>,
}

fn sources_of(value: &PlyValue, span: Span) -> Result<Sources, Diagnostic> {
    let files = field_of(value, "files", span)?
        .as_list(span, "files")?
        .iter()
        .map(|file| {
            Ok(SourceFile {
                path: PathBuf::from(field_of(file, "path", span)?.as_str(span, "a path")?),
                text: field_of(file, "text", span)?
                    .as_bytes(span, "a text")?
                    .clone(),
            })
        })
        .collect::<Result<Vec<SourceFile>, Diagnostic>>()?;
    let packages = field_of(value, "packages", span)?
        .as_list(span, "packages")?
        .iter()
        .map(|package| {
            Ok((
                PathBuf::from(field_of(package, "root", span)?.as_str(span, "a root")?),
                field_of(package, "digest", span)?
                    .as_str(span, "a digest")?
                    .to_string(),
            ))
        })
        .collect::<Result<Vec<(PathBuf, String)>, Diagnostic>>()?;
    Ok(Sources { files, packages })
}

/// A load never fails for its cache: a store that will not open is a load that neither reads nor
/// files one, and says so.
fn open_for_load(path: &Path) -> Result<Opened, Diagnostic> {
    open(path).map_err(|_| {
        let root = crate::load::project_root(path);
        Diagnostic::warning(
            codes::CACHE_UNREADABLE,
            format!("could not open the cache under `{}`", root.display()),
        )
        .note("every definition was checked, and nothing this run checked was recorded")
        .note("check the directory's permissions to get the front-end cache back")
    })
}

/// A value the front end filed that no longer reads as one, which is a slot absent.
fn unreadable_value(why: &str) -> Diagnostic {
    Diagnostic::warning(
        codes::CACHE_CORRUPT,
        format!("the front-end cache is corrupt: a filed value does not read back: {why}"),
    )
    .note("that entry is treated as absent, and what it answered for is checked again")
}

/// What the store holds for the files a load is about to read: every row filed for a definition
/// or a test they declared, whatever its hash — the front end takes a row only where the hash it
/// was filed under is still the item's, and walks the rest — and what the shipped modules were
/// when the store last filed them, if a different compiler filed them.
fn known(path: &Path, sources: &Sources) -> PlyValue {
    let mut store = match open_for_load(path) {
        Ok(opened) => opened,
        Err(warning) => return recorded(Vec::new(), Vec::new(), None, vec![warning]),
    };
    store.store.set_packages(sources.packages.clone());
    let mut defs = Vec::new();
    let mut tests = Vec::new();
    let mut unread: Option<String> = None;
    let mut read = |bytes: &[u8], into: &mut Vec<PlyValue>| match ply_eval::codec::decode(bytes) {
        Ok(value) => into.push(value),
        Err(why) => unread = unread.take().or(Some(why)),
    };
    for file in &sources.files {
        let Some(fingerprint) = store.store.fingerprint(&file.path) else {
            continue;
        };
        for entry in fingerprint.defs.iter().filter(|e| e.kind == DefKind::Fn) {
            if let Some(slot) = store.store.def_of(entry.hash, &entry.name) {
                read(&slot.value, &mut defs);
            }
        }
        for test in &fingerprint.tests {
            read(&test.row, &mut tests);
        }
    }
    let stdlib = moved_stdlib(&store.store);
    let mut warnings = store.warnings;
    warnings.extend(store.store.take_warnings());
    warnings.extend(unread.map(|why| unreadable_value(&why)));
    recorded(defs, tests, stdlib, warnings)
}

fn recorded(
    defs: Vec<PlyValue>,
    tests: Vec<PlyValue>,
    stdlib: Option<PlyValue>,
    warnings: Vec<Diagnostic>,
) -> PlyValue {
    record(vec![
        ("defs", PlyValue::list(defs)),
        ("tests", PlyValue::list(tests)),
        ("stdlib", option(stdlib)),
        ("warnings", diags_value(&warnings)),
    ])
}

/// The shipped modules' entries as they were filed, when the store was last filed by a compiler
/// whose shipped modules were different: the program says what that change reached.
fn moved_stdlib(store: &Store) -> Option<PlyValue> {
    let now = ply_std::digest_short();
    let was = store.stdlib_digest().filter(|was| *was != now)?;
    // Keyed, not placed: a shipped module's key is not relative to this run's root.
    let entries = store
        .source_keys()
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| ply_std::is_pseudo_path(path))
        .filter_map(|path| store.fingerprint(&path))
        .flat_map(|fingerprint| {
            fingerprint
                .defs
                .iter()
                .map(|entry| {
                    record(vec![
                        ("name", PlyValue::bytes(entry.name.as_str().as_bytes())),
                        ("hash", PlyValue::bytes(entry.hash.0)),
                    ])
                })
                .collect::<Vec<PlyValue>>()
        })
        .collect();
    Some(record(vec![
        ("was", PlyValue::str(&was)),
        ("now", PlyValue::str(&now)),
        ("entries", PlyValue::list(entries)),
    ]))
}

fn def_hash(value: &PlyValue, span: Span) -> Result<DefHash, Diagnostic> {
    let bytes = value.as_bytes(span, "a hash")?;
    <[u8; 32]>::try_from(&bytes[..]).map(DefHash).map_err(|_| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a filed hash is {} bytes rather than 32", bytes.len()),
        )
        .primary(span, "this is Ply's fault")
    })
}

fn symbol_at(value: &PlyValue, name: &str, span: Span) -> Result<Symbol, Diagnostic> {
    let bytes = field_of(value, name, span)?.as_bytes(span, name)?;
    Ok(Symbol::new(String::from_utf8_lossy(bytes)))
}

fn file_span(value: &PlyValue, span: Span) -> Result<FileSpan, Diagnostic> {
    let at = |name: &str| -> Result<u32, Diagnostic> {
        let n = field_of(value, name, span)?.as_int(span, name)?;
        Ok(u32::try_from(n).unwrap_or(0))
    };
    Ok(FileSpan {
        start: at("start")?,
        end: at("end")?,
    })
}

/// The front end's own value, as the bytes a slot keeps.
fn encoded(value: &PlyValue, span: Span) -> Result<Vec<u8>, Diagnostic> {
    ply_eval::codec::encode(value).map_err(|why| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a filed value is not plain data: {why}"),
        )
        .primary(span, "this is Ply's fault")
    })
}

fn entry_of(value: &PlyValue, span: Span) -> Result<DefEntry, Diagnostic> {
    let kind = match field_of(value, "kind", span)?
        .as_bytes(span, "a kind")?
        .as_ref()
    {
        b"fn" => DefKind::Fn,
        b"type" => DefKind::Type,
        _ => DefKind::Effect,
    };
    let members = field_of(value, "members", span)?
        .as_list(span, "members")?
        .iter()
        .map(|member| {
            Ok(Member {
                name: symbol_at(member, "name", span)?,
                span: file_span(field_of(member, "span", span)?, span)?,
            })
        })
        .collect::<Result<Vec<Member>, Diagnostic>>()?;
    Ok(DefEntry {
        name: symbol_at(value, "name", span)?,
        hash: def_hash(field_of(value, "hash", span)?, span)?,
        span: file_span(field_of(value, "span", span)?, span)?,
        kind,
        members,
    })
}

fn test_of(value: &PlyValue, span: Span) -> Result<TestEntry, Diagnostic> {
    Ok(TestEntry {
        name: String::from_utf8_lossy(field_of(value, "name", span)?.as_bytes(span, "a name")?)
            .into_owned(),
        hash: def_hash(field_of(value, "hash", span)?, span)?,
        nondet: field_of(value, "nondet", span)?.as_bool(span, "nondet")?,
        span: file_span(field_of(value, "span", span)?, span)?,
        row: encoded(field_of(value, "row", span)?, span)?,
    })
}

fn slot_of(value: &PlyValue, span: Span) -> Result<(DefHash, Slot), Diagnostic> {
    Ok((
        def_hash(field_of(value, "hash", span)?, span)?,
        Slot {
            name: symbol_at(value, "name", span)?,
            value: encoded(field_of(value, "interface", span)?, span)?,
        },
    ))
}

/// Files what an analysis published: each file's fingerprint under its path, each definition's
/// and declaration's value under its hash and name, and each body under its hash. A load of the
/// whole project also drops what no file of it names. Nothing here fails the run: the store's
/// trouble is warned about, and the next run does this work again.
fn file(
    path: &Path,
    sources: &Sources,
    filing: &PlyValue,
    whole: bool,
    span: Span,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    // What opening found, the read that came first has already said.
    let mut store = match open_for_load(path) {
        Ok(opened) => opened.store,
        Err(warning) => return Ok(vec![warning]),
    };
    store.set_packages(sources.packages.clone());

    for module in field_of(filing, "modules", span)?
        .as_list(span, "modules")?
        .iter()
    {
        let source = field_of(module, "source", span)?.as_int(span, "a source index")?;
        let Some(file) = usize::try_from(source)
            .ok()
            .and_then(|i| sources.files.get(i))
        else {
            continue;
        };
        let mut fingerprint = SourceFingerprint::new(ContentHash::of(&file.text));
        fingerprint.module =
            String::from_utf8_lossy(field_of(module, "name", span)?.as_bytes(span, "a module")?)
                .into_owned();
        for entry in field_of(module, "entries", span)?
            .as_list(span, "entries")?
            .iter()
        {
            fingerprint.defs.push(entry_of(entry, span)?);
        }
        for test in field_of(module, "tests", span)?
            .as_list(span, "tests")?
            .iter()
        {
            fingerprint.tests.push(test_of(test, span)?);
        }
        store.put_source(&file.path, fingerprint);
    }
    for def in field_of(filing, "defs", span)?
        .as_list(span, "defs")?
        .iter()
    {
        let (hash, slot) = slot_of(def, span)?;
        store.put_def(hash, slot);
    }
    for decl in field_of(filing, "decls", span)?
        .as_list(span, "decls")?
        .iter()
    {
        let (hash, slot) = slot_of(decl, span)?;
        store.put_decl(hash, slot);
    }
    for body in field_of(filing, "bodies", span)?
        .as_list(span, "bodies")?
        .iter()
    {
        let hash = def_hash(field_of(body, "hash", span)?, span)?;
        let bytes = field_of(body, "body", span)?.as_bytes(span, "a body")?;
        store.put_body(hash, DefBody::new(BODY_ENCODING, bytes.to_vec()));
    }
    // A shipped module no longer imported is pruned like any file that left the program.
    if whole {
        let keep: Vec<PathBuf> = sources.files.iter().map(|f| f.path.clone()).collect();
        store.prune(&keep);
    }
    store.set_stdlib_digest(ply_std::digest_short());
    let mut warnings = match store.flush() {
        Ok(()) => Vec::new(),
        // A flush writes every cache, so naming the one that failed would be a guess.
        Err(e) => vec![
            Diagnostic::warning(
                codes::CACHE_UNREADABLE,
                format!("could not update the cache: {e:#}"),
            )
            .note("this run is unaffected; the next one will do this work again"),
        ],
    };
    warnings.extend(store.take_warnings());
    Ok(warnings)
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
    } = open(&scope.path)?;
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
    } = open(&scope.path)?;

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
    } = open(&scope.path)?;
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

/// One cached name, gathered before anything is written so the two forms cannot disagree. What
/// the front end filed is handed over as it filed it, for the program to print.
struct Entry {
    name: String,
    kind: Kind,
    hash: String,
    file: String,
    location: Option<String>,
    stale: bool,
    filed: Filed,
    body: Option<(u32, usize)>,
    outcome: Option<Outcome>,
}

enum Kind {
    Def(DefKind),
    Test(bool),
}

/// `Unfiled` is a half-pruned cache, a slot written for other names, or one that no longer reads.
enum Filed {
    Definition(PlyValue),
    Declaration(PlyValue),
    Test(PlyValue),
    Unfiled,
}

fn matches(args: &InspectOptions) -> Result<Matches, Refused> {
    let Opened {
        mut store,
        mut warnings,
        ..
    } = open(&args.path)?;
    let found = store.lookup(&args.query);
    let mut unread = None;
    let entries = found
        .iter()
        .map(|f| entry_of_found(f, &store, &mut unread))
        .collect();
    warnings.extend(store.take_warnings());
    warnings.extend(unread.map(|why: String| unreadable_value(&why)));
    Ok(Matches {
        directory: shown(store.dir()),
        warnings,
        entries,
    })
}

fn decoded(bytes: &[u8], unread: &mut Option<String>) -> Option<PlyValue> {
    match ply_eval::codec::decode(bytes) {
        Ok(value) => Some(value),
        Err(why) => {
            unread.get_or_insert(why);
            None
        }
    }
}

fn entry_of_found(found: &Found, store: &Store, unread: &mut Option<String>) -> Entry {
    match found {
        Found::Def(def) => def_entry(def, store, unread),
        Found::Test(test) => test_entry(test, store, unread),
    }
}

fn def_entry(def: &FoundDef, store: &Store, unread: &mut Option<String>) -> Entry {
    let (location, stale) = locate(store, &def.path, def.span);
    let filed = match def.kind {
        DefKind::Fn => store
            .def_of(def.hash, &def.name)
            .and_then(|slot| decoded(&slot.value, unread))
            .map(Filed::Definition),
        DefKind::Type | DefKind::Effect => store
            .decl_of(def.hash, &def.name)
            .and_then(|slot| decoded(&slot.value, unread))
            .map(Filed::Declaration),
    };
    Entry {
        name: def.name.to_string(),
        kind: Kind::Def(def.kind),
        hash: def.hash.to_hex(),
        file: shown(&def.path),
        location,
        stale,
        filed: filed.unwrap_or(Filed::Unfiled),
        body: store.body(def.hash).map(|b| (b.encoding(), b.len())),
        outcome: None,
    }
}

fn test_entry(test: &FoundTest, store: &Store, unread: &mut Option<String>) -> Entry {
    let (location, stale) = locate(store, &test.path, test.span);
    Entry {
        name: test.name.clone(),
        kind: Kind::Test(test.nondet),
        hash: test.hash.to_hex(),
        file: shown(&test.path),
        location,
        stale,
        filed: decoded(&test.row, unread).map_or(Filed::Unfiled, Filed::Test),
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
        ("filed", filed_value(&e.filed)),
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

fn filed_value(filed: &Filed) -> PlyValue {
    match filed {
        Filed::Definition(value) => ctor(PAYLOAD, "FiledDefinition", vec![value.clone()]),
        Filed::Declaration(value) => ctor(PAYLOAD, "FiledDeclaration", vec![value.clone()]),
        Filed::Test(value) => ctor(PAYLOAD, "FiledTestRow", vec![value.clone()]),
        Filed::Unfiled => ctor(PAYLOAD, "Unfiled", Vec::new()),
    }
}

fn outcome_value(outcome: &Outcome) -> PlyValue {
    match outcome {
        Outcome::Pass => ctor(PAYLOAD, "Passed", Vec::new()),
        Outcome::Fail { message, .. } => ctor(PAYLOAD, "Failed", vec![PlyValue::str(message)]),
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
    let mut sources = ply_eval::SourceMap::new();
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

fn shown(path: &Path) -> String {
    path.display().to_string()
}

/// A byte count the program reads as an `Int`; no cache comes near `i64`.
fn size(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}
