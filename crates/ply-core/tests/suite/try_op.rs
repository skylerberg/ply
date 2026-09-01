//! What `e?` does to a check, at the source level.

use crate::fixture::compile;
use ply_core::{CheckOutput, print_scheme};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;

fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

#[track_caller]
fn only<'a>(diags: &'a [Diagnostic], code: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no `{code}` in {diags:#?}"))
}

fn scheme(out: &CheckOutput, name: &str) -> String {
    let key = Symbol::new(format!("m.{name}"));
    let d = out
        .defs
        .get(&key)
        .unwrap_or_else(|| panic!("no `{key}` in {:?}", out.defs.keys().collect::<Vec<_>>()));
    print_scheme(&d.scheme)
}

const DECLS: &str = "type E = {msg: String}\ntype F = {code: Int}\n\
                     fn g(n: Int) -> Result<Int, E> = Ok(n)\n\
                     fn h(n: Int) -> Option<Int> = Some(n)\n";

/// The sugar and the longhand infer the same scheme, including the same row, because inference is
/// looking at the same tree.
#[test]
fn a_try_and_its_longhand_infer_the_same_scheme() {
    let sugar = ok(&format!(
        "{DECLS}fn f(n: Int) -> Result<Int, E> = {{ let a = g(n)?; Ok(a + 1) }}"
    ));
    let longhand = ok(&format!(
        "{DECLS}fn f(n: Int) -> Result<Int, E> = \
         match g(n) {{ Err(er) -> Err(er), Ok(a) -> Ok(a + 1) }}"
    ));
    assert_eq!(scheme(&sugar, "f"), scheme(&longhand, "f"));
}

/// **The row is not a special case, and that is the point.**
#[test]
fn a_try_adds_nothing_to_the_row() {
    let src = "effect ctr { read now() -> Int }\n\
               type E = {msg: String}\n\
               fn g(n: Int) -> Result<Int, E> / {ctr.read} = Ok(n + ctr.now())\n";
    let sugar = ok(&format!(
        "{src}fn f(n: Int) -> Result<Int, E> / {{ctr.read}} = {{ let a = g(n)?; Ok(a) }}"
    ));
    let longhand = ok(&format!(
        "{src}fn f(n: Int) -> Result<Int, E> / {{ctr.read}} = \
         match g(n) {{ Err(er) -> Err(er), Ok(a) -> Ok(a) }}"
    ));
    assert_eq!(scheme(&sugar, "f"), scheme(&longhand, "f"));

    // And a `?` over a pure operand does not manufacture a row: `pure` is declared with none and
    // checks.
    ok(&format!(
        "{src}fn pure_one(n: Int) -> Result<Int, E> = {{ let a = Ok(n)?; Ok(a) }}"
    ));
}

/// `?` does **no** error conversion: there is no `From` in Ply and this does not invent one, so
/// binding a `Result<_, E>` inside a `-> Result<_, F>` function is an ordinary `E0201` and not a
/// code of its own.
#[test]
fn a_try_over_a_different_error_type_is_an_ordinary_type_mismatch() {
    let arg = format!(
        "{DECLS}fn use_it(a: Int, b: Int) -> Result<Int, F> = Ok(a + b)\n\
         fn wrong(n: Int) -> Result<Int, F> = use_it(1, g(n)?)"
    );
    let diags = errors(&arg);
    let d = only(&diags, codes::TYPE_MISMATCH);
    let spans: Vec<&str> = diags
        .iter()
        .filter(|d| d.code == codes::TYPE_MISMATCH)
        .filter_map(|d| d.labels.iter().find(|l| l.primary))
        .map(|l| &arg[l.span.range()])
        .collect();
    assert!(
        spans.contains(&"g(n)?"),
        "no `E0201` underlines the `?` itself; got {spans:?}\n{d:#?}"
    );

    // The block shape, against the longhand written out beside it.
    let sugar = format!("{DECLS}fn wrong(n: Int) -> Result<Int, F> = {{ let a = g(n)?; Ok(a) }}");
    let longhand = format!(
        "{DECLS}fn wrong(n: Int) -> Result<Int, F> = \
         {{ match g(n) {{ Err(er) -> Err(er), Ok(a) -> Ok(a) }} }}"
    );
    let s = errors(&sugar);
    let l = errors(&longhand);
    assert_eq!(
        only(&s, codes::TYPE_MISMATCH).message,
        only(&l, codes::TYPE_MISMATCH).message,
        "`?` should fail exactly the way its longhand fails"
    );
}

/// `Option` is served too, and it is a different pair of constructors rather than a different
/// operator.
#[test]
fn an_option_try_infers_as_its_longhand() {
    let sugar = ok(&format!(
        "{DECLS}fn f(n: Int) -> Option<Int> = {{ let a = h(n)?; Some(a + 1) }}"
    ));
    let longhand = ok(&format!(
        "{DECLS}fn f(n: Int) -> Option<Int> = \
         match h(n) {{ None -> None, Some(a) -> Some(a + 1) }}"
    ));
    assert_eq!(scheme(&sugar, "f"), scheme(&longhand, "f"));
}

/// The mode is read from **this file**: a return type named through another module is refused
/// rather than resolved across the boundary, for the reason `record_update` gives — gate 1 skips a
/// file whose raw bytes are unchanged, so a meaning read across a boundary could go stale in a file
/// that never moved.
#[test]
fn a_return_type_named_through_another_module_refuses() {
    let inputs = vec![
        (
            SourceId(0),
            ModuleName::from_dotted("lib"),
            "pub type R = Result<Int, Int>\npub fn zero() -> R = Ok(0)\n",
        ),
        (
            SourceId(1),
            ModuleName::from_dotted("app"),
            "import lib\nfn f() -> lib::R = Ok(lib::zero()?)\n",
        ),
    ];
    let diags = ply_syntax::parse_program(inputs).expect_err("`?` has no mode here");
    let d = only(&diags, codes::TRY_SCOPE);
    assert!(
        d.notes.iter().any(|n| n.contains("module boundary")),
        "{d:#?}"
    );
}

/// The fixtures `tests/fixtures/` owes for the two new codes, checked here rather than left as
/// files nothing reads.
#[test]
fn the_fixtures_produce_the_codes_they_are_named_for() {
    for (path, code) in [
        ("try_scope.ply", codes::TRY_SCOPE),
        ("try_position.ply", codes::TRY_POSITION),
    ] {
        let full = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/").to_string() + path;
        let source =
            std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("could not read {full}: {e}"));
        let diags = errors(&source);
        only(&diags, code);
    }
}
