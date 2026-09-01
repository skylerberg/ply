//! `ply check --types` over a service, and the provenance `--explain` adds.
//!
//! W3's exit criterion is that an endpoint's declared signature says which
//! resources it touches, read off one command. Everything here is a claim about
//! that output — which means about the *bytes*, because this is an artifact a
//! reviewer diffs, not a debug dump.
//!
//! Two rules from ADR 0013 §1.7 are what most of these tests check:
//!
//! - **The expansion is printed, always, and the alias never is.** The truth
//!   needs no flag; `--explain` adds the abbreviation back as provenance.
//! - **`--explain`'s bytes do not depend on the cache.** Gate 1 skips a file
//!   whose bytes are unchanged, and the set table is read from the AST — so a
//!   warm run and a cold one must print the same thing or the reviewing command
//!   is reporting on what the cache held.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// A service with two `effect set`s, an endpoint that performs less than it
/// declares, and endpoints that write their rows out. Small enough to pin
/// exactly and wide enough that every branch of the printer runs.
const SERVICE: &str = r#"
effect store {
  read  all[table]() -> List<Int>
  write save[table](rows: List<Int>) -> Unit
}

effect set Reads = { store.read[orders], store.read[users] }

effect set Web = { Reads, store.read[inventory], store.write[orders] }

effect set Unused = { store.read[audit] }

pub fn table() -> List<Int> = [1, 2, 3]

pub fn health() -> Int = 200

pub fn list_orders() -> Int / {Reads} = len(store.all[orders]())

pub fn create_order() -> Int / {Web} = {
  store.save[orders](push(store.all[orders](), 1));
  len(store.all[inventory]())
}

pub fn audit() -> Int / {store.read[audit], store.write[audit]} = {
  store.save[audit]([1]);
  len(store.all[audit]())
}
"#;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("m.ply"), source).expect("the fixture is written");
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the binary is built");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

/// The block one module contributes, from its heading to the end.
fn module_block(text: &str) -> String {
    let start = text
        .find("\n   m ")
        .unwrap_or_else(|| panic!("no module heading in:\n{text}"));
    text[start + 1..].trim_end().to_string()
}

/// Every line a row occupies: the `/ {` line and the continuations hanging
/// under it. A type is not wrapped — it is one token stream and breaking it
/// would cost more legibility than it bought — so the width claim is about the
/// part that grows without bound, which is the row.
fn row_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut hanging = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        match hanging {
            Some(column) if indent == column && !line.contains(" : ") => out.push(line),
            _ => hanging = None,
        }
        if let Some(slash) = line.find("/ {")
            && line[..slash].trim().is_empty()
        {
            out.push(line);
            hanging = Some(slash + 3);
        }
    }
    out
}

fn types(dir: &Path, extra: &[&str]) -> String {
    let output = ply(dir)
        .arg("check")
        .arg("--types")
        .args(extra)
        .output()
        .expect("ply ran");
    assert!(output.status.success(), "{}", stdout_of(&output));
    stdout_of(&output)
}

// --- the signatures ---------------------------------------------------------

/// The exit criterion, pinned whole. A row on its own wrapped line under the
/// type, a pure definition with no row at all, and the atoms of an aliased row
/// spelled out — because a row that runs off the right edge is a row nobody
/// reads, which is the failure this milestone exists to prevent.
#[test]
fn a_services_per_endpoint_footprints_are_legible_in_one_command() {
    let dir = project(SERVICE);
    let block = module_block(&types(dir.path(), &[]));
    assert_eq!(
        block,
        [
            "   m m.ply",
            "     effect store",
            "       read all[r]() -> List<Int>",
            "       write save[r](List<Int>) -> Unit",
            "     table        : () -> List<Int>",
            "     health       : () -> Int",
            "     list_orders  : () -> Int",
            "                    / {m.store.read[orders], m.store.read[users]}",
            "     create_order : () -> Int",
            "                    / {m.store.read[inventory], m.store.read[orders],",
            "                       m.store.write[orders], m.store.read[users]}",
            "     audit        : () -> Int",
            "                    / {m.store.read[audit], m.store.write[audit]}",
        ]
        .join(
            "
"
        )
    );
}

/// ADR 0013 §1.7 in its strongest form: the truth needs no flag. An alias that
/// hid what a definition touches would defeat its own purpose, so the reviewing
/// command prints the expansion and the name appears nowhere.
#[test]
fn the_expansion_is_printed_without_a_flag_and_the_alias_is_not() {
    let dir = project(SERVICE);
    let text = types(dir.path(), &[]);
    for alias in ["Web", "Reads", "Unused"] {
        assert!(
            !text.contains(alias),
            "`ply check --types` printed the alias `{alias}`:\n{text}"
        );
    }
    assert!(text.contains("m.store.write[orders]"), "{text}");
}

