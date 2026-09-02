//! The compiled map (ADR 0036): an ordered B-tree over words, after ADR 0034's representation
//! gate — asked of the sorted array the map was — found a shared insert's cost growing with the
//! map's size. A map object holds its entry count and the root of a tree whose leaves are sorted
//! runs of `WIDTH` key–value pairs and whose branches hold up to `WIDTH` children beside each
//! child's greatest key. An insert or a removal walks one path, writing in place along what is
//! held once and copying one node per level where it is not; a probe walks the same path with a
//! binary search at each node; iteration is in key order, which is the interpreter's. Keys are
//! ordered by [`heap::cmp_words`], so a native map holds its entries where the interpreter's
//! would.

use crate::heap::{
    self, Heap, KIND_MAP, KIND_MBRANCH, KIND_MLEAF, Layouts, Obj, Word, dec, inc, is_unique, obj,
    set_word, word_at,
};
use std::cmp::Ordering;

/// Pairs per leaf, and children per branch.
pub const WIDTH: usize = 32;

/// A branch's payload: its children first — with room for one over `WIDTH`, which is what an
/// insert makes before the branch splits — then, from `KEYS`, each child's greatest key.
pub const KEYS: usize = WIDTH + 1;

fn root(m: *mut Obj) -> Word {
    unsafe { word_at(m, 0) }
}

/// The entries a map holds: what `map_len` answers.
pub fn len(m: *mut Obj) -> usize {
    unsafe { (*m).len as usize }
}

fn count(node: *mut Obj) -> usize {
    unsafe { (*node).len as usize }
}

fn is_leaf(node: *mut Obj) -> bool {
    unsafe { (*node).kind == KIND_MLEAF }
}

/// A leaf's `i`-th key and value.
pub fn leaf_key(leaf: *mut Obj, i: usize) -> Word {
    unsafe { word_at(leaf, 2 * i) }
}

pub fn leaf_value(leaf: *mut Obj, i: usize) -> Word {
    unsafe { word_at(leaf, 2 * i + 1) }
}

fn child(branch: *mut Obj, i: usize) -> Word {
    unsafe { word_at(branch, i) }
}

fn child_max(branch: *mut Obj, i: usize) -> Word {
    unsafe { word_at(branch, KEYS + i) }
}

/// The greatest key under a node.
fn max_key(node: *mut Obj) -> Word {
    if is_leaf(node) {
        leaf_key(node, count(node) - 1)
    } else {
        child_max(node, count(node) - 1)
    }
}

/// The word ranges a node's children occupy: a leaf's pairs, or a branch's children and its
/// keys.
pub fn child_words(node: *mut Obj) -> impl Iterator<Item = usize> {
    let n = count(node);
    let (pairs, branch) = if is_leaf(node) {
        (0..2 * n, 0..0)
    } else {
        (0..n, KEYS..KEYS + n)
    };
    pairs.chain(branch)
}

/// Where `k` is in a leaf, or where it would go.
fn leaf_find(layouts: &Layouts, leaf: *mut Obj, k: Word) -> Result<usize, usize> {
    let (mut lo, mut hi) = (0usize, count(leaf));
    while lo < hi {
        let mid = (lo + hi) / 2;
        match heap::cmp_words(layouts, leaf_key(leaf, mid), k) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(mid),
        }
    }
    Err(lo)
}

/// The child of a branch that holds `k` or would: the first whose greatest key is not below
/// it, and the last when every key is.
fn branch_find(layouts: &Layouts, branch: *mut Obj, k: Word) -> usize {
    let n = count(branch);
    let (mut lo, mut hi) = (0usize, n - 1);
    while lo < hi {
        let mid = (lo + hi) / 2;
        match heap::cmp_words(layouts, child_max(branch, mid), k) {
            Ordering::Less => lo = mid + 1,
            _ => hi = mid,
        }
    }
    lo
}

/// The value at `k`, borrowed.
pub fn get(layouts: &Layouts, m: *mut Obj, k: Word) -> Option<Word> {
    let mut node = root(m);
    if node == 0 {
        return None;
    }
    loop {
        let o = obj(node);
        if is_leaf(o) {
            return leaf_find(layouts, o, k).ok().map(|i| leaf_value(o, i));
        }
        node = child(o, branch_find(layouts, o, k));
    }
}

