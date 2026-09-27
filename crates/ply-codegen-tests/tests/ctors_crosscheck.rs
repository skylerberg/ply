//! Temporary: the compiler's emitter constructors against the Rust derivation they replace.

use ply_span::Symbol;
use ply_ty::Front;
use std::collections::HashSet;

fn ctors_of(front: &Front) -> Vec<(Symbol, usize)> {
    let mut out: Vec<(Symbol, usize)> = front
        .check
        .ctors
        .iter()
        .filter(|(_, c)| c.module.is_anonymous())
        .map(|(name, c)| (name.clone(), c.arity))
        .collect();
    let prelude: HashSet<Symbol> = out.iter().map(|(name, _)| name.clone()).collect();
    for (module, _) in &front.ordinals {
        out.extend(
            front
                .check
                .ctors
                .iter()
                .filter(|(name, c)| c.module.as_symbol() == module && !prelude.contains(*name))
                .map(|(name, c)| (name.clone(), c.arity)),
        );
    }
    out
}

const SRC: &str = r#"
type Shape = | Dot | Line(Int)
pub type Pair = { a: Int, b: Bool }
type Level = Debug | Info | Warn | Error
fn f() -> Int = 1
"#;

#[test]
fn the_compilers_emitter_ctors_are_the_ones_rust_derived() {
    let id = ply_span::SourceId(0);
    let front =
        ply_codegen::c::producer::checked_front(&[("m".to_string(), SRC.to_string())], &[id])
            .expect("checks");
    assert_eq!(front.emitter_ctors, ctors_of(&front));
    // The prelude's come first, so a program's tags start past them.
    assert!(
        front
            .emitter_ctors
            .iter()
            .any(|(n, _)| n.as_str() == "Some")
    );
    assert!(
        front
            .emitter_ctors
            .iter()
            .any(|(n, _)| n.as_str() == "m.Dot")
    );
}
