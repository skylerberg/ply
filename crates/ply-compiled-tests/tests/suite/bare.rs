//! The runtime as compiled code meets it, with no C compiled: a context over a unit of nothing, and
//! each helper at the address a unit binds, called at the C type every helper has, a word for each
//! parameter and for its answer.

// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

use ply_compiled::c::helpers;
use ply_compiled::heap::{Heap, Layouts};
use ply_compiled::rt::{Ctx, Tables};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

/// A context over a unit of no definitions, constants or types.
pub fn context() -> Ctx {
    naming(&[])
}

/// [`context`] over a unit whose field table holds `fields`, which name effects and operations by
/// their places.
pub fn naming(fields: &[&str]) -> Ctx {
    Ctx::new(Arc::new(Tables {
        consts: Vec::new(),
        const_words: Vec::new(),
        layouts: Layouts::new(Vec::new()),
        fields: fields.iter().map(ply_core::Symbol::new).collect(),
        builtins: Vec::new(),
        functions: Vec::new(),
        memo: Box::default(),
        memo_costs: Box::default(),
        lambdas: Box::default(),
        immortals: Mutex::new(Heap::persistent()),
        bytes: std::array::from_fn(|_| AtomicI64::new(0)),
        nullaries: Vec::new(),
        empty_list: 0,
        empty_map: 0,
        memo_values: Mutex::default(),
        memo_words: Mutex::default(),
        calls: Mutex::default(),
        roots: Vec::new(),
    }))
}

/// The helper `name` at `F`, which is a C function's type, as a unit binds it.
pub fn helper<F: Copy>(name: &str) -> F {
    let h = helpers()
        .iter()
        .find(|h| h.name == name)
        .unwrap_or_else(|| panic!("the runtime has no helper `{name}`"));
    assert_eq!(
        size_of::<F>(),
        size_of::<*const ()>(),
        "`{name}` at a type no function is"
    );
    // SAFETY: `F` is a function pointer of the C type the helper is declared at.
    unsafe { std::mem::transmute_copy(&h.address) }
}
