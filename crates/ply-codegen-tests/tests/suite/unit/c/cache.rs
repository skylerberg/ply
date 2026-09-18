use ply_codegen::c::cache::{decode, encode};
use ply_codegen::c::tables::Tables;
use ply_eval::Value;
use ply_span::Symbol;

/// A body goes to disk and comes back the same, tables and all.
///
/// The tables hold names a program chose, so they can be spelled anything -- `text` included,
/// which is what a marker-terminated format gets wrong and why the text's start is an offset.
#[test]
fn a_body_round_trips_through_the_encoding() {
    let mut t = Tables::default();
    t.consts.push(Value::Unit);
    t.consts.push(Value::str("hello\nworld"));
    t.consts.push(Value::bytes([0u8, 255, 10]));
    t.builtins.push(ply_eval::Builtin::BytesLen);
    t.fields.push(Symbol::new("text"));
    t.shapes.push(vec![Symbol::new("text"), Symbol::new("b")]);
    t.calls.push("text".to_string());
    t.lambdas.push("ply_m_f_lambda0".to_string());
    let text = "Word f(void) {\n  return @@c1@@;\n}\ntext\n";

    let (back, out) = decode(&encode(text, &t)).expect("the encoding round trips");
    assert_eq!(back, text, "the text came back changed");
    assert_eq!(out.fields, t.fields);
    assert_eq!(out.shapes, t.shapes);
    assert_eq!(out.calls, t.calls);
    assert_eq!(out.lambdas, t.lambdas);
    assert_eq!(out.builtins, t.builtins);
    assert_eq!(out.consts.len(), 3);
    assert!(matches!(out.consts[0], Value::Unit));
    assert_eq!(
        format!("{:?}", out.consts[1]),
        format!("{:?}", Value::str("hello\nworld"))
    );
}
