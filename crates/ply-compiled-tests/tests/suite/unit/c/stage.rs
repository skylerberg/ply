use ply_compiled::c::cache_root_for;
use std::path::PathBuf;

fn path(p: &str) -> Option<PathBuf> {
    Some(PathBuf::from(p))
}

#[test]
fn an_explicitly_named_directory_is_used_as_the_cache_root() {
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
fn the_cache_root_defaults_to_the_platform_cache_directory_of_the_user() {
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

#[test]
fn a_relative_cache_home_directory_is_ignored_and_the_default_cache_root_is_used() {
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
