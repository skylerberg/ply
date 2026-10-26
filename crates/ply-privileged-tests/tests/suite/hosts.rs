//! A binding made from what a run is lent alone.

use ply_core::host::Grants;
use ply_privileged::hosts::Hosts;
use ply_privileged::policy;

/// Every family a host can lend, as the launcher lends `ply` its own program.
fn every_family() -> Grants {
    policy::names()
        .into_iter()
        .fold(Grants::default(), |g, family| g.with("allow", &[family]))
}

#[test]
fn every_family_lent_binds_one_handler_for_each_operation() {
    for grants in [every_family(), every_family().with("spawning", &[])] {
        if let Err(refused) = Hosts::granted(&grants, policy::lent_to(&grants)) {
            let said: Vec<String> = refused.iter().map(|d| d.message.clone()).collect();
            panic!("{said:?}");
        }
    }
}
