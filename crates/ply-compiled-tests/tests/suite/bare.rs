use ply_compiled::c::helpers;
use ply_compiled::heap::Heap;
use ply_compiled::rt::{Context, Tables};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

pub fn new_context() -> Context {
    context_of(&[], Vec::new())
}

#[allow(clippy::arc_with_non_send_sync)]
pub fn context_of(fields: &[&str], functions: Vec<usize>) -> Context {
    Context::new(Arc::new(Tables {
        fields: fields.iter().map(ply_core::Symbol::new).collect(),
        memo: functions.iter().map(|_| AtomicI64::new(0)).collect(),
        functions,
        immortals: Mutex::new(Heap::persistent()),
        roots: Vec::new(),
    }))
}

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
    unsafe { std::mem::transmute_copy(&h.address) }
}
