//! Deployment over the content-addressed store: ADR 0015 §5.

use assert_cmd::Command;
use ply_cli::artifact::{self, Artifact};
use ply_cli::load::{Loaded, load};
use ply_hash::DefHash;
use ply_span::codes;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// An effect, a type, a mutually recursive pair, a definition nothing reaches, a test and a law.
const PROGRAM: &str = r#"
effect log { write emit[c](msg: String) -> Unit }

type Colour = | Red | Blue(Int)

pub fn shade(c: Colour) -> Int = match c { Red -> 0, Blue(n) -> n }

pub fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }

pub fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }

pub fn announce(msg: String) -> Unit / {log.write[audit]} = log.emit[audit](msg)

fn unreached() -> Int = 99

fn main() -> Int =
  shade(Blue(20)) + shade(Blue(21)) + if even(2) { 1 } else { 0 }

test "shade reads a payload" { assert_eq(shade(Blue(7)), 7) }

law "even and odd disagree"
  forall (n: Int) where n >= 0 && n < 4 {
    even(n) != odd(n)
  }
"#;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn built(dir: &Path, sources: bool) -> (Loaded, artifact::Built) {
    let loaded = load(dir).expect("the corpus should load");
    let entry = ply_cli::commands::run::entry_point(&loaded).expect("`main` is the entry point");
    let built = artifact::build(&loaded, entry, &[], sources).expect("the closure should build");
    (loaded, built)
}

fn artifact_of(dir: &Path) -> Artifact {
    built(dir, false).1.artifact
}

fn write_artifact(at: &Path, artifact: &Artifact) {
    std::fs::write(at, artifact.encode()).unwrap();
}

fn json_of(output: &std::process::Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("stdout was not one object: {e}\n{text}"))
}

// --- reproducibility ---------------------------------------------------------

/// Two builds of one source tree, from two different absolute roots, one over a cold cache and one
/// over a cache a full `ply test` warmed.
#[test]
fn two_builds_from_two_roots_are_byte_identical() {
    let cold = project(PROGRAM);
    let warm = project(PROGRAM);
    assert_ne!(cold.path(), warm.path());

    ply(warm.path()).arg("test").assert().success();
    assert!(
        warm.path().join(".ply-cache").exists(),
        "the second root's cache should be warm"
    );

    let first = artifact_of(cold.path());
    let second = artifact_of(warm.path());
    assert_eq!(first.encode(), second.encode());
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.digest_short(), second.digest_short());
    assert!(first.digest_short().starts_with("b3:"));
    assert_eq!(first.digest_short().len(), 15);
}

/// The same claim through the binary, since that is what a deployment runs.
#[test]
fn ply_build_twice_writes_the_same_bytes() {
    let dir = project(PROGRAM);
    ply(dir.path())
        .args(["build", ".", "-o", "one.plyx"])
        .assert()
        .success();
    ply(dir.path())
        .args(["build", ".", "-o", "two.plyx"])
        .assert()
        .success();
    let one = std::fs::read(dir.path().join("one.plyx")).unwrap();
    let two = std::fs::read(dir.path().join("two.plyx")).unwrap();
    assert_eq!(one, two);
}

// --- what is in one ----------------------------------------------------------

/// A test is a definition nothing calls, so it is not in an entry point's closure — and neither is
/// a law, nor a definition nothing reaches.
#[test]
fn an_artifact_carries_no_test_no_law_and_nothing_unreached() {
    let dir = project(PROGRAM);
    let (loaded, built) = built(dir.path(), false);
    assert!(!loaded.check.tests.is_empty(), "the corpus declares a test");
    assert!(!loaded.check.laws.is_empty(), "the corpus declares a law");

    let names: Vec<&str> = built
        .artifact
        .names
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(names.contains(&"m.main"));
    assert!(names.contains(&"m.shade"));
    assert!(names.contains(&"m.Colour"));
    assert!(names.contains(&"m.even") && names.contains(&"m.odd"));
    assert!(
        !names.contains(&"m.unreached"),
        "a definition nothing reaches is not in the closure: {names:?}"
    );
    assert!(
        !names.contains(&"m.announce") && !names.contains(&"m.log"),
        "only what `main` reaches: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("ply_tests")),
        "no test key may appear: {names:?}"
    );

    let opened = artifact::open(&built.artifact, Path::new("t.plyx")).expect("it should open");
    assert!(opened.check.tests.is_empty(), "a test was deployed");
    assert!(opened.check.laws.is_empty(), "a law was deployed");
    assert!(
        opened.check.defs.values().all(|d| d.span.is_dummy()),
        "a deployed definition carried a span"
    );
    assert!(!opened.located, "a bodies-only artifact locates nothing");
}

