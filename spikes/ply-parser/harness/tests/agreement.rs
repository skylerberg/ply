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
//! **What is not.** Diagnostic *message text* and *severity*: `items.ply`
//! carries a `what: Bytes` at all ~120 sites and never reads it, and turning
//! messages on additionally needs `TokenKind::describe`'s forty arms.
//! `FnDef::derived`, which the parser can only write `None` — asserted rather
//! than skipped, in `src/lib.rs`. And, for a file that uses `effect set`,
//! `effect_set::expand`'s effect on the tree, which `reference_dump_unexpanded`
//! projects out and which this file counts and prints rather than hides.
//!
//! Nothing here skips. If the `ply` binary is missing the tests fail and say
//! how to build it; `CONTRIBUTING.md` §"The suite proves less than it looks
//! like it proves" is about four gates that skip silently and this is not going
//! to be a fifth.

use ply_parser_spike_harness::{
    DumpedDiag, bundle, byte_literal, node_count, records, reference_dump,
    reference_dump_unexpanded, split_diags, uses_effect_sets,
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
    projected: usize,
    /// Inputs whose trees agree and whose only difference is a diagnostic
    /// `effect_set::expand` raised, and how many such diagnostics there were.
    expander_inputs: usize,
    expander_diags: usize,
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
            "  {what}: {} inputs, {} bytes, {} records, {} nodes, {} diagnostics, \
             {} with expansion projected out",
            self.inputs, self.bytes, self.records, self.nodes, self.diagnostics, self.projected
        );
        if self.expander_inputs > 0 {
            println!(
                "  {what}: {} of those agree on the tree but not on {} diagnostic(s) that \
                 `effect_set::expand` raises and the port does not",
                self.expander_inputs, self.expander_diags
            );
        }
    }
}

/// The reference dump for one input, and whether the projection was needed.
///
/// A file that names no set takes the plain dump, and the projection is not
/// silently applied to it: `uses_effect_sets` is asked first, the count is
/// carried in the tally, and every test prints it.
fn reference_for(text: &str, tally: &mut Tally) -> String {
    if uses_effect_sets(text) {
        tally.projected += 1;
        reference_dump_unexpanded(text)
    } else {
        reference_dump(text)
    }
}

/// Whether a disagreement is exactly `effect_set::expand`'s diagnostics and
/// nothing else.
///
/// `reference_dump_unexpanded` projects the expander out of the **tree** but
/// deliberately not out of the diagnostics: `expand` raises `E0105`, `E0114`
/// and `E0115`, `items.ply` raises `E0114` for reasons of its own, and telling
/// those apart by code alone would be guessing. So this checks the shape
/// instead, and every clause has to hold:
///
/// 1. the input actually names or declares an `effect set`;
/// 2. the two **trees** are identical, so nothing structural is being waved
///    through;
/// 3. the Ply parser's diagnostics are a **prefix** of the reference's — which
///    they must be, because `expand` runs after the whole parse and can only
///    append; and
/// 4. every diagnostic beyond that prefix carries an effect-set code.
///
/// Anything else is a real disagreement and fails. This is the whole of the
/// tolerance in this harness, it is four conjuncts wide, and `../arm-harness.sh`
/// corrupts a row's atoms to confirm clause 2 is load-bearing.
fn only_the_expanders_diagnostics(text: &str, want: &str, got: &str) -> Option<Vec<DumpedDiag>> {
    if !uses_effect_sets(text) {
        return None;
    }
    let (want_tree, want_ds) = split_diags(want)?;
    let (got_tree, got_ds) = split_diags(got)?;
    if want_tree != got_tree || got_ds.len() > want_ds.len() {
        return None;
    }
    if want_ds[..got_ds.len()] != got_ds[..] {
        return None;
    }
    let extra = want_ds[got_ds.len()..].to_vec();
    if extra.is_empty() {
        return None;
    }
    if extra
        .iter()
        .all(|d| ["E0105", "E0114", "E0115"].contains(&d.code.as_str()))
    {
        Some(extra)
    } else {
        None
    }
}

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
            let want = reference_for(&text, &mut tally);
            match first_difference(&want, &got) {
                None => tally.add(bytes, &want),
                Some(diff) => match only_the_expanders_diagnostics(&text, &want, &got) {
                    Some(extra) => {
                        tally.add(bytes, &got);
                        tally.expander_diags += extra.len();
                        tally.expander_inputs += 1;
                        println!(
                            "  tree agrees, {} expander diagnostic(s) the port does not raise: \
                             {name} {:?}",
                            extra.len(),
                            extra.iter().map(|d| &d.code).collect::<Vec<_>>()
                        );
                    }
                    None => failures.push(format!(
                        "the Ply parser and `ply_syntax` disagree on {name}:\n{diff}"
                    )),
                },
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
    // Pinned, so that a mutation which turns a real disagreement into an
    // "expander diagnostic" one cannot pass by widening this hole.
    assert_eq!(
        tally.expander_inputs, 7,
        "the number of fixtures excused by the expander boundary moved; it is a hole in \
         the comparison and it does not get to grow quietly"
    );
}

// --- what the comparison does not reach ------------------------------------

/// The `effect set` boundary, stated as a number rather than a footnote.
///
/// `items.ply` does not port `effect_set::expand`, so for a file that uses sets
/// the reference tree is compared with that pass projected back out. This test
/// pins how much of the corpus that is, so the figure in the write-up cannot
/// drift away from the tree: exactly one file, and this asserts which.
#[test]
fn exactly_one_corpus_file_needs_the_expansion_projected_out() {
    let mut using: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut projected_bytes = 0usize;
    for dir in ["examples", "crates/ply-std/ply"] {
        for path in ply_files(&repo_root().join(dir)) {
            let text = std::fs::read_to_string(&path).expect("UTF-8");
            total += text.len();
            if uses_effect_sets(&text) {
                projected_bytes += text.len();
                using.push(path.display().to_string());
            }
        }
    }
    println!(
        "  expansion projected out of {} of {} corpus bytes ({:.1}%): {:?}",
        projected_bytes,
        total,
        100.0 * projected_bytes as f64 / total as f64,
        using
    );
    assert_eq!(using.len(), 1, "{using:?}");
    assert!(using[0].ends_with("desk.ply"), "{using:?}");
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
        let d = if uses_effect_sets(text) {
            reference_dump_unexpanded(text)
        } else {
            reference_dump(text)
        };
        seen.extend(ply_parser_spike_harness::tags(&d));
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
/// The three placeholder tags the Ply side has and `ast.rs` does not — `tbad`,
/// `pbad`, `ebad` — and the two placeholder words `%badmode` and `%badlit` are
/// deliberately absent: nothing in `ply_syntax` produces them, so the reference
/// dump cannot, and listing them would make the coverage figure unreachable by
/// construction. That the Ply side agrees anyway is the evidence that no
/// placeholder escapes into a tree the reference builds a real node for.
const EMITTABLE: [&str; 92] = [
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
    "esim",
    "eun",
    "evar",
    "fn",
    "hcl",
    "ident",
    "imp",
    "law",
    "lnm",
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
