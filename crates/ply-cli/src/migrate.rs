//! What a run says when it could not use the front-end cache it found.

use ply_span::{Diagnostic, codes};
use ply_store::Store;

/// The name the front-end cache had while it was a single JSON document.
pub const LEGACY_FRONTEND_FILE: &str = "frontend.json";

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
