#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits at <root>/crates/ply-codegen-tests")
        .to_path_buf()
}

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// One golden dump per input under `fixtures/goldens/<phase>/`; `PLY_DIFF_BLESS` rewrites them from the port's answer.
pub mod golden {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    pub fn dir() -> PathBuf {
        super::fixtures().join("goldens")
    }

    pub fn blessing() -> bool {
        std::env::var_os("PLY_DIFF_BLESS").is_some()
    }

    /// A bundle's records, `<bundle>#<i>`, share `<phase>/<bundle>.dumps`; every other input has `<phase>/<name>.dump`.
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

    /// `diff(want, got)` is the phase's own first-difference report.
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
            return Some(super::folded(&text));
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
            super::folded(&f)
        })
    }

    /// The first record a bless writes to a bundle's file in this process truncates it; the rest append.
    fn write(path: &Path, index: Option<usize>, text: &str) {
        static STARTED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{}: {e}", parent.display()));
        }
        let text = super::unfolded(text);
        let Some(index) = index else {
            std::fs::write(path, &text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
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

/// The self-hosted compiler the binary carries; `PLY_C_EMITTER=ply:<dir>` enters a working copy of its `.ply` sources instead.
pub mod port {
    use ply_eval::{Fields, Value};
    use ply_span::Symbol;
    use std::sync::Arc;

    /// `name` is `module.function`; a raise, a missing entry or a non-string answer panics.
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

/// The items at `index`, `index + of`, …: dealing round-robin keeps every CI partition the same shape.
pub fn part<T: Clone>(items: &[T], index: usize, of: usize) -> Vec<T> {
    items
        .iter()
        .enumerate()
        .filter(|(i, _)| i % of == index)
        .map(|(_, item)| item.clone())
        .collect()
}

/// Programs separated by a `%%%` line, modules by a `%%` line, each module's first line its dotted name.
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

/// A whole-program dump cut to its last module's records; a failed program's is kept whole.
pub mod own {
    pub fn resolved(dump: &str) -> String {
        let t: Vec<&str> = dump.split_terminator(';').collect();
        let last = match t.get(1).and_then(|n| n.parse::<usize>().ok()) {
            Some(n) if n > 0 && t[0] == "R" && t.get(2) == Some(&"M") => (n - 1).to_string(),
            _ => return dump.to_string(),
        };
        let mut out = t[..2].to_vec();
        let mut i = 2;
        while t.get(i) == Some(&"M") {
            let mut j = i + 3;
            while let Some(width) = t.get(j).and_then(|tag| match *tag {
                "B" | "S" => Some(4),
                "V" | "T" | "E" => Some(5),
                _ => None,
            }) {
                j += width;
            }
            if t.get(i + 1) == Some(&last.as_str()) {
                out.extend_from_slice(&t[i..j.min(t.len())]);
            }
            i = j;
        }
        let i = i.min(t.len());
        let trees = (i..t.len()).find(|&k| t[k] == "P").unwrap_or(t.len());
        let own = (trees..t.len())
            .rev()
            .find(|&k| t[k] == "P")
            .unwrap_or(t.len());
        out.extend_from_slice(&t[i..trees]);
        out.extend_from_slice(&t[own..]);
        out.join(";") + ";"
    }

    /// A test's name can hold a `;`, so a record starts only where a tag is followed by a key.
    pub fn keyed(
        dump: &str,
        program: &[(String, String)],
        tags: &[&str],
        hashed: &[&str],
    ) -> String {
        let t: Vec<&str> = dump.split_terminator(';').collect();
        let Some((module, _)) = program.last() else {
            return dump.to_string();
        };
        if !t.get(2).is_some_and(|tag| tags.contains(tag)) {
            return dump.to_string();
        }
        let under = |key: &str, m: &str| key.strip_prefix(m).is_some_and(|k| k.starts_with('.'));
        let mut keep = true;
        let mut out = String::new();
        for (i, &token) in t.iter().enumerate() {
            let next = t.get(i + 1).copied().unwrap_or("");
            if i >= 2 && tags.contains(&token) {
                if hashed.contains(&token) {
                    keep |= next.len() == 64 && next.bytes().all(|b| b.is_ascii_hexdigit());
                } else if program.iter().any(|(m, _)| under(next, m.as_str())) {
                    keep = under(next, module.as_str());
                }
            }
            if keep {
                out.push_str(token);
                out.push(';');
            }
        }
        out
    }
}

pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// On disk a dump without newlines of its own goes one field per line under a `%;` header, so
/// two changes to different definitions merge; a dump holding newlines is written as it is.
const FOLDED: &str = "%;\n";

fn unfolded(text: &str) -> String {
    if text.contains('\n') {
        return text.to_string();
    }
    format!("{FOLDED}{}", text.replace(';', ";\n"))
}

fn folded(text: &str) -> String {
    match text.strip_prefix(FOLDED) {
        Some(rest) => rest.replace(";\n", ";"),
        None => text.to_string(),
    }
}

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

/// The compiled compiler's cost, held to `benches/compiled-compiler.json`.
pub mod census {
    use std::path::PathBuf;

    pub const FILE: &str = "benches/compiled-compiler.json";

    fn path() -> PathBuf {
        super::repo_root().join(FILE)
    }

    /// A missing entry fails with the reading, which is what the file is written from.
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
        // Bytes scale with the source, so the entry's ceiling is read per line it was taken over:
        // the standard library growing does not move the reading, a hasher allocating more does.
        let per_line = lines as f64 / field("source_lines")?.max(1.0);
        for (name, measured, band, scaled) in [
            ("entries", got.entries as f64, 0.0, false),
            ("allocated", got.allocated as f64, 0.01, true),
            ("recycled", got.recycled as f64, 0.01, true),
            ("chunk_bytes", got.chunk_bytes as f64, 0.25, false),
        ] {
            let ceiling = field(name)? * if scaled { per_line } else { 1.0 };
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
