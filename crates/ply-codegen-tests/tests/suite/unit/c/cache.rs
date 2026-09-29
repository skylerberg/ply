use ply_codegen::c::cache::{decode, encode, file_digest};
use ply_codegen::c::tables::{Defined, Tables};
use ply_eval::Value;
use ply_span::Symbol;

/// The binary a body is keyed under is identified by its bytes: a build that writes the same ones
/// keeps the cache it filled, and a change of the same length moves it.
#[test]
fn a_binary_is_identified_by_its_bytes_and_not_by_its_timestamp() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("ply");
    std::fs::write(&path, b"a compiled body\n").expect("the binary is written");
    let first = file_digest(&path).expect("the binary has an identity");

    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("the binary opens");
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .expect("its modification time moves");
    assert_eq!(
        file_digest(&path).as_deref(),
        Some(first.as_str()),
        "the same bytes were given two identities"
    );

    std::fs::write(&path, b"a compiled bodX\n").expect("the binary is rewritten");
    assert_ne!(
        file_digest(&path).as_deref(),
        Some(first.as_str()),
        "a changed byte kept its identity"
    );

    assert_eq!(file_digest(&dir.path().join("absent")), None);
}

/// Table names can be spelled anything, `text` included, so the text's start is an offset rather than a marker.
#[test]
fn a_body_round_trips_through_the_encoding() {
    let mut t = Tables::default();
    t.consts.push(Value::Unit);
    t.consts.push(Value::str("hello\nworld"));
    t.consts.push(Value::bytes([0u8, 255, 10]));
    t.consts.push(Value::Fixed(ply_eval::Fixed::new(
        ply_eval::IntTy::I8,
        -3i64 as u64,
    )));
    t.consts.push(Value::Float(-1.5));
    t.consts.push(Value::Decimal("2.50".parse().unwrap()));
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
    // Rendered, so a decimal's scale is compared as well as its value.
    assert_eq!(format!("{:?}", out.consts), format!("{:?}", t.consts));
}
