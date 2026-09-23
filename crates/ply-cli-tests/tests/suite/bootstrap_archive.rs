use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const PROGRAM: &str = r#"
type Shape = { wide: Int, tall: Int }
pub fn area(s: Shape) -> Int = s.wide * s.tall
pub fn twice(n: Int) -> Int = area({ wide: n, tall: 2 })
"#;

fn project(dir: &TempDir, source: &str) -> std::path::PathBuf {
    let path = dir.path().join("m.ply");
    std::fs::write(&path, source).expect("write the project");
    dir.path().to_path_buf()
}

#[test]
fn an_archive_is_written_and_verifies_against_the_tree_it_came_from() {
    let dir = TempDir::new().expect("a scratch directory");
    let root = project(&dir, PROGRAM);
    let out = dir.path().join("archive");

    let first = Command::cargo_bin("ply")
        .expect("the binary")
        .args(["bootstrap"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .arg("--json")
        .output()
        .expect("run ply");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let wrote: Value = serde_json::from_slice(&first.stdout).expect("json");
    assert_eq!(wrote["ok"], Value::Bool(true));

    let unit = out.join("unit.c.gz");
    let sources = out.join("SOURCES.digest");
    assert!(unit.is_file(), "no bundle was written");
    assert!(sources.is_file(), "no sources digest was written");
    // The C is the program, so it has to contain the definitions rather than merely exist.
    let bundle = ply_codegen::c::bundle::from_dir(&out).expect("the directory holds a bundle");
    let text = ply_codegen::c::bundle::text_of(&bundle).expect("the unit unpacks");
    // The symbol as the unit's own table publishes it: `<name> <arity> <symbol> <entry>`.
    let symbol = text
        .lines()
        .find_map(|l| l.strip_prefix("\"m.area ")?.split(' ').nth(1))
        .expect("the artifact says what it calls `m.area`")
        .to_string();
    assert!(
        text.contains(&format!("Word {symbol}(PlyCtx *ctx")) && text.contains("ply_bind"),
        "the artifact does not hold the program it was made from"
    );

    let recorded: Value = serde_json::from_str(&wrote["archive"].to_string()).expect("json");
    assert_eq!(
        recorded["artifact"],
        Value::String(blake3::hash(bundle.unit_bytes()).to_hex().to_string()),
        "the report does not describe the bundle beside it"
    );

    let verify = Command::cargo_bin("ply")
        .expect("the binary")
        .args(["bootstrap"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .arg("--verify")
        .output()
        .expect("run ply");
    assert!(
        verify.status.success(),
        "the archive did not verify against the tree that wrote it:\n{}",
        String::from_utf8_lossy(&verify.stderr)
    );
}

#[test]
fn an_archive_stops_describing_a_tree_that_moved() {
    let dir = TempDir::new().expect("a scratch directory");
    let root = project(&dir, PROGRAM);
    let out = dir.path().join("archive");

    let write = Command::cargo_bin("ply")
        .expect("the binary")
        .args(["bootstrap"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("run ply");
    assert!(write.status.success());

    std::fs::write(
        root.join("m.ply"),
        format!("{PROGRAM}\npub fn added(n: Int) -> Int = n + 1\n"),
    )
    .expect("edit the project");

    let verify = Command::cargo_bin("ply")
        .expect("the binary")
        .args(["bootstrap"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .arg("--verify")
        .output()
        .expect("run ply");
    assert!(
        !verify.status.success(),
        "a definition was added and the archive still claimed to describe the tree"
    );
}
