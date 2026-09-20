//! What a body names of the unit around it, and the unit's tables those names resolve into. A
//! unit's tables are sorted by content, so a body's resolved C is a function of what the unit
//! holds and never of the order its definitions were taken in.

use super::cache::encode_const;
use crate::heap::Layouts;
use ply_eval::{Builtin, Value};
use ply_span::Symbol;

/// What one body names of the unit around it, by the positions its own text uses.
#[derive(Default, Clone)]
pub struct Tables {
    pub consts: Vec<Value>,
    pub builtins: Vec<Builtin>,
    pub fields: Vec<Symbol>,
    pub shapes: Vec<Vec<Symbol>>,
    /// Every definition this body calls, so a cached body is checked against the fixpoint's set.
    pub calls: Vec<String>,
    /// The C symbols of the lambda entries this body defines, in the order it met them.
    pub lambdas: Vec<String>,
    /// Effects performed and handled, under their program-wide names.
    pub performs: Vec<String>,
    pub handles: Vec<String>,
}

/// What an emitted unit accumulates that is not code: each table sorted and duplicate-free, so
/// an entry's position is its rank among the entries the unit holds.
pub struct Unit {
    /// By [`encode_const`] of the value.
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    /// By name.
    pub builtins: Vec<Builtin>,
    /// Each shape's field names sorted, the shapes sorted; a shape's id is its position.
    pub shapes: Vec<Vec<Symbol>>,
    pub layouts: Layouts,
    /// Lambda entry symbols; `rt_closure` and `rt_constant` take a position here.
    pub lambdas: Vec<String>,
}

fn ascending<T: PartialOrd>(xs: &[T]) -> bool {
    xs.windows(2).all(|w| w[0] < w[1])
}

impl Unit {
    /// The union of what `bodies` name and the lambda symbols given.
    pub fn of<'a>(
        ctors: Vec<(Symbol, usize)>,
        bodies: impl IntoIterator<Item = &'a Tables>,
        lambdas: impl IntoIterator<Item = String>,
    ) -> Unit {
        let mut consts: Vec<(String, Value)> = Vec::new();
        let mut fields: Vec<Symbol> = Vec::new();
        let mut builtins: Vec<Builtin> = Vec::new();
        let mut shapes: Vec<Vec<Symbol>> = vec![Layouts::entry_fields()];
        let mut lambdas: Vec<String> = lambdas.into_iter().collect();
        for t in bodies {
            consts.extend(t.consts.iter().map(|v| (encode_const(v), v.clone())));
            fields.extend(t.fields.iter().cloned());
            builtins.extend(t.builtins.iter().copied());
            shapes.extend(t.shapes.iter().map(|names| {
                let mut names = names.clone();
                names.sort();
                names
            }));
            lambdas.extend(t.lambdas.iter().cloned());
        }
        consts.sort_by(|a, b| a.0.cmp(&b.0));
        consts.dedup_by(|a, b| a.0 == b.0);
        fields.sort();
        fields.dedup();
        builtins.sort_by_key(|b| b.name());
        builtins.dedup();
        shapes.sort();
        shapes.dedup();
        lambdas.sort();
        lambdas.dedup();
        Unit::from_tables(
            ctors,
            consts.into_iter().map(|(_, v)| v).collect(),
            fields,
            builtins,
            shapes,
            lambdas,
        )
        .expect("sorted and deduplicated tables are in order")
    }

    /// The tables as a unit recorded them; `None` unless each is sorted and duplicate-free, since
    /// the positions its C bakes are ranks.
    pub fn from_tables(
        ctors: Vec<(Symbol, usize)>,
        consts: Vec<Value>,
        fields: Vec<Symbol>,
        builtins: Vec<Builtin>,
        shapes: Vec<Vec<Symbol>>,
        lambdas: Vec<String>,
    ) -> Option<Unit> {
        let const_keys: Vec<String> = consts.iter().map(encode_const).collect();
        let builtin_names: Vec<&str> = builtins.iter().map(|b| b.name()).collect();
        let ordered = ascending(&const_keys)
            && ascending(&fields)
            && ascending(&builtin_names)
            && ascending(&shapes)
            && shapes.iter().all(|names| ascending(names))
            && ascending(&lambdas);
        if !ordered {
            return None;
        }
        let layouts = Layouts::of(ctors, &shapes);
        Some(Unit {
            consts,
            fields,
            builtins,
            shapes,
            layouts,
            lambdas,
        })
    }

    pub fn constant(&self, v: &Value) -> Option<usize> {
        let key = encode_const(v);
        self.consts
            .binary_search_by(|c| encode_const(c).cmp(&key))
            .ok()
    }

    pub fn field(&self, name: &Symbol) -> Option<usize> {
        self.fields.binary_search(name).ok()
    }

    pub fn builtin(&self, b: Builtin) -> Option<usize> {
        self.builtins
            .binary_search_by(|x| x.name().cmp(b.name()))
            .ok()
    }

    pub fn shape(&self, names: &[Symbol]) -> Option<u32> {
        let mut names = names.to_vec();
        names.sort();
        self.shapes.binary_search(&names).ok().map(|i| i as u32)
    }

    pub fn lambda(&self, symbol: &str) -> Option<usize> {
        self.lambdas
            .binary_search_by(|l| l.as_str().cmp(symbol))
            .ok()
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

/// The code-table symbol of a pure nullary root's entry, which is also its memo slot.
pub fn memo_symbol(name: &str) -> String {
    format!("{}_entry", mangle(name))
}

/// A name the emitted C can carry: a Ply name holds dots, and a C identifier may not.
pub fn mangle(name: &str) -> String {
    let mut out = String::from("ply_");
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}
