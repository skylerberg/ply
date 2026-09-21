use crate::fixture::Compiled;
use ply_eval::Value;
use ply_eval::arena::{Arena, Reclaim, RegionKind, Slot, Stats};
use ply_span::{Diagnostic, Span, codes};

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

/// A scheduler older than the region drains what nobody joined, after the region's `}`.
#[test]
fn a_cell_handed_to_a_task_the_region_outlives_is_refused_statically() {
    for src in [
        r#"
pub fn attack() -> Int = simulate {
  { let t = with_cell[s](11) { c -> task.spawn(|| cell_get(c)) };
    task.join(t) }
}
"#,
        // A join inside the region does not license it: no type records the join.
        r#"
pub fn attack() -> Int = simulate {
  with_cell[s](11) { c -> { let t = task.spawn(|| cell_get(c)); task.join(t) } }
}
"#,
    ] {
        let diags = refused(src);
        let escape = diags
            .iter()
            .find(|d| d.code == codes::REGION_ESCAPE)
            .unwrap_or_else(|| panic!("a task reached a `with_cell` region's cell: {diags:#?}"));
        assert!(
            escape.message.contains("sent to another task"),
            "{}",
            escape.message
        );
    }

    // The remedy: a scheduler opened inside the region cannot outlive the cell.
    Compiled::new(
        r#"
pub fn guarded() -> Int =
  with_cell[s](11) { c -> simulate { task.join(task.spawn(|| cell_get(c))) } }
"#,
    );
}

/// [`Arena::close`] never reads the region's kind.
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

/// Only the generation catches a stale read, so wrapping is the one way a wrong value returns.
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
