use ply_privileged::toolchain::support_flags;

/// tcc finds `libtcc1.a` relative to `-B`; without it the compile succeeds and the object fails to
/// load with an empty `dlerror`.
#[test]
fn a_tcc_that_is_not_installed_is_told_where_its_support_library_is() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let installed = dir.path().join("bin");
    let lib = dir.path().join("lib/tcc");
    std::fs::create_dir_all(&installed).expect("a scratch directory");
    std::fs::create_dir_all(&lib).expect("a scratch directory");

    assert!(
        support_flags(&installed.join("cc")).is_empty(),
        "a compiler that is not tcc was handed a tcc flag"
    );
    assert!(
        support_flags(&installed.join("tcc")).is_empty(),
        "a tcc with no support library in reach should be left to its own defaults"
    );

    std::fs::write(lib.join("libtcc1.a"), b"").expect("a scratch file");
    assert_eq!(
        support_flags(&installed.join("tcc")),
        vec![format!("-B{}", installed.join("../lib/tcc").display())],
        "an installed layout was not found"
    );

    let beside = dir.path().join("src");
    std::fs::create_dir_all(&beside).expect("a scratch directory");
    std::fs::write(beside.join("libtcc1.a"), b"").expect("a scratch file");
    assert_eq!(
        support_flags(&beside.join("tcc")),
        vec![format!("-B{}", beside.display())],
        "a source build's own directory was not found"
    );
}
