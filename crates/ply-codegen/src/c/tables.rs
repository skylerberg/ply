//! What a body names of the unit around it, and the unit's tables those names resolve into.

use ply_eval::{Builtin, Value};
use ply_span::Symbol;

/// What one body names of the unit around it, by the positions its own text uses.
#[derive(Default, Clone)]
pub struct Tables {
    pub consts: Vec<Value>,
    pub builtins: Vec<Builtin>,
    pub fields: Vec<Symbol>,
    pub shapes: Vec<Vec<Symbol>>,
    /// Every definition this body calls, so that a body restored from a cache can be checked
    /// against the set the fixpoint took rather than trusted.
    pub calls: Vec<String>,
    /// The C symbols of the lambda entries this body defines, in the order it met them.
    pub lambdas: Vec<String>,
    /// The effects this body performs and the effects it handles, under their program-wide names.
    pub performs: Vec<String>,
    pub handles: Vec<String>,
}

/// What an emitted unit accumulates that is not code.
pub struct Unit {
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<Builtin>,
    /// The shapes and constructor indices the runtime reads a record and a variant against.
    pub layouts: crate::heap::Layouts,
    /// The C symbol of every lambda entry in the unit. `rt_closure` is handed a position in this,
    /// and `rt::Tables::functions` holds the address `dlsym` found for each.
    pub lambdas: Vec<String>,
}

impl Unit {
    pub fn new(ctors: Vec<(Symbol, usize)>) -> Unit {
        Unit {
            consts: Vec::new(),
            fields: Vec::new(),
            builtins: Vec::new(),
            layouts: crate::heap::Layouts::new(ctors),
            lambdas: Vec::new(),
        }
    }

    /// The shape a field set interns to, in the same table the runtime will read it against.
    pub fn shape(&mut self, names: &[Symbol]) -> u32 {
        self.layouts.shape(names.to_vec())
    }

    pub(super) fn constant(&mut self, v: Value) -> usize {
        self.consts.push(v);
        self.consts.len() - 1
    }

    pub(super) fn field(&mut self, name: &Symbol) -> usize {
        if let Some(i) = self.fields.iter().position(|f| f == name) {
            return i;
        }
        self.fields.push(name.clone());
        self.fields.len() - 1
    }

    pub(super) fn lambda(&mut self, symbol: &str) -> usize {
        if let Some(i) = self.lambdas.iter().position(|l| l == symbol) {
            return i;
        }
        self.lambdas.push(symbol.to_string());
        self.lambdas.len() - 1
    }

    pub(super) fn builtin(&mut self, b: Builtin) -> usize {
        if let Some(i) = self.builtins.iter().position(|x| *x == b) {
            return i;
        }
        self.builtins.push(b);
        self.builtins.len() - 1
    }
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
