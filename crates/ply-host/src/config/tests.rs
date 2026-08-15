use super::*;

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn sets(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

/// A reader over an in-memory tree, so a test that is about precedence is not
/// also a test about `std::fs`.
fn files(entries: &[(&str, &str)]) -> impl Fn(&Path) -> std::io::Result<String> + use<> {
    let entries: BTreeMap<String, String> = entries
        .iter()
        .map(|(p, t)| ((*p).to_string(), (*t).to_string()))
        .collect();
    move |path: &Path| {
        entries
            .get(&path.display().to_string())
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
    }
}

fn read(
    set: &[&str],
    paths: &[&str],
    environment: &[(&str, &str)],
    tree: &[(&str, &str)],
) -> Result<Sources, Vec<Diagnostic>> {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    Sources::read_with(&sets(set), &paths, &env(environment), &files(tree))
}

fn spec(keys: Vec<Key>) -> Spec {
    Spec::new(keys).expect("the fixture declares each key once")
}

fn key(name: &str, shape: Shape) -> Key {
    Key {
        name: name.to_string(),
        shape,
        required: false,
        default: None,
    }
}

fn required(name: &str, shape: Shape) -> Key {
    Key {
        required: true,
        ..key(name, shape)
    }
}

fn with_default(name: &str, shape: Shape, value: &str) -> Key {
    Key {
        default: Some(value.to_string()),
        ..key(name, shape)
    }
}

fn resolve(sources: &Sources, spec: Option<&Spec>) -> Report {
    Snapshot::resolve(sources, spec).expect("this fixture resolves")
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code).collect()
}

fn renders(diagnostic: &Diagnostic) -> String {
    let mut out = format!("{} {}", diagnostic.code, diagnostic.message);
    for label in &diagnostic.labels {
        out.push(' ');
        out.push_str(&label.message);
    }
    for note in &diagnostic.notes {
        out.push(' ');
        out.push_str(note);
    }
    out
}

// --- precedence -------------------------------------------------------------

/// Required test 19. One key supplied by all four sources resolves to `--set`,
/// and removing sources in order walks it down to the default.
#[test]
fn precedence_walks_down_as_sources_are_removed() {
    let schema = spec(vec![with_default("DESK_REGION", Shape::Text, "default")]);
    let tree = [("deploy.env", "DESK_REGION=file\n")];

    let all = read(
        &["DESK_REGION=set"],
        &["deploy.env"],
        &[("DESK_REGION", "environment")],
        &tree,
    )
    .expect("every source is well formed");
    let snapshot = resolve(&all, Some(&schema)).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("set"));
    assert_eq!(
        snapshot.values["DESK_REGION"].source,
        Source::Set,
        "`--set` is the highest-precedence source"
    );

    let without_set = read(
        &[],
        &["deploy.env"],
        &[("DESK_REGION", "environment")],
        &tree,
    )
    .expect("it is well formed");
    let snapshot = resolve(&without_set, Some(&schema)).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("file"));
    assert_eq!(
        snapshot.values["DESK_REGION"].source,
        Source::File("deploy.env".to_string())
    );

    let without_files =
        read(&[], &[], &[("DESK_REGION", "environment")], &tree).expect("it is well formed");
    let snapshot = resolve(&without_files, Some(&schema)).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("environment"));
    assert_eq!(snapshot.values["DESK_REGION"].source, Source::Environment);

    let nothing = read(&[], &[], &[], &tree).expect("it is well formed");
    let snapshot = resolve(&nothing, Some(&schema)).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("default"));
    assert_eq!(snapshot.values["DESK_REGION"].source, Source::Default);
}

/// Two `--config` files naming one key: the later file wins, which is the rule
/// that lets a deployment layer an override file over a base one.
#[test]
fn a_later_config_file_wins_over_an_earlier_one() {
    let tree = [
        ("base.env", "DESK_REGION=eu\nDESK_PORT=8137\n"),
        ("override.env", "DESK_REGION=us\n"),
    ];
    let sources = read(&[], &["base.env", "override.env"], &[], &tree).expect("both parse");
    let snapshot = resolve(&sources, None).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("us"));
    assert_eq!(snapshot.get("DESK_PORT"), Some("8137"));
}

