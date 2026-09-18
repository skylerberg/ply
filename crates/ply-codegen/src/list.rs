//! The compiled list, `ply_eval::list`'s shape over words: a radix trie of `WIDTH`-wide nodes plus
//! an inline tail, with a dropped-prefix offset so `[x, ..rest]` shares the trie.

use crate::heap::{
    self, Heap, KIND_BRANCH, KIND_LEAF, KIND_LIST, Obj, Word, dec, inc, is_unique, obj, set_word,
    word_at,
};

/// Elements per leaf, children per branch, and the most a tail holds.
pub const WIDTH: usize = 32;
const BITS: u32 = 5;
const MASK: usize = WIDTH - 1;

/// The payload: the trie's root (zero for none), then the tail.
const ROOT: usize = 0;
pub const TAIL: usize = 1;

pub fn root(o: *mut Obj) -> Word {
    unsafe { word_at(o, ROOT) }
}

/// The prefix a `rest` dropped: the list's first element is at this physical index.
fn start(o: *mut Obj) -> usize {
    unsafe { (*o).layout as usize }
}

pub fn tail_len(o: *mut Obj) -> usize {
    unsafe { (*o).flags as usize }
}

fn set_start(o: *mut Obj, v: usize) {
    unsafe { (*o).layout = v as u32 }
}

fn set_tail_len(o: *mut Obj, v: usize) {
    unsafe { (*o).flags = v as u8 }
}

/// The list's length: what `len` answers.
pub fn len(o: *mut Obj) -> usize {
    unsafe { (*o).len as usize }
}

/// The tail's capacity in words.
fn cap(o: *mut Obj) -> usize {
    unsafe { (*o).aux as usize }
}

/// The physical index of the first tail element: how many elements the trie holds.
fn tail_offset(o: *mut Obj) -> usize {
    start(o) + len(o) - tail_len(o)
}

/// The tail's elements, borrowed.
pub fn tail<'a>(o: *mut Obj) -> &'a [Word] {
    unsafe { std::slice::from_raw_parts(heap::words(o).add(TAIL), tail_len(o)) }
}

