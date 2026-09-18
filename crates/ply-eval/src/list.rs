//! A list: a radix trie of `WIDTH`-wide nodes with its newest leaf held apart as the tail.
//! A push onto a shared list copies one leaf and the branches above it; `rest` moves an offset.

use crate::value::Value;
use std::sync::Arc;

/// Elements per leaf, and children per branch.
pub const WIDTH: usize = 32;
pub const BITS: u32 = 5;
const MASK: usize = WIDTH - 1;

#[derive(Clone)]
enum Node {
    Branch(Vec<Arc<Node>>),
    Leaf(Vec<Value>),
}

impl Node {
    fn slots(&self) -> usize {
        match self {
            Node::Branch(kids) => kids.len(),
            Node::Leaf(items) => items.len(),
        }
    }
}

#[derive(Clone, Default)]
pub struct List {
    /// The elements past the trie, at most `WIDTH` of them.
    tail: Arc<Vec<Value>>,
    /// The leaves before the tail, holding `len - tail.len()` elements — a multiple of `WIDTH`.
    root: Option<Arc<Node>>,
    /// Elements written, the dropped prefix included.
    len: u32,
    /// The prefix a `rest` dropped: the list's first element is at this index.
    start: u32,
}

/// The shift of the root of a trie holding `count` elements: zero while the root is a leaf.
pub fn shift_for(count: usize) -> u32 {
    let mut shift = 0;
    let mut capacity = WIDTH;
    while capacity < count {
        capacity <<= BITS;
        shift += BITS;
    }
    shift
}

/// `leaf` under `shift / BITS` single-child branches.
fn path(leaf: Arc<Node>, shift: u32) -> Arc<Node> {
    if shift == 0 {
        leaf
    } else {
        Arc::new(Node::Branch(vec![path(leaf, shift - BITS)]))
    }
}

/// `None` when a write went in place; otherwise the slots copied, zero for an empty array.
pub type Copied = Option<usize>;

fn add(copied: &mut Copied, slots: usize) {
    *copied = Some(copied.unwrap_or(0) + slots);
}

fn writable<'a>(node: &'a mut Arc<Node>, copied: &mut Copied) -> &'a mut Node {
    if Arc::get_mut(node).is_none() {
        add(copied, node.slots());
    }
    Arc::make_mut(node)
}

fn push_leaf(
    root: Option<Arc<Node>>,
    count: usize,
    leaf: Arc<Node>,
    copied: &mut Copied,
) -> Arc<Node> {
    let Some(mut root) = root else {
        return leaf;
    };
    let shift = shift_for(count);
    if count == WIDTH << shift {
        return Arc::new(Node::Branch(vec![root, path(leaf, shift)]));
    }
    insert(&mut root, shift, count, leaf, copied);
    root
}

fn insert(node: &mut Arc<Node>, shift: u32, index: usize, leaf: Arc<Node>, copied: &mut Copied) {
    match writable(node, copied) {
        Node::Branch(kids) => {
            let k = (index >> shift) & MASK;
            let below = shift - BITS;
            if k < kids.len() {
                insert(&mut kids[k], below, index, leaf, copied);
            } else {
                kids.push(path(leaf, below));
            }
        }
        Node::Leaf(_) => unreachable!("a leaf is never on the path to a new leaf"),
    }
}

fn set_in(node: &mut Arc<Node>, shift: u32, index: usize, v: Value, copied: &mut Copied) {
    match writable(node, copied) {
        Node::Branch(kids) => set_in(
            &mut kids[(index >> shift) & MASK],
            shift - BITS,
            index,
            v,
            copied,
        ),
        Node::Leaf(items) => items[index & MASK] = v,
    }
}

impl List {
    pub fn len(&self) -> usize {
        (self.len - self.start) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == self.start
    }

    fn tail_offset(&self) -> usize {
        self.len as usize - self.tail.len()
    }

    pub fn get(&self, i: usize) -> Option<&Value> {
        if i >= self.len() {
            return None;
        }
        let index = self.start as usize + i;
        let tail_offset = self.tail_offset();
        if index >= tail_offset {
            return self.tail.get(index - tail_offset);
        }
        let mut node = self.root.as_ref()?;
        let mut shift = shift_for(tail_offset);
        loop {
            match &**node {
                Node::Branch(kids) => {
                    node = kids.get((index >> shift) & MASK)?;
                    shift = shift.saturating_sub(BITS);
                }
                Node::Leaf(items) => return items.get(index & MASK),
            }
        }
    }

    pub fn first(&self) -> Option<&Value> {
        self.get(0)
    }

