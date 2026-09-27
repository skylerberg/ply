//! The program a unit compiles out of: the front end's answer over it, and each module's text.
//! Every table here is read from a [`Front`].

use ply_span::{SourceId, SourceMap, Span, Symbol};
use ply_ty::{CheckOutput, Front, LawInfo, Ordinal, SpecKind};
use std::collections::{HashMap, HashSet};
use std::sync::{PoisonError, RwLock};

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub front: &'static Front,
    /// [`Front::check`].
    pub check: &'static CheckOutput,
    tables: Tables,
    /// Each root's definition span where failures are reported: `tables.spans` until relocated.
    placed: RwLock<HashMap<String, Span>>,
    /// Each root's cache key, as the compiler emitted them, plus its definition's own text
    /// once attached. Empty: no caching.
    pub keys: HashMap<String, String>,
    /// Each module's source text, by module name; the emitter requires them.
    pub texts: HashMap<String, String>,
}

/// A test's root name, by its place among its module's tests; must match `ply_eval`'s runner.
pub fn test_root_name(ordinal: usize) -> Symbol {
    Symbol::new(format!("test#{ordinal}"))
}

/// A law's guard or body as a root, by its place among the module's laws; binders are its params.
pub fn law_root_name(ordinal: usize, part: &str) -> Symbol {
    Symbol::new(format!("law#{ordinal}.{part}"))
}

/// A definition's `requires` or `ensures` clause as a root: `<owner>#requires#<k>` over the
/// owner's parameters, `<owner>#ensures#<k>` over them and then `result`.
pub fn clause_root_name(owner: &Symbol, kind: &str, ordinal: usize) -> Symbol {
    Symbol::new(format!("{owner}#{kind}#{ordinal}"))
}

/// A program-wide name in `module`, as `ModuleName::qualify` spells it.
fn qualified(module: &Symbol, name: &Symbol) -> String {
    if module.as_str().is_empty() {
        return name.to_string();
    }
    format!("{module}.{name}")
}

/// One clause of `owner`, whose name is already program-wide.
fn clause_root(owner: &Symbol, kind: &str, ordinal: usize) -> String {
    format!("{owner}#{kind}#{ordinal}")
}

/// Everything a unit is emitted against, read once from the front end's answer.
struct Tables {
    /// Every constructor with its arity; the emitted C names tags by position here.
    ctors: Vec<(Symbol, usize)>,
    roots: Vec<String>,
    arities: HashMap<String, usize>,
    /// Roots whose parameters and answer are all `Int` or `Bool`.
    scalars: HashSet<String>,
    modules: Vec<(Symbol, SourceId)>,
    /// Each root's definition span; a clause's is its owner's.
    spans: HashMap<String, Span>,
}

impl Tables {
    fn of(front: &Front) -> Tables {
        let mut t = Tables {
            ctors: ctors_of(front),
            roots: front
                .emitter_roots
                .iter()
                .map(|r| r.root.to_string())
                .collect(),
            arities: front
                .emitter_roots
                .iter()
                .map(|r| (r.root.to_string(), r.arity))
                .collect(),
            scalars: front
                .emitter_roots
                .iter()
                .filter(|r| r.scalar)
                .map(|r| r.root.to_string())
                .collect(),
            modules: Vec::new(),
            spans: HashMap::new(),
        };
        t.spans_of(front);
        t
    }

    /// Each root's definition span and each module's source, by walking the front end's ordinals.
    fn spans_of(&mut self, front: &Front) {
        let laws: HashMap<&Symbol, &LawInfo> =
            front.check.laws.iter().map(|l| (&l.key, l)).collect();
        let tests_by_key: HashMap<&Symbol, Span> =
            front.check.tests.iter().map(|t| (&t.key, t.span)).collect();
        for (module, items) in &front.ordinals {
            if let Some(info) = front.check.modules.get(module) {
                self.modules.push((module.clone(), info.source));
            }
            let (mut ordinal, mut law_ordinal) = (0, 0);
            for item in items {
                match item {
                    Ordinal::Fn(name, kinds) => {
                        let Some(def) = front.check.defs.get(name) else {
                            continue;
                        };
                        self.spans.insert(name.to_string(), def.span);
                        let (mut requires, mut ensures) = (0, 0);
                        for kind in kinds {
                            let (kind, k) = match kind {
                                SpecKind::Requires => {
                                    requires += 1;
                                    ("requires", requires - 1)
                                }
                                SpecKind::Ensures => {
                                    ensures += 1;
                                    ("ensures", ensures - 1)
                                }
                            };
                            self.spans.insert(clause_root(name, kind, k), def.span);
                        }
                    }
                    Ordinal::Test(key) => {
                        let root = qualified(module, &test_root_name(ordinal));
                        if let Some(span) = tests_by_key.get(key) {
                            self.spans.insert(root, *span);
                        }
                        ordinal += 1;
                    }
                    Ordinal::Law(key) => {
                        let law = laws.get(key);
                        for part in ["guard", "body"] {
                            if part == "guard" && !law.is_some_and(|l| l.has_guard) {
                                continue;
                            }
                            let root = qualified(module, &law_root_name(law_ordinal, part));
                            if let Some(l) = law {
                                self.spans.insert(root, l.span);
                            }
                        }
                        law_ordinal += 1;
                    }
                }
            }
        }
    }

