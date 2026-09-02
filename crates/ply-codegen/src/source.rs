//! The program a unit compiles out of, in the three pieces the machine already holds.

use ply_core::CheckOutput;
use ply_span::Symbol;
use ply_syntax::ast::{FnDef, Generics, Ident, Item, Program, TestDef, Visibility};
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
    /// The tests, as the program-wide names of the roots synthesized for them.
    test_roots: Vec<String>,
}

/// The name a test's root takes: its place among its module's tests, which `ply_eval`'s test
/// runner computes the same way when it offers the test.
pub fn test_root_name(ordinal: usize) -> Symbol {
    Symbol::new(format!("test#{ordinal}"))
}

/// A test's body as a nullary definition, so the fragment compiles it like any other.
fn test_as_definition(test: &TestDef, name: &Symbol) -> FnDef {
    FnDef {
        vis: Visibility::Private,
        name: Ident {
            name: name.clone(),
            span: test.name_span,
        },
        generics: Generics {
            types: Vec::new(),
            effects: Vec::new(),
        },
        params: Vec::new(),
        ret: None,
        effects: None,
        constraints: Vec::new(),
        derived: None,
        spec: Vec::new(),
        reuse: None,
        body: test.body.clone(),
        span: test.span,
    }
}

impl Source {
    /// A source over a program that is already `'static`.
    pub fn new(
        program: &'static Program,
        resolved: &'static Resolved,
        check: &'static CheckOutput,
    ) -> Source {
        let mut definitions = HashMap::new();
        let mut roots = Vec::new();
        for (index, module) in program.modules.iter().enumerate() {
            let mut ordinal = 0;
            for item in &module.items {
                match item {
                    Item::Fn(def) => {
                        definitions
                            .entry(module.name.qualify(&def.name.name).to_string())
                            .or_insert((&**def, index));
                    }
                    // A test is a root the machine enters whole: a nullary definition of its
                    // body, named by its place among the module's tests, which is the name the
                    // test runner offers.
                    Item::Test(test) => {
                        let name = test_root_name(ordinal);
                        let def: &'static FnDef =
                            Box::leak(Box::new(test_as_definition(test, &name)));
                        let qualified = module.name.qualify(&name).to_string();
                        definitions.insert(qualified.clone(), (def, index));
                        roots.push(qualified);
                        ordinal += 1;
                    }
                    _ => {}
                }
            }
        }
        Source {
            program,
            resolved,
            check,
            definitions,
            test_roots: roots,
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
        out.extend(self.test_roots.iter().cloned());
        out
    }
}
