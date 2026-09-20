//! What a body names of the unit around it, and the unit's tables those names resolve into. A
//! unit is built with its tables sorted by content, so a body's resolved C is a function of what
//! the unit holds and never of the order its definitions were taken in.

use super::cache::encode_const;
use crate::heap::Layouts;
use ply_eval::{Builtin, Value};
use ply_span::Symbol;
use std::collections::HashMap;

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
    /// The definitions this one body serves when it is a recursive group's, in case order; empty
    /// for a body of its own.
    pub members: Vec<String>,
}

/// What an emitted unit accumulates that is not code, with each entry's position. A unit being
/// built sorts every table by content, so a position is a rank among what the unit holds; a unit
/// read back keeps the order its C was emitted against, whatever that was.
pub struct Unit {
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<Builtin>,
    /// Each shape's field names sorted; a shape's id is its position.
    pub shapes: Vec<Vec<Symbol>>,
    pub layouts: Layouts,
    /// Lambda entry symbols; `rt_closure` and `rt_constant` take a position here.
    pub lambdas: Vec<String>,
}

fn sorted(names: &[Symbol]) -> Vec<Symbol> {
    let mut names = names.to_vec();
    names.sort();
    names
}

impl Unit {
    /// The union of what `bodies` name and the lambda symbols given, each table sorted by
    /// content and duplicate-free.
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
            shapes.extend(t.shapes.iter().map(|names| sorted(names)));
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
        .expect("a sorted, duplicate-free shape list interns to its positions")
    }

    /// The tables as a unit recorded them, positions as given; `None` if a shape would not
    /// intern to its position, since those are the ids its C bakes.
    pub fn from_tables(
        ctors: Vec<(Symbol, usize)>,
        consts: Vec<Value>,
        fields: Vec<Symbol>,
        builtins: Vec<Builtin>,
        shapes: Vec<Vec<Symbol>>,
        lambdas: Vec<String>,
    ) -> Option<Unit> {
        let layouts = Layouts::of(ctors, &shapes);
        let placed = layouts.shape_count() >= shapes.len()
            && shapes
                .iter()
                .enumerate()
                .all(|(id, names)| layouts.shape_names(id as u32)[..] == names[..]);
        if !placed {
            return None;
        }
        Some(Unit {
            consts,
            fields,
            builtins,
            shapes,
            layouts,
            lambdas,
        })
    }

    pub fn lambda(&self, symbol: &str) -> Option<usize> {
        self.lambdas.iter().position(|l| l == symbol)
    }

    /// Every entry's position, for resolving bodies against this unit.
    pub fn positions(&self) -> Positions<'_> {
        Positions {
            consts: index(self.consts.iter().map(encode_const)),
            fields: index(self.fields.iter()),
            builtins: index(self.builtins.iter().map(|b| b.name())),
            shapes: index(self.shapes.iter().map(|names| sorted(names))),
            lambdas: index(self.lambdas.iter().map(String::as_str)),
        }
    }
}

fn index<K: std::hash::Hash + Eq>(keys: impl IntoIterator<Item = K>) -> HashMap<K, usize> {
    let mut at = HashMap::new();
    for (i, k) in keys.into_iter().enumerate() {
        at.entry(k).or_insert(i);
    }
    at
}

/// Where each entry of a unit's tables sits, by content; built once per unit resolved, never
/// for a unit merely loaded.
pub struct Positions<'a> {
    consts: HashMap<String, usize>,
    fields: HashMap<&'a Symbol, usize>,
    builtins: HashMap<&'static str, usize>,
    shapes: HashMap<Vec<Symbol>, usize>,
    lambdas: HashMap<&'a str, usize>,
}

impl Positions<'_> {
    pub fn constant(&self, v: &Value) -> Option<usize> {
        self.consts.get(&encode_const(v)).copied()
    }

    pub fn field(&self, name: &Symbol) -> Option<usize> {
        self.fields.get(name).copied()
    }

    pub fn builtin(&self, b: Builtin) -> Option<usize> {
        self.builtins.get(b.name()).copied()
    }

    pub fn shape(&self, names: &[Symbol]) -> Option<u32> {
        self.shapes.get(&sorted(names)).map(|id| *id as u32)
    }

    pub fn lambda(&self, symbol: &str) -> Option<usize> {
        self.lambdas.get(symbol).copied()
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
