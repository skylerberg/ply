use super::common::{
    IND, diagnostic_json, emit_json, once_each, plural, print_diagnostics, print_warnings,
    report_load_error,
};
use crate::cli::CheckArgs;
use crate::driver;
use crate::load::{Loaded, load, project_root};
use crate::signature;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_core::print_scheme;
use ply_span::{Diagnostic, Symbol};
use ply_store::Store;
use ply_syntax::ast::ModuleName;
use serde_json::{Value, json};

/// The contextual keyword a general clause is written with.
const RESUME: &str = "resume";

/// What every line of the `--types` block is printed at: `IND` plus the two
/// spaces that put a definition under its module heading. Passed to the
/// renderer so that a wrapped row respects the terminal's right edge rather
/// than its own.
const TYPES_INDENT: usize = IND.len() + 2;

pub fn execute(args: &CheckArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let (mut loaded, mut store) = match check(args, &mut warnings) {
        Ok(pair) => pair,
        Err(err) => return report_load_error("check", &err, args.json, style),
    };

    let refused = match refuse_machine_only(args, &mut loaded, store.as_mut(), &mut warnings) {
        Ok(refused) => refused,
        Err(err) => return report_load_error("check", &err, args.json, style),
    };
    let warnings = once_each(warnings);

    if args.json {
        let mut report = report_json(&loaded, &warnings);
        if !refused.is_empty() {
            let rendered: Vec<Value> = refused
                .iter()
                .map(|d| diagnostic_json(d, &loaded.sources))
                .collect();
            report["ok"] = json!(false);
            report["exit_code"] = json!(EXIT_COMPILE_ERROR);
            report["diagnostics"] = Value::Array(rendered);
            emit_json(&report);
            return EXIT_COMPILE_ERROR;
        }
        // After `front_end` is recorded, so that completing the parse cannot
        // rewrite the report of what the gates decided.
        if args.explain {
            if let Err(err) = complete_parse(args, &mut loaded, store.as_mut()) {
                return report_load_error("check", &err, args.json, style);
            }
            attach_provenance(&mut report, &loaded);
        }
        emit_json(&report);
        return EXIT_OK;
    }

    if !refused.is_empty() {
        print_diagnostics(&refused, &loaded.sources, style);
        return EXIT_COMPILE_ERROR;
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
        // Only now, so that `print_explain` above still reports the gates as
        // they actually fired.
        if args.explain
            && let Err(err) = complete_parse(args, &mut loaded, store.as_mut())
        {
            return report_load_error("check", &err, args.json, style);
        }
        print_types(&loaded, args.explain, style);
    }
    EXIT_OK
}

/// Parses whatever gate 1 skipped.
///
/// `--explain`'s effect-set table and alias provenance are read from the AST, so
/// a run that skipped a file would print less than a run that did not — and ADR
/// 0013 §1.6 requires the reviewing command's bytes to be identical either way.
/// The cheapest way to guarantee that is to make the output a function of the
/// source rather than of what the cache held.
///
/// The store's own warnings are drained and dropped: this is a second read of
/// files the first load already reported on, and a duplicate "the cache is
/// unwritable" line tells the reader nothing new.
fn complete_parse(
    args: &CheckArgs,
    loaded: &mut Loaded,
    store: Option<&mut Store>,
) -> Result<(), crate::load::LoadError> {
    let missing: Vec<ModuleName> = loaded
        .modules()
        .iter()
        .filter(|m| !loaded.has_ast(m.name))
        .map(|m| m.name.clone())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    *loaded = match store {
        Some(store) => {
            let full = driver::load_to_evaluate(&args.path, store, &missing);
            let _ = store.take_warnings();
            full?
        }
        None => load(&args.path)?,
    };
    Ok(())
}

/// A program the chosen engine cannot express is not a program that checks,
/// whatever inference said about it. Only `treewalk` refuses: under `both` such
/// a program runs once, on the machine, so it is runnable.
///
/// The clauses live in the AST, and gate 1 may have skipped the file that holds
/// one — so a scan of what this run happened to parse would make the exit code
/// depend on what the cache held, over source that never changed. Any skipped
/// file whose text mentions `resume` is parsed first. The binder has to be
/// written in the file that writes the clause, so the pre-filter cannot miss
/// one, and a project with no general clause pays a substring search per file.
fn refuse_machine_only(
    args: &CheckArgs,
    loaded: &mut Loaded,
    store: Option<&mut Store>,
    warnings: &mut Vec<Diagnostic>,
) -> Result<Vec<Diagnostic>, crate::load::LoadError> {
    if ply_eval::EngineChoice::from(args.engine) != ply_eval::EngineChoice::Treewalk {
        return Ok(Vec::new());
    }
    let unscanned: Vec<ModuleName> = loaded
        .modules()
        .iter()
        .filter(|m| !loaded.has_ast(m.name))
        .filter(|m| {
            loaded
                .sources
                .get(m.info.source)
                .is_some_and(|f| f.text.contains(RESUME))
        })
        .map(|m| m.name.clone())
        .collect();
    if !unscanned.is_empty() {
        *loaded = match store {
            Some(store) => {
                let full = driver::load_to_evaluate(&args.path, store, &unscanned);
                warnings.extend(store.take_warnings());
                full?
            }
            None => load(&args.path)?,
        };
    }
    Ok(ply_eval::machine_only_clauses(&loaded.program))
}

