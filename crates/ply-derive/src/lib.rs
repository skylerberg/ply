//! `derive json for Order` becomes an ordinary definition.
//!
//! Expansion runs immediately after parse and before resolution, and is purely
//! syntactic: it reads the module's own type declarations — which the orphan
//! rule guarantees is where the target is — and emits references to other
//! types' codecs by name, leaving resolution to check them.
//!
//! Generated `FnDef`s are **appended to `Module::items`**, after every source
//! item, in the order their `derive`s were written. One list rather than two: a
//! second list is a thing every walker can forget, and forgetting it drops a
//! definition silently. Appending leaves the index of every `test` and `law`
//! untouched, which `HashOutput::tests` is parallel to. The `Item::Derive` stays
//! in place as the declaration and contributes no definition of its own.
//!
//! Determinism is the property this file exists to keep. The same type must
//! produce byte-identical source on every run and every machine, or a generated
//! definition's hash — and therefore the cache entry, the test selection and the
//! wire format it describes — depends on something outside the program. Nothing
//! below iterates a hash map or sorts by anything but a total order that is
//! already written down.

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

/// Expands every module in place. A module with no `derive` is untouched, and a
/// program with none has hashes byte-identical to what it had before this
/// existed.
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
///
/// This is what the golden pin test renders. A change to a deriver moves these
/// bytes, which moves the generated definition's hash — and gate 1 keys on raw
/// file content, so a compiler upgrade that changed them without a
/// `FRONTEND_VERSION` bump would let a file be skipped and a stale generated
/// definition be reused.
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
    /// Module binders the generated bodies write, for a runtime module the file
    /// imported without binding one. See [`Expander::runtime_prefix`].
    imports: Vec<ImportDecl>,
    diags: Vec<Diagnostic>,
}

struct Expander<'a> {
    module: &'a Module,
    /// The module's own type declarations, by simple name, in source order.
    types: IndexMap<Symbol, &'a TypeDef>,
    /// Those of them that are parameterless aliases, which a written type is
    /// resolved through before a `Map`'s key form is chosen.
    aliases: emit::Aliases<'a>,
    /// Generated name -> the `derive` that claimed it, for the collision that
    /// `snake_case` being total makes possible.
    claimed: IndexMap<String, &'a DeriveDef>,
    generated: Vec<(String, FnDef)>,
    /// Runtime module name -> the binder its calls are written under, and the
    /// import that binds it when this expansion had to add one.
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

    /// Parsing what was just printed is the deriver's own round-trip check: a
    /// generated body that does not parse is Ply's fault and is caught here
    /// rather than as a syntax error against a file that does not contain it.
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
    /// `None` means it cannot write one at all, and the diagnostic has been
    /// recorded.
    ///
    /// **Always a module binder, never bare.** A bare name is resolved in the
    /// *deriving* module, and ADR 0001 says a module's own items come first — so
    /// under `import std.json (..)`, which binds no module name, a generated
    /// body writing `int_json()` would compose with whatever the deriving module
    /// called `int_json`. No import to look at, no `AMBIGUOUS_IMPORT`, no
    /// diagnostic at the `derive` line, and one type with two encodings, which
    /// is the divergence the orphan rule exists to prevent. So when the file
    /// bound no module name for the runtime module, expansion adds one: a
    /// synthesized `import std.json as <binder>` that only generated code
    /// writes.
    ///
    /// The binder is a function of the file's own imports, so expansion stays a
    /// function of the module — and it enters no hash, because a free reference
    /// normalizes to its referent's hash rather than to the name it was written
    /// under.
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

    /// A module binder nothing in this file already binds. Deterministic: the
    /// module's default binder, then that name with underscores appended.
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
