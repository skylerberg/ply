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
fn an_unopened_snapshot_returns_nothing() {
    let snapshot = Snapshot::unopened();
    assert_eq!(snapshot.get("PATH"), None);
    assert_eq!(snapshot.plaintext("PATH"), None);
    assert!(!snapshot.has_spec());
}

#[test]
fn get_rejects_a_secret_key_and_secret_rejects_a_plain_one() {
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
fn without_a_schema_a_secret_value_is_readable_because_nothing_marks_it_secret() {
    let snapshot = snapshot(&[("SERVICE_API_KEY", "s3cret", false)], false);
    assert_eq!(snapshot.get("SERVICE_API_KEY"), Some("s3cret"));
    assert_eq!(snapshot.plaintext("SERVICE_API_KEY"), Some("s3cret"));
}

#[test]
fn a_secret_setting_that_was_never_supplied_returns_none_from_both_get_and_plaintext_lookups() {
    let snapshot = snapshot(&[], true);
    assert_eq!(snapshot.get("SERVICE_API_KEY"), None);
    assert_eq!(snapshot.plaintext("SERVICE_API_KEY"), None);
}

#[test]
fn a_runtime_made_by_a_host_returns_the_host_s_configuration_and_a_runtime_without_a_host_returns_an_empty_one()
 {
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
    let borrowed = ply_host::registry::facilities_of(&*runtime)
        .expect("a host made it")
        .configuration();
    assert_eq!(borrowed.get("SERVICE_REGION"), Some("eu"));
    assert_eq!(borrowed.plaintext("SERVICE_API_KEY"), Some("s3cret"));
    let unbound = ply_host::registry::unbound()();
    let borrowed = ply_host::registry::facilities_of(&*unbound)
        .expect("a host made it")
        .configuration();
    assert_eq!(borrowed.get("PATH"), None);
    assert_eq!(borrowed.plaintext("SERVICE_API_KEY"), None);
}
