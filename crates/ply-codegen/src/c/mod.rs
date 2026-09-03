//! A C tier for the compiled fragment: the same `Code` the Cranelift backend lowers, emitted as C
//! and handed to `cc`.
//!
//! ADR 0037 listed the candidates and `benches/value-model/c-tier/` priced this one on the value
//! model Ply actually compiles: about one and a half times the Rust bar where Cranelift is six.
//! This is that measurement made reachable from a shipping command.

mod build;
mod emit;
mod load;
mod prelude;

pub use build::{Native, build};
pub use load::Library;
pub use prelude::{HELPERS, PRELUDE, pointer_name, runtime_decls};

/// The addresses the loaded unit binds, in [`HELPERS`]' order, so a helper cannot be declared and
/// left unbound: the table below and the table there are read together by a test.
pub fn helper_addresses() -> Vec<*mut std::ffi::c_void> {
    use crate::rt;
    let mut out: Vec<*mut std::ffi::c_void> = Vec::with_capacity(HELPERS.len());
    for h in HELPERS {
        let p = match h.name {
            "rt_dup" => rt::rt_dup as *const (),
            "rt_dec" => rt::rt_dec as *const (),
            "rt_reset" => rt::rt_reset as *const (),
            "rt_box_int" => rt::rt_box_int as *const (),
            "rt_unbox_int" => rt::rt_unbox_int as *const (),
            "rt_unbox_bool" => rt::rt_unbox_bool as *const (),
            "rt_no_fuel" => rt::rt_no_fuel as *const (),
            "rt_arith" => rt::rt_arith as *const (),
            "rt_lit" => rt::rt_lit as *const (),
            "rt_no_match" => rt::rt_no_match as *const (),
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
