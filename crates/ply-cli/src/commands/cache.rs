//! The cache's own interface.
//!
//! The front-end cache stopped being a file anybody could `cat`, and these
//! commands are what has to be worth more than that was. `inspect` is the one
//! that carries the trade: it prints a *resolved* type rather than a
//! serialization of one, which `cat` never did.

use super::common::{IND, diagnostic_json, emit_json, millis, plural, print_warnings};
use crate::cli::{CacheScope, InspectArgs};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_core::{Footprint, print_scheme, print_type};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_store::{
    CacheStats, CachedDecl, DeclBody, DefKind, FRONTEND_VERSION, FileSpan, Found, FoundDef,
    FoundTest, NameRef, Outcome, RUNTIME_VERSION, Store,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A store and how long it took to open it. The time is reported rather than
/// merely measured: `Store::open` under 5 ms at ten thousand definitions is the
/// claim the binary format was chosen for, and a number nobody can see is a
/// claim nobody can check.
struct Opened {
    store: Store,
    took: Duration,
}

pub fn stats(scope: &CacheScope, style: Style) -> i32 {
    let Opened { mut store, took } = match open(scope, "stats", style) {
        Ok(opened) => opened,
        Err(code) => return code,
    };
    let mut warnings = store.take_warnings();
    let notice = crate::migrate::notice(&store, &warnings);
    warnings.extend(notice);
    let stats = store.stats();

    if scope.json {
        emit_json(&json!({
            "command": "cache",
            "action": "stats",
            "ok": true,
            "exit_code": EXIT_OK,
            "runtime_version": RUNTIME_VERSION,
            "frontend_version": FRONTEND_VERSION,
            "root": store.root().display().to_string(),
            "directory": store.dir().display().to_string(),
            "results_file": store.path().display().to_string(),
            "frontend_file": store.frontend_path().display().to_string(),
            "frontend_data_file": store.frontend_data_path().display().to_string(),
            "open_ms": millis(took),
            "entries": stats.results,
            "definitions_seen": stats.definitions_seen,
            "results_bytes": stats.results_bytes,
            "prover_version": ply_store::PROVER_VERSION,
            "obligations": stats.obligations,
            "reviews": stats.reviews,
            "frontend": {
                "sources": stats.sources,
                "definitions": stats.defs,
                "declarations": stats.decls,
                "bodies": stats.bodies,
                "index_bytes": stats.index_bytes,
                "data_bytes": stats.data_bytes,
                "garbage_bytes": stats.garbage_bytes,
                "garbage_ratio": garbage_ratio(&stats),
                "compact_suggested": compact_suggested(&stats),
            },
            "warnings": warnings_json(&warnings),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    println!("{IND}{}", style.dim(&store.dir().display().to_string()));
    println!(
        "{IND}opened in {}",
        style.bold(&format!("{:.2}ms", millis(took)))
    );
    println!(
        "{IND}runtime {RUNTIME_VERSION} · {} cached {} · {} {} seen · {}",
        style.bold(&stats.results.to_string()),
        plural(stats.results, "result"),
        stats.definitions_seen,
        plural(stats.definitions_seen, "definition"),
        style.dim(&bytes(stats.results_bytes)),
    );
    println!(
        "{IND}prover {} · {} discharged {} · {} accepted {}",
        ply_store::PROVER_VERSION,
        style.bold(&stats.obligations.to_string()),
        plural(stats.obligations, "obligation"),
        stats.reviews,
        plural(stats.reviews, "review"),
    );
    println!(
        "{IND}front end {FRONTEND_VERSION} · {} {} · {} {} · {} {} · {} {}",
        style.bold(&stats.sources.to_string()),
        plural(stats.sources, "file"),
        stats.defs,
        plural(stats.defs, "definition"),
        stats.decls,
        plural(stats.decls, "declaration"),
        stats.bodies,
        plural(stats.bodies, "body"),
    );
    println!(
        "{IND}  index {} · data {} · reclaimable {}",
        style.dim(&bytes(stats.index_bytes)),
        style.dim(&bytes(stats.data_bytes)),
        match stats.garbage_bytes {
            Some(garbage) => style.dim(&format!(
                "{} ({:.0}%)",
                bytes(garbage),
                garbage_ratio(&stats).unwrap_or(0.0) * 100.0
            )),
            None => style.dim("— (this store cannot say what is unreachable)"),
        }
    );
    if compact_suggested(&stats) {
        println!(
            "{IND}  {}",
            style.yellow("more than half the data file is unreachable; run `ply cache compact`")
        );
    }
    // A hybrid program is a real configuration with a real hash, so it earns a
    // real cache entry — and a reader counting tests would otherwise read the
    // surplus as corruption.
    if stats.results > 0 {
        println!(
            "{IND}  {}",
            style.dim("a result entry is one proven configuration, not one test")
        );
    }
    EXIT_OK
}

pub fn compact(scope: &CacheScope, style: Style) -> i32 {
    let Opened { mut store, .. } = match open(scope, "compact", style) {
        Ok(opened) => opened,
        Err(code) => return code,
    };
    let mut warnings = store.take_warnings();
    let notice = crate::migrate::notice(&store, &warnings);
    warnings.extend(notice);

    // Compaction drops whatever the surviving files do not name, so a walk that
    // saw less than the whole project would delete work no error would report.
    let keep = match crate::load::ply_files(store.root()) {
        // A shipped module has no file on disk, so the walk cannot see it, and
        // its entry is live: this binary still ships it. Taken from what the
        // store already holds rather than from the whole table, so a project
        // that imports nothing from `std` keeps nothing extra and its
        // `files_kept` still counts its own files.
        Ok(mut keep) => {
            keep.extend(
                store
                    .source_paths()
                    .into_iter()
                    .filter(|p| ply_std::is_pseudo_path(p)),
            );
            keep
        }
        Err(e) => {
            let root = store.root().display().to_string();
            return fail(
                scope.json,
                "compact",
                store.dir(),
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("could not list the `.ply` files under `{root}`: {e}"),
                )
                .primary(Span::DUMMY, "nothing was dropped")
                .note("compaction needs to see every source file before it discards anything")
                .note("check the directory's permissions, then run it again"),
                &warnings,
                style,
            );
        }
    };

    let compaction = match store.compact(&keep) {
        Ok(compaction) => compaction,
        Err(e) => {
            let dir = store.dir().display().to_string();
            return fail(
                scope.json,
                "compact",
                store.dir(),
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("could not compact the cache at `{dir}`: {e:#}"),
                )
                .primary(Span::DUMMY, "the cache was left as it was")
                .note("check the directory's permissions, or run `ply cache clear` to start over"),
                &warnings,
                style,
            );
        }
    };
    warnings.extend(store.take_warnings());

    let dropped = compaction.dropped;
    let reclaimed = compaction
        .bytes_before
        .saturating_sub(compaction.bytes_after);

    if scope.json {
        emit_json(&json!({
            "command": "cache",
            "action": "compact",
            "ok": true,
            "exit_code": EXIT_OK,
            "directory": store.dir().display().to_string(),
            "files_kept": keep.len(),
            "dropped": {
                "sources": dropped.sources,
                "definitions": dropped.defs,
                "declarations": dropped.decls,
                "bodies": dropped.bodies,
            },
            "bytes_before": compaction.bytes_before,
            "bytes_after": compaction.bytes_after,
            "reclaimed_bytes": reclaimed,
            "results": store.stats().results,
            "warnings": warnings_json(&warnings),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    println!(
        "{IND}{} {}",
        style.green("compacted"),
        style.dim(&store.dir().display().to_string())
    );
    // Only when something was actually pruned: a compaction that reclaimed
    // superseded records without dropping a live one is the common case, and
    // four zeroes above the byte counts read as though it had done nothing.
    if dropped.sources + dropped.defs + dropped.decls + dropped.bodies > 0 {
        println!(
            "{IND}  dropped {} {} · {} {} · {} {} · {} {}",
            dropped.sources,
            plural(dropped.sources, "file"),
            dropped.defs,
            plural(dropped.defs, "definition"),
            dropped.decls,
            plural(dropped.decls, "declaration"),
            dropped.bodies,
            plural(dropped.bodies, "body"),
        );
    }
    if reclaimed > 0 {
        println!(
            "{IND}  {} → {} ({} reclaimed)",
            bytes(compaction.bytes_before),
            bytes(compaction.bytes_after),
            style.bold(&bytes(reclaimed)),
        );
    } else {
        println!(
            "{IND}  {} · {}",
            bytes(compaction.bytes_after),
            style.dim("nothing to reclaim")
        );
    }
    println!(
        "{IND}  {}",
        style.dim("the result cache is untouched, so no test re-runs")
    );
    EXIT_OK
}

pub fn inspect(args: &InspectArgs, style: Style) -> i32 {
    let scope = CacheScope {
        path: args.path.clone(),
        json: args.json,
    };
    let Opened { mut store, .. } = match open(&scope, "inspect", style) {
        Ok(opened) => opened,
        Err(code) => return code,
    };
    let mut warnings = store.take_warnings();
    let notice = crate::migrate::notice(&store, &warnings);
    warnings.extend(notice);

    let mut found = store.lookup(&args.query);
    found.sort_by_key(order_key);

    if found.is_empty() {
        let diagnostic = Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("nothing cached under `{}`", args.query),
        )
        .primary(Span::DUMMY, "no definition, declaration or test matched")
        .note("a definition appears here only after a run that checked it: try `ply check` first")
        .note("the query is a program-wide name, a simple name, or a hash prefix of 4+ hex digits");
        return fail(
            args.json,
            "inspect",
            store.dir(),
            diagnostic,
            &warnings,
            style,
        );
    }

    let entries: Vec<Entry> = found.iter().map(|f| Entry::of(f, &store)).collect();

    if args.json {
        emit_json(&json!({
            "command": "cache",
            "action": "inspect",
            "ok": true,
            "exit_code": EXIT_OK,
            "query": args.query,
            "directory": store.dir().display().to_string(),
            "matches": entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
            "warnings": warnings_json(&warnings),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        entry.print(style);
    }
    if entries.len() > 1 {
        println!();
        println!(
            "{IND}{}",
            style.dim(&format!(
                "{} entries matched `{}`",
                entries.len(),
                args.query
            ))
        );
    }
    EXIT_OK
}

// --- one inspected entry ----------------------------------------------------

/// What `inspect` prints, gathered before anything is written so that the human
/// and JSON forms cannot drift into disagreeing about what was found.
struct Entry {
    title: String,
    hash: String,
    kind: &'static str,
    path: PathBuf,
    /// `None` once the file has been edited; see [`locate`].
    location: Option<String>,
    stale: bool,
    interface: Interface,
    witness: Vec<NameRef>,
    body: Option<(u32, usize)>,
    outcome: Option<Outcome>,
    /// Held once, as atoms: the human form renders it and the JSON form lists
    /// it, and a second copy of the rendering is a second thing to keep true.
    footprint: Option<Footprint>,
}

/// One variant per kind rather than a bag of labelled strings, so that a JSON
/// consumer reading `interface.variants` always finds an array and
/// `interface.type` always finds a string. A shape that changes with how many
/// constructors a type happens to have is not a schema.
enum Interface {
    Fn {
        ty: String,
    },
    Type {
        parameters: usize,
        variants: Vec<String>,
    },
    Effect {
        nondet: bool,
        operations: Vec<String>,
    },
    Test {
        nondet: bool,
    },
    /// The fingerprint names a definition whose interface is not in the store —
    /// a half-pruned cache, or a hash whose slots were written for other names.
    Absent,
}

impl Entry {
    fn of(found: &Found, store: &Store) -> Entry {
        match found {
            Found::Def(def) => Entry::of_def(def, store),
            Found::Test(test) => Entry::of_test(test, store),
        }
    }

    fn of_def(def: &FoundDef, store: &Store) -> Entry {
        let (location, stale) = locate(store, &def.path, def.span);
        let mut witness = Vec::new();
        let mut footprint = None;

        let interface = match def.kind {
            DefKind::Fn => match store.def_of(def.hash, &def.name) {
                Some(cached) => {
                    witness = cached.names.clone();
                    footprint = Some(cached.footprint.clone());
                    Interface::Fn {
                        ty: print_scheme(&cached.scheme),
                    }
                }
                None => Interface::Absent,
            },
            DefKind::Type | DefKind::Effect => match store.decl_of(def.hash, &def.name) {
                Some(cached) => {
                    witness = cached.names.clone();
                    declaration(&cached, &variant_names(store, def))
                }
                None => Interface::Absent,
            },
        };

        Entry {
            title: def.name.to_string(),
            hash: def.hash.short(),
            kind: kind_of(def.kind),
            path: def.path.clone(),
            location,
            stale,
            interface,
            witness,
            body: store.body(def.hash).map(|b| (b.encoding(), b.len())),
            outcome: None,
            footprint,
        }
    }

    fn of_test(test: &FoundTest, store: &Store) -> Entry {
        let (location, stale) = locate(store, &test.path, test.span);
        Entry {
            title: format!("{:?}", test.name),
            hash: test.hash.short(),
            kind: if test.nondet { "test/nondet" } else { "test" },
            path: test.path.clone(),
            location,
            stale,
            interface: Interface::Test {
                nondet: test.nondet,
            },
            witness: Vec::new(),
            body: store.body(test.hash).map(|b| (b.encoding(), b.len())),
            outcome: store.get(test.hash),
            footprint: Some(test.footprint.clone()),
        }
    }

    fn result_line(&self) -> String {
        match (&self.outcome, &self.interface) {
            (Some(Outcome::Pass), _) => "passed — this hash will not re-run".to_string(),
            (Some(Outcome::Fail { message, .. }), _) => format!("failed: {message}"),
            (None, Interface::Test { nondet: true }) => {
                "— `test/nondet` is never cached".to_string()
            }
            (None, Interface::Test { nondet: false }) => {
                "— not proven at this hash, so it runs next time".to_string()
            }
            (None, _) => "— not a test".to_string(),
        }
    }

    fn print(&self, style: Style) {
        let where_ = self.location.clone().unwrap_or_else(|| {
            let path = self.path.display().to_string();
            if self.stale {
                format!("{path} (edited since)")
            } else {
                path
            }
        });
        println!(
            "{IND}{}  {}  {}  {}",
            style.bold(&self.title),
            style.dim(&self.hash),
            self.kind,
            style.dim(&where_),
        );
        for (label, value) in self.rows() {
            println!("{IND}  {label:<10} {value}");
        }
        println!("{IND}  {:<10} {}", "witness", self.witness_line());
        println!("{IND}  {:<10} {}", "body", self.body_line());
        println!("{IND}  {:<10} {}", "result", self.result_line());
    }

    /// The label is blank on a continuation line so a multi-variant type reads
    /// as one block rather than as repeated keys.
    fn rows(&self) -> Vec<(&'static str, String)> {
        let listed = |label: &'static str, values: &[String]| {
            values
                .iter()
                .enumerate()
                .map(|(i, v)| (if i == 0 { label } else { "" }, v.clone()))
                .collect::<Vec<_>>()
        };
        let mut rows = match &self.interface {
            Interface::Fn { ty } => vec![("type", ty.clone())],
            Interface::Type {
                parameters,
                variants,
            } => {
                let mut rows = vec![("parameters", parameters.to_string())];
                rows.extend(listed("variants", variants));
                rows
            }
            Interface::Effect { nondet, operations } => {
                let mut rows = vec![("nondet", yes_no(*nondet))];
                rows.extend(listed("operations", operations));
                rows
            }
            Interface::Test { nondet } => vec![("nondet", yes_no(*nondet))],
            Interface::Absent => vec![(
                "interface",
                "— nothing cached under this hash for this name".to_string(),
            )],
        };
        if let Some(footprint) = &self.footprint {
            rows.push(("footprint", footprint.to_string()));
        }
        rows
    }

    fn witness_line(&self) -> String {
        if self.witness.is_empty() {
            return "— mentions nothing this cache had to record".to_string();
        }
        self.witness
            .iter()
            .map(|n| format!("{} → {}", n.name, n.hash.short()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn body_line(&self) -> String {
        match self.body {
            Some((encoding, len)) => format!("{len} {} (encoding {encoding})", plural(len, "byte")),
            None => "— not stored, so this definition cannot join a hybrid".to_string(),
        }
    }

    fn to_json(&self) -> Value {
        let interface = match &self.interface {
            Interface::Fn { ty } => json!({ "type": ty }),
            Interface::Type {
                parameters,
                variants,
            } => json!({ "parameters": parameters, "variants": variants }),
            Interface::Effect { nondet, operations } => {
                json!({ "nondet": nondet, "operations": operations })
            }
            Interface::Test { nondet } => json!({ "nondet": nondet }),
            Interface::Absent => Value::Null,
        };
        json!({
            "name": self.title,
            "hash": self.hash,
            "kind": self.kind,
            "file": self.path.display().to_string(),
            "location": self.location,
            "stale": self.stale,
            "interface": interface,
            "footprint": self.footprint.as_ref().map(|f| f.atoms().map(|a| a.to_string()).collect::<Vec<_>>()),
            "witness": self.witness.iter().map(|n| json!({
                "name": n.name,
                "hash": n.hash.to_hex(),
            })).collect::<Vec<_>>(),
            "body": self.body.map(|(encoding, len)| json!({
                "encoding": encoding,
                "bytes": len,
            })),
            "result": self.result_line(),
        })
    }
}

/// A constructor's *name* is not in the interface a hash is keyed by — two types
/// that differ only by their variants' names are one computation — so it comes
/// from the declaring file's fingerprint, where variants are aligned by
/// position. Without it this prints `(Int)` and nobody can tell which variant.
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

/// Rendered here rather than by `Display` on the stored shapes: an operation's
/// `[r]` and a variant's field list are presentation, and the store has no
/// business knowing how a person likes to read them.
fn declaration(cached: &CachedDecl, variants: &[Symbol]) -> Interface {
    match &cached.body {
        DeclBody::Type { arity, ctors } => Interface::Type {
            parameters: *arity,
            variants: ctors
                .iter()
                .enumerate()
                .map(|(i, ctor)| {
                    let name = match variants.get(i) {
                        Some(name) => name.to_string(),
                        None => print_scheme(&ctor.scheme),
                    };
                    if ctor.fields.is_empty() {
                        name
                    } else {
                        let fields: Vec<String> = ctor.fields.iter().map(print_type).collect();
                        format!("{name}({})", fields.join(", "))
                    }
                })
                .collect(),
        },
        DeclBody::Effect { nondet, ops } => Interface::Effect {
            nondet: *nondet,
            operations: ops
                .iter()
                .map(|op| {
                    let params: Vec<String> = op.params.iter().map(print_type).collect();
                    let resource = if op.resource_param { "[r]" } else { "" };
                    format!(
                        "{} {}{resource}({}) -> {}",
                        op.mode.as_str(),
                        op.name,
                        params.join(", "),
                        print_type(&op.ret),
                    )
                })
                .collect(),
        },
    }
}

fn yes_no(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_string()
}

fn kind_of(kind: DefKind) -> &'static str {
    match kind {
        DefKind::Fn => "fn",
        DefKind::Type => "type",
        DefKind::Effect => "effect",
    }
}

/// A stored span is a byte range into the file *as it was cached*, so a line and
/// column are only meaningful while the file still holds those bytes.
///
/// The offset is checked against the fingerprint's content hash rather than
/// against the file's length: an edit that happens to preserve the length would
/// otherwise yield a confident, wrong position, and a wrong line number is worse
/// than none. Returns `(position, stale)`.
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
    let mut sources = SourceMap::new();
    let id = sources.add(path, text);
    let Some(file) = sources.get(id) else {
        return (None, true);
    };
    let (line, column) = file.line_col(span.start);
    (Some(format!("{}:{line}:{column}", path.display())), false)
}

/// Total and independent of the store's iteration order, so two runs over one
/// cache print the same entries in the same order.
fn order_key(found: &Found) -> (String, String, String) {
    let (name, hash) = match found {
        Found::Def(d) => (d.name.to_string(), d.hash),
        Found::Test(t) => (t.name.clone(), t.hash),
    };
    (name, hash.to_hex(), found.path().display().to_string())
}

// --- shared -----------------------------------------------------------------

pub fn clear(scope: &CacheScope, style: Style) -> i32 {
    let Opened { mut store, .. } = match open(scope, "clear", style) {
        Ok(opened) => opened,
        Err(code) => return code,
    };
    let warnings = store.take_warnings();
    let before = store.len();

    if let Err(e) = store.clear() {
        let dir = store.dir().display().to_string();
        return fail(
            scope.json,
            "clear",
            store.dir(),
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not clear the cache at `{dir}`: {e:#}"),
            )
            .primary(Span::DUMMY, "the cache was left as it was")
            .note("check the directory's permissions, or delete it by hand"),
            &warnings,
            style,
        );
    }

    if scope.json {
        emit_json(&json!({
            "command": "cache",
            "action": "clear",
            "ok": true,
            "exit_code": EXIT_OK,
            "directory": store.dir().display().to_string(),
            "cleared": before,
            "warnings": warnings_json(&warnings),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    println!(
        "{IND}{} {before} cached {} from {}",
        style.green("cleared"),
        plural(before, "result"),
        style.dim(&store.dir().display().to_string())
    );
    EXIT_OK
}

/// Every failure inside a cache command reports the same way, so an agent can
/// key off `action` and `exit_code` without knowing which one it asked for.
fn fail(
    json: bool,
    action: &str,
    dir: &Path,
    diagnostic: Diagnostic,
    warnings: &[Diagnostic],
    style: Style,
) -> i32 {
    if json {
        emit_json(&json!({
            "command": "cache",
            "action": action,
            "ok": false,
            "exit_code": EXIT_COMPILE_ERROR,
            "directory": dir.display().to_string(),
            "diagnostics": [diagnostic_json(&diagnostic, &SourceMap::new())],
            "warnings": warnings_json(warnings),
        }));
    } else {
        print_warnings(warnings, style);
        super::common::print_diagnostics(
            std::slice::from_ref(&diagnostic),
            &SourceMap::new(),
            style,
        );
    }
    EXIT_COMPILE_ERROR
}

fn open(scope: &CacheScope, action: &str, style: Style) -> Result<Opened, i32> {
    let root = crate::load::project_root(&scope.path);
    let started = std::time::Instant::now();
    match Store::open(&root) {
        Ok(store) => Ok(Opened {
            store,
            took: started.elapsed(),
        }),
        Err(e) => {
            let diagnostic = Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not open a cache under `{}`: {e:#}", root.display()),
            )
            .primary(Span::DUMMY, "the cache directory is unusable")
            .note("pass the directory the cache belongs to; the default is `.`");

            if scope.json {
                emit_json(&json!({
                    "command": "cache",
                    "action": action,
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": [diagnostic_json(&diagnostic, &SourceMap::new())],
                }));
            } else {
                super::common::print_diagnostics(
                    std::slice::from_ref(&diagnostic),
                    &SourceMap::new(),
                    style,
                );
            }
            Err(EXIT_COMPILE_ERROR)
        }
    }
}

fn garbage_ratio(stats: &CacheStats) -> Option<f64> {
    let garbage = stats.garbage_bytes?;
    if stats.data_bytes == 0 {
        return Some(0.0);
    }
    Some(garbage as f64 / stats.data_bytes as f64)
}

/// Suggested, never done: dropping an interface costs a recheck, and the
/// definitions most likely to be garbage — a commented-out function, the other
/// side of a branch — are the ones most likely to come back.
fn compact_suggested(stats: &CacheStats) -> bool {
    garbage_ratio(stats).is_some_and(|ratio| ratio > 0.5)
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn warnings_json(warnings: &[Diagnostic]) -> Value {
    let sources = SourceMap::new();
    Value::Array(
        warnings
            .iter()
            .map(|w| diagnostic_json(w, &sources))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_hash::DefHash;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    fn scope(dir: &Path) -> CacheScope {
        CacheScope {
            path: dir.to_path_buf(),
            json: false,
        }
    }

    /// Populates the front-end cache the only way anything ever should: by
    /// running the real front end over a real project.
    fn checked(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = project(files);
        let mut store = Store::open(dir.path()).unwrap();
        crate::driver::load_incremental(dir.path(), &mut store).unwrap();
        store.flush().unwrap();
        dir
    }

    #[test]
    fn clearing_empties_a_populated_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put(DefHash([7u8; 32]), Outcome::Pass);
        store.flush().unwrap();
        assert_eq!(Store::open(dir.path()).unwrap().len(), 1);

        assert_eq!(clear(&scope(dir.path()), Style::plain()), EXIT_OK);
        assert_eq!(Store::open(dir.path()).unwrap().len(), 0);
    }

    #[test]
    fn stats_on_a_directory_with_no_cache_creates_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(stats(&scope(dir.path()), Style::plain()), EXIT_OK);
        assert!(dir.path().join(ply_store::CACHE_DIR_NAME).is_dir());
    }

    #[test]
    fn a_corrupt_cache_degrades_to_empty_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        Store::open(dir.path()).unwrap();
        let path = dir.path().join(ply_store::CACHE_DIR_NAME);
        std::fs::write(path.join("results.json"), "{ not json at all").unwrap();

        let mut store = Store::open(dir.path()).unwrap();
        assert_eq!(store.len(), 0);
        let warnings = store.take_warnings();
        assert!(!warnings.is_empty());
        assert!(!warnings_json(&warnings).as_array().unwrap().is_empty());
    }

    #[test]
    fn inspect_finds_a_function_by_simple_name_and_prints_a_resolved_type() {
        let dir = checked(&[(
            "user.ply",
            "effect db {\n  read all[t]() -> List<Int>\n}\n\
             fn active(n: Int) -> List<Int> / {db.read[users]} = db.all[users]()\n",
        )]);
        let store = Store::open(dir.path()).unwrap();
        let found = store.lookup("active");
        assert_eq!(found.len(), 1, "expected one match, got {found:?}");

        let entry = Entry::of(&found[0], &store);
        assert_eq!(entry.title, "user.active");
        assert_eq!(entry.kind, "fn");
        assert_eq!(
            entry.rows(),
            [
                (
                    "type",
                    "(Int) -> List<Int> / {user.db.read[users]}".to_string()
                ),
                ("footprint", "{user.db.read[users]}".to_string()),
            ],
            "a serialized scheme is exactly what this command exists not to print"
        );
        assert!(entry.location.as_deref().unwrap().contains("user.ply:4:"));
        assert!(entry.result_line().contains("not a test"));
    }

    #[test]
    fn inspect_matches_a_hash_prefix_and_agrees_with_the_name_it_found() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
        let store = Store::open(dir.path()).unwrap();
        let by_name = store.lookup("one");
        assert_eq!(by_name.len(), 1);

        let prefix = &by_name[0].hash().to_hex()[..6];
        let by_hash = store.lookup(prefix);
        assert_eq!(by_hash.len(), 1);
        assert_eq!(by_hash[0].hash(), by_name[0].hash());
        assert_eq!(
            Entry::of(&by_hash[0], &store).title,
            Entry::of(&by_name[0], &store).title
        );
    }

    /// Three characters is not a hash prefix, and a name that happens to be hex
    /// still has to match as a name.
    #[test]
    fn a_query_shorter_than_four_characters_is_never_read_as_a_hash() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
        let store = Store::open(dir.path()).unwrap();
        let hex = store.lookup("one")[0].hash().to_hex();
        assert!(store.lookup(&hex[..3]).is_empty());
        assert!(!store.lookup(&hex[..4]).is_empty());
    }

    #[test]
    fn inspect_prints_an_effect_declaration_operation_by_operation() {
        let dir = checked(&[(
            "store.ply",
            "nondet effect wall {\n  read now() -> Int\n  write set[c](t: Int) -> Unit\n}\n",
        )]);
        let store = Store::open(dir.path()).unwrap();
        let found = store.lookup("wall");
        assert_eq!(found.len(), 1);

        let entry = Entry::of(&found[0], &store);
        assert_eq!(entry.kind, "effect");
        let Interface::Effect { nondet, operations } = &entry.interface else {
            panic!("expected an effect declaration");
        };
        assert!(nondet);
        assert!(operations.iter().any(|o| o == "read now() -> Int"));
        assert!(
            operations.iter().any(|o| o == "write set[r](Int) -> Unit"),
            "a resource-parameterized operation must say so: {operations:?}"
        );

        // One operation or ten, `operations` is an array either way.
        assert_eq!(
            entry.to_json()["interface"]["operations"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(entry.to_json()["interface"]["nondet"], true);
    }

    #[test]
    fn inspect_reports_a_test_with_its_cached_outcome() {
        let dir = checked(&[("m.ply", "test \"one is one\" { assert_eq(1, 1) }\n")]);
        let store = Store::open(dir.path()).unwrap();
        let found = store.lookup("one is one");
        assert_eq!(found.len(), 1, "got {found:?}");

        let entry = Entry::of(&found[0], &store);
        assert_eq!(entry.kind, "test");
        assert!(entry.result_line().contains("not proven"));

        let hash = found[0].hash();
        let mut store = Store::open(dir.path()).unwrap();
        store.put(hash, Outcome::Pass);
        store.flush().unwrap();

        let store = Store::open(dir.path()).unwrap();
        let entry = Entry::of(&store.lookup("one is one")[0], &store);
        assert!(
            entry.result_line().contains("passed"),
            "{}",
            entry.result_line()
        );
    }

    /// Two modules declaring one simple name is an honest ambiguity — the store
    /// holds no namespace that could pick between them — so both are printed,
    /// in an order that must not depend on how the store iterates.
    #[test]
    fn a_simple_name_declared_twice_yields_both_in_a_stable_order() {
        let dir = checked(&[
            ("alpha.ply", "fn shared() -> Int = 1\n"),
            ("beta.ply", "fn shared() -> Int = 2\n"),
        ]);
        let store = Store::open(dir.path()).unwrap();
        let mut found = store.lookup("shared");
        assert_eq!(found.len(), 2);
        found.sort_by_key(order_key);
        let names: Vec<String> = found.iter().map(|f| Entry::of(f, &store).title).collect();
        assert_eq!(names, ["alpha.shared", "beta.shared"]);

        let mut again = store.lookup("shared");
        again.sort_by_key(order_key);
        assert_eq!(
            again.iter().map(order_key).collect::<Vec<_>>(),
            found.iter().map(order_key).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_edited_file_withholds_a_line_and_column_rather_than_guessing() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\nfn two() -> Int = 2\n")]);
        let store = Store::open(dir.path()).unwrap();
        let found = store.lookup("two");
        assert!(Entry::of(&found[0], &store).location.is_some());

        std::fs::write(dir.path().join("m.ply"), "fn one() -> Int = 1\n").unwrap();
        let entry = Entry::of(&found[0], &store);
        assert!(entry.stale);
        assert_eq!(entry.location, None);
    }

    /// The case a length check cannot catch. Every stored offset is still in
    /// bounds here, so a position would be produced and would be wrong.
    #[test]
    fn an_edit_that_preserves_the_length_is_still_stale() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\nfn two() -> Int = 2\n")]);
        let store = Store::open(dir.path()).unwrap();
        let found = store.lookup("two");
        assert!(Entry::of(&found[0], &store).location.is_some());

        let same_length = "fn ONE() -> Int = 7\nfn two() -> Int = 2\n";
        std::fs::write(dir.path().join("m.ply"), same_length).unwrap();
        let entry = Entry::of(&found[0], &store);
        assert!(
            entry.stale,
            "a same-length edit must not pass for unchanged"
        );
        assert_eq!(entry.location, None);
    }

    #[test]
    fn inspect_of_nothing_is_e0101_and_exits_two() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
        let args = InspectArgs {
            query: "no_such_definition".into(),
            path: dir.path().to_path_buf(),
            json: false,
        };
        assert_eq!(inspect(&args, Style::plain()), EXIT_COMPILE_ERROR);
    }

    #[test]
    fn compact_keeps_what_the_project_still_declares() {
        let dir = checked(&[
            ("keep.ply", "fn kept() -> Int = 1\n"),
            ("gone.ply", "fn dropped() -> Int = 2\n"),
        ]);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.stats().sources, 2);

        std::fs::remove_file(dir.path().join("gone.ply")).unwrap();
        assert_eq!(compact(&scope(dir.path()), Style::plain()), EXIT_OK);

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.stats().sources, 1);
        assert!(!store.lookup("kept").is_empty());
        assert!(store.lookup("dropped").is_empty());
    }

    /// Compaction touches the front end only. A result is a claim about a hash,
    /// and no source file being deleted makes that claim false.
    #[test]
    fn compact_never_drops_a_result() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
        let mut store = Store::open(dir.path()).unwrap();
        store.put(DefHash([3u8; 32]), Outcome::Pass);
        store.flush().unwrap();

        std::fs::remove_file(dir.path().join("m.ply")).unwrap();
        assert_eq!(compact(&scope(dir.path()), Style::plain()), EXIT_OK);

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.stats().sources, 0);
    }

    #[test]
    fn compaction_is_reported_even_when_it_reclaims_nothing() {
        let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
        assert_eq!(compact(&scope(dir.path()), Style::plain()), EXIT_OK);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.stats().sources, 1, "a live file must survive");
    }

    #[test]
    fn a_half_wasted_data_file_is_where_compaction_starts_being_suggested() {
        let mut stats = CacheStats {
            data_bytes: 1000,
            garbage_bytes: Some(400),
            ..CacheStats::default()
        };
        assert!(!compact_suggested(&stats));
        stats.garbage_bytes = Some(501);
        assert!(compact_suggested(&stats));

        stats.garbage_bytes = None;
        assert!(
            !compact_suggested(&stats),
            "an unmeasurable ratio is not a reason to suggest anything"
        );
        assert_eq!(garbage_ratio(&stats), None);
    }

    #[test]
    fn sizes_are_readable_rather_than_raw() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1024), "1.00 KB");
        assert_eq!(bytes(12_750_000), "12.16 MB");
    }
}
