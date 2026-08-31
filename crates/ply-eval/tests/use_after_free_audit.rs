//! Reading a slot after its region reclaimed it.
//!
//! Before R2 a region's memory was never actually handed back, so an escape the
//! checks missed was harmless: the cell was still there. R2 makes the close a
//! real free, and the same escape becomes this language's first use-after-free.
//! Every program in this file is written to *produce* one, and the assertion is
//! what happened instead.
//!
//! Three answers are acceptable and one is not. A route may be **refused
//! statically** — the brand, over the resolved type. It may be **retained** — a
//! continuation or a task that can still reach the slots holds a
//! [`ply_eval::arena::Pin`] and the close defers. It may be **diagnosed** — the
//! generation in a [`Slot`] makes a reclaimed position fail to resolve, so a
//! stale access is a diagnostic rather than an answer. A **stale read that
//! returns a value** is the one outcome that is a defect, and it is what the
//! control programs here exist to make possible: an attack is paired with the
//! same shape minus the thing that saves it, so "the slot was not reused" is a
//! measurement rather than a coincidence.
//!
//! Where the attacks are aimed:
//!
//! 1. every carrier a value can leave a region through, including the two the
//!    brief names that no other file walks — a `Map` key and a `Map` value —
//!    and the type-alias round trip that is W2's hole restated;
//! 2. the one escape ADR 0017 §2 *permits* — a cell reaching a task — pushed
//!    past the point the ADR argues from, to a task that outlives the region's
//!    lexical close, sleeps across it, and spawns a child after it;
//! 3. the one capture that deliberately takes no pin, a tail-resumptive clause,
//!    with a region open across it;
//! 4. §2's documented open route, with a second region positioned to take the
//!    freed slot;
//! 5. the inference, on shapes `region_reclamation_audit` does not walk — and
//!    the reason a wrong answer from it is survivable, stated as a property of
//!    [`Arena::close`] rather than as a hope;
//! 6. the generation counter, whose wrap is the one way a stale read comes back
//!    as a value.

use ply_core::{CheckOutput, check_program};
use ply_eval::arena::{Arena, Reclaim, RegionKind, Slot, Stats};
use ply_eval::{Interp, Machine, Value};
use ply_span::{Diagnostic, SourceId, Span, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

// ------------------------------------------------------------------ harness

/// The tree-walker's refusal of a construct only the machine has. Not a
/// disagreement about the program, so it is not compared against.
const MACHINE_ONLY: &str = "E0504";

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        Compiled::modules(&[("m", src)])
    }

    fn modules(sources: &[(&str, &str)]) -> Compiled {
        let inputs: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
            .collect();
        let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    #[track_caller]
    fn refused(src: &str) -> Vec<Diagnostic> {
        let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), src)];
        let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        match check_program(&program, &resolved) {
            Ok(_) => panic!("this reaches a region's slots and was accepted:\n{src}"),
            Err(d) => d,
        }
    }

    /// The machine's answer and the arena it left behind.
    ///
    /// The tree-walker runs the same program wherever it can — it refuses every
    /// clause that binds a continuation and every `simulate`, which is most of
    /// this file — and its answer is compared whenever it has one.
    #[track_caller]
    fn run(&self, name: &str) -> (Result<Value, Diagnostic>, Stats) {
        let mut machine = self.machine();
        let answer = machine.call(name, Vec::new(), Span::DUMMY);
        let treewalk = self.interp().call(name, Vec::new(), Span::DUMMY);
        match (&answer, &treewalk) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "the engines disagree about `{name}`"),
            (_, Err(b)) if b.code == MACHINE_ONLY || b.code == codes::INTERNAL_ERROR => {}
            (Err(a), Err(b)) => assert_eq!(
                a.code, b.code,
                "the engines disagree about why `{name}` failed"
            ),
            (Ok(v), Err(b)) => {
                panic!("only the tree-walker failed `{name}`: {v:?} then {b:#?}")
            }
            (Err(a), Ok(v)) => panic!("only the machine failed `{name}`: {a:#?} then {v:?}"),
        }
        (answer, machine.cells().stats())
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn interp(&self) -> Interp<'_> {
        Interp::new(&self.program, &self.resolved, &self.check)
    }

    fn kinds(&self) -> ply_eval::region_kind::Regions {
        ply_eval::region_kind::infer(&self.program, &self.resolved)
    }
}

