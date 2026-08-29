//! The one thing the differential structurally cannot see.
//!
//! `tests/agreement.rs` compares two dumps. A field that neither dump emits is
//! invisible to it: both sides agree, forever, about a field nobody looks at.
//! The brief for this spike names that failure directly — *"a tree comparison
//! that passes under a dropped field is worth nothing and this project has
//! shipped that exact defect before"* — and `arm-harness.sh` cannot catch it
//! either, because a mutation to a field the dumper ignores changes no output.
//!
//! So this test does not run either parser. It reads `crates/ply-syntax/src/ast.rs`,
//! takes every field of every type the parser builds, and requires each one to
//! be **named** in `src/lib.rs`. That is a weak check — naming a field is not
//! emitting it — but it is exactly strong enough for the class it exists for: a
//! field that is never written down cannot be dumped, and a field added to
//! `ast.rs` tomorrow fails here on the day it is added rather than being
//! absorbed into a green comparison.
//!
//! The exceptions are listed by name, with a reason each, and there are five.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn read(rel: &str) -> String {
    let p: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("<root>/spikes/ply-parser/harness")
        .join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// `Type::field` for every named field of every struct and struct-like enum
/// variant in `ast.rs`, skipping the types the parser never builds.
fn fields_of_ast(src: &str) -> BTreeSet<(String, String)> {
    // Types that are not part of a parsed tree: module identity, the
    // whole-program wrapper, and `Derived`, which only `derive` expansion in a
    // later crate fills in.
    const NOT_PARSED: [&str; 4] = ["ModuleName", "Program", "Module", "Derived"];
    let mut out = BTreeSet::new();
    let mut owner: Option<String> = None;
    for line in src.lines() {
        if let Some(rest) = line
            .strip_prefix("pub struct ")
            .or_else(|| line.strip_prefix("pub enum "))
        {
            owner = Some(
                rest.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("")
                    .to_string(),
            );
            continue;
        }
        // A line at column 0 that is not a declaration ends the declaration.
        if !line.starts_with(char::is_whitespace) && !line.is_empty() && !line.starts_with('}') {
            owner = None;
        }
        let Some(o) = owner.as_ref() else { continue };
        if NOT_PARSED.contains(&o.as_str()) {
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        let body = t.strip_prefix("pub ").unwrap_or(t);
        let Some(colon) = body.find(':') else {
            continue;
        };
        let name = &body[..colon];
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
            || body[colon..].starts_with("::")
        {
            continue;
        }
        out.insert((o.clone(), name.to_string()));
    }
    out
}

/// Fields deliberately not named in the dumper, each with the reason.
///
/// Two entries and no more. Both are `Lit::Decimal`'s, and they are absent
/// because a decimal is dumped as **the raw source over the literal node's own
/// span** rather than as its value: `2.50m` dumps as `2.50m`, which determines
/// `(mantissa 250, scale 2)` and every other reading of those five bytes. Ply
/// cannot build an `i128` from digits, so a dump carrying the value would be
/// comparing `numerics.rs` against nothing at all — the same substitution
/// `spikes/ply-lexer/harness/src/lib.rs` makes for floats, and made here in the
/// direction that removes a normaliser instead of adding one.
const EXPECTED_ABSENT: [(&str, &str); 2] = [("Lit", "mantissa"), ("Lit", "scale")];

#[test]
fn every_field_of_every_parsed_ast_type_is_named_in_the_reference_dumper() {
    let ast = read("crates/ply-syntax/src/ast.rs");
    let dumper = read("spikes/ply-parser/harness/src/lib.rs");
    let fields = fields_of_ast(&ast);
    assert!(
        fields.len() > 100,
        "only {} fields found in ast.rs; the scanner has stopped working and this test \
         would pass over anything",
        fields.len()
    );

    let mut absent: Vec<(String, String)> = Vec::new();
    for (ty, f) in &fields {
        // A whole word, so `ret` does not match `return_clause`.
        let named = dumper
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|w| w == f);
        if !named {
            absent.push((ty.clone(), f.clone()));
        }
    }
    let expected: BTreeSet<(String, String)> = EXPECTED_ABSENT
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let got: BTreeSet<(String, String)> = absent.iter().cloned().collect();
    println!(
        "  {} fields across {} parsed types; {} deliberately not dumped",
        fields.len(),
        fields.iter().map(|(t, _)| t).collect::<BTreeSet<_>>().len(),
        got.len()
    );
    assert_eq!(
        got, expected,
        "a field of a parsed AST type is not named anywhere in the reference dumper. \
         The differential cannot see that: both sides would agree about a field neither \
         emits. Dump it, or add it to EXPECTED_ABSENT with the reason."
    );
}