/// The same rule for a repeated `--set`, so "the last one on the command line
/// wins" has one answer across both explicit sources.
#[test]
fn the_last_set_of_a_key_wins() {
    let sources = read(&["K=first", "K=second"], &[], &[], &[]).expect("both parse");
    assert_eq!(resolve(&sources, None).snapshot.get("K"), Some("second"));
}

/// Required test 20, at this layer: the snapshot is a value, so nothing that
/// happens to the process environment after it is built can be seen through it.
/// The run-level half of the claim is that `Sources::read` is called once, at
/// bind time, which is `Host`'s to keep.
#[test]
fn the_environment_is_read_once_and_the_snapshot_never_changes() {
    let sources = read(&[], &[], &[("DESK_REGION", "eu")], &[]).expect("it is well formed");
    let snapshot = resolve(&sources, None).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("eu"));

    // A second read of a *different* environment produces a different snapshot
    // and leaves the first one exactly as it was. There is no method on
    // `Snapshot` that could have made it otherwise, which is the property.
    let later = read(&[], &[], &[("DESK_REGION", "us")], &[]).expect("it is well formed");
    let later = resolve(&later, None).snapshot;
    assert_eq!(later.get("DESK_REGION"), Some("us"));
    assert_eq!(snapshot.get("DESK_REGION"), Some("eu"));
}

/// A run with no `--host` opens no source, whatever the environment holds. That
/// is ADR 0011's rule, and it is what keeps "a reviewer reads `--host` in the
/// command or the run reached nothing" true of configuration too.
#[test]
fn an_unopened_snapshot_answers_nothing() {
    let snapshot = Snapshot::unopened();
    assert_eq!(snapshot.get("PATH"), None);
    assert_eq!(snapshot.plaintext("PATH"), None);
    assert_eq!(snapshot.counts(), Counts::default());
    assert!(!snapshot.has_spec());
}

// --- the file format --------------------------------------------------------

/// Required test 22. Each malformed shape is `E0440` naming the file and the
/// line, so the fix is an edit to a line rather than a hunt.
#[test]
fn a_malformed_config_file_names_the_file_and_the_line() {
    let tree = [(
        "deploy.env",
        "DESK_REGION=eu\nthis line has no equals\n=empty\nDESK-PORT=8137\n",
    )];
    let errors = read(&[], &["deploy.env"], &[], &tree).expect_err("three lines are malformed");
    assert_eq!(
        codes_of(&errors),
        [
            codes::CONFIG_UNAVAILABLE,
            codes::CONFIG_UNAVAILABLE,
            codes::CONFIG_UNAVAILABLE
        ]
    );
    assert!(
        errors[0].message.contains("deploy.env"),
        "{}",
        errors[0].message
    );
    assert!(
        errors[0].message.contains("line 2"),
        "{}",
        errors[0].message
    );
    assert!(
        errors[0].message.contains("no `=`"),
        "{}",
        errors[0].message
    );
    assert!(
        errors[1].message.contains("line 3"),
        "{}",
        errors[1].message
    );
    assert!(
        errors[1].message.contains("empty key"),
        "{}",
        errors[1].message
    );
    assert!(
        errors[2].message.contains("line 4"),
        "{}",
        errors[2].message
    );
    assert!(
        errors[2].message.contains('-'),
        "the character that is not a key character is named: {}",
        errors[2].message
    );
}

#[test]
fn an_unreadable_config_file_is_e0440_naming_it() {
    let errors = read(&[], &["missing.env"], &[], &[]).expect_err("the file is not there");
    assert_eq!(codes_of(&errors), [codes::CONFIG_UNAVAILABLE]);
    assert!(
        errors[0].message.contains("missing.env"),
        "{}",
        errors[0].message
    );
}

#[test]
fn a_set_that_is_not_key_equals_value_is_e0440_with_the_form() {
    for bad in ["DESK_PORT", "=8137", "1DESK=8137"] {
        let errors = read(&[bad], &[], &[], &[]).expect_err("`{bad}` is not `KEY=VALUE`");
        assert_eq!(codes_of(&errors), [codes::CONFIG_UNAVAILABLE], "{bad}");
        assert!(
            renders(&errors[0]).contains("--set KEY=VALUE"),
            "`{bad}` was refused without saying what to write"
        );
    }
}