#[track_caller]
fn answers(compiled: &Compiled, name: &str, want: i64) -> Stats {
    let (answer, stats) = compiled.run(name);
    match answer {
        Ok(Value::Int(got)) => assert_eq!(
            got, want,
            "`{name}` read a slot its region had handed back: {got} where {want} is the value the \
             region actually held"
        ),
        Ok(other) => panic!("`{name}` answered {other:?}"),
        Err(d) => panic!("`{name}` failed: {d:#?}"),
    }
    stats
}

fn codes_of(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code).collect()
}

// ------------------------- 1. the carriers a value can leave a region through

/// ADR 0017 §2's list, walked to the end. `region_isolation_audit` covers the
/// list element and the record field; a `Map` key and a `Map` value are the two
/// the brief names that nothing walked, and the key is the sharper of them — a
/// key is compared, so a dangling one would decide an ordering rather than only
/// be read back.
#[test]
fn every_carrier_out_of_a_region_is_refused_before_it_can_dangle() {
    let carriers: &[(&str, &str)] = &[
        ("a list element", "fn leak() = with_cell[k](0) { c -> [c] }"),
        (
            "a Map key",
            "fn leak() = with_cell[k](0) { c -> map_insert(map_new(), c, 1) }",
        ),
        (
            "a Map value",
            "fn leak() = with_cell[k](0) { c -> map_insert(map_new(), 1, c) }",
        ),
        (
            "a record field",
            "fn leak() = with_cell[k](0) { c -> {held: c} }",
        ),
        (
            "a generic constructor's field",
            "type Box<a> = | B(a)\nfn leak() = with_cell[k](0) { c -> B(c) }",
        ),
        (
            "a record inside a list",
            "fn leak() = with_cell[k](0) { c -> [{held: [c]}] }",
        ),
        (
            "a closure that reads it",
            "fn leak() = with_cell[k](0) { c -> || cell_get(c) }",
        ),
    ];
    for (what, src) in carriers {
        let diags = Compiled::refused(src);
        assert!(
            diags
                .iter()
                .any(|d| d.code == codes::TYPE_MISMATCH || d.code == codes::REGION_ESCAPE),
            "{what} carried a cell out of its region: {:?}",
            codes_of(&diags)
        );
    }
}

/// W2's hole was a check that ran *before* alias resolution, and ADR 0017's
/// Consequences name it as the way an escape gets past the brand. Three halves
/// together say that it does not, and the first two are the ones that would both
/// pass under a check that expanded no aliases:
///
/// - a cell passes through a definition whose parameter is written as an alias
///   for `Cell<Int>` and comes back, and the read *inside* the region is right;
/// - the same value handed out of the region is refused, so the alias did not
///   erase the brand on the way through;
/// - and the definition it passed through cannot dereference it, which is what
///   keeps an escaped cell unreadable rather than merely unreachable.
#[test]
fn a_cell_round_tripped_through_a_type_alias_keeps_its_brand() {
    const ALIAS: &str = "type Held = Cell<Int>\nfn keep(c: Held) -> Held = c\n";

    let inside = Compiled::new(&format!(
        "{ALIAS}pub fn read_it() -> Int = with_cell[k](42) {{ c -> cell_get(keep(c)) }}"
    ));
    answers(&inside, "m.read_it", 42);

    let out = Compiled::refused(&format!(
        "{ALIAS}pub fn leak() -> Held = with_cell[k](42) {{ c -> keep(c) }}"
    ));
    assert!(
        out.iter().any(|d| d.code == codes::TYPE_MISMATCH
            && d.message.contains("escapes its `with_cell[k]` region")),
        "the alias erased the brand on the way out: {out:#?}"
    );

    let deref = Compiled::refused(&format!("{ALIAS}fn peek(c: Held) -> Int = cell_get(c)"));
    assert!(
        deref
            .iter()
            .any(|d| d.message.contains("cannot tell which `with_cell` region")),
        "a definition holding a written `Cell<Int>` could dereference it, which would make an \
         escaped cell readable rather than only unreadable: {deref:#?}"
    );
}

