//! The shipped modules, checked as one program.

use ply_core::{CheckOutput, check_program};
use ply_span::{Diagnostic, SourceId, Symbol};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(modules: &[(&str, &str)]) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs: Vec<_> = modules
        .iter()
        .enumerate()
        .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
        .collect();
    let mut program = ply_syntax::parse_program(inputs)?;
    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        return Err(diags);
    }
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

fn shipped() -> CheckOutput {
    let modules: Vec<(&str, &str)> = ply_std::sources().collect();
    match compile(&modules) {
        Ok(out) => out,
        Err(d) => panic!("the shipped modules do not check: {d:#?}"),
    }
}

#[test]
fn every_shipped_module_checks() {
    let out = shipped();
    assert!(out.modules.contains_key(&Symbol::new("std.http")));
}

/// The claim ADR 0013 §2 rests on.
#[test]
fn framing_is_pure() {
    let out = shipped();
    for name in [
        "std.http.parse_head",
        "std.http.body_start",
        "std.http.body_step",
        "std.http.encode",
        "std.http.encode_chunked_head",
        "std.http.encode_chunk",
        "std.http.default_limits",
        "std.http.reason_phrase",
    ] {
        let def = out
            .defs
            .get(&Symbol::new(name))
            .unwrap_or_else(|| panic!("`{name}` is not in the program"));
        assert_eq!(
            def.footprint.to_string(),
            "{}",
            "`{name}` performs something"
        );
    }
}

/// The serve loop reaches a socket and says so, and says nothing else: an app's own row is threaded
/// through rather than absorbed, which is what makes an endpoint's footprint visible in `ply check
/// --types`.
#[test]
fn the_serve_loop_publishes_net_and_the_app_s_row() {
    let out = shipped();
    let rows = [
        ("std.http.read_head", "{std.net.net.write[conn]}"),
        ("std.http.read_body", "{std.net.net.write[conn]}"),
        ("std.http.serve_connection", "{std.net.net.write[conn]}"),
    ];
    for (name, expected) in rows {
        let def = out
            .defs
            .get(&Symbol::new(name))
            .unwrap_or_else(|| panic!("`{name}` is not in the program"));
        assert_eq!(def.footprint.to_string(), expected, "{name}");
    }
}
