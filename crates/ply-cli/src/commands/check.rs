use super::common::{IND, emit_json, plural, print_warnings, report_load_error};
use crate::EXIT_OK;
use crate::cli::CheckArgs;
use crate::driver;
use crate::load::{Loaded, load, project_root};
use crate::style::Style;
use ply_core::print_scheme;
use ply_span::Diagnostic;
use ply_store::Store;
use serde_json::{Value, json};

pub fn execute(args: &CheckArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let loaded = match check(args, &mut warnings) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("check", &err, args.json, style),
    };

    if args.json {
        emit_json(&report_json(&loaded, &warnings));
        return EXIT_OK;
    }

    let modules = loaded.module_count();
    println!(
        "{IND}{} {modules} {}, {} {}, {} {}",
        style.green("checked"),
        plural(modules, "module"),
        loaded.check.defs.len(),
        plural(loaded.check.defs.len(), "definition"),
        loaded.check.tests.len(),
        plural(loaded.check.tests.len(), "test"),
    );
    print_warnings(&warnings, style);

    if args.explain {
        print_explain(&loaded, style);
    }
    if args.types {
        print_types(&loaded, style);
    }
    EXIT_OK
}

/// A cache that cannot be opened is never a reason to refuse to typecheck: the
/// front end degrades to the full path and says so.
fn check(
    args: &CheckArgs,
    warnings: &mut Vec<Diagnostic>,
) -> Result<Loaded, crate::load::LoadError> {
    if args.no_incremental {
        return load(&args.path);
    }
    let root = project_root(&args.path);
    match Store::open(&root) {
        Ok(mut store) => {
            let opened = store.take_warnings();
            let migration = crate::migrate::notice(&store, &opened);
            warnings.extend(opened);
            warnings.extend(migration);

            let loaded = driver::load_incremental(&args.path, &mut store);
            warnings.extend(store.take_warnings());
            if let Ok(loaded) = &loaded {
                warnings.extend(loaded.frontend.warnings.iter().cloned());
            }
            loaded
        }
        Err(_) => load(&args.path),
    }
}

fn print_explain(loaded: &Loaded, style: Style) {
    println!();
    println!("{IND}{}", style.bold("front end"));
    for file in &loaded.frontend.files {
        let state = if !file.parsed {
            style.green("skipped")
        } else if file.rechecked {
            style.yellow("checked")
        } else {
            style.dim("parsed")
        };
        println!(
            "{IND}  {state:<9} {} {}",
            file.path.display(),
            style.dim(&file.refusal.describe())
        );
    }
    for def in &loaded.frontend.defs {
        let state = if def.cached {
            style.green("cached")
        } else {
            style.yellow("rechecked")
        };
        println!("{IND}  {state:<9} {}", def.name);
    }
    super::common::print_phases(&loaded.frontend.phases, style);
}

/// Grouped by module and printed with simple names: the module heading already
/// carries the qualification, and repeating it on every line would bury the
/// signatures the flag was asked for.
fn print_types(loaded: &Loaded, style: Style) {
    for module in loaded.modules() {
        let defs = loaded.defs_of(module.name);
        let tests = loaded.tests_of(module.name);
        let effects: Vec<_> = loaded
            .check
            .effects
            .values()
            .filter(|e| &e.module == module.name)
            .collect();

        println!();
        println!(
            "{IND}{} {}",
            style.bold(module.name.as_str()),
            style.dim(&module.path.display().to_string())
        );
        for import in &module.info.imports {
            println!("{IND}  {} {import}", style.dim("import"));
        }

        for effect in effects {
            let marker = if effect.nondet {
                "nondet effect"
            } else {
                "effect"
            };
            println!(
                "{IND}  {} {}",
                style.dim(marker),
                style.bold(effect.simple_name.as_str())
            );
            for op in effect.ops.values() {
                let resource = if op.resource_param { "[r]" } else { "" };
                let params: Vec<String> = op.params.iter().map(ply_core::print_type).collect();
                println!(
                    "{IND}    {} {}{resource}({}) -> {}",
                    style.dim(op.mode.as_str()),
                    op.name,
                    params.join(", "),
                    ply_core::print_type(&op.ret),
                );
            }
        }

        let width = defs
            .iter()
            .map(|d| d.simple_name.as_str().chars().count())
            .max()
            .unwrap_or(0);
        for def in &defs {
            println!(
                "{IND}  {:width$} : {}",
                def.simple_name.as_str(),
                print_scheme(&def.scheme),
                width = width
            );
        }

        for (_, test) in &tests {
            let kind = if test.nondet { "test/nondet" } else { "test" };
            println!(
                "{IND}  {} {:?} : {}",
                style.dim(kind),
                test.name,
                style.dim(&test.footprint.to_string())
            );
        }
    }
}

