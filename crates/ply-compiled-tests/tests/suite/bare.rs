//! The runtime as compiled code meets it, with no C compiled: a context over a unit of nothing, and
//! each helper at the address a unit binds, called at the C type every helper has, a word for each
//! parameter and for its answer.

use ply_compiled::c::helpers;
use ply_compiled::heap::Heap;
use ply_compiled::rt::{Ctx, Tables};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

/// A context over a unit of no definitions, constants or types.
pub fn context() -> Ctx {
    context_of(&[], Vec::new())
}

/// A context over a unit whose bodies read `fields` and whose compiled functions are `functions`,
/// by index.
pub fn context_of(fields: &[&str], functions: Vec<usize>) -> Ctx {
    context_over(fields, functions, &[])
}

/// A context over a unit whose pool of builtins holds those named `builtins`, in order.
pub fn context_calling(builtins: &[&str]) -> Ctx {
    context_over(&[], Vec::new(), builtins)
}

/// A context over a unit whose bodies read `fields`, whose compiled functions are `functions`, by
/// index, and whose pool of builtins holds those named `builtins`, in order.
// `Tables` holds a `Heap`, whose raw block pointers are neither `Send` nor `Sync`.
#[allow(clippy::arc_with_non_send_sync)]
pub fn context_over(fields: &[&str], functions: Vec<usize>, builtins: &[&str]) -> Ctx {
    Ctx::new(Arc::new(Tables {
        fields: fields.iter().map(ply_core::Symbol::new).collect(),
        builtins: (builtins.iter())
            .map(|b| ply_core::Builtin::from_name(b).unwrap_or_else(|| panic!("no builtin `{b}`")))
            .collect(),
        memo: functions.iter().map(|_| AtomicI64::new(0)).collect(),
        memo_costs: (functions.iter())
            .map(|_| (AtomicI64::new(0), AtomicI64::new(0), AtomicI64::new(0)))
            .collect(),
        functions,
        immortals: Mutex::new(Heap::persistent()),
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
