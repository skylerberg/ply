//! The comparison this spike exists for: for every `.ply` file in the tree and
//! every fixture beside it, the Ply parser's **tree and diagnostics** against
//! `crates/ply-syntax`'s.
//!
//! It shells out to the shipping `ply` binary rather than driving `ply-eval` in
//! process, for the reason `spikes/ply-lexer/harness/tests/agreement.rs` gives:
//! the shipping binary installs the 10,000-nested-call bound this parser is
//! shaped around, and driving the evaluator directly would let the harness
//! choose a bound no user can choose.
//!
//! **What is compared.** Every node, in preorder, with its own span; every
//! list's length; every `Option`'s presence; every enum arm; every scalar
//! payload; then every diagnostic's code, primary span, label count, note
//! count, and each label's own span and primary flag.
//!
//! **Which tree.** `ply_syntax::parse_unexpanded`, not `parse_recovering`: the
//! module as the **grammar** built it, before `effect_set`, `record_update` and
//! `try_op` rewrite it. Those three are tree-to-tree passes and none of them is
//! parsing; the port implements the grammar and does not claim them.
//! `../GAPS.md` §11R.D is the decision, its cost and the four measurements
//! behind it, and `src/lib.rs`'s `dumper_boundaries` is the one-screen version.
//!
//! **What is not.** Diagnostic *message text* and *severity*: `items.ply`
//! carries a `what: Bytes` at all ~120 sites and never reads it, and turning
//! messages on additionally needs `TokenKind::describe`'s forty arms.
//! `FnDef::derived`, which the parser can only write `None` — asserted rather
//! than skipped, in `src/lib.rs`. And the three rewrites above, which nothing
//! in this spike tests and which `../GAPS-harness.md` §H2 item 5 keeps saying
//! so.
//!
//! > **Withdrawn 2026-08-30.** The paragraph above ended: *"And, for a file
//! > that uses `effect set`, `effect_set::expand`'s effect on the tree, which
//! > `reference_dump_unexpanded` projects out and which this file counts and
//! > prints rather than hides."* There is no projection any more and nothing to
//! > project: the pass does not run. What it cost — a four-conjunct tolerance
//! > excusing 7 mined inputs — is gone with it, and every diagnostic of every
//! > input is now compared exactly.
//!
//! Nothing here skips. If the `ply` binary is missing the tests fail and say
//! how to build it; `CONTRIBUTING.md` §"The suite proves less than it looks
//! like it proves" is about four gates that skip silently and this is not going
//! to be a fifth.

use ply_parser_spike_harness::{
    bundle, byte_literal, node_count, records, reference_dump, uses_effect_sets,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the harness sits at <root>/spikes/ply-parser/harness")
        .to_path_buf()
}

fn spike_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the harness sits inside the spike")
        .to_path_buf()
}

/// The shipping binary, release first: `examples/desk.ply` is 160 kilobytes and
/// the debug interpreter is several times slower on it.
fn ply_binary() -> PathBuf {
    if let Ok(explicit) = std::env::var("PLY_BIN") {
        let path = PathBuf::from(explicit);
        assert!(
            path.exists(),
            "PLY_BIN names {}, which does not exist",
            path.display()
        );
        return path;
    }
    let root = repo_root();
    for profile in ["release", "debug"] {
        let candidate = root.join("target").join(profile).join("ply");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "no `ply` binary at {}/target/{{release,debug}}/ply — run \
         `cargo build -p ply-cli --bin ply --release` first, or set PLY_BIN",
        root.display()
    );
}

/// The six modules the parser is, copied into a scratch project.
///
/// A project of its own rather than `ply run spikes/ply-parser`, and the reason
/// is `GAPS-spine.md`'s: `ply` typechecks every module in the directory it is
/// pointed at, and the spike directory also holds four `test-*.sh`-shaped
/// projects and whatever an editor left behind. `PLY_PARSER_SRC` points it
/// somewhere else, which is how `../arm-harness.sh` runs a mutated copy without
/// touching the worktree.
fn source_dir() -> PathBuf {
    match std::env::var("PLY_PARSER_SRC") {
        Ok(d) => PathBuf::from(d),
        Err(_) => spike_dir(),
    }
}