/// Every definition in the artifact is filed under the hash `ply hash` gives it, which is what
/// makes the artifact and the store one namespace rather than two.
#[test]
fn every_body_is_filed_under_the_hash_ply_hash_prints() {
    let dir = project(PROGRAM);
    let output = ply(dir.path())
        .args(["hash", ".", "--json"])
        .output()
        .unwrap();
    let report = json_of(&output);

    let artifact = artifact_of(dir.path());
    let mut checked = 0;
    for entry in report["definitions"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let hash = DefHash::from_hex(entry["hash"].as_str().unwrap()).unwrap();
        if let Some((_, filed)) = artifact.names.iter().find(|(n, _)| n == name) {
            assert_eq!(*filed, hash, "`{name}` is filed under a different hash");
            assert!(artifact.bodies.contains_key(&hash));
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} definitions were compared");
}

// --- running one -------------------------------------------------------------

/// The whole point: the artifact answers what the source answers.
#[test]
fn an_artifact_runs_to_the_same_value_as_its_source() {
    let dir = project(PROGRAM);
    let from_source = ply(dir.path()).args(["run", "m.ply"]).output().unwrap();
    assert!(from_source.status.success());

    ply(dir.path())
        .args(["build", ".", "-o", "m.plyx"])
        .assert()
        .success();
    let from_artifact = ply(dir.path()).args(["run", "m.plyx"]).output().unwrap();
    assert!(from_artifact.status.success());

    let source_value = String::from_utf8(from_source.stdout).unwrap();
    let artifact_value = String::from_utf8(from_artifact.stdout).unwrap();
    assert!(source_value.contains("42"), "{source_value}");
    assert!(artifact_value.contains("42"), "{artifact_value}");
}

/// `--engine both` over an artifact is the claim that decoding a body did not change what it does,
/// and a deployed program is not a place to relax it.
#[test]
fn an_artifact_runs_on_both_engines_without_divergence() {
    let dir = project(PROGRAM);
    ply(dir.path())
        .args(["build", ".", "-o", "m.plyx"])
        .assert()
        .success();
    let output = ply(dir.path())
        .args(["run", "m.plyx", "--engine", "both", "--json"])
        .output()
        .unwrap();
    let report = json_of(&output);
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["value"], "42");
    assert_eq!(report["located"], false);
}

