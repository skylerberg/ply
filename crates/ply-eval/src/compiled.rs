//! Where a natively compiled body may be entered in place of evaluating one.
//!
//! The machine hands a backend a name, some scalars and a call budget, and takes
//! back at most one scalar. It hands over no arena, no stack, no handler stack,
//! no host binding, no `&mut Machine` and no way back into itself — so a backend
//! that cannot finish a call has changed nothing the program can observe. `None`
//! means "evaluate it yourself", and the machine does, from the top, with its own
//! diagnostics.
//!
//! That shape is what makes the invariants below hold by construction rather than
//! by a backend remembering them. Declining is the default for everything the
//! machine has not positively cleared:
//!
//! - **Effects.** A backend cannot `perform`: it is handed no machine to perform
//!   into. `Machine::compiled_answer` additionally refuses any definition whose
//!   *published* row is non-empty, which is the reviewable artifact and the same
//!   rule the constant memo reads (`memo::pure_by_published_row`). A machine
//!   built without a `CheckOutput` has no row to read and so enters nothing at
//!   all — which is most of this crate's own tests, and the reason the corpus
//!   tests assert an entry count rather than only a clean report.
//!
//!   > **Narrowed (R5 review, 2026-08-22): the row gate does not cover the whole
//!   > of "effects".** A definition that performs its operations and
//!   > **discharges them under its own `handle`** publishes an *empty* row —
//!   > `crates/ply-codegen-spike/tests/fixtures/hazards/effects.ply`'s
//!   > `handled` is declared `-> Int` with no row and type-checks, and both its
//!   > `footprint` and its `performed` come back empty — so this gate clears it
//!   > and the machine **offers it**. Entering it is then a real difference:
//!   > `ply-test` reports an `observed_footprint` (`report.rs`) and reads a
//!   > declared-but-unobserved atom as "a branch was not taken" (`slice.rs`),
//!   > and a native body performs nothing, so a user would be told a branch was
//!   > not taken when it was. It is **latent and not live** — the only backend
//!   > in the tree refuses `handle` at compile time (`jit.rs`) — but that is a
//!   > backend remembering the invariant, which is the property this module
//!   > claims to have engineered away. No published row can close it: `handled`
//!   > carries no fact distinguishing it from a genuinely pure definition.
//!   > `CONTRIBUTING.md` §"Things known to be broken" carries it as open.
//! - **Continuations.** Nothing runs in the machine while a body runs, so no
//!   continuation can be captured beneath a native activation and no handler
//!   clause can resume into one.
//! - **Cells, regions, tasks.** No arena crosses, and neither does a
//!   `Value::Cell`, `Value::Task` or `Value::Continuation` — see [`crossable`].
//! - **Diagnostics.** A backend cannot raise. A body that would fail answers
//!   `None` and the machine raises its own diagnostic from its own evaluation, so
//!   the code, message, spans, labels and notes are the interpreter's by
//!   construction.
//! - **The deterministic scheduler.** The hook is off inside a `simulate` region,
//!   so every `Access` a search reads is still recorded by the interpreter.
//! - **Recursion, and the whole of the machine's one bound.** `budget` is the
//!   machine's remaining nested calls. A backend that would exceed it answers
//!   `None`, and the machine raises the same
//!   `recursion limit of 10000 nested calls exceeded` both engines answer with.
//!   Nested calls is all there is to express: a machine asked for a frame
//!   ceiling ([`crate::Machine::with_max_frames`]) enters nothing at all, so no
//!   backend is ever offered a call under a limit it was not handed.
//!
//!   > **Closed (2026-08-24). This bullet used to read "and only one of the
//!   > machine's two bounds", under a refutation an R5 review took with the real
//!   > backend, no mutation, one entry and zero declines:** *"The machine has a
//!   > **second** bound — `DEFAULT_MAX_FRAMES`, 1,000,000 pending frames,
//!   > enforced in `Machine::push` — and nothing in this signature can express
//!   > it. A compiled body pushes **one** `Frame::Call` for the whole call; the
//!   > interpreter pends one frame per pending operand as well … `hog(9000)`
//!   > gives `Err(recursion limit of 1000000 pending frames exceeded)` alone and
//!   > `Ok(1350000)` with a backend attached."* The refutation was right and the
//!   > signature was not the thing to change. A native body pends no frames, so
//!   > any frame cost charged to an entry is an estimate, and an estimate that
//!   > differs from the interpreter's exact count is itself the divergence — the
//!   > only conservative charge, `budget × the body's static pend`, declines
//!   > every recursive entry the seam exists for. So the second bound went
//!   > instead: it was a resource guard on this engine's heap that had been
//!   > phrased as a program answer, and it was sensitive to how a body's
//!   > additions were spelled rather than to what the body did.
//!   > `Machine::with_max_frames` carries the measurement.
//!
//! What is **not** structural, stated plainly: a backend that answers an `Int`
//! the definition would not have produced is a wrong answer this boundary cannot
//! detect. It is caught by `--engine both` and the differential corpus, which
//! compare the machine against an independent tree-walker, and by nothing here.
//!
//! That claim is now demonstrated rather than argued —
//! `crates/ply-codegen-spike/tests/mutations.rs` runs eight deliberately wrong
//! backends against the kernel corpus and names what caught each. **One of them
//! is still not caught by any corpus in this tree**: no corpus can exercise the
//! published-row gate, because `benches/kernel` declares no effect and the
//! differential corpus declines effectful names. Read that file's header before
//! trusting this one.
//!
//! > **Narrowed (2026-08-24).** This read "and two of them were **not** caught:
//! > a backend that ignores its budget entirely overflows the native stack
//! > before any comparison runs, and no corpus in this tree can exercise the
//! > published-row gate at all." The first half is closed: a `--mutate` run is
//! > started as a child, and a child that dies by a signal is reported as a
//! > disagreement rather than ending the run. The second half stands.
//!
//! Two more, stated because they are limits rather than guarantees. A backend's
//! panic is not caught, so a backend bug aborts the process rather than becoming
//! a silent slow path. And a run with a backend attached is a third execution
//! strategy whose results a result cache must not keep — a rule that is **not
//! enforced**, because no shipping command can install one; see
//! [`crate::Machine::set_compiled`].
//!
//! No implementation of [`Compiled`] exists in this workspace. The doubles in
//! this module's tests are what keep it exercised.
//!
//! # What polices this seam, and what does not
//!
//! Counted rather than characterised, 2026-08-24, because
//! `CONTRIBUTING.md` §"Things known to be broken" item 13's first bullet is a
//! claim about coverage and a claim about coverage is checkable:
//!
//! | polices it | what it is |
//! | --- | --- |
//! | this module's tests | **32** tests over doubles built here: every gate in [`admit`] with a deletion recorded against it, the budget, the memo interaction, continuations, cells, regions, `Secret`, arity |
//! | `crates/ply-eval/tests/differential_corpus.rs` | **5** tests, two hand-built backends over `examples/` and `tests/fixtures/` — an answering one and a tree-walking one |
//! | `crates/ply-codegen-spike/tests/mutations.rs` | **13** tests, eight deliberately wrong backends, each asserted to have *fired* before it is asserted to be caught |
//! | `crates/ply-codegen-spike/tests/hazards.rs`, `mcts_kernel.rs` | **25** tests over the real cranelift backend |
//! | `mcts --mutate <corruption>` | the same eight corruptions at corpus scale — 2,396 generated cases; run by hand, nothing runs it for you |
//!
//! And what does not, which is the part worth writing down:
//!
//! - **`ply test --engine both` cannot install a backend at all.** `Compiled`
//!   and `set_compiled` occur **zero** times in `crates/ply-cli` — source and
//!   tests both. Every one of the five `set_compiled` call sites in the
//!   workspace is a test or the spike's own harness. So the shipping CLI catches
//!   **none** of the eight wrong backends, on any corpus, and `--engine both`
//!   compares the tree-walker against the machine and nothing else. A backend is
//!   reachable only from a test or from the spike's binaries.
//! - **Nothing enforces the result-cache rule.** A run with a backend attached
//!   is a third execution strategy whose results a cache must not keep; the rule
//!   is unenforced *because it is unreachable* — see [`crate::Machine::set_compiled`].
//!   Wiring a backend into the CLI is what would make it reachable, and it is
//!   gated on the entry-point defect (`CONTRIBUTING.md` item 9) rather than on
//!   this seam.
//! - **No corpus in the tree can exercise the published-row gate.**
//!   `benches/kernel` declares no effect, so every definition in it publishes an
//!   empty row and the gate has nothing to refuse; `ply-eval`'s differential
//!   corpus declines effectful names before the gate is reached. Closing it
//!   means a corpus that declares an effect, which does not exist yet.
//! - **A wrong `Int` is not caught here by anything.** It is caught by
//!   `--engine both` and the differential corpus comparing against an
//!   independent tree-walker, which is the sentence above this section.