/// A `law` body opens regions like any other body, so `check_regions` has to
/// have filed a site for it. `region_escape_audit` pins the `test` form; a law is
/// the other labelled item, and the one nothing enumerates as a definition.
#[test]
fn a_region_in_a_law_body_reports_its_escape() {
    let diags = Compiled::refused(
        r#"law "leak" forall (n: Int) { with_region[r] { with_cell[r](n) { c -> c } } == 0 }"#,
    );
    assert!(
        diags.iter().any(|d| d.code == codes::REGION_ESCAPE),
        "a law's region escaped unchecked: {:?}",
        codes_of(&diags)
    );
}

// --------------------------------------- 2. the escape ADR 0017 §2 permits

/// The route the ADR deliberately leaves open, taken past the argument that
/// opens it.
///
/// §2 excludes `task.spawn` from a bare `with_cell`'s rule and §3 justifies the
/// exclusion with "a `shared` region's slots outlive its close". Under R2 they
/// outlive it only while something holds a pin, so the shape that tests the claim
/// is not the landed one — where the task is joined inside the region — but this
/// one: the task handle leaves the region, the region's `}` is reached with the
/// task never having run, and a *second* region then opens over the position the
/// first would otherwise have handed back.
///
/// The control is the same program with the task removed, and it is what makes
/// this an attack rather than an illustration: there the two cells share one
/// slot, so 999 is what a missing pin would have answered.
#[test]
fn a_task_outliving_its_regions_close_reads_that_region_and_not_the_one_after_it() {
    let attack = Compiled::new(
        r#"
pub fn attack() -> Int = simulate {
  { let t = with_cell[s](11) { c -> task.spawn(|| cell_get(c)) };
    let after = with_cell[q](999) { d -> cell_get(d) };
    task.join(t) + after * 0 }
}
"#,
    );
    let stats = answers(&attack, "m.attack", 11);
    assert_eq!(
        stats.closes_deferred, 1,
        "`s` closed while the task could still reach it, so its close had to defer"
    );
    assert_eq!(
        stats.peak_live, 2,
        "`q` must not be given `s`'s position while the task still holds it"
    );

    let control = Compiled::new(
        r#"
pub fn control() -> Int = simulate {
  { let first = with_cell[s](11) { c -> cell_get(c) };
    let after = with_cell[q](999) { d -> cell_get(d) };
    first + after }
}
"#,
    );
    let stats = answers(&control, "m.control", 1010);
    assert_eq!(
        stats.peak_live, 1,
        "with nothing reaching `s` past its close the two cells share one position — which is the \
         position the attack above would have read"
    );
}

/// The same route with the task asleep across the close, so that it is live and
/// unfinished when the second region opens rather than merely unscheduled.
#[test]
fn a_task_asleep_across_its_regions_close_still_reads_it() {
    let compiled = Compiled::new(
        r#"
pub fn attack() -> Int = simulate {
  { let t = with_cell[s](11) { c -> task.spawn(|| { clock.sleep(100); cell_get(c) }) };
    let after = with_cell[q](999) { d -> cell_get(d) };
    task.join(t) + after * 0 }
}
"#,
    );
    let stats = answers(&compiled, "m.attack", 11);
    assert_eq!(stats.closes_deferred, 1);
    assert_eq!(
        stats.slots_reclaimed_late, 1,
        "and the slot did go back once the task was done with it"
    );
}