/// Blank lines and whole-line comments are ignored; a `#` after the `=` is part
/// of the value, because there is no quoting to escape one with and a password
/// truncated at a `#` is exactly what this file format must not do.
#[test]
fn comments_are_whole_lines_and_a_hash_in_a_value_survives() {
    let tree = [(
        "deploy.env",
        "# a comment\n\n   # an indented comment\nDESK_REGION = eu \nDESK_KEY=pa#ss\n\t\n",
    )];
    let sources = read(&[], &["deploy.env"], &[], &tree).expect("it parses");
    let snapshot = resolve(&sources, None).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("eu"));
    assert_eq!(snapshot.get("DESK_KEY"), Some("pa#ss"));
}

/// A key with an `=` in its value keeps the whole of it: the split is on the
/// first `=` and the value is the rest of the line.
#[test]
fn the_value_is_the_rest_of_the_line() {
    let sources = read(&["DESK_DSN=a=b=c"], &[], &[], &[]).expect("it parses");
    assert_eq!(
        resolve(&sources, None).snapshot.get("DESK_DSN"),
        Some("a=b=c")
    );
}

/// An empty value is a value. It is `SText`-valid and `SSecret`-invalid, which
/// is the whole of the difference between "unset" and "set to nothing" for a
/// credential.
#[test]
fn an_empty_value_is_a_value() {
    let sources = read(&["DESK_REGION="], &[], &[], &[]).expect("it parses");
    assert_eq!(
        resolve(&sources, None).snapshot.get("DESK_REGION"),
        Some("")
    );
}

/// The process environment holds names no program chose, and refusing the run
/// over one would make `ply run --host` fail on a machine with an exported bash
/// function. They are skipped rather than refused; a `--set` of the same shape
/// is refused, because there somebody typed it.
#[test]
fn an_environment_name_that_is_not_a_key_is_skipped_rather_than_refused() {
    let sources = read(
        &[],
        &[],
        &[("BASH_FUNC_x%%", "() { :; }"), ("DESK_REGION", "eu")],
        &[],
    )
    .expect("an environment is never a refusal");
    let snapshot = resolve(&sources, None).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("eu"));
    assert_eq!(snapshot.get("BASH_FUNC_x%%"), None);
}

// --- the schema -------------------------------------------------------------

/// Required test 23, first half. A required key nothing supplies is `E0441` at
/// resolution, naming the key, its shape and the four places the run looked.
#[test]
fn a_required_key_nothing_supplies_is_e0441_naming_the_four_sources() {
    let schema = spec(vec![required("DESK_API_KEY", Shape::Secret)]);
    let sources = read(
        &[],
        &["deploy.env"],
        &[("PATH", "/bin")],
        &[("deploy.env", "")],
    )
    .expect("the file is empty and parses");
    let errors = Snapshot::resolve(&sources, Some(&schema)).expect_err("nothing supplies it");
    assert_eq!(codes_of(&errors), [codes::CONFIG_MISSING]);
    let rendered = renders(&errors[0]);
    assert!(rendered.contains("DESK_API_KEY"), "{rendered}");
    assert!(rendered.contains("SSecret"), "{rendered}");
    assert!(rendered.contains("`--set`"), "{rendered}");
    assert!(rendered.contains("deploy.env"), "{rendered}");
    assert!(rendered.contains("environment variable"), "{rendered}");
    assert!(rendered.contains("default"), "{rendered}");
}

/// A required key with a default is supplied by definition, so it is not
/// `E0441`. Refusing it would make `required` and `default` contradictory in a
/// schema where they are simply both true.
#[test]
fn a_required_key_with_a_default_is_supplied() {
    let schema = spec(vec![Key {
        required: true,
        ..with_default("DESK_REGION", Shape::Text, "eu")
    }]);
    let sources = read(&[], &[], &[], &[]).expect("nothing to parse");
    let snapshot = resolve(&sources, Some(&schema)).snapshot;
    assert_eq!(snapshot.get("DESK_REGION"), Some("eu"));
}

