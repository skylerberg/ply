//! The compiled tier: the machine's lowered `Code`, emitted as C and handed to `cc`.
//! `PLY_C_*`, `PLY_CC*` and `PLY_HEAP_*` knobs never change a program's meaning.

mod build;
pub mod bundle;
pub mod cache;
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
pub struct Refusals(ply_span::Diagnostic);

impl Refusals {
    /// `source` places each refused definition; without a place the label is `Span::DUMMY`.
    pub fn over(source: &crate::source::Source, refused: &[&Refused]) -> Refusals {
        let place = |r: &Refused| source.span_of(&r.function).unwrap_or(ply_span::Span::DUMMY);
        let mut listed = String::from("the emitter refused, in the order it dropped them:");
        for r in refused {
            listed.push_str(&format!("\n  `{}` ({})", r.function, r.construct));
        }
        // In drop order, so the first is a cause and the rest are what that cause carried.
        let (head, rest) = refused.split_first().expect("a refusal to report");
        let mut diagnostic = ply_span::Diagnostic::error(
            ply_span::codes::DEFINITION_REFUSED,
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

    pub fn diagnostic(&self) -> &ply_span::Diagnostic {
        &self.0
    }

    pub fn into_diagnostic(self) -> ply_span::Diagnostic {
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

/// The addresses the loaded unit binds, in [`HELPERS`]' order; a test reads both tables together.
pub fn helper_addresses() -> Vec<*mut std::ffi::c_void> {
    use crate::rt;
    let mut out: Vec<*mut std::ffi::c_void> = Vec::with_capacity(HELPERS.len());
    for h in HELPERS {
        let p = match h.name {
            "rt_cell" => rt::rt_cell as *const (),
            "rt_handle_push" => rt::rt_handle_push as *const (),
            "rt_perform" => rt::rt_perform as *const (),
            "rt_handle_land" => rt::rt_handle_land as *const (),
            "rt_simulate" => rt::rt_simulate as *const (),
            "rt_handle_detached" => rt::rt_handle_detached as *const (),
            "rt_region" => rt::rt_region as *const (),
            "rt_region_close" => rt::rt_region_close as *const (),
            "rt_dup" => rt::rt_dup as *const (),
            "rt_dec" => rt::rt_dec as *const (),
            "rt_reset" => rt::rt_reset as *const (),
            "rt_box_int" => rt::rt_box_int as *const (),
            "rt_unbox_int" => rt::rt_unbox_int as *const (),
            "rt_unbox_bool" => rt::rt_unbox_bool as *const (),
            "rt_no_fuel" => rt::rt_no_fuel as *const (),
            "rt_no_stack" => rt::rt_no_stack as *const (),
            "rt_tick" => rt::rt_tick as *const (),
            "rt_binary" => rt::rt_binary as *const (),
            "rt_negate" => rt::rt_negate as *const (),
            "rt_arith" => rt::rt_arith as *const (),
            "rt_lit" => rt::rt_lit as *const (),
            "rt_no_match" => rt::rt_no_match as *const (),
            "rt_let_no_match" => rt::rt_let_no_match as *const (),
            "rt_overflow" => rt::rt_overflow as *const (),
            "rt_not_that_width" => rt::rt_not_that_width as *const (),
            "rt_equal" => rt::rt_equal as *const (),
            "rt_concat" => rt::rt_concat as *const (),
            "rt_builtin" => rt::rt_builtin as *const (),
            "rt_bytes_join" => rt::rt_bytes_join as *const (),
            "rt_builtin_value" => rt::rt_builtin_value as *const (),
            "rt_ctor_value" => rt::rt_ctor_value as *const (),
            "rt_constant" => rt::rt_constant as *const (),
            "rt_call" => rt::rt_call as *const (),
            "rt_closure" => rt::rt_closure as *const (),
            "rt_map" => rt::rt_map as *const (),
            "rt_filter" => rt::rt_filter as *const (),
            "rt_fold" => rt::rt_fold as *const (),
            "rt_map_fold" => rt::rt_map_fold as *const (),
            "rt_iterate" => rt::rt_iterate as *const (),
            "rt_push" => rt::rt_push as *const (),
            "rt_map_insert" => rt::rt_map_insert as *const (),
            "rt_map_contains" => rt::rt_map_contains as *const (),
            "rt_map_get" => rt::rt_map_get as *const (),
            "rt_compare" => rt::rt_compare as *const (),
            "rt_byte_of_int" => rt::rt_byte_of_int as *const (),
            "rt_bytes_concat" => rt::rt_bytes_concat as *const (),
            "rt_bytes_slice" => rt::rt_bytes_slice as *const (),
            "rt_bytes_scan" => rt::rt_bytes_scan as *const (),
            "rt_bytes_scan_until" => rt::rt_bytes_scan_until as *const (),
            "rt_iterate_bad" => rt::rt_iterate_bad as *const (),
            "rt_shift_count" => rt::rt_shift_count as *const (),
            "rt_ctor" => rt::rt_ctor as *const (),
            "rt_record" => rt::rt_record as *const (),
            "rt_field" => rt::rt_field as *const (),
            "rt_list" => rt::rt_list as *const (),
            "rt_record_fits" => rt::rt_record_fits as *const (),
            "rt_record_has" => rt::rt_record_has as *const (),
            "rt_list_fits" => rt::rt_list_fits as *const (),
            "rt_list_at" => rt::rt_list_at as *const (),
            "rt_list_rest" => rt::rt_list_rest as *const (),
            "rt_ctor_arg" => rt::rt_ctor_arg as *const (),
            "rt_alloc" => rt::rt_alloc as *const (),
            "rt_list_index" => rt::rt_list_index as *const (),
            "rt_nullary" => rt::rt_nullary as *const (),
            "rt_list_set" => rt::rt_list_set as *const (),
            "rt_list_lookup" => rt::rt_list_lookup as *const (),
            "rt_map_lookup" => rt::rt_map_lookup as *const (),
            other => unreachable!("no address for the helper `{other}`"),
        };
        out.push(p as *mut std::ffi::c_void);
    }
    out
}