use crate::value::{Closure, ClosureKind, Value};
use ply_core::CheckOutput;
use ply_span::Symbol;
use ply_syntax::ast::Program;

/// A source of natively compiled bodies for a program's definitions.
pub trait Compiled {
    /// Whether these bodies were compiled from `program`. Pointer identity, as
    /// [`crate::code::Lowering::describes`] is and for the same reason: a
    /// bisection builds programs whose definitions carry the names of the ones
    /// they replace (`crates/ply-eval/tests/hoist_staleness_audit.rs`).
    fn describes(&self, program: &Program) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    ///
    /// `args` are [`Value::Int`] and [`Value::Bool`] only and the answer must be
    /// too; the machine checks both sides and evaluates the definition itself
    /// when either fails, so an unsound backend produces a slow program rather
    /// than a wrong one.
    ///
    /// `budget` is at least 1 and is the nested calls left before the machine's
    /// `max_calls`. A body that would recurse past it answers `None`; the machine
    /// then re-evaluates and raises its own `recursion limit of 10000 nested
    /// calls exceeded`, which is the guarantee `limit.rs` exists to keep in both
    /// engines.
    ///
    /// It is also the *only* bound to honour, which is what makes this one
    /// `usize` sufficient rather than merely convenient. How many frames the
    /// interpreter would have pended running this body is not a fact about the
    /// program and no answer may turn on it; a machine that was nonetheless
    /// asked for a frame ceiling declines to enter anything.
    ///
    /// The machine has committed nothing when this is called and commits nothing
    /// on `None`, so declining is free after no work or after a whole body. That
    /// holds only while this signature hands over no route back into the machine
    /// — see `Machine::compiled_answer`.
    ///
    /// A panic here is not caught. `Machine` is not `UnwindSafe` and swallowing a
    /// backend's panic would turn a loud backend bug into a silent slow path.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;
}

/// What may cross this boundary, in either direction: the unboxed scalar kinds
/// and nothing else.
///
/// The list is short on purpose, and every exclusion closes a hazard rather than
/// being conservative for its own sake.
///
/// - No [`Value::Float`]: the codegen spike has no `Float` path and lowers `+` as
///   `Int` arithmetic whatever the operands are, which ADR 0019 §5 item 4
///   records. Behind this boundary that is a decline; without it, it is a working
///   program that starts raising at a call site nobody opted into.
/// - No [`Value::Str`] and no [`Value::Decimal`]: the same lowering compares them
///   as `Int`s.
/// - No [`Value::Secret`]: a credential cannot reach a constant pool or a
///   `format!` in code the machine did not write.
/// - Nothing that can reach a [`Value::Cell`], [`Value::Task`],
///   [`Value::Continuation`] or [`Value::Closure`], so no handle into this run's
///   world crosses and no heap value is cloned across — which is also why the
///   unique-ownership path `frame.rs` sets up has nothing to lose here.
///
/// This is a capability cut as much as a safety one: nothing taking or returning
/// a `List`, `Map`, `Record`, `Str` or `Float` can be entered at all.
pub(crate) fn crossable(value: &Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_))
}

/// Which gate refused a call, named rather than collapsed into `None`.
///
/// A refusal that carries no reason is a refusal any *other* gate can satisfy,
/// and this seam has already paid for that once: [`Gate::Anonymous`] was
/// asserted by `an_anonymous_closure_is_never_offered` — whose doc said "the
/// name gate is what refuses it" — and replacing that gate with a fabricated
/// empty [`Symbol`] left the test, and all of this crate's unit tests, green,
/// because [`Gate::PublishedRow`] refuses an unknown name one line later. The
/// test named a mechanism it could not see. Every variant here has a test that
/// asserts *it*, and the table in this module's test header records the deletion
/// that was run against each one and the test that went red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Not a body this machine lowered: a tree-walker closure, a constructor or
    /// a builtin.
    NotLoweredCode,
    /// An argument this boundary does not carry — see [`crossable`].
    ArgumentShape,
    /// Inside a `simulate` region.
    SimulateRegion,
    /// A body with no program-wide name: a lambda.
    Anonymous,
    /// The published effect row is non-empty, or there is no row to read at all.
    PublishedRow,
    /// No nested call left before the machine's own bound.
    Budget,
}

/// The name a backend may be offered this call under and the budget to offer it
/// with, or the gate that refused the call.
///
/// Split out of `Machine::compiled_answer` so that each gate is a fact a test
/// can assert directly. The machine half of the seam is the two lines around it:
/// the backend lookup, which is what a machine with no backend fails, and the
/// [`crossable`] test on the answer.
///
/// The gates are ordered cheapest and most-discriminating first, and
/// [`Gate::ArgumentShape`] deliberately precedes the row lookup: with a backend
/// attached, a call taking a record, a list or a string is refused on one
/// discriminant test per argument and never hashes a [`Symbol`] into
/// [`CheckOutput::defs`]. That ordering is a cost claim, so
/// `the_shape_gate_is_reached_before_the_row_is_looked_up` asserts it.
///
/// - **[`Gate::NotLoweredCode`]**: a tree-walker closure carries a program-wide
///   name (`interp.rs:118`) over a body that is a deep clone rather than a node
///   of the program, and `Interp` is the independent oracle `--engine both`
///   audits against. Routing its closures into compiled code would audit the
///   backend against itself.
/// - **[`Gate::SimulateRegion`]**: off inside a `simulate` region for the reason
///   `Machine::constant` is off there — an allocation a search depends on must
///   not be skipped, and an `Access` never recorded is an interleaving never
///   explored. `record_cell_access` and `record_alloc_access` are no-ops outside
///   one, so this single gate is the whole partial-order story: a compiled body
///   cannot fail to record what nothing is recording.
/// - **[`Gate::PublishedRow`]**: necessary and not sufficient, exactly as the
///   memo's note says — an empty row still permits a definition that opens its
///   own `with_cell`, and (see this module's header) one that discharges its own
///   effects under its own `handle`. Outside a `simulate` region that allocation
///   is unobservable, which is the same argument `Machine::constant` rests on.
/// - **[`Gate::Budget`]**: a zero budget declines, and the interpreted path
///   raises the machine's own call-limit diagnostic rather than a backend's.
pub(crate) fn admit<'a>(
    closure: &'a Closure,
    args: &[Value],
    in_simulate: bool,
    check: Option<&CheckOutput>,
    max_calls: usize,
    calls: usize,
) -> Result<(&'a Symbol, usize), Gate> {
    if !matches!(closure.kind, ClosureKind::Code { .. }) {
        return Err(Gate::NotLoweredCode);
    }
    if !args.iter().all(crossable) {
        return Err(Gate::ArgumentShape);
    }
    if in_simulate {
        return Err(Gate::SimulateRegion);
    }
    let name = closure.name.as_ref().ok_or(Gate::Anonymous)?;
    if !crate::memo::pure_by_published_row(check, name) {
        return Err(Gate::PublishedRow);
    }
    let budget = max_calls.checked_sub(calls).ok_or(Gate::Budget)?;
    if budget == 0 {
        return Err(Gate::Budget);
    }
    Ok((name, budget))
}

