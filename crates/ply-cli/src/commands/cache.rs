use super::common::{IND, diagnostic_json, emit_json, plural, print_warnings};
use crate::cli::CacheScope;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, Span, SourceMap, codes};
use ply_store::{RUNTIME_VERSION, Store};
use serde_json::{Value, json};

pub fn stats(scope: &CacheScope, style: Style) -> i32 {
    let mut store = match open(scope, "stats", style) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let warnings = store.take_warnings();

    if scope.json {
        emit_json(&json!({
            "command": "cache",
            "action": "stats",
            "ok": true,
            "exit_code": EXIT_OK,
            "runtime_version": RUNTIME_VERSION,
            "directory": store.dir().display().to_string(),
            "results_file": store.path().display().to_string(),
            "entries": store.len(),
            "warnings": warnings_json(&warnings),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    println!("{IND}{}", style.dim(&store.path().display().to_string()));
    println!(
        "{IND}runtime {RUNTIME_VERSION} · {} cached {}",
        style.bold(&store.len().to_string()),
        plural(store.len(), "result")
    );
    EXIT_OK
}

pub fn clear(scope: &CacheScope, style: Style) -> i32 {
    let mut store = match open(scope, "clear", style) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let mut warnings = store.take_warnings();
    let before = store.len();

    if let Err(e) = store.clear() {
        let diagnostic = Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("could not clear the cache at `{}`: {e:#}", store.dir().display()),
        )
        .primary(Span::DUMMY, "the cache was left as it was")
        .note("check the directory's permissions, or delete it by hand");

        if scope.json {
            emit_json(&json!({
                "command": "cache",
                "action": "clear",
                "ok": false,
                "exit_code": EXIT_COMPILE_ERROR,
                "directory": store.dir().display().to_string(),
                "cleared": 0,
                "diagnostics": [diagnostic_json(&diagnostic, &SourceMap::new())],
            }));
        } else {
            warnings.push(diagnostic);
            print_warnings(&warnings, style);
        }
        return EXIT_COMPILE_ERROR;
    }

    if scope.json {
        emit_json(&json!({
            "command": "cache",
            "action": "clear",
            "ok": true,
            "exit_code": EXIT_OK,
            "directory": store.dir().display().to_string(),
            "cleared": before,
            "warnings": warnings_json(&warnings),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    println!(
        "{IND}{} {before} cached {} from {}",
        style.green("cleared"),
        plural(before, "result"),
        style.dim(&store.dir().display().to_string())
    );
    EXIT_OK
}

fn open(scope: &CacheScope, action: &str, style: Style) -> Result<Store, i32> {
    match Store::open(&scope.path) {
        Ok(store) => Ok(store),
        Err(e) => {
            let diagnostic = Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not open a cache under `{}`: {e:#}", scope.path.display()),
            )
            .primary(Span::DUMMY, "the cache directory is unusable")
            .note("pass the directory the cache belongs to; the default is `.`");

            if scope.json {
                emit_json(&json!({
                    "command": "cache",
                    "action": action,
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": [diagnostic_json(&diagnostic, &SourceMap::new())],
                }));
            } else {
                super::common::print_diagnostics(
                    std::slice::from_ref(&diagnostic),
                    &SourceMap::new(),
                    style,
                );
            }
            Err(EXIT_COMPILE_ERROR)
        }
    }
}

fn warnings_json(warnings: &[Diagnostic]) -> Value {
    let sources = SourceMap::new();
    Value::Array(warnings.iter().map(|w| diagnostic_json(w, &sources)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_hash::DefHash;
    use ply_store::Outcome;

    #[test]
    fn clearing_empties_a_populated_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.put(DefHash([7u8; 32]), Outcome::Pass);
        store.flush().unwrap();
        assert_eq!(Store::open(dir.path()).unwrap().len(), 1);

        let scope = CacheScope { path: dir.path().to_path_buf(), json: false };
        assert_eq!(clear(&scope, Style::plain()), EXIT_OK);
        assert_eq!(Store::open(dir.path()).unwrap().len(), 0);
    }

    #[test]
    fn stats_on_a_directory_with_no_cache_creates_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let scope = CacheScope { path: dir.path().to_path_buf(), json: false };
        assert_eq!(stats(&scope, Style::plain()), EXIT_OK);
        assert!(dir.path().join(ply_store::CACHE_DIR_NAME).is_dir());
    }

    #[test]
    fn a_corrupt_cache_degrades_to_empty_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        Store::open(dir.path()).unwrap();
        let path = dir.path().join(ply_store::CACHE_DIR_NAME);
        std::fs::write(path.join("results.json"), "{ not json at all").unwrap();

        let mut store = Store::open(dir.path()).unwrap();
        assert_eq!(store.len(), 0);
        let warnings = store.take_warnings();
        assert!(!warnings.is_empty());
        assert!(!warnings_json(&warnings).as_array().unwrap().is_empty());
    }
}
