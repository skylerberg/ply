//! What a host may lend a program, by name. The launcher lends its own program every family; any
//! other host names the ones it means, so what may drive a machine is a decision with a name and
//! a summary a reviewer can read.

use crate::hosts::Lent;
use crate::tester::TestOptions;
use ply_span::{Diagnostic, Span, codes};
use ply_ty::CheckOutput;

/// One family of capabilities, and what lending it lets a program do.
pub struct Family {
    pub name: &'static str,
    /// The effect whose operations the family lends, which a program must declare to reach them.
    pub effect: &'static str,
    pub summary: &'static str,
}

/// Every family a host can lend. `machine` is the one that drives another machine: a program lent
/// it can load, bind, enter and call a nested program, so a host lends it only on purpose.
pub const FAMILIES: &[Family] = &[
    Family {
        name: "machine",
        effect: "machine",
        summary: "load, bind, enter and call a nested program",
    },
    Family {
        name: "tester",
        effect: "tester",
        summary: "run a project's tests, and report what ran",
    },
    Family {
        name: "claims",
        effect: "prover",
        summary: "discharge a project's obligations",
    },
    Family {
        name: "builder",
        effect: "builder",
        summary: "build an artifact and stamp it",
    },
    Family {
        name: "cache",
        effect: "store",
        summary: "read, reclaim and discard what the caches hold",
    },
    Family {
        name: "bootstrap",
        effect: "archive",
        summary: "emit the compiler's own bundle",
    },
    Family {
        name: "hosts",
        effect: "tcb",
        summary: "preview what a run would bind",
    },
    Family {
        name: "edit",
        effect: "edit",
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

/// Where the program being lent declares each family's effect, by the effect's simple name: the
/// values a family hands back are named as that program names them.
pub type Declared<'a> = &'a dyn Fn(&str) -> String;

/// Every effect where the `ply` binary's own program declares it.
fn own(effect: &str) -> String {
    match effect {
        "prover" => "claims".to_string(),
        other => other.to_string(),
    }
}

/// The operations one family lends.
pub fn lent(family: &str, declared: Declared<'_>) -> Option<Vec<Lent>> {
    Some(match family {
        "machine" => {
            crate::registrations_for(crate::drive::RunOptions::default(), &declared("machine"))
        }
        "tester" => crate::tester::Session::new(&TestOptions::default()).lent(),
        "claims" => crate::claims::lent(&declared("prover")),
        "builder" => crate::builder::lent(),
        "cache" => crate::cache::lent(),
        "bootstrap" => crate::bootstrap::lent(),
        "hosts" => crate::hosts::lent(),
        "edit" => crate::edit::lent(),
        _ => return None,
    })
}

/// The operations of the named families, or why one of them is not a family.
pub fn lent_for(families: &[&str], declared: Declared<'_>) -> Result<Vec<Lent>, String> {
    let mut out = Vec::new();
    for family in families {
        match lent(family, declared) {
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

/// What `--allow` grants a program: the operations of every family it names, each of whose effect
/// the program must declare. A family whose effect it does not declare reaches nothing, so granting
/// one is refused rather than ignored.
pub fn granted(check: &CheckOutput, allow: &[String]) -> Result<Vec<Lent>, Diagnostic> {
    let declared = |effect: &str| {
        check
            .effects
            .values()
            .find(|e| e.simple_name.as_str() == effect)
    };
    let families: Vec<&str> = allow.iter().map(String::as_str).collect();
    for name in &families {
        let Some(family) = FAMILIES.iter().find(|f| f.name == *name) else {
            // Named by `lent_for`, which says what the families are.
            continue;
        };
        if declared(family.effect).is_none() {
            return Err(Diagnostic::error(
                codes::CAPABILITY_UNDECLARED,
                format!(
                    "`--allow {name}` was granted and the program declares no `{}` effect",
                    family.effect
                ),
            )
            .primary(
                Span::DUMMY,
                "a family the program does not declare reaches nothing",
            )
            .note(format!(
                "`{name}` lends the operations of `{}`, and a run lends only what the program it \
                 runs can reach",
                family.effect
            )));
        }
    }
    let module =
        |effect: &str| declared(effect).map_or_else(|| own(effect), |e| e.module.to_string());
    lent_for(&families, &module).map_err(|why| Diagnostic::error(codes::CAPABILITY_UNDECLARED, why))
}

/// Every family, for a host that lends them all: the `ply` binary's own program.
pub fn all() -> Vec<Lent> {
    FAMILIES
        .iter()
        .flat_map(|f| lent(f.name, &own).unwrap_or_default())
        .collect()
}
