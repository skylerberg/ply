use ply_codegen::opt::{Inlining, optimize, render};
use ply_codegen::source::Source;
use ply_syntax::ast::{Item, ModuleName, Program};

fn optimized(src: &str, name: &str) -> String {
    let src: &'static str = Box::leak(src.to_string().into_boxed_str());
    let mut sources = ply_span::SourceMap::new();
    let id = sources.add("m.ply", src);
    let mut program =
        ply_syntax::parse_program(vec![(id, ModuleName::from_dotted("m"), src)]).expect("parses");
    let resolved = ply_syntax::resolve::resolve(&mut program).expect("resolves");
    ply_codegen::c::producer::ensure_default();
    let front = ply_codegen::c::producer::front(&[("m".to_string(), src.to_string())], &[id])
        .expect("the port answers");
    assert!(
        front.diagnostics.is_empty(),
        "checks: {:?}",
        front.diagnostics
    );
    let check = front.check;
    let program: &'static Program = Box::leak(Box::new(program));
    let source = Source::new(
        program,
        Box::leak(Box::new(resolved)),
        Box::leak(Box::new(check)),
    );
    let def = program.modules[0]
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(def) if def.name.name.as_str() == name => Some(&**def),
            _ => None,
        })
        .expect("the function is defined");
    let mut text = String::new();
    render(
        &optimize(&source, 0, def, Inlining::IN_PROCESS),
        &mut text,
        1,
    );
    text
}

/// An inlined callee's block opens into the caller's, and then the record it answers is
/// split into its fields, so a body over small records is a body over scalars.
#[test]
fn an_inlined_callee_flattens_and_its_record_splits_into_scalars() {
    let text = optimized(
        "type Q = { a: Int, b: Int }\n\
         fn mask(x: Int) -> Int = x & 255\n\
         fn g(q: Q, m: Int) -> Q = { let a = mask(q.a + m); let b = mask(q.b + a); {a: a, b: b} }\n\
         fn round(p: Q) -> Int = { let c = g({a: p.a, b: p.b}, 3); let d = g({a: c.b, b: c.a}, 5); d.a + d.b }\n",
        "round",
    );
    assert!(
        !text.contains("= {\n"),
        "a let still binds a block:\n{text}"
    );
    assert!(!text.contains("{a:"), "a record literal survived:\n{text}");
    assert!(
        !text.contains("mask("),
        "a tiny leaf was left as a call:\n{text}"
    );
    assert!(
        !text.contains("g("),
        "the callee was left as a call:\n{text}"
    );
}

/// A count a callee named is the literal where it is used, and an operator over two
/// literals is its answer: a shift by `32 - n` with `n` known is a shift by a literal.
#[test]
fn a_literal_let_is_propagated_and_folded() {
    let text = optimized(
        "fn turn(x: Int, n: Int) -> Int = ((x >>> n) | (x << (32 - n))) & 255\n\
         fn twice(x: Int) -> Int = turn(turn(x, 7), 12)\n",
        "twice",
    );
    assert!(!text.contains("Sub"), "`32 - n` was not folded:\n{text}");
    assert!(
        text.contains("Shl Int(25)") && text.contains("Shl Int(20)"),
        "{text}"
    );
    assert!(!text.contains("let n"), "a literal let survived:\n{text}");
}
