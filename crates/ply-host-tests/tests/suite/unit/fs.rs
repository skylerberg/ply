use ply_host::fs::*;
use ply_span::{Span, Symbol, codes};
use ply_ty::Resource;

fn root() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

fn span() -> Span {
    Span::DUMMY
}

#[test]
fn a_path_under_the_root_resolves() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    std::fs::write(real.join("a.ply"), b"x").unwrap();
    assert_eq!(confine(&real, "a.ply", span()).unwrap(), real.join("a.ply"));
}

#[test]
fn an_absolute_path_and_a_parent_component_are_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    for path in ["/etc/passwd", "../secrets", "src/../../secrets"] {
        let refusal = confine(&real, path, span()).expect_err("it should be refused");
        assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT, "for `{path}`");
    }
}

/// The half a lexical check cannot do.
#[test]
fn a_symlink_out_of_the_root_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let outside_real = outside.path().canonicalize().unwrap();
    std::fs::write(outside_real.join("secrets"), b"s").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_real, real.join("link")).unwrap();
    #[cfg(not(unix))]
    return;

    let refusal = confine(&real, "link/secrets", span()).expect_err("it should be refused");
    assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
}

/// A write to a path that does not exist yet still traverses the link its parent is, so the
/// check has to look at the nearest existing ancestor rather than give up when the target is
/// absent.
#[test]
fn a_write_through_a_symlinked_directory_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let outside_real = outside.path().canonicalize().unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_real, real.join("out")).unwrap();
    #[cfg(not(unix))]
    return;

    let refusal = confine(&real, "out/artifact.plyx", span()).expect_err("it should be refused");
    assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
}

#[test]
fn a_root_that_is_not_a_directory_does_not_bind() {
    let dir = root();
    let file = dir.path().join("a.ply");
    std::fs::write(&file, b"x").unwrap();
    let mut roots = Roots::new();
    let refused = roots
        .bind("src", &file, span())
        .expect_err("a file is not a root");
    assert_eq!(refused.code, codes::FS_ROOT_INVALID);
    assert!(roots.is_empty());
}

#[test]
fn an_unbound_label_names_the_flag_that_would_bind_it() {
    let d = unbound(Op::ReadFile, &Resource::Named(Symbol::new("src")), span());
    assert_eq!(d.code, codes::FS_ROOT_UNBOUND);
    assert!(
        d.notes.iter().any(|n| n.contains("--fs src=")),
        "the diagnostic should name the flag: {:?}",
        d.notes
    );
}
