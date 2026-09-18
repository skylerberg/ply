//! The compiled tier: the machine's lowered `Code`, emitted as C and handed to `cc`.
//! `PLY_C_*`, `PLY_CC*`, `PLY_HEAP_*` and `PLY_TIER_ONLY` knobs never change a program's meaning.

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
pub use load::{Library, compile_and_load};
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
