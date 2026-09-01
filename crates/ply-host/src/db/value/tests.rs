use super::*;
use ply_eval::Value;
use rust_decimal::Decimal;
use std::str::FromStr;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).expect("a decimal")
}

/// A `std.db` constructor as a `Value` carries one: **qualified**.
fn ctor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(super::ctor(crate::db::MODULE, name), args)
}

fn json_ctor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(super::ctor(JSON_MODULE, name), args)
}

#[test]
fn every_param_constructor_decodes_to_its_wire_type() {
    let cases = [
        (ctor("PNull", vec![]), Param::Null),
        (ctor("PInt", vec![Value::Int(7)]), Param::Int(7)),
        (ctor("PBool", vec![Value::Bool(true)]), Param::Bool(true)),
        (
            ctor("PText", vec![Value::str("a")]),
            Param::Text("a".into()),
        ),
        (
            ctor("PBytes", vec![Value::bytes([1u8, 2])]),
            Param::Bytes(vec![1, 2]),
        ),
        (ctor("PFloat", vec![Value::Float(1.5)]), Param::Float(1.5)),
        (
            ctor("PNumeric", vec![Value::Decimal(dec("1.25"))]),
            Param::Numeric(dec("1.25")),
        ),
        (
            ctor(
                "PArray",
                vec![Value::list(vec![ctor("PInt", vec![Value::Int(1)])])],
            ),
            Param::Array(vec![Param::Int(1)]),
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(param(&value, Span::DUMMY).expect("decodes"), expected);
    }
}

/// Inference checks a perform's argument types, so a constructor `std.db` does not declare cannot
/// arrive.
#[test]
fn a_param_the_declaration_does_not_have_is_ply_s_fault() {
    let d = param(&ctor("PTimestamp", vec![Value::Int(0)]), Span::DUMMY)
        .expect_err("no such constructor");
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    let d = param(&Value::Int(1), Span::DUMMY).expect_err("not a constructor at all");
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    let d = param(&ctor("PInt", vec![]), Span::DUMMY).expect_err("no argument");
    assert_eq!(d.code, codes::INTERNAL_ERROR);
}

#[test]
fn a_statement_is_its_sql_field_and_nothing_else() {
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("sql"), Value::str("select 1"));
    let stmt = Value::Record(Arc::new(fields.into_iter().collect()));
    assert_eq!(statement(&stmt, Span::DUMMY).expect("decodes"), "select 1");

    assert_eq!(
        statement(
            &Value::Record(Arc::new(ply_eval::Fields::default())),
            Span::DUMMY
        )
        .expect_err("no `sql`")
        .code,
        codes::INTERNAL_ERROR
    );
    assert_eq!(
        statement(&Value::str("select 1"), Span::DUMMY)
            .expect_err("not a record")
            .code,
        codes::INTERNAL_ERROR
    );
}

#[test]
fn json_crosses_the_boundary_in_both_directions() {
    let document = Json::Object(vec![
        ("a".into(), Json::Number(dec("1.2500"))),
        ("b".into(), Json::Array(vec![Json::Bool(true), Json::Null])),
        ("c".into(), Json::Str("x".into())),
    ]);
    let value = json_value(&document);
    let back = json(&value, Span::DUMMY).expect("decodes");
    assert_eq!(back, document);
    // A `Map` canonicalises key order, so the way back out is sorted rather than the order the
    // document had.
    assert_eq!(back.render(), document.render());
}

/// A `Json` built here has to carry `std.json`'s own constructor names, not `std.db`'s and not bare
/// ones: a `Value::Ctor`'s identity is its program-wide name, and a document spelled `Str` rather
/// than `std.json.Str` is one no `match` in any program can take apart.
#[test]
fn a_json_document_carries_the_module_that_declares_it() {
    match &json_value(&Json::Str("x".into())) {
        Value::Ctor { name, .. } => assert_eq!(name.as_str(), "std.json.Str"),
        other => panic!("{}", other.type_name()),
    }
    let written = json_ctor(
        "Object",
        vec![Value::map([(
            Value::str("a"),
            json_ctor("Number", vec![Value::Decimal(dec("2"))]),
        )])],
    );
    assert_eq!(
        json(&written, Span::DUMMY).expect("decodes"),
        Json::Object(vec![("a".into(), Json::Number(dec("2")))])
    );
    // A `db` constructor of the same simple name is not a `Json` one: the qualifier is what keeps
    // two modules' `Array`s apart.
    assert!(json(&ctor("Array", vec![Value::list(Vec::new())]), Span::DUMMY).is_err());
}

