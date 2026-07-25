use super::common::{IND, emit_json, plural, report_load_error};
use crate::cli::CheckArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::EXIT_OK;
use ply_core::print_scheme;
use serde_json::{Value, json};

pub fn execute(args: &CheckArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("check", &err, args.json, style),
    };

    if args.json {
        emit_json(&report_json(&loaded));
        return EXIT_OK;
    }

    println!(
        "{IND}{} {} {}, {} {}, {} {}",
        style.green("checked"),
        loaded.files.len(),
        plural(loaded.files.len(), "file"),
        loaded.check.defs.len(),
        plural(loaded.check.defs.len(), "definition"),
        loaded.check.tests.len(),
        plural(loaded.check.tests.len(), "test"),
    );

    if args.types {
        print_types(&loaded, style);
    }
    EXIT_OK
}

fn print_types(loaded: &Loaded, style: Style) {
    let width = loaded
        .check
        .defs
        .keys()
        .map(|name| name.as_str().chars().count())
        .max()
        .unwrap_or(0);

    if !loaded.check.effects.is_empty() {
        println!();
        for effect in loaded.check.effects.values() {
            let marker = if effect.nondet { "nondet effect" } else { "effect" };
            println!("{IND}{} {}", style.dim(marker), style.bold(effect.name.as_str()));
            for op in effect.ops.values() {
                let resource = if op.resource_param { "[r]" } else { "" };
                let params: Vec<String> =
                    op.params.iter().map(ply_core::print_type).collect();
                println!(
                    "{IND}  {} {}{resource}({}) -> {}",
                    style.dim(op.mode.as_str()),
                    op.name,
                    params.join(", "),
                    ply_core::print_type(&op.ret),
                );
            }
        }
    }

    if !loaded.check.defs.is_empty() {
        println!();
        for def in loaded.check.defs.values() {
            println!(
                "{IND}{:width$} : {}",
                def.name.as_str(),
                print_scheme(&def.scheme),
                width = width
            );
        }
    }

    if !loaded.check.tests.is_empty() {
        println!();
        for test in &loaded.check.tests {
            let kind = if test.nondet { "test/nondet" } else { "test" };
            println!(
                "{IND}{} {:?} : {}",
                style.dim(kind),
                test.name,
                style.dim(&test.footprint.to_string())
            );
        }
    }
}

fn report_json(loaded: &Loaded) -> Value {
    let defs: Vec<Value> = loaded
        .check
        .defs
        .values()
        .map(|d| {
            json!({
                "name": d.name,
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
                "name": t.name,
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
        "definitions": defs,
        "tests": tests,
        "effects": effects,
        "diagnostics": Value::Array(Vec::new()),
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
        let v = report_json(&loaded);
        assert_eq!(v["command"], "check");
        assert_eq!(v["ok"], true);
        assert_eq!(v["definitions"][0]["name"], "total");
        assert_eq!(v["definitions"][0]["footprint"], "{db.read[users]}");
        assert_eq!(v["effects"][0]["name"], "db");
        assert_eq!(v["effects"][0]["nondet"], false);
        assert_eq!(v["tests"][0]["name"], "total counts");
        assert_eq!(v["tests"][0]["footprint"], "{}");
    }

    #[test]
    fn a_pure_definition_reports_an_empty_footprint() {
        let (_dir, loaded) = fixture("fn double(x: Int) -> Int = x * 2\n");
        let v = report_json(&loaded);
        assert_eq!(v["definitions"][0]["type"], "(Int) -> Int");
        assert_eq!(v["definitions"][0]["footprint"], "{}");
        assert!(v["definitions"][0]["atoms"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_nondet_effect_is_flagged_in_the_report() {
        let (_dir, loaded) =
            fixture("nondet effect clock {\n  read now() -> Int\n}\nfn f() -> Int = 1\n");
        let v = report_json(&loaded);
        assert_eq!(v["effects"][0]["name"], "clock");
        assert_eq!(v["effects"][0]["nondet"], true);
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
