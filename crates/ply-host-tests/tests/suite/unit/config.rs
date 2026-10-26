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
            ("SERVICE_API_KEY", "s3cret", true),
            ("SERVICE_REGION", "eu", false),
        ],
        true,
    );
    assert_eq!(
        snapshot.get("SERVICE_API_KEY"),
        None,
        "a credential must not leave as a `String`"
    );
    assert_eq!(snapshot.plaintext("SERVICE_API_KEY"), Some("s3cret"));
    assert_eq!(snapshot.get("SERVICE_REGION"), Some("eu"));
    assert_eq!(
        snapshot.plaintext("SERVICE_REGION"),
        None,
        "a plain key must not be laundered into a credential"
    );
}

#[test]
fn without_a_schema_containment_is_only_as_strong_as_the_schema() {
    let snapshot = snapshot(&[("SERVICE_API_KEY", "s3cret", false)], false);
    assert_eq!(snapshot.get("SERVICE_API_KEY"), Some("s3cret"));
    assert_eq!(snapshot.plaintext("SERVICE_API_KEY"), Some("s3cret"));
}

/// `None`, not an empty `Secret`, so unset and set-to-nothing stay distinguishable.
#[test]
fn an_unsupplied_secret_answers_none_from_both() {
    let snapshot = snapshot(&[], true);
    assert_eq!(snapshot.get("SERVICE_API_KEY"), None);
    assert_eq!(snapshot.plaintext("SERVICE_API_KEY"), None);
}

/// The natives read the run's snapshot through its runtime, and a run granted none reads nothing.
#[test]
fn a_hosts_runtime_answers_from_its_snapshot_and_a_bare_one_answers_nothing() {
    let granted = snapshot(
        &[
            ("SERVICE_API_KEY", "s3cret", true),
            ("SERVICE_REGION", "eu", false),
        ],
        true,
    );
    let runtime = ply_host::Host::new()
        .configured(Arc::new(granted))
        .runtime();
    assert_eq!(
        runtime.configured("SERVICE_REGION", false).as_deref(),
        Some("eu")
    );
    assert_eq!(runtime.configured("SERVICE_REGION", true), None);
    assert_eq!(
        runtime.configured("SERVICE_API_KEY", true).as_deref(),
        Some("s3cret")
    );
    assert_eq!(runtime.configured("SERVICE_API_KEY", false), None);
    let bare = ply_host::jobs::runtime();
    assert_eq!(bare.configured("SERVICE_REGION", false), None);
    assert_eq!(bare.configured("SERVICE_API_KEY", true), None);
}
