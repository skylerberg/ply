use super::common::{
    IND, diagnostics_json, emit_json, plural, print_diagnostics, report_load_error,
};
use crate::cli::HashArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_hash::HashOutput;
use ply_span::Symbol;
use serde_json::{Value, json};

/// The grouping below is the one thing in this output that could be mistaken for part of a hash.
const MODULES_ARE_NOT_HASHED: &str = "module names, imports and `pub` are erased by normalization: moving a \
     definition between modules changes no hash";

pub fn execute(args: &HashArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("hash", &err, args.json, style),
    };

    let hashes = match loaded.hashes() {
        Ok(hashes) => hashes,
        Err(diagnostics) => {
            if args.json {
                emit_json(&json!({
                    "command": "hash",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": diagnostics_json(&diagnostics, &loaded.sources),
                }));
            } else {
                print_diagnostics(&diagnostics, &loaded.sources, style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };

    if args.json {
        emit_json(&report_json(&loaded, &hashes, args.deps));
        return EXIT_OK;
    }

    print_human(&loaded, &hashes, args.deps, style);
    EXIT_OK
}

fn print_human(loaded: &Loaded, hashes: &HashOutput, deps: bool, style: Style) {
    let blank = "-".repeat(12);

    for module in loaded.modules() {
        println!();
        println!(
            "{IND}{} {}",
            style.bold(module.name.as_str()),
            style.dim(&module.path.display().to_string())
        );

        for def in loaded.defs_of(module.name) {
            let hash = hashes
                .defs
                .get(&def.name)
                .map(|h| h.short())
                .unwrap_or_else(|| blank.clone());
            println!("{IND}  {}  {}", style.dim(&hash), def.simple_name);
            if deps {
                print_edges(hashes, &def.name, style);
            }
        }

        for (index, test) in loaded.tests_of(module.name) {
            let hash = hashes
                .tests
                .get(index)
                .map(|h| h.short())
                .unwrap_or_else(|| blank.clone());
            println!(
                "{IND}  {}  {} {:?}",
                style.dim(&hash),
                style.dim("test"),
                test.name
            );
            if deps {
                print_edges(hashes, &test.key, style);
            }
        }
    }

    println!();
    let n = hashes.defs.len();
    let modules = loaded.module_count();
    println!(
        "{IND}{n} {} · {} {} · {modules} {}",
        plural(n, "definition"),
        hashes.tests.len(),
        plural(hashes.tests.len(), "test"),
        plural(modules, "module"),
    );
    println!("{IND}{}", style.dim(MODULES_ARE_NOT_HASHED));
}

fn print_edges(hashes: &HashOutput, name: &Symbol, style: Style) {
    if let Some(deps) = hashes.deps.get(name)
        && !deps.is_empty()
    {
        let names: Vec<&str> = deps.iter().map(|d| d.as_str()).collect();
        println!(
            "{IND}                {} {}",
            style.dim("deps:"),
            names.join(", ")
        );
    }
    if let Some(closure) = hashes.closure.get(name) {
        let names: Vec<&str> = closure.iter().map(|d| d.as_str()).collect();
        println!(
            "{IND}                {} {}",
            style.dim("closure:"),
            names.join(", ")
        );
    }
}

pub fn report_json(loaded: &Loaded, hashes: &HashOutput, deps: bool) -> Value {
    let definitions: Vec<Value> = loaded
        .check
        .defs
        .values()
        .map(|def| {
            let hash = hashes.defs.get(&def.name);
            let mut entry = json!({
                "name": def.name,
                "module": def.module.as_str(),
                "simple_name": def.simple_name,
                "hash": hash.map(|h| h.to_hex()),
                "short": hash.map(|h| h.short()),
            });
            if deps {
                entry["deps"] = json!(hashes.deps.get(&def.name));
                entry["closure"] = json!(hashes.closure.get(&def.name));
            }
            entry
        })
        .collect();

    let tests: Vec<Value> = loaded
        .check
        .tests
        .iter()
        .enumerate()
        .map(|(index, test)| {
            let mut entry = json!({
                "index": index,
                "key": test.key,
                "name": test.name,
                "module": test.module.as_str(),
                "hash": hashes.tests.get(index).map(|h| h.to_hex()),
            });
            if deps {
                entry["deps"] = json!(hashes.deps.get(&test.key));
                entry["closure"] = json!(hashes.closure.get(&test.key));
            }
            entry
        })
        .collect();

    let modules: Vec<Value> = loaded
        .modules()
        .iter()
        .map(|m| {
            json!({
                "name": m.name.as_str(),
                "file": m.path.display().to_string(),
            })
        })
        .collect();

    json!({
        "command": "hash",
        "ok": true,
        "exit_code": EXIT_OK,
        "root": loaded.root.display().to_string(),
        "files": loaded.file_names(),
        "modules": modules,
        "module_is_hashed": false,
        "note": MODULES_ARE_NOT_HASHED,
        "definitions": definitions,
        "tests": tests,
        "diagnostics": Value::Array(Vec::new()),
    })
}
