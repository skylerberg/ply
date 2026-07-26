//! Name resolution over a whole [`Program`], done once so that inference,
//! hashing and evaluation cannot disagree about what a name means.
//!
//! Resolution is purely syntactic — it needs no types — and its output is the
//! only place the module namespace lives. Everything downstream works in
//! program-wide qualified names, which is what makes the namespace metadata
//! over hashes rather than part of them.

use crate::ast::{
    Ident, ImportDecl, ImportKind, Item, Module, ModuleName, Program, QName, TypeDefBody,
    Visibility,
};
use indexmap::IndexMap;
use indexmap::map::Entry;
use ply_span::{Diagnostic, Span, Symbol, codes};

/// Ply's three name spaces. Constructors live in [`Namespace::Value`] alongside
/// functions, because an expression cannot tell them apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Namespace {
    Value,
    Type,
    Effect,
}

impl Namespace {
    pub fn describe(self) -> &'static str {
        match self {
            Namespace::Value => "definition",
            Namespace::Type => "type",
            Namespace::Effect => "effect",
        }
    }

    pub const ALL: [Namespace; 3] = [Namespace::Value, Namespace::Type, Namespace::Effect];
}

/// A name a module declares, whether or not it exports it.
#[derive(Clone, Debug)]
pub struct Declared {
    /// The program-wide name, `store.orders.place`.
    pub qualified: Symbol,
    pub vis: Visibility,
    /// Where the item is declared, for a "defined here, but private" label.
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Declarations {
    pub values: IndexMap<Symbol, Declared>,
    pub types: IndexMap<Symbol, Declared>,
    pub effects: IndexMap<Symbol, Declared>,
}

impl Declarations {
    pub fn get(&self, ns: Namespace, name: &Symbol) -> Option<&Declared> {
        self.space(ns).get(name)
    }

    pub fn space(&self, ns: Namespace) -> &IndexMap<Symbol, Declared> {
        match ns {
            Namespace::Value => &self.values,
            Namespace::Type => &self.types,
            Namespace::Effect => &self.effects,
        }
    }

    pub fn space_mut(&mut self, ns: Namespace) -> &mut IndexMap<Symbol, Declared> {
        match ns {
            Namespace::Value => &mut self.values,
            Namespace::Type => &mut self.types,
            Namespace::Effect => &mut self.effects,
        }
    }

    /// The names another module may name through `binder::name`.
    pub fn exported(&self, ns: Namespace) -> impl Iterator<Item = &Symbol> {
        self.space(ns)
            .iter()
            .filter(|(_, d)| d.vis.is_public())
            .map(|(n, _)| n)
    }
}

/// A [`Declared`] as seen from a module that can use it unqualified — its own,
/// or one that imported it selectively.
#[derive(Clone, Debug)]
pub struct Binding {
    pub qualified: Symbol,
    /// Index into [`Program::modules`] of the module that declares it.
    pub owner: usize,
    /// Where the name entered this file: the item's own name, or the import
    /// that brought it in.
    pub span: Span,
}

/// Every name a module declares, as a [`Binding`]. Parallel to
/// [`Resolved::declarations`], and the reason a qualified lookup can hand back a
/// reference: a `binder::name` reference denotes a declaration in *another*
/// module, which is therefore in no scope at all.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    pub values: IndexMap<Symbol, Binding>,
    pub types: IndexMap<Symbol, Binding>,
    pub effects: IndexMap<Symbol, Binding>,
}

impl Bindings {
    pub fn get(&self, ns: Namespace, name: &Symbol) -> Option<&Binding> {
        match ns {
            Namespace::Value => self.values.get(name),
            Namespace::Type => self.types.get(name),
            Namespace::Effect => self.effects.get(name),
        }
    }

    fn space_mut(&mut self, ns: Namespace) -> &mut IndexMap<Symbol, Binding> {
        match ns {
            Namespace::Value => &mut self.values,
            Namespace::Type => &mut self.types,
            Namespace::Effect => &mut self.effects,
        }
    }
}

/// The unqualified names a module's bodies see. Local binders are absent and
/// always win over everything here.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub module: ModuleName,
    /// Module binder -> index into [`Program::modules`]. A separate namespace
    /// from the three below, reachable only through `binder::name`, so a local
    /// variable can share a module binder's name without hiding it.
    pub modules: IndexMap<Symbol, (usize, Span)>,
    /// Modules this file imported names from without binding them as a module,
    /// keyed by the binder such an import would *not* introduce. Kept only so
    /// that `orders::place` after `import store.orders (place)` can say why
    /// `orders` is not in scope, which is the mistake the rule invites.
    pub selective: IndexMap<Symbol, (usize, Span)>,
    pub values: IndexMap<Symbol, Binding>,
    pub types: IndexMap<Symbol, Binding>,
    pub effects: IndexMap<Symbol, Binding>,
}

impl Scope {
    pub fn get(&self, ns: Namespace, name: &Symbol) -> Option<&Binding> {
        match ns {
            Namespace::Value => self.values.get(name),
            Namespace::Type => self.types.get(name),
            Namespace::Effect => self.effects.get(name),
        }
    }

