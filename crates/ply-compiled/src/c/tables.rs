//! The tables a loaded unit's C indexes: its constant pool, field names, builtins and layouts, each
//! in the order the unit recorded it, which is the order its C was emitted against.

use crate::heap::{Layouts, RecordLayout};
use ply_core::{Builtin, Symbol, Value};

/// What a unit holds that is not code, each entry at the position its C names it by.
pub struct UnitTables {
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<Builtin>,
    pub layouts: Layouts,
}

impl UnitTables {
    /// The tables as a unit recorded them, positions as given; `None` if a shape would not
    /// intern to its position, since those are the ids its C bakes.
    pub fn from_tables(
        ctors: Vec<(Symbol, usize, Vec<Symbol>)>,
        consts: Vec<Value>,
        fields: Vec<Symbol>,
        builtins: Vec<Builtin>,
        shapes: &[Vec<Symbol>],
        layout: RecordLayout,
    ) -> Option<UnitTables> {
        let (table, args): (Vec<_>, Vec<_>) = ctors
            .into_iter()
            .map(|(name, arity, args)| ((name, arity), args))
            .unzip();
        let mut layouts = Layouts::of(table, shapes, layout);
        let placed = layouts.shape_count() >= shapes.len()
            && shapes
                .iter()
                .enumerate()
                .all(|(id, names)| layouts.shape_spelled(id as u32)[..] == names[..]);
        if !placed {
            return None;
        }
        layouts.set_ctor_args(&args);
        Some(UnitTables {
            consts,
            fields,
            builtins,
            layouts,
        })
    }
}

/// The id a body's failure sites name their root by: a digest of its name, so a body's C does not
/// depend on which other roots the unit holds. Non-negative, since the C stores it in an `int64_t`.
pub fn root_id(name: &str) -> u64 {
    let hash = blake3::hash(name.as_bytes());
    let bytes: [u8; 8] = hash.as_bytes()[..8]
        .try_into()
        .expect("a digest has eight bytes");
    u64::from_le_bytes(bytes) >> 1
}
