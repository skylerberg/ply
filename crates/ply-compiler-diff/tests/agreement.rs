//! The comparison this spike exists for: for every `.ply` file in the tree and every fixture beside
//! it, the Ply parser's **tree and diagnostics** against `crates/ply-syntax`'s.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the compiler
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::{bundle, node_count, port, records, reference_dump, uses_effect_sets};
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

/// The first record the two dumps disagree on, with context, or `None`.
fn first_difference(reference: &str, actual: &str) -> Option<String> {
    let want = records(reference);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(6);
            let mut report = format!("record {i} of {} differs\n", want.len());
            report.push_str(&format!(
                "  rust: {a:?}\n  ply : {b:?}\n  context (rust):\n"
            ));
            for (j, r) in want.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            report.push_str("  context (ply):\n");
            for (j, r) in got.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

/// Running totals, so a corpus figure can be printed rather than claimed.
#[derive(Default)]
struct Tally {
    inputs: usize,
    bytes: usize,
    records: usize,
    nodes: usize,
    diagnostics: usize,
}

impl Tally {
    fn add(&mut self, input: &[u8], dump: &str) {
        self.inputs += 1;
        self.bytes += input.len();
        self.records += records(dump).len();
        self.nodes += node_count(dump);
        self.diagnostics += dump.matches('!').count();
    }

    fn report(&self, what: &str) {
        println!(
            "  {what}: {} inputs, {} bytes, {} records, {} nodes, {} diagnostics",
            self.inputs, self.bytes, self.records, self.nodes, self.diagnostics
        );
    }
}

fn check_all(inputs: &[(String, Vec<u8>)]) -> Tally {
    let mut tally = Tally::default();
    let mut failures: Vec<String> = Vec::new();
    for (name, bytes) in inputs {
        let got = port::dump("items.dump", bytes);
        let text =
            String::from_utf8(bytes.clone()).unwrap_or_else(|e| panic!("{name} is not UTF-8: {e}"));
        let want = reference_dump(&text);
        match first_difference(&want, &got) {
            None => tally.add(bytes, &want),
            Some(diff) => failures.push(format!(
                "the Ply parser and `ply_syntax` disagree on {name}:\n{diff}"
            )),
        }
    }
    if !failures.is_empty() {
        panic!(
            "{} of {} inputs disagree\n\n{}",
            failures.len(),
            inputs.len(),
            failures.join("\n")
        );
    }
    tally
}

fn ply_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    out.sort();
    assert!(!out.is_empty(), "{} holds no .ply files", dir.display());
    out
}