#[test]
fn a_row_is_a_map_so_two_column_orders_are_one_value() {
    let forward = row(&vec![
        ("a".to_string(), Datum::Int(1)),
        ("b".to_string(), Datum::Int(2)),
    ]);
    let backward = row(&vec![
        ("b".to_string(), Datum::Int(2)),
        ("a".to_string(), Datum::Int(1)),
    ]);
    assert_eq!(forward, backward);
}

/// The name a `Value::Ctor` must carry: `std.db`'s own, qualified.
fn qualified(name: &str) -> String {
    format!("{}.{name}", crate::db::MODULE)
}

#[test]
fn every_datum_becomes_the_constructor_the_declaration_names() {
    let cases = [
        (Datum::Null, "CNull"),
        (Datum::Int(1), "CInt"),
        (Datum::Bool(true), "CBool"),
        (Datum::Text("a".into()), "CText"),
        (Datum::Bytes(vec![1]), "CBytes"),
        (Datum::Float(1.0), "CFloat"),
        (Datum::Numeric(dec("1")), "CNumeric"),
        (Datum::Json(Json::Null), "CJson"),
        (Datum::Array(vec![Datum::Int(1)]), "CArray"),
    ];
    for (value, name) in cases {
        match &datum(&value) {
            Value::Ctor { name: got, .. } => assert_eq!(got.as_str(), qualified(name)),
            other => panic!("`{name}` became {}", other.type_name()),
        }
    }
}

#[test]
fn an_answer_is_one_of_three_shapes() {
    match &answer(&Answer::Count(3)) {
        Value::Ctor { name, args } => {
            assert_eq!(name.as_str(), qualified("Count"));
            assert_eq!(args.len(), 1);
        }
        other => panic!("{}", other.type_name()),
    }
    match &answer(&Answer::Failed(DbError::new(
        "23505",
        "part_pkey",
        "duplicate",
    ))) {
        Value::Ctor { name, args } => {
            assert_eq!(name.as_str(), qualified("Failed"));
            match &args[0] {
                Value::Record(fields) => {
                    assert_eq!(fields.get(&Symbol::new("code")), Some(&Value::str("23505")));
                    assert_eq!(
                        fields.get(&Symbol::new("constraint")),
                        Some(&Value::str("part_pkey"))
                    );
                    // The message is carried for a person and compared by nothing.
                    assert!(fields.contains_key(&Symbol::new("detail")));
                }
                other => panic!("{}", other.type_name()),
            }
        }
        other => panic!("{}", other.type_name()),
    }
    match &answer(&Answer::Rows(vec![vec![("a".into(), Datum::Int(1))]])) {
        Value::Ctor { name, .. } => assert_eq!(name.as_str(), qualified("Rows")),
        other => panic!("{}", other.type_name()),
    }
}

#[test]
fn isolation_and_access_decode_to_the_levels_the_scope_table_names() {
    use crate::db::scope::{Access, Isolation};
    assert_eq!(
        isolation(&ctor("ReadCommitted", vec![]), Span::DUMMY).expect("decodes"),
        Isolation::ReadCommitted
    );
    assert_eq!(
        isolation(&ctor("Serializable", vec![]), Span::DUMMY).expect("decodes"),
        Isolation::Serializable
    );
    assert_eq!(
        isolation(&ctor("RepeatableRead", vec![]), Span::DUMMY).expect("decodes"),
        Isolation::RepeatableRead
    );
    // `ReadUncommitted` is not offered, because postgres implements it as read committed and a name
    // that promised dirty reads would be a name that lies.
    assert!(isolation(&ctor("ReadUncommitted", vec![]), Span::DUMMY).is_err());
    assert_eq!(
        access(&ctor("ReadOnly", vec![]), Span::DUMMY).expect("decodes"),
        Access::ReadOnly
    );
    assert_eq!(
        access(&ctor("ReadWrite", vec![]), Span::DUMMY).expect("decodes"),
        Access::ReadWrite
    );
}
