//! The one thing the differential structurally cannot see.

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

/// Every line with its comments removed: a `//` or `///` line becomes empty and a trailing `// ...`
/// is cut.
fn code_only(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `Type::field` for every named field of every struct and struct-like enum variant in `ast.rs`,
/// skipping the types the parser never builds.
fn fields_of_ast(src: &str) -> BTreeSet<(String, String)> {
    // Types that are not part of a parsed tree: module identity, the whole-program wrapper, and
    // `Derived`, which only `derive` expansion in a later crate fills in.
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
const EXPECTED_ABSENT: [(&str, &str); 2] = [("Lit", "mantissa"), ("Lit", "scale")];

#[test]
fn every_field_of_every_parsed_ast_type_is_named_in_the_reference_dumper() {
    let ast = code_only(&read("crates/ply-syntax/src/ast.rs"));
    let dumper = code_only(&read("spikes/ply-parser/harness/src/lib.rs"));
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
        "a field of a parsed AST type is not named anywhere in the reference dumper's \
         CODE (comments are stripped before this match; see the header). The differential \
         cannot see that: both sides would agree about a field neither emits. Dump it, or \
         add it to EXPECTED_ABSENT with the reason."
    );
}

/// The repair itself, armed.
#[test]
fn a_field_named_only_in_a_comment_does_not_count_as_covered() {
    let code = code_only(
        "/// one named from another module contributes an empty expansion\n\
         fn f() { let x = 1; } // and a default lives in the callee's module\n\
         self.opt(p.ty.as_ref(), Self::ty);\n",
    );
    let words: Vec<&str> = code
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .collect();
    assert!(
        !words.contains(&"named"),
        "the `///` line survived stripping: {code:?}"
    );
    assert!(
        !words.contains(&"default"),
        "the trailing `//` survived stripping: {code:?}"
    );
    assert!(
        words.contains(&"ty"),
        "stripping ate the code as well: {code:?}"
    );
}
