use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

const SOURCE: &str = "\
// The base.
fn one() -> Int = 1

// Doubles the base.
pub fn two() -> Int = one() + one()  // twice

fn three() -> Int = two() + 1
";

const TWO: &str = "// Doubles the base.\npub fn two() -> Int = one() + one()  // twice\n";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), SOURCE).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn json_of(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

/// Every definition's `own` hash, by name.
fn own_hashes(dir: &Path) -> Vec<(String, String)> {
    let out = ply(dir).args(["defs", "--json"]).output().unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 0, "{v}");
    v["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap().to_string(),
                d["own"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn show_prints_the_definition_with_the_comment_above_it_and_its_place() {
    let dir = project();
    let out = ply(dir.path()).args(["show", "two"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), TWO);

    let out = ply(dir.path())
        .args(["show", "m.two", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["command"], "show");
    assert_eq!(v["ok"], true);
    assert_eq!(v["name"], "m.two");
    assert_eq!(v["file"], "m.ply");
    assert_eq!(v["source"], TWO);
    let (start, end) = (
        v["start"].as_u64().unwrap() as usize,
        v["end"].as_u64().unwrap() as usize,
    );
    assert_eq!(&SOURCE[start..end], TWO);

    let out = ply(dir.path())
        .args(["show", "four", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 2);
    assert_eq!(v["diagnostics"][0]["code"], "E0101");
}

#[test]
fn replace_rewrites_only_the_named_definition_and_moves_no_other_hash() {
    let dir = project();
    let before = own_hashes(dir.path());
    std::fs::write(
        dir.path().join("two.txt"),
        "// Doubles the base, once.\npub fn   two() -> Int = one()*2\n",
    )
    .unwrap();
    let out = ply(dir.path())
        .args(["replace", "two", "--with", "two.txt"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "replaced m.two in m.ply\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.ply")).unwrap(),
        "\
// The base.
fn one() -> Int = 1

// Doubles the base, once.
pub fn two() -> Int = one() * 2

fn three() -> Int = two() + 1
"
    );
    let after = own_hashes(dir.path());
    assert_eq!(before.len(), after.len());
    for ((name, was), (_, is)) in before.iter().zip(&after) {
        if name == "m.two" {
            assert_ne!(was, is, "{name}");
        } else {
            assert_eq!(was, is, "{name}");
        }
    }

    let out = ply(dir.path())
        .args(["replace", "two", "--with", "two.txt", "--json"])
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["command"], "replace");
    assert_eq!(v["ok"], true);
    assert_eq!(v["name"], "m.two");
    assert_eq!(v["file"], "m.ply");
    assert_eq!(v["changed"], false);
}

#[test]
fn a_replacement_that_renames_or_breaks_the_program_is_refused_and_writes_nothing() {
    let dir = project();
    let out = ply(dir.path())
        .args(["replace", "two"])
        .write_stdin("pub fn twice() -> Int = one() * 2\n")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0128") && stderr.contains("`fn` `twice`"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.ply")).unwrap(),
        SOURCE
    );

    let out = ply(dir.path())
        .args(["replace", "two", "--json"])
        .write_stdin("pub fn two() -> Int = one() + \"1\"\n")
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["exit_code"], 2);
    assert_eq!(v["changed"], false);
    assert_eq!(v["diagnostics"][0]["code"], "E0128");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.ply")).unwrap(),
        SOURCE
    );
}

#[test]
fn check_reports_the_file_that_would_change_and_writes_nothing() {
    let dir = project();
    let out = ply(dir.path())
        .args(["replace", "two", "--check"])
        .write_stdin("pub fn two() -> Int = one() * 2\n")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "would replace m.ply\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.ply")).unwrap(),
        SOURCE
    );
}
