//! What a run says when it could not use the front-end cache it found.

use ply_span::{Diagnostic, codes};
use ply_store::Store;

/// The name the front-end cache had while it was a single JSON document.
const LEGACY_FRONTEND_FILE: &str = "frontend.json";

/// `warnings` is what the store collected while opening; a caller that has already drained them
/// must pass what it drained, because a store reports each degradation once.
pub fn notice(store: &Store, warnings: &[Diagnostic]) -> Option<Diagnostic> {
    let legacy = store.dir().join(LEGACY_FRONTEND_FILE);
    let superseded = legacy != store.frontend_path() && legacy.is_file();
    if !superseded && !frontend_refused(store, warnings) {
        return None;
    }

    let headline = if superseded {
        format!(
            "the front-end cache format changed; `{}` is no longer read",
            legacy.display()
        )
    } else {
        "the front-end cache was discarded".to_string()
    };

    Some(
        Diagnostic::warning(codes::CACHE_VERSION_CHANGED, headline)
            .note("this run recomputes types and hashes for the whole project")
            .note("the result cache is untouched, so no test re-runs because of this")
            .note("nothing to do: the front-end cache is rebuilt as this run goes"),
    )
}

/// Both caches degrade with the same three codes, so the file a warning names is the only thing
/// that tells them apart — and only the front-end one costs a recompile worth explaining.
fn frontend_refused(store: &Store, warnings: &[Diagnostic]) -> bool {
    let path = store.frontend_path().display().to_string();
    warnings.iter().any(|w| {
        matches!(
            w.code,
            codes::CACHE_UNREADABLE | codes::CACHE_CORRUPT | codes::CACHE_VERSION_CHANGED
        ) && w.message.contains(&path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path) -> Store {
        Store::open(dir).unwrap()
    }

    #[test]
    fn a_healthy_cache_says_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        assert!(notice(&store, &[]).is_none());
    }

    /// The result cache degrading is not this: its contents are lost, but no type or hash is
    /// recomputed and the reassurance below would be a lie.
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

    /// Inert while the store's own front-end file is the legacy one, which is what makes this
    /// detection safe to ship before the binary store does.
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
}
