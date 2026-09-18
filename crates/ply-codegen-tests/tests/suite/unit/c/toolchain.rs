use ply_codegen::c::toolchain::support_flags;

/// tcc finds `libtcc1.a` relative to `-B`; without it the compile succeeds and the object fails to load with an empty `dlerror`.
#[test]
fn a_tcc_that_is_not_installed_is_told_where_its_support_library_is() {
    let dir = std::env::temp_dir().join(format!("ply-tcc-probe-{}", std::process::id()));
    let installed = dir.join("bin");
    let lib = dir.join("lib/tcc");
    std::fs::create_dir_all(&installed).expect("a scratch directory");
    std::fs::create_dir_all(&lib).expect("a scratch directory");

    assert!(
        support_flags("cc", Some(&installed.join("cc"))).is_empty(),
        "a compiler that is not tcc was handed a tcc flag"
    );
    assert!(
        support_flags("tcc", Some(&installed.join("tcc"))).is_empty(),
        "a tcc with no support library in reach should be left to its own defaults"
    );

    std::fs::write(lib.join("libtcc1.a"), b"").expect("a scratch file");
    assert_eq!(
        support_flags("tcc", Some(&installed.join("tcc"))),
        vec![format!("-B{}", installed.join("../lib/tcc").display())],
        "an installed layout was not found"
    );

    let beside = dir.join("src");
    std::fs::create_dir_all(&beside).expect("a scratch directory");
    std::fs::write(beside.join("libtcc1.a"), b"").expect("a scratch file");
    assert_eq!(
        support_flags("tcc", Some(&beside.join("tcc"))),
        vec![format!("-B{}", beside.display())],
        "a source build's own directory was not found"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
