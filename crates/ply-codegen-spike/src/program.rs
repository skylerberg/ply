//! The program the spike compiles out of: the shipped standard library, loaded
//! the way `ply` loads it, so the function under measurement is the one a
//! request actually runs rather than a copy written for a benchmark.

use anyhow::{Result, anyhow};
use ply_core::CheckOutput;
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::{FnDef, Item, ModuleName, Program as Ast};
use ply_syntax::resolve::Resolved;
use std::path::Path;

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

    /// The shipped standard library plus every `.ply` file in `dir`, loaded as
    /// one program.
    ///
    /// `ply` derives a module name from a file's path relative to the project
    /// root, so a project of one directory names its modules by file stem; this
    /// does the same, and takes the files in sorted order so that two runs on
    /// one directory produce one program.
    pub fn project(dir: &Path) -> Result<Loaded> {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| anyhow!("{}: {e}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "ply"))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(anyhow!("no `.ply` file under {}", dir.display()));
        }
        let mut owned: Vec<(ModuleName, String, std::path::PathBuf)> = Vec::new();
        for (module, source) in ply_std::sources() {
            let name = ModuleName::from_dotted(module);
            let path = ply_std::pseudo_path(&name);
            owned.push((name, source.to_string(), path));
        }
        for path in &files {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow!("{} has no module name", path.display()))?;
            let source =
                std::fs::read_to_string(path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
            owned.push((ModuleName::from_dotted(stem), source, path.clone()));
        }
        let mut sources = ply_span::SourceMap::new();
        let mut ids = Vec::new();
        for (_, source, path) in &owned {
            ids.push(sources.add(path.clone(), source.clone()));
        }
        let leaked: Vec<&'static str> = owned
            .iter()
            .map(|(_, source, _)| &*Box::leak(source.clone().into_boxed_str()))
            .collect();
        let inputs: Vec<_> = owned
            .iter()
            .zip(&ids)
            .zip(&leaked)
            .map(|(((module, _, _), id), source)| (*id, module.clone(), *source))
            .collect();
        Loaded::finish(inputs)
    }

    fn finish(inputs: Vec<(ply_span::SourceId, ModuleName, &'static str)>) -> Result<Loaded> {
        let mut ast =
            ply_syntax::parse_program(inputs).map_err(|d| report("parsing the program", &d))?;
        let expanded = ply_derive::expand_program(&mut ast);
        if !expanded.is_empty() {
            return Err(report("expanding a `derive`", &expanded));
        }
        let resolved =
            ply_syntax::resolve::resolve(&ast).map_err(|d| report("resolving the program", &d))?;
        let check = ply_core::check_program(&ast, &resolved)
            .map_err(|d| report("checking the program", &d))?;
        Ok(Loaded {
            ast,
            resolved,
            check,
        })
    }

    /// Every function in one module, by program-wide name, in source order.
    pub fn functions_in(&self, module: &str) -> Vec<String> {
        let mut out = Vec::new();
        for m in &self.ast.modules {
            if m.name.to_string() != module {
                continue;
            }
            for item in &m.items {
                if let Item::Fn(def) = item {
                    out.push(m.name.qualify(&def.name.name).to_string());
                }
            }
        }
        out
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
