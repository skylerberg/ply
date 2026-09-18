//! What holds the port without the reference: the goldens, and the door into the port.
//!
//! These moved out of `ply-compiler-diff` with the five suites that use them (ADR 0052 §2).
//! That crate still holds the parser and lexer differentials, which read the Rust chain and
//! retire with `ply-syntax`; nothing here does. `golden::dir()` resolves against this crate's
//! own manifest directory, so the goldens sit beside the suites that read them.

#![allow(dead_code)]

/// One dump beside every input, as files under `fixtures/goldens/<phase>/`: the specification the
/// port is held to, in the tree, so that it survives the Rust reference's retirement
/// (ADR 0050 §2).
///
/// The golden has to exist and the port has to agree with it. That is the whole check: nothing
/// recomputes a reference dump to compare against, so the goldens are what the phase means.
/// With `PLY_DIFF_BLESS` set, [`golden::check`] rewrites the golden from the **port's** answer --
/// it used to take the reference's -- which makes blessing a deliberate act of moving the
/// specification rather than of re-deriving it from a second implementation.
pub mod golden {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    pub fn dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/goldens")
    }

    pub fn blessing() -> bool {
        std::env::var_os("PLY_DIFF_BLESS").is_some()
    }

    /// Where `phase`'s golden for `name` lives, and which record of it: a bundle's records,
    /// named `<bundle>#<i>`, share one file, `<phase>/<bundle>.dumps`, one record per `%%% <i>`
    /// line; every other input has `<phase>/<name>.dump` to itself. Characters a file name cannot
    /// carry portably are written as `_`.
    pub fn place(phase: &str, name: &str) -> (PathBuf, Option<usize>) {
        let safe = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        let base = dir().join(safe(phase));
        match name.rsplit_once('#') {
            Some((bundle, index)) if index.bytes().all(|b| b.is_ascii_digit()) => (
                base.join(format!("{}.dumps", safe(bundle))),
                Some(index.parse().expect("digits")),
            ),
            _ => (base.join(format!("{}.dump", safe(name))), None),
        }
    }

    /// Holds `port` to the golden, or rewrites the golden from it when blessing. `diff` is the
    /// phase's own first-difference report, `diff(want, got)`.
    pub fn check(
        phase: &str,
        name: &str,
        port: &str,
        diff: impl Fn(&str, &str) -> Option<String>,
    ) -> Result<(), String> {
        let (path, index) = place(phase, name);
        if blessing() {
            write(&path, index, port);
            return Ok(());
        }
        let Some(golden) = read(&path, index) else {
            return Err(format!(
                "no golden for {name} at {}; run this test with PLY_DIFF_BLESS=1 to write it",
                path.display()
            ));
        };
        if let Some(report) = diff(&golden, port) {
            return Err(format!(
                "the port disagrees with the golden on {name}:\n{report}"
            ));
        }
        Ok(())
    }

    fn read(path: &Path, index: Option<usize>) -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        let Some(index) = index else {
            return Some(text);
        };
        let mut found: Option<String> = None;
        for line in text.split_inclusive('\n') {
            if let Some(n) = line.strip_prefix("%%% ") {
                if found.is_some() {
                    break;
                }
                if n.trim().parse::<usize>().ok() == Some(index) {
                    found = Some(String::new());
                }
            } else if let Some(f) = found.as_mut() {
                f.push_str(line);
            }
        }
        found.map(|mut f| {
            if f.ends_with('\n') {
                f.pop();
            }
            f
        })
    }

    /// The first record a bless writes to a bundle's file in this process starts it afresh, and
    /// the rest append, so a bless is the run's own order and nothing older survives in it.
    fn write(path: &Path, index: Option<usize>, text: &str) {
        static STARTED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{}: {e}", parent.display()));
        }
        let Some(index) = index else {
            std::fs::write(path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            return;
        };
        let mut started = STARTED.lock().unwrap();
        let fresh = started
            .get_or_insert_with(HashSet::new)
            .insert(path.to_path_buf());
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(!fresh)
            .truncate(fresh)
            .open(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        use std::io::Write as _;
        write!(file, "%%% {index}\n{text}\n").unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
}

/// The port, entered in-process: the self-hosted compiler as the binary carries it, compiled,
/// each phase called with its input and answering the dump the reference side is compared to.
///
/// The bundle is the compiler, and the fixpoint test in `crates/ply-codegen-tests` is what says
/// it was emitted from the sources in the tree; a working copy is entered through
/// `PLY_C_EMITTER=ply:<dir>` once `stage` has bootstrapped it.
pub mod port {
    use ply_eval::{Fields, Value};
    use ply_span::Symbol;
    use std::sync::Arc;

    /// Enters `name` -- `module.function`, as the sources spell it -- and answers the string it
    /// returned. A raise, a missing entry or a non-string answer is the harness's own failure and
    /// panics with the reason.
    pub fn call(name: &str, args: &[Value]) -> String {
        ply_codegen::c::producer::ensure_default();
        match ply_codegen::c::producer::call(name, args) {
            Ok(Value::Str(ref s)) => s.to_string(),
            Ok(other) => panic!(
                "`{name}` answered a {} rather than a string",
                other.type_name()
            ),
            Err(e) => panic!("{e:#}"),
        }
    }

    /// A phase over one input: `name(src: Bytes) -> String`.
    pub fn dump(name: &str, src: &[u8]) -> String {
        call(name, &[Value::bytes(src)])
    }

    /// `resolve.Source`, the module record every whole-program phase takes a list of.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn source(name: &str, src: &str) -> Value {
        Value::Record(Arc::new(Fields::from_unsorted(vec![
            (Symbol::new("name"), Value::bytes(name.as_bytes())),
            (Symbol::new("src"), Value::bytes(src.as_bytes())),
        ])))
    }

    /// A phase over a program: `name(sources: List<Source>) -> String`.
    pub fn dump_program(name: &str, modules: &[(String, String)]) -> String {
        let sources = modules.iter().map(|(n, s)| source(n, s)).collect();
        call(name, &[Value::list(sources)])
    }

    pub fn bytes_list(items: &[String]) -> Value {
        Value::list(items.iter().map(|s| Value::bytes(s.as_bytes())).collect())
    }
}

