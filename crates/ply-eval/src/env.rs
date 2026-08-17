use crate::pool::{self, Free, Link, Pooled};
use crate::rc;
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
    /// The reference-counting pass proved this binding dead here, so its value
    /// was dropped out of the scope. See [`Env::release`].
    released: bool,
}

/// What a name denotes in a scope.
#[derive(Clone, Copy)]
pub enum Slot<'a> {
    Live(&'a Value),
    /// Bound, but dropped by [`Env::release`] or moved out by
    /// [`Env::take_unique`]. Reaching one is a defect in the reference-counting
    /// pass and the machine reports it as such rather than falling through to an
    /// outer binding of the same name, which would be a different value under
    /// the same name and no error at all.
    Released,
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
            head: Some(pool::link(
                Binding {
                    name,
                    value,
                    released: false,
                },
                self.head.clone(),
            )),
        }
    }

    pub fn lookup(&self, name: &Symbol) -> Option<Slot<'_>> {
        let mut cur = self.head.as_deref();
        while let Some(node) = cur {
            if let Some(binding) = &node.value
                && &binding.name == name
            {
                return Some(if binding.released {
                    Slot::Released
                } else {
                    Slot::Live(&binding.value)
                });
            }
            cur = node.next.as_deref();
        }
        None
    }

    /// Every live value the scope binds, innermost first. Shadowed bindings are
    /// included: a caller asking what the scope can *reach* has to see them.
    pub(crate) fn values(&self) -> impl Iterator<Item = &Value> {
        let mut cur = self.head.as_deref();
        std::iter::from_fn(move || {
            while let Some(node) = cur {
                cur = node.next.as_deref();
                if let Some(binding) = &node.value
                    && !binding.released
                {
                    return Some(&binding.value);
                }
            }
            None
        })
    }

    /// Moves a binding's value out, when this scope is provably its only owner.
    ///
    /// This is Perceus' "a last use is a move", and the guard is what makes it
    /// safe against a persistent scope that a closure, a prompt and a captured
    /// continuation all share by pointer. Every link from the head down to and
    /// including the binding must be uniquely referenced — which means the only
    /// path to that binding is through this `Env` value, and the machine's `env`
    /// at a `Var` node is a by-value local it drops immediately afterwards.
    ///
    /// The multi-shot case falls out of the same rule rather than needing one of
    /// its own: a frame resumed from a captured continuation is *cloned* out of
    /// a segment the continuation still holds, so its scope is shared, so
    /// nothing in it is ever moved and both resumptions read the same value.
    ///
    /// `None` — refused — is always correct and costs a clone, so the caller
    /// never has to know why.
    pub fn take_unique(&mut self, name: &Symbol) -> Option<Value> {
        let taken = self.take_unique_inner(name);
        rc::note_take(taken.is_some());
        taken
    }

    fn take_unique_inner(&mut self, name: &Symbol) -> Option<Value> {
        let mut cur = &mut self.head;
        loop {
            let node = cur.as_mut()?;
            // Refuses at the first shared link: past it, some other holder can
            // still reach everything below.
            let link = Rc::get_mut(node)?;
            let hit = link
                .value
                .as_ref()
                .is_some_and(|binding| &binding.name == name);
            if hit {
                let binding = link.value.as_mut().expect("just matched");
                if binding.released {
                    return None;
                }
                binding.released = true;
                return Some(std::mem::replace(&mut binding.value, Value::Unit));
            }
            cur = &mut link.next;
        }
    }

    /// The scope with `dead`'s bindings dropped — Perceus' `drop`.
    ///
    /// **Functional, never a write through a shared link.** A closure or a
    /// prompt that captured this scope keeps every binding it captured; only the
    /// chain returned here has them released. That is what bounds the damage a
    /// wrong dead set can do to the continuation it was computed for, where it
    /// is an `INTERNAL_ERROR` naming the binding.
    ///
    /// Only the links above the deepest released binding are rebuilt; everything
    /// below is shared.
    pub fn release(&self, dead: &[Symbol]) -> Env {
        if dead.is_empty() {
            return self.clone();
        }
        // Innermost first, and each name released once: an outer binding a
        // shadowed name still reaches is not this statement's to drop.
        let mut left = dead.len();
        let mut done: Vec<Symbol> = Vec::with_capacity(dead.len());
        let mut above: Vec<(Symbol, Option<Value>)> = Vec::new();
        let mut cur = self.head.as_deref();
        let mut tail: Option<Rc<Link<Binding>>> = None;
        while let Some(node) = cur {
            let Some(binding) = &node.value else {
                cur = node.next.as_deref();
                continue;
            };
            let drop_here =
                !binding.released && dead.contains(&binding.name) && !done.contains(&binding.name);
            if drop_here {
                done.push(binding.name.clone());
                above.push((binding.name.clone(), None));
                left -= 1;
                if left == 0 {
                    tail = node.next.clone();
                    break;
                }
            } else if binding.released {
                above.push((binding.name.clone(), None));
            } else {
                above.push((binding.name.clone(), Some(binding.value.clone())));
            }
            cur = node.next.as_deref();
        }
        if done.is_empty() {
            return self.clone();
        }
        let mut out = Env { head: tail };
        for (name, value) in above.into_iter().rev() {
            out.head = Some(pool::link(
                match value {
                    Some(value) => Binding {
                        name,
                        value,
                        released: false,
                    },
                    // A binding released earlier stays released, and one this
                    // call is releasing becomes so.
                    None => Binding {
                        name,
                        value: Value::Unit,
                        released: true,
                    },
                },
                out.head.take(),
            ));
        }
        out
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