const MODULES: [&str; 6] = [
    "lexer.ply",
    "spine.ply",
    "types.ply",
    "patterns.ply",
    "exprs.ply",
    "items.ply",
];

struct Project(PathBuf);

impl Project {
    fn new(label: &str) -> Project {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-parser-spike-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            label.replace(['/', '.'], "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp directory");
        for m in MODULES {
            std::fs::copy(source_dir().join(m), dir.join(m))
                .unwrap_or_else(|e| panic!("copying {m} from {}: {e}", source_dir().display()));
        }
        Project(dir)
    }

    /// One `ply run` for many inputs.
    ///
    /// Batched because process start plus typechecking six modules is about
    /// 90ms and dwarfs the parse of a small fixture: 716 mined fixtures cost
    /// 80 seconds one at a time and under two batched. The separator is `~`,
    /// which the dump grammar does not use, and the count is asserted, so a
    /// dump that ran the separator together with its neighbour fails loudly
    /// rather than shifting every fixture by one.
    fn dumps(&self, inputs: &[Vec<u8>]) -> Vec<String> {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/agreement.rs. Not checked in.\n\
             import items (dump)\n",
        );
        let mut parts: Vec<String> = Vec::new();
        for (i, bytes) in inputs.iter().enumerate() {
            src.push_str(&format!("fn s{i}() -> Bytes = {}\n", byte_literal(bytes)));
            parts.push(format!("bytes_of_string(dump(s{i}()))"));
            parts.push("b\"~\"".to_string());
        }
        src.push_str(&format!(
            "fn main() -> String = string_of_bytes(bytes_concat_all([{}]))\n",
            parts.join(", ")
        ));
        std::fs::write(self.0.join("probe.ply"), src).expect("write the probe");
        // The cache keys on the project's modules, and `probe.ply` changes
        // every call, so this is belt and braces — but a stale hit here would
        // be a green comparison against a dump of some other input, which is
        // the one failure this harness must not have.
        let _ = std::fs::remove_dir_all(self.0.join(".ply-cache"));

