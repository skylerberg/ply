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
//! Read the raw columns, not only the differences: the nesting is a claim about the port's
//! sources, and an entry that short-circuits would make every subtraction wrong while still
//! printing a plausible number.
//!
//! A dump beginning `diag ` is a program the port refused. It is fast for the wrong reason, so the
//! probe says so instead of reporting a time for it.
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Every `.ply` under `dir`, as the `(module name, source)` pairs a whole-program entry takes.
fn modules_of(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path: PathBuf = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "ply") {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                match std::fs::read_to_string(&path) {
                    Ok(src) => out.push((name, src)),
                    Err(e) => eprintln!("  cannot read {}: {e}", path.display()),
                }
            }
        }
    }
    out.sort();
    out
}

/// One reading: the wall clock in milliseconds, the dump's size, and whether it is a refusal.
fn reading(entry: &str, modules: &[(String, String)]) -> (f64, usize, bool) {
    let began = Instant::now();
    let dump = ply_compiler_diff::port::dump_program(entry, modules);
    let millis = began.elapsed().as_secs_f64() * 1e3;
    (millis, dump.len(), dump.starts_with("diag "))
}

fn main() {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    if dirs.is_empty() {
        eprintln!("usage: stages <corpus dir> [<corpus dir> ...]");
        std::process::exit(2);
    }
    for dir in &dirs {
        let modules = modules_of(Path::new(dir));
        println!("STAGES {dir} modules {}", modules.len());
        if modules.is_empty() {
            println!("  no .ply under it; nothing to time");
            continue;
        }
        for entry in [
            "resolve.resolve_dump",
            "infer.check_dump",
            "hash.hash_dump",
            "front.front_dump",
        ] {
            // Once to settle whatever the first call into the unit pays for, then two readings.
            let _ = reading(entry, &modules);
            let (first, bytes, refused) = reading(entry, &modules);
            let (second, _, _) = reading(entry, &modules);
            if refused {
                println!(
                    "  {entry:<22} REFUSED: the dump is diagnostics, so its clock means nothing"
                );
            } else {
                println!("  {entry:<22} {first:9.1} ms {second:9.1} ms   dump {bytes} bytes");
            }
        }
    }
}
