//! The agreement corpus, through the command a user runs — the one `CONTRIBUTING.md` item 18
//! recorded as red while this suite stayed green, because nothing here ran it.

use std::path::PathBuf;
use std::process::Command;

fn kernel_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("benches")
        .join("kernel")
}

/// `mcts --only agreement` exits non-zero on a disagreement and prints each one; a green run here
/// is the command itself passing, not a re-implementation of what it checks.
#[test]
fn the_agreement_corpus_is_green_through_the_command_a_user_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_mcts"))
        .arg("--dir")
        .arg(kernel_dir())
        .args(["--only", "agreement"])
        .output()
        .expect("the `mcts` binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`mcts --only agreement` failed ({}):\n{stdout}\n{stderr}",
        out.status
    );
    assert!(
        stdout.contains("== agreement"),
        "the run never reached the agreement corpus:\n{stdout}"
    );
    assert!(
        !stdout.contains("DISAGREEMENT"),
        "the command exited 0 with a disagreement printed:\n{stdout}"
    );
}
