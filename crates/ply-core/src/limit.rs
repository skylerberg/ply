use std::cell::Cell;
use std::ptr::NonNull;

pub trait Reserved {
    fn writable(&self) -> (usize, usize);

    fn extend(&self) -> bool;
}

thread_local! {
    static RUNNING: Cell<Option<NonNull<dyn Reserved>>> = const { Cell::new(None) };
}

pub fn running() -> Option<NonNull<dyn Reserved>> {
    RUNNING.with(Cell::get)
}

pub unsafe fn set_active_stack(stack: Option<NonNull<dyn Reserved>>) {
    RUNNING.with(|r| r.set(stack));
}

fn with_running<R>(f: impl FnOnce(&dyn Reserved) -> R) -> Option<R> {
    let stack = running()?;
    Some(f(unsafe { stack.as_ref() }))
}

pub fn writable_stack_start(at: usize) -> Option<usize> {
    with_running(|stack| {
        let (low, top) = stack.writable();
        (low..top).contains(&at).then_some(low)
    })
    .flatten()
}

pub fn grow_active_stack() -> bool {
    with_running(|stack| stack.extend()).unwrap_or(false)
}
