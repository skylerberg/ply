//! The reference graph over every top-level definition in a program, and the
//! strongly connected components that decide which definitions must be hashed
//! together.
//!
//! The index is deliberately module-*aware* and the hashes it feeds are
//! module-*blind*: which module owns a definition decides only what a name
//! denotes, and a resolved reference is written as the referent's hash. Moving a
//! definition therefore changes the keys of the output maps and nothing else.

use indexmap::IndexMap;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{
    EffectDef, FnDef, Ident, Item, LawDef, Module, ModuleName, Program, QName, TestDef, TypeDef,
    TypeDefBody,
};
use ply_syntax::resolve::{Binding, Resolved};
use rustc_hash::FxHashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub usize);

#[derive(Clone, Copy, Debug)]
pub enum NodeBody<'a> {
    Fn(&'a FnDef),
    Type(&'a TypeDef),
    Effect(&'a EffectDef),
}

#[derive(Clone, Debug)]
pub struct Node<'a> {
    /// The program-wide name, `store.orders.place`. It keys the output maps and
    /// never enters a hash.
    pub name: Symbol,
    pub simple: &'a Symbol,
    /// Index into [`ProgramIndex::modules`].
    pub module: usize,
    pub body: NodeBody<'a>,
}

#[derive(Clone, Debug)]
pub struct TestNode<'a> {
    /// `<module>.<label>`, which is what keeps two identically-labelled tests in
    /// different modules distinct.
    pub key: Symbol,
    pub module: usize,
    pub def: &'a TestDef,
}

/// A `law`, which is an item with a body and no name a reference could reach —
/// so it is indexed beside the tests rather than among the definitions.
#[derive(Clone, Debug)]
pub struct LawNode<'a> {
    /// `<module>.<label>`, which is what keeps two identically-labelled laws in
    /// different modules distinct.
    pub key: Symbol,
    pub module: usize,
    pub def: &'a LawDef,
}

/// One entry of the output order: modules in load order, items in source order.
#[derive(Clone, Copy, Debug)]
pub enum Entry {
    Def(NodeId),
    Test(usize),
    Law(usize),
}

#[derive(Clone, Debug)]
pub enum ValueTarget {
    Fn(NodeId),
    /// The type that declares the variant, plus the variant's own name — a
    /// constructor has no node of its own.
    Ctor {
        owner: NodeId,
        name: Symbol,
    },
}

#[derive(Debug, Default)]
struct ModuleItems {
    fns: FxHashMap<Symbol, NodeId>,
    types: FxHashMap<Symbol, NodeId>,
    effects: FxHashMap<Symbol, NodeId>,
    /// Variant name -> the type definition that declares it.
    ctors: FxHashMap<Symbol, NodeId>,
}

/// A declaring module and the name it declares the referent under. Resolution is
/// carried as a `(module, name)` pair rather than a qualified string so that
/// nothing here depends on how [`ModuleName::qualify`] spells a program-wide
/// name.
type Target = (usize, Symbol);

/// The unqualified names one module's bodies see, projected onto nodes. It is a
/// view of what resolution already decided, never a second decision.
#[derive(Debug, Default)]
struct ScopeIndex {
    values: FxHashMap<Symbol, Target>,
    types: FxHashMap<Symbol, Target>,
    effects: FxHashMap<Symbol, Target>,
    modules: FxHashMap<Symbol, usize>,
}

pub struct ProgramIndex<'a> {
    pub modules: Vec<&'a Module>,
    pub nodes: Vec<Node<'a>>,
    pub tests: Vec<TestNode<'a>>,
    pub laws: Vec<LawNode<'a>>,
    pub order: Vec<Entry>,
    /// The binder an `ensures` clause introduces beside the parameters. Owned
    /// here so the normalizer can push a reference to it onto its scope without
    /// borrowing from itself.
    pub result: Symbol,
    items: Vec<ModuleItems>,
    scopes: Vec<ScopeIndex>,
}

