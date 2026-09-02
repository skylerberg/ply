//! The program a unit compiles out of, in the three pieces the machine already holds.

use ply_core::CheckOutput;
use ply_span::Symbol;
use ply_syntax::ast::{FnDef, Item, Program};
use ply_syntax::resolve::Resolved;
use std::collections::HashMap;

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub program: &'static Program,
    pub resolved: &'static Resolved,
    pub check: &'static CheckOutput,
    /// Every definition by program-wide name, with the index of its module: the code generator
    /// asks for one at each name it resolves.
    definitions: HashMap<String, (&'static FnDef, usize)>,
}

impl Source {
    /// A source over a program that is already `'static`.
    pub fn new(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> Source {
        let mut definitions = HashMap::new();
        for (index, module) in program.modules.iter().enumerate() {
            for item in &module.items {
                if let Item::Fn(def) = item {
                    definitions
                        .entry(module.name.qualify(&def.name.name).to_string())
                        .or_insert((&**def, index));
                }
            }
        }
        Source {
            program,
            resolved,
            check,
            definitions,
        }
    }

    /// The definition a program-wide name denotes, and the index of the module its bare names
    /// resolve in — the pair the machine keys everything on.
    pub fn definition(&self, name: &str) -> Option<(&'static FnDef, usize)> {
        self.definitions.get(name).copied()
    }

    /// Every sum-type constructor in the program, by program-wide name, with its arity — the table
    /// `Machine::build` assembles and `lookup` reads.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        let mut out: Vec<(Symbol, usize)> = ply_core::prelude::ctor_arities();
        for module in &self.program.modules {
            for item in &module.items {
                if let Item::Type(t) = item
                    && let ply_syntax::ast::TypeDefBody::Sum(variants) = &t.body
                {
                    for v in variants {
                        out.push((module.name.qualify(&v.name.name), v.fields.len()));
                    }
                }
            }
        }
        out
    }

    /// Every function in the program, by program-wide name, in source order.
    pub fn functions(&self) -> Vec<String> {
        let mut out = Vec::new();
        for module in &self.program.modules {
            for item in &module.items {
                if let Item::Fn(def) = item {
                    out.push(module.name.qualify(&def.name.name).to_string());
                }
            }
        }
        out
    }
}
