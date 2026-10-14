//! The compiled `Columns`: an array of records holding each field apart. The object's words are
//! one array per field, in its records' shape's order, each holding that field of every record as an
//! array of its values holds them; `layout` names the shape. It holds at least one field, and its
//! first array's length is its own.

use crate::array;
use crate::heap::{
    self, Heap, KIND_COLUMNS, KIND_RECORD, Layouts, Obj, Word, dec, field_word, inc, is_unique,
    obj, set_word, word_at,
};

/// How many records it holds.
pub fn len(o: *mut Obj) -> usize {
    array::len(column(o, 0))
}

/// The array holding field `k` of every record.
pub fn column(o: *mut Obj, k: usize) -> *mut Obj {
    obj(unsafe { word_at(o, k) })
}

/// Record `i`, made of each field's element `i`.
pub fn record(heap: &mut Heap, layouts: &Layouts, o: *mut Obj, i: usize) -> Word {
    let shape = unsafe { (*o).layout };
    let words: Vec<Word> = (0..layouts.shape_names(shape).len())
        .map(|k| array::element(heap, layouts, column(o, k), i))
        .collect();
    heap.record_shaped(layouts, shape, &words)
}

/// Every record, each made of its fields' elements.
pub fn records(heap: &mut Heap, layouts: &Layouts, o: *mut Obj) -> Vec<Word> {
    (0..len(o)).map(|i| record(heap, layouts, o, i)).collect()
}

/// The shape every one of `items` is a record of, where they share one of at least one field.
fn shared_shape(layouts: &Layouts, items: &[Word]) -> Option<u32> {
    let first = *items.first()?;
    if heap::kind(first) != KIND_RECORD {
        return None;
    }
    let shape = unsafe { (*obj(first)).layout };
    let same = items
        .iter()
        .all(|w| heap::kind(*w) == KIND_RECORD && unsafe { (*obj(*w)).layout } == shape);
    (same && layouts.shape_fields(shape) > 0).then_some(shape)
}

impl Heap {
    /// `items`, which it takes, held in columns, where they are records of one shape of at least one
    /// field; otherwise `None`, taking nothing.
    pub fn columns_of(&mut self, layouts: &Layouts, items: &[Word]) -> Option<Word> {
        let shape = shared_shape(layouts, items)?;
        let (offsets, slots) = (layouts.shape_offsets(shape), layouts.shape_slots(shape));
        let columns: Vec<Word> = (0..slots.len())
            .map(|k| {
                let values: Vec<Word> = items
                    .iter()
                    .map(|w| field_word(self, layouts, obj(*w), offsets[k], slots[k]))
                    .collect();
                self.array_of(layouts, &values)
            })
            .collect();
        for w in items {
            dec(*w);
        }
        Some(self.columns_from(shape, &columns))
    }

    /// Records of `shape` held as `columns`, which it takes, one per field in the shape's order.
    pub fn columns_from(&mut self, shape: u32, columns: &[Word]) -> Word {
        let o = self.alloc(KIND_COLUMNS, 0, columns.len() as u32, shape);
        for (k, c) in columns.iter().enumerate() {
            unsafe { set_word(o, k, *c) };
        }
        o as Word
    }

    /// `xs` with in-range record `i` replaced by `v`, a record of its shape, field by field: in
    /// place where `xs` and the arrays are unshared. Takes both.
    pub fn columns_set(&mut self, layouts: &Layouts, xs: Word, i: usize, v: Word) -> Word {
        let o = if is_unique(xs) {
            obj(xs)
        } else {
            let (shape, n) = unsafe { ((*obj(xs)).layout, (*obj(xs)).len as usize) };
            let columns: Vec<Word> = (0..n)
                .map(|k| {
                    let c = unsafe { word_at(obj(xs), k) };
                    inc(c);
                    c
                })
                .collect();
            dec(xs);
            obj(self.columns_from(shape, &columns))
        };
        let shape = unsafe { (*o).layout };
        let (offsets, slots) = (layouts.shape_offsets(shape), layouts.shape_slots(shape));
        for k in 0..slots.len() {
            let x = field_word(self, layouts, obj(v), offsets[k], slots[k]);
            let set = self.array_set(layouts, unsafe { word_at(o, k) }, i, x);
            unsafe { set_word(o, k, set) };
        }
        dec(v);
        o as Word
    }
}
