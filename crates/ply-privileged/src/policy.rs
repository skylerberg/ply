//! What a host may grant a program, by name. The launcher grants its own program every family;
//! a run that program makes gets those its `allow` entries name, so what may drive a machine is a
//! decision with a name. Each family's natives read the grant themselves.

/// Every family a host can grant. `machine` is what lets `std.machine`'s natives grant the host
/// to a run they make, so a host grants it only on purpose.
pub const FAMILIES: [&str; 3] = ["machine", "shipped", "toolchain"];

/// Every family's name, in the order [`FAMILIES`] lists them.
pub fn names() -> Vec<&'static str> {
    FAMILIES.to_vec()
}
