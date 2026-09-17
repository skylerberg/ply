//! Where the port's front end spends its time, by stage, over corpora already on disk.
//!
//!     stages <corpus dir> [<corpus dir> ...]
//!
//! The port answers each phase at its own entry and each one parses from source, so three of them
//! are nested prefixes of one pipeline: `resolve.resolve_dump` parses and resolves,
//! `infer.check_dump` goes on to check, and `front.front_dump` goes on to hash and build the
//! tables. Their differences are what a stage costs. `hash.hash_dump` is printed beside them and
//! not subtracted, because it is the hasher's own dump rather than a prefix of the front entry.
//!
//! **A program is handed to the port on top of the standard library, and its modules are named the
//! way the driver names them.** Two ways to get this wrong, both of which this probe did, and both
//! of which read as four wonderfully quick stages because a refusal is fast. A generated corpus
//! imports `std.json` and friends, so a program passed without them resolves nothing. And a module
//! is named by its path relative to the project root, dots for separators, which the port says
//! itself when it raises `E0106`: `store/orders.ply` is `store.orders`. A bare file stem loses the
//! directory and every sibling import fails. Hence the head of every dump is printed: the first
//! failure was legible only as three phases returning byte-identical dumps.
//!
//! Read the raw columns, not only the differences: the nesting is a claim about the port's
//! sources, and an entry that short-circuits would make every subtraction wrong while still
//! printing a plausible number.
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The shipped standard library, as the `(module name, source)` pairs a whole-program entry takes.
fn std_modules() -> Vec<(String, String)> {
    ply_std::MODULES
        .iter()
        .map(|(name, src)| ((*name).to_string(), (*src).to_string()))
        .collect()
}

/// A module's name: its path below the root, separators as dots, without the extension.
fn module_name(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if let Some(last) = parts.last_mut()
        && let Some(stem) = last.strip_suffix(".ply")
    {
        *last = stem.to_string();
    }
    parts.join(".")
}

/// Every `.ply` under `root`, in the same shape.
fn modules_of(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path: PathBuf = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "ply") {
                match std::fs::read_to_string(&path) {
                    Ok(src) => out.push((module_name(root, &path), src)),
                    Err(e) => eprintln!("  cannot read {}: {e}", path.display()),
                }
            }
        }
    }
    out.sort();
    out
}

/// The start of a dump on one line, which is what says whether it is a dump at all.
fn head(dump: &str) -> String {
    dump.chars()
        .take(52)
        .collect::<String>()
        .replace('\n', "\\n")
}

/// One reading: the wall clock in milliseconds, the dump's size, and its head.
fn reading(entry: &str, modules: &[(String, String)]) -> (f64, usize, String) {
    let began = Instant::now();
    let dump = ply_compiler_diff::port::dump_program(entry, modules);
    let millis = began.elapsed().as_secs_f64() * 1e3;
    (millis, dump.len(), head(&dump))
}

fn main() {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    if dirs.is_empty() {
        eprintln!("usage: stages <corpus dir> [<corpus dir> ...]");
        std::process::exit(2);
    }
    let library = std_modules();
    for dir in &dirs {
        let own = modules_of(Path::new(dir));
        if own.is_empty() {
            println!("STAGES {dir} no .ply under it; nothing to time");
            continue;
        }
        let mut modules = library.clone();
        modules.extend(own.iter().cloned());
        println!(
            "STAGES {dir} modules {} ({} of them the standard library)",
            modules.len(),
            library.len()
        );
        for entry in [
            "resolve.resolve_dump",
            "infer.check_dump",
            "hash.hash_dump",
            "front.front_dump",
        ] {
            // Once to settle whatever the first call into the unit pays for, then two readings.
            let _ = reading(entry, &modules);
            let (first, bytes, head) = reading(entry, &modules);
            let (second, _, _) = reading(entry, &modules);
            println!("  {entry:<22} {first:9.1} ms {second:9.1} ms   {bytes:>9} bytes   {head}");
        }
    }
}
