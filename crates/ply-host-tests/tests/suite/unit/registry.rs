use ply_core::codes;
use ply_core::host::Pending;
use ply_host::Host;

#[test]
fn a_runtime_rejects_a_token_that_it_did_not_create() {
    let runtime = Host::new().runtime();
    let stray = Pending {
        token: 0,
        label: "stray",
    };
    assert_eq!(
        (ply_host::registry::run_of(&*runtime).expect("a host made the runtime"))
            .watch(&stray)
            .expect_err("a token nothing minted is rejected")
            .code,
        codes::INTERNAL_ERROR
    );
}
