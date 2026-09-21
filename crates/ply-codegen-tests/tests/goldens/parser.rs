use crate::harness::{bundle, fixtures, golden, port, records, repo_root};
use std::path::{Path, PathBuf};

fn first_difference(golden: &str, actual: &str) -> Option<String> {
    let want = records(golden);
    let got = records(actual);
    for i in 0..want.len().max(got.len()) {
        let a = want.get(i).copied();
        let b = got.get(i).copied();
        if a != b {
            let from = i.saturating_sub(6);
            let mut report = format!("record {i} of {} differs\n", want.len());
            report.push_str(&format!(
                "  golden: {a:?}\n  port  : {b:?}\n  context (golden):\n"
            ));
            for (j, r) in want.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            report.push_str("  context (port):\n");
            for (j, r) in got.iter().enumerate().skip(from).take(14) {
                report.push_str(&format!("    {j:>7} {r}\n"));
            }
            return Some(report);
        }
    }
    None
}

/// Not `#` a list's length, `?` an option's presence, `%` a word, `@` a payload, `!` a diagnostic
/// or `=` its label.
fn is_node(record: &str) -> bool {
    !record.is_empty() && !record.starts_with(['#', '?', '%', '@', '!', '='])
}

/// Every distinct node tag and word the dump reached.
fn tags(dump: &str) -> Vec<String> {
    records(dump)
        .iter()
        .filter(|r| !r.is_empty() && !r.starts_with(['#', '?', '@', '!', '=']))
        .map(|r| match r.strip_prefix('%') {
            Some(word) => format!("%{word}"),
            None => r.rsplit(':').next().unwrap_or("").to_string(),
        })
        .collect()
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
        self.nodes += records(dump).iter().filter(|r| is_node(r)).count();
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
        match golden::check("parser", name, &got, first_difference) {
            Ok(()) => tally.add(bytes, &got),
            Err(report) => failures.push(report),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} inputs disagree\n\n{}",
        failures.len(),
        inputs.len(),
        failures.join("\n")
    );
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
                p.strip_prefix(repo_root())
                    .expect("an input under the repository")
                    .display()
                    .to_string(),
                std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
        })
        .collect()
}

fn mined() -> Vec<(String, Vec<u8>)> {
    let path = fixtures().join("reference-tests.corpus");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bundle(&text)
        .into_iter()
        .enumerate()
        .map(|(i, f)| (format!("reference-tests.corpus#{i}"), f.into_bytes()))
        .collect()
}

#[test]
fn the_parser_matches_its_golden_on_every_example() {
    let files = read_all(&ply_files(&repo_root().join("examples")));
    let tally = check_all(&files);
    assert_eq!(tally.inputs, files.len());
    tally.report("examples");
}

#[test]
fn the_parser_matches_its_golden_on_the_shipped_standard_library() {
    let files = read_all(&ply_files(&repo_root().join("crates/ply-std/ply")));
    let tally = check_all(&files);
    assert_eq!(tally.inputs, files.len());
    tally.report("stdlib");
}

#[test]
fn the_parser_matches_its_golden_on_the_hand_written_fixtures() {
    let files = read_all(&ply_files(&fixtures()));
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
fn the_parser_matches_its_golden_on_the_mined_parser_inputs() {
    let inputs = mined();
    assert!(
        inputs.len() > 700,
        "the mined bundle holds {} fixtures; some have been lost",
        inputs.len()
    );
    let tally = check_all(&inputs);
    assert_eq!(tally.inputs, inputs.len());
    tally.report("mined parser inputs");
}

/// A tag no input reaches is a construct the goldens above say nothing about.
#[test]
fn the_goldens_reach_every_tag_the_parser_can_emit() {
    let mut inputs = read_all(&ply_files(&repo_root().join("examples")));
    inputs.extend(read_all(&ply_files(
        &repo_root().join("crates/ply-std/ply"),
    )));
    inputs.extend(read_all(&ply_files(&fixtures())));
    inputs.extend(mined());
    let mut seen: Vec<String> = inputs
        .iter()
        .flat_map(|(_, bytes)| tags(&port::dump("items.dump", bytes)))
        .collect();
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
        "  tag coverage: {} of {} tags; unreached: {:?}",
        EMITTABLE.len() - missing.len(),
        EMITTABLE.len(),
        missing
    );
    assert!(
        unlisted.is_empty(),
        "the parser emitted tags this list does not name, so the coverage figure is wrong: \
         {unlisted:?}"
    );
    assert!(
        missing.is_empty(),
        "the corpus and fixtures no longer reach {missing:?}; a fixture that covered them \
         has been deleted or changed"
    );
}

/// Every tag the parser's dump can emit.
const EMITTABLE: [&str; 101] = [
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