/// No line may exceed the fixed width, and the width is fixed rather than the
/// terminal's: this output is diffed, so a run in a narrow window and a run in
/// a wide one have to produce the same bytes.
#[test]
fn no_signature_line_runs_past_the_column_it_wraps_at() {
    let dir = project(SERVICE);
    for text in [types(dir.path(), &[]), types(dir.path(), &["--explain"])] {
        for line in text.lines() {
            assert!(
                line.chars().count() <= 80,
                "{} columns: {line}",
                line.chars().count()
            );
        }
    }
}

// --- --explain --------------------------------------------------------------

/// The set table and the provenance, pinned whole. The alias is here and only
/// here, beside the row it stands for — an abbreviation printed without its
/// expansion is the abbreviation without the definition.
#[test]
fn explain_prints_the_set_table_the_alias_and_the_difference_it_hides() {
    let dir = project(SERVICE);
    let block = module_block(&types(dir.path(), &["--explain"]));
    assert_eq!(
        block,
        [
            "   m m.ply",
            "     effect store",
            "       read all[r]() -> List<Int>",
            "       write save[r](List<Int>) -> Unit",
            "",
            "     effect set Reads",
            "       = {m.store.read[orders], m.store.read[users]}",
            "       used by 2 definitions",
            "",
            "     effect set Web",
            "       = {m.store.read[inventory], m.store.read[orders], m.store.write[orders],",
            "          m.store.read[users]}",
            "       used by 1 definition",
            "",
            "     effect set Unused",
            "       = {m.store.read[audit]}",
            "       used by 0 definitions",
            "",
            "     table        : () -> List<Int>",
            "     health       : () -> Int",
            "     list_orders  : () -> Int",
            "                    / {m.store.read[orders], m.store.read[users]}",
            "       written as     / {Reads}",
            "       body performs  {m.store.read[orders]}",
            "       declared, not performed: m.store.read[users]",
            "     create_order : () -> Int",
            "                    / {m.store.read[inventory], m.store.read[orders],",
            "                       m.store.write[orders], m.store.read[users]}",
            "       written as     / {Web}",
            "       body performs  {m.store.read[inventory], m.store.read[orders],",
            "                       m.store.write[orders]}",
            "       declared, not performed: m.store.read[users]",
            "     audit        : () -> Int",
            "                    / {m.store.read[audit], m.store.write[audit]}",
        ]
        .join(
            "
"
        )
    );
}

/// A set used through another set is used. `Reads` is named directly by
/// `list_orders` and reached by `create_order` through `Web`, and an included
/// set reporting zero uses while its atoms are in half the module would be a
/// number that reads as dead code.
#[test]
fn an_included_set_counts_the_definitions_that_reach_it() {
    let dir = project(SERVICE);
    let text = types(dir.path(), &["--explain"]);
    assert!(text.contains("effect set Reads"), "{text}");
    let reads = text
        .split("effect set Reads")
        .nth(1)
        .expect("the block is there");
    assert!(
        reads.contains("used by 2 definitions"),
        "`Reads` is named by `list_orders` and reached through `Web`:\n{text}"
    );
}

/// ADR 0013 §1.6: `--explain` must print the same bytes for a warm run and a
/// cold one, or the reviewing command's output is a function of what the cache
/// held. Gate 1 skips a file whose bytes are unchanged and the set table is
/// read from the AST, so this is the test that keeps the two from drifting.
#[test]
fn explain_prints_the_same_bytes_whether_gate_one_parsed_the_file_or_skipped_it() {
    let dir = project(SERVICE);
    let cold = types(dir.path(), &["--explain"]);
    let warm = types(dir.path(), &["--explain"]);
    let fresh = types(dir.path(), &["--explain", "--no-incremental"]);

    assert!(
        warm.contains("skipped"),
        "the second run must have skipped the file, or this proves nothing:\n{warm}"
    );
    assert_eq!(module_block(&cold), module_block(&warm));
    assert_eq!(module_block(&cold), module_block(&fresh));
}

/// The front-end report says what the gates actually decided. Completing the
/// parse so the set table can be printed must not rewrite it into a run that
/// parsed everything.
#[test]
fn completing_the_parse_does_not_rewrite_the_report_of_what_the_gates_decided() {
    let dir = project(SERVICE);
    types(dir.path(), &["--explain"]);
    let warm = types(dir.path(), &["--explain"]);
    let front_end = warm
        .split("front-end time")
        .next()
        .expect("a front-end block");
    assert!(front_end.contains("skipped"), "{warm}");
    assert!(!front_end.contains("checked   "), "{warm}");
}

// --- --json -----------------------------------------------------------------

fn json_types(dir: &Path, extra: &[&str]) -> Value {
    let output = ply(dir)
        .arg("check")
        .arg("--json")
        .args(extra)
        .output()
        .expect("ply ran");
    let text = stdout_of(&output);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("stdout was not one object: {e}\n{text}"))
}

