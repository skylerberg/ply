//! Temporary: the compiler's constant roots against the Rust computation they replace.

use ply_span::Symbol;
use ply_ty::Front;
use std::collections::BTreeSet;

fn old(front: &Front) -> BTreeSet<Symbol> {
    front
        .emitter_roots
        .iter()
        .filter(|r| r.arity == 0)
        .filter(|r| {
            front
                .check
                .defs
                .get(&r.root)
                .is_some_and(|d| d.footprint.is_empty() && d.constraints.is_empty())
        })
        .map(|r| r.root.clone())
        .collect()
}

const SRC: &str = r#"
fn pure_zero() -> Int = 1
fn takes(x: Int) -> Int = x
pub effect log { write emit(Bytes) -> Unit }
fn effectful() -> Unit / { log.write } = log.emit(b"x")
pub fn needs_eq<a>() -> Int where derivable(eq, a) = 1
fn uses_const() -> Int = pure_zero()
"#;

#[test]
fn the_compilers_constants_are_the_ones_rust_derived() {
    let id = ply_span::SourceId(0);
    let front =
        ply_codegen::c::producer::checked_front(&[("m".to_string(), SRC.to_string())], &[id])
            .expect("checks");
    assert_eq!(front.emitter_constants, old(&front));
    assert!(
        front
            .emitter_constants
            .contains(&Symbol::new("m.pure_zero"))
    );
    assert!(
        !front
            .emitter_constants
            .contains(&Symbol::new("m.effectful"))
    );
}
