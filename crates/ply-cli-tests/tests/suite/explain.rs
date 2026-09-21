use assert_cmd::Command;
use serde_json::Value;

fn ply() -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never");
    cmd
}

#[test]
fn a_code_is_explained_in_one_line_and_as_json() {
    let out = ply().args(["explain", "e0302"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text, "E0302 effect not permitted by the written row\n");

    let out = ply().args(["explain", "W0611", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["code"], "W0611");
    assert_eq!(v["severity"], "warning");
    assert_eq!(v["meaning"], ply_span::meaning("W0611").unwrap());
    assert_eq!(v["exit_code"], 0);
}

#[test]
fn the_label_instantiation_code_is_explained_like_any_other() {
    let out = ply().args(["explain", "E0306"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "E0306 label instantiation: a call leaves a label unfilled or writes the wrong number \
         of them, or a label-generic definition is used as a value\n"
    );

    let out = ply().args(["explain", "e0306", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["code"], "E0306");
    assert_eq!(v["severity"], "error");
    assert_eq!(v["meaning"], ply_span::meaning("E0306").unwrap());
}

#[test]
fn a_code_nothing_raises_exits_two_and_points_at_the_list() {
    let out = ply().args(["explain", "E9999"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("explain --all"));

    let out = ply().args(["explain", "--all", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let codes = v["codes"].as_array().unwrap();
    assert_eq!(codes.len(), ply_span::MEANINGS.len());
    assert_eq!(codes[0]["code"], "E0001");
}