/// A node's children, borrowed: a leaf's elements or a branch's subtrees.
pub fn kids<'a>(node: *mut Obj) -> &'a [Word] {
    unsafe { std::slice::from_raw_parts(heap::words(node), (*node).len as usize) }
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

impl Heap {
    /// A fresh, empty list with room for `cap` tail elements.
    pub fn alloc_list(&mut self, cap: u32) -> *mut Obj {
        let cap = (cap as usize).clamp(1, WIDTH);
        let o = self.raw_alloc(KIND_LIST, 0, 0, 0, (TAIL + cap) * 8);
        unsafe {
            (*o).aux = cap as u16;
            set_word(o, ROOT, 0);
        }
        o
    }

    fn alloc_node(&mut self, kind: u8, items: &[Word]) -> Word {
        let o = self.raw_alloc(kind, 0, items.len() as u32, WIDTH as u32, WIDTH * 8);
        for (i, w) in items.iter().enumerate() {
            unsafe { set_word(o, i, *w) };
        }
        o as Word
    }

    /// A list of `items`, which it takes.
    pub fn list_from(&mut self, items: &[Word]) -> Word {
        // Sized exactly: pushes grow the tail by doubling.
        if items.len() <= WIDTH {
            let o = self.alloc_list(items.len() as u32);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    items.as_ptr(),
                    heap::words(o).add(TAIL),
                    items.len(),
                );
                (*o).len = items.len() as u32;
            }
            set_tail_len(o, items.len());
            return o as Word;
        }
        let full = items.len() / WIDTH * WIDTH;
        let rest = &items[full..];
        let o = self.alloc_list(if full == 0 { rest.len() } else { WIDTH } as u32);
        let mut root = 0;
        for (i, leaf) in items[..full].chunks(WIDTH).enumerate() {
            let leaf = self.alloc_node(KIND_LEAF, leaf);
            root = self.push_leaf(root, i * WIDTH, leaf);
        }
        unsafe {
            set_word(o, ROOT, root);
            for (i, w) in rest.iter().enumerate() {
                set_word(o, TAIL + i, *w);
            }
            (*o).len = items.len() as u32;
        }
        set_tail_len(o, rest.len());
        o as Word
    }

    /// `leaf` under `shift / BITS` single-child branches.
    fn path(&mut self, leaf: Word, shift: u32) -> Word {
        if shift == 0 {
            leaf
        } else {
            let below = self.path(leaf, shift - BITS);
            self.alloc_node(KIND_BRANCH, &[below])
        }
    }

    /// Appends a full leaf to a trie of `count` elements. Takes `root` and `leaf`.
    fn push_leaf(&mut self, root: Word, count: usize, leaf: Word) -> Word {
        if root == 0 {
            return leaf;
        }
        let shift = shift_for(count);
        if count == WIDTH << shift {
            let right = self.path(leaf, shift);
            return self.alloc_node(KIND_BRANCH, &[root, right]);
        }
        self.insert(root, shift, count, leaf)
    }

    /// `node` with `leaf` inserted at `index`, in place when held once; hold the node answered.
    fn insert(&mut self, node: Word, shift: u32, index: usize, leaf: Word) -> Word {
        let node = self.writable(node);
        let o = obj(node);
        let k = (index >> shift) & MASK;
        let below = shift - BITS;
        let n = unsafe { (*o).len } as usize;
        if k < n {
            let child = unsafe { word_at(o, k) };
            let child = self.insert(child, below, index, leaf);
            unsafe { set_word(o, k, child) };
        } else {
            let child = self.path(leaf, below);
            unsafe {
                set_word(o, n, child);
                (*o).len = n as u32 + 1;
            }
        }
        node
    }

    /// Itself when held once, else a copy holding its children once more (the original released).
    fn writable(&mut self, node: Word) -> Word {
        if is_unique(node) {
            return node;
        }
        let o = obj(node);
        let items = kids(o);
        for w in items {
            inc(*w);
        }
        let copy = self.alloc_node(unsafe { (*o).kind }, items);
        dec(node);
        copy
    }

    /// A copy of `xs` with room for `cap` tail elements, holding its contents once more.
    fn clone_list(&mut self, xs: *mut Obj, cap: usize) -> *mut Obj {
        let out = self.alloc_list(cap as u32);
        let r = root(xs);
        if r != 0 {
            inc(r);
        }
        unsafe {
            set_word(out, ROOT, r);
            (*out).len = (*xs).len;
        }
        set_start(out, start(xs));
        let items = tail(xs);
        for (i, w) in items.iter().enumerate() {
            inc(*w);
            unsafe { set_word(out, TAIL + i, *w) };
        }
        set_tail_len(out, items.len());
        out
    }

    /// `xs` with `x` appended. Takes both; an unshared list grows in place.
    pub fn list_push(&mut self, xs: Word, x: Word) -> Word {
        let mut o = obj(xs);
        let n = tail_len(o);
        // The machine's reuse counters, which `ply run --json` reports.
        let in_place = is_unique(xs);
        ply_eval::rc::note_update_of(
            in_place,
            if in_place { 0 } else { n },
            ply_span::Span::DUMMY,
        );
        if !is_unique(xs) || (n == cap(o) && n < WIDTH) {
            let room = if n == cap(o) {
                (cap(o) * 2).min(WIDTH)
            } else {
                cap(o)
            };
            let copy = self.clone_list(o, room);
            dec(xs);
            o = copy;
        }
        let n = tail_len(o);
        if n == WIDTH {
            let leaf = self.alloc_node(KIND_LEAF, tail(o));
            let r = self.push_leaf(root(o), tail_offset(o), leaf);
            unsafe { set_word(o, ROOT, r) };
            set_tail_len(o, 0);
        }
        let n = tail_len(o);
        unsafe {
            set_word(o, TAIL + n, x);
            (*o).len += 1;
        }
        set_tail_len(o, n + 1);
        o as Word
    }

    /// `xs` with in-range element `i` replaced by `v`, in place when unshared. Takes both.
    pub fn list_set(&mut self, xs: Word, i: usize, v: Word) -> Word {
        let mut o = obj(xs);
        debug_assert!(i < len(o));
        let mut copied: Option<usize> = None;
        if !is_unique(xs) {
            copied = Some(tail_len(o));
            let copy = self.clone_list(o, cap(o));
            dec(xs);
            o = copy;
        }
        let p = start(o) + i;
        let offset = tail_offset(o);
        let (node, at) = if p >= offset {
            (o, TAIL + p - offset)
        } else {
            let r = self.owned(root(o), &mut copied);
            unsafe { set_word(o, ROOT, r) };
            let mut node = obj(r);
            let mut shift = shift_for(offset);
            while shift > 0 {
                let k = (p >> shift) & MASK;
                let child = self.owned(unsafe { word_at(node, k) }, &mut copied);
                unsafe { set_word(node, k, child) };
                node = obj(child);
                shift -= BITS;
            }
            (node, p & MASK)
        };
        let old = unsafe { word_at(node, at) };
        unsafe { set_word(node, at, v) };
        dec(old);
        ply_eval::rc::note_update_of(copied.is_none(), copied.unwrap_or(0), ply_span::Span::DUMMY);
        o as Word
    }

    /// [`Heap::writable`], counting the children a shared node was copied for.
    fn owned(&mut self, node: Word, copied: &mut Option<usize>) -> Word {
        if !is_unique(node) {
            let n = unsafe { (*obj(node)).len } as usize;
            *copied = Some(copied.unwrap_or(0) + n);
        }
        self.writable(node)
    }

    /// The list without its first `k` elements, sharing the trie and copying at most the tail.
    /// Reads `xs`.
    pub fn list_skip(&mut self, xs: Word, k: usize) -> Word {
        let o = obj(xs);
        let k = k.min(len(o));
        let from = start(o) + k;
        let offset = tail_offset(o);
        if from >= offset {
            let items = &tail(o)[from - offset..];
            let out = self.alloc_list(items.len() as u32);
            for (i, w) in items.iter().enumerate() {
                inc(*w);
                unsafe { set_word(out, TAIL + i, *w) };
            }
            unsafe { (*out).len = items.len() as u32 };
            set_tail_len(out, items.len());
            return out as Word;
        }
        let out = self.clone_list(o, cap(o));
        set_start(out, from);
        unsafe { (*out).len -= k as u32 };
        out as Word
    }
}