/// `f` on every entry in key order, borrowed.
pub fn for_each<F: FnMut(Word, Word)>(m: *mut Obj, mut f: F) {
    let r = root(m);
    if r != 0 {
        walk(obj(r), &mut f);
    }
}

fn walk<F: FnMut(Word, Word)>(node: *mut Obj, f: &mut F) {
    if is_leaf(node) {
        for i in 0..count(node) {
            f(leaf_key(node, i), leaf_value(node, i));
        }
        return;
    }
    for i in 0..count(node) {
        walk(obj(child(node, i)), f);
    }
}

/// Every entry in key order, borrowed.
pub fn to_vec(m: *mut Obj) -> Vec<(Word, Word)> {
    let mut out = Vec::with_capacity(len(m));
    for_each(m, |k, v| out.push((k, v)));
    out
}

/// What an insert into a node answered: the node to hold from now on, its greatest key, and a
/// new sibling to its right with its greatest key when the node had to split.
struct Put {
    node: Word,
    max: Word,
    split: Option<(Word, Word)>,
    added: bool,
}

impl Heap {
    /// An empty map.
    pub fn map_new(&mut self) -> Word {
        let m = self.raw_alloc(KIND_MAP, 0, 0, 0, 8);
        unsafe { set_word(m, 0, 0) };
        m as Word
    }

    fn alloc_leaf(&mut self) -> *mut Obj {
        self.raw_alloc(KIND_MLEAF, 0, 0, WIDTH as u32, WIDTH * 16)
    }

    fn alloc_branch(&mut self) -> *mut Obj {
        self.raw_alloc(KIND_MBRANCH, 0, 0, WIDTH as u32, 2 * KEYS * 8)
    }

    /// A map over sorted, distinct entries, which it takes: leaves filled left to right and
    /// branches above them, with no path walked.
    pub fn map_from_sorted(&mut self, entries: &[(Word, Word)]) -> Word {
        let mw = self.map_new();
        let m = obj(mw);
        if entries.is_empty() {
            return mw;
        }
        let mut level: Vec<Word> = Vec::with_capacity(entries.len().div_ceil(WIDTH));
        for run in entries.chunks(WIDTH) {
            let leaf = self.alloc_leaf();
            for (i, (k, v)) in run.iter().enumerate() {
                unsafe {
                    set_word(leaf, 2 * i, *k);
                    set_word(leaf, 2 * i + 1, *v);
                }
            }
            unsafe { (*leaf).len = run.len() as u32 };
            level.push(leaf as Word);
        }
        while level.len() > 1 {
            let mut above = Vec::with_capacity(level.len().div_ceil(WIDTH));
            for run in level.chunks(WIDTH) {
                let branch = self.alloc_branch();
                for (i, node) in run.iter().enumerate() {
                    let max = max_key(obj(*node));
                    inc(max);
                    unsafe {
                        set_word(branch, i, *node);
                        set_word(branch, KEYS + i, max);
                    }
                }
                unsafe { (*branch).len = run.len() as u32 };
                above.push(branch as Word);
            }
            level = above;
        }
        unsafe {
            set_word(m, 0, level[0]);
            (*m).len = entries.len() as u32;
        }
        mw
    }

    /// A node that may be written: itself when held once, and otherwise a copy holding its
    /// children and keys once more, with the original released.
    fn writable_node(&mut self, node: Word) -> *mut Obj {
        if is_unique(node) {
            return obj(node);
        }
        let o = obj(node);
        let copy = if is_leaf(o) {
            self.alloc_leaf()
        } else {
            self.alloc_branch()
        };
        for i in child_words(o) {
            let w = unsafe { word_at(o, i) };
            inc(w);
            unsafe { set_word(copy, i, w) };
        }
        unsafe { (*copy).len = (*o).len };
        dec(node);
        copy
    }

