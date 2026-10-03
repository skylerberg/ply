//! What a host may lend a program, by name. The launcher lends its own program every family; any
//! other host names the ones it means, so what may drive a machine is a decision with a name and
//! a summary a reviewer can read.

use crate::hosts::LentOp;
use ply_eval::{CheckOutput, Diagnostic, Span, codes};

/// One family of capabilities, and what lending it lets a program do.
pub struct Family {
    pub name: &'static str,
    /// The effect whose operations the family lends, which a program must declare to reach them.
    pub effect: &'static str,
    /// The same operations answered from what they are handed alone, by deterministic handlers.
    pub hermetic: Option<&'static str>,
    pub summary: &'static str,
}

/// Every family a host can lend. `machine` is the one that drives another machine: a program lent
/// it can load, bind, enter and call a nested program, so a host lends it only on purpose.
pub const FAMILIES: &[Family] = &[
    Family {
        name: "machine",
        effect: "machine",
        hermetic: Some("hermetic_machine"),
        summary: "load, bind, enter and call a nested program",
    },
    Family {
        name: "tester",
        effect: "tester",
        hermetic: Some("hermetic_tester"),
        summary: "run a project's tests, and report what ran; `hermetic_tester` runs them binding nothing",
    },
    Family {
        name: "claims",
        effect: "prover",
        hermetic: Some("hermetic_prover"),
        summary: "discharge a project's obligations",
    },
    Family {
        name: "hosts",
        effect: "tcb",
        hermetic: Some("hermetic_tcb"),
        summary: "preview what a run would bind",
    },
    Family {
        name: "shipped",
        effect: "shipped",
        hermetic: None,
        summary: "read the modules and the version this binary ships",
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
        "prover" => "claims",
        "tcb" => "hosts",
        "shipped" => "compiler.unit",
        other => other,
    }
    .to_string()
}

/// The operations one family lends.
pub fn lent(family: &str, declared: Declared<'_>) -> Option<Vec<LentOp>> {
    Some(match family {
        "machine" => {
            let mut ops = crate::registrations_with(crate::drive::RunOptions::default());
            ops.extend(crate::hermetic_registrations());
            ops
        }
        "tester" => {
            let mut ops = crate::tester::Session::new().lent();
            ops.extend(crate::tester::Session::hermetic().lent());
            ops
        }
        "claims" => crate::claims::lent(&declared("prover")),
        "hosts" => crate::hosts::lent(&declared("tcb")),
        "shipped" => crate::shipped::lent(),
        _ => return None,
    })
}

/// The effect a family's operations are performed under, which is what a program must declare to
/// be lent it: `claims` lends `prover`, so the family's name is not it.
pub fn effect_of(family: &str) -> Option<&'static str> {
    FAMILIES.iter().find(|f| f.name == family).map(|f| f.effect)
}

pub fn hermetic_of(family: &str) -> Option<&'static str> {
    FAMILIES.iter().find(|f| f.name == family)?.hermetic
}

/// The operations of the named families, or why one of them is not a family.
pub fn lent_for(families: &[&str], declared: Declared<'_>) -> Result<Vec<LentOp>, String> {
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
pub fn granted(check: &CheckOutput, allow: &[String]) -> Result<Vec<LentOp>, Diagnostic> {
    let declared = |effect: &str| {
        check
            .effects
            .values()
            .find(|e| e.simple_name.as_str() == effect)
    };
    let families: Vec<&str> = allow.iter().map(String::as_str).collect();
    for name in &families {
        // A name that is no family is refused by `lent_for`, which says what the families are.
        let Some(effect) = effect_of(name) else {
            continue;
        };
        if declared(effect).is_none() {
            return Err(Diagnostic::error(
                codes::CAPABILITY_UNDECLARED,
                format!("`--allow {name}` was granted and the program declares no `{effect}` effect"),
            )
            .primary(
                Span::DUMMY,
                "a family the program does not declare reaches nothing",
            )
            .note(format!(
                "`{name}` lends the operations of `{effect}`, and a run lends only what the program \
                 it runs can reach"
            )));
        }
    }
    let module =
        |effect: &str| declared(effect).map_or_else(|| own(effect), |e| e.module.to_string());
    lent_for(&families, &module).map_err(|why| Diagnostic::error(codes::CAPABILITY_UNDECLARED, why))
}

/// Every family, for a host that lends them all: the `ply` binary's own program.
pub fn all() -> Vec<LentOp> {
    FAMILIES
        .iter()
        .flat_map(|f| lent(f.name, &own).unwrap_or_default())
        .collect()
}
