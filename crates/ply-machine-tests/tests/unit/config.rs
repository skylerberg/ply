use ply_eval::Value;
use ply_host::config::{Key, Shape, Snapshot, Sources, Spec};
use ply_machine::config::*;
use ply_span::{SourceId, Symbol, codes};
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn a_hermetic_run_opens_no_source() {
    let options = ConfigOptions {
        set: vec!["K=v".to_string()],
        files: vec![PathBuf::from("deploy.env")],
        schema: Some("desk.config".to_string()),
    };
    assert!(
        options.read(false).expect("nothing is read").is_none(),
        "a run with no `--host` opens nothing"
    );
    assert!(!Configuration::default().is_opened());
}

fn check(source: &str) -> ply_ty::CheckOutput {
    ply_codegen::c::producer::checked_front(&[(String::new(), source.to_string())], &[SourceId(0)])
        .expect("the fixture typechecks")
        .check
}

/// The machine evaluates and decodes the definition it is handed, and refuses a name its program
/// does not carry: an artifact built without the schema carries none.
#[test]
fn a_schema_is_evaluated_and_decoded_and_one_the_program_does_not_carry_is_refused() {
    let program = check("fn config() -> Int = 1\n");
    let spec = spec_value(vec![key("DESK_PORT", "SInt", true, None)]);
    let answered = |_: &str| Ok(spec.clone());
    let decoded = schema::materialise(&program, "config", &answered).expect("it decodes");
    assert_eq!(decoded.keys.len(), 1);

    let absent =
        schema::materialise(&program, "desk.config", &answered).expect_err("nothing is named so");
    assert_eq!(absent.code, codes::CONFIG_UNAVAILABLE);
    assert!(
        absent.message.contains("names no definition"),
        "{}",
        absent.message
    );

    let raised = |_: &str| {
        Err(ply_span::Diagnostic::error(
            codes::RUNTIME_ERROR,
            "it raised",
        ))
    };
    let failed = schema::materialise(&program, "config", &raised).expect_err("it raised");
    assert_eq!(failed.code, codes::CONFIG_UNAVAILABLE);
    assert!(failed.message.contains("could not be evaluated: it raised"));
    assert!(
        !failed.labels[0].span.is_dummy(),
        "it points at the definition"
    );
}

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

fn shape(name: &str) -> Value {
    ctor(&format!("std.config.{name}"), Vec::new())
}

fn key(name: &str, shape_name: &str, required: bool, default: Option<&str>) -> Value {
    record(&[
        ("name", Value::Str(name.into())),
        ("shape", shape(shape_name)),
        ("required", Value::Bool(required)),
        (
            "default",
            match default {
                None => ctor("None", Vec::new()),
                Some(text) => ctor("Some", vec![Value::Str(text.into())]),
            },
        ),
    ])
}

#[allow(clippy::arc_with_non_send_sync)]
fn empty_list() -> Value {
    Value::list(Vec::new())
}

#[allow(clippy::arc_with_non_send_sync)]
fn spec_value(keys: Vec<Value>) -> Value {
    record(&[("keys", Value::list(keys))])
}

#[test]
fn a_config_spec_decodes_into_the_keys_the_run_resolves() {
    let value = spec_value(vec![
        key("DESK_PORT", "SInt", true, None),
        key("DESK_API_KEY", "SSecret", true, None),
        key("DESK_REGION", "SText", false, Some("eu")),
    ]);
    let spec = schema::spec_of(&value, "desk.config").expect("it is a `ConfigSpec`");
    assert_eq!(
        spec.keys,
        vec![
            Key {
                name: "DESK_PORT".to_string(),
                shape: Shape::Int,
                required: true,
                default: None
            },
            Key {
                name: "DESK_API_KEY".to_string(),
                shape: Shape::Secret,
                required: true,
                default: None
            },
            Key {
                name: "DESK_REGION".to_string(),
                shape: Shape::Text,
                required: false,
                default: Some("eu".to_string())
            },
        ]
    );
}

/// A partial decode would drop a required key and turn `E0441` into the `None` at first use it exists to prevent.
#[test]
fn a_value_that_is_not_a_config_spec_is_refused_rather_than_partly_read() {
    let cases: Vec<(&str, Value)> = vec![
        ("not a record", Value::Int(1)),
        ("no keys", record(&[("tables", empty_list())])),
        (
            "a key that is not a record",
            spec_value(vec![Value::Int(1)]),
        ),
        (
            "no name",
            spec_value(vec![record(&[("shape", shape("SText"))])]),
        ),
        (
            "no required flag",
            spec_value(vec![record(&[
                ("name", Value::Str("K".into())),
                ("shape", shape("SText")),
                ("default", ctor("None", Vec::new())),
            ])]),
        ),
    ];
    for (what, value) in cases {
        let error = schema::spec_of(&value, "desk.config").expect_err("`{what}` is not a spec");
        assert_eq!(error.code, codes::CONFIG_UNAVAILABLE, "{what}");
    }
}

