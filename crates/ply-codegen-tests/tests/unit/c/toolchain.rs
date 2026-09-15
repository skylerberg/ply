use ply_codegen::c::toolchain::{Profile, support_flags};

/// The compiler and the inlining are one choice.
///
/// A non-optimising compiler gives every temporary its own stack slot, so this tier's
/// depth-3 bodies compile to 128KB frames against `-O2`'s 8, and the self-hosted front end
/// recurses far enough to overflow the stack -- an `abort` with no diagnostic, under `tcc`,
/// `cc -O0` and `cc -O1` alike. Every one of them runs the whole corpus at depth 0. If this
/// assertion is in your way, that is what it is in the way of.
#[test]
fn the_fast_toolchain_does_not_inline() {
    assert_eq!(Profile::Development.inlining().depth, 0);
    assert_eq!(
        Profile::Release.inlining().depth,
        ply_codegen::opt::Inlining::EMITTED.depth
    );
}

/// tcc finds `libtcc1.a` relative to `-B`, and a build that was never installed has no default
/// that finds it. Without the flag the compile *succeeds* and the object will not load, with
/// an empty reason from `dlerror`.
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