/// The element at `i`, borrowed.
pub fn get(o: *mut Obj, i: usize) -> Word {
    debug_assert!(i < len(o));
    // A list without a trie has dropped no prefix: it is its tail.
    if root(o) == 0 {
        return unsafe { word_at(o, TAIL + i) };
    }
    let p = start(o) + i;
    let offset = tail_offset(o);
    if p >= offset {
        return tail(o)[p - offset];
    }
    let mut node = obj(root(o));
    let mut shift = shift_for(offset);
    while shift > 0 {
        node = obj(unsafe { word_at(node, (p >> shift) & MASK) });
        shift -= BITS;
    }
    unsafe { word_at(node, p & MASK) }
}

/// Every element in order, borrowed.
pub fn to_vec(o: *mut Obj) -> Vec<Word> {
    let mut out = Vec::with_capacity(len(o));
    for_each(o, &mut |w| out.push(w));
    out
}

/// `f` on every element in order; `f` may run user code, so the list must stay held throughout.
pub fn for_each<F: FnMut(Word)>(o: *mut Obj, mut f: F) {
    let r = root(o);
    if r != 0 {
        let mut skip = start(o);
        walk(obj(r), &mut skip, &mut f);
    }
    for w in tail(o) {
        f(*w);
    }
}

fn walk<F: FnMut(Word)>(node: *mut Obj, skip: &mut usize, f: &mut F) {
    let items = kids(node);
    if unsafe { (*node).kind } == KIND_LEAF {
        let from = (*skip).min(items.len());
        *skip -= from;
        for w in &items[from..] {
            f(*w);
        }
        return;
    }
    for child in items {
        walk(obj(*child), skip, f);
    }
}

/// The words a list holds, for a walk over its children: the root if any, then the tail.
pub fn children(o: *mut Obj) -> impl Iterator<Item = Word> {
    let r = root(o);
    (r != 0)
        .then_some(r)
        .into_iter()
        .chain(tail(o).iter().copied())
}

/// Whether the list's whole structure is held by this list alone.
pub fn unique_throughout(xs: Word) -> bool {
    fn unique_path(node: Word) -> bool {
        if !is_unique(node) {
            return false;
        }
        let o = obj(node);
        if unsafe { (*o).kind } == KIND_LEAF {
            return true;
        }
        kids(o).last().is_none_or(|k| unique_path(*k))
    }
    is_unique(xs) && {
        let r = root(obj(xs));
        r == 0 || unique_path(r)
    }
}
