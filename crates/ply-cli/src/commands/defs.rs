//! `ply defs` — every definition with its place, hash, signature and footprint.

use super::common::{IND, emit_json, location, plural, report_load_error};
use crate::EXIT_OK;
use crate::cli::DefsArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use ply_ty::{DefInfo, HashOutput, print_scheme};
use serde_json::{Value, json};

pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &DefsArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("defs", &err, args.json, style),
    };
    let hashes = match loaded.hashes() {
        Ok(hashes) => hashes,
        Err(diagnostics) => {
            return super::common::report_diagnostics(
                "defs",
                &diagnostics,
                &loaded,
                args.json,
                style,
            );
        }
    };
    let defs: Vec<&DefInfo> = loaded
        .check
        .defs
        .values()
        .filter(|d| !crate::shipped::is_shipped(&d.module))
        .filter(|d| {
            args.filter
                .as_ref()
                .is_none_or(|f| d.name.as_str().contains(f.as_str()))
        })
        .collect();
    if args.json {
        emit_json(&json!({
            "command": "defs",
            "schema_version": SCHEMA_VERSION,
            "ok": true,
            "exit_code": EXIT_OK,
            "definitions": defs.iter().map(|d| definition_json(&loaded, &hashes, d)).collect::<Vec<Value>>(),
            "diagnostics": Value::Array(Vec::new()),
        }));
        return EXIT_OK;
    }
    println!();
    for d in &defs {
        let hash = hashes
            .defs
            .get(&d.name)
            .map(|h| h.short())
            .unwrap_or_default();
        let place = location(&loaded.sources, d.span).unwrap_or_default();
        println!(
            "{IND}{}  {}  {}",
            style.dim(&hash),
            style.bold(d.name.as_str()),
            style.dim(&place)
        );
        println!("{IND}{IND}{} / {}", print_scheme(&d.scheme), d.footprint);
    }
    println!();
    println!("{IND}{} {}", defs.len(), plural(defs.len(), "definition"));
    EXIT_OK
}

/// One definition, as an agent reads it: where, what it is, what it touches, and what it hashes to.
pub fn definition_json(loaded: &Loaded, hashes: &HashOutput, d: &DefInfo) -> Value {
    json!({
        "name": d.name.as_str(),
        "module": d.module.to_string(),
        "simple_name": d.simple_name.as_str(),
        "location": location(&loaded.sources, d.span),
        "file": loaded.sources.containing(d.span).map(|f| f.path.display().to_string()),
        "start": d.span.start,
        "end": d.span.end,
        "hash": hashes.defs.get(&d.name).map(|h| h.to_hex()),
        "own": hashes.own.get(&d.name).map(|h| h.to_hex()),
        "type": print_scheme(&d.scheme),
        "footprint": d.footprint.to_string(),
        "performed": d.performed.to_string(),
        "specs": hashes.specs.get(&d.name).map(|s| s.len()).unwrap_or(0),
        "deps": hashes.deps.get(&d.name).map(|ds| ds.iter().map(|s| s.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
    })
}
