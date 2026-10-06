//! The compiled map: a B-tree over words, keyed by [`heap::cmp_words`] so iteration matches a
//! `Value::Map`'s. Writes go in place along what is held once, else copy one node per level. A
//! branch counts the entries under each child, so an entry's place in key order is found, and a map
//! cut at one, without comparing or walking what lies beside the path.

use crate::heap::{
    self, Heap, KIND_MAP, KIND_MBRANCH, KIND_MLEAF, Layouts, Obj, Word, dec, inc, is_unique, obj,
    set_word, word_at,
};
use std::cmp::Ordering;

/// Pairs per leaf, and children per branch.
pub const WIDTH: usize = 32;

/// A branch's payload: children (room for `WIDTH + 1` before a split), then from `KEYS` each
/// child's greatest key, then from `SIZES` the entries under each child, as immediates.
pub const KEYS: usize = WIDTH + 1;
pub const SIZES: usize = 2 * KEYS;
pub const BRANCH_BYTES: usize = 3 * KEYS * 8;

pub fn root(m: *mut Obj) -> Word {
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

fn child_size(branch: *mut Obj, i: usize) -> usize {
    heap::imm_value(unsafe { word_at(branch, SIZES + i) }) as usize
}

fn set_child_size(branch: *mut Obj, i: usize, entries: usize) {
    unsafe { set_word(branch, SIZES + i, heap::imm(entries as i64)) }
}

/// Child `from` of `source` becomes child `to` of `target`, its key and size with it: nothing is
/// counted again.
fn move_child(source: *mut Obj, from: usize, target: *mut Obj, to: usize) {
    unsafe {
        set_word(target, to, child(source, from));
        set_word(target, KEYS + to, child_max(source, from));
        set_word(target, SIZES + to, word_at(source, SIZES + from));
    }
}

/// The greatest key under a node.
fn max_key(node: *mut Obj) -> Word {
    if is_leaf(node) {
        leaf_key(node, count(node) - 1)
    } else {
        child_max(node, count(node) - 1)
    }
}

/// The entries under a node.
fn entries(node: *mut Obj) -> usize {
    if is_leaf(node) {
        count(node)
    } else {
        (0..count(node)).map(|i| child_size(node, i)).sum()
    }
}

/// The word ranges a node's children occupy: a leaf's pairs, or a branch's children and keys.
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

/// The child that holds `k` or would: the first whose greatest key is `>= k`, else the last.
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

/// How many keys lie below `k`, and the value at `k`, borrowed.
pub fn locate(layouts: &Layouts, m: *mut Obj, k: Word) -> (usize, Option<Word>) {
    let mut node = root(m);
    if node == 0 {
        return (0, None);
    }
    let mut below = 0;
    loop {
        let o = obj(node);
        if is_leaf(o) {
            return match leaf_find(layouts, o, k) {
                Ok(i) => (below + i, Some(leaf_value(o, i))),
                Err(i) => (below + i, None),
            };
        }
        let i = branch_find(layouts, o, k);
        below += (0..i).map(|j| child_size(o, j)).sum::<usize>();
        node = child(o, i);
    }
}

/// The entry at `index` of the key order, borrowed.
pub fn at(m: *mut Obj, index: usize) -> Option<(Word, Word)> {
    if index >= len(m) {
        return None;
    }
    let (mut node, mut index) = (obj(root(m)), index);
    while !is_leaf(node) {
        let mut i = 0;
        while index >= child_size(node, i) {
            index -= child_size(node, i);
            i += 1;
            debug_assert!(
                i < count(node),
                "a branch's sizes fall short of its map's length"
            );
        }
        node = obj(child(node, i));
    }
    Some((leaf_key(node, index), leaf_value(node, index)))
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

/// `f` on at most `most` entries from the one at `start` on, in key order, borrowed: what lies
/// before `start` is stepped over a child at a time.
pub fn for_each_from<F: FnMut(Word, Word)>(m: *mut Obj, start: usize, most: usize, mut f: F) {
    let r = root(m);
    let mut most = most;
    if r != 0 {
        walk_from(obj(r), start, &mut most, &mut f);
    }
}

fn walk_from<F: FnMut(Word, Word)>(node: *mut Obj, skip: usize, most: &mut usize, f: &mut F) {
    if is_leaf(node) {
        for i in skip..count(node) {
            if *most == 0 {
                return;
            }
            f(leaf_key(node, i), leaf_value(node, i));
            *most -= 1;
        }
        return;
    }
    let mut skip = skip;
    for i in 0..count(node) {
        if *most == 0 {
            return;
        }
        let size = child_size(node, i);
        if skip >= size {
            skip -= size;
            continue;
        }
        walk_from(obj(child(node, i)), skip, most, f);
        skip = 0;
    }
}

/// Every entry in key order, borrowed.
pub fn to_vec(m: *mut Obj) -> Vec<(Word, Word)> {
    let mut out = Vec::with_capacity(len(m));
    for_each(m, |k, v| out.push((k, v)));
    out
}

/// An insert's result: the node to hold now, its greatest key, and a right sibling if it split.
struct Inserted {
    node: Word,
    max: Word,
    split: Option<(Word, Word)>,
    added: bool,
}

/// The entry a removal takes.
#[derive(Clone, Copy)]
enum Which {
    Key(Word),
    Least,
    Greatest,
}

/// A removal's result: the node without the entry and its greatest key, or neither when the entry
/// was its last, and the entry, each word the caller's; no entry when the key was absent.
struct Taken {
    node: Option<Word>,
    max: Option<Word>,
    entry: Option<(Word, Word)>,
}

/// The root a map holds a tree by: a branch of one child is that child, and no node is none.
fn settled(node: Option<Word>) -> Word {
    let Some(mut node) = node else {
        return 0;
    };
    while !is_leaf(obj(node)) && count(obj(node)) == 1 {
        let only = child(obj(node), 0);
        inc(only);
        dec(node);
        node = only;
    }
    node
}

impl Heap {
    pub fn map_new(&mut self) -> Word {
        self.map_over(0, 0) as Word
    }

    /// A map of `entries` entries under `root`, which it takes.
    fn map_over(&mut self, root: Word, entries: usize) -> *mut Obj {
        let m = self.raw_alloc(KIND_MAP, 0, entries as u32, 0, 8);
        unsafe { set_word(m, 0, root) };
        m
    }

    fn alloc_leaf(&mut self) -> *mut Obj {
        self.raw_alloc(KIND_MLEAF, 0, 0, WIDTH as u32, WIDTH * 16)
    }

    fn alloc_branch(&mut self) -> *mut Obj {
        self.raw_alloc(KIND_MBRANCH, 0, 0, WIDTH as u32, BRANCH_BYTES)
    }

    /// A map over sorted, distinct entries, which it takes, built bottom-up.
    pub fn map_from_sorted(&mut self, entries: &[(Word, Word)]) -> Word {
        let mw = self.map_new();
        let m = obj(mw);
        if entries.is_empty() {
            return mw;
        }
        let mut level: Vec<(Word, usize)> = Vec::with_capacity(entries.len().div_ceil(WIDTH));
        for run in entries.chunks(WIDTH) {
            let leaf = self.alloc_leaf();
            for (i, (k, v)) in run.iter().enumerate() {
                unsafe {
                    set_word(leaf, 2 * i, *k);
                    set_word(leaf, 2 * i + 1, *v);
                }
            }
            unsafe { (*leaf).len = run.len() as u32 };
            level.push((leaf as Word, run.len()));
        }
        while level.len() > 1 {
            let mut above = Vec::with_capacity(level.len().div_ceil(WIDTH));
            for run in level.chunks(WIDTH) {
                let branch = self.alloc_branch();
                for (i, (node, size)) in run.iter().enumerate() {
                    let max = max_key(obj(*node));
                    inc(max);
                    unsafe {
                        set_word(branch, i, *node);
                        set_word(branch, KEYS + i, max);
                    }
                    set_child_size(branch, i, *size);
                }
                unsafe { (*branch).len = run.len() as u32 };
                above.push((branch as Word, run.iter().map(|(_, size)| size).sum()));
            }
            level = above;
        }
        unsafe {
            set_word(m, 0, level[0].0);
            (*m).len = entries.len() as u32;
        }
        mw
    }

    /// Itself when held once, else a copy holding its children and keys once more.
    fn writable_node(&mut self, node: Word) -> *mut Obj {
        if is_unique(node) {
            return obj(node);
        }
        let o = obj(node);
        let copy = if is_leaf(o) {
            self.alloc_leaf()
        } else {
            let copy = self.alloc_branch();
            for i in 0..count(o) {
                set_child_size(copy, i, child_size(o, i));
            }
            copy
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

    /// Itself when held once, else a copy holding the root once more.
    fn writable_map(&mut self, m: Word) -> *mut Obj {
        if is_unique(m) {
            return obj(m);
        }
        let o = obj(m);
        let r = root(o);
        if r != 0 {
            inc(r);
        }
        let copy = self.map_over(r, len(o));
        dec(m);
        copy
    }

    /// `map_insert`, replacing key and value when present. Takes all three.
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
                set_child_size(branch, 0, entries(obj(put.node)));
                set_child_size(branch, 1, entries(obj(right)));
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

    fn put(&mut self, layouts: &Layouts, node: Word, k: Word, v: Word) -> Inserted {
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
        let grown = child_size(o, i) + usize::from(below.added);
        match below.split {
            Some((right, right_max)) => {
                let moved = entries(obj(right));
                set_child_size(o, i, grown - moved);
                self.branch_insert(o, i + 1, right, right_max, moved);
            }
            None => set_child_size(o, i, grown),
        }
        let n = count(o);
        if n <= WIDTH {
            return Inserted {
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
            move_child(o, j, right, j - half);
        }
        unsafe {
            (*right).len = (n - half) as u32;
            (*o).len = half as u32;
        }
        Inserted {
            node: o as Word,
            max: child_max(o, half - 1),
            split: Some((right as Word, child_max(right, n - half - 1))),
            added: below.added,
        }
    }

    /// `right`, its greatest key and its entries go in at `i` of a branch that is not full past one
    /// over.
    fn branch_insert(
        &mut self,
        branch: *mut Obj,
        i: usize,
        right: Word,
        right_max: Word,
        size: usize,
    ) {
        let n = count(branch);
        inc(right_max);
        for j in (i..n).rev() {
            move_child(branch, j, branch, j + 1);
        }
        unsafe {
            set_word(branch, i, right);
            set_word(branch, KEYS + i, right_max);
            (*branch).len = n as u32 + 1;
        }
        set_child_size(branch, i, size);
    }

    fn put_leaf(&mut self, layouts: &Layouts, leaf: *mut Obj, k: Word, v: Word) -> Inserted {
        let n = count(leaf);
        match leaf_find(layouts, leaf, k) {
            Ok(i) => {
                unsafe {
                    dec(leaf_key(leaf, i));
                    dec(leaf_value(leaf, i));
                    set_word(leaf, 2 * i, k);
                    set_word(leaf, 2 * i + 1, v);
                }
                Inserted {
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
                Inserted {
                    node: leaf as Word,
                    max: leaf_key(leaf, n),
                    split: None,
                    added: true,
                }
            }
            Err(i) => {
                // Full: split, and insert into whichever half the entry falls in.
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
                Inserted {
                    node: leaf as Word,
                    max: leaf_key(leaf, count(leaf) - 1),
                    split: Some((right as Word, leaf_key(right, count(right) - 1))),
                    added: true,
                }
            }
        }
    }

    /// `map_remove`. Takes the map, reads the key.
    pub fn map_remove(&mut self, layouts: &Layouts, m: Word, k: Word) -> Word {
        if get(layouts, obj(m), k).is_none() {
            return m;
        }
        let (m, entry) = self.map_take(layouts, m, Which::Key(k));
        if let Some((key, value)) = entry {
            dec(key);
            dec(value);
        }
        m
    }

    /// The map without its least entry, or its greatest, and that entry, each word the caller's;
    /// the map as it was and no entry when it is empty. Takes the map.
    pub fn map_pop(
        &mut self,
        layouts: &Layouts,
        m: Word,
        greatest: bool,
    ) -> (Word, Option<(Word, Word)>) {
        if root(obj(m)) == 0 {
            return (m, None);
        }
        let which = if greatest {
            Which::Greatest
        } else {
            Which::Least
        };
        self.map_take(layouts, m, which)
    }

    /// The map without the entry `which` names, and the entry. Takes the map, which is not empty.
    fn map_take(
        &mut self,
        layouts: &Layouts,
        m: Word,
        which: Which,
    ) -> (Word, Option<(Word, Word)>) {
        let m = self.writable_map(m);
        let taken = self.take(layouts, root(m), which);
        unsafe {
            set_word(m, 0, settled(taken.node));
            if taken.entry.is_some() {
                (*m).len -= 1;
            }
        }
        (m as Word, taken.entry)
    }

    fn take(&mut self, layouts: &Layouts, node: Word, which: Which) -> Taken {
        let o = self.writable_node(node);
        let n = count(o);
        if is_leaf(o) {
            let i = match which {
                Which::Key(k) => match leaf_find(layouts, o, k) {
                    Ok(i) => i,
                    Err(_) => {
                        return Taken {
                            node: Some(o as Word),
                            max: Some(leaf_key(o, n - 1)),
                            entry: None,
                        };
                    }
                },
                Which::Least => 0,
                Which::Greatest => n - 1,
            };
            let entry = Some((leaf_key(o, i), leaf_value(o, i)));
            unsafe {
                let base = heap::words(o);
                std::ptr::copy(base.add(2 * i + 2), base.add(2 * i), 2 * (n - i - 1));
                (*o).len = n as u32 - 1;
            }
            if n == 1 {
                dec(o as Word);
                return Taken {
                    node: None,
                    max: None,
                    entry,
                };
            }
            return Taken {
                node: Some(o as Word),
                max: Some(leaf_key(o, n - 2)),
                entry,
            };
        }
        let i = match which {
            Which::Key(k) => branch_find(layouts, o, k),
            Which::Least => 0,
            Which::Greatest => n - 1,
        };
        let below = self.take(layouts, child(o, i), which);
        match below.node {
            Some(node) => {
                unsafe { set_word(o, i, node) };
                let max = below.max.expect("a node that remains has a greatest key");
                if child_max(o, i) != max {
                    inc(max);
                    dec(child_max(o, i));
                    unsafe { set_word(o, KEYS + i, max) };
                }
                if below.entry.is_some() {
                    set_child_size(o, i, child_size(o, i) - 1);
                }
            }
            None => {
                dec(child_max(o, i));
                for j in i + 1..n {
                    move_child(o, j, o, j - 1);
                }
                unsafe { (*o).len = n as u32 - 1 };
                if n == 1 {
                    dec(o as Word);
                    return Taken {
                        node: None,
                        max: None,
                        entry: below.entry,
                    };
                }
            }
        }
        Taken {
            node: Some(o as Word),
            max: Some(child_max(o, count(o) - 1)),
            entry: below.entry,
        }
    }

    /// The map of the entries below `k`, the value at `k`, which is the caller's, and the map of
    /// the entries above it. Takes the map, reads the key.
    pub fn map_split(&mut self, layouts: &Layouts, m: Word, k: Word) -> (Word, Option<Word>, Word) {
        let (below, found) = locate(layouts, obj(m), k);
        let (lower, rest) = self.map_cut(m, below);
        if found.is_none() {
            return (lower, None, rest);
        }
        let (upper, entry) = self.map_pop(layouts, rest, false);
        let (key, value) = entry.expect("the key located is the first past the cut");
        dec(key);
        (lower, Some(value), upper)
    }

    /// The map of the first `index` entries and the map of the rest. Takes the map.
    pub fn map_cut(&mut self, m: Word, index: usize) -> (Word, Word) {
        let total = len(obj(m));
        if index == 0 {
            return (self.map_new(), m);
        }
        if index >= total {
            return (m, self.map_new());
        }
        let m = self.writable_map(m);
        let (lower, upper) = self.cut(root(m), index);
        let rest = self.map_over(settled(Some(upper)), total - index);
        unsafe {
            set_word(m, 0, settled(Some(lower)));
            (*m).len = index as u32;
        }
        (m as Word, rest as Word)
    }

    /// The node of the first `index` entries under `node` and the node of the rest, where both
    /// hold one: the first is `node` when it is held once. Takes the node.
    fn cut(&mut self, node: Word, index: usize) -> (Word, Word) {
        let o = self.writable_node(node);
        let n = count(o);
        if is_leaf(o) {
            let right = self.alloc_leaf();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    heap::words(o).add(2 * index),
                    heap::words(right),
                    2 * (n - index),
                );
                (*right).len = (n - index) as u32;
                (*o).len = index as u32;
            }
            return (o as Word, right as Word);
        }
        let (mut i, mut before) = (0, 0);
        while before + child_size(o, i) <= index {
            before += child_size(o, i);
            i += 1;
        }
        let right = self.alloc_branch();
        let mut kept = 0;
        if index > before {
            // The cut falls inside child `i`: its upper part leads the right node under the key
            // the child had, and its lower part stays under its own.
            let size = child_size(o, i);
            let (lower, upper) = self.cut(child(o, i), index - before);
            let max = max_key(obj(lower));
            inc(max);
            unsafe {
                set_word(right, 0, upper);
                set_word(right, KEYS, child_max(o, i));
                set_word(o, i, lower);
                set_word(o, KEYS + i, max);
            }
            set_child_size(right, 0, size - (index - before));
            set_child_size(o, i, index - before);
            kept = 1;
            i += 1;
        }
        for j in i..n {
            move_child(o, j, right, kept + j - i);
        }
        unsafe {
            (*right).len = (kept + n - i) as u32;
            (*o).len = i as u32;
        }
        (o as Word, right as Word)
    }
}
