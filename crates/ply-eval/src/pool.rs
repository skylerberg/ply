//! A free list for the persistent-chain links the machine rewrites per step.
//!
//! Two structures dominate a request's allocation count: the control stack's
//! frames ([`crate::cont::Stack`]) and the environment's bindings
//! ([`crate::env::Env`]). Both are `Rc`-linked persistent lists, both are
//! rewritten on almost every machine step, and both free the link they just
//! allocated a few steps later — so the steady state is a `malloc`/`free` pair
//! per push, which W6 measured at 20.8% of the pure request path's samples.
//!
//! Neither structure can stop being persistent. Continuation capture is O(1)
//! *because* a segment is shared by pointer, and a closure captures its scope by
//! cloning one pointer. What can change is where the link's memory comes from:
//! a link whose last owner is releasing it goes on a thread-local free list
//! instead of back to the allocator, and the next push takes it from there.
//!
//! Nothing observable moves. A pooled link is emptied of its value before it is
//! kept, so it holds nothing alive; the list is thread-local, so it cannot make
//! a `Value` cross a thread; and no Ply expression can observe an address, so a
//! reused allocation is indistinguishable from a fresh one. `--engine both` is
//! what checks that claim rather than this paragraph.

use std::cell::RefCell;
use std::rc::Rc;
use std::thread::LocalKey;

/// Links kept per type per thread.
///
/// Bounded because the pool's job is to serve the steady state, not to hold a
/// deep recursion's peak forever: a run that pushed a hundred thousand frames
/// and returned should give that memory back. Past the bound a link is freed as
/// it was before.
const KEEP: usize = 1024;

/// One link of a persistent chain.
///
/// `value` is an `Option` so a link can be emptied without being deallocated,
/// which is the whole mechanism: the slot is what gets reused, and a pooled
/// link holds `None` so it keeps no `Value` alive.
pub(crate) struct Link<T> {
    pub(crate) value: Option<T>,
    pub(crate) next: Option<Rc<Link<T>>>,
}

pub(crate) struct Free<T> {
    links: RefCell<Vec<Rc<Link<T>>>>,
}

impl<T> Free<T> {
    pub(crate) const fn new() -> Free<T> {
        Free {
            links: RefCell::new(Vec::new()),
        }
    }
}

/// A type whose chain links are pooled. One free list per implementor.
pub(crate) trait Pooled: Sized + 'static {
    fn free() -> &'static LocalKey<Free<Self>>;
}

/// A link holding `value` on top of `next`, from the free list when it has one.
///
/// `try_with` throughout: thread-local destruction order is unspecified, so a
/// chain can outlive its own free list, and a push during teardown must fall
/// back to the allocator rather than abort a worker.
pub(crate) fn link<T: Pooled>(value: T, next: Option<Rc<Link<T>>>) -> Rc<Link<T>> {
    let recycled = T::free()
        .try_with(|f| f.links.borrow_mut().pop())
        .unwrap_or(None);
    let Some(mut node) = recycled else {
        return Rc::new(Link {
            value: Some(value),
            next,
        });
    };
    // A pooled link is uniquely owned by the pool by construction. The fallback
    // is here because trusting that without checking would be a use-after-share
    // rather than a slow path.
    let Some(slot) = Rc::get_mut(&mut node) else {
        return Rc::new(Link {
            value: Some(value),
            next,
        });
    };
    slot.value = Some(value);
    slot.next = next;
    node
}

/// Keeps `node`'s allocation, dropping whatever it holds.
///
/// The caller must already own it uniquely and must have detached its `next`;
/// both are re-checked, and a link failing either is dropped normally. A link
/// kept with its tail attached would make the pool a second owner of a whole
/// chain, which is how a free list turns into a leak.
pub(crate) fn give<T: Pooled>(mut node: Rc<Link<T>>) {
    let Some(slot) = Rc::get_mut(&mut node) else {
        return;
    };
    if slot.next.is_some() {
        return;
    }
    slot.value = None;
    let _ = T::free().try_with(|f| {
        let mut links = f.links.borrow_mut();
        if links.len() < KEEP {
            links.push(node);
        }
    });
}