    pub fn space_mut(&mut self, ns: Namespace) -> &mut IndexMap<Symbol, Binding> {
        match ns {
            Namespace::Value => &mut self.values,
            Namespace::Type => &mut self.types,
            Namespace::Effect => &mut self.effects,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Resolved {
    /// Parallel to [`Program::modules`].
    pub scopes: Vec<Scope>,
    /// Parallel to [`Program::modules`].
    pub declarations: Vec<Declarations>,
    /// Parallel to [`Program::modules`].
    pub declared: Vec<Bindings>,
    /// Module name -> index into [`Program::modules`].
    pub index: IndexMap<Symbol, usize>,
    /// Dependency-first order over [`Program::modules`]. Acyclic by
    /// construction: a cycle is rejected before this is built.
    pub order: Vec<usize>,
}

impl Resolved {
    pub fn scope(&self, module: usize) -> Option<&Scope> {
        self.scopes.get(module)
    }

    pub fn index_of(&self, name: &ModuleName) -> Option<usize> {
        self.index.get(name.as_symbol()).copied()
    }

    /// A bare name must already have failed local lookup — locals are not in
    /// scope here and win unconditionally. A qualified name never consults the
    /// current module's scope at all.
    pub fn lookup(&self, module: usize, ns: Namespace, q: &QName) -> Result<&Binding, Diagnostic> {
        let Some(scope) = self.scopes.get(module) else {
            return Err(Diagnostic::error(
                codes::UNKNOWN_MODULE,
                format!("`{q}` was resolved against a module outside this program"),
            )
            .primary(q.span, "no such module")
            .note("this is a compiler bug; the reference itself is probably fine"));
        };

        let Some(binder) = &q.module else {
            return scope
                .get(ns, q.symbol())
                .ok_or_else(|| self.unknown_bare(scope, ns, q));
        };

        let Some(&(owner, _)) = scope.modules.get(&binder.name) else {
            return Err(self.unknown_module(scope, binder, q));
        };

        let Some(declared) = self.declarations[owner].get(ns, q.symbol()) else {
            return Err(self.unknown_in_module(owner, ns, q));
        };
        if !declared.vis.is_public() {
            return Err(self.private(owner, ns, q, declared));
        }
        self.declared[owner].get(ns, q.symbol()).ok_or_else(|| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("`{q}` could not be resolved"))
                .primary(q.span, "not found")
                .note("this is a compiler bug: a declaration exists with no binding")
        })
    }

    fn unknown_bare(&self, scope: &Scope, ns: Namespace, q: &QName) -> Diagnostic {
        let mut d = Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("unknown {} `{}`", ns.describe(), q.symbol()),
        )
        .primary(q.span, "not found in this scope");
        if let Some(owner) = self.exporter_of(ns, q.symbol()) {
            let module = &self.scopes[owner].module;
            d = d.note(format!(
                "module `{module}` exports `{}`; add `import {module} ({})`, or `import {module}` \
                 and write `{}::{}`",
                q.symbol(),
                q.symbol(),
                module.default_binder(),
                q.symbol()
            ));
        } else {
            d = d.note(format!(
                "define it in `{}`, or import it from the module that declares it",
                scope.module
            ));
        }
        d
    }

    /// The first module that exports this name, for a "you meant this import"
    /// note on a bare name that resolves nowhere.
    fn exporter_of(&self, ns: Namespace, name: &Symbol) -> Option<usize> {
        self.declarations
            .iter()
            .position(|d| d.get(ns, name).is_some_and(|decl| decl.vis.is_public()))
    }

    fn unknown_module(&self, scope: &Scope, binder: &Ident, q: &QName) -> Diagnostic {
        let mut d = Diagnostic::error(
            codes::UNKNOWN_MODULE,
            format!("no module is imported as `{}`", binder.name),
        )
        .primary(binder.span, "not a module imported by this file");

        if let Some(&(owner, span)) = scope.selective.get(&binder.name) {
            let module = &self.scopes[owner].module;
            return d
                .secondary(span, format!("this import brings in names from `{module}`"))
                .note(format!(
                    "a selective import binds no module name: add `import {module}` as well, or \
                     write `{}` unqualified",
                    q.symbol()
                ));
        }

        let candidate = self.index.keys().find(|name| {
            name.as_str() == binder.name.as_str()
                || name.as_str().rsplit('.').next() == Some(binder.name.as_str())
        });
        if let Some(name) = candidate {
            d = d.note(format!(
                "`{name}` is a module in this program: add `import {name}` above the first item"
            ));
        } else if scope.modules.is_empty() {
            d = d.note(format!(
                "`{}` imports no modules; add `import <module>` above the first item",
                scope.module
            ));
        } else {
            let known: Vec<String> = scope.modules.keys().map(|k| format!("`{k}`")).collect();
            d = d.note(format!("modules imported here: {}", known.join(", ")));
        }
        d
    }

    fn unknown_in_module(&self, owner: usize, ns: Namespace, q: &QName) -> Diagnostic {
        let module = &self.scopes[owner].module;
        let exported: Vec<String> = self.declarations[owner]
            .exported(ns)
            .map(|n| format!("`{n}`"))
            .collect();
        let mut d = Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!(
                "module `{module}` has no {} `{}`",
                ns.describe(),
                q.symbol()
            ),
        )
        .primary(q.span, format!("not declared in `{module}`"));
        d = if exported.is_empty() {
            d.note(format!("`{module}` exports no {}s", ns.describe()))
        } else {
            d.note(format!("`{module}` exports: {}", exported.join(", ")))
        };
        d
    }

    fn private(&self, owner: usize, ns: Namespace, q: &QName, declared: &Declared) -> Diagnostic {
        let module = &self.scopes[owner].module;
        let keyword = match ns {
            Namespace::Value => "fn",
            Namespace::Type => "type",
            Namespace::Effect => "effect",
        };
        Diagnostic::error(
            codes::PRIVATE_NAME,
            format!("`{}` is private to module `{module}`", q.symbol()),
        )
        .primary(q.span, "not exported")
        .secondary(declared.span, "declared here, without `pub`")
        .note(format!(
            "write `pub {keyword} {}` in `{module}` to export it",
            q.symbol()
        ))
        .note("items are private by default")
    }
}

