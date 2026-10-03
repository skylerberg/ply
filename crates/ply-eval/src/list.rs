//! A list as it crosses the runtime's boundary: its elements in order, shared until written.
//! Compiled code holds its own, `ply_codegen::list`, and no program's `push` runs on this one.

use crate::value::Value;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct List(Arc<Vec<Value>>);

/// `None` when a write went in place; otherwise the slots copied.
pub type Copied = Option<usize>;

pub type Iter<'a> = std::slice::Iter<'a, Value>;

impl List {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<&Value> {
        self.0.get(i)
    }

    pub fn first(&self) -> Option<&Value> {
        self.0.first()
    }

    pub fn last(&self) -> Option<&Value> {
        self.0.last()
    }

    pub fn iter(&self) -> Iter<'_> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[Value] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<Value> {
        self.0.to_vec()
    }

    fn writable(&mut self) -> (&mut Vec<Value>, Copied) {
        let copied = Arc::get_mut(&mut self.0).is_none().then(|| self.0.len());
        (Arc::make_mut(&mut self.0), copied)
    }

    pub fn push(&mut self, x: Value) -> Copied {
        let (items, copied) = self.writable();
        items.push(x);
        copied
    }

    /// `i` must be in range.
    pub fn set(&mut self, i: usize, v: Value) -> Copied {
        let (items, copied) = self.writable();
        items[i] = v;
        copied
    }

    pub fn skip(&self, k: usize) -> List {
        List(Arc::new(self.0[k.min(self.0.len())..].to_vec()))
    }

    /// Moves the elements onto `out` when nothing else holds them, so a drop need not recurse.
    pub fn drain_unique(&mut self, out: &mut Vec<Value>) {
        if let Some(items) = Arc::get_mut(&mut self.0) {
            out.append(items);
        }
    }

    /// Equal identities mean equal elements, while one of the lists keeps the allocation alive.
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl From<Vec<Value>> for List {
    fn from(items: Vec<Value>) -> List {
        List(Arc::new(items))
    }
}

impl FromIterator<Value> for List {
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> List {
        List(Arc::new(iter.into_iter().collect()))
    }
}

impl<'a> IntoIterator for &'a List {
    type Item = &'a Value;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl std::fmt::Debug for List {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for List {
    fn eq(&self, other: &List) -> bool {
        self.0 == other.0
    }
}
