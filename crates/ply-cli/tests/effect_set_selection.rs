//! What an edit to an `effect set` costs, measured in tests re-run.
//!
//! ADR 0013 §1.5's table is a claim about hashes, and a hash is only worth
//! anything because selection is keyed on it. So the same table is asserted here
//! from the other end: renaming a set, reordering its members or declaring an
//! unused one selects **zero** tests, and widening one selects exactly the tests
//! that reach the definitions annotated with it.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the `ply` binary is built");
    cmd.current_dir(dir);
    cmd
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn combined(out: &std::process::Output) -> String {
    format!("{}{}", stdout_of(out), String::from_utf8_lossy(&out.stderr))
}

/// `store` is handled in both tests, so both are `det` and both are cacheable —
/// which is what makes "selected zero" a statement about the cache rather than
/// about a test that could not be cached anyway.
fn source(web: &str) -> String {
    format!(
        r#"
effect store {{
  read  all[r]() -> List<Int>
  write save[r](rows: List<Int>) -> Unit
}}

{web}

fn list_orders() -> Int / {{Web}} = len(store.all[orders]())

fn health() -> Int = 200

test "orders are listed" {{
  handle {{ assert_eq(list_orders(), 0) }} with {{ store.all[orders]() -> [] }}
}}

test "health is 200" {{
  assert_eq(health(), 200)
}}
"#
    )
}

const NARROW: &str = "effect set Web = {store.read[orders], store.write[audit]}";

fn project(web: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(dir.path().join("m.ply"), source(web)).expect("the module is writable");
    dir
}

/// Runs `ply test` once to warm the cache, then edits and reports how many tests
/// the next run selected.
#[track_caller]
fn selected_after(before: &str, after: &str) -> u64 {
    let dir = project(before);
    let warm = ply(dir.path()).arg("test").output().expect("ply test runs");
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));

    std::fs::write(dir.path().join("m.ply"), source(after)).expect("the module is writable");
    let out = ply(dir.path())
        .args(["test", "--json"])
        .output()
        .expect("ply test runs");
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    let report: Value = serde_json::from_str(&stdout_of(&out))
        .unwrap_or_else(|e| panic!("{e}: {}", stdout_of(&out)));
    report["selection"]["selected"]
        .as_u64()
        .unwrap_or_else(|| panic!("no selection count in {report}"))
}

#[test]
fn renaming_a_set_selects_no_tests() {
    let renamed = "effect set Surface = {store.read[orders], store.write[audit]}";
    // The row has to be renamed with it, so the edit is the rename and nothing
    // else.
    let dir = project(NARROW);
    let warm = ply(dir.path()).arg("test").output().expect("ply test runs");
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));

    let after = source(renamed).replace("/ {Web}", "/ {Surface}");
    std::fs::write(dir.path().join("m.ply"), after).expect("the module is writable");
    let out = ply(dir.path())
        .args(["test", "--json"])
        .output()
        .expect("ply test runs");
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    let report: Value = serde_json::from_str(&stdout_of(&out)).expect("json");
    assert_eq!(
        report["selection"]["selected"], 0,
        "a set's name is namespace metadata: {report}"
    );
}

#[test]
fn reordering_a_sets_members_selects_no_tests() {
    let reordered = "effect set Web = {store.write[audit], store.read[orders]}";
    assert_eq!(selected_after(NARROW, reordered), 0);
}

#[test]
fn writing_a_member_twice_selects_no_tests() {
    let twice = "effect set Web = {store.read[orders], store.write[audit], store.read[orders]}";
    assert_eq!(selected_after(NARROW, twice), 0);
}

#[test]
fn declaring_a_set_nothing_uses_selects_no_tests() {
    let extra = "effect set Web = {store.read[orders], store.write[audit]}\n\
                 effect set Unused = {store.read[inventory]}";
    assert_eq!(selected_after(NARROW, extra), 0);
}