/// Reports, per module: unknown modules, import cycles, duplicate import
/// bindings, imports that collide with a local definition, and selective
/// imports of a name that is missing or private.
///
/// Two items declaring the same name in one module is *not* reported here —
/// inference reports it, where the declaration's own span and kind are known.
/// The first declaration wins so that resolution can continue.
///
/// Reference-site failures are not found here — bodies are walked by the
/// consumer, which calls [`Resolved::lookup`] and reports what it returns.
pub fn resolve(program: &Program) -> Result<Resolved, Vec<Diagnostic>> {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut index: IndexMap<Symbol, usize> = IndexMap::new();
    for (i, module) in program.modules.iter().enumerate() {
        match index.entry(module.name.as_symbol().clone()) {
            Entry::Vacant(slot) => {
                slot.insert(i);
            }
            Entry::Occupied(_) if module.name.is_anonymous() => {}
            Entry::Occupied(_) => diags.push(
                Diagnostic::error(
                    codes::DUPLICATE_DEFINITION,
                    format!("two files claim the module name `{}`", module.name),
                )
                .primary(start_of(module), "loaded a second time under this name")
                .note("one file is one module, and a module name is derived from its path"),
            ),
        }
    }

    let declarations: Vec<Declarations> = program.modules.iter().map(declarations_of).collect();
    let declared: Vec<Bindings> = declarations
        .iter()
        .enumerate()
        .map(|(owner, d)| bindings_of(owner, d))
        .collect();

    let mut scopes: Vec<Scope> = Vec::with_capacity(program.modules.len());
    let mut edges: Vec<Vec<(usize, Span)>> = Vec::with_capacity(program.modules.len());
    for (i, module) in program.modules.iter().enumerate() {
        let mut builder = ScopeBuilder {
            program,
            index: &index,
            declarations: &declarations,
            declared: &declared,
            scope: Scope {
                module: module.name.clone(),
                modules: IndexMap::new(),
                selective: IndexMap::new(),
                values: declared[i].values.clone(),
                types: declared[i].types.clone(),
                effects: declared[i].effects.clone(),
            },
            local: declarations[i].clone(),
            imported: Declarations::default(),
            edges: Vec::new(),
            diags: Vec::new(),
        };
        builder.imports(&module.imports);
        scopes.push(builder.scope);
        edges.push(builder.edges);
        diags.append(&mut builder.diags);
    }

    let (cycles, order) = traverse(&edges);
    for cycle in &cycles {
        diags.push(cycle_diagnostic(program, cycle));
    }

    if diags.is_empty() {
        Ok(Resolved {
            scopes,
            declarations,
            declared,
            index,
            order,
        })
    } else {
        Err(diags)
    }
}

fn start_of(module: &Module) -> Span {
    Span::new(module.source, 0, 0)
}

fn declarations_of(module: &Module) -> Declarations {
    let mut out = Declarations::default();
    for item in &module.items {
        let vis = item.visibility();
        match item {
            Item::Fn(def) => declare(&mut out, Namespace::Value, module, &def.name, vis),
            Item::Effect(def) => declare(&mut out, Namespace::Effect, module, &def.name, vis),
            Item::Type(def) => {
                declare(&mut out, Namespace::Type, module, &def.name, vis);
                if let TypeDefBody::Sum(variants) = &def.body {
                    for variant in variants {
                        // A type you can name but cannot match on is not a
                        // useful export, so a constructor is as public as it is.
                        declare(&mut out, Namespace::Value, module, &variant.name, vis);
                    }
                }
            }
            Item::Test(_) => {}
        }
    }
    out
}

fn declare(out: &mut Declarations, ns: Namespace, module: &Module, name: &Ident, vis: Visibility) {
    out.space_mut(ns)
        .entry(name.name.clone())
        .or_insert_with(|| Declared {
            qualified: module.name.qualify(&name.name),
            vis,
            span: name.span,
        });
}

fn bindings_of(owner: usize, declarations: &Declarations) -> Bindings {
    let mut out = Bindings::default();
    for ns in Namespace::ALL {
        for (name, declared) in declarations.space(ns) {
            out.space_mut(ns).insert(
                name.clone(),
                Binding {
                    qualified: declared.qualified.clone(),
                    owner,
                    span: declared.span,
                },
            );
        }
    }
    out
}

struct ScopeBuilder<'a> {
    program: &'a Program,
    index: &'a IndexMap<Symbol, usize>,
    declarations: &'a [Declarations],
    declared: &'a [Bindings],
    scope: Scope,
    /// This module's own items, for the import-versus-local collision.
    local: Declarations,
    /// What earlier imports already bound, for the import-versus-import one.
    imported: Declarations,
    edges: Vec<(usize, Span)>,
    diags: Vec<Diagnostic>,
}

