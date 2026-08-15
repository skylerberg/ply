use super::*;
use ply_span::Span;
use std::str::FromStr;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).expect("a decimal")
}

fn bound(param: Param, ty: Type) -> Result<Bound, BindError> {
    bind_one(&param, &ty, 1, Span::DUMMY)
}

fn refused(param: Param, ty: Type) -> Diagnostic {
    match bound(param, ty) {
        Err(BindError::Refused(d)) => d,
        Err(BindError::Failed(e)) => panic!("expected a refusal, got `Failed` {}", e.code),
        Ok(_) => panic!("expected a refusal"),
    }
}

fn failed(param: Param, ty: Type) -> DbError {
    match bound(param, ty) {
        Err(BindError::Failed(e)) => e,
        Err(BindError::Refused(d)) => panic!("expected a `Failed`, got {}: {}", d.code, d.message),
        Ok(_) => panic!("expected a `Failed`"),
    }
}

#[test]
fn every_row_of_the_pinned_mapping_binds() {
    for (param, ty) in [
        (Param::Int(7), Type::INT8),
        (Param::Int(7), Type::INT4),
        (Param::Int(7), Type::INT2),
        (Param::Bool(true), Type::BOOL),
        (Param::Text("a".into()), Type::TEXT),
        (Param::Text("a".into()), Type::VARCHAR),
        (Param::Text("a".into()), Type::BPCHAR),
        (Param::Text("a".into()), Type::NAME),
        (
            Param::Text("6ba7b810-9dad-11d1-80b4-00c04fd430c8".into()),
            Type::UUID,
        ),
        (Param::Bytes(vec![0, 1, 255]), Type::BYTEA),
        (Param::Float(1.5), Type::FLOAT8),
        (Param::Numeric(dec("1.2345")), Type::NUMERIC),
        (Param::Json(Json::Null), Type::JSONB),
        (Param::Json(Json::Null), Type::JSON),
        (Param::Array(vec![Param::Int(1)]), Type::INT8_ARRAY),
        // `Null` fits every column, which is what makes `Option<a>` a nullable
        // column of `a` rather than a type of its own.
        (Param::Null, Type::INT8),
        (Param::Null, Type::JSONB),
        (Param::Null, Type::TEXT_ARRAY),
    ] {
        assert!(
            bound(param.clone(), ty.clone()).is_ok(),
            "{} into `{ty}` was refused",
            param.what()
        );
    }
}

/// An `Int` narrows to nothing. The column decides the width and a value that
/// does not fit is the server's own `22003`, never a truncation the program
/// cannot see.
#[test]
fn an_int_that_does_not_fit_its_column_is_a_failure_and_never_a_truncation() {
    assert_eq!(failed(Param::Int(i64::MAX), Type::INT4).code, "22003");
    assert_eq!(failed(Param::Int(70_000), Type::INT2).code, "22003");
    assert!(bound(Param::Int(2_147_483_647), Type::INT4).is_ok());
    assert!(bound(Param::Int(-32_768), Type::INT2).is_ok());
}

#[test]
fn a_parameter_outside_the_mapping_names_the_type_it_was_going_to_be_sent_as() {
    let d = refused(Param::Text("x".into()), Type::INT8);
    assert_eq!(d.code, codes::DB_STATEMENT_REFUSED);
    assert!(d.message.contains("`PText`"), "{}", d.message);
    assert!(d.message.contains("int8"), "{}", d.message);

    // No time type in Ply, so a column of one is refused rather than rendered
    // to text — with the workaround named, because "unsupported" alone is a
    // dead end.
    let d = refused(Param::Int(0), Type::TIMESTAMPTZ);
    assert!(
        d.notes.iter().any(|n| n.contains("microseconds")),
        "{:?}",
        d.notes
    );
    refused(Param::Int(0), Type::DATE);
    refused(Param::Int(0), Type::INTERVAL);
}

/// §4.2's table maps `Float` to `float8` as a **parameter** and to `float4` or
/// `float8` only as a *result*, so a `float4` parameter is outside the mapping.
/// Narrowing it here would store `1e300` as `Infinity` and `0.1234567890123` as
/// `0.12345679` — a rounding the program never asked for and cannot see.
#[test]
fn a_float4_parameter_is_refused_rather_than_narrowed() {
    for value in [1.5, 1e300, 0.1234567890123] {
        let d = refused(Param::Float(value), Type::FLOAT4);
        assert_eq!(d.code, codes::DB_STATEMENT_REFUSED);
        assert!(d.message.contains("float4"), "{}", d.message);
        assert!(
            d.notes.iter().any(|n| n.contains("float8::float4")),
            "the refusal names the narrowing a program can write for itself: {:?}",
            d.notes
        );
    }
    // The result direction is unchanged: a `float4` column still decodes.
    assert!(mapped(&Type::FLOAT4));
}

#[test]
fn an_array_is_one_dimensional_and_holds_no_null() {
    let d = refused(
        Param::Array(vec![Param::Int(1), Param::Null]),
        Type::INT8_ARRAY,
    );
    assert!(d.message.contains("index 1"), "{}", d.message);

    let d = refused(
        Param::Array(vec![Param::Array(vec![Param::Int(1)])]),
        Type::INT8_ARRAY,
    );
    assert!(d.message.contains("nested"), "{}", d.message);

    // Empty is legal and takes its element type from the description.
    assert!(bound(Param::Array(Vec::new()), Type::TEXT_ARRAY).is_ok());
    // Heterogeneous is refused at the element that does not fit.
    refused(
        Param::Array(vec![Param::Int(1), Param::Text("x".into())]),
        Type::INT8_ARRAY,
    );
}

