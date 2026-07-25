use super::common::{IND, diagnostics_json, emit_json, plural, print_diagnostics, report_load_error};
use crate::cli::HashArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_hash::HashOutput;
use serde_json::{Value, json};

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
    for (name, hash) in &hashes.defs {
        println!("{IND}{}  {name}", style.dim(&hash.short()));
        if deps {
            print_edges(hashes, name.as_str(), style);
        }
    }

    if !loaded.check.tests.is_empty() {
        println!();
        for (index, test) in loaded.check.tests.iter().enumerate() {
            let hash = hashes
                .tests
                .get(index)
                .map(|h| h.short())
                .unwrap_or_else(|| "-".repeat(12));
            println!("{IND}{}  {} {:?}", style.dim(&hash), style.dim("test"), test.name);
            if deps {
                print_edges(hashes, &test.name, style);
            }
        }
    }

    println!();
    let n = hashes.defs.len();
    println!(
        "{IND}{n} {} · {} {}",
        plural(n, "definition"),
        hashes.tests.len(),
        plural(hashes.tests.len(), "test")
    );
}

fn print_edges(hashes: &HashOutput, name: &str, style: Style) {
    let key = ply_span::Symbol::new(name);
    if let Some(deps) = hashes.deps.get(&key)
        && !deps.is_empty()
    {
        let names: Vec<&str> = deps.iter().map(|d| d.as_str()).collect();
        println!("{IND}              {} {}", style.dim("deps:"), names.join(", "));
    }
    if let Some(closure) = hashes.closure.get(&key) {
        let names: Vec<&str> = closure.iter().map(|d| d.as_str()).collect();
        println!("{IND}              {} {}", style.dim("closure:"), names.join(", "));
    }
}

fn report_json(loaded: &Loaded, hashes: &HashOutput, deps: bool) -> Value {
    let definitions: Vec<Value> = hashes
        .defs
        .iter()
        .map(|(name, hash)| {
            let mut entry = json!({
                "name": name,
                "hash": hash.to_hex(),
                "short": hash.short(),
            });
            if deps {
                entry["deps"] = json!(hashes.deps.get(name));
                entry["closure"] = json!(hashes.closure.get(name));
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
            let key = ply_span::Symbol::new(&test.name);
            let mut entry = json!({
                "index": index,
                "name": test.name,
                "hash": hashes.tests.get(index).map(|h| h.to_hex()),
            });
            if deps {
                entry["deps"] = json!(hashes.deps.get(&key));
                entry["closure"] = json!(hashes.closure.get(&key));
            }
            entry
        })
        .collect();

    json!({
        "command": "hash",
        "ok": true,
        "exit_code": EXIT_OK,
        "root": loaded.root.display().to_string(),
        "files": loaded.file_names(),
        "definitions": definitions,
        "tests": tests,
        "diagnostics": Value::Array(Vec::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(text: &str) -> (tempfile::TempDir, Loaded, HashOutput) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.ply"), text).unwrap();
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
    fn deps_adds_the_graph_and_the_closure() {
        let (_dir, loaded, hashes) = fixture(SOURCE);
        let v = report_json(&loaded, &hashes, true);
        let two = v["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "two")
            .unwrap();
        assert_eq!(two["deps"], json!(["one"]));
        assert_eq!(two["closure"], json!(["one", "two"]));
        assert_eq!(v["tests"][0]["closure"], json!(["one", "two", "two is two"]));
    }

    fn def(hashes: &HashOutput, name: &str) -> ply_hash::DefHash {
        hashes.defs[&ply_span::Symbol::new(name)]
    }

    #[test]
    fn renaming_a_definition_changes_no_hash_in_the_report() {
        let (_dir, _l, before) = fixture(SOURCE);
        let (_dir2, _l2, after) = fixture(
            "fn uno() -> Int = 1\n\
             fn two() -> Int = uno() + uno()\n\
             test \"two is two\" { assert_eq(two(), 2) }\n",
        );
        assert_eq!(def(&before, "one"), def(&after, "uno"));
        assert_eq!(def(&before, "two"), def(&after, "two"));
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
        assert_ne!(def(&before, "one"), def(&after, "one"));
        assert_ne!(def(&before, "two"), def(&after, "two"));
        assert_ne!(before.tests[0], after.tests[0]);
    }
}