fn qualifier(q: &QName) -> Option<&Symbol> {
    q.module.as_ref().map(|m| &m.name)
}

/// The name `qualified` has inside the module that declares it. Inverse of
/// [`ModuleName::qualify`], and the identity for the anonymous module.
fn simple_of(qualified: &Symbol, owner: &ModuleName) -> Symbol {
    if owner.is_anonymous() {
        return qualified.clone();
    }
    let prefix = format!("{owner}.");
    qualified
        .as_str()
        .strip_prefix(prefix.as_str())
        .map(Symbol::new)
        .unwrap_or_else(|| qualified.clone())
}

impl<'a> ProgramIndex<'a> {
    /// One module with no project root, which can neither import nor be
    /// imported.
    pub fn single(module: &'a Module) -> Result<ProgramIndex<'a>, Vec<Diagnostic>> {
        ProgramIndex::build(vec![module], None)
    }

    /// Without a [`Resolved`] there is no module namespace, so an imported name
    /// would be written into the hash as the name the file spelled it with.
    /// Two files importing different modules under one binder would then hash
    /// alike, and editing the referent would move no hash here — a green cache
    /// over changed code, which nothing downstream can detect.
    fn imports_need_a_program(modules: &[&'a Module]) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for module in modules {
            for import in &module.imports {
                diags.push(
                    Diagnostic::error(
                        codes::UNKNOWN_MODULE,
                        format!("no module named `{}` in this program", import.module_name()),
                    )
                    .primary(import.path_span(), "not found")
                    .note(
                        "this module is being hashed on its own, so there is no namespace for \
                         an import to resolve against",
                    )
                    .note(
                        "hash every module of the program together — `hash_program` — so that a \
                         cross-module reference normalizes to the referent's hash rather than to \
                         its name",
                    ),
                );
            }
        }
        diags
    }

    pub fn of_program(
        program: &'a Program,
        resolved: &Resolved,
    ) -> Result<ProgramIndex<'a>, Vec<Diagnostic>> {
        ProgramIndex::build(program.modules.iter().collect(), Some(resolved))
    }

    /// `resolved` is the single source of truth for what an unqualified name
    /// means. Without it each module sees only its own items, and a module that
    /// imports is refused rather than hashed against a namespace that is not
    /// there.
    pub fn build(
        modules: Vec<&'a Module>,
        resolved: Option<&Resolved>,
    ) -> Result<ProgramIndex<'a>, Vec<Diagnostic>> {
        let mut nodes: Vec<Node<'a>> = Vec::new();
        let mut tests: Vec<TestNode<'a>> = Vec::new();
        let mut laws: Vec<LawNode<'a>> = Vec::new();
        let mut order: Vec<Entry> = Vec::new();
        let mut all_items: Vec<ModuleItems> = Vec::new();
        let mut diags = match resolved {
            Some(_) => Vec::new(),
            None => ProgramIndex::imports_need_a_program(&modules),
        };

