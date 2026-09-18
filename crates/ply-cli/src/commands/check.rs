use super::common::{IND, emit_json, once_each, plural, print_warnings, report_load_error};
use crate::EXIT_OK;
use crate::cli::CheckArgs;
use crate::driver;
use crate::load::{Loaded, load, project_root};
use crate::signature;
use crate::style::Style;
use ply_span::{Diagnostic, Symbol};
use ply_store::Store;
use ply_syntax::ast::ModuleName;
use ply_ty::print_scheme;
use serde_json::{Value, json};

/// `IND` plus the two spaces that put a definition under its module heading.
const TYPES_INDENT: usize = IND.len() + 2;

pub fn execute(args: &CheckArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let (loaded, _store) = match check(args, &mut warnings) {
        Ok(pair) => pair,
        Err(err) => return report_load_error("check", &err, args.json, style),
    };

    let warnings = once_each(warnings);

    if loaded.promised {
        // Every module is parsed on every load, so the whole-program check has what it needs.
        let broken = crate::costs::promises(&loaded.program, &loaded.resolved);
        if !broken.is_empty() {
            let err = crate::load::LoadError {
                sources: loaded.sources.clone(),
                diagnostics: broken,
            };
            return report_load_error("check", &err, args.json, style);
        }
    }

    if args.json {
        let mut report = report_json(&loaded, &warnings);
        if args.explain {
            attach_provenance(&mut report, &loaded);
        }
        emit_json(&report);
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
        super::common::print_phases(&loaded.frontend.phases, style);
    }
    if args.types {
        print_types(&loaded, args.explain, style);
    }
    if args.costs {
        print_costs(&loaded, style);
    }
    EXIT_OK
}

/// An unopenable cache never refuses a typecheck: the front end degrades to the full path.
fn check(
    args: &CheckArgs,
    warnings: &mut Vec<Diagnostic>,
) -> Result<(Loaded, Option<Store>), crate::load::LoadError> {
    let cache = if args.no_incremental {
        None
    } else {
        Store::open(&project_root(&args.path)).ok()
    };
    let (loaded, store) = match cache {
        Some(mut store) => {
            let opened = store.take_warnings();
            let migration = crate::migrate::notice(&store, &opened);
            warnings.extend(opened);
            warnings.extend(migration);

            let loaded = driver::load_incremental(&args.path, &mut store);
            warnings.extend(store.take_warnings());
            (loaded?, Some(store))
        }
        None => (load(&args.path)?, None),
    };
    warnings.extend(loaded.frontend.warnings.iter().cloned());
    Ok((loaded, store))
}

/// For every `push`, whether it grows its list in place or copies it.
fn print_costs(loaded: &Loaded, style: Style) {
    println!();
    match crate::costs::lines(&loaded.program, &loaded.resolved, &loaded.sources, style) {
        Some(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        None => println!("{IND}{}", style.dim("no appends: nothing to cost")),
    }
}

/// Grouped by module with simple names: the heading already carries the qualification.
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
                let params: Vec<String> = op.params.iter().map(ply_ty::print_type).collect();
                println!(
                    "{IND}    {} {}{resource}({}) -> {}",
                    style.dim(op.mode.as_str()),
                    op.name,
                    params.join(", "),
                    ply_ty::print_type(&op.ret),
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

/// Under `--explain`, the `effect set` table per module and the sets each definition's row named.
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

pub fn report_json(loaded: &Loaded, warnings: &[Diagnostic]) -> Value {
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
                    "params": op.params.iter().map(ply_ty::print_type).collect::<Vec<_>>(),
                    "returns": ply_ty::print_type(&op.ret),
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
            "phases": super::common::phases_json(&loaded.frontend.phases),
        }),
        "diagnostics": super::common::diagnostics_json(warnings, &loaded.sources),
    })
}
