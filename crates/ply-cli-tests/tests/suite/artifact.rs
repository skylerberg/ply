use assert_cmd::Command;
use ply_cli::artifact::{self, Artifact, Binds};
use ply_cli::load::{Loaded, load};
use ply_host::process::Executables;
use ply_span::{Span, codes};
use ply_ty::DefHash;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

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

fn built(dir: &Path) -> (Loaded, artifact::Built) {
    let loaded = load(dir).expect("the corpus should load");
    let entry = loaded
        .sole_entry_point()
        .expect("`main` is the entry point");
    let built = artifact::build(&loaded, entry, &[]).expect("the closure should build");
    (loaded, built)
}

fn artifact_of(dir: &Path) -> Artifact {
    built(dir).1.artifact
}

fn written(artifact: &Artifact) -> Vec<u8> {
    artifact.encode().expect("the container should be written")
}

fn write_artifact(at: &Path, artifact: &Artifact) {
    std::fs::write(at, written(artifact)).unwrap();
}

fn json_of(output: &std::process::Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("stdout was not one object: {e}\n{text}"))
}

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
    assert_eq!(written(&first), written(&second));
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.digest_short(), second.digest_short());
    assert!(first.digest_short().starts_with("b3:"));
    assert_eq!(first.digest_short().len(), 15);
}

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

#[test]
fn an_artifact_carries_no_test_no_law_and_nothing_unreached() {
    let dir = project(PROGRAM);
    let (loaded, built) = built(dir.path());
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
    assert!(opened.front.check.tests.is_empty(), "a test was deployed");
    assert!(opened.front.check.laws.is_empty(), "a law was deployed");

    let mut shipped = vec![String::from_utf8_lossy(&written(&built.artifact)).into_owned()];
    if let Some(unit) = &built.artifact.unit {
        shipped.push(ply_codegen::c::bundle::unpack(&unit.text).unwrap());
    }
    for text in &shipped {
        for word in [
            "unreached",
            "announce",
            "shade reads a payload",
            "even and odd disagree",
        ] {
            assert!(!text.contains(word), "`{word}` shipped");
        }
    }
}

