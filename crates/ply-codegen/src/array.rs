//! The compiled array: its elements laid out in the object's own payload, so an index is a load.
//! An array of records or of scalars holds each in place, as field `e` of a run of one-field
//! records: `layout` names that record's shape and `aux` its words, the array's stride, and the
//! bitmap after the words says which of them it counts.

use crate::heap::{
    self, FLAT, Heap, KIND_ARRAY, KIND_RECORD, Layouts, MIXED, Obj, RAW, Slot, Word, counted, dec,
    field_word, inc, is_unique, obj, set_word, word_at,
};
use ply_eval::Symbol;

/// How many elements it holds.
pub fn len(o: *mut Obj) -> usize {
    unsafe {
        match (*o).aux {
            0 => (*o).len as usize,
            stride => (*o).len as usize / stride as usize,
        }
    }
}

/// Whether it holds its elements in place.
pub fn in_place(o: *mut Obj) -> bool {
    unsafe { (*o).aux != 0 }
}

/// The elements of an array holding them as words, borrowed.
pub fn items<'a>(o: *mut Obj) -> &'a [Word] {
    debug_assert!(!in_place(o));
    unsafe { std::slice::from_raw_parts(heap::words(o), len(o)) }
}

/// How an array holding its elements in place holds each, and where element `i` starts.
pub fn slot(layouts: &Layouts, o: *mut Obj) -> Slot {
    layouts.slot_at(unsafe { (*o).layout }, 0)
}

fn start(o: *mut Obj, i: usize) -> usize {
    i * unsafe { (*o).aux } as usize
}

/// Element `i`, held once more: its word, or as a record's field of its slot is read.
pub fn element(heap: &mut Heap, layouts: &Layouts, o: *mut Obj, i: usize) -> Word {
    if in_place(o) {
        field_word(heap, layouts, o, start(o, i), slot(layouts, o))
    } else {
        let w = unsafe { word_at(o, i) };
        inc(w);
        w
    }
}

/// Every element, each held once more.
pub fn elements(heap: &mut Heap, layouts: &Layouts, o: *mut Obj) -> Vec<Word> {
    (0..len(o)).map(|i| element(heap, layouts, o, i)).collect()
}

/// The field `e` an array holds elements of `w`'s kind in place as: a record of its shape, or a
/// scalar as its own words; `None` for any other element, which it holds as its word.
fn slot_of(layouts: &Layouts, w: Word) -> Option<Slot> {
    match heap::kind(w) {
        KIND_RECORD => Some(Slot::Record(unsafe { (*obj(w)).layout })),
        _ => heap::raw_slot_of(layouts, w),
    }
}

/// The shape of the one-field record an array holds elements of `slot` in place as.
fn element_shape(layouts: &Layouts, slot: Slot) -> u32 {
    let e = Symbol::new("e");
    layouts.shape(vec![match slot {
        Slot::Record(inner) => heap::spelled_record(&e, &layouts.shape_spelled(inner)),
        _ => heap::spelled(&e, slot),
    }])
}

impl Heap {
    /// An array of `items`, which it takes, holding them as words.
    pub fn array_from(&mut self, items: &[Word]) -> Word {
        let o = self.alloc(KIND_ARRAY, 0, items.len() as u32, 0);
        unsafe { std::ptr::copy_nonoverlapping(items.as_ptr(), heap::words(o), items.len()) };
        o as Word
    }

    /// An array of `items`, which it takes, laid out as an array of their type is wherever it is
    /// made: records and scalars in place.
    pub fn array_of(&mut self, layouts: &Layouts, items: &[Word]) -> Word {
        let Some(slot) = items
            .first()
            .and_then(|w| slot_of(layouts, *w))
            .filter(|s| items.iter().all(|w| slot_of(layouts, *w) == Some(*s)))
        else {
            return self.array_from(items);
        };
        let shape = element_shape(layouts, slot);
        let (stride, _) = layouts.shape_words(shape);
        let Ok(aux) = u16::try_from(stride) else {
            return self.array_from(items);
        };
        let counted_each = layouts.shape_counted(shape);
        let width = stride * items.len();
        let counted: Vec<usize> = (0..items.len())
            .flat_map(|k| counted_each.iter().map(move |c| k * stride + c))
            .collect();
        let tail = if counted.is_empty() {
            0
        } else {
            heap::tail_words(width)
        };
        let flags = RAW | if counted.is_empty() { FLAT } else { MIXED };
        let o = self.alloc(KIND_ARRAY, flags, width as u32, shape);
        unsafe {
            (*o).aux = aux;
            for (k, w) in items.iter().enumerate() {
                match slot {
                    Slot::Record(inner) => heap::embed(layouts, o, k * stride, inner, *w),
                    raw => heap::hold_raw(o, k * stride, raw, *w),
                }
            }
            heap::write_tail(o, width, tail, &counted);
        }
        o as Word
    }

    /// `n` holders of `x`, which it takes.
    pub fn array_new(&mut self, layouts: &Layouts, n: usize, x: Word) -> Word {
        for _ in 1..n {
            inc(x);
        }
        if n == 0 {
            dec(x);
        }
        self.array_of(layouts, &vec![x; n])
    }

    /// `xs` with in-range element `i` replaced by `v`, in place when unshared. Takes both.
    pub fn array_set(&mut self, layouts: &Layouts, xs: Word, i: usize, v: Word) -> Word {
        let in_place_now = is_unique(xs);
        let o = if in_place_now {
            obj(xs)
        } else {
            let copy = unsafe { copied(self, obj(xs)) };
            dec(xs);
            copy
        };
        ply_eval::rc::note_update_of(
            in_place_now,
            if in_place_now { 0 } else { len(o) },
            ply_eval::Span::DUMMY,
        );
        debug_assert!(i < len(o));
        unsafe {
            if in_place(o) {
                let at = start(o, i);
                match slot(layouts, o) {
                    Slot::Record(inner) => {
                        heap::drop_embedded(o, at);
                        heap::embed(layouts, o, at, inner, v);
                    }
                    Slot::Maybe => {
                        let old = word_at(o, at);
                        heap::hold_raw(o, at, Slot::Maybe, v);
                        dec(old);
                    }
                    raw => heap::hold_raw(o, at, raw, v),
                }
            } else {
                let old = word_at(o, i);
                set_word(o, i, v);
                dec(old);
            }
        }
        o as Word
    }
}

/// A fresh copy of array `o`, its words, its bitmap and what it counts held once more.
unsafe fn copied(heap: &mut Heap, o: *mut Obj) -> *mut Obj {
    unsafe {
        let (flags, words, layout, aux) = ((*o).flags, (*o).len as usize, (*o).layout, (*o).aux);
        let c = heap.alloc(KIND_ARRAY, flags, words as u32, layout);
        (*c).aux = aux;
        let tail = if flags & MIXED != 0 {
            heap::tail_words(words)
        } else {
            0
        };
        std::ptr::copy_nonoverlapping(heap::words(o), heap::words(c), words + tail);
        for i in 0..words {
            if counted(o, i) {
                inc(word_at(o, i));
            }
        }
        c
    }
}
