use ply_machine::migrate::*;
use ply_span::codes;
use ply_store::Store;

fn store(dir: &std::path::Path) -> Store {
    Store::open(dir).unwrap()
}

#[test]
fn a_healthy_cache_says_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    assert!(notice(&store, &[]).is_none());
}

/// A degraded result cache loses its contents, but no type or hash is recomputed.
#[test]
fn a_result_cache_warning_is_not_a_front_end_migration() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = store(dir.path());
    std::fs::write(store.path(), "{ not json").unwrap();
    store = Store::open(dir.path()).unwrap();

    let warnings = store.warnings().to_vec();
    assert!(!warnings.is_empty(), "a corrupt result cache must warn");
    assert!(notice(&store, &warnings).is_none());
}

#[test]
fn an_unreadable_front_end_cache_is_reported_with_what_survived_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let frontend = store.frontend_path().to_path_buf();
    std::fs::write(&frontend, "{ not json").unwrap();

    let store = Store::open(dir.path()).unwrap();
    let warnings = store.warnings().to_vec();
    let notice = notice(&store, &warnings).expect("a discarded front end must be reported");
    assert_eq!(notice.code, codes::CACHE_VERSION_CHANGED);
    assert!(
        notice.notes.iter().any(|n| n.contains("no test re-runs")),
        "the user has to be told their results survived: {:?}",
        notice.notes
    );
}

/// Inert while the store's own front-end file is the legacy one.
#[test]
fn a_superseded_json_cache_is_named_even_without_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let legacy = store.dir().join(LEGACY_FRONTEND_FILE);
    std::fs::write(&legacy, "{}").unwrap();

    let notice = notice(&store, &[]);
    if store.frontend_path() == legacy {
        assert!(notice.is_none(), "the live cache is not a leftover");
    } else {
        let notice = notice.expect("a leftover JSON cache must be explained");
        assert!(notice.message.contains(LEGACY_FRONTEND_FILE));
    }
}
