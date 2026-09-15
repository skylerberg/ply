use ply_prove::prove::arith::{Feasibility, System, ceil_div};
use std::collections::BTreeMap;

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
