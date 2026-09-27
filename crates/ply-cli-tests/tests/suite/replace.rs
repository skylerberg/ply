use crate::harness::{json_of, ply, project};
use std::path::Path;

const SOURCE: &str = "\
// The base.
fn one() -> Int = 1

// Doubles the base.
pub fn two() -> Int = one() + one()  // twice

fn three() -> Int = two() + 1
";

const TWO: &str = "// Doubles the base.\npub fn two() -> Int = one() + one()  // twice\n";

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
    let dir = project(SOURCE);
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
    let dir = project(SOURCE);
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
    let dir = project(SOURCE);
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
    let dir = project(SOURCE);
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

/// A root package over a dependency whose own module imports a sibling: re-checking a spliced
/// program has to resolve both the root's `lib.answer` and that module's bare `extra`.
fn packaged() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    for pkg in ["app", "lib"] {
        std::fs::create_dir(root.join(pkg)).unwrap();
    }
    let manifest = |name: &str, deps: &str| {
        format!(
            "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n"
        )
    };
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest(
            "app",
            "{name: \"lib\", prefix: None, min: {major: 0, minor: 0, patch: 1}, source: Path(\"../lib\")}",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("app/main.ply"),
        "import lib.answer\n\nfn base() -> Int = 1\n\npub fn twice() -> Int = base() + base()\n\nfn main() -> Int = twice() + answer::answer()\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/ply.pkg"), manifest("lib", "")).unwrap();
    std::fs::write(
        root.join("lib/answer.ply"),
        "import extra\npub fn answer() -> Int = extra::hidden()\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/extra.ply"), "pub fn hidden() -> Int = 7\n").unwrap();
    dir
}

#[test]
fn replace_re_checks_a_packaged_project_with_its_own_closure() {
    let dir = packaged();
    let out = ply(dir.path())
        .args(["replace", "twice", "app", "--with", "new.txt"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "a replacement nothing read is refused: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("new.txt"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = ply(dir.path())
        .args(["replace", "twice", "app"])
        .write_stdin("pub fn twice() -> Int = base() * 2\n")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "replaced main.twice in app/main.ply\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("app/main.ply")).unwrap(),
        "import lib.answer\n\nfn base() -> Int = 1\n\npub fn twice() -> Int = base() * 2\n\nfn main() -> Int = twice() + answer::answer()\n"
    );
    assert!(
        !dir.path().join("main.ply").exists(),
        "the write went beside the root, not into it"
    );

    // The program still checks and still runs, so the splice was the whole program's: the root's
    // own definition and the dependency's both answer.
    let out = ply(dir.path()).args(["run", "app"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("9"));
}
