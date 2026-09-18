//! What a run says when it could not use the front-end cache it found.

use ply_span::{Diagnostic, codes};
use ply_store::Store;

/// The superseded single-document front-end cache.
pub const LEGACY_FRONTEND_FILE: &str = "frontend.json";

/// Pass back any `warnings` already drained: a store reports each degradation once.
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
            .note("the result cache is untouched, so no test re-runs because of this")
            .note("nothing to do: the front-end cache is rebuilt as this run goes"),
    )
}

/// Both caches share these codes; only the path in the message tells them apart.
fn frontend_refused(store: &Store, warnings: &[Diagnostic]) -> bool {
    let path = store.frontend_path().display().to_string();
    warnings.iter().any(|w| {
        matches!(
            w.code,
            codes::CACHE_UNREADABLE | codes::CACHE_CORRUPT | codes::CACHE_VERSION_CHANGED
        ) && w.message.contains(&path)
    })
}