/// A task spawned **after** the region closed, which is where the pin discipline
/// runs out: `TaskRegions::pin` answers `None` when no program region is open, so
/// the grandchild's own `spawn` claims nothing at all. What holds `s` is the pin
/// its *parent* took, and the parent has already finished — so this reads the
/// right value only because a scheduler never reaps a finished task's record.
/// That is load-bearing rather than incidental, and a change that dropped a
/// finished task's state would turn this program into a stale read.
#[test]
fn a_grandchild_task_spawned_after_the_close_still_reads_the_region() {
    let compiled = Compiled::new(
        r#"
pub fn attack() -> Int = simulate {
  { let outer = with_cell[s](11) { c ->
      task.spawn(|| { clock.sleep(50); task.spawn(|| { clock.sleep(400); cell_get(c) }) }) };
    let inner = task.join(outer);
    let after = with_cell[q](999) { d -> cell_get(d) };
    let later = with_cell[p](777) { e -> cell_get(e) };
    task.join(inner) + after * 0 + later * 0 }
}
"#,
    );
    let stats = answers(&compiled, "m.attack", 11);
    assert_eq!(
        stats.closes_deferred, 1,
        "`s` must still have been held when it closed"
    );
}

/// The asymmetry that pays for the three above, and the reason it is a decision
/// rather than an oversight: `with_region` is new syntax with no program
/// depending on the loose rule, so the identical escape is a compile error that
/// names the task.
#[test]
fn the_same_escape_out_of_a_with_region_is_refused_statically() {
    let diags = Compiled::refused(
        r#"
pub fn attack() -> Int = simulate {
  { let t = with_region[s] { with_cell[s](11) { c -> task.spawn(|| cell_get(c)) } };
    task.join(t) }
}
"#,
    );
    let escape = diags
        .iter()
        .find(|d| d.code == codes::REGION_ESCAPE)
        .unwrap_or_else(|| panic!("a task reached a `with_region`'s cell: {diags:#?}"));
    assert!(
        escape.message.contains("sent to another task"),
        "{}",
        escape.message
    );
}

// ------------------------------ 3. the capture that deliberately takes no pin

/// `handler::perform` takes no pin for a tail-resumptive clause, on the argument
/// that the only thing which will ever splice that continuation is the
/// `Frame::Resume` pushed for it, above the `CloseRegion` frames of every region
/// open at the capture.
///
/// The shape that tests the argument rather than restating it: the
/// tail-resumptive clause's own body performs, and the handler answering *that*
/// is multi-shot and outside everything. The tail continuation is therefore
/// inside a continuation resumed twice, and it carries `r`'s `CloseRegion` frame
/// — so `r` closes during the first resumption and is read during the second.
/// What the answer pins is that the capture one level up took the pin the
/// tail-resumptive one did not.
#[test]
fn a_tail_resumptive_continuation_crossing_a_region_is_covered_from_above() {
    let compiled = Compiled::new(
        r#"
effect e { read op() -> Int }
effect f { read g() -> Int }

pub fn attack() -> Int =
  handle {
    handle {
      with_cell[r](7) { c -> { let v = e.op(); v + cell_get(c) } }
    } with { e.op() -> f.g() }
  } with { f.g() resume k -> k(1) + k(2) }
"#,
    );
    // (1 + 7) + (2 + 7). The second resumption re-enters a region that closed
    // during the first, and reads 7 rather than nothing and rather than whatever
    // a later region put at that position.
    let stats = answers(&compiled, "m.attack", 17);
    assert_eq!(
        stats.pins_taken, 1,
        "one pin, taken where the continuation is named — not at the tail-resumptive capture"
    );
    assert_eq!(stats.closes_deferred, 1);
}