fn read_all(paths: &[PathBuf]) -> Vec<(String, Vec<u8>)> {
    paths
        .iter()
        .map(|p| {
            (
                p.display().to_string(),
                std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
        })
        .collect()
}

// --- the corpus ------------------------------------------------------------

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_every_example() {
    let files = read_all(&ply_files(&repo_root().join("examples")));
    let tally = check_all(&files);
    assert_eq!(tally.inputs, files.len());
    tally.report("examples");
}

/// Modules this port cannot read, because their syntax postdates it.
///
/// Empty since the port learned the bit operators and hex literals; `hash.ply`
/// stood here while it had not. The list stays, with both assertions below, so
/// that the next surface to land is named here rather than quietly skipped —
/// and so that the naming cannot rot into a permanent exemption.
const POSTDATES_THE_PORT: &[&str] = &[];

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_the_shipped_standard_library() {
    let all = ply_files(&repo_root().join("crates/ply-std/ply"));
    let (skipped, covered): (Vec<PathBuf>, Vec<PathBuf>) = all.iter().cloned().partition(|p| {
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        POSTDATES_THE_PORT.contains(&name.as_str())
    });
    assert_eq!(
        skipped.len(),
        POSTDATES_THE_PORT.len(),
        "`POSTDATES_THE_PORT` names a module the standard library does not ship: {skipped:?}"
    );

    let files = read_all(&covered);
    let tally = check_all(&files);
    assert_eq!(tally.inputs, files.len());
    tally.report("stdlib");

    // Every skipped module still has to *fail*. A port that learned the syntax, or a module that
    // stopped using it, makes this red — which is the only thing that gets the row deleted.
    for path in &skipped {
        let one = read_all(std::slice::from_ref(path));
        let outcome = std::panic::catch_unwind(|| check_all(&one));
        assert!(
            outcome.is_err(),
            "`{}` now agrees with `ply_syntax`, so remove it from `POSTDATES_THE_PORT`",
            path.display()
        );
    }
}

// --- the error paths -------------------------------------------------------

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_the_hand_written_fixtures() {
    let files = read_all(&ply_files(&here().join("fixtures")));
    let tally = check_all(&files);
    assert_eq!(tally.inputs, files.len());
    assert!(
        tally.diagnostics >= 40,
        "the fixtures raise {} diagnostics; they exist to exercise the recovery half \
         and something has stopped them reaching it",
        tally.diagnostics
    );
    tally.report("fixtures");
}

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_the_reference_own_test_inputs() {
    let path = here().join("fixtures/reference-tests.corpus");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let fixtures = bundle(&text);
    assert!(
        fixtures.len() > 700,
        "the mined bundle holds {} fixtures; regenerate it with mine-fixtures.py",
        fixtures.len()
    );
    let inputs: Vec<(String, Vec<u8>)> = fixtures
        .iter()
        .enumerate()
        .map(|(i, f)| (format!("reference-tests.corpus#{i}"), f.as_bytes().to_vec()))
        .collect();
    let tally = check_all(&inputs);
    assert_eq!(tally.inputs, inputs.len());
    tally.report("mined from crates/ply-syntax/src/tests.rs");
}

// --- what the comparison does not reach ------------------------------------

/// The `effect set` boundary, kept as a test after the boundary went.
#[test]
fn the_one_file_that_used_to_need_a_projection_is_now_compared_whole() {
    let mut using: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut set_bytes = 0usize;
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            let text = std::fs::read_to_string(&path).expect("UTF-8");
            total += text.len();
            if uses_effect_sets(&text) {
                set_bytes += text.len();
                using.push(path.display().to_string());
            }
        }
    }
    println!(
        "  `effect set` is used by {} of {} corpus bytes ({:.1}%): {:?} — compared whole, \
         with no projection and no tolerance",
        set_bytes,
        total,
        100.0 * set_bytes as f64 / total as f64,
        using
    );
    assert_eq!(using.len(), 1, "{using:?}");
    assert!(using[0].ends_with("desk.ply"), "{using:?}");

    // Not merely that it is compared: that comparing it works.
    let desk = std::fs::read(&using[0]).expect("desk.ply");
    let want = reference_dump(&String::from_utf8(desk.clone()).expect("UTF-8"));
    let got = port::dump("items.dump", &desk);
    assert!(
        first_difference(&want, &got).is_none(),
        "{}",
        first_difference(&want, &got).unwrap_or_default()
    );
}

/// **The cost of `../GAPS.md` §11R.D, taken here rather than asserted there.**
#[test]
fn the_rewrites_this_comparison_gives_up_raise_exactly_these_diagnostics() {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut inputs = 0usize;
    let mut affected: Vec<String> = Vec::new();
    let mut note =
        |name: &str, text: &str, counts: &mut std::collections::BTreeMap<String, usize>| {
            let added = ply_compiler_diff::diagnostics_the_rewrites_add(text);
            if !added.is_empty() {
                affected.push(format!("{name} {added:?}"));
            }
            for code in added {
                *counts.entry(code).or_default() += 1;
            }
        };
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            inputs += 1;
            note(
                &path.display().to_string(),
                &std::fs::read_to_string(&path).expect("UTF-8"),
                &mut counts,
            );
        }
    }
    for path in ply_files(&here().join("fixtures")) {
        inputs += 1;
        note(
            &path.display().to_string(),
            &std::fs::read_to_string(&path).expect("UTF-8"),
            &mut counts,
        );
    }
    let mined =
        std::fs::read_to_string(here().join("fixtures/reference-tests.corpus")).expect("UTF-8");
    for (i, f) in bundle(&mined).iter().enumerate() {
        inputs += 1;
        note(&format!("reference-tests.corpus#{i}"), f, &mut counts);
    }
    let total: usize = counts.values().sum();
    println!(
        "  the three rewrites raise {total} diagnostic(s) over {inputs} inputs, on {} of them: \
         {counts:?}",
        affected.len()
    );
    for a in &affected {
        println!("    {a}");
    }
    // Pinned so it cannot grow quietly, and so that a rewrite gaining an error path the
    // differential will never see is a failing test rather than a paragraph nobody re-reads.
    assert_eq!(
        counts,
        [
            ("E0105".to_string(), 1usize),
            ("E0114".to_string(), 4),
            ("E0115".to_string(), 2),
            ("E0116".to_string(), 6),
            ("E0117".to_string(), 1),
            ("E0118".to_string(), 17),
            ("E0119".to_string(), 10),
        ]
        .into_iter()
        .collect(),
        "the set of diagnostics this comparison gives up has moved. Every one of them is \
         raised by `effect_set`, `record_update` or `try_op` — the three passes the port \
         does not implement — and `../GAPS.md` §11R.D priced the decision at exactly this \
         list. Re-take the price before changing the number."
    );

    // The tree half, printed rather than pinned: it moves whenever a `.ply` in the tree gains or
    // loses a `?`, which is not a fact about this spike.
    let mut added = 0isize;
    let mut biggest: Vec<(isize, String)> = Vec::new();
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            let n = ply_compiler_diff::nodes_the_rewrites_add(
                &std::fs::read_to_string(&path).expect("UTF-8"),
            );
            added += n;
            if n > 0 {
                biggest.push((n, path.display().to_string()));
            }
        }
    }
    for path in ply_files(&here().join("fixtures")) {
        added += ply_compiler_diff::nodes_the_rewrites_add(
            &std::fs::read_to_string(&path).expect("UTF-8"),
        );
    }
    for f in bundle(&mined) {
        added += ply_compiler_diff::nodes_the_rewrites_add(&f);
    }
    biggest.sort();
    biggest.reverse();
    println!(
        "  and they add {added} node(s) the comparison therefore does not see; by file: {:?}",
        biggest
            .iter()
            .map(|(n, p)| format!("{n} {}", p.rsplit('/').next().unwrap_or(p)))
            .collect::<Vec<_>>()
    );
    assert!(
        added > 0,
        "the rewrites add no node anywhere in the corpus, so either no `.ply` in the tree \
         uses `?` or `{{..}}` any more — in which case this whole decision costs nothing and \
         should be re-read — or this measurement has stopped working"
    );
}

