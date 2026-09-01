//! The list representation, and the bound on what a shared append costs (ADR 0034 Decision 2).
//!
//! A list is a radix trie of `WIDTH`-wide nodes with its newest leaf held apart as the *tail* —
//! the shape Clojure's vector takes. A list no longer than a leaf is only a tail: one array, the
//! flat vector the request path is measured on, so a small list costs what it did. Above that, a
//! push whose path is uniquely held writes in place, a push onto a shared list copies one leaf
//! and the branches above it, and a `[x, ..rest]` pattern moves an offset rather than copying the
//! tail. No operation's cost grows with the list's length on a property the source does not show.

use crate::value::Value;
use std::sync::Arc;

/// Elements per leaf, and children per branch.
pub const WIDTH: usize = 32;
const BITS: u32 = 5;
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
fn shift_for(count: usize) -> u32 {
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

/// What a push copied: `None` when every array on its path was uniquely held and the write
/// went in place, and otherwise the slots copied — zero for an empty array, which is still a copy.
pub type Copied = Option<usize>;

fn add(copied: &mut Copied, slots: usize) {
    *copied = Some(copied.unwrap_or(0) + slots);
}

/// A node made writable, counting the slots a shared node had to be copied for.
fn writable<'a>(node: &'a mut Arc<Node>, copied: &mut Copied) -> &'a mut Node {
    if Arc::get_mut(node).is_none() {
        add(copied, node.slots());
    }
    Arc::make_mut(node)
}

/// Appends a full leaf to a trie of `count` elements.
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

    /// Appends, and answers what the append copied: nothing when every array on its path was
    /// uniquely held, and otherwise at most one leaf and one branch per level, whatever the length.
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

    /// The list without its first `k` elements, sharing every array with this one.
    pub fn skip(&self, k: usize) -> List {
        let mut out = self.clone();
        out.start = (self.start as usize + k).min(self.len as usize) as u32;
        out.compact();
        out
    }

    /// Once the dropped prefix covers the whole trie the tail is the list: drop the trie, so a
    /// chain of `rest`s over a long list holds only the leaf it is reading, and pays one copy of
    /// at most a leaf for the whole chain.
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

    /// Moves every element this list holds alone onto `out`, leaving it empty of them: what a drop
    /// that must not recurse through a deep value takes. Shared arrays stay with their other owner.
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

    /// Whether the list's arrays are all held by this list alone, so a push writes in place.
    pub fn is_unique(&mut self) -> bool {
        Arc::get_mut(&mut self.tail).is_some() && self.root.as_mut().is_none_or(unique_path)
    }
}

fn drain_node(node: Arc<Node>, out: &mut Vec<Value>) {
    match Arc::try_unwrap(node) {
        Ok(Node::Leaf(mut items)) => out.append(&mut items),
        Ok(Node::Branch(kids)) => kids.into_iter().for_each(|k| drain_node(k, out)),
        Err(_shared) => {}
    }
}

/// Whether the rightmost path — the one a push writes — is uniquely held.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(n: usize) -> Vec<Value> {
        (0..n as i64).map(Value::Int).collect()
    }

    /// Every operation, against a `Vec` model, across the sizes that reach three levels of trie.
    #[test]
    fn a_list_agrees_with_a_vec_under_every_operation() {
        let mut list = List::default();
        let mut model: Vec<Value> = Vec::new();
        let mut seed = 0x9e37_79b9_u64;
        for step in 0..40_000u64 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let roll = seed >> 33;
            if roll.is_multiple_of(97) && !model.is_empty() {
                let k = (roll as usize / 97) % model.len().min(70);
                list = list.skip(k);
                model.drain(..k);
            } else {
                let copied = list.push(Value::Int(step as i64));
                model.push(Value::Int(step as i64));
                assert_eq!(
                    copied, None,
                    "a uniquely held list copied on push at step {step}"
                );
            }
            if step % 977 == 0 {
                assert_eq!(list.len(), model.len());
                assert!(list.iter().eq(model.iter()), "diverged at step {step}");
                assert!(list.iter().rev().eq(model.iter().rev()));
                for i in [0, model.len() / 2, model.len().saturating_sub(1)] {
                    assert_eq!(list.get(i), model.get(i));
                }
                assert_eq!(list.get(model.len()), None);
            }
        }
        assert_eq!(list.to_vec(), model);
        assert_eq!(List::from(model.clone()), list);
    }

    #[test]
    fn a_shared_push_copies_one_leaf_and_the_path_above_it_whatever_the_length() {
        let mut worst = 0;
        for n in [1usize, 31, 32, 33, 1_000, 1_024, 1_025, 40_000, 100_000] {
            let base = List::from(ints(n));
            let mut pushed = base.clone();
            let copied = pushed
                .push(Value::Int(-1))
                .expect("a shared push is a copy");
            assert_eq!(pushed.len(), n + 1);
            assert_eq!(base.len(), n, "the shared base moved");
            assert_eq!(pushed.last(), Some(&Value::Int(-1)));
            worst = worst.max(copied);
            let levels = shift_for(n) / BITS + 1;
            assert!(
                copied <= WIDTH * (levels as usize + 1),
                "pushing onto a shared list of {n} copied {copied} slots"
            );
        }
        assert!(worst > 0, "the instrument saw no copy at all");
    }

    #[test]
    fn a_rest_shares_the_list_and_a_chain_of_rests_holds_one_leaf() {
        let list = List::from(ints(2_000));
        let mut cursor = list.clone();
        let mut seen = 0;
        while let Some(head) = cursor.first() {
            assert_eq!(head, &Value::Int(seen));
            seen += 1;
            cursor = cursor.skip(1);
        }
        assert_eq!(seen, 2_000);
        assert_eq!(list.len(), 2_000, "the original moved");
        let mut late = list.skip(1_990);
        assert!(
            late.root.is_none(),
            "a rest past the trie still holds the trie"
        );
        assert_eq!(late.push(Value::Int(7)), None);
        assert_eq!(
            late.to_vec(),
            [ints(2_000)[1_990..].to_vec(), vec![Value::Int(7)]].concat()
        );
    }

    #[test]
    fn a_push_onto_a_shared_empty_list_is_a_copy_of_nothing_and_not_an_in_place_write() {
        let empty = List::default();
        let mut pushed = empty.clone();
        assert_eq!(pushed.push(Value::Int(1)), Some(0));
        assert!(empty.is_empty());
        let mut alone = List::default();
        assert_eq!(alone.push(Value::Int(1)), None);
    }

    #[test]
    fn the_header_fits_the_value_the_refusal_to_widen_pins() {
        assert!(size_of::<List>() <= 24);
    }
}