/// Required test 23, second half. A shape mismatch is `E0442` naming the key and
/// the source that won.
#[test]
fn a_value_that_is_not_of_its_shape_is_e0442_naming_the_source() {
    let schema = spec(vec![required("DESK_PORT", Shape::Int)]);
    let sources = read(&["DESK_PORT=eight"], &[], &[], &[]).expect("it parses");
    let errors = Snapshot::resolve(&sources, Some(&schema)).expect_err("`eight` is not an Int");
    assert_eq!(codes_of(&errors), [codes::CONFIG_INVALID]);
    let rendered = renders(&errors[0]);
    assert!(rendered.contains("DESK_PORT"), "{rendered}");
    assert!(rendered.contains("SInt"), "{rendered}");
    assert!(rendered.contains("--set"), "{rendered}");
    assert!(
        rendered.contains("eight"),
        "a non-secret value is printed, because an operator debugging it needs to see it: {rendered}"
    );
}

#[test]
fn a_bool_is_true_or_false_and_nothing_else() {
    let schema = spec(vec![required("DESK_DEBUG", Shape::Bool)]);
    for good in ["true", "false"] {
        let sources = read(&[&format!("DESK_DEBUG={good}")], &[], &[], &[]).expect("it parses");
        assert_eq!(
            resolve(&sources, Some(&schema)).snapshot.get("DESK_DEBUG"),
            Some(good)
        );
    }
    for bad in ["True", "yes", "1", ""] {
        let sources = read(&[&format!("DESK_DEBUG={bad}")], &[], &[], &[]).expect("it parses");
        let errors = Snapshot::resolve(&sources, Some(&schema))
            .expect_err("`{bad}` is neither `true` nor `false`");
        assert_eq!(codes_of(&errors), [codes::CONFIG_INVALID], "{bad}");
    }
}

/// Required test 23, third half, and the one that matters. A malformed secret is
/// `E0442` and its value appears **nowhere** in the diagnostic — not in the
/// message, not in a label, not in a note. A diagnostic's message reaches
/// stderr, `--json` and a cached failure report, so a credential in one is a
/// credential in a store designed never to forget.
#[test]
fn a_malformed_secret_is_e0442_without_printing_the_value() {
    let schema = spec(vec![required("DESK_API_KEY", Shape::Secret)]);
    let sources = read(&["DESK_API_KEY="], &[], &[], &[]).expect("it parses");
    let errors = Snapshot::resolve(&sources, Some(&schema)).expect_err("an empty credential");
    assert_eq!(codes_of(&errors), [codes::CONFIG_INVALID]);
    let rendered = renders(&errors[0]);
    assert!(rendered.contains("DESK_API_KEY"), "{rendered}");
    assert!(rendered.contains("SSecret"), "{rendered}");
    assert!(
        rendered.contains("not printed"),
        "the refusal says why the value is absent: {rendered}"
    );

    // And the same with a value that is not empty, to check the omission is a
    // property of the shape rather than of there being nothing to print.
    let schema = spec(vec![Key {
        shape: Shape::Secret,
        ..required("DESK_API_KEY", Shape::Int)
    }]);
    let sources = read(&["DESK_API_KEY=  "], &[], &[], &[]).expect("it parses");
    let errors = Snapshot::resolve(&sources, Some(&schema)).expect_err("it trims to empty");
    assert!(
        !renders(&errors[0]).contains("  "),
        "the value must not appear: {}",
        renders(&errors[0])
    );
}

/// Required test 24. A `--set` the schema does not declare is `W0607`; an
/// environment variable it does not declare is not, because an environment is
/// full of names that have nothing to do with this program.
#[test]
fn an_undeclared_explicit_key_is_w0607_and_an_undeclared_environment_key_is_not() {
    let schema = spec(vec![key("DESK_REGION", Shape::Text)]);
    let tree = [("deploy.env", "DESK_RGION=eu\n")];
    let sources = read(
        &["DESK_PROT=8137"],
        &["deploy.env"],
        &[("PATH", "/bin"), ("HOME", "/root")],
        &tree,
    )
    .expect("everything parses");
    let report = resolve(&sources, Some(&schema));
    assert_eq!(
        codes_of(&report.warnings),
        [codes::CONFIG_UNDECLARED, codes::CONFIG_UNDECLARED],
        "the two typos are warned about and the two environment names are not"
    );
    let messages: Vec<&str> = report.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("DESK_PROT")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("DESK_RGION")),
        "{messages:?}"
    );
    assert!(
        renders(&report.warnings[0]).contains("DESK_REGION"),
        "the warning lists what the schema does declare, because the fix is a spelling"
    );
}