/// A cache that cannot be opened is never a reason to refuse to typecheck: the
/// front end degrades to the full path and says so.
fn check(
    args: &CheckArgs,
    warnings: &mut Vec<Diagnostic>,
) -> Result<(Loaded, Option<Store>), crate::load::LoadError> {
    if args.no_incremental {
        return Ok((load(&args.path)?, None));
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
            Ok((loaded?, Some(store)))
        }
        Err(_) => Ok((load(&args.path)?, None)),
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
///
/// A definition's effect row is printed on its own wrapped line under the type
/// rather than run onto the end of it. That is the whole of W3's exit criterion:
/// a service has a hundred endpoints and each one's row is what says which
/// resources it touches, so a row that scrolls off the right edge is a row
/// nobody reads. A pure definition prints no row at all — the absence of a line
/// says more than `{}` does.
///
/// `explain` adds the `effect set` table and, per definition, the alias its row
/// was written with. The **expansion** is what the signature line prints, always
/// and whatever the flag says: ADR 0013 §1.7's rule that the truth needs no flag.
fn print_types(loaded: &Loaded, explain: bool, style: Style) {
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

        let sets = if explain {
            signature::effect_sets(
                &loaded.program,
                &loaded.resolved,
                &loaded.check,
                module.name,
                &defs,
            )
        } else {
            Vec::new()
        };
        for set in &sets {
            println!();
            for line in set.lines(TYPES_INDENT) {
                println!("{IND}  {line}");
            }
        }
        if !sets.is_empty() {
            println!();
        }

        let width = defs
            .iter()
            .map(|d| d.simple_name.as_str().chars().count())
            .max()
            .unwrap_or(0);
        for def in &defs {
            for line in signature::definition_lines(
                TYPES_INDENT,
                width,
                def.simple_name.as_str(),
                &def.scheme,
            ) {
                println!("{IND}  {line}");
            }
            if explain {
                for line in signature::provenance(def).lines(TYPES_INDENT) {
                    println!("{IND}  {}", style.dim(&line));
                }
            }
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

/// Adds under `--explain` what the AST knows and the check output does not: the
/// `effect set` table per module and, per definition, the sets its row named.
///
/// Only under `--explain`, and only after [`complete_parse`], so that these
/// fields are either absent or a function of the source — never a function of
/// which files gate 1 skipped.
fn attach_provenance(report: &mut Value, loaded: &Loaded) {
    if let Some(modules) = report["modules"].as_array_mut() {
        for entry in modules {
            let name = ModuleName::from_dotted(entry["name"].as_str().unwrap_or_default());
            let defs = loaded.defs_of(&name);
            let sets: Vec<Value> = signature::effect_sets(
                &loaded.program,
                &loaded.resolved,
                &loaded.check,
                &name,
                &defs,
            )
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "expansion": s.atoms,
                    "used_by": s.used_by,
                })
            })
            .collect();
            entry["effect_sets"] = Value::Array(sets);
        }
    }

    if let Some(defs) = report["definitions"].as_array_mut() {
        for entry in defs {
            let Some(def) = entry["name"]
                .as_str()
                .and_then(|n| loaded.check.defs.get(&Symbol::new(n)))
            else {
                continue;
            };
            entry["written_as"] = json!(
                def.row_aliases
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
            );
            entry["performed"] = json!(
                def.performed
                    .atoms()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
            );
            entry["declared_not_performed"] = json!(signature::provenance(def).unperformed);
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

    /// The report also carries the prelude's own effects, so an effect is found
    /// by the name the module gave it rather than by its position.
    fn effect_named<'a>(report: &'a Value, name: &str) -> &'a Value {
        report["effects"]
            .as_array()
            .expect("effects is a list")
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("no effect named {name} in {report:#}"))
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
        let db = effect_named(&v, "m.db");
        assert_eq!(db["simple_name"], "db");
        assert_eq!(db["nondet"], false);
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
            fixture("nondet effect wall {\n  read now() -> Int\n}\nfn f() -> Int = 1\n");
        let v = report_json(&loaded, &[]);
        let wall = effect_named(&v, "m.wall");
        assert_eq!(wall["simple_name"], "wall");
        assert_eq!(wall["nondet"], true);
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