/// Doubles, because nothing in this workspace implements [`Compiled`].
///
/// `rm -r crates/ply-codegen-spike` must leave a seam that is still exercised
/// rather than a `pub` API with no live caller, so every gate in
/// `Machine::compiled_answer` is asserted here against a backend that would
/// violate it. Two of them are deliberately wrong backends: one answers a value
/// this boundary refuses, one answers the wrong `Int`. The second is not caught
/// here and the test says so.
///
/// # What each gate's test is worth
///
/// A test named after a mechanism it cannot see is this crate's known defect
/// (`CONTRIBUTING.md` §"Things known to be broken" item 13), so each gate's
/// deletion was run against the whole of `cargo test -p ply-eval --lib` — 526
/// tests — and the tests that went red are recorded. Nothing below is reasoning
/// about what a deletion would do.
///
/// The 526 is that suite's size when the table was taken. The frame-ceiling
/// change (`CONTRIBUTING.md` items 9 and 10) has since added one test to it, so
/// a re-run reads 527 green and the reds below are unchanged — none of the six
/// deletions touches the ceiling gate, which lives in `Machine::compiled_answer`
/// rather than here.
///
/// Re-running it: mutate, `touch` the file, and check the run actually printed
/// `Compiling ply-eval` before believing its result. Cargo fingerprints on
/// second-granular mtimes, so a script that rewrites this file and invokes
/// `cargo test` within the same second can be served the previous artifact —
/// which reports the *previous* mutation's reds under the current one's name.
/// Every row below was taken with that check enforced.
///
/// | Deletion from [`admit`] | Red |
/// |---|---|
/// | the [`ClosureKind::Code`] test | `a_body_this_machine_did_not_lower_is_refused_by_the_kind_gate`, `a_tree_walker_closure_with_a_program_wide_name_is_never_offered` |
/// | the [`crossable`] test on arguments | 5, incl. `an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate`, `a_secret_is_never_offered_and_never_accepted` |
/// | the `in_simulate` test | `a_call_inside_a_simulate_region_is_refused_by_the_region_gate`, `nothing_is_offered_inside_a_simulate_region` |
/// | the name test, replaced by a fabricated empty [`Symbol`] | `an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate`, **and nothing else** |
/// | the published-row test | 5, incl. `a_definition_whose_published_row_is_not_empty_is_never_offered` |
/// | the `budget == 0` test | `the_last_nested_call_is_refused_by_the_budget_gate`, `the_budget_is_the_machines_remaining_depth_and_never_reaches_zero` |
/// | `checked_sub` weakened to `saturating_sub` | **nothing — 526 still green** |
///
/// > **Corrected in place (2026-08-24).** The last two rows were one row, and it
/// > read: *"| the budget test, replaced by `saturating_sub` |
/// > `the_last_nested_call_is_refused_by_the_budget_gate`,
/// > `the_budget_is_the_machines_remaining_depth_and_never_reaches_zero` |"*.
/// > That credits one mutation with another mutation's result. Re-run one at a
/// > time: deleting the `budget == 0` test turns those two red, and weakening
/// > `checked_sub` to `saturating_sub` turns **nothing** red. The second is not
/// > a hole — `saturating_sub` answers `0` where `checked_sub` answers `None`,
/// > and `0` is what the next line already refuses, so the two spellings are
/// > indistinguishable to any test. `checked_sub` is there to say in the source
/// > that a machine whose `max_calls` was lowered under a live stack is a case
/// > somebody thought about; it is a spelling, not a gate, and it is the one
/// > line in this function no test can bite.
///
/// The fourth row is the whole of item 13. Before this block that deletion was
/// caught by nothing at all: the row gate refuses an unpublished name one line
/// later, so a fabricated name produced the same *behaviour* through a
/// different mechanism, and `an_anonymous_closure_is_never_offered` — which
/// claimed the name gate by name — stayed green. One test covering one gate is
/// the thinnest row in the table, and it is thin because the gate below it
/// masks it; that is the fact the row is recording, not a gap in it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::*;
    use crate::differential::compare_answers;
    use crate::env::Env;
    use crate::limit::DEFAULT_MAX_CALLS;
    use crate::machine::Machine;
    use crate::value::{Closure, ClosureKind};
    use crate::{Interp, argv};
    use ply_core::{CheckOutput, check_program};
    use ply_span::Diagnostic;
    use ply_syntax::ast::{BinOp, Expr, ExprKind, Item, Program};
    use ply_syntax::resolve::Resolved;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    type Reply = dyn Fn(&Symbol, &[Value], usize) -> Option<Value>;

    /// One call the machine offered a backend.
    #[derive(Clone, Debug, PartialEq)]
    struct Offer {
        name: Symbol,
        args: Vec<Value>,
        budget: usize,
    }

    /// A backend that records every offer and answers by a closure the test
    /// supplies. `describes` is the pointer comparison a real backend owes.
    struct Double {
        /// Never dereferenced. A backend may not borrow the program — see the
        /// `compiled` field on `Machine` for why the field is `'static`.
        program: *const Program,
        reply: Box<Reply>,
        offers: RefCell<Vec<Offer>>,
    }

    impl Double {
        fn over(
            program: &Program,
            reply: impl Fn(&Symbol, &[Value], usize) -> Option<Value> + 'static,
        ) -> Rc<Double> {
            Rc::new(Double {
                program: std::ptr::from_ref(program),
                reply: Box::new(reply),
                offers: RefCell::new(Vec::new()),
            })
        }

        /// Declines everything and remembers what it was offered.
        fn declining(program: &Program) -> Rc<Double> {
            Double::over(program, |_, _, _| None)
        }

        /// Answers `value` for `name` and declines everything else.
        fn answering(program: &Program, name: &str, value: Value) -> Rc<Double> {
            let wanted = Symbol::new(name);
            Double::over(program, move |asked, _, _| {
                (*asked == wanted).then(|| value.clone())
            })
        }

        fn offers(&self) -> Vec<Offer> {
            self.offers.borrow().clone()
        }

        fn names(&self) -> Vec<String> {
            self.offers
                .borrow()
                .iter()
                .map(|o| o.name.as_str().to_string())
                .collect()
        }

        fn forget(&self) {
            self.offers.borrow_mut().clear();
        }
    }

    impl Compiled for Double {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
            self.offers.borrow_mut().push(Offer {
                name: name.clone(),
                args: args.to_vec(),
                budget,
            });
            (self.reply)(name, args, budget)
        }
    }

    /// A program and the check output the purity gate reads. Held together
    /// because a `Machine` borrows all three and must drop before they do.
    struct Checked {
        program: Program,
        resolved: Resolved,
        check: CheckOutput,
    }

    fn checked(items: Vec<Item>) -> Checked {
        let (program, resolved) = standalone(items);
        let check = match check_program(&program, &resolved) {
            Ok(check) => check,
            Err(ds) => panic!("the program under test does not check: {ds:#?}"),
        };
        Checked {
            program,
            resolved,
            check,
        }
    }

    impl Checked {
        fn machine(&self) -> Machine<'_> {
            Machine::new(&self.program, &self.resolved, &self.check)
        }
    }

    /// `Result<Value, Diagnostic>` has no `PartialEq`, and the comparison this
    /// wants is the one `differential` makes: the code, the message, every label
    /// with its span, and every note.
    fn rendered(outcome: &Result<Value, Diagnostic>) -> String {
        format!("{outcome:?}")
    }

    /// `Diagnostic` has no `PartialEq`, so a failing outcome cannot be compared
    /// with `assert_eq!`. A test that wanted a value says so here.
    #[track_caller]
    fn ok(outcome: Result<Value, Diagnostic>) -> Value {
        match outcome {
            Ok(value) => value,
            Err(d) => panic!("expected a value, got {}: {}", d.code, d.message),
        }
    }

    fn double_def() -> Item {
        fn_def("double", &["x"], bin(BinOp::Mul, var("x"), int(2)))
    }

    #[test]
    fn a_machine_with_no_backend_never_asks_and_never_counts() {
        let c = checked(vec![double_def()]);
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_counts(), (0, 0));
        assert_eq!(machine.compiled_refusals(), 0);
    }

    /// The property every other claim rests on: with a backend that answers
    /// nothing, the machine is the machine.
    #[test]
    fn a_backend_that_declines_everything_changes_nothing() {
        let items = vec![
            double_def(),
            fn_def("half", &["x"], bin(BinOp::Div, var("x"), int(2))),
            fn_def("boom", &["x"], bin(BinOp::Div, var("x"), int(0))),
            fn_def("table", &[], list(vec![int(1), int(2), int(3)])),
        ];
        let subjects = [
            callv("double", vec![int(21)]),
            bin(
                BinOp::Add,
                callv("double", vec![int(1)]),
                callv("half", vec![int(8)]),
            ),
            callv("boom", vec![int(1)]),
            callv("double", vec![string("not a number")]),
            bin(BinOp::Add, callv("table", vec![]), callv("table", vec![])),
        ];

        let c = checked(items);
        let baseline: Vec<String> = {
            let mut machine = c.machine();
            subjects
                .iter()
                .map(|e| rendered(&machine.eval_expr_for_test(e)))
                .collect()
        };

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        for (e, want) in subjects.iter().zip(&baseline) {
            assert_eq!(&rendered(&machine.eval_expr_for_test(e)), want);
        }
        let (entries, declines) = machine.compiled_counts();
        assert_eq!(entries, 0);
        assert!(declines > 0, "the backend was never offered a call at all");
        assert_eq!(machine.compiled_refusals(), 0);
        assert!(
            backend.offers().iter().any(|o| o.name.as_str() == "double"),
            "the backend was never offered `double`: {:?}",
            backend.offers()
        );
    }

    #[test]
    fn an_accepted_call_gets_its_name_its_arguments_and_a_budget_and_its_answer_is_used() {
        let c = checked(vec![double_def()]);
        let backend = Double::answering(&c.program, "double", Value::Int(84));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());

        // 84 rather than 42: the compiled answer was used and the body was not
        // evaluated.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(84)
        );
        assert_eq!(
            backend.offers(),
            vec![Offer {
                name: Symbol::new("double"),
                args: vec![Value::Int(21)],
                budget: DEFAULT_MAX_CALLS,
            }]
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    #[test]
    fn a_bool_crosses_in_both_directions_and_a_float_crosses_in_neither() {
        let c = checked(vec![
            fn_def("not", &["b"], un(ply_syntax::ast::UnOp::Not, var("b"))),
            fn_def("twice", &["f"], bin(BinOp::Add, var("f"), var("f"))),
        ]);

        let backend = Double::answering(&c.program, "not", Value::Bool(true));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("not", vec![boolean(true)]))),
            Value::Bool(true)
        );
        assert_eq!(backend.offers().len(), 1);
        assert_eq!(backend.offers()[0].args, vec![Value::Bool(true)]);
        drop(machine);

        // ADR 0019 §5 item 4: the spike's fragment accepts `Float` arithmetic and
        // fails on it at run time. A `Float` argument is refused before any
        // backend sees it, so that hole cannot reach a program.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("twice", vec![float(1.5)]))),
            Value::Float(3.0)
        );
        assert!(backend.offers().is_empty(), "a `Float` reached a backend");
        // Control: the same definition with an `Int` is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("twice", vec![int(2)]))),
            Value::Int(4)
        );
        assert_eq!(backend.names(), vec!["twice"]);
    }

    /// The boundary checks the *kind* of what comes back, in every profile. A
    /// `debug_assert!` here would leave the release half — the half a measurement
    /// runs in — unexercised.
    #[test]
    fn an_answer_this_boundary_refuses_is_declined_and_the_body_is_evaluated() {
        let c = checked(vec![double_def()]);
        for refused in [
            Value::str("a string"),
            Value::Float(1.0),
            Value::Unit,
            Value::List(Default::default()),
        ] {
            let backend = Double::answering(&c.program, "double", refused.clone());
            let mut machine = c.machine();
            machine.set_compiled(backend.clone());
            assert_eq!(
                ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
                Value::Int(42),
                "a backend answering {refused:?} was believed"
            );
            assert_eq!(machine.compiled_counts(), (0, 1));
            assert_eq!(machine.compiled_refusals(), 1);
            assert_eq!(backend.offers().len(), 1);
        }
    }

    /// Stated as a limitation, not a guarantee: the seam checks a kind and never
    /// a value. What catches a wrong `Int` is the independent engine.
    #[test]
    fn a_wrong_int_passes_the_seam_and_is_caught_only_by_the_other_engine() {
        let c = checked(vec![double_def()]);
        let backend = Double::answering(&c.program, "double", Value::Int(99));
        let mut machine = c.machine();
        machine.set_compiled(backend);
        let subject = callv("double", vec![int(21)]);
        let from_machine = machine.eval_expr_for_test(&subject);

        assert_eq!(from_machine.as_ref().ok(), Some(&Value::Int(99)));
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(
            machine.compiled_refusals(),
            0,
            "the boundary reported a violation it cannot actually see"
        );

        let mut treewalk = Interp::new(&c.program, &c.resolved, &c.check);
        let from_treewalk = treewalk.eval_expr_for_test(&subject);
        assert_eq!(from_treewalk.as_ref().ok(), Some(&Value::Int(42)));
        assert!(
            compare_answers(
                &treewalk,
                &machine,
                "the expression under test",
                &from_treewalk,
                &from_machine,
            )
            .is_some(),
            "`--engine both` did not report a backend that answered 99 for 42"
        );
    }

    /// `hoist_staleness_audit.rs`'s hazard: a bisection builds a program whose
    /// definitions carry the names of the ones they replace.
    #[test]
    fn a_backend_built_over_another_program_is_ignored() {
        let elsewhere = checked(vec![fn_def("double", &["x"], int(1000))]);
        let backend = Double::answering(&elsewhere.program, "double", Value::Int(84));

        let c = checked(vec![double_def()]);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_counts(), (0, 0));
        assert!(backend.offers().is_empty());
    }

    /// `interp.rs` mints a closure per top-level `fn` carrying the program-wide
    /// name, and one handed into a machine reaches `enter_code` through the
    /// `ClosureKind::Fn` arm. Routing those into a backend would audit the
    /// backend against itself.
    #[test]
    fn a_tree_walker_closure_with_a_program_wide_name_is_never_offered() {
        let body = bin(BinOp::Mul, var("x"), int(2));
        let call_it = callv("f", vec![int(21)]);
        let c = checked(vec![double_def()]);

        let treewalk_closure = Value::Closure(Arc::new(Closure {
            name: Some(Symbol::new("double")),
            kind: ClosureKind::Fn {
                params: vec![Symbol::new("x")],
                body: Arc::new(body),
                env: Env::empty(),
                module: 0,
            },
        }));

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let got = machine.eval_expr_in(&call_it, 0, &[(Symbol::new("f"), treewalk_closure)]);
        assert_eq!(ok(got), Value::Int(42));
        assert!(
            backend.offers().is_empty(),
            "a tree-walker closure was routed into a backend: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (0, 0));

        // Control: the machine's own `double`, under the same name the closure
        // above carries, is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A lambda is `ClosureKind::Code` with no name, and nothing anonymous
    /// reaches a backend — a backend is keyed by program-wide name and has
    /// nothing to answer for an anonymous body.
    ///
    /// > **Corrected in place (2026-08-24).** This doc used to say "so the name
    /// > gate is what refuses it". It could not see that: replacing
    /// > `closure.name.as_ref()?` with a fabricated empty `Symbol` left this
    /// > test — and every one of this crate's unit tests — green, because the
    /// > row gate refuses the fabricated name one line later. What it asserts is
    /// > the behaviour. The *mechanism* is asserted by
    /// > [`an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate`],
    /// > which is the test that goes red under that substitution.
    #[test]
    fn an_anonymous_closure_is_never_offered() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let e = bin(
            BinOp::Add,
            call(
                lam(&["x"], bin(BinOp::Mul, var("x"), int(2))),
                vec![int(21)],
            ),
            callv("double", vec![int(0)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(42));
        assert_eq!(
            backend.names(),
            vec!["double"],
            "an anonymous closure was offered to a backend"
        );
    }

    /// A `Code` closure built by hand, so [`admit`] can be asked about a body
    /// the machine would not otherwise hand it: an anonymous one, or one under a
    /// name no definition publishes.
    fn code_closure(name: Option<&str>, params: &[&str], body: Expr) -> Closure {
        Closure {
            name: name.map(Symbol::new),
            kind: ClosureKind::Code {
                params: Rc::new(params.iter().copied().map(Symbol::new).collect()),
                body: crate::code::lower(&body),
                env: Env::empty(),
                module: 0,
            },
        }
    }

    fn double_closure() -> Closure {
        code_closure(Some("double"), &["x"], bin(BinOp::Mul, var("x"), int(2)))
    }

    /// The gate chain over `c`'s program, outside a region and at full budget.
    /// The name is rendered because what a test wants to say is "offered as
    /// `double`", and `Symbol` is not what it wants to write.
    fn gate(c: &Checked, closure: &Closure, args: &[Value]) -> Result<(String, usize), Gate> {
        admit(closure, args, false, Some(&c.check), DEFAULT_MAX_CALLS, 0)
            .map(|(name, budget)| (name.as_str().to_string(), budget))
    }

    /// What every gate test below reads its refusal against: this call, on this
    /// program, clears all of them.
    fn admitted() -> Result<(String, usize), Gate> {
        Ok(("double".to_string(), DEFAULT_MAX_CALLS))
    }

    /// A tree-walker closure carries a program-wide name over a body that is a
    /// deep clone rather than a node of the program, and `Interp` is the oracle
    /// `--engine both` audits the machine against. The behaviour is
    /// [`a_tree_walker_closure_with_a_program_wide_name_is_never_offered`]; this
    /// is the gate that produces it.
    #[test]
    fn a_body_this_machine_did_not_lower_is_refused_by_the_kind_gate() {
        let c = checked(vec![double_def()]);
        let treewalk = Closure {
            name: Some(Symbol::new("double")),
            kind: ClosureKind::Fn {
                params: vec![Symbol::new("x")],
                body: Arc::new(bin(BinOp::Mul, var("x"), int(2))),
                env: Env::empty(),
                module: 0,
            },
        };
        assert_eq!(
            gate(&c, &treewalk, &[Value::Int(21)]),
            Err(Gate::NotLoweredCode)
        );
        assert_eq!(
            gate(&c, &double_closure(), &[Value::Int(21)]),
            admitted(),
            "the same name over a lowered body is refused too, so the test above says nothing"
        );
    }

    /// The kinds this boundary carries, asked of the gate rather than of a run.
    #[test]
    fn an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        for refused in [
            Value::Float(1.0),
            Value::str("21"),
            Value::Unit,
            Value::List(Default::default()),
            Value::Secret(Arc::new(Value::Int(21))),
        ] {
            assert_eq!(
                gate(&c, &subject, std::slice::from_ref(&refused)),
                Err(Gate::ArgumentShape),
                "{refused:?} was carried across the boundary"
            );
        }
        assert_eq!(gate(&c, &subject, &[Value::Int(21)]), admitted());
        assert_eq!(gate(&c, &subject, &[Value::Bool(true)]), admitted());
    }

    /// Inside a `simulate` region every cell touch and every allocation is an
    /// `Access` the search prunes on, and a body the machine did not run records
    /// none of them.
    #[test]
    fn a_call_inside_a_simulate_region_is_refused_by_the_region_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        let args = [Value::Int(21)];
        assert_eq!(
            admit(&subject, &args, true, Some(&c.check), DEFAULT_MAX_CALLS, 0),
            Err(Gate::SimulateRegion)
        );
        assert_eq!(gate(&c, &subject, &args), admitted());
    }

    /// The gate this block exists to arm, and the one hole
    /// `CONTRIBUTING.md` §"Things known to be broken" item 13 named.
    ///
    /// [`an_anonymous_closure_is_never_offered`] asserts the behaviour — nothing
    /// anonymous reaches a backend — and is satisfied by whichever gate happens
    /// to refuse first. Replacing `closure.name.as_ref()?` with a fabricated
    /// empty `Symbol` leaves it green, because the row gate refuses the
    /// fabrication downstream. The third assertion here is that fabrication,
    /// spelled out: a name no definition publishes really is refused by
    /// [`Gate::PublishedRow`], which is *why* it could stand in for the name
    /// gate without anything noticing. Under that substitution the first
    /// assertion reads `Err(Gate::PublishedRow)` and this test is what goes red.
    #[test]
    fn an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate() {
        let c = checked(vec![double_def()]);
        let anonymous = code_closure(None, &["x"], bin(BinOp::Mul, var("x"), int(2)));
        assert_eq!(
            gate(&c, &anonymous, &[Value::Int(21)]),
            Err(Gate::Anonymous)
        );
        assert_eq!(
            gate(&c, &double_closure(), &[Value::Int(21)]),
            admitted(),
            "the same body under a published name is refused too, so the refusal above is not \
             the name"
        );
        let fabricated = code_closure(Some(""), &["x"], bin(BinOp::Mul, var("x"), int(2)));
        assert_eq!(
            gate(&c, &fabricated, &[Value::Int(21)]),
            Err(Gate::PublishedRow),
            "a name the program does not publish cleared the row gate, and the substitution \
             above would now be visible to the behavioural test after all"
        );
    }

    /// The published row is the reviewable artifact, and "no row at all" is the
    /// same refusal as "a row that is not empty" — a machine built without a
    /// `CheckOutput` enters nothing, which is most of this crate's own tests.
    #[test]
    fn a_row_that_is_not_empty_and_a_row_that_is_missing_are_both_refused_by_the_row_gate() {
        let c = checked(vec![
            double_def(),
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def(
                "touch",
                &["x"],
                perform("state", "get", None, vec![var("x")]),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("touch")].footprint.is_empty(),
            "the fixture is wrong: `touch` publishes an empty row"
        );
        let effectful = code_closure(Some("touch"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &effectful, &[Value::Int(1)]),
            Err(Gate::PublishedRow)
        );
        let unknown = code_closure(Some("never.declared"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &unknown, &[Value::Int(1)]),
            Err(Gate::PublishedRow)
        );
        assert_eq!(
            admit(
                &double_closure(),
                &[Value::Int(21)],
                false,
                None,
                DEFAULT_MAX_CALLS,
                0
            ),
            Err(Gate::PublishedRow),
            "a machine with no `CheckOutput` cleared a definition it has no row for"
        );
        assert_eq!(gate(&c, &double_closure(), &[Value::Int(21)]), admitted());
    }

    /// `budget` is the machine's remaining nested calls, so the last one belongs
    /// to the machine: the interpreted path raises the bound both engines raise,
    /// at the machine's own span.
    ///
    /// What this test bites is the `budget == 0` refusal — deleting it turns
    /// this and `the_budget_is_the_machines_remaining_depth_and_never_reaches_zero`
    /// red. It does *not* bite the checked subtraction underneath it, and this
    /// doc no longer says it does: see the table in this module's header for why
    /// no test can.
    #[test]
    fn the_last_nested_call_is_refused_by_the_budget_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        let args = [Value::Int(21)];
        let at = |max: usize, calls: usize| {
            admit(&subject, &args, false, Some(&c.check), max, calls).map(|(_, budget)| budget)
        };
        assert_eq!(at(8, 8), Err(Gate::Budget));
        // Not evidence for `checked_sub` over `saturating_sub`: both answer
        // `Err` here, one via `None` and one via the `budget == 0` refusal. What
        // it pins is that an over-subscribed stack is refused rather than
        // wrapping to an enormous budget.
        assert_eq!(at(8, 9), Err(Gate::Budget));
        assert_eq!(at(8, 7), Ok(1));
        assert_eq!(at(8, 0), Ok(8));
    }

    /// The ordering is a cost claim — a call taking a record, a list or a string
    /// is refused on one discriminant test per argument and never hashes a
    /// `Symbol` into `CheckOutput::defs` — and a cost claim nothing asserts is a
    /// comment. Two orderings are load-bearing and both are observable now that
    /// a refusal carries its reason.
    #[test]
    fn the_shape_gate_is_reached_before_the_row_is_looked_up() {
        let c = checked(vec![double_def()]);
        let unknown = code_closure(Some("never.declared"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &unknown, &[Value::str("21")]),
            Err(Gate::ArgumentShape),
            "the row was looked up for a call the argument shape had already refused"
        );
        let anonymous = code_closure(None, &["x"], var("x"));
        assert_eq!(
            gate(&c, &anonymous, &[Value::str("21")]),
            Err(Gate::ArgumentShape)
        );
    }

    /// The published row is the reviewable artifact, and it is what both the
    /// constant memo and this boundary read. A definition that can `perform` is
    /// refused whatever the backend claims — and a backend has no route to
    /// `perform` in any case, which is why the refusal is a correctness gate and
    /// not a courtesy.
    #[test]
    fn a_definition_whose_published_row_is_not_empty_is_never_offered() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def(
                "touch",
                &["x"],
                perform("state", "get", None, vec![var("x")]),
            ),
            fn_def("bump", &["x"], bin(BinOp::Add, var("x"), int(0))),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("touch")].footprint.is_empty(),
            "the fixture is wrong: `touch` publishes an empty row"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // `bump` is the control: same shape, same arguments, empty row, and it
        // sits inside the same `handle` so the hook is demonstrably live there.
        let e = handle(
            bin(
                BinOp::Add,
                callv("touch", vec![int(1)]),
                callv("bump", vec![int(0)]),
            ),
            vec![clause(
                "state",
                "get",
                None,
                &["n"],
                bin(BinOp::Add, var("n"), int(1)),
            )],
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(2));
        assert_eq!(
            backend.names(),
            vec!["bump"],
            "a definition that can `perform` was offered to a backend"
        );
    }

    /// The whole partial-order story, and the reason it is one gate: inside a
    /// region every cell touch and every allocation is an `Access` the search
    /// prunes on, and a body the machine did not run records none of them.
    /// Outside one there is no trail to disturb.
    #[test]
    fn nothing_is_offered_inside_a_simulate_region() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // The control is the second half of the same expression, on the same
        // machine and the same definition: `double(1)` outside the region is
        // offered, so the silence inside it is a gate firing rather than a
        // fixture that never reached the hook.
        let e = bin(
            BinOp::Add,
            ex(ExprKind::Simulate {
                body: Box::new(callv("double", vec![int(21)])),
            }),
            callv("double", vec![int(1)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(44));
        assert_eq!(
            backend.offers(),
            vec![Offer {
                name: Symbol::new("double"),
                args: vec![Value::Int(1)],
                budget: DEFAULT_MAX_CALLS,
            }],
            "a call inside a `simulate` region reached the backend"
        );
    }

    /// The read side of the constant memo stays ahead of the hook, and the write
    /// side still goes through `Frame::Call { memo }`.
    #[test]
    fn a_nullary_constant_is_entered_once_and_memoized_afterwards() {
        let c = checked(vec![fn_def("answer", &[], int(1))]);
        let backend = Double::answering(&c.program, "answer", Value::Int(7));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let e = bin(
            BinOp::Add,
            callv("answer", vec![]),
            bin(BinOp::Add, callv("answer", vec![]), callv("answer", vec![])),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(21));
        assert_eq!(
            backend.offers().len(),
            1,
            "the memo did not take over after the first compiled entry: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// `limit.rs` exists so a runaway recursion is a diagnostic in both engines.
    /// A backend is handed the machine's own remaining depth so it can decline
    /// rather than recurse natively past it; when it declines, the machine raises
    /// exactly what it raises with no backend at all.
    #[test]
    fn the_budget_is_the_machines_remaining_depth_and_never_reaches_zero() {
        let c = checked(vec![fn_def(
            "down",
            &["n"],
            if_(
                bin(BinOp::Eq, var("n"), int(0)),
                int(0),
                callv("down", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        )]);
        let subject = callv("down", vec![int(1_000)]);

        let baseline = rendered(&c.machine().with_max_calls(8).eval_expr_for_test(&subject));
        assert!(
            baseline.contains("recursion limit of 8 nested calls exceeded"),
            "the fixture never reached the bound: {baseline}"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine().with_max_calls(8);
        machine.set_compiled(backend.clone());
        assert_eq!(rendered(&machine.eval_expr_for_test(&subject)), baseline);

        let budgets: Vec<usize> = backend.offers().iter().map(|o| o.budget).collect();
        assert_eq!(
            budgets,
            vec![8, 7, 6, 5, 4, 3, 2, 1],
            "a backend was handed a depth the machine did not have left"
        );
    }

    /// `argv.rs` is 40.9% of ADR 0019 §1. The entered path takes the same buffer
    /// the interpreted path takes and owes the same hand-back.
    #[test]
    fn an_entered_call_returns_its_argument_vector_to_the_free_list() {
        let c = checked(vec![double_def()]);
        let subject = callv("double", vec![int(21)]);

        argv::drain_the_free_list();
        let mut interpreted = c.machine();
        assert_eq!(ok(interpreted.eval_expr_for_test(&subject)), Value::Int(42));
        let after_interpreted = argv::kept();

        argv::drain_the_free_list();
        let backend = Double::answering(&c.program, "double", Value::Int(84));
        let mut machine = c.machine();
        machine.set_compiled(backend);
        assert_eq!(ok(machine.eval_expr_for_test(&subject)), Value::Int(84));
        let after_entered = argv::kept();

        assert!(
            after_interpreted[0] > 0,
            "the fixture never used a pooled buffer at all"
        );
        assert_eq!(
            after_entered, after_interpreted,
            "the entered path left the free list in a different state than the interpreted one"
        );
    }

    /// The gates are ordered so that the argument shape is tested before the name
    /// is looked up. What is observable is the refusal; the ordering is a cost
    /// claim and is not asserted here.
    #[test]
    fn a_call_taking_a_non_scalar_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def("head", &["xs"], callv("len", vec![var("xs")])),
            fn_def("width", &["s"], callv("len", vec![var("s")])),
        ]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("head", vec![list(vec![int(1), int(2)])]))),
            Value::Int(2)
        );
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("width", vec![string("abcd")]))),
            Value::Int(4)
        );
        assert!(
            backend.offers().is_empty(),
            "a non-scalar argument reached a backend: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (0, 0));

        // Control: the hook is live on this machine.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A `Secret` may not cross in either direction: `value.rs` redacts it on
    /// render and `escape.rs` walks its payload deliberately, and a backend
    /// builds messages the machine never sees.
    #[test]
    fn a_secret_is_never_offered_and_never_accepted() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // Reaching `enter_code` with a `Secret` argument goes through `call`,
        // which is the only route that can carry a value the program did not
        // build. Checked rather than assumed: `escape::check` lets it through and
        // the machine's own arithmetic is what refuses it, so the value did reach
        // the hook and the argument gate is what kept it from the backend.
        let outcome = machine.call(
            "double",
            vec![Value::Secret(Arc::new(Value::str("hunter2")))],
            sp(),
        );
        let rendered = rendered(&outcome);
        assert!(
            rendered.contains("E0502") && rendered.contains("arithmetic expects Int"),
            "the fixture stopped reaching the hook: {rendered}"
        );
        assert!(!rendered.contains("hunter2"), "a credential was printed");
        assert!(
            backend.offers().is_empty(),
            "a `Secret` was handed to a backend"
        );
        backend.forget();
        // Control: the same definition with an `Int` is offered.
        assert_eq!(
            ok(machine.call("double", vec![Value::Int(21)], sp())),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
        drop(machine);

        let answering =
            Double::answering(&c.program, "double", Value::Secret(Arc::new(Value::Int(1))));
        let mut machine = c.machine();
        machine.set_compiled(answering);
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_refusals(), 1);
    }

    /// The `--engine both` comparison, taken between a machine with a backend
    /// and one without: the rendered value, the outcome field by field, the
    /// footprint, and the cell arena slot by slot.
    #[track_caller]
    fn agree_on(c: &Checked, backend: Rc<Double>, e: &Expr) {
        let mut plain = c.machine();
        let mut entered = c.machine();
        entered.set_compiled(backend);
        let left = plain.eval_expr_for_test(e);
        let right = entered.eval_expr_for_test(e);
        if let Some(d) =
            compare_answers(&plain, &entered, "the expression under test", &left, &right)
        {
            panic!("a backend changed what the machine did — {d}");
        }
    }

    /// A continuation cannot be captured beneath a native activation, because
    /// nothing runs in the machine while a body runs and the body has returned
    /// before its `Frame::Call` is even pushed. The fixture resumes twice, so a
    /// compiled entry that had left anything parked would be entered twice
    /// against one activation.
    #[test]
    fn a_multi_shot_resume_over_an_entered_call_answers_what_the_machine_answers() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def("triple", &["x"], bin(BinOp::Mul, var("x"), int(3))),
        ]);
        let e = handle(
            bin(
                BinOp::Add,
                perform("state", "get", None, vec![]),
                callv("triple", vec![int(2)]),
            ),
            vec![general_clause(
                "state",
                "get",
                None,
                &[],
                "k",
                bin(
                    BinOp::Add,
                    callv("k", vec![int(1)]),
                    callv("k", vec![int(10)]),
                ),
            )],
        );

        // The backend answers exactly what the body computes, so an identical
        // result is evidence about the control flow rather than about the value.
        agree_on(
            &c,
            Double::answering(&c.program, "triple", Value::Int(6)),
            &e,
        );

        let backend = Double::answering(&c.program, "triple", Value::Int(6));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // `k(1)` gives `1 + 6`, `k(10)` gives `10 + 6`.
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(23));
        assert_eq!(
            backend.names(),
            vec!["triple", "triple"],
            "the fixture did not enter compiled code once per resumption"
        );
        assert_eq!(machine.compiled_counts(), (2, 0));
    }

    /// The other half of the same invariant: a clause that never resumes leaves
    /// the delimiter with its own value, and an entered call that already
    /// finished is not parked waiting for anything.
    #[test]
    fn a_discarded_continuation_over_an_entered_call_halts_with_the_handlers_value() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def("triple", &["x"], bin(BinOp::Mul, var("x"), int(3))),
        ]);
        let e = handle(
            bin(
                BinOp::Add,
                callv("triple", vec![int(2)]),
                perform("state", "get", None, vec![]),
            ),
            vec![general_clause("state", "get", None, &[], "k", int(99))],
        );

        agree_on(
            &c,
            Double::answering(&c.program, "triple", Value::Int(6)),
            &e,
        );

        let backend = Double::answering(&c.program, "triple", Value::Int(6));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(99));
        assert_eq!(
            backend.names(),
            vec!["triple"],
            "the fixture never entered compiled code before the clause discarded"
        );
    }

    /// `differential::audit_state` compares the final arena as the ordered
    /// `(Slot, rendered value)` sequence, and an entered call must leave it
    /// alone. Here the entered definition touches no cell and its caller does.
    #[test]
    fn a_cell_touching_caller_agrees_slot_for_slot_with_an_entered_callee() {
        let c = checked(vec![fn_def(
            "bump",
            &["x"],
            bin(BinOp::Add, var("x"), int(1)),
        )]);
        let e = with_cell(
            "s",
            int(1),
            "c",
            block(
                vec![discard(callv(
                    "cell_set",
                    vec![var("c"), callv("bump", vec![int(8)])],
                ))],
                Some(callv("cell_get", vec![var("c")])),
            ),
        );
        agree_on(&c, Double::answering(&c.program, "bump", Value::Int(9)), &e);

        let backend = Double::answering(&c.program, "bump", Value::Int(9));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(9));
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// The one difference this boundary knowingly makes, asserted rather than
    /// assumed. `memo.rs`'s note names the case: a definition that opens its own
    /// `with_cell` publishes an empty row, so it passes the purity gate, and a
    /// compiled entry skips the allocation the interpreter makes.
    ///
    /// Unobservable to the program — the arena the two runs end with is equal
    /// slot for slot, which is what `compare_answers` checks — and observable to
    /// `w6-alloc`, which is why a `w6-alloc` figure taken with a backend attached
    /// may not be quoted from a run without one.
    #[test]
    fn an_entered_definition_that_opens_its_own_region_skips_an_allocation() {
        let c = checked(vec![fn_def(
            "boxed",
            &["n"],
            with_cell("s", var("n"), "c", callv("cell_get", vec![var("c")])),
        )]);
        assert!(
            c.check.defs[&Symbol::new("boxed")].footprint.is_empty(),
            "the fixture is wrong: `boxed` does not publish an empty row"
        );
        let e = callv("boxed", vec![int(5)]);

        // Program-visible state is identical, arena included.
        agree_on(
            &c,
            Double::answering(&c.program, "boxed", Value::Int(5)),
            &e,
        );

        let mut plain = c.machine();
        assert_eq!(ok(plain.eval_expr_for_test(&e)), Value::Int(5));
        let interpreted_allocations = plain.cells().stats().allocations;

        let mut entered = c.machine();
        entered.set_compiled(Double::answering(&c.program, "boxed", Value::Int(5)));
        assert_eq!(ok(entered.eval_expr_for_test(&e)), Value::Int(5));
        assert_eq!(entered.compiled_counts(), (1, 0));

        assert!(interpreted_allocations > 0);
        assert_eq!(
            entered.cells().stats().allocations,
            interpreted_allocations - 1,
            "the accounting this test exists to pin down moved"
        );
    }

    /// A backend cannot raise — `enter` answers a `Value` or nothing — so every
    /// diagnostic a run produces is the machine's own, at the machine's own span,
    /// with the machine's own labels and notes. `rt::error`'s `Span::DUMMY` and
    /// its "in compiled code" labels are unreachable through this seam rather
    /// than mitigated on it.
    #[test]
    fn a_failure_after_an_accepted_call_is_the_machines_own_diagnostic() {
        let c = checked(vec![
            fn_def("safe", &["x"], bin(BinOp::Add, var("x"), int(1))),
            fn_def("risky", &["x"], bin(BinOp::Div, int(10), var("x"))),
        ]);
        let subjects = [
            bin(BinOp::Div, callv("safe", vec![int(1)]), int(0)),
            callv("risky", vec![callv("safe", vec![int(-1)])]),
            bin(
                BinOp::Add,
                callv("safe", vec![int(i64::MAX - 1)]),
                int(i64::MAX),
            ),
        ];

        let baseline: Vec<String> = {
            let mut plain = c.machine();
            subjects
                .iter()
                .map(|e| rendered(&plain.eval_expr_for_test(e)))
                .collect()
        };
        assert!(
            baseline.iter().all(|r| r.starts_with("Err(")),
            "the fixture stopped failing: {baseline:?}"
        );

        // Faithful rather than constant: what is under test is where the
        // diagnostic comes from, and a backend answering the wrong number would
        // be testing that instead.
        let backend = Double::over(&c.program, |name, args, _| match (name.as_str(), args) {
            ("safe", [Value::Int(x)]) => x.checked_add(1).map(Value::Int),
            _ => None,
        });
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        for (e, want) in subjects.iter().zip(&baseline) {
            assert_eq!(&rendered(&machine.eval_expr_for_test(e)), want);
        }
        let (entries, _) = machine.compiled_counts();
        assert_eq!(
            entries,
            subjects.len() as u64,
            "the backend was never entered, so this proves nothing about failures under it"
        );
    }

    /// The purity gate reads the published row, so a machine driven without a
    /// type-check pass has nothing to clear a definition with and the hook is
    /// inert. `Machine::for_program` is what the corpus harness, the prover's
    /// generators and most of this crate's own tests build, so this is the
    /// common case rather than a corner: found by the entry counter in
    /// `tests/differential_corpus.rs`, which was green over a seam it had never
    /// once reached.
    #[test]
    fn a_machine_with_no_check_output_offers_nothing() {
        let (program, resolved) = standalone(vec![double_def()]);
        let backend = Double::declining(&program);
        let mut machine = Machine::for_program(&program, &resolved);
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert!(backend.offers().is_empty());
        assert_eq!(machine.compiled_counts(), (0, 0));
    }

    /// A `simulate` in a *definition's* body is a different case from a call
    /// made inside a live region, and it is refused by a different gate: the row
    /// gains `sim.read`, so the purity gate takes it. Armed rather than asserted
    /// — the row is read out of the fixture before the run.
    #[test]
    fn a_definition_that_opens_its_own_simulate_region_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def(
                "searched",
                &["n"],
                ex(ExprKind::Simulate {
                    body: Box::new(bin(BinOp::Add, var("n"), int(1))),
                }),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("searched")].footprint.is_empty(),
            "the fixture is wrong: a `simulate` body published an empty row"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&bin(
                BinOp::Add,
                callv("searched", vec![int(1)]),
                callv("double", vec![int(0)]),
            ))),
            Value::Int(2)
        );
        assert_eq!(
            backend.names(),
            vec!["double"],
            "a definition that opens a `simulate` region was offered to a backend"
        );
    }

    #[test]
    fn crossable_admits_the_two_scalar_kinds_and_nothing_else() {
        assert!(crossable(&Value::Int(0)));
        assert!(crossable(&Value::Bool(false)));
        for refused in [
            Value::Float(0.0),
            Value::str("s"),
            Value::Unit,
            Value::List(Default::default()),
            Value::Secret(Arc::new(Value::Int(1))),
        ] {
            assert!(!crossable(&refused), "{refused:?} crossed the boundary");
        }
    }

    /// An arity mismatch is the machine's diagnostic, phrased from
    /// `closure.describe()`, and it stays ahead of the hook.
    #[test]
    fn an_arity_mismatch_is_the_machines_diagnostic_and_no_backend_sees_it() {
        let c = checked(vec![double_def()]);
        let subject = callv("double", vec![int(1), int(2)]);
        let baseline = rendered(&c.machine().eval_expr_for_test(&subject));

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(rendered(&machine.eval_expr_for_test(&subject)), baseline);
        assert!(
            backend.offers().is_empty(),
            "a call whose arity does not match was offered to a backend"
        );
        // Control: at the right arity the same call is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A `Float` in flight is what `crossable` refuses; this is the same
    /// statement about the answer rather than the argument, and it is separate
    /// because the two are separate gates.
    fn float(f: f64) -> Expr {
        ex(ExprKind::Lit(ply_syntax::ast::Lit::Float(f)))
    }
}