    /// The map object to write: itself when held once, a copy holding the root once more
    /// otherwise.
    fn writable_map(&mut self, m: Word) -> *mut Obj {
        if is_unique(m) {
            return obj(m);
        }
        let o = obj(m);
        let copy = self.raw_alloc(KIND_MAP, 0, unsafe { (*o).len }, 0, 8);
        let r = root(o);
        if r != 0 {
            inc(r);
        }
        unsafe { set_word(copy, 0, r) };
        dec(m);
        copy
    }

    /// `map_insert`: the entry replaced, key and value both, when the key is present, and put
    /// in order when it is not — in place along what is held once. Takes all three.
    pub fn map_insert(&mut self, layouts: &Layouts, m: Word, k: Word, v: Word) -> Word {
        let m = self.writable_map(m);
        let r = root(m);
        if r == 0 {
            let leaf = self.alloc_leaf();
            unsafe {
                set_word(leaf, 0, k);
                set_word(leaf, 1, v);
                (*leaf).len = 1;
                set_word(m, 0, leaf as Word);
                (*m).len = 1;
            }
            return m as Word;
        }
        let put = self.put(layouts, r, k, v);
        let new_root = match put.split {
            None => put.node,
            Some((right, right_max)) => {
                let branch = self.alloc_branch();
                inc(put.max);
                inc(right_max);
                unsafe {
                    set_word(branch, 0, put.node);
                    set_word(branch, KEYS, put.max);
                    set_word(branch, 1, right);
                    set_word(branch, KEYS + 1, right_max);
                    (*branch).len = 2;
                }
                branch as Word
            }
        };
        unsafe {
            set_word(m, 0, new_root);
            if put.added {
                (*m).len += 1;
            }
        }
        m as Word
    }

    fn put(&mut self, layouts: &Layouts, node: Word, k: Word, v: Word) -> Put {
        let o = self.writable_node(node);
        if is_leaf(o) {
            return self.put_leaf(layouts, o, k, v);
        }
        let i = branch_find(layouts, o, k);
        let below = self.put(layouts, child(o, i), k, v);
        unsafe { set_word(o, i, below.node) };
        // The child's greatest key may have moved: the branch holds the current one.
        if child_max(o, i) != below.max {
            inc(below.max);
            dec(child_max(o, i));
            unsafe { set_word(o, KEYS + i, below.max) };
        }
        if let Some((right, right_max)) = below.split {
            self.branch_insert(o, i + 1, right, right_max);
        }
        let n = count(o);
        if n <= WIDTH {
            return Put {
                node: o as Word,
                max: child_max(o, n - 1),
                split: None,
                added: below.added,
            };
        }
        // Over by one: the upper half moves to a new branch beside this one.
        let right = self.alloc_branch();
        let half = n / 2;
        for j in half..n {
            unsafe {
                set_word(right, j - half, child(o, j));
                set_word(right, KEYS + j - half, child_max(o, j));
            }
        }
        unsafe {
            (*right).len = (n - half) as u32;
            (*o).len = half as u32;
        }
        Put {
            node: o as Word,
            max: child_max(o, half - 1),
            split: Some((right as Word, child_max(right, n - half - 1))),
            added: below.added,
        }
    }

    /// `right` and its greatest key go in at `i` of a branch that is not full past one over.
    fn branch_insert(&mut self, branch: *mut Obj, i: usize, right: Word, right_max: Word) {
        let n = count(branch);
        inc(right_max);
        unsafe {
            for j in (i..n).rev() {
                set_word(branch, j + 1, child(branch, j));
                set_word(branch, KEYS + j + 1, child_max(branch, j));
            }
            set_word(branch, i, right);
            set_word(branch, KEYS + i, right_max);
            (*branch).len = n as u32 + 1;
        }
    }