/// The other half of the pin's shape: it claims every region open at the capture
/// and **none opened afterwards**, which is what keeps a region opened inside a
/// handler clause cheap. So a region opened inside a clause body is covered by
/// nothing the outer capture took, and must be covered by a capture taken inside
/// it — which is what this program arranges by performing again from within the
/// new region and resuming that twice.
#[test]
fn a_region_opened_inside_a_clause_body_is_covered_by_the_capture_inside_it() {
    let compiled = Compiled::new(
        r#"
effect e { read op() -> Int }
effect f { read g() -> Int }

pub fn attack() -> Int =
  handle {
    handle { e.op() + f.g() } with {
      e.op() resume k -> with_cell[inner](5) { c -> k(cell_get(c)) },
    }
  } with { f.g() resume j -> j(1) + j(2) }
"#,
    );
    // (5 + 1) + (5 + 2). `inner` closes when the first resumption leaves it, and
    // the second reads it after that.
    let stats = answers(&compiled, "m.attack", 13);
    assert_eq!(stats.closes_deferred, 1);
    assert_eq!(stats.slots_reclaimed_late, 1);
}

// -------------------------------------------- 4. the documented open route

/// ADR 0017 §2's one open route — a continuation parked in an enclosing region's
/// cell, where a nominal constructor's field type erases the brand — with a
/// second region placed where the freed slot would be.
///
/// `region_isolation_audit` asks whether the resumption reads *this run's* cell.
/// This asks the question reclamation added: whether it reads the cell of the
/// region it was captured in, or the one that opened over that position
/// afterwards. The two answers are 0 and 999.
#[test]
fn a_parked_continuation_reads_its_own_region_and_not_the_one_that_replaced_it() {
    let compiled = Compiled::new(
        r#"
effect amb { read flip[coin]() -> Bool }
type Saved = Nothing | Just((Bool) -> Int)

pub fn attack() -> Int =
  with_cell[slot](Nothing) { s -> {
    with_cell[log](0) { c ->
      handle { if amb.flip[coin]() { cell_get(c) } else { 100 } } with {
        amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 },
      }
    };
    with_cell[replacement](999) { d ->
      match cell_get(s) { Just(k) -> k(true), Nothing -> 0 - 1 }
    }
  } }
"#,
    );
    answers(&compiled, "m.attack", 0);
}

// --------------------------------------------------------- 5. the inference

/// The shapes `region_reclamation_audit` does not walk. A capture reachable
/// through two definitions, through a callback builtin the analysis cannot name,
/// through a definition that spawns rather than performs, and through a value
/// applied out of a binding — each of them a region whose own body writes no
/// `handle`, no `perform` and no `simulate`.
#[test]
fn no_region_reaching_a_capture_indirectly_is_inferred_unique() {
    const AMB: &str = "effect amb { read flip[coin]() -> Bool }\n";
    let shapes: &[(&str, &str)] = &[
        (
            "a chain of two definitions",
            "fn coin() -> Bool = amb.flip[coin]()
             fn middle() -> Bool = coin()
             fn go() -> Bool = with_cell[r](0) { c -> middle() }",
        ),
        (
            "a callback this analysis cannot name",
            "fn capturing() -> Bool = amb.flip[coin]()
             fn go(f: (Int) -> Int) -> List<Int> = with_cell[r](0) { c -> map([1, 2], f) }",
        ),
        (
            "a definition that spawns rather than performs",
            "fn work() -> Int = 1
             fn fork() -> Int = simulate { { let t = task.spawn(|| work()); task.join(t) } }
             fn go() -> Int = with_cell[r](0) { c -> fork() }",
        ),
        (
            "a value applied out of a binding, which may be any function",
            "fn coin() -> Bool = amb.flip[coin]()
             fn go(f: () -> Bool) -> Bool = with_cell[r](0) { c -> f() }",
        ),
    ];
    for (what, body) in shapes {
        let compiled = Compiled::new(&format!("{AMB}{body}\n"));
        let regions = compiled.kinds();
        assert!(!regions.is_empty(), "{what}: this shape opens no region");
        assert_eq!(
            regions.unique(),
            0,
            "{what}: `unique` is the claim that nothing can reach the region's slots after its \
             close, and a capture reaches them"
        );
    }
}

