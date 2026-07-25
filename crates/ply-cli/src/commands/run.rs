use super::common::{
    IND, diagnostic_json, emit_json, location, print_diagnostics, report_load_error,
};
use crate::cli::RunArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK};
use ply_core::DefInfo;
use ply_eval::Interp;
use ply_span::{Diagnostic, Span, codes};
use serde_json::{Value, json};

pub fn execute(args: &RunArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("run", &err, args.json, style),
    };

    let entry = match entry_point(&loaded) {
        Ok(entry) => entry,
        Err(diagnostic) => {
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "diagnostics": [diagnostic_json(&diagnostic, &loaded.sources)],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &loaded.sources, style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };

    let name = entry.name.clone();
    let module = entry.module.to_string();
    let span = entry.span;
    let mut interp = Interp::new(&loaded.program, &loaded.resolved, &loaded.check);

    match interp.call(name.as_str(), Vec::new(), span) {
        Ok(value) => {
            let rendered = value.to_string();
            if args.json {
                emit_json(&json!({
                    "command": "run",
                    "ok": true,
                    "exit_code": EXIT_OK,
                    "root": loaded.root.display().to_string(),
                    "files": loaded.file_names(),
                    "entry": name,
                    "module": module,
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
                    "entry": name,
                    "module": module,
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

/// A file argument names one module, so its `main` is the only candidate. A
/// directory may hold several, and picking one would make the answer depend on
/// which file sorted first — so it is refused and the candidates are listed.
pub fn entry_point(loaded: &Loaded) -> Result<&DefInfo, Diagnostic> {
    let mut candidates = loaded.entry_points();
    match candidates.len() {
        0 => Err(no_main(loaded)),
        1 => Ok(candidates.remove(0)),
        _ => Err(ambiguous_main(loaded, &candidates)),
    }
}

/// Inference already proved every name resolves, so a missing `main` is a
/// missing entry point rather than an unbound reference — say so before the
/// evaluator gets a chance to phrase it worse.
fn no_main(loaded: &Loaded) -> Diagnostic {
    let span = loaded
        .program
        .modules
        .first()
        .and_then(|m| m.items.first())
        .map(|item| item.span())
        .unwrap_or(Span::DUMMY);

    let mut diagnostic = Diagnostic::error(codes::UNKNOWN_NAME, "no `main` to run")
        .primary(span, "no module in this program defines an entry point")
        .note("add `fn main() -> Unit = ...` to one of the loaded modules")
        .note("`ply test` runs the tests; `ply run` runs `main`");

    if loaded.module_count() > 1 {
        let names: Vec<&str> = loaded.modules().iter().map(|m| m.name.as_str()).collect();
        diagnostic = diagnostic.note(format!("modules loaded: {}", names.join(", ")));
    }
    diagnostic
}

fn ambiguous_main(loaded: &Loaded, candidates: &[&DefInfo]) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::AMBIGUOUS_ENTRY_POINT,
        format!("{} modules declare `main`", candidates.len()),
    );

    for (i, def) in candidates.iter().enumerate() {
        let message = format!("`{}` declares `main` here", def.module);
        diagnostic = if i == 0 {
            diagnostic.primary(def.span, message)
        } else {
            diagnostic.secondary(def.span, message)
        };
    }

    for def in candidates {
        let path = loaded
            .check
            .modules
            .get(def.module.as_symbol())
            .map(|m| loaded.path_of(m.source).display().to_string())
            .unwrap_or_else(|| def.module.to_string());
        diagnostic = diagnostic.note(format!("run it with `ply run {path}`"));
    }

    diagnostic.note("a directory is a whole program, so `ply run` will not pick one for you")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn loaded(text: &str) -> (tempfile::TempDir, Loaded) {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "m.ply", text);
        let l = load(dir.path()).unwrap();
        (dir, l)
    }

    fn eval(l: &Loaded) -> Result<String, Diagnostic> {
        let entry = entry_point(l)?;
        let (name, span) = (entry.name.clone(), entry.span);
        Interp::new(&l.program, &l.resolved, &l.check)
            .call(name.as_str(), Vec::new(), span)
            .map(|v| v.to_string())
    }

    #[test]
    fn main_is_evaluated_and_its_value_rendered() {
        let (_dir, l) = loaded("fn main() -> Int = 20 + 22\n");
        assert_eq!(eval(&l).unwrap(), "42");
    }

    #[test]
    fn a_missing_main_points_at_the_program_rather_than_nowhere() {
        let (_dir, l) = loaded("fn other() -> Int = 1\n");
        let d = no_main(&l);
        assert_eq!(d.code, codes::UNKNOWN_NAME);
        assert!(!d.primary_span().unwrap().is_dummy());
        assert!(d.notes.iter().any(|n| n.contains("fn main")));
    }

    #[test]
    fn a_raising_main_yields_a_diagnostic_not_a_panic() {
        let (_dir, l) = loaded("fn main() -> Unit = panic(\"nope\")\n");
        let err = eval(&l).unwrap_err();
        assert_eq!(err.code, codes::RUNTIME_ERROR);
        assert!(location(&l.sources, err.primary_span().unwrap()).is_some());
    }

    #[test]
    fn the_one_main_in_a_multi_module_program_is_the_entry_point() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "lib.ply", "pub fn answer() -> Int = 42\n");
        write(dir.path(), "app.ply", "import lib\nfn main() -> Int = lib::answer()\n");

        let l = load(dir.path()).unwrap();
        assert_eq!(entry_point(&l).unwrap().name.as_str(), "app.main");
        assert_eq!(eval(&l).unwrap(), "42");
    }

    #[test]
    fn several_mains_are_refused_with_the_candidates_named() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
        write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

        let l = load(dir.path()).unwrap();
        let d = entry_point(&l).unwrap_err();
        assert_eq!(d.code, codes::AMBIGUOUS_ENTRY_POINT);
        assert!(d.message.contains("2 modules"));
        assert_eq!(d.labels.len(), 2);
        assert!(d.labels.iter().all(|l| !l.span.is_dummy()));
        assert!(d.notes.iter().any(|n| n.contains("one.ply")));
        assert!(d.notes.iter().any(|n| n.contains("two.ply")));
    }

    #[test]
    fn naming_the_file_resolves_the_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
        write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

        let l = load(&dir.path().join("two.ply")).unwrap();
        assert_eq!(entry_point(&l).unwrap().name.as_str(), "two.main");
        assert_eq!(eval(&l).unwrap(), "2");
    }

    #[test]
    fn a_missing_main_lists_the_modules_that_were_searched() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ply", "fn f() -> Int = 1\n");
        write(dir.path(), "b.ply", "fn g() -> Int = 2\n");

        let l = load(dir.path()).unwrap();
        let d = entry_point(&l).unwrap_err();
        assert_eq!(d.code, codes::UNKNOWN_NAME);
        assert!(d.notes.iter().any(|n| n.contains("a, b")), "notes: {:?}", d.notes);
    }
}
