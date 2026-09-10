//! The compiled tier: the machine's lowered `Code`, emitted as C and handed to `cc`.
//!
//! ADR 0037 listed the candidates and `benches/value-model/c-tier/` priced this one on the value
//! model Ply actually compiles before it was built; `benches/value-model/` carries what was built
//! reads against the Rust bar, and ADR 0042 records why it is the only code generator.
//!
//! ## The instruments
//!
//! Thirteen environment variables steer this tier, and until they were listed here the only way to
//! find one was to grep. None changes what a program means; each narrows, widens or reports on
//! how it gets compiled.
//!
//! | variable | what it does | where |
//! | --- | --- | --- |
//! | `PLY_C_ONLY` | compile only the definitions named, comma-separated. An allow-list does not fit in an environment variable at corpus scale -- fourteen hundred names is about thirty kilobytes -- and a truncated one silently compiles a different program | `build.rs` |
//! | `PLY_C_SKIP` | the same the other way round: drop every definition whose name starts with one of these prefixes. This is the one to reach for | `build.rs` |
//! | `PLY_C_REFUSALS` | print what the tier refused and why, and how much of the offered set it took | `build.rs` |
//! | `PLY_C_DUMP` | print one body's emitted C by name, or `*` for the unit's size and its largest bodies | `build.rs` |
//! | `PLY_C_SPLIT` | print where the emit's time went: optimise-and-lower against emit | `build.rs` |
//! | `PLY_C_PHASES` | print where a whole build's time went -- emit-and-resolve, assemble, compile-and-load, tables, and the source's size -- or that the unit came back whole from the cache; and at every entry's end what it allocated, recycled and still held, by kind | `build.rs`, `rt.rs` |
//! | `PLY_C_CACHE` | where compiled objects and emitted bodies are kept. A directory of its own is what makes one measurement independent of the last | `load.rs` |
//! | `PLY_C_KEEP` | keep the emitted `.c` beside the object, which the cache otherwise throws away | `load.rs` |
//! | `PLY_C_PROFILE` | `development` (the default) picks the fast toolchain and the inlining that survives it -- `tcc` if installed, else `cc -O0`, at depth 0; `release` is `cc -O2` at depth 3. Overrides the CLI's `--profile`, so that a bench script pins one without a command line. The compiler and the depth are one choice, not two: read `toolchain.rs` before separating them | `toolchain.rs` |
//! | `PLY_CC` | the C compiler to shell out to, overriding the profile's | `load.rs` |
//! | `PLY_CC_OPT` | the optimisation flag it is given, overriding the profile's | `load.rs` |
//! | `PLY_INLINE_BUDGET` | the most syntax nodes a callee may have to be inlined | `../opt.rs` |
//! | `PLY_C_EMITTER` | `ply:<dir>` produces with the Ply emitter in `<dir>` rather than the one beside the binary: a working copy, for an emitter change not yet bootstrapped | `producer.rs` |
//! | `PLY_C_BOOTSTRAP` | `off` builds the Ply emitter with the reference emitter instead of from `spikes/ply-parser/bootstrap`, the bundle of the C it last emitted for itself; the fixpoint test in `crates/ply-codegen-tests` is what says the bundle serves, and `PLY_C_BOOTSTRAP_REFRESH=1` on that test rewrites it | `bundle.rs` |
//! | `PLY_TIER_ONLY` | `1` makes this backend the only engine: a test or an entry it does not hold fails with `E0505`, and the machine evaluates nothing (ADR 0045) | `backend.rs` |
//! | `PLY_INLINE_DEPTH` | how many times a callee's own calls are inlined in turn. This is what the unit's size follows; the budget barely moves it | `../opt.rs` |
//!
//! Two things a fourteenth would have to know. `PLY_C_ONLY` and `PLY_C_SKIP` narrow the offered set
//! **before** its digest is taken, because a refusal is cached against that digest -- filtering
//! after it served a narrowed run's refusals back to an unfiltered one and built a unit neither
//! run would produce. And anything that changes an emitted body has to reach the cache key:
//! `PLY_INLINE_*` does, through `Inlining::overridden`, and did not until a measurement at one
//! depth was served bodies emitted at another.

mod build;
pub mod bundle;
pub mod cache;
mod emit;
mod load;
mod prelude;
pub mod producer;
mod toolchain;

pub use build::{
    Native, Produced, build, emit_body, emit_body_encoded, emit_unit, emit_unit_record, load_unit,
    produce,
};
pub use load::Library;
pub use prelude::{HELPERS, PRELUDE, pointer_name, runtime_decls};
pub use toolchain::{Profile, select as select_profile};

/// What the emitter refused, and where.
#[derive(Debug)]
pub struct Refused {
    pub function: String,
    pub construct: String,
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

/// The addresses the loaded unit binds, in [`HELPERS`]' order, so a helper cannot be declared and
/// left unbound: the table below and the table there are read together by a test.
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
            other => unreachable!("no address for the helper `{other}`"),
        };
        out.push(p as *mut std::ffi::c_void);
    }
    out
}

#[cfg(test)]
mod tests;