/// A constructor's identity in a `Value` is its program-wide name.
#[test]
fn a_shape_from_another_module_is_not_one_of_std_configs() {
    let value = spec_value(vec![record(&[
        ("name", Value::Str("K".into())),
        ("shape", ctor("desk.SSecret", Vec::new())),
        ("required", Value::Bool(false)),
        ("default", ctor("None", Vec::new())),
    ])]);
    let error = schema::spec_of(&value, "desk.config").expect_err("not `std.config`'s shape");
    assert!(error.message.contains("desk.SSecret"), "{}", error.message);
}

fn configured(set: &[&str], keys: Vec<(&str, Shape, bool, Option<&str>)>) -> Configuration {
    let sources = Sources::read_with(
        &set.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
        &[],
        &[],
        &|_| Err(std::io::Error::other("no files in this fixture")),
    )
    .expect("the fixture parses");
    let spec = Spec::new(
        keys.iter()
            .map(|(name, shape, required, default)| Key {
                name: (*name).to_string(),
                shape: *shape,
                required: *required,
                default: default.map(str::to_string),
            })
            .collect(),
    )
    .expect("each key once");
    let report = Snapshot::resolve(&sources, Some(&spec)).expect("it resolves");
    Configuration {
        snapshot: Arc::new(report.snapshot),
        schema: Some(SchemaView {
            name: "desk.config".to_string(),
            keys: spec
                .keys
                .iter()
                .map(|k| (k.name.clone(), k.shape))
                .collect(),
        }),
    }
}

#[test]
fn no_projection_of_a_report_carries_a_secrets_value() {
    let configuration = configured(
        &["DESK_API_KEY=s3cret-value", "DESK_REGION=eu"],
        vec![
            ("DESK_API_KEY", Shape::Secret, true, None),
            ("DESK_REGION", Shape::Text, false, Some("us")),
        ],
    );

    let banner = configuration.banner();
    let json = serde_json::to_string(&configuration.to_json()).expect("it serializes");
    for rendered in [&banner, &json] {
        assert!(
            !rendered.contains("s3cret-value"),
            "a credential reached a report: {rendered}"
        );
    }

    assert!(json.contains("\"value\":\"****\""), "{json}");
    assert!(json.contains("\"source\":\"--set\""), "{json}");
    assert!(banner.contains("2 keys"), "{banner}");
    assert!(banner.contains("1 secrets (values not shown)"), "{banner}");
    assert!(json.contains("\"secret\":true"), "{json}");
}

#[test]
fn a_run_with_no_schema_is_still_an_opened_configuration() {
    let sources = Sources::read_with(&["K=v".to_string()], &[], &[], &|_| {
        Err(std::io::Error::other("no files"))
    })
    .expect("it parses");
    let configuration = Configuration {
        snapshot: Arc::new(
            Snapshot::resolve(&sources, None)
                .expect("it resolves")
                .snapshot,
        ),
        schema: None,
    };
    assert_eq!(configuration.to_json()["schema"], serde_json::Value::Null);
    assert!(configuration.is_opened(), "a `--set` opened a source");
}

fn digest(configuration: &Configuration) -> String {
    let mut out = String::new();
    configuration.digest_into(&mut |text| {
        out.push_str(text);
        out.push('\u{1}');
    });
    out
}

#[test]
fn the_digest_covers_a_keys_name_and_shape() {
    let base = configured(
        &["DESK_REGION=eu"],
        vec![("DESK_REGION", Shape::Text, false, None)],
    );
    let renamed = configured(
        &["DESK_AREA=eu"],
        vec![("DESK_AREA", Shape::Text, false, None)],
    );
    let reshaped = configured(
        &["DESK_REGION=1"],
        vec![("DESK_REGION", Shape::Int, false, None)],
    );
    let added = configured(
        &["DESK_REGION=eu", "DESK_PORT=1"],
        vec![
            ("DESK_REGION", Shape::Text, false, None),
            ("DESK_PORT", Shape::Int, false, None),
        ],
    );
    assert_ne!(digest(&base), digest(&renamed));
    assert_ne!(digest(&base), digest(&reshaped));
    assert_ne!(digest(&base), digest(&added));
}

#[test]
fn the_digest_does_not_cover_a_resolved_value_or_the_source_that_won() {
    let from_set = configured(
        &["DESK_REGION=eu"],
        vec![("DESK_REGION", Shape::Text, false, Some("us"))],
    );
    let from_default = configured(&[], vec![("DESK_REGION", Shape::Text, false, Some("us"))]);
    assert_eq!(
        from_set.snapshot.get("DESK_REGION"),
        Some("eu"),
        "the two really do differ in what they resolved"
    );
    assert_eq!(from_default.snapshot.get("DESK_REGION"), Some("us"));
    assert_eq!(digest(&from_set), digest(&from_default));
}

#[test]
fn a_run_with_no_schema_contributes_nothing_to_the_digest() {
    assert!(digest(&Configuration::default()).is_empty());
}
