use ply_core::host::*;

#[test]
fn a_host_binding_created_with_grants_is_granted_and_allows_only_the_grants_it_was_given() {
    let binding = HostBinding::granted(Grants::default().with("allow", &["toolchain"]));
    assert!(binding.is_granted());
    assert!(binding.grants().allows("toolchain"));
    assert!(!binding.grants().allows("memory"));
}

#[test]
fn an_unbound_host_binding_is_not_granted_and_its_set_of_grants_is_empty() {
    let binding = HostBinding::unbound();
    assert!(!binding.is_granted());
    assert_eq!(binding.grants(), &Grants::default());
}

#[test]
fn host_use_counts_every_recorded_operation_and_reports_it_acted_only_after_a_nondeterministic_operation()
 {
    let mut use_ = HostUse::default();
    assert!(use_.is_empty());
    use_.record(Determinism::Deterministic);
    assert!(
        !use_.used_nondeterminism(),
        "a deterministic handler returns the same on a re-run"
    );
    use_.record(Determinism::Nondeterministic);
    assert_eq!(use_.operations, 2);
    assert!(!use_.is_empty());
    assert!(use_.used_nondeterminism());
}
