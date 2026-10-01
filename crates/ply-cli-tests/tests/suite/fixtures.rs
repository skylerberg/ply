//! Every program under `tests/fixtures` has a test here named for it, which `ply check`s it before
//! anything runs it for what it is for: one that exists to be refused is refused with exactly the
//! errors listed beside its name, and every other one checks clean. A fixture a change to the
//! language breaks fails as itself, rather than as the outcome some other test is waiting for.

use crate::harness::{copy_sources, json_of, ply, repo, scratch};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixture_root() -> PathBuf {
    repo().join("tests/fixtures")
}

/// The fixture `name`, a file or a directory, as a project of its own, so nothing a run writes
/// lands in the tree.
fn copied(name: &str) -> TempDir {
    let project = scratch();
    let file = format!("{name}.ply");
    let from = fixture_root().join(&file);
    if from.is_file() {
        std::fs::copy(&from, project.path().join(&file)).expect("the fixture is copied");
    } else {
        copy_sources(&fixture_root().join(name), project.path());
    }
    project
}

fn listing(errors: &[&Value]) -> String {
    errors
        .iter()
        .map(|d| {
            let at = d["labels"]
                .as_array()
                .and_then(|labels| labels.iter().find(|l| l["primary"] == true))
                .map(|l| {
                    format!(
                        " at {}:{}",
                        l["file"].as_str().unwrap_or("?"),
                        l["start"]["line"]
                    )
                })
                .unwrap_or_default();
            format!(
                "  {} {}{at}\n",
                d["code"].as_str().unwrap_or("?"),
                d["message"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

#[track_caller]
fn checks_as_listed(name: &str, refused: &[&str]) {
    let project = copied(name);
    let answer = json_of(
        &ply(project.path())
            .args(["check", "--json"])
            .output()
            .expect("`ply check` runs"),
    );
    let errors: Vec<&Value> = answer["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("`ply check` answered no diagnostics array: {answer}"))
        .iter()
        .filter(|d| d["severity"] == "error")
        .collect();
    let mut found: Vec<&str> = errors
        .iter()
        .map(|d| d["code"].as_str().unwrap_or_default())
        .collect();
    found.sort_unstable();
    let mut listed = refused.to_vec();
    listed.sort_unstable();
    if listed.is_empty() {
        assert!(
            found.is_empty(),
            "`tests/fixtures/{name}` does not compile, so nothing that runs it reaches what it is \
             for:\n{}",
            listing(&errors)
        );
    } else {
        assert_eq!(
            found,
            listed,
            "`tests/fixtures/{name}` exists to be refused with exactly the errors listed for it:\n{}",
            listing(&errors)
        );
    }
}

/// A test per fixture, named for it, with the errors `ply check` refuses it with: none for a
/// program that is run for what it does.
macro_rules! fixtures {
    ($($name:ident: [$($code:literal),*],)*) => {
        const LISTED: &[&str] = &[$(stringify!($name)),*];
        $(
            #[test]
            fn $name() {
                checks_as_listed(stringify!($name), &[$($code),*]);
            }
        )*
    };
}

fixtures! {
    ambiguous_import: ["E0108"],
    arity_mismatch: ["E0202"],
    artifact_invalid: [],
    artifact_version: [],
    assertion_failed: [],
    bank_race: [],
    concurrency_law_binder: [],
    config_invalid: [],
    config_missing: [],
    config_unavailable: [],
    config_undeclared: [],
    deadlock: [],
    decimal_division: ["E0209", "E0209"],
    drain_incomplete: [],
    duplicate_definition: ["E0105"],
    duplicate_import: ["E0110"],
    effect_in_spec: ["E0417", "E0417", "E0417"],
    effect_not_permitted: ["E0302"],
    effect_set_cycle: ["E0115"],
    invalid_module_path: ["E0111"],
    lang: [],
    module_cycle: ["E0109"],
    multi_shot_clause: [],
    nested_simulation: ["E0416", "E0416"],
    non_exhaustive_match: ["E0205"],
    nondet_fail: ["E0412"],
    nondet_in_det_test: ["E0412"],
    not_a_function: ["E0204"],
    not_derivable: ["E0206"],
    not_derivable_map_key: ["E0206", "E0206"],
    obligation_not_discharged: [],
    occurs_check: ["E0203"],
    orphan_derive: ["E0208"],
    private_name: ["E0107"],
    record_update_field: ["E0117"],
    record_update_shape: ["E0116", "E0116"],
    refuted_law: [],
    reserved_module_name: ["E0133"],
    resource_required: ["E0304"],
    runtime_error: [],
    secret_containment: ["E0101", "E0101", "E0201", "E0201", "E0201", "E0205", "E0418"],
    secret_not_derivable: ["E0206", "E0206"],
    self_handled_effect: [],
    span_abandoned: [],
    span_unbalanced: [],
    task_escapes_scope: ["E0413"],
    tls_credential_invalid: [],
    tls_credential_unknown: [],
    try_position: ["E0119"],
    try_scope: ["E0118"],
    type_mismatch: ["E0201"],
    unbound_row_var: ["E0301"],
    unexpected_token: ["E0001"],
    unhandled_effect: [],
    unknown_deriver: ["E0207", "E0207"],
    unknown_effect: ["E0103"],
    unknown_effect_set: ["E0114", "E0114"],
    unknown_module: ["E0106"],
    unknown_name: ["E0101"],
    unknown_operation: ["E0104"],
    unknown_type: ["E0102"],
    unquantifiable_type: ["E0418", "E0418", "E0418"],
    unscheduled_task: ["E0412", "E0412"],
    unterminated_string: ["E0002"],
    vacuous_law: [],
}

/// A fixture with no test here is one a change can break with nothing failing.
#[test]
fn every_fixture_is_listed() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(fixture_root())
        .expect("tests/fixtures is readable")
        .map(|entry| entry.expect("a directory entry").path())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            // `ply` leaves a `.ply-cache` beside a program checked where it lies.
            if name.starts_with('.') {
                None
            } else if path.is_dir() {
                Some(name)
            } else {
                name.strip_suffix(".ply").map(str::to_string)
            }
        })
        .collect();
    let listed: BTreeSet<String> = LISTED.iter().map(|name| name.to_string()).collect();
    let unlisted: Vec<&String> = on_disk.difference(&listed).collect();
    let gone: Vec<&String> = listed.difference(&on_disk).collect();
    assert!(
        unlisted.is_empty(),
        "these fixtures have no test: list each in `fixtures!` with the errors `ply check` refuses \
         it with, or none: {unlisted:?}"
    );
    assert!(
        gone.is_empty(),
        "`fixtures!` lists these, and `tests/fixtures` holds no such program: {gone:?}"
    );
}

/// Run as its header says, `span_abandoned.ply` answers its value with two spans still open; the
/// run closes them `Abandoned`, warns of both innermost first, and still exits 0, since a warning
/// is no failure.
#[test]
fn a_run_warns_of_the_spans_its_entry_left_open_and_exits_zero() {
    let project = copied("span_abandoned");
    let run = ["run", "--host", "--trace", "json"];
    let warning = "2 spans were still open when their task or the entry point ended: `reserving` on \
                   `orders`, `order` on `orders`";

    let out = ply(project.path())
        .args(run)
        .arg("span_abandoned.ply")
        .output()
        .expect("`ply run` runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .map(str::trim),
        Some("3"),
        "the entry's value is the answer: {stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.trim() == format!("warning: {warning}")),
        "the run says nothing of the spans it closed: {stderr}"
    );
    let abandoned: Vec<Value> = stderr
        .lines()
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["outcome"] == "abandoned")
        .collect();
    let closed: Vec<&str> = abandoned
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(closed, ["reserving", "order"], "{stderr}");

    let document = json_of(
        &ply(project.path())
            .args(run)
            .args(["--json", "span_abandoned.ply"])
            .output()
            .expect("`ply run --json` runs"),
    );
    assert_eq!(document["exit_code"], 0, "{document}");
    let warned: Vec<&Value> = document["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("the document carries diagnostics: {document}"))
        .iter()
        .filter(|d| d["code"] == "W0609")
        .collect();
    assert_eq!(warned.len(), 1, "{document}");
    assert_eq!(warned[0]["severity"], "warning", "{document}");
    assert_eq!(warned[0]["message"], warning, "{document}");
    assert_eq!(
        document["shutdown"]["spans_left_open"], 2,
        "the report counts the spans the warning names: {document}"
    );
}

fn tested(dir: &Path, flags: &[&str]) -> Value {
    json_of(
        &ply(dir)
            .args(["test", "--json", "--no-cache"])
            .args(flags)
            .output()
            .expect("`ply test` runs"),
    )
}

#[track_caller]
fn only_failure(report: &Value) -> &Value {
    match report["failures"].as_array().map(Vec::as_slice) {
        Some([failure]) => failure,
        _ => panic!("expected exactly one failing test: {report}"),
    }
}

/// The byte range of `text` in the fixture `name`, which writes it once.
fn written_in(name: &str, text: &str) -> (u64, u64) {
    let source = std::fs::read_to_string(fixture_root().join(format!("{name}.ply")))
        .expect("the fixture is readable");
    let found: Vec<usize> = source.match_indices(text).map(|(at, _)| at).collect();
    let [at] = found[..] else {
        panic!("`{name}` writes `{text}` {} times, not once", found.len());
    };
    (at as u64, (at + text.len()) as u64)
}

/// Run end to end, `bank_race.ply`'s two transfers pass; the search moves one's balance check ahead
/// of the other's debit, so both pass a check only one should. The failure names the two sides of
/// that reordering, each at the perform in `transfer` that made its access, and the seed it prints
/// replays it.
#[test]
fn the_search_finds_the_bank_race_and_its_seed_replays_it() {
    let project = copied("bank_race");
    let searched = tested(project.path(), &[]);
    assert_eq!(searched["exit_code"], 1, "{searched}");
    let failure = only_failure(&searched);
    assert_eq!(
        failure["key"], "bank_race.no account is ever overdrawn",
        "{failure}"
    );
    assert_eq!(failure["diagnostic"]["code"], "E0501", "{failure}");
    assert_eq!(
        failure["diagnostic"]["message"], "assertion failed: expected 0, found 1",
        "alice is overdrawn: {failure}"
    );

    let race = &failure["race"];
    let sides = [&race["left"], &race["right"]];
    let tasks: BTreeSet<&str> = sides.iter().filter_map(|s| s["task"].as_str()).collect();
    let accesses: BTreeSet<&str> = sides.iter().filter_map(|s| s["access"].as_str()).collect();
    assert_eq!(
        tasks,
        BTreeSet::from(["@1", "@2"]),
        "the race is between the two transfers: {race}"
    );
    assert_eq!(
        accesses,
        BTreeSet::from([
            "bank_race.bank.read[accounts]",
            "bank_race.bank.write[accounts]"
        ]),
        "one transfer's balance check is reordered against the other's debit: {race}"
    );
    for (access, perform) in [
        (
            "bank_race.bank.read[accounts]",
            "bank.balance[accounts](from)",
        ),
        (
            "bank_race.bank.write[accounts]",
            "bank.credit[accounts](from, -amount)",
        ),
    ] {
        let side = sides
            .iter()
            .find(|s| s["access"] == access)
            .unwrap_or_else(|| panic!("no side of the race is `{access}`: {race}"));
        assert_eq!(
            side["definition"], "bank_race.transfer",
            "`{access}` is named in the definition that performed it: {race}"
        );
        let (start, end) = written_in("bank_race", perform);
        assert_eq!(
            (side["span"]["start"].as_u64(), side["span"]["end"].as_u64()),
            (Some(start), Some(end)),
            "`{access}` is placed at `{perform}`, where its step first touched the accounts: {race}"
        );
    }

    let seed = failure["seed"]
        .as_str()
        .unwrap_or_else(|| panic!("a failure the search found carries its seed: {failure}"));
    let test = "no account is ever overdrawn";
    assert_eq!(
        failure["replay"],
        format!("ply test --seed {seed} --filter \"{test}\""),
        "{failure}"
    );
    let replayed = tested(project.path(), &["--seed", seed, "--filter", test]);
    assert_eq!(
        only_failure(&replayed)["diagnostic"]["message"],
        failure["diagnostic"]["message"],
        "the seed the failure printed does not replay it: {replayed}"
    );
}
