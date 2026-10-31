use ply_compiled::c::cache_root_for;
use std::path::PathBuf;

fn path(p: &str) -> Option<PathBuf> {
    Some(PathBuf::from(p))
}

#[test]
fn a_named_directory_is_the_one_cached_in() {
    assert_eq!(
        cache_root_for(
            "stage",
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
fn what_a_run_caches_is_stored_under_the_users_caches() {
    assert_eq!(
        cache_root_for(
            "stage",
            None,
            path("/Users/u"),
            path("/xdg"),
            true,
            "/tmp".into()
        ),
        PathBuf::from("/Users/u/Library/Caches/ply/stage")
    );
    assert_eq!(
        cache_root_for(
            "cache",
            None,
            path("/home/u"),
            path("/xdg"),
            false,
            "/tmp".into()
        ),
        PathBuf::from("/xdg/ply/cache")
    );
    assert_eq!(
        cache_root_for("cache", None, path("/home/u"), None, false, "/tmp".into()),
        PathBuf::from("/home/u/.cache/ply/cache")
    );
}

/// The specification of `XDG_CACHE_HOME` has a relative path ignored.
#[test]
fn a_relative_cache_home_is_not_one() {
    assert_eq!(
        cache_root_for(
            "stage",
            None,
            path("/home/u"),
            path("caches"),
            false,
            "/tmp".into()
        ),
        PathBuf::from("/home/u/.cache/ply/stage")
    );
}

#[test]
fn a_user_with_no_home_caches_it_under_the_temp_directory() {
    assert_eq!(
        cache_root_for("stage", None, None, None, false, "/tmp".into()),
        PathBuf::from("/tmp/ply/stage")
    );
    assert_eq!(
        cache_root_for("cache", None, path(""), None, true, "/tmp".into()),
        PathBuf::from("/tmp/ply/cache")
    );
}
