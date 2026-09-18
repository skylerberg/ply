use crate::unit::build::sp;
use ply_eval::handler::*;
use ply_span::Symbol;

#[test]
fn check_operation_accepts_an_effect_no_module_declares() {
    let effect = Symbol::new("mystery");
    let op = Symbol::new("go");
    assert!(check_operation(OpDecl::UnknownEffect, &effect, &op, false, sp()).is_ok());
}
