use ply_core::{Span, Symbol, Value, codes};
use ply_privileged::config::*;
use std::sync::Arc;

// A `Value` pins `Arc` for shared payloads and `Rc` for shared code, so none of these `Arc`s can be `Send`.
#[allow(clippy::arc_with_non_send_sync)]
fn record(fields: &[(&str, Value)]) -> Value {
    Value::Record(Arc::new(
        fields
            .iter()
            .map(|(k, v)| (Symbol::new(*k), v.clone()))
            .collect(),
    ))
}

#[allow(clippy::arc_with_non_send_sync)]
fn ctor(name: &str, args: Vec<Value>) -> Value {
    Value::Ctor {
        name: Symbol::new(name),
        args: Arc::new(args),
    }
}

#[allow(clippy::arc_with_non_send_sync)]
fn list(items: Vec<Value>) -> Value {
    Value::list(items)
}

/// The `Configured` record the program hands a binding.
fn configured(
    values: &[(&str, &str, bool)],
    schema: Option<(&str, &[(&str, &str)])>,
    opened: bool,
) -> Value {
    let values = values
        .iter()
        .map(|(key, value, secret)| {
            record(&[
                ("key", Value::Str((*key).into())),
                ("value", Value::Str((*value).into())),
                ("secret", Value::Bool(*secret)),
            ])
        })
        .collect();
    let schema = match schema {
        None => ctor("None", Vec::new()),
        Some((function, keys)) => ctor(
            "Some",
            vec![record(&[
                ("function", Value::Str(function.into())),
                (
                    "keys",
                    list(
                        keys.iter()
                            .map(|(name, shape)| {
                                record(&[
                                    ("name", Value::Str((*name).into())),
                                    ("shape", Value::Str((*shape).into())),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])],
        ),
    };
    record(&[
        ("values", list(values)),
        ("schema", schema),
        ("opened", Value::Bool(opened)),
    ])
}

fn of(value: &Value) -> Configuration {
    Configuration::of(value, Span::DUMMY).expect("it is a `Configured`")
}

#[test]
fn a_binding_answers_from_the_values_the_program_resolved() {
    let configuration = of(&configured(
        &[
            ("SERVICE_API_KEY", "s3cret", true),
            ("SERVICE_REGION", "eu", false),
        ],
        Some((
            "service.config",
            &[("SERVICE_API_KEY", "secret"), ("SERVICE_REGION", "text")],
        )),
        true,
    ));
    assert!(configuration.is_opened());
    assert!(configuration.is_pinned());
    assert_eq!(configuration.snapshot.get("SERVICE_REGION"), Some("eu"));
    assert_eq!(configuration.snapshot.get("SERVICE_API_KEY"), None);
    assert_eq!(
        configuration.snapshot.plaintext("SERVICE_API_KEY"),
        Some("s3cret")
    );
    assert_eq!(configuration.snapshot.plaintext("SERVICE_REGION"), None);
}

#[test]
fn a_run_with_no_schema_is_opened_but_not_pinned() {
    let configuration = of(&configured(&[("K", "v", false)], None, true));
    assert!(configuration.is_opened(), "a `--set` opened a source");
    assert!(!configuration.is_pinned());
    assert!(!configuration.snapshot.has_spec());
    assert_eq!(configuration.snapshot.plaintext("K"), Some("v"));
    assert!(!Configuration::default().is_opened());
}

#[test]
fn a_value_that_is_not_a_configured_is_refused() {
    for value in [
        Value::Int(1),
        record(&[("values", list(Vec::new()))]),
        record(&[
            ("values", list(vec![Value::Int(1)])),
            ("schema", ctor("None", Vec::new())),
            ("opened", Value::Bool(true)),
        ]),
    ] {
        assert!(Configuration::of(&value, Span::DUMMY).is_err());
    }
}

fn digest(configuration: &Configuration) -> String {
    let mut out = String::new();
    configuration.digest_into(&mut |text| {
        out.push_str(text);
        out.push('\u{1}');
    });
    out
}

fn pinned(values: &[(&str, &str, bool)], keys: &[(&str, &str)]) -> Configuration {
    of(&configured(values, Some(("service.config", keys)), true))
}

#[test]
fn the_digest_covers_a_keys_name_and_shape() {
    let base = pinned(
        &[("SERVICE_REGION", "eu", false)],
        &[("SERVICE_REGION", "text")],
    );
    let renamed = pinned(
        &[("SERVICE_AREA", "eu", false)],
        &[("SERVICE_AREA", "text")],
    );
    let reshaped = pinned(
        &[("SERVICE_REGION", "1", false)],
        &[("SERVICE_REGION", "int")],
    );
    let added = pinned(
        &[
            ("SERVICE_REGION", "eu", false),
            ("SERVICE_PORT", "1", false),
        ],
        &[("SERVICE_REGION", "text"), ("SERVICE_PORT", "int")],
    );
    assert_ne!(digest(&base), digest(&renamed));
    assert_ne!(digest(&base), digest(&reshaped));
    assert_ne!(digest(&base), digest(&added));
}

#[test]
fn the_digest_does_not_cover_a_resolved_value() {
    let one = pinned(
        &[("SERVICE_REGION", "eu", false)],
        &[("SERVICE_REGION", "text")],
    );
    let other = pinned(
        &[("SERVICE_REGION", "us", false)],
        &[("SERVICE_REGION", "text")],
    );
    assert_ne!(
        one.snapshot.get("SERVICE_REGION"),
        other.snapshot.get("SERVICE_REGION")
    );
    assert_eq!(digest(&one), digest(&other));
}

#[test]
fn a_run_with_no_schema_contributes_nothing_to_the_digest() {
    assert!(digest(&Configuration::default()).is_empty());
    assert!(digest(&of(&configured(&[("K", "v", false)], None, true))).is_empty());
}

/// An artifact built without the schema carries none, and a definition no backend can enter has no
/// value to read.
#[test]
fn a_schema_the_program_does_not_carry_or_cannot_evaluate_is_refused() {
    // `fn config() -> Int = 1` in module `m`, which no backend is handed to enter.
    let program = crate::sketch::Sketch::default()
        .def("m", "config", Vec::new())
        .check();

    let absent = schema_of(&program, None, "service.config").expect_err("nothing is named so");
    assert_eq!(absent.code, codes::CONFIG_UNAVAILABLE);
    assert!(
        absent.message.contains("names no definition"),
        "{}",
        absent.message
    );

    let failed = schema_of(&program, None, "m.config").expect_err("no backend enters it");
    assert_eq!(failed.code, codes::CONFIG_UNAVAILABLE);
    assert!(
        failed.message.contains("could not be evaluated"),
        "{}",
        failed.message
    );
    assert!(
        !failed.labels[0].span.is_dummy(),
        "it points at the definition"
    );

    match &schema_answer(Err(failed)) {
        Value::Ctor { name, args } => {
            assert_eq!(name.as_str(), "Err");
            assert_eq!(args.len(), 1);
        }
        other => panic!("not a `Result`: {other:?}"),
    }
}