    pub fn last(&self) -> Option<&Value> {
        self.len().checked_sub(1).and_then(|i| self.get(i))
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            list: self,
            next: 0,
            end: self.len(),
        }
    }

    pub fn to_vec(&self) -> Vec<Value> {
        self.iter().cloned().collect()
    }

    /// Copies at most one leaf and one branch per level, whatever the length.
    pub fn push(&mut self, x: Value) -> Copied {
        let mut copied = None;
        if self.tail.len() < WIDTH {
            match Arc::get_mut(&mut self.tail) {
                Some(tail) => tail.push(x),
                None => {
                    add(&mut copied, self.tail.len());
                    let mut tail = Vec::with_capacity(self.tail.len() + 1);
                    tail.extend(self.tail.iter().cloned());
                    tail.push(x);
                    self.tail = Arc::new(tail);
                }
            }
            self.len += 1;
            return copied;
        }
        let count = self.tail_offset();
        let full = std::mem::take(&mut self.tail);
        let leaf = Arc::try_unwrap(full).unwrap_or_else(|shared| {
            add(&mut copied, WIDTH);
            (*shared).clone()
        });
        self.root = Some(push_leaf(
            self.root.take(),
            count,
            Arc::new(Node::Leaf(leaf)),
            &mut copied,
        ));
        // A list past its first leaf is growing: the next tail is sized once rather than doubled.
        let mut tail = Vec::with_capacity(WIDTH);
        tail.push(x);
        self.tail = Arc::new(tail);
        self.len += 1;
        copied
    }

    /// `i` must be in range; copies only the shared arrays on the path to it.
    pub fn set(&mut self, i: usize, v: Value) -> Copied {
        debug_assert!(i < self.len());
        let mut copied = None;
        let index = self.start as usize + i;
        let tail_offset = self.tail_offset();
        if index >= tail_offset {
            if Arc::get_mut(&mut self.tail).is_none() {
                add(&mut copied, self.tail.len());
            }
            Arc::make_mut(&mut self.tail)[index - tail_offset] = v;
            return copied;
        }
        let root = self
            .root
            .as_mut()
            .expect("an index below the tail is in the trie");
        set_in(root, shift_for(tail_offset), index, v, &mut copied);
        copied
    }

    pub fn skip(&self, k: usize) -> List {
        let mut out = self.clone();
        out.start = (self.start as usize + k).min(self.len as usize) as u32;
        out.compact();
        out
    }

    /// Drops the trie once the dropped prefix covers it, so a chain of `rest`s holds only one leaf.
    fn compact(&mut self) {
        let tail_offset = self.tail_offset();
        if self.root.is_some() && self.start as usize >= tail_offset {
            let from = self.start as usize - tail_offset;
            self.tail = Arc::new(self.tail[from..].to_vec());
            self.root = None;
            self.len = self.tail.len() as u32;
            self.start = 0;
        }
    }

    /// Moves every uniquely held element onto `out`, so a drop need not recurse through it.
    pub fn drain_unique(&mut self, out: &mut Vec<Value>) {
        if let Some(tail) = Arc::get_mut(&mut self.tail) {
            out.append(tail);
        }
        if let Some(root) = self.root.take() {
            drain_node(root, out);
        }
        self.start = 0;
        self.len = self.tail.len() as u32;
    }

    /// Whether a push would write in place.
    pub fn is_unique(&mut self) -> bool {
        Arc::get_mut(&mut self.tail).is_some() && self.root.as_mut().is_none_or(unique_path)
    }

    /// Equal identities mean equal elements, while one of the lists keeps the allocations alive.
    pub fn identity(&self) -> (usize, usize, usize, usize) {
        (
            Arc::as_ptr(&self.tail) as usize,
            self.root.as_ref().map_or(0, |r| Arc::as_ptr(r) as usize),
            self.len as usize,
            self.start as usize,
        )
    }
}

fn drain_node(node: Arc<Node>, out: &mut Vec<Value>) {
    match Arc::try_unwrap(node) {
        Ok(Node::Leaf(mut items)) => out.append(&mut items),
        Ok(Node::Branch(kids)) => kids.into_iter().for_each(|k| drain_node(k, out)),
        Err(_shared) => {}
    }
}

/// Whether the rightmost path, which a push writes, is uniquely held.
fn unique_path(node: &mut Arc<Node>) -> bool {
    match Arc::get_mut(node) {
        None => false,
        Some(Node::Leaf(_)) => true,
        Some(Node::Branch(kids)) => kids.last_mut().is_none_or(unique_path),
    }
}

impl From<Vec<Value>> for List {
    fn from(items: Vec<Value>) -> List {
        if items.len() <= WIDTH {
            return List {
                len: items.len() as u32,
                tail: Arc::new(items),
                root: None,
                start: 0,
            };
        }
        let mut list = List::default();
        let mut remaining = items.len();
        let mut items = items.into_iter();
        let mut copied = None;
        while remaining > WIDTH {
            let leaf: Vec<Value> = items.by_ref().take(WIDTH).collect();
            let count = list.len as usize;
            list.root = Some(push_leaf(
                list.root.take(),
                count,
                Arc::new(Node::Leaf(leaf)),
                &mut copied,
            ));
            list.len += WIDTH as u32;
            remaining -= WIDTH;
        }
        let tail: Vec<Value> = items.collect();
        list.len += tail.len() as u32;
        list.tail = Arc::new(tail);
        list
    }
}

impl FromIterator<Value> for List {
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> List {
        List::from(iter.into_iter().collect::<Vec<Value>>())
    }
}

impl<'a> IntoIterator for &'a List {
    type Item = &'a Value;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

pub struct Iter<'a> {
    list: &'a List,
    next: usize,
    end: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<&'a Value> {
        if self.next >= self.end {
            return None;
        }
        let v = self.list.get(self.next);
        self.next += 1;
        v
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end - self.next;
        (n, Some(n))
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        self.end -= 1;
        self.list.get(self.end)
    }
}

impl ExactSizeIterator for Iter<'_> {}

impl std::fmt::Debug for List {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for List {
    fn eq(&self, other: &List) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}