/// The capture is in another module, so a whole-program call graph is what has to
/// find it. A per-module analysis would call `r` unique.
#[test]
fn a_capture_installed_in_another_module_still_makes_the_region_shared() {
    let compiled = Compiled::modules(&[
        (
            "a",
            "pub effect e { read op() -> Int }
pub fn body() -> Int / {e.read} = with_cell[r](7) { c -> { let v = e.op(); v + cell_get(c) } }",
        ),
        (
            "b",
            "import a
pub fn attack() -> Int = handle { a::body() } with { a::e.op() resume k -> k(1) + k(2) }",
        ),
    ]);
    assert_eq!(compiled.kinds().unique(), 0);
    // (1 + 7) + (2 + 7), with `r` closing during the first resumption.
    answers(&compiled, "b.attack", 17);
}

/// Why a wrong answer from the inference is survivable, stated as a property of
/// the allocator rather than as a hope: [`Arena::close`] never reads the region's
/// kind. What it reads is whether a live pin covers the region, so the four
/// combinations of kind and pin collapse to two answers decided entirely by the
/// pin.
///
/// This is what makes `region_kind` a report rather than a memory-safety
/// component, and it is what a future precision improvement there must not
/// quietly trade away.
#[test]
fn what_a_close_reclaims_is_decided_by_the_pin_and_never_by_the_kind() {
    for kind in [RegionKind::Unique, RegionKind::Shared] {
        for hold in [false, true] {
            let mut arena = Arena::new();
            let region = arena.open(kind, Span::DUMMY);
            let cell = arena.alloc(Value::Int(1)).expect("the region is open");
            let pin = arena.pin().expect("a region is open");
            if !hold {
                drop(pin);
            }

            let reclaimed = arena.close(region);

            if hold {
                assert_eq!(
                    reclaimed,
                    Reclaim::Retained(1),
                    "{kind}: a live continuation's claim was ignored because of the kind"
                );
                assert_eq!(arena.get(cell), Some(&Value::Int(1)));
            } else {
                assert_eq!(
                    reclaimed,
                    Reclaim::Freed(1),
                    "{kind}: nothing can reach these slots and they were kept anyway"
                );
                assert_eq!(arena.get(cell), None);
            }
        }
    }
}

// ------------------------------------------------------------ 6. generations

/// The generation is what turns a stale read into a diagnostic instead of a wrong
/// value, so the one way a wrong value comes back is the counter coming back
/// around. This states the bound rather than assuming it: a position's generation
/// is exactly the number of invalidating truncations that covered it, it never
/// falls, and the identity it carries today is one no earlier slot at that
/// position held.
///
/// The wrap is `u32::MAX + 1` closes over one position within one arena's life,
/// and an arena lives for one task of one machine. No program in this repository
/// comes near it — `region_reclamation_census` counts 709 closes over the whole
/// corpus — but it is a `wrapping_add`, so the bound is written down here rather
/// than left to be rediscovered.
#[test]
fn a_positions_generation_only_rises_and_never_hands_back_an_identity() {
    const ROUNDS: u32 = 2_000;
    let mut arena = Arena::new();
    let mut seen: Vec<Slot> = Vec::new();
    for round in 0..ROUNDS {
        let region = arena.open(RegionKind::Unique, Span::DUMMY);
        let slot = arena.alloc(Value::Int(round as i64)).expect("just opened");
        assert_eq!(
            slot.index(),
            0,
            "the bump pointer went back to the same place"
        );
        assert_eq!(
            slot.generation(),
            round,
            "a position's generation is the number of closes it has been through"
        );
        assert!(
            !seen.contains(&slot),
            "position 0 handed out an identity it had used before, which is the only way a stale \
             read becomes a value rather than a diagnostic"
        );
        seen.push(slot);
        arena.close(region);
    }
    for stale in &seen[..seen.len() - 1] {
        assert!(
            arena.get(*stale).is_none(),
            "{stale} resolved after its close"
        );
    }
    assert_eq!(
        arena.stats().closes_freed as u32,
        ROUNDS,
        "every round has to have been a real free, or the counter never moved"
    );
}
