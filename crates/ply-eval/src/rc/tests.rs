//! What the reference-counting pass claims, checked against what a run does.

use super::*;
use crate::build::*;
use crate::{Machine, Value};
use ply_span::codes;
use ply_syntax::ast::{BinOp, Expr, Item};

/// Runs `e` on the machine with the counters cleared, and answers the value beside what reference
/// counting did while producing it.
#[track_caller]
fn run(items: Vec<Item>, e: Expr) -> (Value, Stats) {
    let (program, resolved) = standalone(items);
    let mut machine = Machine::for_program(&program, &resolved);
    reset();
    let value = machine.eval_expr_for_test(&e).expect("the program ran");
    (value, stats())
}

#[track_caller]
fn run_expr(e: Expr) -> (Value, Stats) {
    run(Vec::new(), e)
}

fn ints(xs: &[i64]) -> Expr {
    list(xs.iter().copied().map(int).collect())
}

#[track_caller]
fn int_of(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        other => panic!("expected an Int, got {other}"),
    }
}

/// The elision the pass performs, as a number.
#[test]
fn a_straight_line_body_elides_every_reference_counting_operation() {
    let e = block(
        vec![
            letv("a", ints(&[1])),
            letv("b", callv("push", vec![var("a"), int(2)])),
        ],
        Some(callv("len", vec![var("b")])),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 2);
    // `a` and `b` are read once each, and each read is the last one.
    assert_eq!((stats.dup_sites, stats.dup_emitted), (2, 0));
    // Two bindings, and neither pays a drop of its own: a last use moves the value out of its
    // slot, and the scope's end is one window truncation.
    assert_eq!((stats.drop_sites, stats.drop_emitted), (2, 0));
    assert_eq!(stats.elided(), Some(1.0));
}

/// A binding read twice keeps its `dup`, which is the half of the accounting that would make the
/// elision figure a lie if it were dropped.
#[test]
fn a_binding_read_twice_keeps_the_duplication_at_its_earlier_read() {
    let e = block(
        vec![letv("a", ints(&[1, 2]))],
        Some(bin(
            BinOp::Add,
            callv("len", vec![var("a")]),
            callv("len", vec![var("a")]),
        )),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 4);
    assert_eq!(
        (stats.dup_sites, stats.dup_emitted),
        (2, 1),
        "the left read is not the last one and must clone"
    );
}

/// A read to the left of a capture of the same binding is not a last use: the capture copies the
/// value later in evaluation order, so the earlier read must clone. The capture itself may be the
/// move — the closure then owns the value outright, which is exactly right.
#[test]
fn a_read_before_a_capture_is_cloned_and_the_program_still_answers() {
    let e = block(
        vec![letv("xs", ints(&[1, 2, 3]))],
        Some(callv(
            "len",
            vec![callv(
                "map",
                vec![var("xs"), lam(&["_x"], callv("len", vec![var("xs")]))],
            )],
        )),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 3);
    assert!(
        stats.dup_emitted >= 1,
        "the argument read of `xs` runs before the lambda captures it, so it clones: {stats:?}"
    );
}

/// A cell made to contain itself leaks, and says so.
#[test]
fn a_cell_that_contains_itself_is_reported_rather_than_collected() {
    let e = with_cell(
        "r",
        int(0),
        "c",
        block(
            vec![discard(callv(
                "cell_set",
                vec![var("c"), list(vec![var("c")])],
            ))],
            Some(int(1)),
        ),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 1, "the write is reported, never refused");
    assert_eq!(stats.cycles, 1);

    let cycles = take_cycles();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].code, codes::REFERENCE_CYCLE);
    assert!(
        cycles[0]
            .notes
            .iter()
            .any(|n| n.contains("does not collect cycles")),
        "the diagnostic must say why nothing will free it: {:?}",
        cycles[0].notes
    );
    assert!(
        cycles[0].labels.iter().any(|l| l.primary),
        "a cycle report needs the write that closed it"
    );
}

/// A value stored into a cell it does not reach is not a cycle, so the detector cannot turn into
/// noise on every `cell_set`.
#[test]
fn an_ordinary_cell_write_reports_nothing() {
    let e = with_cell(
        "r",
        int(0),
        "c",
        block(
            vec![discard(callv("cell_set", vec![var("c"), ints(&[1, 2, 3])]))],
            Some(callv("len", vec![callv("cell_get", vec![var("c")])])),
        ),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 3);
    assert_eq!(stats.cycles, 0);
    assert!(take_cycles().is_empty());
}