        for (m, module) in modules.iter().copied().enumerate() {
            let mut items = ModuleItems::default();
            let mut spans = Spans::default();
            let push = |nodes: &mut Vec<Node<'a>>,
                        order: &mut Vec<Entry>,
                        body: NodeBody<'a>,
                        name: &'a Ident| {
                let id = NodeId(nodes.len());
                nodes.push(Node {
                    name: module.name.qualify(&name.name),
                    simple: &name.name,
                    module: m,
                    body,
                });
                order.push(Entry::Def(id));
                id
            };
            for item in &module.items {
                match item {
                    Item::Fn(d) => {
                        let id = push(&mut nodes, &mut order, NodeBody::Fn(d), &d.name);
                        declare(
                            &mut items.fns,
                            &mut spans.fns,
                            &d.name,
                            id,
                            "definition",
                            &mut diags,
                        );
                    }
                    Item::Type(d) => {
                        let id = push(&mut nodes, &mut order, NodeBody::Type(d), &d.name);
                        declare(
                            &mut items.types,
                            &mut spans.types,
                            &d.name,
                            id,
                            "type",
                            &mut diags,
                        );
                        if let TypeDefBody::Sum(variants) = &d.body {
                            for v in variants {
                                let (ctors, seen) = (&mut items.ctors, &mut spans.ctors);
                                declare(ctors, seen, &v.name, id, "variant", &mut diags);
                            }
                        }
                    }
                    Item::Effect(d) => {
                        let id = push(&mut nodes, &mut order, NodeBody::Effect(d), &d.name);
                        declare(
                            &mut items.effects,
                            &mut spans.effects,
                            &d.name,
                            id,
                            "effect",
                            &mut diags,
                        );
                    }
                    Item::Test(d) => {
                        order.push(Entry::Test(tests.len()));
                        tests.push(TestNode {
                            key: module.name.qualify(&Symbol::new(&d.name)),
                            module: m,
                            def: d,
                        });
                    }
                    // A law hashes like a test: an item with a body, its own
                    // discriminant, its binder types and guard and body
                    // normalized together.
                    Item::Law(d) => {
                        order.push(Entry::Law(laws.len()));
                        laws.push(LawNode {
                            key: module.name.qualify(&Symbol::new(&d.name)),
                            module: m,
                            def: d,
                        });
                    }
                    // Expansion has already appended this derive's generated
                    // definitions as `Item::Fn`, and those are the nodes. An
                    // `effect set` stands for no definition at all: the parser
                    // expanded every row that named it.
                    Item::Derive(_) | Item::EffectSet(_) => {}
                }
            }
            all_items.push(items);
        }

        if !diags.is_empty() {
            return Err(diags);
        }

        let scopes = build_scopes(&modules, &all_items, resolved);
        Ok(ProgramIndex {
            modules,
            nodes,
            tests,
            laws,
            order,
            result: Symbol::new("result"),
            items: all_items,
            scopes,
        })
    }

    pub fn is_effect(&self, node: NodeId) -> bool {
        self.nodes
            .get(node.0)
            .is_some_and(|n| matches!(n.body, NodeBody::Effect(_)))
    }

    pub fn value(&self, module: usize, q: &QName) -> Option<ValueTarget> {
        let (owner, name) = self.target(module, |s| &s.values, qualifier(q), &q.name.name)?;
        let items = self.items.get(owner)?;
        if let Some(&node) = items.fns.get(&name) {
            return Some(ValueTarget::Fn(node));
        }
        items
            .ctors
            .get(&name)
            .map(|&owner| ValueTarget::Ctor { owner, name })
    }

    pub fn ctor(&self, module: usize, q: &QName) -> Option<ValueTarget> {
        let (owner, name) = self.target(module, |s| &s.values, qualifier(q), &q.name.name)?;
        let items = self.items.get(owner)?;
        items
            .ctors
            .get(&name)
            .map(|&owner| ValueTarget::Ctor { owner, name })
    }

    pub fn ty(&self, module: usize, qual: Option<&Symbol>, name: &Symbol) -> Option<NodeId> {
        let (owner, name) = self.target(module, |s| &s.types, qual, name)?;
        self.items.get(owner)?.types.get(&name).copied()
    }

    pub fn effect(&self, module: usize, q: &QName) -> Option<NodeId> {
        let (owner, name) = self.target(module, |s| &s.effects, qualifier(q), &q.name.name)?;
        self.items.get(owner)?.effects.get(&name).copied()
    }

    /// A qualified name consults only the named module's declarations and never
    /// the current scope; a bare one consults only the current scope. Visibility
    /// is deliberately not checked: `pub` decides whether a reference is *legal*,
    /// which inference reports, and never which definition it denotes — so
    /// adding or removing it cannot move a hash.
    fn target(
        &self,
        module: usize,
        space: impl Fn(&ScopeIndex) -> &FxHashMap<Symbol, Target>,
        qual: Option<&Symbol>,
        name: &Symbol,
    ) -> Option<Target> {
        let scope = self.scopes.get(module)?;
        match qual {
            Some(binder) => scope
                .modules
                .get(binder)
                .map(|&owner| (owner, name.clone())),
            None => space(scope).get(name).cloned(),
        }
    }
}

