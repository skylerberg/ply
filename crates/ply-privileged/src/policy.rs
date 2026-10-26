//! What a host may lend a program, by name. The launcher lends its own program every family; a run
//! that program makes is lent the ones its `allow` entries name, so what may drive a machine is a
//! decision with a name.

use crate::hosts::LentOp;

/// Every family a host can lend. `machine` registers nothing: it is what lets `std.machine`'s
/// natives lend the host to a run they make, so a host lends it only on purpose.
pub const FAMILIES: [&str; 3] = ["machine", "shipped", "toolchain"];

/// Every family's name, in the order [`FAMILIES`] lists them.
pub fn names() -> Vec<&'static str> {
    FAMILIES.to_vec()
}

/// The operations one family registers; none for a name that is no family.
pub fn lent(family: &str) -> Vec<LentOp> {
    match family {
        "shipped" => crate::shipped::lent(),
        "toolchain" => crate::toolchain::lent(),
        _ => Vec::new(),
    }
}

/// The families a run's `allow` entries name.
pub fn lent_to(grants: &ply_core::host::Grants) -> Vec<LentOp> {
    FAMILIES
        .iter()
        .filter(|family| grants.allows(family))
        .flat_map(|family| lent(family))
        .collect()
}

/// Every family, for a host that lends them all: the `ply` binary's own program.
pub fn all() -> Vec<LentOp> {
    FAMILIES.iter().flat_map(|family| lent(family)).collect()
}