/// The provenance an agent reads, and the rule that keeps it honest: present
/// only under `--explain`, where the parse is completed first, so the object
/// either carries these fields or does not — and never carries a subset that
/// depends on which files gate 1 skipped.
#[test]
fn the_json_report_carries_the_provenance_only_under_explain() {
    let dir = project(SERVICE);

    let plain = json_types(dir.path(), &[]);
    assert_eq!(plain["modules"][0]["effect_sets"], Value::Null);
    assert_eq!(plain["definitions"][0]["written_as"], Value::Null);

    let explained = json_types(dir.path(), &["--explain"]);
    let sets = explained["modules"][0]["effect_sets"]
        .as_array()
        .expect("a set table");
    let names: Vec<&str> = sets.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Reads", "Web", "Unused"]);
    assert_eq!(sets[0]["used_by"], 2);
    assert_eq!(
        sets[0]["expansion"],
        serde_json::json!(["m.store.read[orders]", "m.store.read[users]"])
    );

    let defs = explained["definitions"].as_array().expect("definitions");
    let orders = defs
        .iter()
        .find(|d| d["name"] == "m.list_orders")
        .expect("list_orders");
    assert_eq!(orders["written_as"], serde_json::json!(["Reads"]));
    assert_eq!(
        orders["performed"],
        serde_json::json!(["m.store.read[orders]"])
    );
    assert_eq!(
        orders["declared_not_performed"],
        serde_json::json!(["m.store.read[users]"])
    );

    // Exactly one object on stdout, whatever the flags say.
    assert!(explained.is_object());
}

/// `--json --explain` completes the parse too, so its provenance cannot be a
/// function of what the cache held either.
#[test]
fn the_json_provenance_is_the_same_warm_and_cold() {
    let dir = project(SERVICE);
    let cold = json_types(dir.path(), &["--explain"]);
    let warm = json_types(dir.path(), &["--explain"]);
    assert_eq!(
        cold["modules"][0]["effect_sets"],
        warm["modules"][0]["effect_sets"]
    );
    assert_eq!(cold["definitions"], warm["definitions"]);
}

// --- the real service -------------------------------------------------------

fn repo(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The claim on the example service rather than on a fixture only this file has
/// ever seen: every endpoint that touches a resource prints a row, the pure
/// parts of routing print none, and no alias name reaches the output.
#[test]
fn the_example_service_reads_as_a_map_of_the_api_to_what_it_touches() {
    let desk = repo("examples/desk.ply");
    if !desk.exists() {
        return;
    }
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::copy(&desk, dir.path().join("desk.ply")).expect("the example is copied");

    let text = types(dir.path(), &[]);
    assert!(
        text.contains("table") && text.contains(" / {"),
        "a multi-route service must print both pure definitions and rows:\n{text}"
    );
    for line in row_lines(&text) {
        assert!(
            line.chars().count() <= 80,
            "{} columns: {line}",
            line.chars().count()
        );
    }

    // Whatever sets the example declares, `--types` alone names none of them.
    let source = std::fs::read_to_string(&desk).expect("the example is readable");
    for line in source.lines() {
        let Some(rest) = line.strip_prefix("effect set ") else {
            continue;
        };
        let name = rest
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .next()
            .expect("a set name");
        assert!(
            !text.contains(name),
            "`ply check --types` printed the alias `{name}`:\n{text}"
        );
        assert!(
            types(dir.path(), &["--explain"]).contains(&format!("effect set {name}")),
            "`--explain` must add `{name}` back as provenance"
        );
    }
}

// --- ply prove --------------------------------------------------------------

/// ADR 0013 §1.6: the footprint is the frame condition, so an annotation wider
/// than the body promises less about less — at the same tier and with no other
/// sign that anything was lost. On a definition carrying an obligation that is
/// a weakened claim rather than a scheduling cost, so `ply prove --explain`
/// names the atoms too.
#[test]
fn prove_explain_names_what_an_over_broad_row_gave_up() {
    let dir = project(
        "\
effect store {
  read  all[table]() -> List<Int>
  write save[table](rows: List<Int>) -> Unit
}

effect set Web = { store.read[orders], store.read[users], store.write[orders] }

pub fn count() -> Int / {Web}
  ensures result >= 0
  = len(store.all[orders]())
",
    );
    let output = ply(dir.path())
        .arg("prove")
        .arg("--explain")
        .output()
        .expect("ply ran");
    let text = stdout_of(&output);
    assert!(
        text.contains("frame covers, body never touches:"),
        "an `ensures` under a wider row must say what it gave up:\n{text}"
    );
    assert!(text.contains("m.store.read[users]"), "{text}");
    assert!(
        !text.contains("Web"),
        "the alias name is provenance, never the claim:\n{text}"
    );

    // A definition whose row is exactly its body's gives nothing up, so there
    // is nothing to report and the line is absent rather than empty.
    let tight = project("pub fn double(x: Int) -> Int\n  ensures result >= x\n  = x + x\n");
    let text = stdout_of(
        &ply(tight.path())
            .arg("prove")
            .arg("--explain")
            .output()
            .unwrap(),
    );
    assert!(!text.contains("frame covers"), "{text}");
}
