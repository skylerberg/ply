//! `derive json for Order` becomes an ordinary definition.

pub mod emit;
pub mod retarget;
pub mod rules;
pub mod walk;

#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, Symbol, codes};
use ply_syntax::ast::{
    DeriveDef, Derived, FnDef, Ident, ImportDecl, ImportKind, Item, Module, Program, TypeDef,
    TypeDefBody,
};
use ply_syntax::parser::parse_recovering;

pub use rules::{generated_name, snake_case};
pub use walk::Blocker;

/// Expands every module in place.
pub fn expand_program(program: &mut Program) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for module in &mut program.modules {
        diags.append(&mut expand_module(module));
    }
    diags
}

pub fn expand_module(module: &mut Module) -> Vec<Diagnostic> {
    if !module.items.iter().any(|i| matches!(i, Item::Derive(_))) {
        return Vec::new();
    }
    let Expansion {
        generated,
        imports,
        diags,
    } = Expander::new(module).run();
    module.imports.extend(imports);
    module
        .items
        .extend(generated.into_iter().map(|(_, d)| Item::Fn(Box::new(d))));
    diags
}

/// The source each of this module's derivations generates, in `derive` order.
pub fn preview(module: &Module) -> Vec<String> {
    Expander::new(module)
        .run()
        .generated
        .into_iter()
        .map(|(source, _)| source)
        .collect()
}

/// What one module's derivations produced.
struct Expansion {
    generated: Vec<(String, FnDef)>,
    /// Module binders the generated bodies write, for a runtime module the file imported without
    /// binding one.
    imports: Vec<ImportDecl>,
    diags: Vec<Diagnostic>,
}

struct Expander<'a> {
    module: &'a Module,
    /// The module's own type declarations, by simple name, in source order.
    types: IndexMap<Symbol, &'a TypeDef>,
    /// Those of them that are parameterless aliases, which a written type is resolved through
    /// before a `Map`'s key form is chosen.
    aliases: emit::Aliases<'a>,
    /// Generated name -> the `derive` that claimed it, for the collision that `snake_case` being
    /// total makes possible.
    claimed: IndexMap<String, &'a DeriveDef>,
    generated: Vec<(String, FnDef)>,
    /// Runtime module name -> the binder its calls are written under, and the import that binds it
    /// when this expansion had to add one.
    runtimes: IndexMap<String, (String, Option<ImportDecl>)>,
    diags: Vec<Diagnostic>,
}