impl ScopeBuilder<'_> {
    fn imports(&mut self, imports: &[ImportDecl]) {
        for import in imports {
            let Some(target) = self.target(import) else {
                continue;
            };
            self.edges.push((target, import.path_span()));
            match &import.kind {
                ImportKind::Module | ImportKind::Alias(_) => self.bind_module(import, target),
                ImportKind::Names(names) => {
                    self.scope
                        .selective
                        .entry(import.module_name().default_binder())
                        .or_insert((target, import.path_span()));
                    for name in names {
                        self.bind_name(target, name);
                    }
                }
            }
        }
    }

    fn target(&mut self, import: &ImportDecl) -> Option<usize> {
        let name = import.module_name();
        if !name.is_anonymous()
            && let Some(&i) = self.index.get(name.as_symbol())
            && !self.program.modules[i].name.is_anonymous()
        {
            return Some(i);
        }
        let mut d = Diagnostic::error(
            codes::UNKNOWN_MODULE,
            format!("no module named `{name}` in this program"),
        )
        .primary(import.path_span(), "not found");
        let near: Vec<String> = self
            .index
            .keys()
            .filter(|k| !k.as_str().is_empty())
            .filter(|k| k.as_str().ends_with(name.default_binder().as_str()))
            .map(|k| format!("`{k}`"))
            .collect();
        d = if near.is_empty() {
            d.note("a module is a file: `store/orders.ply` is `store.orders`, relative to the project root")
        } else {
            d.note(format!(
                "modules with that last segment: {}",
                near.join(", ")
            ))
        };
        self.diags.push(d);
        None
    }

    fn bind_module(&mut self, import: &ImportDecl, target: usize) {
        let Some(binder) = import.binder() else {
            return;
        };
        let span = import.binder_span();
        match self.scope.modules.entry(binder.clone()) {
            Entry::Vacant(slot) => {
                slot.insert((target, span));
            }
            Entry::Occupied(slot) => {
                let (previous, first) = *slot.get();
                let previous = &self.program.modules[previous].name;
                self.diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_IMPORT,
                        format!("two imports bind the module name `{binder}`"),
                    )
                    .primary(
                        span,
                        format!("`{}` would also be `{binder}`", import.module_name()),
                    )
                    .secondary(
                        first,
                        format!("`{previous}` is already bound as `{binder}`"),
                    )
                    .note(format!(
                        "rename one of them: `import {} as <other>`",
                        import.module_name()
                    )),
                );
            }
        }
    }

    fn bind_name(&mut self, target: usize, name: &Ident) {
        let module = &self.program.modules[target].name;
        let found: Vec<Namespace> = Namespace::ALL
            .into_iter()
            .filter(|ns| self.declarations[target].get(*ns, &name.name).is_some())
            .collect();
        if found.is_empty() {
            let exported: Vec<String> = Namespace::ALL
                .into_iter()
                .flat_map(|ns| self.declarations[target].exported(ns))
                .map(|n| format!("`{n}`"))
                .collect();
            let mut d = Diagnostic::error(
                codes::UNKNOWN_NAME,
                format!("module `{module}` declares no `{}`", name.name),
            )
            .primary(name.span, format!("not found in `{module}`"));
            d = if exported.is_empty() {
                d.note(format!(
                    "`{module}` exports nothing; mark an item `pub` to export it"
                ))
            } else {
                d.note(format!("`{module}` exports: {}", exported.join(", ")))
            };
            self.diags.push(d);
            return;
        }

        let public: Vec<Namespace> = found
            .iter()
            .copied()
            .filter(|ns| {
                self.declarations[target]
                    .get(*ns, &name.name)
                    .is_some_and(|d| d.vis.is_public())
            })
            .collect();
        if public.is_empty() {
            let ns = found[0];
            let declared = self.declarations[target]
                .get(ns, &name.name)
                .expect("just found");
            let keyword = match ns {
                Namespace::Value => "fn",
                Namespace::Type => "type",
                Namespace::Effect => "effect",
            };
            self.diags.push(
                Diagnostic::error(
                    codes::PRIVATE_NAME,
                    format!("`{}` is private to module `{module}`", name.name),
                )
                .primary(name.span, "not exported")
                .secondary(declared.span, "declared here, without `pub`")
                .note(format!(
                    "write `pub {keyword} {}` in `{module}` to export it",
                    name.name
                ))
                .note("items are private by default"),
            );
            return;
        }

        for ns in public {
            let binding = self.declared[target]
                .get(ns, &name.name)
                .expect("a public declaration always has a binding")
                .clone();
            if let Some(previous) = self.local.get(ns, &name.name) {
                self.ambiguous(name, ns, previous.span, module);
                continue;
            }
            if let Some(previous) = self.imported.get(ns, &name.name) {
                let previous = previous.clone();
                self.duplicate_import(name, ns, &previous);
                continue;
            }
            self.imported.space_mut(ns).insert(
                name.name.clone(),
                Declared {
                    qualified: binding.qualified.clone(),
                    vis: Visibility::Public,
                    span: name.span,
                },
            );
            self.scope.space_mut(ns).insert(
                name.name.clone(),
                Binding {
                    qualified: binding.qualified,
                    owner: target,
                    span: name.span,
                },
            );
        }
    }

    /// Local-wins was rejected deliberately: adding a local `place` beside
    /// `import m (place)` would silently steal every existing call site.
    fn ambiguous(&mut self, name: &Ident, ns: Namespace, local: Span, module: &ModuleName) {
        self.diags.push(
            Diagnostic::error(
                codes::AMBIGUOUS_IMPORT,
                format!(
                    "the {} `{}` is both imported and defined in `{}`",
                    ns.describe(),
                    name.name,
                    self.scope.module
                ),
            )
            .primary(name.span, format!("imported from `{module}` here"))
            .secondary(local, format!("`{}` is also defined here", name.name))
            .note(format!(
                "drop it from the import list and write `import {module}`, then `{}::{}` where you \
                 mean the imported one",
                module.default_binder(),
                name.name
            ))
            .note("neither one silently wins: a call site would change meaning without changing"),
        );
    }

    fn duplicate_import(&mut self, name: &Ident, ns: Namespace, previous: &Declared) {
        self.diags.push(
            Diagnostic::error(
                codes::DUPLICATE_IMPORT,
                format!("the {} `{}` is imported twice", ns.describe(), name.name),
            )
            .primary(name.span, "imported again here")
            .secondary(previous.span, "first imported here")
            .note(format!(
                "remove one import, or import the module and write `<module>::{}`",
                name.name
            )),
        );
    }
}

