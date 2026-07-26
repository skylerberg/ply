use crate::value::Value;
use ply_span::Symbol;
use std::rc::Rc;

/// A persistent chain so a closure can capture its defining scope by cloning a
/// single pointer.
#[derive(Clone, Default)]
pub struct Env {
    head: Option<Rc<Node>>,
}

struct Node {
    name: Symbol,
    value: Value,
    next: Option<Rc<Node>>,
}

impl Env {
    pub fn empty() -> Env {
        Env { head: None }
    }

    pub fn bind(&self, name: Symbol, value: Value) -> Env {
        Env {
            head: Some(Rc::new(Node {
                name,
                value,
                next: self.head.clone(),
            })),
        }
    }

    pub fn lookup(&self, name: &Symbol) -> Option<&Value> {
        let mut cur = self.head.as_deref();
        while let Some(node) = cur {
            if &node.name == name {
                return Some(&node.value);
            }
            cur = node.next.as_deref();
        }
        None
    }
}
