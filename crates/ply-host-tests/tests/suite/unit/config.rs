use ply_eval::Symbol;
use ply_eval::host::{Determinism, Linearity};
use ply_host::config::*;
use std::sync::Arc;

fn snapshot(entries: &[(&str, &str, bool)], has_spec: bool) -> Snapshot {
    Snapshot::new(
        entries
            .iter()
            .map(|(key, value, secret)| {
                (
                    (*key).to_string(),
                    Entry {
                        value: (*value).to_string(),
                        secret: *secret,
                    },
                )
            })
            .collect(),
        has_spec,
    )
}

#[test]
fn an_unopened_snapshot_answers_nothing() {
    let snapshot = Snapshot::unopened();
    assert_eq!(snapshot.get("PATH"), None);
    assert_eq!(snapshot.plaintext("PATH"), None);
    assert!(!snapshot.has_spec());
}

#[test]
fn get_refuses_a_secret_key_and_secret_refuses_a_plain_one() {
    let snapshot = snapshot(
        &[
            ("DESK_API_KEY", "s3cret", true),
            ("DESK_REGION", "eu", false),
        ],
        true,
    );
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

#[test]
fn without_a_schema_containment_is_only_as_strong_as_the_schema() {
    let snapshot = snapshot(&[("DESK_API_KEY", "s3cret", false)], false);
    assert_eq!(snapshot.get("DESK_API_KEY"), Some("s3cret"));
    assert_eq!(snapshot.plaintext("DESK_API_KEY"), Some("s3cret"));
}

/// `None`, not an empty `Secret`, so unset and set-to-nothing stay distinguishable.
#[test]
fn an_unsupplied_secret_answers_none_from_both() {
    let snapshot = snapshot(&[], true);
    assert_eq!(snapshot.get("DESK_API_KEY"), None);
    assert_eq!(snapshot.plaintext("DESK_API_KEY"), None);
}

#[test]
fn two_configuration_readers_never_conflict() {
    use ply_eval::{EffectAtom, Footprint, Mode, Resource};

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

    // The conflicting shape too, so `conflicts_with` is shown not to answer `false` to everything.
    let writer = Footprint::from_atoms([EffectAtom::new(
        effect.clone(),
        Resource::Named(Symbol::new("credentials")),
        Mode::Write,
    )]);
    assert!(atom("credentials").conflicts_with(&writer));
}

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