fn report_json(loaded: &Loaded, warnings: &[Diagnostic]) -> Value {
    let modules: Vec<Value> = loaded
        .modules()
        .iter()
        .map(|m| {
            json!({
                "name": m.name.as_str(),
                "file": m.path.display().to_string(),
                "imports": m.info.imports.iter().map(|i| i.as_str()).collect::<Vec<_>>(),
                "items": m.info.items,
            })
        })
        .collect();

    let defs: Vec<Value> = loaded
        .check
        .defs
        .values()
        .map(|d| {
            json!({
                "name": d.name,
                "module": d.module.as_str(),
                "simple_name": d.simple_name,
                "type": print_scheme(&d.scheme),
                "footprint": d.footprint.to_string(),
                "atoms": d.footprint.atoms().map(|a| a.to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();

    let tests: Vec<Value> = loaded
        .check
        .tests
        .iter()
        .map(|t| {
            json!({
                "index": t.index,
                "key": t.key,
                "name": t.name,
                "module": t.module.as_str(),
                "nondet": t.nondet,
                "footprint": t.footprint.to_string(),
            })
        })
        .collect();

    let effects: Vec<Value> = loaded
        .check
        .effects
        .values()
        .map(|e| {
            json!({
                "name": e.name,
                "module": e.module.as_str(),
                "simple_name": e.simple_name,
                "nondet": e.nondet,
                "operations": e.ops.values().map(|op| json!({
                    "name": op.name,
                    "mode": op.mode,
                    "resource_param": op.resource_param,
                    "params": op.params.iter().map(ply_core::print_type).collect::<Vec<_>>(),
                    "returns": ply_core::print_type(&op.ret),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "command": "check",
        "ok": true,
        "exit_code": EXIT_OK,
        "root": loaded.root.display().to_string(),
        "files": loaded.file_names(),
        "modules": modules,
        "definitions": defs,
        "tests": tests,
        "effects": effects,
        "front_end": json!({
            "incremental": loaded.frontend.incremental,
            "parsed": loaded.frontend.parsed(),
            "skipped": loaded.frontend.skipped(),
            "cached": loaded.frontend.cached(),
            "rechecked": loaded.frontend.rechecked(),
            "phases": super::common::phases_json(&loaded.frontend.phases),
            "files": loaded.frontend.files.iter().map(|f| json!({
                "file": f.path.display().to_string(),
                "module": f.module.as_str(),
                "parsed": f.parsed,
                "rechecked": f.rechecked,
                "reason": f.refusal.describe(),
            })).collect::<Vec<_>>(),
        }),
        "diagnostics": super::common::diagnostics_json(warnings, &loaded.sources),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EXIT_COMPILE_ERROR;
    use std::path::Path;

    fn fixture(text: &str) -> (tempfile::TempDir, Loaded) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.ply"), text).unwrap();
        let loaded = load(dir.path()).unwrap();
        (dir, loaded)
    }

    #[test]
    fn the_json_report_publishes_signatures_and_footprints() {
        let (_dir, loaded) = fixture(
            "effect db {\n  read all[t]() -> List<Int>\n}\n\
             fn total() -> Int / {db.read[users]} = len(db.all[users]())\n\
             test \"total counts\" {\n  handle { assert_eq(total(), 0) } with { db.all[users]() -> [] }\n}\n",
        );
        let v = report_json(&loaded, &[]);
        assert_eq!(v["command"], "check");
        assert_eq!(v["ok"], true);
        assert_eq!(v["definitions"][0]["name"], "m.total");
        assert_eq!(v["definitions"][0]["module"], "m");
        assert_eq!(v["definitions"][0]["simple_name"], "total");
        assert_eq!(v["definitions"][0]["footprint"], "{m.db.read[users]}");
        assert_eq!(v["effects"][0]["name"], "m.db");
        assert_eq!(v["effects"][0]["simple_name"], "db");
        assert_eq!(v["effects"][0]["nondet"], false);
        assert_eq!(v["tests"][0]["name"], "total counts");
        assert_eq!(v["tests"][0]["key"], "m.total counts");
        assert_eq!(v["tests"][0]["footprint"], "{}");
    }

    #[test]
    fn a_pure_definition_reports_an_empty_footprint() {
        let (_dir, loaded) = fixture("fn double(x: Int) -> Int = x * 2\n");
        let v = report_json(&loaded, &[]);
        assert_eq!(v["definitions"][0]["type"], "(Int) -> Int");
        assert_eq!(v["definitions"][0]["footprint"], "{}");
        assert!(v["definitions"][0]["atoms"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_nondet_effect_is_flagged_in_the_report() {
        let (_dir, loaded) =
            fixture("nondet effect clock {\n  read now() -> Int\n}\nfn f() -> Int = 1\n");
        let v = report_json(&loaded, &[]);
        assert_eq!(v["effects"][0]["simple_name"], "clock");
        assert_eq!(v["effects"][0]["nondet"], true);
    }

    #[test]
    fn every_module_is_reported_with_its_file_and_its_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("store")).unwrap();
        std::fs::write(
            dir.path().join("store/orders.ply"),
            "pub fn place() -> Int = 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("app.ply"),
            "import store.orders\nfn run() -> Int = orders::place()\n",
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();
        let v = report_json(&loaded, &[]);
        let modules = v["modules"].as_array().unwrap();
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0]["name"], "app");
        assert_eq!(modules[0]["imports"], json!(["store.orders"]));
        assert_eq!(modules[1]["name"], "store.orders");
        assert!(
            modules[1]["file"]
                .as_str()
                .unwrap()
                .ends_with("store/orders.ply")
        );

        // Definitions follow the run's files and each file's source order. The
        // check's own order is dependency-first, which would have put
        // `store.orders.place` first — and would have put it somewhere else
        // again on a run where gate 1 skipped that module.
        let names: Vec<&str> = v["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["app.run", "store.orders.place"]);
    }

    #[test]
    fn two_modules_may_reuse_a_name_without_colliding() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.ply"), "fn total() -> Int = 1\n").unwrap();
        std::fs::write(dir.path().join("b.ply"), "fn total() -> Int = 2\n").unwrap();

        let loaded = load(dir.path()).unwrap();
        let v = report_json(&loaded, &[]);
        let names: Vec<&str> = v["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["a.total", "b.total"]);
    }

    #[test]
    fn a_broken_module_never_reaches_the_report() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.ply"), "fn f() -> Int = true\n").unwrap();
        let err = load(dir.path()).unwrap_err();
        let code = report_load_error("check", &err, true, Style::plain());
        assert_eq!(code, EXIT_COMPILE_ERROR);
        assert!(!Path::new(&dir.path().join(".ply-cache")).exists());
    }
}
