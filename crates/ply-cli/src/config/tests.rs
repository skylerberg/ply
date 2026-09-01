use super::*;
use crate::cli::{Cli, Command};
use clap::Parser;
use ply_eval::Value;
use ply_span::SourceId;

// --- the command line -------------------------------------------------------

#[test]
fn the_three_sources_parse_and_repeat() {
    let args = match Cli::parse_from([
        "ply",
        "run",
        "--host",
        "--set",
        "DESK_REGION=eu",
        "--set",
        "DESK_PORT=8137",
        "--config",
        "base.env",
        "--config",
        "override.env",
        "--config-schema",
        "desk.config",
    ])
    .command
    {
        Command::Run(args) => args,
        other => panic!("expected `run`, got {other:?}"),
    };
    assert_eq!(args.config.set, ["DESK_REGION=eu", "DESK_PORT=8137"]);
    assert_eq!(
        args.config.files,
        [PathBuf::from("base.env"), PathBuf::from("override.env")]
    );
    assert_eq!(args.config.schema.as_deref(), Some("desk.config"));
}

/// Configuration configures a *binding*.
#[test]
fn configuration_without_host_is_refused_rather_than_ignored() {
    for flag in [
        vec!["--set", "K=v"],
        vec!["--config", "deploy.env"],
        vec!["--config-schema", "desk.config"],
    ] {
        let mut argv = vec!["ply", "run"];
        argv.extend(flag.iter().copied());
        assert!(
            Cli::try_parse_from(&argv).is_err(),
            "{flag:?} must not be accepted without `--host`"
        );
        let mut with_host = vec!["ply", "run", "--host"];
        with_host.extend(flag.iter().copied());
        assert!(Cli::try_parse_from(&with_host).is_ok(), "{flag:?}");
    }
}

#[test]
fn every_binding_command_accepts_configuration() {
    for command in ["run", "test", "prove", "hosts"] {
        assert!(
            Cli::try_parse_from(["ply", command, "--host", "--set", "K=v"]).is_ok(),
            "`ply {command}` does not accept `--set`"
        );
    }
}

/// Without `--host` no source is opened at all, whatever the environment holds.
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

/// A `--config-schema` that is not `<module>.<fn>` is refused before any program is loaded, with
/// the form rather than with a hunt.
#[test]
fn a_config_schema_that_is_not_a_qualified_name_is_refused() {
    for bad in ["config", "desk.", ".config", "desk..config", "1desk.config"] {
        let error = schema::check_shape(bad).expect_err("`{bad}` is not `<module>.<fn>`");
        assert_eq!(error.code, codes::CONFIG_UNAVAILABLE, "{bad}");
    }
    assert!(schema::check_shape("desk.config").is_ok());
    assert!(schema::check_shape("store.orders.config").is_ok());
}

// --- resolving the schema function ------------------------------------------

fn check(source: &str) -> ply_core::CheckOutput {
    let module = ply_syntax::parse(SourceId(0), source).expect("the fixture parses");
    ply_core::check_module(&module).expect("the fixture typechecks")
}

const SPEC_SOURCE: &str = "\
type Shape = SText | SInt | SBool | SSecret
type Key = { name: String, shape: Shape, required: Bool, default: Option<String> }
type ConfigSpec = { keys: List<Key> }
fn config() -> ConfigSpec = { keys: [] }
fn two(a: Int) -> ConfigSpec = { keys: [] }
fn number() -> Int = 1
";

#[test]
fn a_nullary_pure_function_returning_a_spec_resolves() {
    let program = check(SPEC_SOURCE);
    assert_eq!(
        schema::resolve(&program, "config")
            .expect("it is a schema function")
            .as_str(),
        "config"
    );
}

/// Each refusal says what is wrong with the *argument*, because the fix is a different argument
/// rather than an edit to the program.
#[test]
fn a_schema_function_that_is_not_one_is_refused_with_the_reason() {
    let program = check(SPEC_SOURCE);
    for (name, why) in [
        ("two", "argument"),
        ("number", "rather than a `ConfigSpec`"),
    ] {
        let error = schema::resolve(&program, name).expect_err("`{name}` is not a schema function");
        assert_eq!(error.code, codes::CONFIG_UNAVAILABLE, "{name}");
        assert!(error.message.contains(why), "{name}: {}", error.message);
    }
}

/// An operator who mistyped the module prefix should not have to run a second command to find out
/// what they meant.
#[test]
fn an_unknown_schema_function_lists_the_candidates() {
    let program = check(SPEC_SOURCE);
    let error = schema::resolve(&program, "desk.config").expect_err("no such definition");
    assert_eq!(error.code, codes::CONFIG_UNAVAILABLE);
    assert!(
        error.notes.iter().any(|n| n.contains("config")),
        "{:?}",
        error.notes
    );
}

// --- decoding the value it returns ------------------------------------------

// A `Value` pins `Arc` for its shared payloads and `Rc` for shared code, so none of these `Arc`s
// can ever be `Send`.
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

/// A `ConfigSpec` that decoded partially would silently drop a required key and turn `E0441` into
/// the `None` at first use it exists to prevent.
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

/// A constructor's identity in a `Value` is its **program-wide** name, so a `SText` some other
/// module declared is not read as `std.config`'s.
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

// --- what a report says -----------------------------------------------------

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

/// The whole point of the `keys` line: the value for a plain key, `****` for a credential, and the
/// winning source beside each.
#[test]
fn no_projection_of_a_report_carries_a_secrets_value() {
    let configuration = configured(
        &["DESK_API_KEY=s3cret-value", "DESK_REGION=eu"],
        vec![
            ("DESK_API_KEY", Shape::Secret, true, None),
            ("DESK_REGION", Shape::Text, false, Some("us")),
        ],
    );

    let human = configuration.lines().join("\n");
    let banner = configuration.banner();
    let json = serde_json::to_string(&configuration.to_json()).expect("it serializes");
    for rendered in [&human, &banner, &json] {
        assert!(
            !rendered.contains("s3cret-value"),
            "a credential reached a report: {rendered}"
        );
    }

    assert!(human.contains("DESK_API_KEY=****"), "{human}");
    assert!(human.contains("DESK_REGION=eu"), "{human}");
    assert!(human.contains("(--set)"), "{human}");
    assert!(banner.contains("2 keys"), "{banner}");
    assert!(banner.contains("1 secrets (values not shown)"), "{banner}");
    assert!(json.contains("\"secret\":true"), "{json}");
}

/// A run that named no schema says so rather than printing a block of zeroes that reads like a run
/// configured with nothing.
#[test]
fn a_run_with_no_schema_says_what_that_costs() {
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
    let lines = configuration.lines().join("\n");
    assert!(lines.contains("schema     none"), "{lines}");
    assert!(lines.contains("`--config-schema`"), "{lines}");
    assert!(configuration.is_opened(), "a `--set` opened a source");
}

// --- the digest -------------------------------------------------------------

fn digest(configuration: &Configuration) -> String {
    let mut out = String::new();
    configuration.digest_into(&mut |text| {
        out.push_str(text);
        out.push('\u{1}');
    });
    out
}

/// A key that appears, a key that changes shape and a schema function that moves are all structural
/// changes to what the run requires of its environment, and CI should break on each.
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

/// A hermetic run contributes nothing, so no existing corpus's digest moves for want of a block it
/// has nothing to put in.
#[test]
fn a_run_with_no_schema_contributes_nothing_to_the_digest() {
    assert!(digest(&Configuration::default()).is_empty());
}
