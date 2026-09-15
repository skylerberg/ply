//! Union-find over terms, plus the two structural rules a constructor obeys.

use super::term::{Node, TermId, Terms};

#[derive(Clone, Default)]
pub struct Classes {
    parent: Vec<TermId>,
    /// Pairs the branch has asserted distinct.
    diseqs: Vec<(TermId, TermId)>,
    /// A contradiction was derived: two distinct constructors, two distinct literals, or an
    /// asserted disequality between terms proved equal.
    pub contradiction: bool,
}

impl Classes {
    pub fn new(size: usize) -> Classes {
        Classes {
            parent: (0..size).collect(),
            diseqs: Vec::new(),
            contradiction: false,
        }
    }

    /// Terms created after this branch started — a case split's constructor fields — join as their
    /// own classes.
    pub fn grow(&mut self, size: usize) {
        while self.parent.len() < size {
            self.parent.push(self.parent.len());
        }
    }

    pub fn find(&self, mut t: TermId) -> TermId {
        while self.parent[t] != t {
            t = self.parent[t];
        }
        t
    }

    /// `true` when the two were not already one class.
    pub fn union(&mut self, a: TermId, b: TermId) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return false;
        }
        // Lower id wins, so a branch's class representatives are a function of the assertions
        // rather than of the order they arrived in.
        let (keep, drop) = if a < b { (a, b) } else { (b, a) };
        self.parent[drop] = keep;
        true
    }

    pub fn equal(&self, a: TermId, b: TermId) -> bool {
        self.find(a) == self.find(b)
    }

    pub fn distinguish(&mut self, a: TermId, b: TermId) {
        self.diseqs.push((a, b));
    }

    pub fn diseqs(&self) -> &[(TermId, TermId)] {
        &self.diseqs
    }

    pub fn check_diseqs(&mut self) {
        if self.diseqs.iter().any(|(a, b)| self.equal(*a, *b)) {
            self.contradiction = true;
        }
    }

    /// Every term, grouped by class, in class-representative order.
    pub fn groups(&self, size: usize) -> Vec<(TermId, Vec<TermId>)> {
        let mut out: Vec<(TermId, Vec<TermId>)> = Vec::new();
        let mut index: Vec<Option<usize>> = vec![None; size];
        for t in 0..size {
            let rep = self.find(t);
            match index[rep] {
                Some(i) => out[i].1.push(t),
                None => {
                    index[rep] = Some(out.len());
                    out.push((rep, vec![t]));
                }
            }
        }
        out
    }
}

/// What a class is known to be, structurally.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Shape<'a> {
    Int(i64),
    Bool(bool),
    Str(&'a str),
    /// Already normalized by [`Terms::decimal`](super::term::Terms::decimal), so two shapes differ
    /// exactly when the two values do.
    Decimal(i128, u32),
    Ctor(&'a Node),
    List(usize),
    Record(&'a Node),
}

pub fn shape_of<'a>(terms: &'a Terms, t: TermId) -> Option<Shape<'a>> {
    match terms.node(t) {
        Node::Int(k) => Some(Shape::Int(*k)),
        Node::Bool(b) => Some(Shape::Bool(*b)),
        Node::Str(s) => Some(Shape::Str(s)),
        Node::Decimal { mantissa, scale } => Some(Shape::Decimal(*mantissa, *scale)),
        Node::Ctor { .. } => Some(Shape::Ctor(terms.node(t))),
        Node::List(items) => Some(Shape::List(items.len())),
        Node::Record(_) => Some(Shape::Record(terms.node(t))),
        _ => None,
    }
}

/// Whether two shapes of the same kind name different values.
pub fn conflict(a: &Shape<'_>, b: &Shape<'_>) -> Option<bool> {
    match (a, b) {
        (Shape::Int(x), Shape::Int(y)) => Some(x != y),
        (Shape::Bool(x), Shape::Bool(y)) => Some(x != y),
        (Shape::Str(x), Shape::Str(y)) => Some(x != y),
        (Shape::Decimal(m1, s1), Shape::Decimal(m2, s2)) => Some((m1, s1) != (m2, s2)),
        (Shape::List(x), Shape::List(y)) => Some(x != y),
        (Shape::Ctor(Node::Ctor { name: x, .. }), Shape::Ctor(Node::Ctor { name: y, .. })) => {
            Some(x != y)
        }
        (Shape::Record(Node::Record(x)), Shape::Record(Node::Record(y))) => {
            let names = |fields: &Vec<(ply_span::Symbol, TermId)>| {
                fields.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
            };
            Some(names(x) != names(y))
        }
        _ => None,
    }
}