// The chain-level release and take-unique unit tests that stood here died with the chain: a moved
// slot is [`crate::window::SlotVal::Moved`], its read is an internal error rather than an outer
// binding of the same name, and both are pinned in `crate::window`'s own tests — a shadowed name
// cannot be uncovered because the two bindings are two different slots.

/// A binding read again after an inner scope reused its name.
#[test]
fn a_binding_reread_after_an_inner_scope_shadowed_it_survives() {
    let inner = block(vec![letv("x", int(9))], Some(int(0)));
    let e = block(
        vec![
            letv("x", ints(&[1, 2, 3])),
            letv("a", callv("len", vec![var("x")])),
            letv("b", inner),
        ],
        Some(callv("len", vec![var("x")])),
    );
    let (value, _) = run_expr(e);
    assert_eq!(int_of(&value), 3);
}

/// The same shape with the shadow inside a `match` arm, which is the other construct that binds
/// without opening a barrier.
#[test]
fn a_binding_reread_after_a_match_arm_shadowed_it_survives() {
    let e = block(
        vec![
            letv("x", ints(&[1, 2, 3])),
            letv("a", callv("len", vec![var("x")])),
            letv("b", match_(int(1), vec![arm(pvar("x"), int(0))])),
        ],
        Some(callv("len", vec![var("x")])),
    );
    let (value, _) = run_expr(e);
    assert_eq!(int_of(&value), 3);
}

/// And with `with_cell`'s binder, whose region makes it look unlike the other two and whose live
/// set is the same one.
#[test]
fn a_binding_reread_after_a_region_binder_shadowed_it_survives() {
    let e = block(
        vec![
            letv("c", ints(&[1, 2, 3])),
            letv("a", callv("len", vec![var("c")])),
            letv(
                "b",
                with_cell("r", int(9), "c", callv("cell_get", vec![var("c")])),
            ),
        ],
        Some(callv("len", vec![var("c")])),
    );
    let (value, _) = run_expr(e);
    assert_eq!(int_of(&value), 3);
}

/// A read to the left of a shadowing scope is not a last use when the outer binding is read to the
/// right of it, and marking it one would let the machine move the value out of a scope something
/// else still reads.
#[test]
fn a_read_left_of_a_shadowing_scope_is_not_owned_when_the_outer_binding_lives_on() {
    let shadow = block(vec![letv("xs", int(9))], Some(int(0)));
    let e = block(
        vec![letv("xs", ints(&[1, 2, 3]))],
        Some(bin(
            BinOp::Add,
            bin(BinOp::Add, callv("len", vec![var("xs")]), shadow),
            callv("len", vec![var("xs")]),
        )),
    );
    let (value, _) = run_expr(e);
    assert_eq!(int_of(&value), 6);
}

/// A generated corpus, because the shapes somebody thinks to write down are not the shapes that
/// break a liveness analysis.
mod generated {
    use super::*;
    use ply_syntax::ast::Stmt as AstStmt;

    const POOL: [&str; 3] = ["a", "b", "c"];

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Sort {
        Int,
        List,
        Cell,
    }

    struct Gen {
        state: u64,
        scope: Vec<(&'static str, Sort)>,
    }

    impl Gen {
        fn new(seed: u64) -> Gen {
            Gen {
                // Odd and large, so a zero seed is not a fixed point.
                state: seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1),
                scope: Vec::new(),
            }
        }

        fn next(&mut self) -> u64 {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            self.state
        }

        fn pick(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn name(&mut self) -> &'static str {
            POOL[self.pick(POOL.len() as u64) as usize]
        }