/// Which node tags the whole comparison reaches, and which it does not.
#[test]
fn the_comparison_reaches_every_tag_the_reference_side_can_emit() {
    let mut seen: Vec<String> = Vec::new();
    let mut push = |text: &str| {
        seen.extend(ply_compiler_diff::tags(&reference_dump(text)));
    };
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            push(&std::fs::read_to_string(&path).expect("UTF-8"));
        }
    }
    for path in ply_files(&here().join("fixtures")) {
        push(&std::fs::read_to_string(&path).expect("UTF-8"));
    }
    let mined =
        std::fs::read_to_string(here().join("fixtures/reference-tests.corpus")).expect("UTF-8");
    for f in bundle(&mined) {
        push(&f);
    }
    seen.sort();
    seen.dedup();

    let missing: Vec<&str> = EMITTABLE
        .iter()
        .copied()
        .filter(|t| !seen.iter().any(|s| s == t))
        .collect();
    let unlisted: Vec<&String> = seen
        .iter()
        .filter(|s| !EMITTABLE.contains(&s.as_str()))
        .collect();
    println!(
        "  tag coverage: {} of {} reachable tags; unreached: {:?}",
        EMITTABLE.len() - missing.len(),
        EMITTABLE.len(),
        missing
    );
    assert!(
        unlisted.is_empty(),
        "the dump emitted tags this list does not name, so the coverage figure is wrong: \
         {unlisted:?}"
    );
    assert!(
        missing.is_empty(),
        "the corpus and fixtures no longer reach {missing:?}; a fixture that covered them \
         has been deleted or changed, and the agreement figure now says less than it did"
    );
}

/// Every tag the **reference** side of this dump can emit.
const EMITTABLE: [&str; 102] = [
    // nodes
    "arm",
    "atm",
    "bnd",
    "cst",
    "der",
    "dsp",
    "eapp",
    "ebin",
    "eblk",
    "ecel",
    "eff",
    "efld",
    "ehnd",
    "eif",
    "elam",
    "elit",
    "elst",
    "emat",
    "eprf",
    "erec",
    "ergn",
    "erup",
    "esim",
    "etry",
    "eun",
    "evar",
    "fn",
    "reu",
    "hcl",
    "ident",
    "imp",
    "law",
    "lnm",
    "narg",
    "op",
    "pctr",
    "plit",
    "plst",
    "prec",
    "prm",
    "pvar",
    "pwld",
    "qname",
    "rcl",
    "row",
    "set",
    "sexp",
    "slet",
    "spc",
    "tcon",
    "tfn",
    "tnm",
    "trec",
    "tst",
    "tuni",
    "tvar",
    "ty",
    "var",
    // words
    "%add",
    "%alias",
    "%and",
    "%bitand",
    "%bitor",
    "%bitxor",
    "%bool",
    "%bytes",
    "%concat",
    "%dec",
    "%div",
    "%ensures",
    "%eq",
    "%false",
    "%float",
    "%ge",
    "%gen",
    "%gt",
    "%int",
    "%fixed",
    "%json",
    "%le",
    "%lt",
    "%mod",
    "%mul",
    "%names",
    "%ne",
    "%neg",
    "%not",
    "%or",
    "%ord",
    "%priv",
    "%pub",
    "%read",
    "%rem",
    "%requires",
    "%shl",
    "%str",
    "%sub",
    "%sum",
    "%true",
    "%unit",
    "%ushr",
    "%write",
];
