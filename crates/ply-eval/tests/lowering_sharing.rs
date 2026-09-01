//! One program, one lowering — however many machines run it.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Machine, Value};
use ply_span::{SourceId, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::rc::Rc;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    let out = f();
    ARMED.with(|c| c.set(false));
    (out, ALLOCS.with(Cell::get))
}

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
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

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn run(&self, machine: &mut Machine<'_>) -> i64 {
        match machine.call("m.top", Vec::new(), Span::DUMMY) {
            Ok(Value::Int(i)) => i,
            other => panic!("`m.top` answered {other:?}"),
        }
    }
}

/// Wide enough that lowering it is more than a rounding error and narrow enough to read: eleven
/// definitions, every one of them reached from `top`.
const SOURCE: &str = r#"
fn scale(x: Int) -> Int = x * 3 + 1
fn clip(x: Int) -> Int = if x > 40 { 40 } else { x }
fn blend(a: Int, b: Int) -> Int {
  let l = [a, b, a + b, a - b];
  let m = {left: a, right: b, sum: a + b};
  clip(scale(a)) + clip(scale(b)) + len(l) + m.sum
}
fn step1(x: Int) -> Int = blend(x, scale(x))
fn step2(x: Int) -> Int = blend(step1(x), clip(x))
fn step3(x: Int) -> Int = blend(step2(x), step1(x))
fn step4(x: Int) -> Int = blend(step3(x), step2(x))
fn step5(x: Int) -> Int = blend(step4(x), step3(x))
fn fold_up(xs: List<Int>) -> Int = fold(xs, 0, |acc, x| acc + clip(x))
fn spread(x: Int) -> Int = fold_up([step1(x), step2(x), step3(x), step4(x), step5(x)])
fn top() -> Int = spread(2) + spread(3) + spread(4)
"#;

/// The number this milestone is about: what a *second* machine over one program pays.
fn second_machine_cost(compiled: &Compiled, share: bool) -> (i64, usize) {
    let mut first = compiled.machine();
    let expected = compiled.run(&mut first);
    let shared = first.share_lowering();
    counted(|| {
        let mut second = compiled.machine();
        if share {
            second.set_lowering(Rc::clone(&shared));
        }
        let answered = compiled.run(&mut second);
        assert_eq!(
            answered, expected,
            "the second machine answered differently from the first"
        );
        answered
    })
}

#[test]
fn a_second_machine_over_one_program_costs_less_when_it_is_handed_the_first_ones_lowering() {
    let compiled = Compiled::new(SOURCE);
    let (answer, alone) = second_machine_cost(&compiled, false);
    let (shared_answer, shared) = second_machine_cost(&compiled, true);

    println!(
        "a second machine over one program: {alone} allocations lowering for itself, \
         {shared} reading the first machine's — {:.1}% of it, {} saved",
        100.0 * shared as f64 / alone as f64,
        alone - shared
    );

    assert_eq!(
        answer, shared_answer,
        "sharing the lowering changed what the program answers"
    );
    assert!(
        shared < alone,
        "a machine handed a filled lowering cost {shared} allocations against {alone} for one \
         that lowered the program itself, so nothing was hoisted"
    );
}

/// The count, so that a regression putting lowering back on the per-machine path fails rather than
/// merely costing.
#[test]
fn a_second_machine_lowers_no_body_the_first_already_lowered() {
    let compiled = Compiled::new(SOURCE);
    let mut first = compiled.machine();
    compiled.run(&mut first);
    let shared = first.share_lowering();
    let filled = shared.len();
    assert!(
        filled >= 11,
        "the fixture reaches eleven definitions and {filled} bodies were lowered"
    );

    let mut second = compiled.machine();
    second.set_lowering(Rc::clone(&shared));
    compiled.run(&mut second);
    assert_eq!(
        shared.len(),
        filled,
        "the second machine lowered {} bodies the first had already lowered",
        shared.len() - filled
    );
}

/// Two programs, same module, same definition names, different answers.
#[test]
fn a_machine_refuses_a_lowering_built_over_a_different_program() {
    let one = Compiled::new("fn top() -> Int = 11\n");
    let two = Compiled::new("fn top() -> Int = 22\n");

    let mut first = one.machine();
    assert_eq!(one.run(&mut first), 11);

    let mut second = two.machine();
    second.set_lowering(first.share_lowering());
    assert!(
        !Rc::ptr_eq(&second.share_lowering(), &first.share_lowering()),
        "a machine took a lowering built over another program"
    );
    assert_eq!(
        two.run(&mut second),
        22,
        "the second program's `top` answered with the first program's body"
    );
}
