use super::common::{
    IND, diagnostic_json, emit_json, location, print_diagnostics, report_load_error,
};
use crate::cli::RunArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK};
use ply_eval::Interp;
use ply_span::{Diagnostic, Span, Symbol, codes};
use serde_json::{Value, json};

pub fn execute(args: &RunArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("run", &err, args.json, style),
    };

    if !loaded.check.defs.contains_key(&Symbol::new("main")) {
        let diagnostic = no_main(&loaded);
        if args.json {
            emit_json(&json!({
                "command": "run",
                "ok": false,
                "exit_code": EXIT_COMPILE_ERROR,
                "diagnostics": [diagnostic_json(&diagnostic, &loaded.sources)],
            }));
        } else {
            print_diagnostics(std::slice::from_ref(&diagnostic), &loaded.sources, style);
        }
        return EXIT_COMPILE_ERROR;
    }

    match Interp::new(&loaded.module, &loaded.check).eval_main() {
        Ok(value) => {
            let rendered = value.to_string();
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": true,
                    "exit_code": EXIT_OK,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "value": rendered,
                    "diagnostics": Value::Array(Vec::new()),
                }));
            } else {
                println!("{IND}{rendered}");
            }
            EXIT_OK
        }
        Err(diagnostic) => {
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": false,
                    "exit_code": EXIT_FAILED,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "value": Value::Null,
                    "diagnostics": [diagnostic_json(&diagnostic, &loaded.sources)],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &loaded.sources, style);
                if let Some(at) =
                    diagnostic.primary_span().and_then(|s| location(&loaded.sources, s))
                {
                    eprintln!("{IND}{} {at}", style.red("raised at"));
                }
            }
            EXIT_FAILED
        }
    }
}

/// Inference already proved every name resolves, so a missing `main` is a
/// missing entry point rather than an unbound reference — say so before the
/// evaluator gets a chance to phrase it worse.
fn no_main(loaded: &Loaded) -> Diagnostic {
    let span = loaded
        .module
        .items
        .first()
        .map(|item| item.span())
        .unwrap_or(Span::DUMMY);

    Diagnostic::error(codes::UNKNOWN_NAME, "no `main` to run")
        .primary(span, "this module defines no entry point")
        .note("add `fn main() -> Unit = ...` to one of the loaded files")
        .note("`ply test` runs the tests; `ply run` runs `main`")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(text: &str) -> (tempfile::TempDir, Loaded) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.ply"), text).unwrap();
        let l = load(dir.path()).unwrap();
        (dir, l)
    }

    #[test]
    fn main_is_evaluated_and_its_value_rendered() {
        let (_dir, l) = loaded("fn main() -> Int = 20 + 22\n");
        let value = Interp::new(&l.module, &l.check).eval_main().unwrap();
        assert_eq!(value.to_string(), "42");
    }

    #[test]
    fn a_missing_main_points_at_the_module_rather_than_nowhere() {
        let (_dir, l) = loaded("fn other() -> Int = 1\n");
        let d = no_main(&l);
        assert_eq!(d.code, codes::UNKNOWN_NAME);
        assert!(!d.primary_span().unwrap().is_dummy());
        assert!(d.notes.iter().any(|n| n.contains("fn main")));
    }

    #[test]
    fn a_raising_main_yields_a_diagnostic_not_a_panic() {
        let (_dir, l) = loaded("fn main() -> Unit = panic(\"nope\")\n");
        let err = Interp::new(&l.module, &l.check).eval_main().unwrap_err();
        assert_eq!(err.code, codes::RUNTIME_ERROR);
        assert!(location(&l.sources, err.primary_span().unwrap()).is_some());
    }
}
