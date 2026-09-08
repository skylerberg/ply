//! Reading a slot after its region reclaimed it.

use crate::fixture::Compiled;
use ply_eval::Value;
use ply_eval::arena::{Arena, Reclaim, RegionKind, Slot, Stats};
use ply_span::{Diagnostic, Span, codes};

// ------------------------------------------------------------------ harness

/// The diagnostics that refused `src`, insisting there are some: every caller here is asserting
/// that a use-after-free was caught, so an accepted fixture is the interesting failure.
#[track_caller]
fn refused(src: &str) -> Vec<Diagnostic> {
    match Compiled::rejected(src) {
        d if d.is_empty() => panic!("this reaches a region's slots and was accepted:\n{src}"),
        d => d,
    }
}

impl Compiled {
    /// The machine's answer and the arena it left behind.
    #[track_caller]
    fn run_call(&self, name: &str) -> (Result<Value, Diagnostic>, Stats) {
        let mut machine = self.machine();
        let answer = machine.call(name, Vec::new(), Span::DUMMY);
        (answer, machine.cells().stats())
    }

    fn kinds(&self) -> ply_eval::region_kind::Regions {
        ply_eval::region_kind::infer(&self.program, &self.resolved)
    }
}

#[track_caller]
fn answers(compiled: &Compiled, name: &str, want: i64) -> Stats {
    let (answer, stats) = compiled.run_call(name);
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

/// The escape brand's list, walked to the end.
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
        let diags = refused(src);
        assert!(
            diags
                .iter()
                .any(|d| d.code == codes::TYPE_MISMATCH || d.code == codes::REGION_ESCAPE),
            "{what} carried a cell out of its region: {:?}",
            codes_of(&diags)
        );
    }
}

/// W2's hole was a check that ran *before* alias resolution, and the region model's Consequences name it as
/// the way an escape gets past the brand.
#[test]
fn a_cell_round_tripped_through_a_type_alias_keeps_its_brand() {
    const ALIAS: &str = "type Held = Cell<Int>\nfn keep(c: Held) -> Held = c\n";

    let inside = Compiled::new(&format!(
        "{ALIAS}pub fn read_it() -> Int = with_cell[k](42) {{ c -> cell_get(keep(c)) }}"
    ));
    answers(&inside, "m.read_it", 42);

    let out = refused(&format!(
        "{ALIAS}pub fn leak() -> Held = with_cell[k](42) {{ c -> keep(c) }}"
    ));
    assert!(
        out.iter().any(|d| d.code == codes::TYPE_MISMATCH
            && d.message.contains("escapes its `with_cell[k]` region")),
        "the alias erased the brand on the way out: {out:#?}"
    );

    let deref = refused(&format!("{ALIAS}fn peek(c: Held) -> Int = cell_get(c)"));
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
    let diags = refused(
        r#"law "leak" forall (n: Int) { with_region[r] { with_cell[r](n) { c -> c } } == 0 }"#,
    );
    assert!(
        diags.iter().any(|d| d.code == codes::REGION_ESCAPE),
        "a law's region escaped unchecked: {:?}",
        codes_of(&diags)
    );
}




/// The asymmetry that pays for the three above, and the reason it is a decision rather than an
/// oversight: `with_region` is new syntax with no program depending on the loose rule, so the
/// identical escape is a compile error that names the task.
#[test]
fn the_same_escape_out_of_a_with_region_is_refused_statically() {
    let diags = refused(
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
