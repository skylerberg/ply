//! The compiled tier: the machine's lowered `Code`, emitted as C and handed to `cc`.
//! `PLY_C_*`, `PLY_CC*` and `PLY_HEAP_*` knobs never change a program's meaning.

pub mod answers;
mod build;
pub mod bundle;
pub mod cache;
pub mod dump;
pub mod exports;
mod load;
mod prelude;
pub mod producer;
pub mod sweep;
pub mod tables;
pub mod toolchain;
pub mod upgrade;

pub use build::{Native, Produced, build, load_unit, produce, served};
pub use exports::{Exports, Unserved};
pub use load::{Library, Parts, compile_and_load, split};
pub use prelude::{HELPERS, PRELUDE, RUNTIME_MARK, pointer_name, runtime_header, runtime_object};
pub use toolchain::{Profile, select as select_profile};

/// The sources of the runtime the emitter runs on, as `build.rs` digests them. Not the binary: a
/// change anywhere else in `ply`, or the other build profile, answers and emits the same.
const RUNTIME: &str = env!("PLY_RUNTIME_DIGEST");

/// [`RUNTIME`], for what is kept under the runtime that answered it.
pub fn runtime_digest() -> &'static str {
    RUNTIME
}

/// What the emitter refused, and where.
#[derive(Debug)]
pub struct Refused {
    pub function: String,
    pub construct: String,
    /// The callee whose absence dropped this body, when the fixpoint dropped it rather than the
    /// emitter refusing the body itself. A unit read back from its own C carries none.
    pub missing: Option<String>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is outside the compiled fragment: {}",
            self.function, self.construct
        )
    }
}

impl std::error::Error for Refused {}

/// The refusals in `refusals` that are errors, given the definitions the build was `offered`.
///
/// A body the fixpoint dropped because a callee was never offered is the offer's doing — a caller
/// that narrowed the set, `PLY_C_ONLY`/`PLY_C_SKIP` among them, asked for a partial unit and got
/// one — and so is anything that then lost a callee to such a body. Every other refusal is a
/// definition the program reaches and cannot run.
pub fn fatal_refusals<'a>(offered: &[&str], refusals: &'a [Refused]) -> Vec<&'a Refused> {
    let mut narrowed: std::collections::HashSet<&'a str> = std::collections::HashSet::new();
    loop {
        let grew: Vec<&'a str> = refusals
            .iter()
            .filter(|r| !narrowed.contains(r.function.as_str()))
            .filter_map(|r| {
                let missing = r.missing.as_deref()?;
                (!offered.contains(&missing) || narrowed.contains(missing))
                    .then_some(r.function.as_str())
            })
            .collect();
        if grew.is_empty() {
            break;
        }
        narrowed.extend(grew);
    }
    refusals
        .iter()
        .filter(|r| !narrowed.contains(r.function.as_str()))
        .collect()
}

/// Definitions the program reaches that the emitter refused. Which they are is settled once the
/// admitted set has, so it is raised where the unit is built rather than left to the entry that
/// would find no body.
#[derive(Debug)]
pub struct Refusals(ply_eval::Diagnostic);

impl Refusals {
    /// `source` places each refused definition; without a place the label is `Span::DUMMY`.
    pub fn over(source: &crate::source::Source, refused: &[&Refused]) -> Refusals {
        let place = |r: &Refused| source.span_of(&r.function).unwrap_or(ply_eval::Span::DUMMY);
        let mut listed = String::from("the emitter refused, in the order it dropped them:");
        for r in refused {
            listed.push_str(&format!("\n  `{}` ({})", r.function, r.construct));
        }
        // In drop order, so the first is a cause and the rest are what that cause carried.
        let (head, rest) = refused.split_first().expect("a refusal to report");
        let mut diagnostic = ply_eval::Diagnostic::error(
            ply_eval::codes::DEFINITION_REFUSED,
            format!(
                "the compiled tier cannot compile {} of the definitions this program reaches",
                refused.len()
            ),
        )
        .primary(place(head), head.construct.clone());
        for r in rest.iter().take(4) {
            diagnostic = diagnostic.secondary(place(r), r.construct.clone());
        }
        Refusals(diagnostic.note(listed).note(
            "compiled code is the only evaluator, so a definition it cannot compile is one \
                 nothing can enter; the build fails here, where the construct that refused it is \
                 still in hand, rather than at the call that would find no body",
        ))
    }

    pub fn diagnostic(&self) -> &ply_eval::Diagnostic {
        &self.0
    }

    pub fn into_diagnostic(self) -> ply_eval::Diagnostic {
        self.0
    }
}

impl std::fmt::Display for Refusals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.message)?;
        for note in &self.0.notes {
            write!(f, "\n{note}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Refusals {}

/// The refusal a failed build carries, when a definition the program reaches is what failed it
/// rather than this host's toolchain.
pub fn refused_in(error: &anyhow::Error) -> Option<&Refusals> {
    error.downcast_ref::<Refusals>()
}

/// The addresses the loaded unit binds, in [`HELPERS`]' order.
pub fn helper_addresses() -> Vec<*mut std::ffi::c_void> {
    HELPERS
        .iter()
        .map(|h| h.address as *mut std::ffi::c_void)
        .collect()
}