    /// The text of `root`'s definition in `texts`, which are by module name.
    fn written<'t>(&self, root: &str, texts: &'t HashMap<String, String>) -> Option<&'t str> {
        let span = self.spans.get(root)?;
        let (module, _) = self
            .modules
            .iter()
            .find(|(_, source)| *source == span.source)?;
        texts.get(module.as_str())?.get(span.range())
    }
}

/// The prelude's constructors, then each module's in program order, as the compiler emits them:
/// not `CheckOutput::ctors`' dependency order, or emitted tags would move with the import graph.
fn ctors_of(front: &Front) -> Vec<(Symbol, usize)> {
    front.emitter_ctors.clone()
}

impl Source {
    pub fn from_front(front: &'static Front) -> Source {
        let keys: HashMap<String, String> = front
            .keys
            .iter()
            .map(|(root, key)| (root.to_string(), key.clone()))
            .collect();
        let tables = Tables::of(front);
        Source {
            front,
            check: &front.check,
            placed: RwLock::new(tables.spans.clone()),
            tables,
            keys,
            texts: HashMap::new(),
        }
    }

    pub fn with_texts(mut self, texts: HashMap<String, String>) -> Source {
        // A site is an offset into its definition's text, whose layout the hash does not cover, so
        // a root whose text is not here keeps no key.
        let own = |root: &str| -> Option<String> {
            let written = self.tables.written(root, &texts)?;
            Some(blake3::hash(written.as_bytes()).to_hex()[..32].to_string())
        };
        self.keys = std::mem::take(&mut self.keys)
            .into_iter()
            .filter_map(|(root, key)| {
                let digest = own(&root)?;
                Some((root, format!("{key}@{digest}")))
            })
            .collect();
        self.texts = texts;
        self
    }

    /// The span of the definition `root` is part of: what its sites are offsets from.
    pub fn span_of(&self, root: &str) -> Option<Span> {
        self.placed
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(root)
            .copied()
    }

    /// Places every root where `front` has it, if each definition's text in `sources` is the one
    /// this was emitted over: a site is an offset into that text, so any other edit needs a rebuild.
    pub fn relocate(&self, front: &Front, sources: &SourceMap) -> bool {
        let now = Tables::of(front);
        let unchanged = now.roots == self.tables.roots
            && now.spans.len() == self.tables.spans.len()
            && self.tables.spans.keys().all(|root| {
                now.spans.get(root).is_some_and(|span| {
                    let is = sources
                        .get(span.source)
                        .and_then(|file| file.text.get(span.range()));
                    self.tables.written(root, &self.texts) == is
                })
            });
        if unchanged {
            *self.placed.write().unwrap_or_else(PoisonError::into_inner) = now.spans;
        }
        unchanged
    }

    /// Every constructor, by program-wide name, with its arity.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.tables.ctors.clone()
    }

    /// Every root: each `fn` in source order, then tests, then laws and clauses.
    pub fn functions(&self) -> Vec<String> {
        self.tables.roots.clone()
    }

    pub fn arity_of(&self, name: &str) -> Option<usize> {
        self.tables.arities.get(name).copied()
    }

    /// Whether every parameter and the answer are `Int` or `Bool`.
    pub fn scalar_signature(&self, name: &str) -> bool {
        self.tables.scalars.contains(name)
    }

    pub fn module_count(&self) -> usize {
        self.tables.modules.len()
    }

    pub fn module_names(&self) -> impl Iterator<Item = &Symbol> {
        self.tables.modules.iter().map(|(name, _)| name)
    }
}
