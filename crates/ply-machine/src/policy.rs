//! What a host may lend a program, by name. The launcher lends its own program every family; any
//! other host names the ones it means, so what may drive a machine is a decision with a name and
//! a summary a reviewer can read.

use crate::hosts::Lent;
use crate::tester::TestOptions;

/// One family of capabilities, and what lending it lets a program do.
pub struct Family {
    pub name: &'static str,
    pub summary: &'static str,
}

/// Every family a host can lend. `machine` is the one that drives another machine: a program lent
/// it can load, bind, enter and call a nested program, so a host lends it only on purpose.
pub const FAMILIES: &[Family] = &[
    Family {
        name: "machine",
        summary: "load, bind, enter and call a nested program",
    },
    Family {
        name: "tester",
        summary: "run a project's tests, and report what ran",
    },
    Family {
        name: "claims",
        summary: "discharge a project's obligations",
    },
    Family {
        name: "builder",
        summary: "build an artifact and stamp it",
    },
    Family {
        name: "cache",
        summary: "read, reclaim and discard what the caches hold",
    },
    Family {
        name: "bootstrap",
        summary: "emit the compiler's own bundle",
    },
    Family {
        name: "hosts",
        summary: "preview what a run would bind",
    },
    Family {
        name: "edit",
        summary: "replace one item of a file with another",
    },
];

/// Every family's name, in the order [`FAMILIES`] lists them.
pub fn names() -> Vec<&'static str> {
    FAMILIES.iter().map(|f| f.name).collect()
}

pub fn known(name: &str) -> bool {
    FAMILIES.iter().any(|f| f.name == name)
}

/// The operations one family lends. `machine_module` names the machine module as the program
/// being lent declares it: the values a machine hands back are named as that program names them.
pub fn lent(family: &str, machine_module: &str) -> Option<Vec<Lent>> {
    Some(match family {
        "machine" => crate::registrations_for(crate::drive::RunOptions::default(), machine_module),
        "tester" => crate::tester::Session::new(&TestOptions::default()).lent(),
        "claims" => crate::claims::lent(),
        "builder" => crate::builder::lent(),
        "cache" => crate::cache::lent(),
        "bootstrap" => crate::bootstrap::lent(),
        "hosts" => crate::hosts::lent(),
        "edit" => crate::edit::lent(),
        _ => return None,
    })
}

/// The operations of the named families, or why one of them is not a family.
pub fn lent_for(families: &[&str], machine_module: &str) -> Result<Vec<Lent>, String> {
    let mut out = Vec::new();
    for family in families {
        match lent(family, machine_module) {
            Some(ops) => out.extend(ops),
            None => {
                return Err(format!(
                    "`{family}` is not a family a host lends; it lends {}",
                    names().join(", ")
                ));
            }
        }
    }
    Ok(out)
}

/// Every family, for a host that lends them all: the `ply` binary's own program.
pub fn all() -> Vec<Lent> {
    FAMILIES
        .iter()
        .flat_map(|f| lent(f.name, "machine").unwrap_or_default())
        .collect()
}