#[test]
fn a_parameter_list_of_the_wrong_length_is_refused_before_anything_is_sent() {
    let d = match bind(&[Param::Int(1)], &[Type::INT8, Type::INT8], Span::DUMMY) {
        Err(BindError::Refused(d)) => d,
        _ => panic!("expected a refusal"),
    };
    assert!(d.message.contains("takes 2"), "{}", d.message);
}

#[test]
fn a_uuid_parameter_is_parsed_rather_than_sent_as_text() {
    assert!(
        bound(
            Param::Text("6ba7b810-9dad-11d1-80b4-00c04fd430c8".into()),
            Type::UUID
        )
        .is_ok()
    );
    assert_eq!(
        failed(Param::Text("not-a-uuid".into()), Type::UUID).code,
        "22P02"
    );
    assert_eq!(
        parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("a uuid")[0],
        0x6b
    );
    assert_eq!(
        parse_uuid("6ba7b8109dad11d180b400c04fd430c8"),
        parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
    );
    assert!(parse_uuid("6ba7b810").is_none());
    assert!(parse_uuid("zzzzzzzz-9dad-11d1-80b4-00c04fd430c8").is_none());
}

#[test]
fn the_mapping_admits_exactly_what_it_says_it_does() {
    for ty in [
        Type::BOOL,
        Type::INT2,
        Type::INT4,
        Type::INT8,
        Type::FLOAT4,
        Type::FLOAT8,
        Type::TEXT,
        Type::VARCHAR,
        Type::BPCHAR,
        Type::NAME,
        Type::UUID,
        Type::BYTEA,
        Type::NUMERIC,
        Type::JSON,
        Type::JSONB,
        Type::INT8_ARRAY,
        Type::TEXT_ARRAY,
    ] {
        assert!(mapped(&ty), "`{ty}` should be mapped");
    }
    for ty in [
        Type::TIMESTAMPTZ,
        Type::TIMESTAMP,
        Type::DATE,
        Type::TIME,
        Type::INTERVAL,
        Type::INET,
        Type::POINT,
        Type::XML,
        Type::MONEY,
    ] {
        assert!(!mapped(&ty), "`{ty}` should not be mapped");
    }
}

// --- json -------------------------------------------------------------------

fn parsed(text: &str) -> Json {
    Json::parse(text.as_bytes(), "a test").unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

#[test]
fn json_round_trips_through_its_canonical_text() {
    for text in [
        "null",
        "true",
        "false",
        "0",
        "-1",
        "1.2500",
        "\"\"",
        "\"a\\nb\"",
        "[]",
        "[1,2,3]",
        "{}",
        "{\"a\":1,\"b\":[true,null]}",
        "{\"nested\":{\"deep\":{\"x\":\"y\"}}}",
    ] {
        let value = parsed(text);
        assert_eq!(value.render(), text, "{text}");
        assert_eq!(parsed(&value.render()), value, "{text}");
    }
}

/// The same rule `std.json` states, and the reason `Number` is a `Decimal`: a
/// scale that quietly moved is a total that quietly lost a cent.
#[test]
fn a_json_number_keeps_the_scale_it_was_written_with() {
    assert_eq!(parsed("1.2500"), Json::Number(dec("1.2500")));
    assert_eq!(parsed("1.2500").render(), "1.2500");
    assert_ne!(parsed("1.25").render(), parsed("1.2500").render());
}

#[test]
fn json_outside_the_strict_grammar_is_refused() {
    for text in [
        "",
        "{",
        "[1,]",
        "{a:1}",
        "{\"a\":}",
        "'a'",
        "NaN",
        "Infinity",
        "1 2",
        "\"unterminated",
        "\"\\q\"",
        "[1,2",
        "{\"a\" 1}",
        "\"\u{1}\"",
        // Past `Decimal`'s range: a decode failure naming the offset, never a
        // value that rounded.
        "123456789012345678901234567890123456789",
    ] {
        assert!(
            Json::parse(text.as_bytes(), "a test").is_err(),
            "`{text}` was accepted"
        );
    }
    assert!(Json::parse(&[0xff, 0xfe], "a test").is_err());
}

#[test]
fn a_surrogate_pair_is_one_character() {
    assert_eq!(parsed("\"\\ud83d\\ude00\""), Json::Str("😀".into()));
    assert!(Json::parse(b"\"\\ud83d\"", "a test").is_err());
    assert!(Json::parse(b"\"\\ud83dx\"", "a test").is_err());
    assert!(Json::parse(b"\"\\ud83d\\u0041\"", "a test").is_err());
}

#[test]
fn json_nesting_is_bounded_rather_than_a_stack_overflow() {
    let deep = format!("{}1{}", "[".repeat(512), "]".repeat(512));
    assert!(Json::parse(deep.as_bytes(), "a test").is_err());
}

#[test]
fn a_control_character_is_escaped_on_the_way_out() {
    assert_eq!(Json::Str("\u{1}".into()).render(), "\"\\u0001\"");
    assert_eq!(Json::Str("a\"b\\c".into()).render(), "\"a\\\"b\\\\c\"");
    assert_eq!(
        parsed(&Json::Str("\u{1}".into()).render()),
        Json::Str("\u{1}".into())
    );
}