/// A run out of an artifact is hermetic for the same reason a run out of source is: nothing is
/// bound unless `--host` is written in the command.
#[test]
fn an_artifact_run_binds_nothing_without_host() {
    let dir = project(PROGRAM);
    ply(dir.path())
        .args(["build", ".", "-o", "m.plyx"])
        .assert()
        .success();
    let report = json_of(
        &ply(dir.path())
            .args(["run", "m.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["binding"], "hermetic");
}

/// With sources the text is re-parsed and re-checked, and then re-hashed and compared against the
/// artifact — so a failure carries a line number and the sources cannot be a different program than
/// the digest names.
#[test]
fn an_artifact_built_with_sources_locates_its_failures() {
    let dir = project("fn main() -> Int = 1 / 0\n");
    ply(dir.path())
        .args(["build", ".", "-o", "bare.plyx"])
        .assert()
        .success();
    ply(dir.path())
        .args(["build", ".", "-o", "src.plyx", "--sources"])
        .assert()
        .success();

    let bare = json_of(
        &ply(dir.path())
            .args(["run", "bare.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(bare["ok"], false);
    assert_eq!(bare["located"], false);

    let located = json_of(
        &ply(dir.path())
            .args(["run", "src.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(located["ok"], false);
    assert_eq!(located["located"], true);
    assert_eq!(located["diagnostics"][0]["labels"][0]["start"]["line"], 1);
}

/// Sources are believed only if they rebuild the artifact they arrived in.
#[test]
fn embedded_sources_that_are_not_the_sources_are_refused() {
    let dir = project(PROGRAM);
    let mut artifact = built(dir.path(), true).1.artifact;
    assert!(artifact.has_sources());
    for (_, text) in artifact.sources.iter_mut() {
        *text = text.replace("Blue(20)", "Blue(19)");
    }
    let diags = match artifact::open(&artifact, Path::new("t.plyx")) {
        Ok(_) => panic!("altered sources must not be believed"),
        Err(diags) => diags,
    };
    assert_eq!(diags[0].code, codes::ARTIFACT_INVALID);
}

// --- verification ------------------------------------------------------------

/// A flipped bit in one body is refused at that definition, naming the hash it was filed under and
/// where in the file it was — a per-definition refusal rather than a plausible wrong program.
#[test]
fn a_flipped_bit_in_a_body_is_e0443_naming_the_definition() {
    let dir = project(PROGRAM);
    let artifact = artifact_of(dir.path());
    let mut bytes = artifact.encode();

    // The first record starts past the header and three descriptors; its payload starts past the
    // 32-byte key and the length prefix.
    let record = 188 + 24 * 3;
    bytes[record + 36] ^= 0x40;
    let path = dir.path().join("bad.plyx");
    std::fs::write(&path, &bytes).unwrap();

    let err = artifact::decode(&bytes, &path).expect_err("a corrupt body must be refused");
    assert_eq!(err.code, codes::ARTIFACT_INVALID);
    assert!(
        err.message.contains(&format!("offset {record}")),
        "{}",
        err.message
    );
    let first = artifact.bodies.keys().next().unwrap();
    assert!(err.message.contains(&first.short()), "{}", err.message);

    let output = ply(dir.path())
        .args(["run", "bad.plyx", "--json"])
        .output()
        .unwrap();
    let report = json_of(&output);
    assert_eq!(report["ok"], false);
    assert_eq!(report["diagnostics"][0]["code"], "E0443");
}

/// Every truncation, not one: a reader that believed any prefix would believe a transfer that
/// stopped halfway.
#[test]
fn no_prefix_of_an_artifact_is_believed() {
    let dir = project(PROGRAM);
    let bytes = artifact_of(dir.path()).encode();
    let path = dir.path().join("t.plyx");
    for cut in [0, 1, 100, 187, 188, 200, bytes.len() / 2, bytes.len() - 1] {
        let err = artifact::decode(&bytes[..cut], &path)
            .expect_err("a truncated artifact must not decode");
        assert_eq!(err.code, codes::ARTIFACT_INVALID, "at {cut}");
    }
    assert!(artifact::decode(&bytes, &path).is_ok());
}

/// A body naming a hash the artifact does not hold is a closure computed wrong or a file assembled
/// wrong, and either way it is not a program.
#[test]
fn a_body_referring_to_a_hash_the_artifact_lacks_is_e0443() {
    let dir = project(PROGRAM);
    let mut artifact = artifact_of(dir.path());
    // A solo definition something else references.
    let victim = artifact
        .names
        .iter()
        .find(|(name, _)| name == "m.shade")
        .map(|(_, hash)| *hash)
        .expect("`shade` is in the closure");
    artifact.bodies.remove(&victim);
    artifact.names.retain(|(_, h)| *h != victim);

    // Re-encoded, so the digest and every remaining body still verify: the only thing wrong with
    // this file is that its closure is open.
    let bytes = artifact.encode();
    let path = dir.path().join("open.plyx");
    let (decoded, _) = artifact::decode(&bytes, &path).expect("the container still verifies");
    let diags = match artifact::open(&decoded, &path) {
        Ok(_) => panic!("an open closure is not a program"),
        Err(diags) => diags,
    };
    assert_eq!(diags[0].code, codes::ARTIFACT_INVALID);
}

/// The two call for opposite responses — rebuild the artifact, versus transfer it again — so they
/// are two codes.
#[test]
fn a_foreign_encoding_is_e0444_and_not_e0443() {
    let dir = project(PROGRAM);
    let mut artifact = artifact_of(dir.path());
    artifact.body_encoding += 1;
    let path = dir.path().join("old.plyx");
    write_artifact(&path, &artifact);

    let output = ply(dir.path())
        .args(["run", "old.plyx", "--json"])
        .output()
        .unwrap();
    let report = json_of(&output);
    assert_eq!(report["diagnostics"][0]["code"], "E0444");

    let mut stale = artifact_of(dir.path());
    stale.frontend = [7; 32];
    let err = artifact::decode(&stale.encode(), &path).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);

    let mut future = artifact_of(dir.path());
    future.runtime = [7; 32];
    let err = artifact::decode(&future.encode(), &path).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);
}

/// A differing stdlib digest is a fact, not a fault: a shipped definition is content-addressed like
/// any other, and the artifact's own definitions are what it runs.
#[test]
fn a_differing_stdlib_digest_is_w0605_and_the_run_proceeds() {
    let dir = project(PROGRAM);
    let mut artifact = artifact_of(dir.path());
    artifact.std = [11; 32];
    let path = dir.path().join("std.plyx");
    write_artifact(&path, &artifact);

    let (_, warnings) = artifact::decode(&std::fs::read(&path).unwrap(), &path).unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, codes::STDLIB_CHANGED);

    let report = json_of(
        &ply(dir.path())
            .args(["run", "std.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["value"], "42");
}

/// The magic is what stops a `.plyx` argument from being read as anything else.
#[test]
fn a_file_that_is_not_an_artifact_says_so() {
    let dir = project(PROGRAM);
    std::fs::write(dir.path().join("junk.plyx"), b"not a program at all").unwrap();
    let report = json_of(
        &ply(dir.path())
            .args(["run", "junk.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["diagnostics"][0]["code"], "E0443");
}

// --- the digest ---------------------------------------------------------------

/// `--digest` is one line and nothing else, and it is the same digest the build reports — otherwise
/// a deployment pins one number and ships another.
#[test]
fn the_digest_is_one_line_and_agrees_with_the_build() {
    let dir = project(PROGRAM);
    let output = ply(dir.path())
        .args(["build", ".", "--digest"])
        .output()
        .unwrap();
    let printed = String::from_utf8(output.stdout).unwrap();
    assert_eq!(printed.lines().count(), 1, "{printed:?}");
    let digest = printed.trim().to_string();
    assert!(digest.starts_with("b3:") && digest.len() == 15, "{digest}");
    assert!(
        !dir.path().join("m.plyx").exists(),
        "`--digest` writes no file"
    );

    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "-o", "m.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["digest"], digest);
}

/// Embedding sources changes the digest, so "was this built with sources" is answerable from the
/// digest alone.
#[test]
fn sources_change_the_digest() {
    let dir = project(PROGRAM);
    let bare = artifact_of(dir.path());
    let with = built(dir.path(), true).1.artifact;
    assert_ne!(bare.digest(), with.digest());
    assert_eq!(bare.bodies, with.bodies, "only the sources differ");
}

/// A change to any definition in the closure moves the digest; a change to one outside it does not.
#[test]
fn the_digest_moves_with_the_closure_and_with_nothing_else() {
    let dir = project(PROGRAM);
    let before = artifact_of(dir.path()).digest();

    std::fs::write(
        dir.path().join("m.ply"),
        PROGRAM.replace("fn unreached() -> Int = 99", "fn unreached() -> Int = 98"),
    )
    .unwrap();
    assert_eq!(
        artifact_of(dir.path()).digest(),
        before,
        "a definition outside the closure moved the artifact"
    );

    std::fs::write(
        dir.path().join("m.ply"),
        PROGRAM.replace("Blue(21)", "Blue(22)"),
    )
    .unwrap();
    assert_ne!(artifact_of(dir.path()).digest(), before);
}

// --- the difference between two artifacts -------------------------------------

/// The incremental story, delivered as review rather than as transport.
#[test]
fn diff_reports_added_changed_dropped_and_what_a_change_is_reached_by() {
    let dir = project(PROGRAM);
    ply(dir.path())
        .args(["build", ".", "-o", "old.plyx"])
        .assert()
        .success();

    let next = PROGRAM.replace("Red -> 0", "Red -> 1").replace(
        "fn unreached() -> Int = 99",
        "fn restock() -> Int = 1\nfn unreached() -> Int = 99",
    );
    std::fs::write(dir.path().join("m.ply"), &next).unwrap();

    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "--diff", "old.plyx", "--json"])
            .output()
            .unwrap(),
    );
    let changed: Vec<&str> = report["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(changed.contains(&"m.shade"), "{changed:?}");
    assert!(
        changed.contains(&"m.main"),
        "changing a callee changes its caller: {changed:?}"
    );
    assert_eq!(report["added"].as_array().unwrap().len(), 0, "{report}");
    assert_eq!(report["dropped"].as_array().unwrap().len(), 0, "{report}");

    // The reached set is the reverse closure: every definition that reaches a changed one, and a
    // definition reaches itself.
    let reached: Vec<&str> = report["reached"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for name in &changed {
        assert!(reached.contains(name), "{name} does not reach itself");
    }
    assert!(reached.contains(&"m.main"));
    assert!(!reached.contains(&"m.Colour"), "a type reaches no function");

    assert!(report["artifact_bytes"].as_u64().unwrap() > 0);
}

/// A definition that appears and one that goes away, counted apart from the ones that merely moved.
#[test]
fn diff_counts_an_addition_and_a_removal() {
    let dir = project("fn helper() -> Int = 1\nfn main() -> Int = helper()\n");
    ply(dir.path())
        .args(["build", ".", "-o", "old.plyx"])
        .assert()
        .success();
    std::fs::write(
        dir.path().join("m.ply"),
        "fn other() -> Int = 2\nfn main() -> Int = other()\n",
    )
    .unwrap();

    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "--diff", "old.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["added"], serde_json::json!(["m.other"]));
    assert_eq!(report["dropped"], serde_json::json!(["m.helper"]));
    assert_eq!(report["changed"], serde_json::json!(["m.main"]));
}

/// Renaming a definition is a namespace edit, so the hash does not move — and the diff says exactly
/// that: one name added, one dropped, nothing changed.
#[test]
fn a_rename_moves_a_name_and_no_hash() {
    let dir = project("fn helper() -> Int = 1\nfn main() -> Int = helper()\n");
    let before = artifact_of(dir.path());
    std::fs::write(
        dir.path().join("m.ply"),
        "fn assistant() -> Int = 1\nfn main() -> Int = assistant()\n",
    )
    .unwrap();
    let (_, after) = built(dir.path(), false);

    let diff = artifact::diff(&before, &after);
    assert_eq!(diff.added, ["m.assistant"]);
    assert_eq!(diff.dropped, ["m.helper"]);
    assert!(diff.changed.is_empty(), "{:?}", diff.changed);
    assert_eq!(
        before.bodies, after.artifact.bodies,
        "a rename may not move a body"
    );
}

// --- what the command prints ---------------------------------------------------

/// ADR 0015 §5.1 refused incremental transfer because the binary is the part that actually changes.
#[test]
fn the_build_prints_the_artifacts_size_beside_the_binarys() {
    let dir = project(PROGRAM);
    let output = ply(dir.path())
        .args(["build", ".", "-o", "m.plyx"])
        .output()
        .unwrap();
    let printed = String::from_utf8(output.stdout).unwrap();
    assert!(printed.contains("artifact"), "{printed}");
    assert!(printed.contains("binary"), "{printed}");

    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "-o", "m.plyx", "--json"])
            .output()
            .unwrap(),
    );
    let artifact_bytes = report["artifact_bytes"].as_u64().unwrap();
    let binary_bytes = report["binary_bytes"].as_u64().unwrap();
    assert_eq!(
        artifact_bytes,
        std::fs::metadata(dir.path().join("m.plyx")).unwrap().len()
    );
    assert!(
        binary_bytes > artifact_bytes,
        "the ratio §5.1 argued from: artifact {artifact_bytes}, binary {binary_bytes}"
    );
}

/// A directory with two `main`s is refused the same way `ply run` refuses it, and `--entry` is how
/// a program says which closure it meant.
#[test]
fn entry_names_the_closure_and_ambiguity_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), "fn main() -> Int = 1\n").unwrap();
    std::fs::write(dir.path().join("b.ply"), "fn main() -> Int = 2\n").unwrap();

    ply(dir.path()).args(["build", "."]).assert().failure();

    ply(dir.path())
        .args(["build", ".", "--entry", "a.main", "-o", "a.plyx"])
        .assert()
        .success();
    let report = json_of(
        &ply(dir.path())
            .args(["run", "a.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["value"], "1");
}

/// `--entry` also ships a closure that is not `main`'s, which is what a service with several roles
/// needs.
#[test]
fn entry_can_name_something_other_than_main() {
    let dir = project("fn serve() -> Int = 7\nfn main() -> Int = 1\n");
    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "--entry", "serve", "-o", "s.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["entry"], "m.serve");
    let ran = json_of(
        &ply(dir.path())
            .args(["run", "s.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(ran["value"], "7");
}

/// The output path defaults to the entry point's module, so the ordinary case needs no flag.
#[test]
fn the_default_output_is_named_after_the_module() {
    let dir = project(PROGRAM);
    ply(dir.path()).args(["build", "."]).assert().success();
    assert!(dir.path().join("m.plyx").exists());
}

/// A build is a full check of the source tree, so a program that does not check produces no
/// artifact at all rather than a broken one.
#[test]
fn a_program_that_does_not_check_produces_no_artifact() {
    let dir = project("fn main() -> Int = true\n");
    ply(dir.path())
        .args(["build", ".", "-o", "m.plyx"])
        .assert()
        .failure()
        .code(2);
    assert!(!dir.path().join("m.plyx").exists());
}

/// `-o` into a directory that does not exist yet is the normal deploy shape.
#[test]
fn the_output_directory_is_created() {
    let dir = project(PROGRAM);
    ply(dir.path())
        .args(["build", ".", "-o", "dist/nested/m.plyx"])
        .assert()
        .success();
    assert!(dir.path().join("dist/nested/m.plyx").exists());
}

/// A program that reaches the host boundary, so the artifact's namespace has a job to do.
const SERVICE: &str = r#"
import std.net (net)

fn main() -> Int / {net.write[listener]} = net.listen[listener](0)
"#;

#[test]
fn an_artifact_keeps_the_names_a_host_handler_is_registered_against() {
    let dir = project(SERVICE);
    ply(dir.path())
        .args(["build", ".", "-o", "svc.plyx"])
        .assert()
        .success();

    let from_source = json_of(
        &ply(dir.path())
            .args(["run", "m.ply", "--json"])
            .output()
            .unwrap(),
    );
    let from_artifact = json_of(
        &ply(dir.path())
            .args(["run", "svc.plyx", "--json"])
            .output()
            .unwrap(),
    );

    assert_eq!(from_source["diagnostics"][0]["code"], "E0424");
    assert_eq!(
        from_artifact["diagnostics"][0]["code"], from_source["diagnostics"][0]["code"],
        "{from_artifact}"
    );
    let message = from_artifact["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("std.net.net.listen[listener]"),
        "the artifact lost the name a handler is registered against: {message}"
    );
    // The one thing the artifact does lose, stated rather than discovered.
    assert!(from_artifact["diagnostics"][0]["labels"][0]["start"].is_null());
}

/// The entry point answers to the name it was built under, so `--entry`, `--db-schema` and
/// `--config-schema` all name the same thing on both sides of a deploy.
#[test]
fn the_entry_point_keeps_its_program_wide_name() {
    let dir = project(PROGRAM);
    let artifact = artifact_of(dir.path());
    assert_eq!(artifact.entry_name(), Some("m.main"));
    let opened = artifact::open(&artifact, Path::new("t.plyx")).unwrap();
    assert_eq!(opened.entry.as_str(), "m.main");
    assert!(opened.check.defs.contains_key(&opened.entry));
}

/// A namespace that cannot be applied consistently falls back to the synthesized naming that always
/// works, rather than producing a program named half one way and half the other.
#[test]
fn an_inconsistent_namespace_falls_back_rather_than_failing() {
    let dir = project(PROGRAM);
    let mut artifact = artifact_of(dir.path());
    for (name, _) in artifact.names.iter_mut() {
        *name = "collides".to_string();
    }
    // Every name is now the same, so no namespace can be applied; the entry point is still found by
    // hash and the program still runs.
    let opened = artifact::open(&artifact, Path::new("t.plyx"))
        .expect("a useless namespace is not a broken artifact");
    assert!(
        opened.entry.as_str().starts_with('m'),
        "{}",
        opened.entry.as_str()
    );
}

/// An artifact is started by calling its entry point with nothing, so an entry point that takes an
/// argument is one nothing could ever run.
#[test]
fn an_entry_point_that_takes_an_argument_is_refused() {
    let dir = project("fn serve(port: Int) -> Int = port\nfn main() -> Int = serve(1)\n");
    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "--entry", "serve", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["ok"], false);
    assert_eq!(report["diagnostics"][0]["code"], "E0201");
    assert!(!dir.path().join("m.plyx").exists());
}

// --- a deployed artifact is a service, not only a program --------------------

/// The gap `ply build` and the shutdown sequence each left on the other's side: a run from a
/// `.plyx` bound no signal handler, so a readiness route that consults `signal.stopping()` — which
/// ADR 0015 §6.1 says is *the* thing a readiness route checks — answered `E0424` in the deployed
/// form and `false` in the source form.
#[test]
fn an_artifact_run_binds_the_signal_handler_a_source_run_binds() {
    const READY: &str = r#"
import std.signal (signal)

pub fn ready() -> Int / {signal.read} = if signal.stopping() { 503 } else { 200 }

fn main() -> Int = ready()
"#;
    let dir = project(READY);
    write_artifact(&dir.path().join("m.plyx"), &artifact_of(dir.path()));

    let source = ply(dir.path())
        .args(["run", "m.ply", "--host", "--trace", "off", "--json"])
        .output()
        .unwrap();
    let built = ply(dir.path())
        .args(["run", "m.plyx", "--host", "--trace", "off", "--json"])
        .output()
        .unwrap();
    let source = json_of(&source);
    let built = json_of(&built);
    assert_eq!(source["ok"], true, "{source}");
    assert_eq!(built["ok"], true, "{built}");
    assert_eq!(built["value"], source["value"]);
    assert_eq!(built["value"], "200");

    // And the drain is reported the same way, so a deployment reads one shape of `shutdown` object
    // whichever form it shipped.
    assert_eq!(built["shutdown"]["requested"], false);
    assert_eq!(
        built["shutdown"]["drain_ms"],
        source["shutdown"]["drain_ms"]
    );
    assert_eq!(built["shutdown"]["transactions_rolled_back"], 0);

    // Without `--host` it is still `E0424`, naming the twin: an artifact is configured exactly as a
    // source tree is, and the flag is the only way out.
    let hermetic = ply(dir.path())
        .args(["run", "m.plyx", "--json"])
        .output()
        .unwrap();
    let hermetic = json_of(&hermetic);
    assert_eq!(hermetic["ok"], false, "{hermetic}");
    assert_eq!(hermetic["diagnostics"][0]["code"], codes::HERMETIC_BOUNDARY);
}

/// A configuration schema, the entry point, and a definition only the schema reaches.
const WITH_SCHEMA: &str = r#"
import std.config

pub fn required_keys() -> List<config::Key> = [
  {name: "API_KEY", shape: config::SSecret, required: true, default: None},
]

pub fn spec() -> config::ConfigSpec = {keys: required_keys()}

fn main() -> Int = 1
"#;

/// **A `.plyx` has to be able to carry its own `--config-schema`.**
#[test]
fn a_config_schema_named_at_build_time_is_in_the_artifact_and_still_refuses() {
    let dir = project(WITH_SCHEMA);

    // Without the flag, the schema is outside the closure and naming it is a refusal — the
    // behaviour that made the deployed form lose its guarantee.
    ply(dir.path())
        .args(["build", "-o", "bare.plyx"])
        .assert()
        .success();
    let bare_run = ply(dir.path())
        .args([
            "run",
            "bare.plyx",
            "--host",
            "--config-schema",
            "m.spec",
            "--json",
        ])
        .output()
        .unwrap();
    let bare_run = json_of(&bare_run);
    assert_eq!(bare_run["ok"], false, "{bare_run}");
    assert_eq!(
        bare_run["diagnostics"][0]["code"],
        codes::CONFIG_UNAVAILABLE,
        "{bare_run}"
    );

    // Named at build time, the schema ships, and the deployed artifact refuses to start on the
    // missing key exactly as the source tree does.
    let build = String::from_utf8(
        ply(dir.path())
            .args(["build", "--config-schema", "m.spec", "-o", "with.plyx"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(build.contains("startup m.spec"), "{build}");
    let missing = json_of(
        &ply(dir.path())
            .args([
                "run",
                "with.plyx",
                "--host",
                "--config-schema",
                "m.spec",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(missing["ok"], false, "{missing}");
    assert_eq!(
        missing["diagnostics"][0]["code"],
        codes::CONFIG_MISSING,
        "{missing}"
    );

    // And it starts once the key is supplied, which is the other half of the claim: the schema is
    // applied rather than merely present.
    let served = json_of(
        &ply(dir.path())
            .args([
                "run",
                "with.plyx",
                "--host",
                "--config-schema",
                "m.spec",
                "--set",
                "API_KEY=hunter2",
                "--trace",
                "off",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(served["ok"], true, "{served}");

    // The extra definitions are the schema's closure and nothing else, and the build says so in
    // `--json` so a deploy pipeline can assert on it.
    let bare = json_of(
        &ply(dir.path())
            .args(["build", "--json", "-o", "b2.plyx"])
            .output()
            .unwrap(),
    );
    let with = json_of(
        &ply(dir.path())
            .args([
                "build",
                "--json",
                "--config-schema",
                "m.spec",
                "-o",
                "w2.plyx",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(bare["startup"].as_array().unwrap().len(), 0);
    assert_eq!(with["startup"][0], "m.spec");
    assert!(
        with["definitions"].as_u64().unwrap() > bare["definitions"].as_u64().unwrap(),
        "the schema's closure is in the artifact: {bare} vs {with}"
    );
    assert_ne!(
        bare["digest"], with["digest"],
        "an artifact that carries a schema is a different artifact"
    );

    // And it is still the closure of its roots: no test, no law, no fixture.
    let (_, built) = built(dir.path(), false);
    assert!(
        !built.artifact.names.iter().any(|(n, _)| n.contains("test")),
        "{:?}",
        built.artifact.names
    );
}

/// A `--config-schema` at build time that names nothing is refused where it can be fixed, rather
/// than shipping an artifact whose flag will fail on a machine nobody is holding the source on.
#[test]
fn a_build_schema_that_names_nothing_is_refused_at_build_time() {
    let dir = project(WITH_SCHEMA);
    let out = ply(dir.path())
        .args(["build", "--config-schema", "m.absent", "-o", "x.plyx"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(codes::UNKNOWN_NAME), "{text}");
    assert!(!dir.path().join("x.plyx").exists());
}
