use ply_eval::decode::At;
use ply_eval::{Fields, Value};
use ply_span::Symbol;
use std::sync::Arc;

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    )))
}

#[test]
fn a_record_missing_a_field_names_the_path_to_it_and_the_fields_it_has() {
    let answer = Value::list(vec![record(vec![
        ("name", Value::bytes(b"m.f")),
        ("tables", record(vec![("calls", Value::list(Vec::new()))])),
    ])]);
    let first = At::new("the answer", &answer)
        .list()
        .unwrap()
        .next()
        .unwrap();
    assert_eq!(first.field("name").unwrap().utf8().unwrap(), "m.f");
    let err = first
        .field("tables")
        .unwrap()
        .field("consts")
        .expect_err("the tables have no `consts`");
    assert_eq!(err.path, "the answer[0].tables.consts");
    assert!(err.message.contains("`calls`"), "{err}");
    assert_eq!(err.to_string(), format!("{}: {}", err.path, err.message));
}

#[test]
fn a_value_of_another_kind_is_refused_where_it_is() {
    let answer = record(vec![("count", Value::str("three"))]);
    let count = At::new("the answer", &answer).field("count").unwrap();
    let err = count.int().unwrap_err();
    assert_eq!(err.path, "the answer.count");
    assert!(err.message.contains("expected an `Int`"), "{err}");
    assert!(count.utf8().is_err(), "a `String` is not `Bytes`");
    assert_eq!(count.str().unwrap(), "three");
    let err = At::new("the answer", &answer)
        .field("count")
        .unwrap()
        .field("n")
        .expect_err("a string has no fields");
    assert_eq!(err.path, "the answer.count");
}

#[test]
fn a_constructor_is_read_by_its_simple_name_whichever_program_named_it() {
    for name in ["Body", "emit.Body", "compiler.emit.Body"] {
        let answer = Value::ctor(name, vec![record(vec![("text", Value::bytes(b"f"))])]);
        let body = At::new("the answer", &answer).ctor().unwrap();
        assert_eq!(body.name(), "Body");
        let text = body.arg(0).unwrap().field("text").unwrap();
        assert_eq!(text.utf8().unwrap(), "f");
    }
    let answer = Value::list(vec![Value::ctor(
        "compiler.emit.Refused",
        vec![record(vec![("why", Value::Int(1))])],
    )]);
    let refused = At::new("the answer", &answer)
        .list()
        .unwrap()
        .next()
        .unwrap()
        .ctor()
        .unwrap();
    let err = refused
        .arg(0)
        .unwrap()
        .field("why")
        .unwrap()
        .utf8()
        .unwrap_err();
    assert_eq!(err.path, "the answer[0].Refused.why");
}

#[test]
fn an_unknown_constructor_and_a_missing_argument_are_named() {
    let answer = record(vec![(
        "kind",
        Value::ctor("items.SMaybe", vec![Value::Unit, Value::Unit]),
    )]);
    let kind = At::new("the answer", &answer)
        .field("kind")
        .unwrap()
        .ctor()
        .unwrap();
    let unknown = kind.unknown();
    assert_eq!(unknown.path, "the answer.kind");
    assert!(unknown.message.contains("`SMaybe`"), "{unknown}");
    let err = kind.arg(2).unwrap_err();
    assert!(err.message.contains("argument 2"), "{err}");
    let err = kind.arg(1).unwrap().int().unwrap_err();
    assert_eq!(err.path, "the answer.kind.SMaybe.1");
}

#[test]
fn an_option_a_result_and_a_number_are_read_or_refused() {
    let some = Value::ctor("Some", vec![Value::Int(7)]);
    let inner = At::new("a", &some).option().unwrap().unwrap();
    assert_eq!(inner.int().unwrap(), 7);
    assert!(
        At::new("a", &Value::ctor("None", Vec::new()))
            .option()
            .unwrap()
            .is_none()
    );
    assert!(At::new("a", &Value::Int(1)).option().is_err());
    assert!(
        At::new("a", &Value::ctor("Some", Vec::new()))
            .option()
            .is_err(),
        "a `Some` holding nothing is not an `Option`"
    );
    let failed = Value::ctor("Err", vec![Value::bytes(b"no")]);
    let refusal = At::new("a", &failed).result().unwrap().unwrap_err();
    assert_eq!(refusal.utf8().unwrap(), "no");
    let err = At::new("a", &Value::Int(-1)).number::<usize>().unwrap_err();
    assert!(err.message.contains("-1"), "{err}");
    assert_eq!(At::new("a", &Value::Int(9)).number::<u32>().unwrap(), 9);
    let short = Value::bytes([1u8, 2]);
    let err = At::new("a", &short).byte_array::<32>().unwrap_err();
    assert!(err.message.contains("32 bytes"), "{err}");
    assert!(At::new("a", &Value::bytes([0xffu8])).utf8().is_err());
    assert!(At::new("a", &Value::Bool(true)).bool().unwrap());
}

#[test]
fn a_map_value_is_named_by_its_key() {
    let answer = Value::map([(Value::bytes(b"k"), record(Vec::new()))]);
    let (key, value) = At::new("the answer", &answer)
        .entries()
        .unwrap()
        .next()
        .unwrap();
    assert_eq!(key.utf8().unwrap(), "k");
    let err = value.field("x").expect_err("the record is empty");
    assert!(err.path.starts_with("the answer["), "{err}");
    assert!(err.path.ends_with("].x"), "{err}");
    assert!(err.message.contains("no fields"), "{err}");
}