struct Cycle {
    nodes: Vec<usize>,
    /// The import that closed it — the one worth pointing at.
    closing: Span,
}

/// One iterative DFS: it finds every cycle and, when there is none, leaves a
/// postorder that is exactly the dependency-first load order.
fn traverse(edges: &[Vec<(usize, Span)>]) -> (Vec<Cycle>, Vec<usize>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let n = edges.len();
    let mut color = vec![Color::White; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut cycles: Vec<Cycle> = Vec::new();
    let mut found: Vec<Vec<usize>> = Vec::new();
    let mut path: Vec<usize> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();

    for root in 0..n {
        if color[root] != Color::White {
            continue;
        }
        color[root] = Color::Gray;
        path.push(root);
        stack.push((root, 0));
        while let Some((v, edge)) = stack.pop() {
            if edge < edges[v].len() {
                stack.push((v, edge + 1));
                let (w, span) = edges[v][edge];
                match color[w] {
                    Color::White => {
                        color[w] = Color::Gray;
                        path.push(w);
                        stack.push((w, 0));
                    }
                    Color::Gray => {
                        let start = path.iter().position(|&x| x == w).unwrap_or(0);
                        let nodes: Vec<usize> = path[start..].to_vec();
                        let key = canonical(&nodes);
                        if !found.contains(&key) {
                            found.push(key);
                            cycles.push(Cycle {
                                nodes,
                                closing: span,
                            });
                        }
                    }
                    Color::Black => {}
                }
            } else {
                color[v] = Color::Black;
                path.pop();
                order.push(v);
            }
        }
    }
    (cycles, order)
}

/// Rotated so the smallest index leads: the same cycle reached from two roots
/// must compare equal, or it would be reported twice.
fn canonical(nodes: &[usize]) -> Vec<usize> {
    let Some(at) = nodes
        .iter()
        .enumerate()
        .min_by_key(|(_, n)| **n)
        .map(|(i, _)| i)
    else {
        return Vec::new();
    };
    nodes[at..].iter().chain(&nodes[..at]).copied().collect()
}

fn cycle_diagnostic(program: &Program, cycle: &Cycle) -> Diagnostic {
    let names: Vec<String> = cycle
        .nodes
        .iter()
        .map(|&i| program.modules[i].name.to_string())
        .collect();
    let first = names.first().cloned().unwrap_or_default();
    if names.len() == 1 {
        return Diagnostic::error(
            codes::MODULE_CYCLE,
            format!("module `{first}` imports itself"),
        )
        .primary(cycle.closing, "a module cannot import itself")
        .note("every name a module declares is already in scope in its own file");
    }
    let chain = names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(" -> ");
    Diagnostic::error(
        codes::MODULE_CYCLE,
        format!("module cycle: {chain} -> `{first}`"),
    )
    .primary(cycle.closing, "this import closes the cycle")
    .note(
        "each of these has to be checked before the next, and the last before the first, so \
             there is no order to check them in",
    )
    .note("move what they share into a module that all of them import")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Ident, ImportDecl, ImportKind, Module, ModuleName, QName};
    use crate::parser::parse_module;
    use ply_span::{SourceId, Span};

    fn module_of(source: SourceId, name: &str, text: &str) -> Module {
        parse_module(source, ModuleName::from_dotted(name), text).expect("test source parses")
    }

    fn program(files: &[(&str, &str)]) -> Program {
        Program {
            modules: files
                .iter()
                .enumerate()
                .map(|(i, (name, text))| module_of(SourceId(i as u32), name, text))
                .collect(),
        }
    }

    fn at(r: &Resolved, name: &str) -> usize {
        r.index_of(&ModuleName::from_dotted(name))
            .expect("module is in the program")
    }

    fn bare(name: &str) -> QName {
        QName::bare(Ident::new(name, Span::DUMMY))
    }

    fn qualified(module: &str, name: &str) -> QName {
        QName::qualified(
            Ident::new(module, Span::DUMMY),
            Ident::new(name, Span::DUMMY),
        )
    }

    fn errors(files: &[(&str, &str)]) -> Vec<Diagnostic> {
        match resolve(&program(files)) {
            Ok(_) => panic!("expected resolution to fail"),
            Err(diags) => diags,
        }
    }

    fn only(diags: &[Diagnostic], code: &str) -> Diagnostic {
        let hits: Vec<&Diagnostic> = diags.iter().filter(|d| d.code == code).collect();
        assert_eq!(hits.len(), 1, "expected one {code}, got {diags:?}");
        hits[0].clone()
    }

    fn text_of(d: &Diagnostic) -> String {
        let labels: Vec<&str> = d.labels.iter().map(|l| l.message.as_str()).collect();
        format!(
            "{} | {} | {}",
            d.message,
            labels.join(" / "),
            d.notes.join(" / ")
        )
    }

    #[test]
    fn a_name_resolves_across_three_modules() {
        let r = resolve(&program(&[
            ("base", "pub fn one() -> Int = 1"),
            (
                "middle",
                "import base (one)\npub fn two() -> Int = one() + one()",
            ),
            (
                "top",
                "import middle\nfn four() -> Int = middle::two() + middle::two()",
            ),
        ]))
        .expect("resolves");

        let middle = at(&r, "middle");
        let top = at(&r, "top");
        assert_eq!(
            r.lookup(middle, Namespace::Value, &bare("one"))
                .unwrap()
                .qualified
                .as_str(),
            "base.one"
        );
        assert_eq!(
            r.lookup(top, Namespace::Value, &qualified("middle", "two"))
                .unwrap()
                .qualified
                .as_str(),
            "middle.two"
        );
        let position = |name: &str| r.order.iter().position(|&i| i == at(&r, name)).unwrap();
        assert!(position("base") < position("middle"));
        assert!(position("middle") < position("top"));
    }

    #[test]
    fn a_diamond_import_visits_the_shared_module_once() {
        let r = resolve(&program(&[
            ("base", "pub fn one() -> Int = 1"),
            ("left", "import base (one)\npub fn l() -> Int = one()"),
            ("right", "import base (one)\npub fn r() -> Int = one()"),
            (
                "top",
                "import left\nimport right\nfn t() -> Int = left::l() + right::r()",
            ),
        ]))
        .expect("resolves");

        assert_eq!(r.order.len(), 4);
        let position = |name: &str| r.order.iter().position(|&i| i == at(&r, name)).unwrap();
        assert!(position("base") < position("left"));
        assert!(position("base") < position("right"));
        assert!(position("left") < position("top"));
        assert!(position("right") < position("top"));

        let top = at(&r, "top");
        assert!(
            r.lookup(top, Namespace::Value, &qualified("left", "l"))
                .is_ok()
        );
        // `base` is reachable only through `left` and `right`, never directly.
        let d = r
            .lookup(top, Namespace::Value, &qualified("base", "one"))
            .unwrap_err();
        assert_eq!(d.code, codes::UNKNOWN_MODULE);
    }

    #[test]
    fn a_private_name_cannot_be_reached_from_another_module() {
        let r = resolve(&program(&[
            (
                "store",
                "fn secret() -> Int = 1\npub fn public() -> Int = secret()",
            ),
            ("app", "import store\nfn use() -> Int = store::public()"),
        ]))
        .expect("resolves");

        let app = at(&r, "app");
        assert!(
            r.lookup(app, Namespace::Value, &qualified("store", "public"))
                .is_ok()
        );
        let d = r
            .lookup(app, Namespace::Value, &qualified("store", "secret"))
            .unwrap_err();
        assert_eq!(d.code, codes::PRIVATE_NAME);
        let shown = text_of(&d);
        assert!(shown.contains("private to module `store`"), "{shown}");
        assert!(shown.contains("pub fn secret"), "{shown}");
    }

    #[test]
    fn selectively_importing_a_private_name_is_rejected_at_the_import() {
        let diags = errors(&[
            ("store", "fn secret() -> Int = 1"),
            ("app", "import store (secret)\nfn use() -> Int = secret()"),
        ]);
        let d = only(&diags, codes::PRIVATE_NAME);
        let shown = text_of(&d);
        assert!(
            shown.contains("`secret` is private to module `store`"),
            "{shown}"
        );
        assert!(shown.contains("pub fn secret"), "{shown}");
    }

    #[test]
    fn importing_a_name_a_module_does_not_declare_names_what_it_exports() {
        let diags = errors(&[
            ("store", "pub fn place() -> Int = 1"),
            ("app", "import store (plce)"),
        ]);
        let d = only(&diags, codes::UNKNOWN_NAME);
        let shown = text_of(&d);
        assert!(shown.contains("declares no `plce`"), "{shown}");
        assert!(shown.contains("`store` exports: `place`"), "{shown}");
    }

    #[test]
    fn a_two_module_cycle_is_rejected_and_names_the_cycle() {
        let diags = errors(&[
            ("a", "import b\npub fn f() -> Int = 1"),
            ("b", "import a\npub fn g() -> Int = 1"),
        ]);
        let d = only(&diags, codes::MODULE_CYCLE);
        let shown = text_of(&d);
        assert!(shown.contains("`a` -> `b` -> `a`"), "{shown}");
        assert!(shown.contains("closes the cycle"), "{shown}");
    }

    #[test]
    fn a_three_module_cycle_prints_every_module_in_order() {
        let diags = errors(&[
            ("a", "import b\npub fn f() -> Int = 1"),
            ("b", "import c\npub fn g() -> Int = 1"),
            ("c", "import a\npub fn h() -> Int = 1"),
        ]);
        let d = only(&diags, codes::MODULE_CYCLE);
        assert!(
            d.message.contains("`a` -> `b` -> `c` -> `a`"),
            "{}",
            d.message
        );
    }

    #[test]
    fn a_self_import_is_the_length_one_cycle() {
        let diags = errors(&[("a", "import a\npub fn f() -> Int = 1")]);
        let d = only(&diags, codes::MODULE_CYCLE);
        assert!(d.message.contains("`a` imports itself"), "{}", d.message);
    }

    #[test]
    fn an_import_that_collides_with_a_local_definition_is_ambiguous() {
        let diags = errors(&[
            ("store", "pub fn place() -> Int = 1"),
            ("app", "import store (place)\nfn place() -> Int = 2"),
        ]);
        let d = only(&diags, codes::AMBIGUOUS_IMPORT);
        let shown = text_of(&d);
        assert!(shown.contains("both imported and defined"), "{shown}");
        assert!(shown.contains("imported from `store` here"), "{shown}");
        assert!(shown.contains("also defined here"), "{shown}");
    }

    #[test]
    fn qualifying_the_reference_fixes_an_ambiguous_import() {
        let r = resolve(&program(&[
            ("store", "pub fn place() -> Int = 1"),
            (
                "app",
                "import store\nfn place() -> Int = store::place() + 1",
            ),
        ]))
        .expect("resolves once the import binds the module rather than the name");

        let app = at(&r, "app");
        assert_eq!(
            r.lookup(app, Namespace::Value, &bare("place"))
                .unwrap()
                .qualified
                .as_str(),
            "app.place"
        );
        assert_eq!(
            r.lookup(app, Namespace::Value, &qualified("store", "place"))
                .unwrap()
                .qualified
                .as_str(),
            "store.place"
        );
    }

    #[test]
    fn two_imports_of_one_name_are_a_duplicate_import() {
        let diags = errors(&[
            ("left", "pub fn place() -> Int = 1"),
            ("right", "pub fn place() -> Int = 2"),
            ("app", "import left (place)\nimport right (place)"),
        ]);
        let d = only(&diags, codes::DUPLICATE_IMPORT);
        let shown = text_of(&d);
        assert!(shown.contains("imported twice"), "{shown}");
        assert!(shown.contains("first imported here"), "{shown}");
    }

    #[test]
    fn two_imports_binding_one_module_name_are_a_duplicate_import() {
        let diags = errors(&[
            ("left", "pub fn f() -> Int = 1"),
            ("right", "pub fn g() -> Int = 2"),
            ("app", "import left\nimport right as left"),
        ]);
        let d = only(&diags, codes::DUPLICATE_IMPORT);
        assert!(
            d.message.contains("bind the module name `left`"),
            "{}",
            d.message
        );
    }

    #[test]
    fn an_unknown_module_is_reported_at_the_import_path() {
        let diags = errors(&[("app", "import store.orders\nfn f() -> Int = 1")]);
        let d = only(&diags, codes::UNKNOWN_MODULE);
        assert!(
            d.message.contains("no module named `store.orders`"),
            "{}",
            d.message
        );
    }

    #[test]
    fn a_module_binder_lives_in_its_own_namespace() {
        let r = resolve(&program(&[
            ("orders", "pub fn place() -> Int = 1"),
            (
                "app",
                "import orders\nfn f(orders: Int) -> Int = orders + orders::place()",
            ),
        ]))
        .expect("a local named `orders` does not hide the module binder");

        let app = at(&r, "app");
        assert!(r.scopes[app].modules.contains_key(&Symbol::new("orders")));
        assert_eq!(
            r.lookup(app, Namespace::Value, &qualified("orders", "place"))
                .unwrap()
                .qualified
                .as_str(),
            "orders.place"
        );
    }

    #[test]
    fn an_alias_rebinds_the_module_and_the_default_binder_goes_away() {
        let r = resolve(&program(&[
            ("store.orders", "pub fn place() -> Int = 1"),
            (
                "app",
                "import store.orders as ord\nfn f() -> Int = ord::place()",
            ),
        ]))
        .expect("resolves");

        let app = at(&r, "app");
        assert!(
            r.lookup(app, Namespace::Value, &qualified("ord", "place"))
                .is_ok()
        );
        let d = r
            .lookup(app, Namespace::Value, &qualified("orders", "place"))
            .unwrap_err();
        assert_eq!(d.code, codes::UNKNOWN_MODULE);
    }

    #[test]
    fn a_selective_import_binds_no_module_binder() {
        let r = resolve(&program(&[
            ("orders", "pub fn place() -> Int = 1"),
            ("app", "import orders (place)\nfn f() -> Int = place()"),
        ]))
        .expect("resolves");

        let app = at(&r, "app");
        assert!(r.scopes[app].modules.is_empty());
        let d = r
            .lookup(app, Namespace::Value, &qualified("orders", "place"))
            .unwrap_err();
        assert_eq!(d.code, codes::UNKNOWN_MODULE);
        let shown = text_of(&d);
        assert!(shown.contains("add `import orders`"), "{shown}");
        assert!(
            shown.contains("a selective import binds no module name"),
            "{shown}"
        );
        assert!(
            shown.contains("this import brings in names from `orders`"),
            "{shown}"
        );
    }

    #[test]
    fn a_public_type_exports_its_constructors_and_a_private_one_does_not() {
        let r = resolve(&program(&[
            (
                "shapes",
                "pub type Shape = Circle(Int) | Square(Int)\ntype Hidden = Only(Int)",
            ),
            ("app", "import shapes\nfn f() -> Int = 1"),
        ]))
        .expect("resolves");

        let app = at(&r, "app");
        assert!(
            r.lookup(app, Namespace::Type, &qualified("shapes", "Shape"))
                .is_ok()
        );
        assert!(
            r.lookup(app, Namespace::Value, &qualified("shapes", "Circle"))
                .is_ok()
        );
        assert_eq!(
            r.lookup(app, Namespace::Value, &qualified("shapes", "Only"))
                .unwrap_err()
                .code,
            codes::PRIVATE_NAME
        );
        assert_eq!(
            r.lookup(app, Namespace::Type, &qualified("shapes", "Hidden"))
                .unwrap_err()
                .code,
            codes::PRIVATE_NAME
        );
    }

    #[test]
    fn effects_and_modules_of_the_same_name_coexist() {
        let r = resolve(&program(&[
            ("clock", "pub nondet effect clock { read now() -> Int }"),
            ("app", "import clock\nfn f() -> Int = 1"),
        ]))
        .expect("resolves");

        let app = at(&r, "app");
        assert!(
            r.lookup(app, Namespace::Effect, &qualified("clock", "clock"))
                .is_ok()
        );
        let clock = at(&r, "clock");
        assert_eq!(
            r.declarations[clock].effects[&Symbol::new("clock")]
                .qualified
                .as_str(),
            "clock.clock"
        );
    }

    #[test]
    fn a_name_missing_everywhere_points_at_the_module_that_exports_it() {
        let r = resolve(&program(&[
            ("store", "pub fn place() -> Int = 1"),
            ("app", "fn f() -> Int = 1"),
        ]))
        .expect("resolves");

        let d = r
            .lookup(at(&r, "app"), Namespace::Value, &bare("place"))
            .unwrap_err();
        assert_eq!(d.code, codes::UNKNOWN_NAME);
        let shown = text_of(&d);
        assert!(shown.contains("import store (place)"), "{shown}");
    }

    #[test]
    fn the_anonymous_module_keeps_its_names_bare() {
        let module = parse_module(SourceId(0), ModuleName::anonymous(), "fn f() -> Int = 1")
            .expect("parses");
        let r = resolve(&Program::single(module)).expect("resolves");
        assert_eq!(
            r.scopes[0].values[&Symbol::new("f")].qualified.as_str(),
            "f"
        );
        assert_eq!(r.order, vec![0]);
    }

    #[test]
    fn two_items_of_one_name_leave_the_first_binding_for_inference_to_report() {
        let r = resolve(&program(&[("app", "fn f() -> Int = 1\nfn f() -> Int = 2")]))
            .expect("a duplicate definition is inference's diagnostic, not resolution's");
        let first = &r.scopes[at(&r, "app")].values[&Symbol::new("f")];
        assert_eq!(first.qualified.as_str(), "app.f");
    }

    /// The graph walk is iterative for the same reason the definition-level SCC
    /// pass is: a generated project can be deeper than the native stack.
    #[test]
    fn a_deep_import_chain_does_not_overflow_the_stack() {
        let depth = 20_000;
        let modules: Vec<Module> = (0..depth)
            .map(|i| {
                let imports = if i + 1 < depth {
                    vec![ImportDecl {
                        path: vec![Ident::new(format!("m{}", i + 1), Span::DUMMY)],
                        kind: ImportKind::Module,
                        span: Span::DUMMY,
                    }]
                } else {
                    Vec::new()
                };
                Module {
                    name: ModuleName::from_dotted(format!("m{i}")),
                    source: SourceId(i as u32),
                    imports,
                    items: Vec::new(),
                }
            })
            .collect();

        let r = resolve(&Program { modules }).expect("a chain is acyclic");
        assert_eq!(r.order.len(), depth);
        assert_eq!(r.order[0], depth - 1, "the deepest import is checked first");
        assert_eq!(r.order[depth - 1], 0);
    }

    #[test]
    fn two_independent_cycles_are_both_reported() {
        let diags = errors(&[
            ("a", "import b\npub fn f() -> Int = 1"),
            ("b", "import a\npub fn g() -> Int = 1"),
            ("c", "import d\npub fn h() -> Int = 1"),
            ("d", "import c\npub fn i() -> Int = 1"),
        ]);
        let cycles: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code == codes::MODULE_CYCLE)
            .collect();
        assert_eq!(cycles.len(), 2, "{diags:?}");
    }

    #[test]
    fn importing_one_module_twice_is_not_a_cycle() {
        let r = resolve(&program(&[
            ("store", "pub fn place() -> Int = 1"),
            (
                "app",
                "import store\nimport store (place)\nfn f() -> Int = place() + store::place()",
            ),
        ]))
        .expect("resolves");
        let app = at(&r, "app");
        assert!(r.lookup(app, Namespace::Value, &bare("place")).is_ok());
        assert!(
            r.lookup(app, Namespace::Value, &qualified("store", "place"))
                .is_ok()
        );
    }
}
