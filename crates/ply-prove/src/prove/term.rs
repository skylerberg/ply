//! The prover's term language: one hash-consed DAG per obligation.

use ply_core::Type;
use ply_span::Symbol;
use std::collections::HashMap;

pub type TermId = usize;

/// The names the prover reserves for operators it does not interpret.
pub const ADD: &str = "(+)";
pub const SUB: &str = "(-)";
pub const MUL: &str = "(*)";
pub const DIV: &str = "(/)";
pub const REM: &str = "(%)";
pub const CONCAT: &str = "(++)";
/// The bit operators, uninterpreted: seven symbols, because `-1 >> 1` and `-1 >>> 1` differ and one
/// symbol would prove them equal by congruence.
pub const BIT_AND: &str = "(&)";
pub const BIT_OR: &str = "(|)";
pub const BIT_XOR: &str = "(^)";
pub const BIT_NOT: &str = "(~)";
pub const SHL: &str = "(<<)";
pub const SHR: &str = "(>>)";
pub const USHR: &str = "(>>>)";
/// The ordered comparisons, uninterpreted.
pub const LT: &str = "(<)";
pub const LE: &str = "(<=)";
pub const GT: &str = "(>)";
pub const GE: &str = "(>=)";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
}

/// `Σ coefficient · term + konst`, ascending by term with no zero coefficients and no nested
/// [`Node::Lin`].
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Poly {
    pub monomials: Vec<(TermId, i128)>,
    pub konst: i128,
}

impl Poly {
    pub fn constant(k: i128) -> Poly {
        Poly {
            monomials: Vec::new(),
            konst: k,
        }
    }

    fn var(t: TermId) -> Poly {
        Poly {
            monomials: vec![(t, 1)],
            konst: 0,
        }
    }

    fn scaled(&self, factor: i128) -> Option<Poly> {
        let mut monomials = Vec::with_capacity(self.monomials.len());
        for (t, c) in &self.monomials {
            let c = c.checked_mul(factor)?;
            if c != 0 {
                monomials.push((*t, c));
            }
        }
        Some(Poly {
            monomials,
            konst: self.konst.checked_mul(factor)?,
        })
    }

    fn plus(&self, other: &Poly) -> Option<Poly> {
        let mut monomials: Vec<(TermId, i128)> = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.monomials.len() || j < other.monomials.len() {
            let take_left = match (self.monomials.get(i), other.monomials.get(j)) {
                (Some((a, _)), Some((b, _))) => a <= b,
                (Some(_), None) => true,
                _ => false,
            };
            let both = matches!(
                (self.monomials.get(i), other.monomials.get(j)),
                (Some((a, _)), Some((b, _))) if a == b
            );
            if both {
                let (t, a) = self.monomials[i];
                let c = a.checked_add(other.monomials[j].1)?;
                if c != 0 {
                    monomials.push((t, c));
                }
                i += 1;
                j += 1;
            } else if take_left {
                monomials.push(self.monomials[i]);
                i += 1;
            } else {
                monomials.push(other.monomials[j]);
                j += 1;
            }
        }
        Some(Poly {
            monomials,
            konst: self.konst.checked_add(other.konst)?,
        })
    }
}

