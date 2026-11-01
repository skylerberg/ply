use ply_compiled::hosted::toolchain::{ended, tinycc_library_flags};
use std::path::Path;
use std::process::Command;

#[test]
fn a_program_killed_by_a_signal_returns_a_message_naming_the_signal_and_an_exited_program_returns_no_message()
 {
    let shell = Path::new("/bin/sh");
    let stopped = Command::new(shell)
        .args(["-c", "kill -SEGV $$"])
        .status()
        .expect("a shell");
    let (code, said) = ended(shell, stopped);
    assert_eq!(code, -1);
    assert_eq!(
        said,
        Some("/bin/sh was stopped by signal 11 (SIGSEGV)".to_string())
    );
    let exited = Command::new(shell)
        .args(["-c", "exit 3"])
        .status()
        .expect("a shell");
    assert_eq!(ended(shell, exited), (3, None));
}

#[test]
fn a_tcc_that_is_not_installed_is_told_where_its_support_library_is() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let installed = dir.path().join("bin");
    let lib = dir.path().join("lib/tcc");
    std::fs::create_dir_all(&installed).expect("a scratch directory");
    std::fs::create_dir_all(&lib).expect("a scratch directory");

    assert!(
        tinycc_library_flags(&installed.join("cc")).is_empty(),
        "a compiler that is not tcc was handed a tcc flag"
    );
    assert!(
        tinycc_library_flags(&installed.join("tcc")).is_empty(),
        "a tcc with no support library in reach should be left to its own defaults"
    );

    std::fs::write(lib.join("libtcc1.a"), b"").expect("a scratch file");
    assert_eq!(
        tinycc_library_flags(&installed.join("tcc")),
        vec![format!("-B{}", installed.join("../lib/tcc").display())],
        "an installed layout was not found"
    );

    let beside = dir.path().join("src");
    std::fs::create_dir_all(&beside).expect("a scratch directory");
    std::fs::write(beside.join("libtcc1.a"), b"").expect("a scratch file");
    assert_eq!(
        tinycc_library_flags(&beside.join("tcc")),
        vec![format!("-B{}", beside.display())],
        "a source build's own directory was not found"
    );
}
