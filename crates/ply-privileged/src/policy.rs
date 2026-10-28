//! What a host may lend a program, by name. The launcher lends its own program every family; a run
//! that program makes is lent the ones its `allow` entries name, so what may drive a machine is a
//! decision with a name. Each family's natives read the grant themselves.

/// Every family a host can lend. `machine` is what lets `std.machine`'s natives lend the host to a
/// run they make, so a host lends it only on purpose.
pub const FAMILIES: [&str; 3] = ["machine", "shipped", "toolchain"];

/// Every family's name, in the order [`FAMILIES`] lists them.
pub fn names() -> Vec<&'static str> {
    FAMILIES.to_vec()
}