/// With no schema there is nothing to be undeclared against, so a `--set` is not
/// warned about. That is what makes `--config-schema` optional rather than
/// something every run has to carry to be quiet.
#[test]
fn without_a_schema_nothing_is_undeclared() {
    let sources = read(&["ANYTHING=1"], &[], &[], &[]).expect("it parses");
    assert!(resolve(&sources, None).warnings.is_empty());
}

/// Two declarations of one key may disagree about the shape, and which one
/// applied would depend on the order the schema function built its list in.
#[test]
fn a_schema_that_declares_a_key_twice_is_refused() {
    let error = Spec::new(vec![
        key("DESK_REGION", Shape::Text),
        key("DESK_REGION", Shape::Int),
    ])
    .expect_err("one name, two declarations");
    assert_eq!(error.code, codes::CONFIG_UNAVAILABLE);
    assert!(error.message.contains("DESK_REGION"), "{}", error.message);
}

/// Every refusal is raised together rather than one per run, so an operator
/// fixing a deployment sees the whole list instead of discovering the second
/// missing key after correcting the first.
#[test]
fn every_missing_and_invalid_key_is_reported_at_once() {
    let schema = spec(vec![
        required("A", Shape::Text),
        required("B", Shape::Text),
        required("C", Shape::Int),
    ]);
    let sources = read(&["C=three"], &[], &[], &[]).expect("it parses");
    let errors = Snapshot::resolve(&sources, Some(&schema)).expect_err("two missing, one invalid");
    assert_eq!(
        codes_of(&errors),
        [
            codes::CONFIG_MISSING,
            codes::CONFIG_MISSING,
            codes::CONFIG_INVALID
        ]
    );
}

// --- the secret gate --------------------------------------------------------

/// Required test 25. The schema decides which keys are credentials, and neither
/// operation can be talked into answering for the other's keys.
#[test]
fn get_refuses_a_secret_key_and_secret_refuses_a_plain_one() {
    let schema = spec(vec![
        required("DESK_API_KEY", Shape::Secret),
        required("DESK_REGION", Shape::Text),
    ]);
    let sources =
        read(&["DESK_API_KEY=s3cret", "DESK_REGION=eu"], &[], &[], &[]).expect("both parse");
    let snapshot = resolve(&sources, Some(&schema)).snapshot;

    assert_eq!(
        snapshot.get("DESK_API_KEY"),
        None,
        "a credential must not leave as a `String`"
    );
    assert_eq!(snapshot.plaintext("DESK_API_KEY"), Some("s3cret"));
    assert_eq!(snapshot.get("DESK_REGION"), Some("eu"));
    assert_eq!(
        snapshot.plaintext("DESK_REGION"),
        None,
        "a plain key must not be laundered into a credential"
    );
}

/// The boundary of the claim, stated as a test rather than left to a reader.
/// Without a schema there are no `SSecret` keys, so `config.get` answers a
/// password as a `String`. That is §3.4's stated hole and this is what it looks
/// like.
#[test]
fn without_a_schema_containment_is_only_as_strong_as_the_schema() {
    let sources = read(&["DESK_API_KEY=s3cret"], &[], &[], &[]).expect("it parses");
    let snapshot = resolve(&sources, None).snapshot;
    assert_eq!(snapshot.get("DESK_API_KEY"), Some("s3cret"));
    assert_eq!(snapshot.plaintext("DESK_API_KEY"), Some("s3cret"));
}

/// A key the schema declares `SSecret` that no source supplies is `None` from
/// both operations rather than an empty `Secret`, so "unset" and "set to
/// nothing" stay distinguishable at the call site as well as at start-up.
#[test]
fn an_unsupplied_optional_secret_answers_none_from_both() {
    let schema = spec(vec![key("DESK_API_KEY", Shape::Secret)]);
    let sources = read(&[], &[], &[], &[]).expect("nothing to parse");
    let snapshot = resolve(&sources, Some(&schema)).snapshot;
    assert_eq!(snapshot.get("DESK_API_KEY"), None);
    assert_eq!(snapshot.plaintext("DESK_API_KEY"), None);
}

// --- what a report may print ------------------------------------------------