impl<'a> Expander<'a> {
    fn new(module: &'a Module) -> Expander<'a> {
        let mut types = IndexMap::new();
        let mut aliases = emit::Aliases::new();
        for item in &module.items {
            if let Item::Type(def) = item {
                types.entry(def.name.name.clone()).or_insert(&**def);
                if let (true, TypeDefBody::Alias(body)) = (def.params.is_empty(), &def.body) {
                    aliases.entry(def.name.name.clone()).or_insert(body);
                }
            }
        }
        Expander {
            module,
            types,
            aliases,
            claimed: IndexMap::new(),
            generated: Vec::new(),
            runtimes: IndexMap::new(),
            diags: Vec::new(),
        }
    }

    fn run(mut self) -> Expansion {
        for item in &self.module.items {
            let Item::Derive(def) = item else { continue };
            self.one(def);
        }
        Expansion {
            generated: self.generated,
            imports: self
                .runtimes
                .into_values()
                .filter_map(|(_, import)| import)
                .collect(),
            diags: self.diags,
        }
    }

    fn one(&mut self, def: &'a DeriveDef) {
        let Some(target) = self.types.get(&def.target.name).copied() else {
            self.orphan(def);
            return;
        };
        let name = rules::generated_name(def.deriver, def.target.name.as_str());
        if let Some(prev) = self.claimed.get(&name) {
            self.collision(def, prev, &name);
            return;
        }
        if let Err(blocker) = walk::check_decl(def.deriver, target) {
            self.not_derivable(def, &blocker);
            return;
        }
        let Some(runtime) = self.runtime_prefix(def) else {
            return;
        };

        let emitter = emit::Emitter::new(def.deriver, runtime, &target.params, &self.aliases);
        let source = emitter.item(target, target.vis);
        match self.parse_generated(&source) {
            Some(mut generated) => {
                retarget::fn_def(&mut generated, def.span);
                generated.derived = Some(Derived {
                    deriver: def.deriver,
                    target: def.target.name.clone(),
                });
                self.claimed.insert(name, def);
                self.generated.push((source, generated));
            }
            None => self.internal(def, &source),
        }
    }

    /// Parsing what was just printed is the deriver's own round-trip check: a generated body that
    /// does not parse is Ply's fault and is caught here rather than as a syntax error against a
    /// file that does not contain it.
    fn parse_generated(&self, source: &str) -> Option<FnDef> {
        let (parsed, diags) =
            parse_recovering(self.module.source, self.module.name.clone(), source);
        if !diags.is_empty() || parsed.items.len() != 1 {
            return None;
        }
        match parsed.items.into_iter().next() {
            Some(Item::Fn(def)) => Some(*def),
            _ => None,
        }
    }

    /// How this module writes a name that lives in the deriver's runtime module.
    fn runtime_prefix(&mut self, def: &DeriveDef) -> Option<String> {
        let Some(wanted) = rules::runtime_module(def.deriver) else {
            return Some(String::new());
        };
        if let Some((binder, _)) = self.runtimes.get(wanted) {
            return Some(binder.clone());
        }
        let Some(import) = self.module.imports.iter().find(|i| dotted(i) == wanted) else {
            self.diags.push(
                Diagnostic::error(
                    codes::NOT_DERIVABLE,
                    format!(
                        "`{}` cannot be derived here: this module does not import `{wanted}`",
                        def.deriver
                    ),
                )
                .primary(def.span, format!("needs `{wanted}`"))
                .note(format!(
                    "add `import {wanted}` — the dictionary type and the codecs \
                     a derivation composes with ship there"
                )),
            );
            return None;
        };
        let (binder, added) = match import.binder() {
            Some(binder) => (binder.to_string(), None),
            None => {
                let binder = self.free_binder(wanted);
                let span = import.path_span();
                let path = wanted
                    .split('.')
                    .map(|s| Ident::new(s, span))
                    .collect::<Vec<_>>();
                let synthesized = ImportDecl {
                    path,
                    kind: ImportKind::Alias(Ident::new(binder.as_str(), span)),
                    span,
                };
                (binder, Some(synthesized))
            }
        };
        let prefix = format!("{binder}::");
        self.runtimes
            .insert(wanted.to_string(), (prefix.clone(), added));
        Some(prefix)
    }

    /// A module binder nothing in this file already binds.
    fn free_binder(&self, wanted: &str) -> String {
        let mut binder = wanted.rsplit('.').next().unwrap_or(wanted).to_string();
        while self
            .module
            .imports
            .iter()
            .any(|i| i.binder().is_some_and(|b| b.as_str() == binder))
            || self
                .runtimes
                .values()
                .any(|(p, _)| *p == format!("{binder}::"))
        {
            binder.push('_');
        }
        binder
    }

    fn orphan(&mut self, def: &DeriveDef) {
        let mut d = Diagnostic::error(
            codes::ORPHAN_DERIVE,
            format!("`{}` is not a type this module declares", def.target.name),
        )
        .primary(def.target.span, "declared in another module, or not at all")
        .note(
            "a `derive` may only name a type its own module declares, so that one type \
             has one canonical encoding rather than one per module that thought of it",
        );
        if let Some(near) = self.nearest_type(&def.target.name) {
            d = d.note(format!("this module declares `{near}`"));
        }
        self.diags.push(d);
    }

    fn nearest_type(&self, name: &Symbol) -> Option<&Symbol> {
        self.types
            .keys()
            .find(|k| k.as_str().eq_ignore_ascii_case(name.as_str()))
    }

    fn collision(&mut self, def: &DeriveDef, prev: &DeriveDef, name: &str) {
        let same = prev.target.name == def.target.name;
        let mut d = Diagnostic::error(
            codes::DUPLICATE_DEFINITION,
            format!("`{name}` is generated twice"),
        )
        .primary(def.span, "this derivation generates it")
        .secondary(prev.span, "and so does this one");
        d = if same {
            d.note(format!(
                "`derive {} for {}` is already written above; remove one",
                def.deriver, def.target.name
            ))
        } else {
            d.note(format!(
                "`{}` and `{}` both name `{name}`; rename one of the types",
                prev.target.name, def.target.name
            ))
        };
        self.diags.push(d);
    }

    fn not_derivable(&mut self, def: &DeriveDef, blocker: &Blocker) {
        let mut d = Diagnostic::error(
            codes::NOT_DERIVABLE,
            format!(
                "`{}` cannot be derived for `{}`",
                def.deriver, def.target.name
            ),
        )
        .primary(blocker.span, blocker.reason.clone())
        .secondary(def.span, "required by this derivation")
        .note(format!(
            "`derive {} for {}` requires every field to be derivable",
            def.deriver, def.target.name
        ));
        if let Some(note) = blocker.note {
            d = d.note(note);
        }
        if let Some(variant) = &blocker.variant {
            d = d.note(format!("the field is in variant `{variant}`"));
        }
        self.diags.push(d.note(format!(
            "remove the field from `{}`, or write the dictionary by hand",
            def.target.name
        )));
    }

    fn internal(&mut self, def: &DeriveDef, source: &str) {
        self.diags.push(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "the `{}` deriver generated a definition that does not parse",
                    def.deriver
                ),
            )
            .primary(def.span, "this derivation")
            .note("this is a compiler bug: derivation is total, so generation cannot fail")
            .note(format!("it generated: {source}")),
        );
    }
}

fn dotted(import: &ImportDecl) -> String {
    import
        .path
        .iter()
        .map(|s| s.name.to_string())
        .collect::<Vec<_>>()
        .join(".")
}
