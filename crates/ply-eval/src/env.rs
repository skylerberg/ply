use crate::pool::{self, Free, Link, Pooled};
use crate::value::Value;
use ply_span::Symbol;
use std::rc::Rc;

/// A persistent chain so a closure can capture its defining scope by cloning a
/// single pointer.
///
/// Its links come from [`crate::pool`]: a scope is built and released on almost
/// every call, and the binding a return retires is the one the next call wants.
#[derive(Clone, Default)]
pub struct Env {
    head: Option<Rc<Link<Binding>>>,
}

pub struct Binding {
    name: Symbol,
    value: Value,
}

thread_local! {
    static BINDING_LINKS: Free<Binding> = const { Free::new() };
}

impl Pooled for Binding {
    fn free() -> &'static std::thread::LocalKey<Free<Binding>> {
        &BINDING_LINKS
    }
}

impl Env {
    pub fn empty() -> Env {
        Env { head: None }
    }

    pub fn bind(&self, name: Symbol, value: Value) -> Env {
        Env {
            head: Some(pool::link(Binding { name, value }, self.head.clone())),
        }
    }

    pub fn lookup(&self, name: &Symbol) -> Option<&Value> {
        let mut cur = self.head.as_deref();
        while let Some(node) = cur {
            if let Some(binding) = &node.value
                && &binding.name == name
            {
                return Some(&binding.value);
            }
            cur = node.next.as_deref();
        }
        None
    }
}

/// Iterative, and the reason it exists at all: a scope is a chain as long as the
/// bindings in view, the drop glue for one recurses once per link, and a
/// thousand-deep `let` chain would abort the process on the way out. The walk
/// stops at the first link a closure still holds, which is where the chain stops
/// being this scope's to free.
impl Drop for Env {
    fn drop(&mut self) {
        let mut cur = self.head.take();
        while let Some(mut node) = cur {
            match Rc::get_mut(&mut node) {
                Some(link) => {
                    link.value = None;
                    cur = link.next.take();
                    pool::give(node);
                }
                None => break,
            }
        }
    }
}