/// One arm of a [`Node::Match`], already lowered.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Arm {
    pub test: ArmTest,
    /// One per constructor field, in field order.
    pub binds: Vec<TermId>,
    pub body: TermId,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ArmTest {
    /// A wildcard or a bare variable: taken whenever it is reached.
    Always,
    /// Taken iff the scrutinee's outermost constructor is this one.
    Ctor(Symbol),
    /// Taken iff the scrutinee equals this literal.
    Lit(TermId),
    /// A pattern the fragment does not decide — a nested constructor, a record or a list pattern.
    Undecidable,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Node {
    Int(i64),
    Bool(bool),
    Str(String),
    /// A `Decimal` literal, **normalized to its numeric value**: `1.5m` and `1.50m` are one node.
    Decimal {
        mantissa: i128,
        scale: u32,
    },
    Unit,
    /// A `forall` binder, `result`, a constructor field exposed by a case split, or an opaque
    /// stand-in for a term outside the fragment.
    Sym(u32),
    /// A top-level definition named as a value, or an operator the fragment leaves uninterpreted:
    /// `*` over two symbolics, `/`, `%`, `++`.
    Opaque(Symbol),
    Lin(Poly),
    /// An application of an arbitrary head.
    App {
        head: TermId,
        args: Vec<TermId>,
    },
    Ctor {
        name: Symbol,
        args: Vec<TermId>,
    },
    /// A list literal.
    List(Vec<TermId>),
    /// Ascending by field name.
    Record(Vec<(Symbol, TermId)>),
    Field {
        base: TermId,
        field: Symbol,
    },
    Not(TermId),
    And(TermId, TermId),
    Or(TermId, TermId),
    /// Over `Int` only.
    Cmp {
        op: CmpOp,
        lhs: TermId,
        rhs: TermId,
    },
    Eq {
        lhs: TermId,
        rhs: TermId,
    },
    If {
        cond: TermId,
        then_branch: TermId,
        else_branch: TermId,
    },
    Match {
        scrutinee: TermId,
        arms: Vec<Arm>,
    },
}

pub struct Terms {
    nodes: Vec<Node>,
    sorts: Vec<Option<Type>>,
    /// Set for anything the type system has already proved is an `Int`: a literal, a linear
    /// combination, and every operand of an arithmetic operator or a comparison.
    int: Vec<bool>,
    index: HashMap<Node, TermId>,
    next_sym: u32,
    pub true_id: TermId,
    pub false_id: TermId,
}

impl Default for Terms {
    fn default() -> Terms {
        Terms::new()
    }
}

impl Terms {
    pub fn new() -> Terms {
        let mut terms = Terms {
            nodes: Vec::new(),
            sorts: Vec::new(),
            int: Vec::new(),
            index: HashMap::new(),
            next_sym: 0,
            true_id: 0,
            false_id: 0,
        };
        terms.true_id = terms.boolean(true);
        terms.false_id = terms.boolean(false);
        terms
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn node(&self, t: TermId) -> &Node {
        &self.nodes[t]
    }

    pub fn nodes(&self) -> impl Iterator<Item = (TermId, &Node)> {
        self.nodes.iter().enumerate()
    }

    pub fn sort(&self, t: TermId) -> Option<&Type> {
        self.sorts[t].as_ref()
    }

    pub fn is_int(&self, t: TermId) -> bool {
        self.int[t]
    }

    /// Records what the type system already knows.
    pub fn force_int(&mut self, t: TermId) {
        self.int[t] = true;
    }

    pub fn mk(&mut self, node: Node, sort: Option<Type>) -> TermId {
        if let Some(&existing) = self.index.get(&node) {
            if self.sorts[existing].is_none() {
                self.sorts[existing] = sort;
            }
            return existing;
        }
        let id = self.nodes.len();
        let is_int =
            sort.as_ref().is_some_and(is_int_type) || matches!(node, Node::Int(_) | Node::Lin(_));
        self.index.insert(node.clone(), id);
        self.nodes.push(node);
        self.sorts.push(sort);
        self.int.push(is_int);
        id
    }

    pub fn int_lit(&mut self, k: i64) -> TermId {
        self.mk(Node::Int(k), Some(Type::int()))
    }

    pub fn boolean(&mut self, b: bool) -> TermId {
        self.mk(Node::Bool(b), Some(Type::bool()))
    }

    pub fn string(&mut self, s: String) -> TermId {
        self.mk(Node::Str(s), Some(Type::string()))
    }

    /// Interned by numeric value: trailing zeros are stripped so that two literals the language
    /// calls equal are one term.
    pub fn decimal(&mut self, mantissa: i128, scale: u32) -> TermId {
        let (mut mantissa, mut scale) = (mantissa, scale);
        while scale > 0 && mantissa % 10 == 0 {
            mantissa /= 10;
            scale -= 1;
        }
        self.mk(Node::Decimal { mantissa, scale }, Some(Type::decimal()))
    }

    pub fn unit(&mut self) -> TermId {
        self.mk(Node::Unit, Some(Type::unit()))
    }

    /// A fresh symbolic constant.
    pub fn sym(&mut self, sort: Option<Type>) -> TermId {
        let n = self.next_sym;
        self.next_sym += 1;
        self.mk(Node::Sym(n), sort)
    }

    /// The linear view of a term.
    pub fn poly(&self, t: TermId) -> Poly {
        match &self.nodes[t] {
            Node::Int(k) => Poly::constant(*k as i128),
            Node::Lin(p) => p.clone(),
            _ => Poly::var(t),
        }
    }

    pub fn intern_poly(&mut self, p: Poly) -> Option<TermId> {
        if p.monomials.is_empty() {
            return match i64::try_from(p.konst) {
                Ok(k) => Some(self.int_lit(k)),
                // A constant no `Int` can hold is not a Ply value, so there is nothing honest to
                // fold it to.
                Err(_) => None,
            };
        }
        if p.monomials.len() == 1 && p.monomials[0].1 == 1 && p.konst == 0 {
            return Some(p.monomials[0].0);
        }
        let id = self.mk(Node::Lin(p), Some(Type::int()));
        self.force_int(id);
        Some(id)
    }

    pub fn add(&mut self, a: TermId, b: TermId) -> Option<TermId> {
        let p = self.poly(a).plus(&self.poly(b))?;
        self.intern_poly(p)
    }

    pub fn sub(&mut self, a: TermId, b: TermId) -> Option<TermId> {
        let negated = self.poly(b).scaled(-1)?;
        let p = self.poly(a).plus(&negated)?;
        self.intern_poly(p)
    }

    pub fn neg(&mut self, a: TermId) -> Option<TermId> {
        let p = self.poly(a).scaled(-1)?;
        self.intern_poly(p)
    }

    /// Multiplication is in the fragment only when a factor is an integer literal.
    pub fn mul(&mut self, a: TermId, b: TermId) -> Option<TermId> {
        let (poly, factor) = match (&self.nodes[a], &self.nodes[b]) {
            (Node::Int(k), _) => (self.poly(b), *k as i128),
            (_, Node::Int(k)) => (self.poly(a), *k as i128),
            _ => return None,
        };
        let p = poly.scaled(factor)?;
        self.intern_poly(p)
    }

    /// Projection reduces over a record literal on sight; the same reduction up to a proved
    /// equality is a rule in the solver.
    pub fn field(&mut self, base: TermId, field: Symbol) -> TermId {
        if let Node::Record(fields) = &self.nodes[base]
            && let Some((_, v)) = fields.iter().find(|(n, _)| *n == field)
        {
            return *v;
        }
        let sort = match self.sorts[base].as_ref() {
            Some(Type::Record(fields)) => fields.get(&field).cloned(),
            _ => None,
        };
        self.mk(Node::Field { base, field }, sort)
    }

    pub fn not(&mut self, a: TermId) -> TermId {
        match self.nodes[a] {
            Node::Bool(b) => self.boolean(!b),
            Node::Not(inner) => inner,
            _ => self.mk(Node::Not(a), Some(Type::bool())),
        }
    }

    pub fn eq(&mut self, lhs: TermId, rhs: TermId) -> TermId {
        let (lhs, rhs) = if lhs <= rhs { (lhs, rhs) } else { (rhs, lhs) };
        self.project_fields(lhs, rhs);
        self.mk(Node::Eq { lhs, rhs }, Some(Type::bool()))
    }

    /// Intern `a.f` and `b.f` for every field of a record-sorted equality, so that the solver's
    /// extensionality rule has projections to compare.
    fn project_fields(&mut self, a: TermId, b: TermId) {
        let (Some(Type::Record(left)), Some(Type::Record(right))) =
            (self.sorts[a].clone(), self.sorts[b].clone())
        else {
            return;
        };
        if left.keys().ne(right.keys()) {
            return;
        }
        for name in left.keys() {
            let x = self.field(a, name.clone());
            let y = self.field(b, name.clone());
            if x != y {
                self.project_fields(x, y);
            }
        }
    }

    pub fn opaque(&mut self, name: &str, sort: Option<Type>) -> TermId {
        self.mk(Node::Opaque(Symbol::new(name)), sort)
    }
}

pub fn is_int_type(t: &Type) -> bool {
    matches!(t, Type::Con(name, args) if name.as_str() == "Int" && args.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_combination_is_canonical() {
        let mut terms = Terms::new();
        let x = terms.sym(Some(Type::int()));
        let zero = terms.int_lit(0);
        let plus_zero = terms.add(x, zero).unwrap();
        assert_eq!(plus_zero, x, "`x + 0` and `x` are one term");

        let one = terms.int_lit(1);
        let a = terms.add(x, one).unwrap();
        let b = terms.add(one, x).unwrap();
        assert_eq!(a, b, "addition commutes into one canonical form");

        let back = terms.sub(a, one).unwrap();
        assert_eq!(back, x);
    }

    /// The prover's own arithmetic must never wrap.
    #[test]
    fn a_coefficient_that_overflows_produces_no_term() {
        let mut terms = Terms::new();
        let x = terms.sym(Some(Type::int()));
        let big = terms.int_lit(i64::MAX);
        let scaled = terms.mul(x, big).unwrap();
        let again = terms.mul(scaled, big).unwrap();
        // i64::MAX cubed leaves i128.
        assert!(terms.mul(again, big).is_none());
    }

    /// A sum of two `i64::MAX`s is not an `Int`, so there is no literal to fold it to and the
    /// prover declines rather than wrapping.
    #[test]
    fn a_constant_outside_int_produces_no_term() {
        let mut terms = Terms::new();
        let big = terms.int_lit(i64::MAX);
        assert!(terms.add(big, big).is_none());
        let small = terms.int_lit(i64::MIN);
        assert!(terms.add(small, small).is_none());
    }

    #[test]
    fn multiplication_of_two_symbolics_is_not_arithmetic() {
        let mut terms = Terms::new();
        let x = terms.sym(Some(Type::int()));
        let y = terms.sym(Some(Type::int()));
        assert!(terms.mul(x, y).is_none());
    }

    #[test]
    fn projection_reduces_over_a_record_literal() {
        let mut terms = Terms::new();
        let v = terms.int_lit(7);
        let record = terms.mk(Node::Record(vec![(Symbol::new("balance"), v)]), None);
        assert_eq!(terms.field(record, Symbol::new("balance")), v);
    }

    #[test]
    fn every_fresh_symbol_is_a_distinct_term() {
        let mut terms = Terms::new();
        let a = terms.sym(None);
        let b = terms.sym(None);
        assert_ne!(a, b);
    }
}
