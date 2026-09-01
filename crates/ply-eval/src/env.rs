use crate::pool::{self, Free, Link, Pooled};
use crate::rc;
use crate::value::Value;
use ply_span::Symbol;
use std::rc::Rc;

/// A persistent chain so a closure can capture its defining scope by cloning a single pointer.
#[derive(Clone, Default)]
pub struct Env {
    head: Option<Rc<Link<Binding>>>,
}

pub struct Binding {
    name: Symbol,
    value: Value,
    /// The reference-counting pass proved this binding dead here, so its value was dropped out of
    /// the scope.
    released: bool,
}

/// What a name denotes in a scope.
#[derive(Clone, Copy)]
pub enum Slot<'a> {
    Live(&'a Value),
    /// Bound, but dropped by [`Env::release`] or moved out by [`Env::take_unique`].
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

    /// Every live value the scope binds, innermost first.
    pub(crate) fn values(&self) -> impl Iterator<Item = &Value> {
        self.bindings().map(|(_, value)| value)
    }

    /// [`Env::values`] with the name each value is bound under, for a caller that has to say
    /// *which* binding it found something in.
    pub(crate) fn bindings(&self) -> impl Iterator<Item = (&Symbol, &Value)> {
        let mut cur = self.head.as_deref();
        std::iter::from_fn(move || {
            while let Some(node) = cur {
                cur = node.next.as_deref();
                if let Some(binding) = &node.value
                    && !binding.released
                {
                    return Some((&binding.name, &binding.value));
                }
            }
            None
        })
    }

    /// Moves a binding's value out, when this scope is provably its only owner.
    pub fn take_unique(&mut self, name: &Symbol) -> Option<Value> {
        let taken = self.take_unique_inner(name);
        rc::note_take(taken.is_some());
        taken
    }

    fn take_unique_inner(&mut self, name: &Symbol) -> Option<Value> {
        let mut cur = &mut self.head;
        loop {
            let node = cur.as_mut()?;
            // Refuses at the first shared link: past it, some other holder can still reach
            // everything below.
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
    pub fn release(&self, dead: &[Symbol]) -> Env {
        if dead.is_empty() {
            return self.clone();
        }
        // Innermost first, and each name released once: an outer binding a shadowed name still
        // reaches is not this statement's to drop.
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
                    // A binding released earlier stays released, and one this call is releasing
                    // becomes so.
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

/// Iterative, and the reason it exists at all: a scope is a chain as long as the bindings in view,
/// the drop glue for one recurses once per link, and a thousand-deep `let` chain would abort the
/// process on the way out.
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
