//! The seventh comparison: `code.ply`'s lowering against `ply_eval::code::lower_fn`.
//!
//! The first differential for a stage *after* the front end, and the one that makes converting the
//! C emitter to Ply a thing that can be done incrementally at all: the emitter reads the lowered
//! form, so a port of it has to produce that form, and this is what says whether it has.
//!
//! **It compares only what `code.ply` claims to lower.** `lower` answers `None` for a node kind
//! the port has not reached, `lower_module` leaves that function out of its dump, and this test
//! compares the functions that *are* there against the oracle's records for the same names. A port
//! that lowered nothing would pass every comparison and fail `the_port_reaches_a_growing_share`,
//! which is the assertion that keeps this honest as the port grows.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::{port, reference_lower_dump};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits at <root>/crates/ply-compiler-diff")
        .to_path_buf()
}

/// This crate's own directory, which is where the mined corpora live.
fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The compiler's Ply sources, one of the corpora.
fn compiler_ply() -> PathBuf {
    here()
        .parent()
        .expect("this crate sits under crates/")
        .join("ply-compiler")
        .join("ply")
}

fn ply_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    out.sort();
    out
}

/// The records `code.ply` produced, by the function name each is for.
///
/// The port leaves out what it cannot lower, so a comparison keyed on position would compare a
/// record against the wrong function the moment one was skipped. Keyed on the name, a skipped
/// function is simply absent and the ones present are compared against their own reference.
fn by_name(dump: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for record in dump.split(";f:").skip(1) {
        let Some((head, body)) = record.split_once(';') else {
            continue;
        };
        let name = head.split(':').next().unwrap_or_default().to_string();
        out.insert(name, body.trim_end_matches(';').to_string());
    }
    out
}

/// What the port covers, over one corpus: every function it lowered agrees with the oracle, and
/// the share it reached is reported so that a port going backwards is visible.
fn compare(label: &str, inputs: &[(String, Vec<u8>)]) -> (usize, usize, usize) {
    let mut failures: Vec<String> = Vec::new();
    let (mut reached, mut available, mut compared) = (0usize, 0usize, 0usize);
    for (name, text) in inputs {
        let actual = port::dump("code.lower_dump", text);
        let source = String::from_utf8_lossy(text).to_string();
        let reference = reference_lower_dump(&[("m".to_string(), source)]);
        let expected = by_name(&reference);
        let mine = by_name(&actual);
        available += expected.len();
        reached += mine.len();
        for (fname, body) in &mine {
            compared += 1;
            match expected.get(fname) {
                None => failures.push(format!(
                    "{label}: `{fname}` in {name} was lowered by the port and not by the oracle"
                )),
                Some(want) if want != body => failures.push(format!(
                    "{label}: the two lowerings disagree on `{fname}` in {name}:\n  oracle {want}\n  port   {body}"
                )),
                Some(_) => {}
            }
        }
    }
    println!(
        "  {label}: {} input(s), {reached} of {available} function(s) lowered, {compared} compared",
        inputs.len()
    );
    assert!(
        failures.is_empty(),
        "{} disagreement(s)\n\n{}",
        failures.len(),
        failures.join("\n")
    );
    (reached, available, compared)
}

fn files_in(dir: &Path) -> Vec<(String, Vec<u8>)> {
    ply_files(dir)
        .into_iter()
        .map(|p| {
            (
                p.display().to_string(),
                std::fs::read(&p).expect("readable"),
            )
        })
        .collect()
}

/// The port lowers what it says it lowers, and agrees wherever it does.
#[test]
fn the_lowering_agrees_with_ply_eval_wherever_the_port_reaches() {
    let mut reached = 0usize;
    let mut available = 0usize;
    let mut compared = 0usize;
    for (label, dir) in [
        ("stdlib", repo_root().join("crates/ply-std/ply")),
        ("examples", repo_root().join("examples")),
        ("fixtures", here().join("fixtures")),
        // The emitter's own sources: the bootstrap builds the emitter from what it emits for them.
        ("emitter", compiler_ply()),
    ] {
        let (r, a, c) = compare(label, &files_in(&dir));
        reached += r;
        available += a;
        compared += c;
    }
    // The assertion that keeps the one above honest: a port that lowered nothing would agree with
    // the oracle on every function it produced, because it would produce none. This is the share,
    // and it is written down so that raising it is a visible change and lowering it is a failure.
    // Two numbers, because they are not the same claim. `reached` is what the port lowered;
    // `compared` is what was checked against the oracle. They were apart by the record updates,
    // which nothing verified; they now meet, and the second is the one that means anything.
    println!("  the port reaches {reached} of {available} function bodies, {compared} compared");
    assert!(
        compared >= 2668,
        "the port was compared on {compared} bodies, and it was 2668 when the emitter's own \
         sources joined the corpus -- raise this number when the port grows, and never lower it"
    );
    assert!(
        reached >= 2668,
        "the port lowered {reached} of {available} bodies, and it reached 2668 when the emitter's \
         own sources joined the corpus -- raise this number when the port grows, and never lower it"
    );
}
