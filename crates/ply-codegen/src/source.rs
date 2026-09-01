//! The program a unit compiles out of, in the three pieces the machine already holds.

use ply_core::CheckOutput;
use ply_span::Symbol;
use ply_syntax::ast::{FnDef, Item, Program};
use ply_syntax::resolve::Resolved;

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub program: &'static Program,
    pub resolved: &'static Resolved,
    pub check: &'static CheckOutput,
}

impl Source {
    /// A source over a program that is already `'static`.
    pub fn new(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> Source {
        Source {
            program,
            resolved,
            check,
        }
    }

    /// The definition a program-wide name denotes, and the index of the module its bare names
    /// resolve in — the pair the machine keys everything on.
    pub fn definition(&self, name: &str) -> Option<(&'static FnDef, usize)> {
        for (index, module) in self.program.modules.iter().enumerate() {
            for item in &module.items {
                if let Item::Fn(def) = item
                    && module.name.qualify(&def.name.name).as_str() == name
                {
                    return Some((def, index));
                }
            }
        }
        None
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