/// The `keys` line prints values for non-secret keys and `****` for secret ones,
/// with the winning source beside each. That is the answer to "it connected to
/// the wrong thing", and it is the reason the precedence order is worth writing
/// down.
#[test]
fn a_report_prints_a_value_for_every_key_but_a_secret() {
    let schema = spec(vec![
        required("DESK_API_KEY", Shape::Secret),
        with_default("DESK_REGION", Shape::Text, "eu"),
        required("DESK_PORT", Shape::Int),
    ]);
    let sources = read(&["DESK_PORT=8137"], &[], &[("DESK_API_KEY", "s3cret")], &[])
        .expect("everything parses");
    let snapshot = resolve(&sources, Some(&schema)).snapshot;

    let shown: Vec<(String, String, String)> = snapshot
        .declared()
        .map(|(k, r)| {
            (
                k.to_string(),
                r.shown().to_string(),
                r.source.as_str().to_string(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                "DESK_API_KEY".to_string(),
                REDACTED.to_string(),
                "env".to_string()
            ),
            (
                "DESK_PORT".to_string(),
                "8137".to_string(),
                "--set".to_string()
            ),
            (
                "DESK_REGION".to_string(),
                "eu".to_string(),
                "default".to_string()
            ),
        ]
    );
    assert_eq!(
        snapshot.counts(),
        Counts {
            keys: 3,
            set: 1,
            file: 0,
            environment: 1,
            default: 1,
            secret: 1,
        }
    );
}

/// The environment's own names are never listed. A run holds hundreds of them
/// and none is this program's business; the counts are printed and the contents
/// are not.
#[test]
fn a_report_lists_only_the_declared_keys() {
    let schema = spec(vec![key("DESK_REGION", Shape::Text)]);
    let sources = read(
        &[],
        &[],
        &[("DESK_REGION", "eu"), ("AWS_SECRET_ACCESS_KEY", "nope")],
        &[],
    )
    .expect("it parses");
    let snapshot = resolve(&sources, Some(&schema)).snapshot;
    let listed: Vec<&str> = snapshot.declared().map(|(k, _)| k).collect();
    assert_eq!(listed, ["DESK_REGION"]);
    assert_eq!(snapshot.environment, 2, "the count is printed");
}

// --- the registration -------------------------------------------------------

/// Required test 21, at the layer that decides it. Every atom this effect
/// contributes is a **read**, so no two of them conflict and the concurrency
/// graph puts two configuration-reading tests in one group.
///
/// That is sound only because the snapshot is frozen: two readers cannot
/// disagree, and there is no writer for them to race. A `config.set` would make
/// the atom a write and serialise every test in a suite that reads one key,
/// which is the whole argument for there not being one.
#[test]
fn two_configuration_readers_never_conflict() {
    use ply_core::ty::{EffectAtom, Footprint, Resource};
    use ply_syntax::ast::Mode;

    let effect = Symbol::new(EFFECT);
    let atom = |namespace: &str| {
        Footprint::from_atoms([EffectAtom::new(
            effect.clone(),
            Resource::Named(Symbol::new(namespace)),
            Mode::Read,
        )])
    };
    assert!(!atom("database").conflicts_with(&atom("credentials")));
    assert!(
        !atom("credentials").conflicts_with(&atom("credentials")),
        "two readers of one namespace do not conflict either, which is the whole point of a read"
    );

    // And the shape that would conflict, so the test is about the mode rather
    // than about `conflicts_with` answering `false` to everything.
    let writer = Footprint::from_atoms([EffectAtom::new(
        effect.clone(),
        Resource::Named(Symbol::new("credentials")),
        Mode::Write,
    )]);
    assert!(atom("credentials").conflicts_with(&writer));
}

/// The three columns `ply hosts` prints, and each is a claim this module has to
/// be able to defend. `Repeatable` is the one worth naming: it is true because
/// a `Snapshot` is immutable, and it would be false the moment a `config.set`
/// existed.
#[test]
fn the_registration_declares_what_the_snapshot_makes_true() {
    let registry = registry(Arc::new(Snapshot::unopened()));
    let ops: Vec<String> = registry.ops().map(|op| op.to_string()).collect();
    assert_eq!(
        ops,
        ["std.config.config.get[..]", "std.config.config.secret[..]"]
    );
    for op in registry.ops() {
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        assert_eq!(op.linearity, Linearity::Repeatable);
        assert!(!op.blocking, "no source is opened at a call site");
        assert!(op.path.starts_with("ply_host::config::"));
    }
}