        fn visible(&mut self, sort: Sort) -> Option<&'static str> {
            let found: Vec<&'static str> = self
                .scope
                .iter()
                .rev()
                .scan(Vec::new(), |seen: &mut Vec<&'static str>, (n, s)| {
                    let first = !seen.contains(n);
                    seen.push(n);
                    Some((first && *s == sort).then_some(*n))
                })
                .flatten()
                .collect();
            if found.is_empty() {
                return None;
            }
            Some(found[self.pick(found.len() as u64) as usize])
        }

        fn ints(&mut self, depth: u32) -> Expr {
            if depth == 0 {
                return match self.pick(3) {
                    0 => int((self.pick(9) as i64) - 4),
                    1 => match self.visible(Sort::Int) {
                        Some(n) => var(n),
                        None => int(1),
                    },
                    _ => match self.visible(Sort::Cell) {
                        Some(n) => callv("cell_get", vec![var(n)]),
                        None => int(2),
                    },
                };
            }
            match self.pick(7) {
                0 => bin(BinOp::Add, self.ints(depth - 1), self.ints(depth - 1)),
                1 => callv("len", vec![self.lists(depth - 1)]),
                2 => self.block_of(depth - 1),
                3 => self.match_of(depth - 1),
                4 => self.cell_of(depth - 1),
                5 => self.lambda_of(depth - 1),
                _ => self.ints(depth - 1),
            }
        }

        fn lists(&mut self, depth: u32) -> Expr {
            if depth == 0 {
                return match self.visible(Sort::List) {
                    Some(n) if self.pick(2) == 0 => var(n),
                    _ => ints(&[1, 2, 3]),
                };
            }
            match self.pick(3) {
                0 => callv("push", vec![self.lists(depth - 1), self.ints(depth - 1)]),
                1 => match self.visible(Sort::List) {
                    Some(n) => var(n),
                    None => ints(&[4, 5]),
                },
                _ => list(vec![self.ints(depth - 1), self.ints(depth - 1)]),
            }
        }

        fn of_sort(&mut self, sort: Sort, depth: u32) -> Expr {
            match sort {
                Sort::List => self.lists(depth),
                _ => self.ints(depth),
            }
        }

        fn block_of(&mut self, depth: u32) -> Expr {
            let n = 1 + self.pick(3) as usize;
            let mut stmts: Vec<AstStmt> = Vec::new();
            let mut introduced = 0;
            for _ in 0..n {
                let name = self.name();
                let sort = if self.pick(2) == 0 {
                    Sort::Int
                } else {
                    Sort::List
                };
                // The value is generated before the binding enters scope, so a read inside it is a
                // read of whatever this name meant before.
                let value = self.of_sort(sort, depth);
                stmts.push(letv(name, value));
                self.scope.push((name, sort));
                introduced += 1;
            }
            let tail = self.ints(depth);
            self.scope.truncate(self.scope.len() - introduced);
            block(stmts, Some(tail))
        }

        fn match_of(&mut self, depth: u32) -> Expr {
            let scrutinee = self.ints(depth);
            let name = self.name();
            self.scope.push((name, Sort::Int));
            let bound = self.ints(depth);
            self.scope.pop();
            // Half the time every arm binds the name, because an arm that does not is enough on its
            // own to keep the outer binding's liveness and would hide a construct that drops it.
            let catch_all = if self.pick(2) == 0 {
                arm(pwild(), self.ints(depth))
            } else {
                self.scope.push((name, Sort::Int));
                let body = self.ints(depth);
                self.scope.pop();
                arm(pvar(name), body)
            };
            match_(
                scrutinee,
                vec![
                    guarded(pvar(name), bin(BinOp::Gt, var(name), int(0)), bound),
                    catch_all,
                ],
            )
        }

        fn cell_of(&mut self, depth: u32) -> Expr {
            let init = self.ints(depth);
            let name = self.name();
            self.scope.push((name, Sort::Cell));
            let body = self.ints(depth);
            self.scope.pop();
            with_cell("r", init, name, body)
        }

        fn lambda_of(&mut self, depth: u32) -> Expr {
            let name = self.name();
            let arg = self.ints(depth);
            self.scope.push((name, Sort::Int));
            let body = self.ints(depth);
            self.scope.pop();
            call(lam(&[name], body), vec![arg])
        }
    }

    /// No generated program reaches the released-binding path.
    #[test]
    fn no_generated_program_releases_a_binding_something_still_reads() {
        let (program, resolved) = standalone(Vec::new());
        for seed in 0..4_000u64 {
            let e = Gen::new(seed).ints(4);
            let mut machine = Machine::for_program(&program, &resolved);
            if let Err(d) = machine.eval_expr_for_test(&e) {
                assert_ne!(
                    d.code,
                    codes::INTERNAL_ERROR,
                    "seed {seed} released a binding something still read: {}",
                    d.message
                );
            }
        }
    }
}

/// [`record_sites`] clears on the way in as well as on the way out.
#[test]
fn arming_site_recording_clears_what_the_last_caller_left() {
    let span = Span::new(ply_span::SourceId(0), 0, 1);
    record_sites(true);
    note_update_of(true, 0, span);
    assert_eq!(sites().len(), 1, "the update was attributed to its span");

    // The caller panicked here: no disarm ran.
    record_sites(true);
    assert!(
        sites().is_empty(),
        "arming inherited the last caller's residue: {:?}",
        sites()
    );
    record_sites(false);
}
