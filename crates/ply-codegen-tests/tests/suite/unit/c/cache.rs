use ply_codegen::c::cache::{decode, encode};
use ply_codegen::c::tables::{Defined, Tables};
use ply_eval::Value;
use ply_span::Symbol;

/// Table names can be spelled anything, `text` included, so the text's start is an offset rather than a marker.
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
    t.members.push("m.f".to_string());
    t.members.push("m.g".to_string());
    for symbol in ["ply_m_1f_1", "ply_m_1g_1"] {
        t.symbols.push(Defined {
            symbol: symbol.to_string(),
            entry: format!("{symbol}_entry"),
        });
    }
    let text = "Word f(void) {\n  return @@c1@@;\n}\ntext\n";

    let (back, out) = decode(&encode(text, &t)).expect("the encoding round trips");
    assert_eq!(back, text, "the text came back changed");
    assert_eq!(out.fields, t.fields);
    assert_eq!(out.shapes, t.shapes);
    assert_eq!(out.calls, t.calls);
    assert_eq!(out.lambdas, t.lambdas);
    assert_eq!(out.builtins, t.builtins);
    assert_eq!(out.members, t.members);
    assert_eq!(out.symbols, t.symbols);
    assert_eq!(out.consts.len(), 3);
    assert!(matches!(out.consts[0], Value::Unit));
    assert_eq!(
        format!("{:?}", out.consts[1]),
        format!("{:?}", Value::str("hello\nworld"))
    );
}

/// The committed emitter stages the sources of one that writes a members table and a symbols
/// table; its frames have neither.
#[test]
fn a_body_framed_without_a_members_or_symbols_table_decodes_as_its_own() {
    let mut t = Tables::default();
    t.calls.push("m.g".to_string());
    let text = "Word f(void) {\n  return 0;\n}\n";
    let encoded = encode(text, &t)
        .replace("members 0\n", "")
        .replace("symbols 0\n", "");
    assert!(
        !encoded.contains("members") && !encoded.contains("symbols"),
        "{encoded}"
    );
    let (back, out) = decode(&encoded).expect("a frame without members or symbols decodes");
    assert_eq!(back, text);
    assert_eq!(out.calls, t.calls);
    assert!(out.members.is_empty() && out.symbols.is_empty());
}
