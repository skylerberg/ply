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

/// Printed under every listing, because the grouping below is the one thing in
/// this output that could be mistaken for part of a hash.
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

fn report_json(loaded: &Loaded, hashes: &HashOutput, deps: bool) -> Value {
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

    fn fixture(text: &str) -> (tempfile::TempDir, Loaded, HashOutput) {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "m.ply", text);
        let loaded = load(dir.path()).unwrap();
        let hashes = loaded.hashes().unwrap();
        (dir, loaded, hashes)
    }

    const SOURCE: &str = "fn one() -> Int = 1\n\
                          fn two() -> Int = one() + one()\n\
                          test \"two is two\" { assert_eq(two(), 2) }\n";

    #[test]
    fn every_definition_and_test_gets_a_full_hex_hash() {
        let (_dir, loaded, hashes) = fixture(SOURCE);
        let v = report_json(&loaded, &hashes, false);
        assert_eq!(v["definitions"].as_array().unwrap().len(), 2);
        assert_eq!(v["definitions"][0]["hash"].as_str().unwrap().len(), 64);
        assert_eq!(v["definitions"][0]["short"].as_str().unwrap().len(), 12);
        assert_eq!(v["tests"][0]["hash"].as_str().unwrap().len(), 64);
        assert!(v["definitions"][0].get("deps").is_none());
    }

    #[test]
    fn every_entry_says_which_module_it_came_from_and_that_the_module_is_not_hashed() {
        let (_dir, loaded, hashes) = fixture(SOURCE);
        let v = report_json(&loaded, &hashes, false);
        assert_eq!(v["module_is_hashed"], false);
        assert_eq!(v["definitions"][0]["module"], "m");
        assert_eq!(v["definitions"][0]["name"], "m.one");
        assert_eq!(v["definitions"][0]["simple_name"], "one");
        assert_eq!(v["tests"][0]["module"], "m");
        assert_eq!(v["tests"][0]["key"], "m.two is two");
        assert_eq!(v["modules"][0]["name"], "m");
    }

    #[test]
    fn deps_adds_the_graph_and_the_closure() {
        let (_dir, loaded, hashes) = fixture(SOURCE);
        let v = report_json(&loaded, &hashes, true);
        let two = v["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "m.two")
            .unwrap();
        assert_eq!(two["deps"], json!(["m.one"]));
        assert_eq!(two["closure"], json!(["m.one", "m.two"]));
        assert_eq!(
            v["tests"][0]["closure"],
            json!(["m.one", "m.two", "m.two is two"])
        );
    }

    fn def(hashes: &HashOutput, name: &str) -> ply_hash::DefHash {
        hashes.defs[&Symbol::new(name)]
    }

    #[test]
    fn renaming_a_definition_changes_no_hash_in_the_report() {
        let (_dir, _l, before) = fixture(SOURCE);
        let (_dir2, _l2, after) = fixture(
            "fn uno() -> Int = 1\n\
             fn two() -> Int = uno() + uno()\n\
             test \"two is two\" { assert_eq(two(), 2) }\n",
        );
        assert_eq!(def(&before, "m.one"), def(&after, "m.uno"));
        assert_eq!(def(&before, "m.two"), def(&after, "m.two"));
        assert_eq!(before.tests, after.tests);
    }

    #[test]
    fn editing_a_body_moves_that_hash_and_its_dependents() {
        let (_dir, _l, before) = fixture(SOURCE);
        let (_dir2, _l2, after) = fixture(
            "fn one() -> Int = 2\n\
             fn two() -> Int = one() + one()\n\
             test \"two is two\" { assert_eq(two(), 2) }\n",
        );
        assert_ne!(def(&before, "m.one"), def(&after, "m.one"));
        assert_ne!(def(&before, "m.two"), def(&after, "m.two"));
        assert_ne!(before.tests[0], after.tests[0]);
    }

    #[test]
    fn moving_a_definition_between_modules_changes_no_hash_in_the_report() {
        let together = tempfile::tempdir().unwrap();
        write(
            together.path(),
            "app.ply",
            "fn one() -> Int = 1\n\
             fn two() -> Int = one() + one()\n\
             test \"two is two\" { assert_eq(two(), 2) }\n",
        );
        let l = load(together.path()).unwrap();
        let before = l.hashes().unwrap();

        let apart = tempfile::tempdir().unwrap();
        write(apart.path(), "lib.ply", "pub fn one() -> Int = 1\n");
        write(
            apart.path(),
            "app.ply",
            "import lib\n\
             fn two() -> Int = lib::one() + lib::one()\n\
             test \"two is two\" { assert_eq(two(), 2) }\n",
        );
        let l2 = load(apart.path()).unwrap();
        let after = l2.hashes().unwrap();

        assert_eq!(def(&before, "app.one"), def(&after, "lib.one"));
        assert_eq!(def(&before, "app.two"), def(&after, "app.two"));
        assert_eq!(before.tests, after.tests);
    }
}