    fn put_leaf(&mut self, layouts: &Layouts, leaf: *mut Obj, k: Word, v: Word) -> Put {
        let n = count(leaf);
        match leaf_find(layouts, leaf, k) {
            Ok(i) => {
                unsafe {
                    dec(leaf_key(leaf, i));
                    dec(leaf_value(leaf, i));
                    set_word(leaf, 2 * i, k);
                    set_word(leaf, 2 * i + 1, v);
                }
                Put {
                    node: leaf as Word,
                    max: leaf_key(leaf, n - 1),
                    split: None,
                    added: false,
                }
            }
            Err(i) if n < WIDTH => {
                unsafe {
                    let base = heap::words(leaf);
                    std::ptr::copy(base.add(2 * i), base.add(2 * i + 2), 2 * (n - i));
                    set_word(leaf, 2 * i, k);
                    set_word(leaf, 2 * i + 1, v);
                    (*leaf).len = n as u32 + 1;
                }
                Put {
                    node: leaf as Word,
                    max: leaf_key(leaf, n),
                    split: None,
                    added: true,
                }
            }
            Err(i) => {
                // Full: the upper half moves to a new leaf beside this one, and the entry goes
                // into whichever half it falls in.
                let right = self.alloc_leaf();
                let half = n / 2;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        heap::words(leaf).add(2 * half),
                        heap::words(right),
                        2 * (n - half),
                    );
                    (*right).len = (n - half) as u32;
                    (*leaf).len = half as u32;
                }
                let (target, at) = if i <= half {
                    (leaf, i)
                } else {
                    (right, i - half)
                };
                let t = count(target);
                unsafe {
                    let base = heap::words(target);
                    std::ptr::copy(base.add(2 * at), base.add(2 * at + 2), 2 * (t - at));
                    set_word(target, 2 * at, k);
                    set_word(target, 2 * at + 1, v);
                    (*target).len = t as u32 + 1;
                }
                Put {
                    node: leaf as Word,
                    max: leaf_key(leaf, count(leaf) - 1),
                    split: Some((right as Word, leaf_key(right, count(right) - 1))),
                    added: true,
                }
            }
        }
    }

    /// `map_remove`: the map without `k`, in place along what is held once. Takes the map,
    /// reads the key.
    pub fn map_remove(&mut self, layouts: &Layouts, m: Word, k: Word) -> Word {
        if get(layouts, obj(m), k).is_none() {
            return m;
        }
        let m = self.writable_map(m);
        let r = root(m);
        let (node, _) = self.take(layouts, r, k);
        let new_root = match node {
            // The root branch left with one child is that child; an emptied root is none.
            Some(node) if !is_leaf(obj(node)) && count(obj(node)) == 1 => {
                let only = child(obj(node), 0);
                inc(only);
                dec(node);
                only
            }
            Some(node) => node,
            None => 0,
        };
        unsafe {
            set_word(m, 0, new_root);
            (*m).len -= 1;
        }
        m as Word
    }

    /// The node without `k`, and its greatest key, or nothing when the removal emptied it.
    fn take(&mut self, layouts: &Layouts, node: Word, k: Word) -> (Option<Word>, Option<Word>) {
        let o = self.writable_node(node);
        if is_leaf(o) {
            let n = count(o);
            let Ok(i) = leaf_find(layouts, o, k) else {
                return (Some(o as Word), Some(leaf_key(o, n - 1)));
            };
            unsafe {
                dec(leaf_key(o, i));
                dec(leaf_value(o, i));
                let base = heap::words(o);
                std::ptr::copy(base.add(2 * i + 2), base.add(2 * i), 2 * (n - i - 1));
                (*o).len = n as u32 - 1;
            }
            if n == 1 {
                dec(o as Word);
                return (None, None);
            }
            return (Some(o as Word), Some(leaf_key(o, n - 2)));
        }
        let i = branch_find(layouts, o, k);
        let (below, below_max) = self.take(layouts, child(o, i), k);
        let n = count(o);
        match below {
            Some(below) => {
                unsafe { set_word(o, i, below) };
                let below_max = below_max.expect("a node that remains has a greatest key");
                if child_max(o, i) != below_max {
                    inc(below_max);
                    dec(child_max(o, i));
                    unsafe { set_word(o, KEYS + i, below_max) };
                }
            }
            None => {
                dec(child_max(o, i));
                unsafe {
                    for j in i + 1..n {
                        set_word(o, j - 1, child(o, j));
                        set_word(o, KEYS + j - 1, child_max(o, j));
                    }
                    (*o).len = n as u32 - 1;
                }
                if n == 1 {
                    dec(o as Word);
                    return (None, None);
                }
            }
        }
        (Some(o as Word), Some(child_max(o, count(o) - 1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::{KIND_DEAD, Layouts, kind};
    use ply_span::Symbol;

    fn layouts() -> Layouts {
        Layouts::new(vec![(Symbol::new("Some"), 1), (Symbol::new("None"), 0)])
    }

    fn entries(m: Word) -> Vec<(i64, i64)> {
        to_vec(obj(m))
            .into_iter()
            .map(|(k, v)| (heap::as_int(k).unwrap(), heap::as_int(v).unwrap()))
            .collect()
    }

    #[test]
    fn inserts_in_any_order_read_back_sorted_across_leaves_and_branches() {
        let mut h = Heap::new();
        let l = layouts();
        let mut m = h.map_new();
        let n = 5000i64;
        for i in 0..n {
            let k = i * 7919 % 100003;
            m = h.map_insert(&l, m, heap::imm(k), heap::imm(i));
        }
        assert_eq!(len(obj(m)), n as usize);
        let got = entries(m);
        let mut want: Vec<(i64, i64)> = (0..n).map(|i| (i * 7919 % 100003, i)).collect();
        want.sort();
        assert_eq!(got, want);
        for (k, v) in &want {
            assert_eq!(
                heap::as_int(get(&l, obj(m), heap::imm(*k)).unwrap()),
                Some(*v)
            );
        }
        assert!(get(&l, obj(m), heap::imm(-1)).is_none());
        // A replaced key keeps the count and takes the value.
        m = h.map_insert(&l, m, heap::imm(want[10].0), heap::imm(-7));
        assert_eq!(len(obj(m)), n as usize);
        assert_eq!(
            heap::as_int(get(&l, obj(m), heap::imm(want[10].0)).unwrap()),
            Some(-7)
        );
        h.end();
    }

    #[test]
    fn a_shared_insert_copies_one_path_and_leaves_the_original_whole() {
        let mut h = Heap::new();
        let l = layouts();
        let mut m = h.map_new();
        for i in 0..3000i64 {
            m = h.map_insert(&l, m, heap::imm(i), heap::imm(i));
        }
        inc(m);
        let before = h.allocated();
        let other = h.map_insert(&l, m, heap::imm(100_000), heap::imm(1));
        let copied = h.allocated() - before;
        assert!(
            copied <= 6,
            "a shared insert copied {copied} nodes for a map of three levels"
        );
        assert_ne!(other, m);
        assert_eq!(len(obj(m)), 3000);
        assert_eq!(len(obj(other)), 3001);
        assert!(get(&l, obj(m), heap::imm(100_000)).is_none());
        assert!(get(&l, obj(other), heap::imm(100_000)).is_some());
        dec(other);
        assert_eq!(len(obj(m)), 3000);
        assert_eq!(entries(m).len(), 3000);
        h.end();
    }

    #[test]
    fn removals_empty_leaves_and_collapse_the_root_and_a_dying_map_lets_its_nodes_go() {
        let mut h = Heap::new();
        let l = layouts();
        let mut m = h.map_new();
        for i in 0..1000i64 {
            m = h.map_insert(&l, m, heap::imm(i), heap::imm(i));
        }
        for i in (0..1000i64).filter(|i| i % 3 != 0) {
            m = h.map_remove(&l, m, heap::imm(i));
        }
        let got = entries(m);
        let want: Vec<(i64, i64)> = (0..1000).filter(|i| i % 3 == 0).map(|i| (i, i)).collect();
        assert_eq!(got, want);
        for i in (0..1000i64).filter(|i| i % 3 == 0) {
            m = h.map_remove(&l, m, heap::imm(i));
        }
        assert_eq!(len(obj(m)), 0);
        assert_eq!(root(obj(m)), 0);
        m = h.map_insert(&l, m, heap::imm(5), heap::imm(5));
        let leaf = root(obj(m));
        dec(m);
        assert_eq!(kind(leaf), KIND_DEAD);
        h.end();
    }
}