#[test]
fn every_body_is_filed_under_the_hash_the_hash_command_prints() {
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

#[test]
fn a_failure_in_an_artifact_carries_no_line_number() {
    let dir = project("fn main() -> Int = 1 / 0\n");
    ply(dir.path())
        .args(["build", ".", "-o", "m.plyx"])
        .assert()
        .success();

    let from_source = json_of(
        &ply(dir.path())
            .args(["run", "m.ply", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        from_source["diagnostics"][0]["labels"][0]["start"]["line"],
        1
    );
    let failed = json_of(
        &ply(dir.path())
            .args(["run", "m.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(failed["ok"], false);
    assert_eq!(
        failed["diagnostics"][0]["code"],
        from_source["diagnostics"][0]["code"]
    );
    assert!(
        failed["diagnostics"][0]["labels"][0]["start"].is_null(),
        "{failed}"
    );
}

#[test]
fn a_closure_that_is_not_the_closure_is_refused() {
    let dir = project(PROGRAM);
    let artifact = artifact_of(dir.path());
    let mut altered = artifact.clone();
    for (_, text) in altered.closure.iter_mut() {
        *text = text.replace("Blue(20)", "Blue(19)");
    }
    assert_ne!(
        altered.closure, artifact.closure,
        "the closure spells the literal"
    );
    let mut garbled = artifact.clone();
    garbled.closure[0].1 = "fn (".to_string();

    for broken in [altered, garbled] {
        let diags = match artifact::open(&broken, Path::new("t.plyx")) {
            Ok(_) => panic!("a closure that is not the program must not be believed"),
            Err(diags) => diags,
        };
        assert_eq!(diags[0].code, codes::ARTIFACT_INVALID);
    }
}

#[test]
fn a_flipped_bit_in_a_body_is_e0443_naming_the_definition() {
    let dir = project(PROGRAM);
    let artifact = artifact_of(dir.path());
    let mut bytes = written(&artifact);

    // The first record starts past the header and section descriptors; its payload, past the 32-byte key and length prefix.
    let sections = u32::from_le_bytes(bytes[180..184].try_into().unwrap()) as usize;
    let record = 188 + 24 * sections;
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

#[test]
fn no_prefix_of_an_artifact_is_believed() {
    let dir = project(PROGRAM);
    let bytes = written(&artifact_of(dir.path()));
    let path = dir.path().join("t.plyx");
    for cut in [0, 1, 100, 187, 188, 200, bytes.len() / 2, bytes.len() - 1] {
        let err = artifact::decode(&bytes[..cut], &path)
            .expect_err("a truncated artifact must not decode");
        assert_eq!(err.code, codes::ARTIFACT_INVALID, "at {cut}");
    }
    assert!(artifact::decode(&bytes, &path).is_ok());
}

#[test]
fn removing_a_body_is_e0443() {
    let dir = project(PROGRAM);
    let mut artifact = artifact_of(dir.path());
    let victim = artifact
        .names
        .iter()
        .find(|(name, _)| name == "m.shade")
        .map(|(_, hash)| *hash)
        .expect("`shade` is in the closure");
    artifact.bodies.remove(&victim);
    artifact.names.retain(|(_, h)| *h != victim);

    let path = dir.path().join("open.plyx");
    write_artifact(&path, &artifact);
    let (decoded, _) = artifact::read(&path).expect("the container still verifies");
    let diags = match artifact::open(&decoded, &path) {
        Ok(_) => panic!("a closure missing a body is not a program"),
        Err(diags) => diags,
    };
    assert_eq!(diags[0].code, codes::ARTIFACT_INVALID);

    let report = json_of(
        &ply(dir.path())
            .args(["run", "open.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["diagnostics"][0]["code"], "E0443", "{report}");
}

/// Rebuild the artifact versus transfer it again: opposite responses, so two codes.
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
    let err = artifact::decode(&written(&stale), &path).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);

    let mut future = artifact_of(dir.path());
    future.runtime = [7; 32];
    let err = artifact::decode(&written(&future), &path).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);
}

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

#[test]
fn the_digest_moves_with_the_closure_and_with_nothing_else() {
    let dir = project(PROGRAM);
    let before = written(&artifact_of(dir.path()));

    for (what, edited) in [
        (
            "an unreached definition",
            PROGRAM.replace("fn unreached() -> Int = 99", "fn unreached() -> Int = 98"),
        ),
        (
            "a new unreached type",
            format!("{PROGRAM}\ntype Spare = | Unused | Kept(Int)\n"),
        ),
        (
            "a test",
            PROGRAM.replace(
                "assert_eq(shade(Blue(7)), 7)",
                "assert_eq(shade(Blue(8)), 8)",
            ),
        ),
        (
            "a law",
            PROGRAM.replace("n >= 0 && n < 4", "n >= 0 && n < 5"),
        ),
        (
            "a local's name",
            PROGRAM.replace("Blue(n) -> n", "Blue(k) -> k"),
        ),
    ] {
        assert_ne!(edited, PROGRAM, "{what}: the edit did not apply");
        std::fs::write(dir.path().join("m.ply"), &edited).unwrap();
        assert!(
            written(&artifact_of(dir.path())) == before,
            "{what} moved the artifact"
        );
    }

    std::fs::write(
        dir.path().join("m.ply"),
        PROGRAM.replace("Blue(21)", "Blue(22)"),
    )
    .unwrap();
    let inside = artifact_of(dir.path());
    assert_ne!(artifact::digest_of(&before), Some(inside.digest()));
}

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

    // The reverse closure: every definition that reaches a changed one, and a definition reaches itself.
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

#[test]
fn a_rename_moves_a_name_and_no_hash() {
    let dir = project("fn helper() -> Int = 1\nfn main() -> Int = helper()\n");
    ply(dir.path())
        .args(["build", ".", "-o", "old.plyx"])
        .assert()
        .success();
    let (before, _) = artifact::read(&dir.path().join("old.plyx")).unwrap();
    std::fs::write(
        dir.path().join("m.ply"),
        "fn assistant() -> Int = 1\nfn main() -> Int = assistant()\n",
    )
    .unwrap();

    let report = json_of(
        &ply(dir.path())
            .args(["build", ".", "--diff", "old.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(report["added"], serde_json::json!(["m.assistant"]));
    assert_eq!(report["dropped"], serde_json::json!(["m.helper"]));
    assert_eq!(report["changed"], serde_json::json!([]));

    ply(dir.path())
        .args(["build", ".", "-o", "new.plyx"])
        .assert()
        .success();
    let (after, _) = artifact::read(&dir.path().join("new.plyx")).unwrap();
    assert_eq!(before.bodies, after.bodies, "a rename may not move a body");
}

fn runs_as_its_source(dir: &Path) -> Value {
    ply(dir)
        .args(["build", ".", "-o", "app.plyx"])
        .assert()
        .success();
    let from_source = json_of(&ply(dir).args(["run", ".", "--json"]).output().unwrap());
    let from_artifact = json_of(
        &ply(dir)
            .args(["run", "app.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(from_source["ok"], true, "{from_source}");
    assert_eq!(
        from_artifact["value"], from_source["value"],
        "{from_artifact}"
    );
    from_artifact["value"].clone()
}

#[test]
fn modules_sharing_a_last_segment_ship_and_run() {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("left/util.ply", "pub fn one() -> Int = 1\n"),
        ("right/util.ply", "pub fn two() -> Int = 20\n"),
        (
            "m.ply",
            "import left.util as l\nimport right.util as r\n\nfn main() -> Int = l::one() + r::two()\n",
        ),
    ] {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    assert_eq!(runs_as_its_source(dir.path()), "21");

    let (shipped, _) = artifact::read(&dir.path().join("app.plyx")).unwrap();
    let files: Vec<&str> = shipped.closure.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(files, ["left/util.ply", "m.ply", "right/util.ply"]);
}

#[test]
fn two_names_for_one_body_ship_and_run() {
    let dir = project(
        "fn one() -> Int = 1\nfn uno() -> Int = 1\nfn main() -> Int = one() + uno() * 10\n",
    );
    assert_eq!(runs_as_its_source(dir.path()), "11");

    let (shipped, _) = artifact::read(&dir.path().join("app.plyx")).unwrap();
    let hash = |name: &str| {
        shipped
            .names
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| *h)
    };
    assert!(hash("m.one").is_some(), "{:?}", shipped.names);
    assert_eq!(hash("m.one"), hash("m.uno"), "one body, two names");
}

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
    assert_eq!(
        report["format"],
        artifact::format().expect("the container states its format"),
        "{report}"
    );
    assert!(report.get("sources").is_none(), "{report}");
    let artifact_bytes = report["artifact_bytes"].as_u64().unwrap();
    let binary_bytes = report["binary_bytes"].as_u64().unwrap();
    assert_eq!(
        artifact_bytes,
        std::fs::metadata(dir.path().join("m.plyx")).unwrap().len()
    );
    assert!(
        binary_bytes > artifact_bytes,
        "the ratio the whole-artifact decision argued from: artifact {artifact_bytes}, binary {binary_bytes}"
    );
}

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

#[test]
fn the_default_output_is_named_after_the_module() {
    let dir = project(PROGRAM);
    ply(dir.path()).args(["build", "."]).assert().success();
    assert!(dir.path().join("m.plyx").exists());
}

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

#[test]
fn the_output_directory_is_created() {
    let dir = project(PROGRAM);
    ply(dir.path())
        .args(["build", ".", "-o", "dist/nested/m.plyx"])
        .assert()
        .success();
    assert!(dir.path().join("dist/nested/m.plyx").exists());
}

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
    assert!(from_artifact["diagnostics"][0]["labels"][0]["start"].is_null());
}

#[test]
fn the_entry_point_keeps_its_program_wide_name() {
    let dir = project(PROGRAM);
    let artifact = artifact_of(dir.path());
    assert_eq!(artifact.entry_name(), Some("m.main"));
    let opened = artifact::open(&artifact, Path::new("t.plyx")).unwrap();
    assert_eq!(opened.entry.as_str(), "m.main");
    assert!(opened.front.check.defs.contains_key(&opened.entry));
}

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

    // Reported the same way, so a deployment reads one `shutdown` shape whichever form it shipped.
    assert_eq!(built["shutdown"]["requested"], false);
    assert_eq!(
        built["shutdown"]["drain_ms"],
        source["shutdown"]["drain_ms"]
    );
    assert_eq!(built["shutdown"]["transactions_rolled_back"], 0);

    // Without `--host` it is still `E0424`, naming the twin: the flag is the only way out.
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

#[test]
fn a_config_schema_named_at_build_time_is_in_the_artifact_and_still_refuses() {
    let dir = project(WITH_SCHEMA);

    // Without the flag the schema is outside the closure, so naming it at run is a refusal.
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

    // Named at build time, the schema ships, and the artifact refuses to start on the missing key as the source does.
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

    // And starts once the key is supplied: the schema is applied, not merely present.
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

    // The extra definitions are the schema's closure and nothing else.
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
    let (_, built) = built(dir.path());
    assert!(
        !built.artifact.names.iter().any(|(n, _)| n.contains("test")),
        "{:?}",
        built.artifact.names
    );
}

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

/// "Another runtime": the unit's own helper table is rewritten so its first helper takes one argument more.
#[test]
fn a_unit_built_for_another_runtime_is_left_aside_with_a_warning() {
    let dir = project("fn main() -> Int = 6 * 7\n");
    let mut artifact = artifact_of(dir.path());
    let unit = artifact
        .unit
        .as_mut()
        .expect("an artifact carries its unit");
    let text = ply_codegen::c::bundle::unpack(&unit.text).unwrap();
    let first = &ply_codegen::c::HELPERS[0];
    let line = format!(
        "\"{} {} {}\\n\"\n",
        first.name,
        first.args,
        u8::from(first.answers)
    );
    assert_eq!(
        text.matches(&line).count(),
        1,
        "the unit's table names its first helper once"
    );
    let foreign = line.replacen(
        &format!(" {} ", first.args),
        &format!(" {} ", first.args + 1),
        1,
    );
    unit.text = ply_codegen::c::bundle::pack(&text.replacen(&line, &foreign, 1)).unwrap();
    write_artifact(&dir.path().join("m.plyx"), &artifact);

    let out = ply(dir.path()).args(["run", "m.plyx"]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("built for another runtime"), "{text}");
    let v = json_of(
        &ply(dir.path())
            .args(["run", "m.plyx", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(v["value"], "42", "{v}");
}

/// Exits with the code of whatever `cc` names, so nothing but the caller's binding decides what
/// ran.
const SPAWNS: &str = r#"
import std.process (process, exit_code)

fn main() -> Unit / {process.spawn[cc], process.exit[proc]} = {
  let done = process.spawn[cc](["-c", "exit 7"], "", []);
  match exit_code(done) {
    Some(code) -> process.exit[proc](code),
    None -> process.exit[proc](9),
  }
}
"#;

fn spawning() -> (TempDir, Artifact, artifact::Opened) {
    let dir = project(SPAWNS);
    let artifact = artifact_of(dir.path());
    let opened = artifact::open(&artifact, Path::new("spawns.plyx")).expect("it opens");
    (dir, artifact, opened)
}

/// What the five ported commands are entered with: the label is the capability, and a caller that
/// binds no program hands the program none to start.
#[test]
fn an_entered_program_cannot_spawn_a_label_nothing_bound() {
    let (_dir, artifact, opened) = spawning();
    let entered = artifact::enter(&artifact, &opened, Vec::new(), Binds::default());
    let refused = entered.expect_err("`cc` is bound to nothing");
    assert_eq!(refused.code, codes::PROCESS_EXEC_UNBOUND);
}

#[test]
fn an_entered_program_starts_what_its_caller_bound_to_the_label() {
    let (_dir, artifact, opened) = spawning();
    let mut executables = Executables::new();
    executables
        .bind("cc", Path::new("/bin/sh"), Span::DUMMY)
        .expect("a shell is a program");
    let binds = Binds {
        executables,
        ..Binds::default()
    };
    let entered = artifact::enter(&artifact, &opened, Vec::new(), binds);
    let code = entered.expect("the program runs");
    assert_eq!(code, 7, "the child's own code is what came back");
}
