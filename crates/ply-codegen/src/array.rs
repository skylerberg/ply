//! The compiled array: its elements laid out in the object's own payload, so an index is a load.

use crate::heap::{self, Heap, KIND_ARRAY, Obj, Word, dec, inc, is_unique, obj, set_word, word_at};

pub fn len(o: *mut Obj) -> usize {
    unsafe { (*o).len as usize }
}

/// The elements, borrowed.
pub fn items<'a>(o: *mut Obj) -> &'a [Word] {
    unsafe { std::slice::from_raw_parts(heap::words(o), len(o)) }
}

impl Heap {
    /// An array of `items`, which it takes.
    pub fn array_from(&mut self, items: &[Word]) -> Word {
        let o = self.alloc(KIND_ARRAY, 0, items.len() as u32, 0);
        unsafe { std::ptr::copy_nonoverlapping(items.as_ptr(), heap::words(o), items.len()) };
        o as Word
    }

    /// `n` holders of `x`, which it takes.
    pub fn array_new(&mut self, n: usize, x: Word) -> Word {
        let o = self.alloc(KIND_ARRAY, 0, n as u32, 0);
        for i in 0..n {
            if i > 0 {
                inc(x);
            }
            unsafe { set_word(o, i, x) };
        }
        if n == 0 {
            dec(x);
        }
        o as Word
    }

    /// `xs` with in-range element `i` replaced by `v`, in place when unshared. Takes both.
    pub fn array_set(&mut self, xs: Word, i: usize, v: Word) -> Word {
        let in_place = is_unique(xs);
        let o = if in_place {
            obj(xs)
        } else {
            let held = items(obj(xs));
            for w in held {
                inc(*w);
            }
            let copy = obj(self.array_from(held));
            dec(xs);
            copy
        };
        ply_eval::rc::note_update_of(
            in_place,
            if in_place { 0 } else { len(o) },
            ply_eval::Span::DUMMY,
        );
        debug_assert!(i < len(o));
        let old = unsafe { word_at(o, i) };
        unsafe { set_word(o, i, v) };
        dec(old);
        o as Word
    }
}