        let out = Command::new(ply_binary())
            .arg("run")
            .arg(&self.0)
            .arg("--json")
            .output()
            .expect("run ply");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "`ply run {}` failed ({}):\n{stdout}\n{stderr}",
            self.0.display(),
            out.status
        );
        let joined = extract_value(&stdout, &self.0);
        let mut parts: Vec<String> = joined.split('~').map(str::to_string).collect();
        let last = parts.pop().expect("split yields at least one part");
        assert!(
            last.is_empty() && parts.len() == inputs.len(),
            "the probe answered {} dumps for {} inputs",
            parts.len(),
            inputs.len()
        );
        parts
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The `value` field of `ply run --json`, unwrapped.
///
/// Two layers of quoting, both load-bearing. `ply run` puts
/// `Value::to_string()` in that field, and a `Value::Str` renders with its own
/// quotes and its own escaping (`ply_eval::value::escape`), so the JSON string
/// holds `"<dump>"` and serde escapes those two quotes again. The dump is
/// printable ASCII with no `"` and no `\` by construction — the library's
/// `the_dump_is_printable_ascii_with_no_quote_and_no_backslash` asserts that of
/// the reference side — so between those markers the dump is verbatim and no
/// unescaper is needed. If either assumption breaks this fails loudly instead
/// of quietly comparing an unescaper against a parser.
fn extract_value(stdout: &str, dir: &Path) -> String {
    let needle = "\n  \"value\": \"";
    let occurrences = stdout.matches(needle).count();
    assert_eq!(
        occurrences,
        1,
        "`ply run {}` printed {occurrences} top-level `value` fields:\n{stdout}",
        dir.display()
    );
    let after = &stdout[stdout.find(needle).expect("checked above") + needle.len()..];
    let quote = "\\\"";
    assert!(
        after.starts_with(quote),
        "`ply run {}` answered with something that is not a rendered `String`:\n{stdout}",
        dir.display()
    );
    let rest = &after[quote.len()..];
    let end = rest
        .find(quote)
        .unwrap_or_else(|| panic!("the rendered `String` is never closed in:\n{stdout}"));
    let body = &rest[..end];
    assert!(
        !body.contains('\\') && !body.contains('"'),
        "the Ply dump holds a character `ply run --json` escapes, so it is not the \
         printable-ASCII form the comparison assumes:\n{body}"
    );
    body.to_string()
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

// > **Withdrawn 2026-08-30 — both of them, and nothing replaces either.**
// > `reference_for` chose between two dumps and `only_the_expanders_diagnostics`
// > was this harness's one tolerance. Their headers read:
// >
// > > *"The reference dump for one input, and whether the projection was
// > > needed. A file that names no set takes the plain dump, and the projection
// > > is not silently applied to it: `uses_effect_sets` is asked first, the
// > > count is carried in the tally, and every test prints it."*
// >
// > > *"Whether a disagreement is exactly `effect_set::expand`'s diagnostics
// > > and nothing else … So this checks the shape instead, and every clause has
// > > to hold: 1. the input actually names or declares an `effect set`; 2. the
// > > two **trees** are identical …; 3. the Ply parser's diagnostics are a
// > > **prefix** of the reference's — which they must be, because `expand` runs
// > > after the whole parse and can only append; and 4. every diagnostic beyond
// > > that prefix carries an effect-set code. Anything else is a real
// > > disagreement and fails. This is the whole of the tolerance in this
// > > harness, it is four conjuncts wide …"*
// >
// > The comparison entered at `parse_recovering`, where `expand` had run.
// > It enters at `parse_unexpanded`, where it has not — so there is no
// > projection to choose and no appended diagnostic to excuse. `check_all`
// > below now has exactly two outcomes per input, agree or fail, and the
// > `assert_eq!(tally.expander_inputs, 7)` that pinned the hole so it could not
// > grow is gone because the hole is.
// >
// > **What this gave back, measured rather than asserted:** 7 mined fixtures
// > that agreed only by tolerance now agree exactly, and 9 diagnostics
// > (E0114 x6, E0115 x2, E0105 x1) leave the corpus with the pass that raised
// > them. `../GAPS-harness.md` §H4.

fn check_all(inputs: &[(String, Vec<u8>)], label: &str, batch: usize) -> Tally {
    let project = Project::new(label);
    let mut tally = Tally::default();
    let mut failures: Vec<String> = Vec::new();
    for chunk in inputs.chunks(batch) {
        let payloads: Vec<Vec<u8>> = chunk.iter().map(|(_, b)| b.clone()).collect();
        let actual = project.dumps(&payloads);
        for ((name, bytes), got) in chunk.iter().zip(actual) {
            let text = String::from_utf8(bytes.clone())
                .unwrap_or_else(|e| panic!("{name} is not UTF-8: {e}"));
            let want = reference_dump(&text);
            match first_difference(&want, &got) {
                None => tally.add(bytes, &want),
                Some(diff) => failures.push(format!(
                    "the Ply parser and `ply_syntax` disagree on {name}:\n{diff}"
                )),
            }
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
//
// One `ply run` per file rather than a batch: `db.ply` and `desk.ply` are 135
// and 160 kilobytes, and a batch holding both would build one probe of a third
// of a megabyte for no gain — the per-run overhead these batches exist to
// amortise is already noise at that size.

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_every_example() {
    let files = read_all(&ply_files(&repo_root().join("examples")));
    let tally = check_all(&files, "examples", 1);
    assert_eq!(tally.inputs, files.len());
    tally.report("examples");
}

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_the_shipped_standard_library() {
    let files = read_all(&ply_files(&repo_root().join("crates/ply-std/ply")));
    let tally = check_all(&files, "stdlib", 1);
    assert_eq!(tally.inputs, files.len());
    tally.report("stdlib");
}

// --- the error paths -------------------------------------------------------
//
// The corpus above raises **zero** diagnostics, so agreeing on it says nothing
// at all about the recovery half of the parser — and
// `docs/adr/0020-self-hosting-the-front-end.md` §3.1 records the lexer spike's
// headline of 768,760 bytes turning out to be 0.15% error paths as the thing
// not to repeat. These two tests are the answer, and their tallies print the
// diagnostic count so the ratio is on the record rather than assumed.

#[test]
fn the_ply_parser_agrees_with_ply_syntax_on_the_hand_written_fixtures() {
    let files = read_all(&ply_files(&spike_dir().join("fixtures")));
    let tally = check_all(&files, "fixtures", 32);
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
    let path = spike_dir().join("fixtures/reference-tests.corpus");
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
    let tally = check_all(&inputs, "mined", 180);
    assert_eq!(tally.inputs, inputs.len());
    tally.report("mined from crates/ply-syntax/src/tests.rs");
    // > **Withdrawn 2026-08-30.** This stood here, and it was the right shape
    // > for a comparison that had a hole in it:
    // >
    // > > *"Pinned, so that a mutation which turns a real disagreement into an
    // > > 'expander diagnostic' one cannot pass by widening this hole.
    // > > `assert_eq!(tally.expander_inputs, 7, "the number of fixtures excused
    // > > by the expander boundary moved; it is a hole in the comparison and it
    // > > does not get to grow quietly")`"*
    // >
    // > There is no such category any more: `effect_set::expand` does not run,
    // > so no input is excused and the seven that were are compared exactly.
    // > A pin at 0 would be a pin on a variable that no longer exists.
}

// --- what the comparison does not reach ------------------------------------

/// The `effect set` boundary, kept as a test after the boundary went.
///
/// This used to pin how much of the corpus was compared with
/// `effect_set::expand` projected back out — 21.2% of corpus bytes, one file.
/// Nothing is projected now, and the assertion that matters is the opposite
/// one: the file that used to need a weaker comparison gets the same one as
/// everything else, and its trees agree under it. Kept rather than deleted for
/// two reasons: the *set* of such files is still worth watching (a second one
/// would mean a second file whose history a reader should know), and deleting
/// the test would delete the only place the old boundary is named next to the
/// code that used to have it.
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

    // Not merely that it is compared: that comparing it works. The whole point
    // of the projection was that this file's rows did not match, so the claim
    // "it is compared whole now" has to be an exit code rather than a sentence.
    let desk = std::fs::read(&using[0]).expect("desk.ply");
    let want = reference_dump(&String::from_utf8(desk.clone()).expect("UTF-8"));
    let project = Project::new("desk-whole");
    let got = project.dumps(&[desk]).remove(0);
    assert!(
        first_difference(&want, &got).is_none(),
        "{}",
        first_difference(&want, &got).unwrap_or_default()
    );
}

/// **The cost of `../GAPS.md` §11R.D, taken here rather than asserted there.**
///
/// The comparison is against the tree before `effect_set`, `record_update` and
/// `try_op` rewrite it, so every diagnostic those three raise leaves it. §11R.D
/// argued that this is cheap and an argument in a document is exactly the thing
/// that goes stale, so this is the same claim as an exit code, over every input
/// the differential compares.
///
/// **And taking it corrected the argument.** §11R.D priced the loss at *"9
/// (E0114 ×6, E0115 ×2, E0105 ×1)"*, counted by grepping those three codes out
/// of the reference's dump. Differencing the two entry points directly — the
/// only way to attribute a diagnostic to a pass rather than to a code — gives
/// **7**, on 7 mined fixtures: E0114 ×4, E0115 ×2, E0105 ×1. The two extra
/// `E0114`s belong to `items.ply`'s own refusal of `pub effect set`, which
/// shares the code and is raised by the **grammar** on both sides.
///
/// So the price is not 9 of 835 but **7 of 835 (0.84%), and all seven are the
/// ones the deleted tolerance already excused**. Nothing that this differential
/// ever actually compared is given up.
///
/// It also states the *tree* half, which no count can: `record_update` and
/// `try_op` rewrite nodes and raise nothing on this corpus, so what leaves is
/// 2,087 lines of Rust that nothing in this spike tests. `../GAPS-harness.md`
/// §H2 item 5 is the enforced list; this is the number beside it.
#[test]
fn the_rewrites_this_comparison_gives_up_raise_exactly_these_diagnostics() {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut inputs = 0usize;
    let mut affected: Vec<String> = Vec::new();
    let mut note =
        |name: &str, text: &str, counts: &mut std::collections::BTreeMap<String, usize>| {
            let added = ply_parser_spike_harness::diagnostics_the_rewrites_add(text);
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
    for path in ply_files(&spike_dir().join("fixtures")) {
        inputs += 1;
        note(
            &path.display().to_string(),
            &std::fs::read_to_string(&path).expect("UTF-8"),
            &mut counts,
        );
    }
    let mined = std::fs::read_to_string(spike_dir().join("fixtures/reference-tests.corpus"))
        .expect("UTF-8");
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
    // Pinned so it cannot grow quietly, and so that a rewrite gaining an error
    // path the differential will never see is a failing test rather than a
    // paragraph nobody re-reads. Update it deliberately, with the reason.
    assert_eq!(
        counts,
        [
            ("E0105".to_string(), 1usize),
            ("E0114".to_string(), 4),
            ("E0115".to_string(), 2),
        ]
        .into_iter()
        .collect(),
        "the set of diagnostics this comparison gives up has moved. Every one of them is \
         raised by `effect_set`, `record_update` or `try_op` — the three passes the port \
         does not implement — and `../GAPS.md` §11R.D priced the decision at exactly this \
         list. Re-take the price before changing the number."
    );

    // The tree half, printed rather than pinned: it moves whenever a `.ply` in
    // the tree gains or loses a `?`, which is not a fact about this spike.
    let mut added = 0usize;
    let mut biggest: Vec<(usize, String)> = Vec::new();
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            let n = ply_parser_spike_harness::nodes_the_rewrites_add(
                &std::fs::read_to_string(&path).expect("UTF-8"),
            );
            added += n;
            if n > 0 {
                biggest.push((n, path.display().to_string()));
            }
        }
    }
    for path in ply_files(&spike_dir().join("fixtures")) {
        added += ply_parser_spike_harness::nodes_the_rewrites_add(
            &std::fs::read_to_string(&path).expect("UTF-8"),
        );
    }
    for f in bundle(&mined) {
        added += ply_parser_spike_harness::nodes_the_rewrites_add(&f);
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
///
/// A differential that agrees over a corpus reaching half the grammar has
/// checked half the grammar. This prints both halves and fails if the corpus
/// stops reaching a tag it reaches today, which is the failure mode a fixture
/// deletion would otherwise cause silently.
#[test]
fn the_comparison_reaches_every_tag_the_reference_side_can_emit() {
    let mut seen: Vec<String> = Vec::new();
    let mut push = |text: &str| {
        seen.extend(ply_parser_spike_harness::tags(&reference_dump(text)));
    };
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            push(&std::fs::read_to_string(&path).expect("UTF-8"));
        }
    }
    for path in ply_files(&spike_dir().join("fixtures")) {
        push(&std::fs::read_to_string(&path).expect("UTF-8"));
    }
    let mined = std::fs::read_to_string(spike_dir().join("fixtures/reference-tests.corpus"))
        .expect("UTF-8");
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
///
/// Three arrived on 2026-08-30 with `../GAPS.md` §11R.D's move to the
/// pre-rewrite tree: `erup` and `etry` are the two variants that used to be
/// `unreachable!()` in `src/lib.rs`, and `narg` is `App`'s keyword-argument
/// list, which nothing emitted at all. `narg` is reached by
/// `fixtures/13-named-arguments-and-defaults.ply` and by nothing else in the
/// corpus — the mined half has zero named arguments — so if that fixture is
/// deleted this test is what says the coverage figure fell.
///
/// The three placeholder tags the Ply side has and `ast.rs` does not — `tbad`,
/// `pbad`, `ebad` — and the two placeholder words `%badmode` and `%badlit` are
/// deliberately absent: nothing in `ply_syntax` produces them, so the reference
/// dump cannot, and listing them would make the coverage figure unreachable by
/// construction. That the Ply side agrees anyway is the evidence that no
/// placeholder escapes into a tree the reference builds a real node for.
const EMITTABLE: [&str; 95] = [
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
    "%str",
    "%sub",
    "%sum",
    "%true",
    "%unit",
    "%write",
];
