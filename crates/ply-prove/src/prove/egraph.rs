//! Union-find over terms, plus the two structural rules a constructor obeys.
//!
//! A branch of the case analysis is exactly one of these plus a list of
//! disequalities. Branches are explored by cloning, which is affordable because
//! an obligation's term graph is small and which removes every question about
//! restoring state on backtracking.

use super::term::{Node, TermId, Terms};

#[derive(Clone, Default)]
pub struct Classes {
    parent: Vec<TermId>,
    /// Pairs the branch has asserted distinct. Checked after every closure.
    diseqs: Vec<(TermId, TermId)>,
    /// A contradiction was derived: two distinct constructors, two distinct
    /// literals, or an asserted disequality between terms proved equal.
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

    /// Terms created after this branch started — a case split's constructor
    /// fields — join as their own classes.
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
        // Lower id wins, so a branch's class representatives are a function of
        // the assertions rather than of the order they arrived in.
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

/// What a class is known to be, structurally. Two members of one class with
/// different shapes of the **same kind** are a contradiction; two of different
/// kinds are not, because a wrong sort must never be able to close a branch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Shape<'a> {
    Int(i64),
    Bool(bool),
    Str(&'a str),
    Ctor(&'a Node),
    List(usize),
    Record(&'a Node),
}

pub fn shape_of<'a>(terms: &'a Terms, t: TermId) -> Option<Shape<'a>> {
    match terms.node(t) {
        Node::Int(k) => Some(Shape::Int(*k)),
        Node::Bool(b) => Some(Shape::Bool(*b)),
        Node::Str(s) => Some(Shape::Str(s)),
        Node::Ctor { .. } => Some(Shape::Ctor(terms.node(t))),
        Node::List(items) => Some(Shape::List(items.len())),
        Node::Record(_) => Some(Shape::Record(terms.node(t))),
        _ => None,
    }
}

/// Whether two shapes of the same kind name different values. `None` means the
/// question was not asked of comparable shapes and nothing may be concluded.
pub fn conflict(a: &Shape<'_>, b: &Shape<'_>) -> Option<bool> {
    match (a, b) {
        (Shape::Int(x), Shape::Int(y)) => Some(x != y),
        (Shape::Bool(x), Shape::Bool(y)) => Some(x != y),
        (Shape::Str(x), Shape::Str(y)) => Some(x != y),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_representative_does_not_depend_on_the_union_order() {
        let mut forward = Classes::new(4);
        forward.union(0, 1);
        forward.union(1, 2);
        let mut backward = Classes::new(4);
        backward.union(2, 1);
        backward.union(1, 0);
        assert_eq!(forward.find(2), backward.find(0));
        assert_eq!(forward.find(2), 0);
    }

    #[test]
    fn an_asserted_disequality_between_equal_terms_contradicts() {
        let mut classes = Classes::new(3);
        classes.distinguish(0, 2);
        classes.check_diseqs();
        assert!(!classes.contradiction);
        classes.union(0, 2);
        classes.check_diseqs();
        assert!(classes.contradiction);
    }

    /// Shapes of different kinds never conflict. A sort the lowering guessed
    /// wrong must not be able to close a branch.
    #[test]
    fn shapes_of_different_kinds_conclude_nothing() {
        assert_eq!(conflict(&Shape::Int(1), &Shape::Bool(true)), None);
        assert_eq!(conflict(&Shape::List(0), &Shape::Int(0)), None);
        assert_eq!(conflict(&Shape::Int(1), &Shape::Int(2)), Some(true));
        assert_eq!(conflict(&Shape::Int(1), &Shape::Int(1)), Some(false));
    }
}
