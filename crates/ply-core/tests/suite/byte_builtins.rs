//! The byte builtins' type surface, at the source level.
//!
//! `ply-core`'s own unit test pins the printed schemes. This asks the two
//! questions a pin cannot: that eight of the nine are **pure**, and that
//! `bytes_position` — the one that calls back into user code — threads its
//! predicate's row into the caller's footprint. A builtin that swallowed the
//! row would let a definition reaching a socket publish an empty one, which is
//! the failure ADR 0012 calls a green result over unexplored space.

use ply_core::{CheckOutput, check_program, print_type};
use ply_span::{Diagnostic, SourceId, Symbol};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_type(&out.defs[&Symbol::new(format!("m.{name}"))].scheme.ty)
}

fn footprint(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(format!("m.{name}"))]
        .footprint
        .to_string()
}

#[test]
fn the_byte_builtins_have_the_types_the_contract_states() {
    // The middle column is the probe's generic list: only `bytes_position`'s
    // type names a row variable, so only it binds one.
    let expected = [
        ("bytes_index_of", "", "(Bytes, Bytes) -> Option<Int>"),
        (
            "bytes_index_of_from",
            "",
            "(Bytes, Bytes, Int) -> Option<Int>",
        ),
        ("bytes_index_of_byte", "", "(Bytes, Int) -> Option<Int>"),
        ("bytes_starts_with", "", "(Bytes, Bytes) -> Bool"),
        ("bytes_ends_with", "", "(Bytes, Bytes) -> Bool"),
        ("bytes_split", "", "(Bytes, Bytes) -> List<Bytes>"),
        ("bytes_scan", "", "(Bytes, Int, Bytes, Int) -> Int"),
        ("bytes_scan_until", "", "(Bytes, Int, Bytes, Int) -> Int"),
        (
            "bytes_position",
            "<| e>",
            "(Bytes, Int, (Int) -> Bool / e) -> Option<Int> / e",
        ),
    ];
    let source: String = expected
        .iter()
        .map(|(name, generics, ty)| format!("fn probe_{name}{generics}() -> {ty} = {name}\n"))
        .collect();
    let out = ok(&source);
    for (name, _, ty) in expected {
        // The probe now *writes* the contract's type rather than reading the
        // builtin's off inference (`MISSING_SIGNATURE`), which makes this
        // strictly stronger: the builtin has to unify with the type the
        // contract names, not merely print the same way it does.
        assert_eq!(
            sig(&out, &format!("probe_{name}")),
            format!("() -> {ty}"),
            "{name}"
        );
    }
}

/// Eight of the nine perform nothing, so a parser written with them has an
/// empty row — which is what lets `examples/hello.ply` keep its head parser out
/// of the trusted computing base and still publish `{}`.
#[test]
fn every_builtin_but_position_is_pure() {
    let out = ok(r#"
fn parse(head: Bytes) -> Int =
  if bytes_starts_with(head, b"GET ") && !bytes_ends_with(head, b"\n") {
    len(bytes_split(head, b"\r\n"))
  } else {
    match bytes_index_of(head, b"\r\n\r\n") {
      Some(at) -> at + bytes_scan(head, 0, b"GET", bytes_len(head)),
      None -> match bytes_index_of_from(head, b" ", 0) {
        Some(sp) -> bytes_scan_until(head, sp, b"\r", 64),
        None -> match bytes_index_of_byte(head, 10) {
          Some(lf) -> lf,
          None -> -1,
        },
      },
    }
  }
"#);
    assert_eq!(footprint(&out, "parse"), "{}");
}

/// `bytes_position` calls user code, so its row is the predicate's. A caller
/// that hands it a predicate performing `net.write[conn]` publishes exactly
/// that, and one that hands it a pure predicate publishes nothing.
#[test]
fn position_threads_its_predicates_row_and_nothing_more() {
    let out = ok(r#"
effect audit {
  write note[trail](byte: Int) -> Bool
}

fn first_noted(b: Bytes) -> Option<Int> =
  bytes_position(b, 0, |x| audit.note[trail](x))

fn first_space(b: Bytes) -> Option<Int> =
  bytes_position(b, 0, |x| x == 32)
"#);
    assert_eq!(footprint(&out, "first_noted"), "{m.audit.write[trail]}");
    assert_eq!(footprint(&out, "first_space"), "{}");
    assert_eq!(
        sig(&out, "first_noted"),
        "(Bytes) -> Option<Int> / {m.audit.write[trail]}"
    );
}

/// The declared row is checked as an upper bound like any other, so a
/// definition that claims purity and hands `bytes_position` an effectful
/// predicate is refused rather than believed.
#[test]
fn a_declared_empty_row_refuses_an_effectful_predicate() {
    let d = compile(
        r#"
effect audit {
  write note[trail](byte: Int) -> Bool
}

fn first_noted(b: Bytes) -> Option<Int> / {} =
  bytes_position(b, 0, |x| audit.note[trail](x))
"#,
    )
    .expect_err("a pure annotation over an effectful predicate must be refused");
    assert_eq!(d[0].code, ply_span::codes::EFFECT_NOT_PERMITTED, "{d:#?}");
}