/// Rewriting the row from the set to the atoms it stands for is the headline
/// property, and selection is where it is felt: the definition did not change,
/// so nothing re-runs.
#[test]
fn replacing_a_set_with_its_expansion_selects_no_tests() {
    let dir = project(NARROW);
    let warm = ply(dir.path()).arg("test").output().expect("ply test runs");
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));

    // Written in the other order, too: a row is a set and the annotation's
    // spelling may not decide what it means.
    let after = source("").replace("/ {Web}", "/ {store.write[audit], store.read[orders]}");
    std::fs::write(dir.path().join("m.ply"), after).expect("the module is writable");
    let out = ply(dir.path())
        .args(["test", "--json"])
        .output()
        .expect("ply test runs");
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    let report: Value = serde_json::from_str(&stdout_of(&out)).expect("json");
    assert_eq!(
        report["selection"]["selected"], 0,
        "the alias was an abbreviation: {report}"
    );
}

/// Not a regression. A `/ {..}` annotation is the published signature, so
/// widening the set widens what `list_orders` promises its callers — and gate 2
/// only rechecks a definition whose own hash moved. Exactly the test reaching it
/// is selected, and the one that does not is left cached.
#[test]
fn widening_a_set_selects_exactly_the_tests_that_reach_it() {
    let wider = "effect set Web = {store.read[orders], store.write[audit], store.read[inventory]}";
    assert_eq!(selected_after(NARROW, wider), 1);
}

/// The equivalence property, across a sequence of `effect set` edits: a cold
/// cache never exercises an invalidation, and an invalidation is the only thing
/// that can be wrong.
#[test]
fn incremental_and_from_scratch_agree_across_a_sequence_of_set_edits() {
    use ply_cli::driver;
    use ply_cli::load::Loaded;
    use ply_store::Store;
    use std::collections::BTreeMap;

    fn snapshot(loaded: &Loaded) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (name, hash) in &loaded.hashes.defs {
            out.insert(format!("hash {name}"), hash.to_hex());
        }
        for (name, def) in &loaded.check.defs {
            out.insert(format!("scheme {name}"), format!("{:?}", def.scheme));
            out.insert(format!("footprint {name}"), def.footprint.to_string());
            out.insert(format!("performed {name}"), def.performed.to_string());
            out.insert(format!("aliases {name}"), format!("{:?}", def.row_aliases));
        }
        for (i, test) in loaded.check.tests.iter().enumerate() {
            let hash = loaded
                .hashes
                .tests
                .get(i)
                .map(|h| h.to_hex())
                .unwrap_or_default();
            out.insert(
                format!("test {i}"),
                format!("{} {} {hash}", test.key, test.footprint),
            );
        }
        out
    }

    let dir = project(NARROW);
    let sequence = [
        NARROW,
        "effect set Web = {store.write[audit], store.read[orders]}",
        "effect set Inner = {store.read[orders]}\n\
         effect set Web = {Inner, store.write[audit]}",
        "effect set Inner = {store.read[orders], store.read[inventory]}\n\
         effect set Web = {Inner, store.write[audit]}",
        "effect set Web = {store.read[orders], store.write[audit]}\n\
         effect set Unused = {store.read[nothing]}",
    ];
    for (step, web) in sequence.iter().enumerate() {
        std::fs::write(dir.path().join("m.ply"), source(web)).expect("the module is writable");
        let mut store = Store::open(dir.path()).expect("the cache is creatable");
        let incremental = driver::load_incremental(dir.path(), &mut store)
            .unwrap_or_else(|e| panic!("step {step}: incremental failed: {e:?}"));
        let full = driver::load_full(dir.path())
            .unwrap_or_else(|e| panic!("step {step}: from scratch failed: {e:?}"));
        assert_eq!(
            snapshot(&incremental),
            snapshot(&full),
            "step {step}: the two paths disagreed"
        );
    }
}
