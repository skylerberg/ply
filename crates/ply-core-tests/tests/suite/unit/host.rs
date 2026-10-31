use ply_core::host::*;

#[test]
fn a_granted_binding_keeps_what_its_run_is_granted() {
    let binding = HostBinding::granted(Grants::default().with("allow", &["toolchain"]));
    assert!(binding.is_granted());
    assert!(binding.grants().allows("toolchain"));
    assert!(!binding.grants().allows("memory"));
}

#[test]
fn an_unbound_binding_is_granted_nothing() {
    let binding = HostBinding::unbound();
    assert!(!binding.is_granted());
    assert_eq!(binding.grants(), &Grants::default());
}

#[test]
fn host_use_records_what_actually_happened() {
    let mut use_ = HostUse::default();
    assert!(use_.is_empty());
    use_.record(Determinism::Deterministic);
    assert!(
        !use_.acted(),
        "a deterministic handler returns the same on a re-run"
    );
    use_.record(Determinism::Nondeterministic);
    assert_eq!(use_.operations, 2);
    assert!(!use_.is_empty());
    assert!(use_.acted());
}
