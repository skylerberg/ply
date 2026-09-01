//! What the reference-counting pass claims, checked against what a run does.

use super::*;
use crate::build::*;
use crate::{Machine, Value};
use ply_span::codes;
use ply_syntax::ast::{BinOp, Expr, Item, Mode};

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

fn amb() -> Item {
    effect_def("amb", &[("flip", Mode::Read, false)])
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

#[track_caller]
fn list_of(v: &Value) -> Vec<i64> {
    match v {
        Value::List(xs) => xs.iter().map(int_of).collect(),
        other => panic!("expected a List, got {other}"),
    }
}

/// The point of the whole milestone: a list whose last owner is the caller grows in place, and the
/// same program written so that somebody else can still see it copies.
#[test]
fn a_uniquely_owned_list_is_updated_in_place_and_a_shared_one_is_copied() {
    let unique = block(
        vec![
            letv("xs", ints(&[1, 2, 3])),
            letv("ys", callv("push", vec![var("xs"), int(4)])),
        ],
        Some(var("ys")),
    );
    let (value, unique_stats) = run_expr(unique);
    assert_eq!(list_of(&value), [1, 2, 3, 4]);
    assert_eq!(unique_stats.updates, 1);
    assert_eq!(
        unique_stats.updates_in_place, 1,
        "`xs` is dead after the push, so nothing else can see the list it was handed"
    );

    // The only difference is that the tail reads `xs` as well, so the binding is still live when
    // `push` runs and the list has two owners.
    let shared = block(
        vec![
            letv("xs", ints(&[1, 2, 3])),
            letv("ys", callv("push", vec![var("xs"), int(4)])),
        ],
        Some(bin(
            BinOp::Add,
            callv("len", vec![var("xs")]),
            callv("len", vec![var("ys")]),
        )),
    );
    let (value, shared_stats) = run_expr(shared);
    assert_eq!(int_of(&value), 3 + 4, "the original list is unchanged");
    assert_eq!(shared_stats.updates, 1);
    assert_eq!(
        shared_stats.updates_in_place, 0,
        "`xs` is read after the push, so the push may not rewrite it"
    );
}

/// Appending in a fold is the shape reference counting exists for: without reuse every element
/// copies the whole accumulator, which is quadratic.
#[test]
fn an_accumulator_folded_over_is_reused_rather_than_recopied() {
    let e = callv(
        "fold",
        vec![
            callv("range", vec![int(0), int(64)]),
            list(Vec::new()),
            lam(&["acc", "x"], callv("push", vec![var("acc"), var("x")])),
        ],
    );
    let (value, stats) = run_expr(e);
    assert_eq!(list_of(&value).len(), 64);
    assert_eq!(stats.updates, 64);
    assert_eq!(
        stats.updates_in_place, 64,
        "every step of the fold owned the accumulator outright"
    );
    assert_eq!(stats.in_place(), Some(1.0));
}

/// A value built inside a region and returned from it outlives the region, and is still an ordinary
/// refcounted value on the other side — including for reuse, which is the observable half of "what
/// escapes is reference-counted".
#[test]
fn a_value_that_outlives_its_region_is_still_reference_counted_after_it() {
    let e = block(
        vec![letv(
            "xs",
            with_cell(
                "r",
                int(7),
                "c",
                list(vec![callv("cell_get", vec![var("c")]), int(1)]),
            ),
        )],
        Some(callv("push", vec![var("xs"), int(2)])),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(list_of(&value), [7, 1, 2]);
    assert_eq!(
        (stats.updates, stats.updates_in_place),
        (1, 1),
        "the escaping list left its region with one owner and was reused"
    );
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
    // Two bindings; `a` dies at the second statement and is dropped there, while `b` is still live
    // at the tail and its scope's end frees it for nothing.
    assert_eq!((stats.drop_sites, stats.drop_emitted), (2, 1));
    assert_eq!(stats.elided(), Some(0.75));
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

/// A free variable of a closure is never a last use: the closure holds the scope for as long as it
/// lives, and nothing in this body bounds that.
#[test]
fn a_variable_captured_by_a_closure_is_never_owned() {
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
    assert_eq!(
        stats.takes_moved, 0,
        "nothing may be moved out of a scope a closure captured"
    );
}

/// A last use in a position the machine holds the scope alone moves the value out instead of
/// cloning it — Perceus' "a last use is a move", and the only place the analysis has a runtime
/// effect of its own.
#[test]
fn a_last_use_the_scope_alone_can_see_is_moved_rather_than_cloned() {
    let e = block(vec![letv("xs", ints(&[1, 2, 3]))], Some(var("xs")));
    let (value, stats) = run_expr(e);
    assert_eq!(list_of(&value), [1, 2, 3]);
    assert!(
        stats.takes_moved >= 1,
        "the tail read of `xs` is a last use of a scope nothing else holds"
    );
}

/// The case ADR 0017 §3 says decides the design, asked of reference counting rather than of the
/// world: a resumption may not observe what an earlier one wrote.
#[test]
fn a_list_reachable_from_several_resumptions_is_copied_by_all_but_the_last() {
    for resumptions in 2..5usize {
        let body = block(
            vec![
                letv("xs", ints(&[1, 2])),
                letv("b", perform("amb", "flip", None, Vec::new())),
            ],
            Some(callv("len", vec![callv("push", vec![var("xs"), var("b")])])),
        );
        let sum = (1..resumptions).fold(call(var("k"), vec![int(0)]), |acc, i| {
            bin(BinOp::Add, acc, call(var("k"), vec![int(10 * i as i64)]))
        });
        let e = handle(
            body,
            vec![general_clause("amb", "flip", None, &[], "k", sum)],
        );
        let (value, stats) = run(vec![amb()], e);
        assert_eq!(
            int_of(&value),
            3 * resumptions as i64,
            "{resumptions} resumptions each pushed onto a two-element list of their own"
        );
        assert_eq!(stats.updates, resumptions as u64);
        assert!(
            stats.updates_in_place <= 1,
            "{resumptions} resumptions rewrote the list {} times; only the one nothing can \
             resume past may reuse it",
            stats.updates_in_place
        );
    }
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

/// Releasing a binding builds a new scope rather than writing through a shared link, which is what
/// keeps a closure that captured the scope reading what it captured.
#[test]
fn releasing_a_binding_leaves_a_scope_that_captured_it_intact() {
    use crate::Env;
    use ply_span::Symbol;

    let name = Symbol::new("xs");
    let scope = Env::empty().bind(name.clone(), Value::Int(7));
    let captured = scope.clone();
    let released = scope.release(std::slice::from_ref(&name));

    assert!(
        matches!(captured.lookup(&name), Some(crate::ScopeSlot::Live(_))),
        "the captured scope lost a binding it was holding"
    );
    assert!(matches!(
        released.lookup(&name),
        Some(crate::ScopeSlot::Released)
    ));
    assert!(matches!(
        scope.lookup(&name),
        Some(crate::ScopeSlot::Live(_))
    ));
}

/// Taking refuses whenever anything else can reach the binding, which is the whole safety argument
/// stated as a unit test.
#[test]
fn taking_refuses_a_scope_anybody_else_holds() {
    use crate::Env;
    use ply_span::Symbol;

    let name = Symbol::new("xs");
    let mut scope = Env::empty().bind(name.clone(), Value::Int(7));
    let held = scope.clone();
    assert!(
        scope.take_unique(&name).is_none(),
        "a scope two owners hold may not be emptied"
    );
    drop(held);
    assert!(
        matches!(scope.take_unique(&name), Some(Value::Int(7))),
        "a scope nothing else holds hands its binding over"
    );
    assert!(matches!(
        scope.lookup(&name),
        Some(crate::ScopeSlot::Released)
    ));
}

/// A binding released and then read is Ply's fault and stops the run, rather than walking past the
/// released slot to an outer binding of the same name and answering with a value nobody wrote.
#[test]
fn reading_a_released_binding_is_an_internal_error_and_not_an_outer_binding() {
    use crate::Env;
    use ply_span::Symbol;

    let name = Symbol::new("xs");
    let outer = Env::empty().bind(name.clone(), Value::Int(1));
    let inner = outer.bind(name.clone(), Value::Int(2));
    let released = inner.release(std::slice::from_ref(&name));
    assert!(
        matches!(released.lookup(&name), Some(crate::ScopeSlot::Released)),
        "the release must be visible rather than uncovering the shadowed binding"
    );
}

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

/// A shadowing binder that the enclosing activation does not read again is still a last use, so the
/// fix costs no reuse where there was nothing to protect: the inner list has one owner at the
/// `push` and is rewritten.
#[test]
fn shadowing_costs_no_reuse_where_the_outer_binding_is_dead() {
    let e = block(
        vec![
            letv("xs", ints(&[1, 2, 3])),
            letv("n", callv("len", vec![var("xs")])),
        ],
        Some(block(
            vec![
                letv("xs", ints(&[4, 5])),
                letv("ys", callv("push", vec![var("xs"), int(6)])),
            ],
            Some(bin(BinOp::Add, var("n"), callv("len", vec![var("ys")]))),
        )),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 6);
    assert_eq!((stats.updates, stats.updates_in_place), (1, 1));
}

/// The converse, and the one the fix is for: the outer binding is read after the shadowing scope,
/// so the inner `push` reuses the inner list while the outer read still finds the outer one.
#[test]
fn a_shadowed_outer_binding_is_neither_reused_nor_released() {
    let e = block(
        vec![letv("xs", ints(&[1, 2, 3]))],
        Some(bin(
            BinOp::Add,
            block(
                vec![
                    letv("xs", ints(&[4, 5])),
                    letv("ys", callv("push", vec![var("xs"), int(6)])),
                ],
                Some(callv("len", vec![var("ys")])),
            ),
            callv("len", vec![var("xs")]),
        )),
    );
    let (value, stats) = run_expr(e);
    assert_eq!(int_of(&value), 3 + 3);
    assert_eq!(
        (stats.updates, stats.updates_in_place),
        (1, 1),
        "the inner list is the one being pushed onto, and only it"
    );
}

/// A generated corpus, because the shapes somebody thinks to write down are not the shapes that
/// break a liveness analysis.
mod generated {
    use super::*;
    use crate::Interp;
    use crate::differential::compare_answers;
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

    /// Every generated program answers the same thing on both engines, and none of them reaches the
    /// released-binding path.
    #[test]
    fn the_reference_counted_engine_answers_what_the_uncounted_one_does() {
        let (program, resolved) = standalone(Vec::new());
        for seed in 0..4_000u64 {
            let e = Gen::new(seed).ints(4);
            let mut treewalk = Interp::for_program(&program, &resolved);
            let mut machine = Machine::for_program(&program, &resolved);
            let left = treewalk.eval_expr_for_test(&e);
            let right = machine.eval_expr_for_test(&e);
            if let Err(d) = &right {
                assert_ne!(
                    d.code,
                    codes::INTERNAL_ERROR,
                    "seed {seed} released a binding something still read: {}",
                    d.message
                );
            }
            if let Some(divergence) = compare_answers(
                &treewalk,
                &machine,
                &format!("generated program {seed}"),
                &left,
                &right,
            ) {
                panic!("seed {seed} diverged — {divergence}");
            }
        }
    }

    /// The same corpus under a handler that resumes twice, which the tree-walker refuses and so
    /// cannot audit.
    #[test]
    fn each_resumption_answers_as_though_it_were_the_only_one() {
        let items = vec![effect_def("amb", &[("flip", Mode::Read, false)])];
        let (program, resolved) = standalone(items);
        let mut compared = 0;
        for seed in 0..1_000u64 {
            let generated = Gen::new(seed ^ 0x5eed).ints(3);
            let alone = Machine::for_program(&program, &resolved).eval_expr_for_test(&generated);
            let Ok(Value::Int(g)) = alone else {
                continue;
            };
            let body = block(
                vec![
                    letv("xs", ints(&[1, 2])),
                    letv("b", perform("amb", "flip", None, Vec::new())),
                ],
                Some(bin(
                    BinOp::Add,
                    callv("len", vec![callv("push", vec![var("xs"), var("b")])]),
                    generated,
                )),
            );
            let e = handle(
                body,
                vec![general_clause(
                    "amb",
                    "flip",
                    None,
                    &[],
                    "k",
                    bin(
                        BinOp::Add,
                        call(var("k"), vec![int(1)]),
                        call(var("k"), vec![int(2)]),
                    ),
                )],
            );
            let mut machine = Machine::for_program(&program, &resolved);
            reset();
            let answer = machine.eval_expr_for_test(&e);
            let stats = stats();
            if let Err(d) = &answer {
                assert_ne!(
                    d.code,
                    codes::INTERNAL_ERROR,
                    "seed {seed} released a binding something still read: {}",
                    d.message
                );
            }
            let got = answer.as_ref().map(int_of).unwrap_or_else(|d| {
                panic!("seed {seed} failed: {}", d.message);
            });
            assert_eq!(
                got,
                2 * (3 + g),
                "seed {seed}: one resumption observed the other, {stats:?}"
            );
            compared += 1;
        }
        assert!(
            compared > 900,
            "only {compared} of the generated programs were comparable"
        );
    }
}