#[derive(Default)]
struct Spans {
    fns: FxHashMap<Symbol, Span>,
    types: FxHashMap<Symbol, Span>,
    effects: FxHashMap<Symbol, Span>,
    ctors: FxHashMap<Symbol, Span>,
}

fn build_scopes(
    modules: &[&Module],
    items: &[ModuleItems],
    resolved: Option<&Resolved>,
) -> Vec<ScopeIndex> {
    (0..modules.len())
        .map(|m| match resolved.and_then(|r| r.scopes.get(m)) {
            Some(scope) => {
                let project = |bindings: &IndexMap<Symbol, Binding>| {
                    bindings
                        .iter()
                        .filter(|(_, b)| b.owner < modules.len())
                        .map(|(name, b)| {
                            (
                                name.clone(),
                                (b.owner, simple_of(&b.qualified, &modules[b.owner].name)),
                            )
                        })
                        .collect()
                };
                ScopeIndex {
                    values: project(&scope.values),
                    types: project(&scope.types),
                    effects: project(&scope.effects),
                    modules: scope
                        .modules
                        .iter()
                        .filter(|(_, (owner, _))| *owner < modules.len())
                        .map(|(binder, (owner, _))| (binder.clone(), *owner))
                        .collect(),
                }
            }
            None => ScopeIndex {
                values: items[m]
                    .fns
                    .keys()
                    .chain(items[m].ctors.keys())
                    .map(|name| (name.clone(), (m, name.clone())))
                    .collect(),
                types: items[m]
                    .types
                    .keys()
                    .map(|n| (n.clone(), (m, n.clone())))
                    .collect(),
                effects: items[m]
                    .effects
                    .keys()
                    .map(|n| (n.clone(), (m, n.clone())))
                    .collect(),
                modules: FxHashMap::default(),
            },
        })
        .collect()
}

fn declare(
    map: &mut FxHashMap<Symbol, NodeId>,
    spans: &mut FxHashMap<Symbol, Span>,
    name: &Ident,
    id: NodeId,
    what: &str,
    diags: &mut Vec<Diagnostic>,
) {
    if let Some(&first) = spans.get(&name.name) {
        diags.push(
            Diagnostic::error(
                codes::DUPLICATE_DEFINITION,
                format!("duplicate {what} `{}`", name.name),
            )
            .primary(name.span, "redefined here")
            .secondary(first, "first defined here")
            .note(format!(
                "rename one of them; every {what} in a module needs a distinct name"
            )),
        );
        return;
    }
    spans.insert(name.name.clone(), name.span);
    map.insert(name.name.clone(), id);
}

/// Tarjan's algorithm, iterative so that a deep dependency chain cannot blow the
/// stack. Components come out in reverse topological order: a component is
/// emitted only after every component it references, which is exactly the order
/// definitions have to be hashed in.
pub fn tarjan(n: usize, edges: &[Vec<NodeId>]) -> Vec<Vec<usize>> {
    const UNVISITED: usize = usize::MAX;
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut call: Vec<(usize, usize)> = Vec::new();

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        call.push((root, 0));

        while let Some(&mut (v, ref mut edge_ix)) = call.last_mut() {
            let outgoing = edges.get(v).map(|e| e.as_slice()).unwrap_or(&[]);
            if *edge_ix < outgoing.len() {
                let w = outgoing[*edge_ix].0;
                *edge_ix += 1;
                if w >= n {
                    continue;
                }
                if index[w] == UNVISITED {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut component = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        component.push(w);
                        if w == v {
                            break;
                        }
                    }
                    out.push(component);
                }
            }
        }
    }
    out
}

pub fn is_cyclic(component: &[usize], edges: &[Vec<NodeId>]) -> bool {
    match component {
        [only] => edges
            .get(*only)
            .is_some_and(|e| e.iter().any(|r| r.0 == *only)),
        _ => true,
    }
}
