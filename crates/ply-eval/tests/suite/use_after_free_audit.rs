//! Reading a slot after its region reclaimed it.

use ply_core::{CheckOutput, check_program};
use ply_eval::arena::{Arena, Reclaim, RegionKind, Slot, Stats};
use ply_eval::{Machine, Value};
use ply_span::{Diagnostic, SourceId, Span, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

// ------------------------------------------------------------------ harness

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
    #[track_caller]
    fn run(&self, name: &str) -> (Result<Value, Diagnostic>, Stats) {
        let mut machine = self.machine();
        let answer = machine.call(name, Vec::new(), Span::DUMMY);
        (answer, machine.cells().stats())
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
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

/// ADR 0017 §2's list, walked to the end.
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

/// W2's hole was a check that ran *before* alias resolution, and ADR 0017's Consequences name it as
/// the way an escape gets past the brand.
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

/// A `law` body opens regions like any other body, so `check_regions` has to have filed a site for
/// it.
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

/// The route the ADR deliberately leaves open, taken past the argument that opens it.
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

/// The same route with the task asleep across the close, so that it is live and unfinished when the
/// second region opens rather than merely unscheduled.
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

/// A task spawned **after** the region closed, which is where the pin discipline runs out:
/// `TaskRegions::pin` answers `None` when no program region is open, so the grandchild's own
/// `spawn` claims nothing at all.
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

/// The asymmetry that pays for the three above, and the reason it is a decision rather than an
/// oversight: `with_region` is new syntax with no program depending on the loose rule, so the
/// identical escape is a compile error that names the task.
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

/// `handler::perform` takes no pin for a tail-resumptive clause, on the argument that the only
/// thing which will ever splice that continuation is the `Frame::Resume` pushed for it, above the
/// `CloseRegion` frames of every region open at the capture.
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
    // (1 + 7) + (2 + 7).
    let stats = answers(&compiled, "m.attack", 17);
    assert_eq!(
        stats.pins_taken, 1,
        "one pin, taken where the continuation is named — not at the tail-resumptive capture"
    );
    assert_eq!(stats.closes_deferred, 1);
}

/// The other half of the pin's shape: it claims every region open at the capture and **none opened
/// afterwards**, which is what keeps a region opened inside a handler clause cheap.
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
    // (5 + 1) + (5 + 2).
    let stats = answers(&compiled, "m.attack", 13);
    assert_eq!(stats.closes_deferred, 1);
    assert_eq!(stats.slots_reclaimed_late, 1);
}

/// ADR 0017 §2's one open route — a continuation parked in an enclosing region's cell, where a
/// nominal constructor's field type erases the brand — with a second region placed where the freed
/// slot would be.
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

/// The shapes `region_reclamation_audit` does not walk.
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

/// The capture is in another module, so a whole-program call graph is what has to find it.
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

/// Why a wrong answer from the inference is survivable, stated as a property of the allocator
/// rather than as a hope: [`Arena::close`] never reads the region's kind.
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

/// The generation is what turns a stale read into a diagnostic instead of a wrong value, so the one
/// way a wrong value comes back is the counter coming back around.
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