/// The items at positions `index`, `index + of`, `index + 2·of`, …: one round-robin part of a
/// corpus, for a differential too long to be one test. CI's partitions are bounded by their
/// slowest single test, and dealing a corpus this way keeps every part the same shape.
pub fn part<T: Clone>(items: &[T], index: usize, of: usize) -> Vec<T> {
    items
        .iter()
        .enumerate()
        .filter(|(i, _)| i % of == index)
        .map(|(_, item)| item.clone())
        .collect()
}

/// A program bundle: programs separated by a line holding exactly `%%%`, modules within one by a
/// line holding exactly `%%`, and each module's first line its dotted name.
pub fn programs(text: &str) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    // Everything before the first separator is the bundle's header, not a program.
    for chunk in text.split("\n%%%\n").skip(1) {
        let chunk = chunk.trim_start_matches('\n');
        if chunk.trim().is_empty() {
            continue;
        }
        let mut modules = Vec::new();
        for m in chunk.split("\n%%\n") {
            let (name, src) = m.split_once('\n').unwrap_or((m, ""));
            modules.push((name.trim().to_string(), src.to_string()));
        }
        out.push(modules);
    }
    out
}

/// The dump as a list of records, for a diff that names the first disagreement instead of printing
/// two multi-megabyte strings.
pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// The fixtures in a bundle file, in order.
pub fn bundle(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "%%" {
            if let Some(c) = cur.take() {
                out.push(strip_one_newline(c));
            }
            cur = Some(String::new());
        } else if let Some(c) = cur.as_mut() {
            c.push_str(line);
        }
    }
    if let Some(c) = cur {
        out.push(strip_one_newline(c));
    }
    out
}

fn strip_one_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

/// The compiled compiler's cost, held to `benches/compiled-compiler.json` (ADR 0051 §2): what
/// its entries allocated and recycled, which do not vary with a machine, and the most chunk
/// bytes one held, which varies with how the chunks grew.
pub mod census {
    use std::path::PathBuf;

    pub const FILE: &str = "benches/compiled-compiler.json";

    fn path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the crate lives two levels below the repository root")
            .join(FILE)
    }

    /// Holds the thread's census since the last reset to the file's entry for `key`, within one
    /// per cent on the counts and a quarter on the chunk bytes. A missing entry fails with the
    /// reading, which is what the file is written from.
    pub fn hold(key: &str, lines: usize) -> Result<(), String> {
        let got = ply_codegen::c::producer::census();
        let reading = serde_json::json!({
            "entries": got.entries,
            "allocated": got.allocated,
            "recycled": got.recycled,
            "chunk_bytes": got.chunk_bytes,
            "source_lines": lines,
        });
        let text = std::fs::read_to_string(path()).unwrap_or_else(|_| "{}".to_string());
        let file: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} does not parse: {e}", path().display()))?;
        let Some(want) = file.get(key) else {
            return Err(format!(
                "{} has no entry `{key}`; this reading, from the run that asked, is what it takes:\n  \"{key}\": {reading}",
                path().display()
            ));
        };
        let field = |name: &str| -> Result<f64, String> {
            want.get(name)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| format!("`{key}` in {} has no `{name}`", path().display()))
        };
        for (name, measured, band) in [
            ("entries", got.entries as f64, 0.0),
            ("allocated", got.allocated as f64, 0.01),
            ("recycled", got.recycled as f64, 0.01),
            ("chunk_bytes", got.chunk_bytes as f64, 0.25),
        ] {
            let ceiling = field(name)?;
            if measured > ceiling * (1.0 + band) {
                return Err(format!(
                    "`{key}` in {} caps {name} at {ceiling:.0} and this tree reads {measured:.0}; lower the reading, or raise the entry if the cost is meant:\n  \"{key}\": {reading}",
                    path().display()
                ));
            }
        }
        println!("  {key}: {reading}");
        Ok(())
    }
}
