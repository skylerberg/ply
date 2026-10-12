use ply_codegen::c::stage::root_of;
use std::path::PathBuf;

fn path(p: &str) -> Option<PathBuf> {
    Some(PathBuf::from(p))
}

#[test]
fn a_named_root_is_the_root() {
    assert_eq!(
        root_of(
            path("/named"),
            path("/home/u"),
            path("/xdg"),
            false,
            "/tmp".into()
        ),
        PathBuf::from("/named")
    );
}

#[test]
fn the_stages_are_kept_under_the_users_caches() {
    assert_eq!(
        root_of(None, path("/Users/u"), path("/xdg"), true, "/tmp".into()),
        PathBuf::from("/Users/u/Library/Caches/ply/stage")
    );
    assert_eq!(
        root_of(None, path("/home/u"), path("/xdg"), false, "/tmp".into()),
        PathBuf::from("/xdg/ply/stage")
    );
    assert_eq!(
        root_of(None, path("/home/u"), None, false, "/tmp".into()),
        PathBuf::from("/home/u/.cache/ply/stage")
    );
}

/// The specification of `XDG_CACHE_HOME` has a relative path ignored.
#[test]
fn a_relative_cache_home_is_not_one() {
    assert_eq!(
        root_of(None, path("/home/u"), path("caches"), false, "/tmp".into()),
        PathBuf::from("/home/u/.cache/ply/stage")
    );
}

#[test]
fn a_user_with_no_home_keeps_them_under_the_temp_directory() {
    assert_eq!(
        root_of(None, None, None, false, "/tmp".into()),
        PathBuf::from("/tmp/ply/stage")
    );
    assert_eq!(
        root_of(None, path(""), None, true, "/tmp".into()),
        PathBuf::from("/tmp/ply/stage")
    );
}
