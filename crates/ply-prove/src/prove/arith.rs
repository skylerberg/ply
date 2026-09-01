//! Linear integer arithmetic, by Fourier–Motzkin elimination with integer tightening.

use std::collections::BTreeMap;

/// `Σ coefficients[v]·v + konst ≤ 0`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub coefficients: BTreeMap<usize, i128>,
    pub konst: i128,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feasibility {
    /// No integer solution exists.
    Infeasible,
    /// Everything else: satisfiable, out of budget, or out of range.
    Unknown,
}

/// The largest row set an elimination may grow to before the answer becomes `Unknown`.
const MAX_ROWS: usize = 512;

#[derive(Default)]
pub struct System {
    rows: Vec<Row>,
    /// Set when a caller's coefficient left `i128`.
    overflowed: bool,
}

impl System {
    /// `Σ cᵢvᵢ + k ≤ 0`.
    pub fn leq(&mut self, coefficients: BTreeMap<usize, i128>, konst: i128) {
        self.rows.push(Row {
            coefficients,
            konst,
        });
    }

    /// `Σ cᵢvᵢ + k = 0`, as the two inequalities.
    pub fn eq(&mut self, coefficients: BTreeMap<usize, i128>, konst: i128) {
        let mut negated = BTreeMap::new();
        let mut ok = true;
        for (v, c) in &coefficients {
            match c.checked_neg() {
                Some(c) => {
                    negated.insert(*v, c);
                }
                None => ok = false,
            }
        }
        match (ok, konst.checked_neg()) {
            (true, Some(k)) => self.leq(negated, k),
            _ => self.overflowed = true,
        }
        self.leq(coefficients, konst);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// `Infeasible` iff no assignment of integers to the variables satisfies every row.
    pub fn feasibility(mut self, budget: &mut u32) -> Feasibility {
        for row in &mut self.rows {
            tighten(row);
        }
        if self.rows.iter().any(is_contradiction) {
            return Feasibility::Infeasible;
        }

        loop {
            let Some(variable) = self.next_variable() else {
                break;
            };
            if !charge(budget, 1) {
                return Feasibility::Unknown;
            }
            let (positive, negative, mut kept) = self.partition(variable);
            if positive.len().saturating_mul(negative.len()) + kept.len() > MAX_ROWS {
                return Feasibility::Unknown;
            }
            for p in &positive {
                for n in &negative {
                    if !charge(budget, 1) {
                        return Feasibility::Unknown;
                    }
                    // A combination whose coefficients left `i128` is **dropped**, not fatal.
                    let Some(mut combined) = combine(p, n, variable) else {
                        continue;
                    };
                    tighten(&mut combined);
                    if is_contradiction(&combined) {
                        return Feasibility::Infeasible;
                    }
                    if !combined.coefficients.is_empty() {
                        kept.push(combined);
                    }
                }
            }
            self.rows = kept;
        }

        if self.rows.iter().any(is_contradiction) {
            Feasibility::Infeasible
        } else {
            Feasibility::Unknown
        }
    }

    /// The variable whose elimination produces the fewest rows, with the lowest index winning a
    /// tie, so the search is a function of the input alone.
    fn next_variable(&self) -> Option<usize> {
        let mut counts: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for row in &self.rows {
            for (v, c) in &row.coefficients {
                let entry = counts.entry(*v).or_default();
                if *c > 0 {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }
        }
        counts
            .into_iter()
            .min_by_key(|(v, (p, n))| (p * n, *v))
            .map(|(v, _)| v)
    }

    fn partition(&self, variable: usize) -> (Vec<Row>, Vec<Row>, Vec<Row>) {
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        let mut kept = Vec::new();
        for row in &self.rows {
            match row.coefficients.get(&variable) {
                Some(c) if *c > 0 => positive.push(row.clone()),
                Some(_) => negative.push(row.clone()),
                None => kept.push(row.clone()),
            }
        }
        (positive, negative, kept)
    }
}

fn charge(budget: &mut u32, cost: u32) -> bool {
    match budget.checked_sub(cost) {
        Some(left) => {
            *budget = left;
            true
        }
        None => {
            *budget = 0;
            false
        }
    }
}

/// `a·x + P ≤ 0` and `-b·x + N ≤ 0` with `a, b > 0` give `b·P + a·N ≤ 0`.
fn combine(positive: &Row, negative: &Row, variable: usize) -> Option<Row> {
    let a = *positive.coefficients.get(&variable)?;
    let b = -*negative.coefficients.get(&variable)?;
    let mut coefficients: BTreeMap<usize, i128> = BTreeMap::new();
    for (v, c) in &positive.coefficients {
        if *v == variable {
            continue;
        }
        coefficients.insert(*v, c.checked_mul(b)?);
    }
    for (v, c) in &negative.coefficients {
        if *v == variable {
            continue;
        }
        let scaled = c.checked_mul(a)?;
        let slot = coefficients.entry(*v).or_insert(0);
        *slot = slot.checked_add(scaled)?;
    }
    coefficients.retain(|_, c| *c != 0);
    Some(Row {
        coefficients,
        konst: positive
            .konst
            .checked_mul(b)?
            .checked_add(negative.konst.checked_mul(a)?)?,
    })
}

/// Divides a row through by the gcd of its coefficients and raises the constant to the next
/// integer, which is valid because the variables are integers.
fn tighten(row: &mut Row) {
    row.coefficients.retain(|_, c| *c != 0);
    let mut g: i128 = 0;
    for c in row.coefficients.values() {
        g = gcd(g, c.unsigned_abs() as i128);
    }
    if g <= 1 {
        return;
    }
    for c in row.coefficients.values_mut() {
        *c /= g;
    }
    row.konst = ceil_div(row.konst, g);
}

fn is_contradiction(row: &Row) -> bool {
    row.coefficients.is_empty() && row.konst > 0
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn ceil_div(a: i128, b: i128) -> i128 {
    debug_assert!(b > 0);
    let q = a.div_euclid(b);
    if a.rem_euclid(b) == 0 { q } else { q + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(usize, i128)], konst: i128) -> (BTreeMap<usize, i128>, i128) {
        (pairs.iter().copied().collect(), konst)
    }

    fn decide(build: impl FnOnce(&mut System)) -> Feasibility {
        let mut system = System::default();
        build(&mut system);
        let mut budget = 10_000;
        system.feasibility(&mut budget)
    }

    #[test]
    fn a_variable_below_and_above_a_bound_is_infeasible() {
        // x <= -1 and x >= 1.
        let f = decide(|s| {
            let (c, k) = row(&[(0, 1)], 1);
            s.leq(c, k);
            let (c, k) = row(&[(0, -1)], 1);
            s.leq(c, k);
        });
        assert_eq!(f, Feasibility::Infeasible);
    }

    /// The false instance, which matters more: a satisfiable system must never come back
    /// infeasible.
    #[test]
    fn a_satisfiable_system_is_not_infeasible() {
        let f = decide(|s| {
            // x >= 1, x <= 10.
            let (c, k) = row(&[(0, -1)], 1);
            s.leq(c, k);
            let (c, k) = row(&[(0, 1)], -10);
            s.leq(c, k);
        });
        assert_eq!(f, Feasibility::Unknown);
    }

    /// Feasible over ℚ, infeasible over ℤ.
    #[test]
    fn an_equation_no_integer_satisfies_is_infeasible() {
        let f = decide(|s| {
            let (c, k) = row(&[(0, 2)], -1);
            s.eq(c, k); // 2x - 1 = 0
        });
        assert_eq!(f, Feasibility::Infeasible);
    }

    #[test]
    fn a_transitive_chain_closes() {
        // x < y, y < z, z < x  ⟹  x + 1 <= y, y + 1 <= z, z + 1 <= x.
        let f = decide(|s| {
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (c, k) = row(&[(a, 1), (b, -1)], 1);
                s.leq(c, k);
            }
        });
        assert_eq!(f, Feasibility::Infeasible);
    }

    #[test]
    fn a_chain_that_is_merely_tight_stays_satisfiable() {
        let f = decide(|s| {
            for (a, b) in [(0, 1), (1, 2)] {
                let (c, k) = row(&[(a, 1), (b, -1)], 1);
                s.leq(c, k);
            }
        });
        assert_eq!(f, Feasibility::Unknown);
    }

    /// A coefficient that leaves `i128` is `Unknown`, never a claim.
    #[test]
    fn an_overflowing_combination_is_unknown() {
        let f = decide(|s| {
            let (c, k) = row(&[(0, i128::MAX / 2), (1, 1)], 0);
            s.leq(c, k);
            let (c, k) = row(&[(0, -(i128::MAX / 2)), (2, 1)], 1);
            s.leq(c, k);
            let (c, k) = row(&[(1, i128::MAX / 3)], i128::MAX / 3);
            s.leq(c, k);
        });
        assert_eq!(f, Feasibility::Unknown);
    }

    #[test]
    fn a_spent_budget_is_unknown_and_never_infeasible() {
        let mut system = System::default();
        for v in 0..8 {
            let (c, k) = row(&[(v, 1), (v + 1, -1)], 1);
            system.leq(c, k);
        }
        let (c, k) = row(&[(8, 1), (0, -1)], 1);
        system.leq(c, k);
        let mut budget = 1;
        assert_eq!(system.feasibility(&mut budget), Feasibility::Unknown);
    }

    #[test]
    fn ceil_div_rounds_toward_positive_infinity() {
        assert_eq!(ceil_div(3, 2), 2);
        assert_eq!(ceil_div(4, 2), 2);
        assert_eq!(ceil_div(-3, 2), -1);
        assert_eq!(ceil_div(-4, 2), -2);
        assert_eq!(ceil_div(0, 5), 0);
    }
}
