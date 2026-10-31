//! A binding made from what a run is granted alone.

use ply_core::host::Grants;
use ply_privileged::hosts::Hosts;
use ply_privileged::policy;

/// Every family a host can grant, as the launcher grants `ply` its own program.
fn every_family() -> Grants {
    policy::names()
        .into_iter()
        .fold(Grants::default(), |g, family| g.with("allow", &[family]))
}

#[test]
fn every_family_granted_binds_one_handler_for_each_operation() {
    for grants in [every_family(), every_family().with("spawning", &[])] {
        if let Err(rejected) = Hosts::granted(&grants) {
            let said: Vec<String> = rejected.iter().map(|d| d.message.clone()).collect();
            panic!("{said:?}");
        }
    }
}
