//! The program a unit runs over: the front end's answer, and the tables its C names it by. Every
//! table here is read from a [`Analysis`].

use ply_eval::{Analysis, EmitterRoot, Span, Symbol};
use std::collections::{HashMap, HashSet};

/// A checked program, borrowed for as long as the unit compiled from it lives.
pub struct Source {
    pub front: &'static Analysis,
    /// [`Analysis::check`].
    tables: Tables,
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

/// Everything a unit runs against, read once from the front end's answer.
struct Tables {
    /// Every constructor with its arity; the emitted C names tags by position here.
    ctors: Vec<(Symbol, usize)>,
    /// Each root's place in [`Analysis::emitter_roots`].
    rows: HashMap<String, usize>,
    /// Roots the compiler published pure.
    pures: HashSet<String>,
    /// Each root's definition span, where failures are reported; a clause's is its owner's. The
    /// root name the row holds is the name a site is reported under.
    spans: HashMap<String, Span>,
}

impl Tables {
    fn of(front: &Analysis) -> Tables {
        Tables {
            ctors: front.emitter_ctors.clone(),
            rows: front
                .emitter_roots
                .iter()
                .enumerate()
                .map(|(i, r)| (r.root.to_string(), i))
                .collect(),
            pures: front
                .emitter_roots
                .iter()
                .filter(|r| r.pure)
                .map(|r| r.root.to_string())
                .collect(),
            spans: front
                .emitter_roots
                .iter()
                .map(|r| (r.root.to_string(), r.span))
                .collect(),
        }
    }
}

impl Source {
    pub fn from_analysis(front: &'static Analysis) -> Source {
        Source {
            front,
            tables: Tables::of(front),
        }
    }

    /// The span of the definition `root` is part of: what its sites are offsets from.
    pub fn span_of(&self, root: &str) -> Option<Span> {
        self.tables.spans.get(root).copied()
    }

    /// Every constructor, by program-wide name, with its arity.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.tables.ctors.clone()
    }

    /// The row the compiler published for `name`, which says how its values read.
    pub fn row(&self, name: &str) -> Option<&'static EmitterRoot> {
        let front: &'static Analysis = self.front;
        self.tables.rows.get(name).map(|&i| &front.emitter_roots[i])
    }

    /// Whether the compiler published the root pure.
    pub fn pure(&self, name: &str) -> bool {
        self.tables.pures.contains(name)
    }
}
