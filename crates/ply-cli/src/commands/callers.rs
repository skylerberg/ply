//! `ply callers` — what mentions a definition: directly, and through any number of calls.

use super::common::{IND, emit_json, location, plural, report_load_error};
use crate::EXIT_OK;
use crate::cli::CallersArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_test::DepEdges;
use ply_ty::HashOutput;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &CallersArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("callers", &err, args.json, style),
    };
    let hashes = match loaded.hashes() {
        Ok(hashes) => hashes,
        Err(diagnostics) => {
            return super::common::report_diagnostics(
                "callers",
                &diagnostics,
                &loaded,
                args.json,
                style,
            );
        }
    };
    let name = match resolve(&loaded, &args.query) {
        Ok(name) => name,
        Err(diagnostic) => {
            return super::common::report_diagnostics(
                "callers",
                &[diagnostic],
                &loaded,
                args.json,
                style,
            );
        }
    };
    let direct = partition(
        &loaded,
        DepEdges::from(&hashes).referrers(&name).cloned().collect(),
    );
    let transitive = partition(&loaded, reached(&hashes, &name));

    if args.json {
        let def = &loaded.check.defs[&name];
        emit_json(&json!({
            "command": "callers",
            "schema_version": SCHEMA_VERSION,
            "ok": true,
            "exit_code": EXIT_OK,
            "definition": {
                "name": name.as_str(),
                "module": def.module.to_string(),
                "location": location(&loaded.sources, def.span),
            },
            "direct": direct.to_json(&loaded),
            "transitive": transitive.to_json(&loaded),
            "diagnostics": Value::Array(Vec::new()),
        }));
        return EXIT_OK;
    }
    println!();
    println!("{IND}{}", style.bold(name.as_str()));
    for (title, group) in [("mentioned by", &direct), ("reached from", &transitive)] {
        println!("{IND}{title}: {}", group.summary());
        for d in &group.definitions {
            println!("{IND}{IND}{d}");
        }
        for t in &group.tests {
            println!("{IND}{IND}test {:?}", loaded.check.tests[*t].name);
        }
        for l in &group.laws {
            println!("{IND}{IND}law {:?}", loaded.check.laws[*l].name);
        }
    }
    EXIT_OK
}

/// A program-wide name, or a simple name that names exactly one definition.
pub(crate) fn resolve(loaded: &Loaded, query: &str) -> Result<Symbol, Diagnostic> {
    let exact = Symbol::new(query);
    if loaded.check.defs.contains_key(&exact) {
        return Ok(exact);
    }
    let matching: Vec<&Symbol> = loaded
        .check
        .defs
        .values()
        .filter(|d| d.simple_name.as_str() == query)
        .map(|d| &d.name)
        .collect();
    match matching[..] {
        [one] => Ok(one.clone()),
        [] => Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("no definition is named `{query}`"),
        )
        .primary(Span::DUMMY, "not a definition of this program")
        .note("the query is a program-wide name such as `store.orders.place`, or a simple name unique in the program")),
        _ => Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("`{query}` names {} definitions", matching.len()),
        )
        .primary(Span::DUMMY, "say which")
        .note(format!(
            "one of: {}",
            matching.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// Every key whose closure holds `name`, other than `name` itself.
fn reached(hashes: &HashOutput, name: &Symbol) -> BTreeSet<Symbol> {
    hashes
        .closure
        .iter()
        .filter(|(key, closure)| *key != name && closure.contains(name))
        .map(|(key, _)| key.clone())
        .collect()
}

struct Group {
    definitions: Vec<Symbol>,
    tests: Vec<usize>,
    laws: Vec<usize>,
}

impl Group {
    fn summary(&self) -> String {
        format!(
            "{} {}, {} {}, {} {}",
            self.definitions.len(),
            plural(self.definitions.len(), "definition"),
            self.tests.len(),
            plural(self.tests.len(), "test"),
            self.laws.len(),
            plural(self.laws.len(), "law")
        )
    }

    fn to_json(&self, loaded: &Loaded) -> Value {
        json!({
            "definitions": self.definitions.iter().map(|d| json!({
                "name": d.as_str(),
                "location": location(&loaded.sources, loaded.check.defs[d].span),
            })).collect::<Vec<_>>(),
            "tests": self.tests.iter().map(|&i| {
                let t = &loaded.check.tests[i];
                json!({ "index": i, "key": t.key.as_str(), "name": t.name, "module": t.module.to_string(),
                        "location": location(&loaded.sources, t.span) })
            }).collect::<Vec<_>>(),
            "laws": self.laws.iter().map(|&i| {
                let l = &loaded.check.laws[i];
                json!({ "index": i, "key": l.key.as_str(), "name": l.name, "module": l.module.to_string(),
                        "location": location(&loaded.sources, l.span) })
            }).collect::<Vec<_>>(),
        })
    }
}

/// Keys sorted into what they name; a key that is none of the three is a declaration and is dropped.
fn partition(loaded: &Loaded, keys: BTreeSet<Symbol>) -> Group {
    let mut group = Group {
        definitions: Vec::new(),
        tests: Vec::new(),
        laws: Vec::new(),
    };
    for key in keys {
        if loaded.check.defs.contains_key(&key) {
            group.definitions.push(key);
        } else if let Some(t) = loaded.check.tests.iter().find(|t| t.key == key) {
            group.tests.push(t.index);
        } else if let Some(l) = loaded.check.laws.iter().find(|l| l.key == key) {
            group.laws.push(l.index);
        }
    }
    group
}
