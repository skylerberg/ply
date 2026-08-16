//! The program the spike compiles out of: the shipped standard library, loaded
//! the way `ply` loads it, so the function under measurement is the one a
//! request actually runs rather than a copy written for a benchmark.

use anyhow::{Result, anyhow};
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::{FnDef, Item, ModuleName, Program as Ast};
use ply_syntax::resolve::Resolved;

pub struct Loaded {
    pub ast: Ast,
    pub resolved: Resolved,
    pub check: CheckOutput,
}

fn report(what: &str, ds: &[Diagnostic]) -> anyhow::Error {
    let joined: Vec<String> = ds.iter().map(|d| d.message.clone()).collect();
    anyhow!("{what}: {}", joined.join("; "))
}

impl Loaded {
    pub fn std_library() -> Result<Loaded> {
        let mut sources = ply_span::SourceMap::new();
        let owned: Vec<(ModuleName, &'static str)> = ply_std::sources()
            .into_iter()
            .map(|(module, source)| (ModuleName::from_dotted(module), source))
            .collect();
        let mut inputs = Vec::new();
        for (module, source) in &owned {
            let id = sources.add(ply_std::pseudo_path(module), source.to_string());
            inputs.push((id, module.clone(), *source));
        }
        let mut ast =
            ply_syntax::parse_program(inputs).map_err(|d| report("parsing the stdlib", &d))?;
        let expanded = ply_derive::expand_program(&mut ast);
        if !expanded.is_empty() {
            return Err(report("expanding a `derive`", &expanded));
        }
        let resolved =
            ply_syntax::resolve::resolve(&ast).map_err(|d| report("resolving the stdlib", &d))?;
        let check = ply_core::check_program(&ast, &resolved)
            .map_err(|d| report("checking the stdlib", &d))?;
        Ok(Loaded {
            ast,
            resolved,
            check,
        })
    }

    /// The definition a program-wide name denotes, and the index of the module
    /// its bare names resolve in — the pair the machine keys everything on.
    pub fn definition(&self, name: &str) -> Option<(&FnDef, usize)> {
        for (index, module) in self.ast.modules.iter().enumerate() {
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

    /// Every sum-type constructor in the program, by program-wide name, with its
    /// arity — the table `Machine::build` assembles and `lookup` reads.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        let mut out: Vec<(Symbol, usize)> = ply_core::prelude::ctor_arities();
        for module in &self.ast.modules {
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
}
