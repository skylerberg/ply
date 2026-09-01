//! A free list for the persistent-chain links the machine rewrites per step.

use std::cell::RefCell;
use std::rc::Rc;
use std::thread::LocalKey;

/// Links kept per type per thread.
const KEEP: usize = 1024;

/// One link of a persistent chain.
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

/// A type whose chain links are pooled.
pub(crate) trait Pooled: Sized + 'static {
    fn free() -> &'static LocalKey<Free<Self>>;
}

/// A link holding `value` on top of `next`, from the free list when it has one.
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
    // A pooled link is uniquely owned by the pool by construction.
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
