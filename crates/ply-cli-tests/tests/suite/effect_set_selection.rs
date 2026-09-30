use crate::harness::{ply, stdout_of};
use serde_json::Value;
use tempfile::TempDir;

fn combined(out: &std::process::Output) -> String {
    format!("{}{}", stdout_of(out), String::from_utf8_lossy(&out.stderr))
}

/// `store` is handled in both tests, so both are cacheable and "selected zero" is a statement about the cache.
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
    // The row has to be renamed with it, so the edit is the rename and nothing else.
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

#[test]
fn replacing_a_set_with_its_expansion_selects_no_tests() {
    let dir = project(NARROW);
    let warm = ply(dir.path()).arg("test").output().expect("ply test runs");
    assert_eq!(warm.status.code(), Some(0), "{}", combined(&warm));

    // Written in the other order too: the annotation's spelling may not decide what a row means.
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

#[test]
fn widening_a_set_selects_exactly_the_tests_that_reach_it() {
    let wider = "effect set Web = {store.read[orders], store.write[audit], store.read[inventory]}";
    assert_eq!(selected_after(NARROW, wider), 1);
}

/// A cold cache never exercises an invalidation, and an invalidation is the only thing that can be wrong.
#[test]
fn a_warm_check_agrees_with_a_cold_one_across_a_sequence_of_set_edits() {
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
        crate::harness::warm_agrees(dir.path(), &format!("step {step}"));
    }
}
