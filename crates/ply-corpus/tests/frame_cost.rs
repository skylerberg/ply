//! What a step costs the allocator, exactly.

use ply_corpus::build::generate;
use ply_corpus::pipeline::front;
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write::write;
use ply_eval::cont::{Frame, Prompt, Segment};
use ply_eval::{Engine, Env, Evaluator, Interp, Machine, Stack};
use ply_span::Span;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::path::Path;
use std::rc::Rc;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|c| c.set(0));
    let out = f();
    (out, ALLOCS.with(Cell::get))
}

fn call_frame() -> Frame {
    Frame::Call {
        name: None,
        call_site: Span::DUMMY,
        memo: false,
    }
}

/// One allocation to push a frame — the link that holds it — and none at all to pop one.
#[test]
fn pushing_one_frame_costs_one_allocation_and_popping_costs_none() {
    let mut stack = Stack::new();
    // Warm the allocator so the first iteration is not measuring a fresh arena.
    for _ in 0..64 {
        stack = stack.push(call_frame());
    }

    const PUSHES: usize = 1_000;
    let (grown, allocs) = allocations_of(|| {
        let mut s = stack.clone();
        for _ in 0..PUSHES {
            s = s.push(call_frame());
        }
        s
    });
    let per_push = allocs as f64 / PUSHES as f64;

    // `into_next` rather than `next`, because that is what the machine's return transition uses: it
    // owns its stack, so the frame is moved out of its link rather than cloned.
    let (_, pop_allocs) = allocations_of(|| {
        let mut s = grown.clone();
        for _ in 0..PUSHES {
            match s.into_next() {
                ply_eval::Next::Frame(_, rest) => s = rest,
                other => {
                    black_box(&other);
                    return;
                }
            }
        }
        black_box(s);
    });
    let per_pop = pop_allocs as f64 / PUSHES as f64;

    println!(
        "Frame is {} bytes, Segment {} bytes; push = {per_push} allocations, pop = {per_pop}",
        std::mem::size_of::<Frame>(),
        std::mem::size_of::<Segment>(),
    );

    assert!(
        per_push <= 1.0,
        "a frame push cost {per_push} allocations, above the one link holding it"
    );
    assert_eq!(
        per_pop, 0.0,
        "a frame pop cost {per_pop} allocations, and popping an unshared link allocates nothing"
    );
}

/// Opening a prompt is the operation that genuinely needs a new segment, and a frame push must
/// never cost more than it: a `handle` is rare and a push happens on all but a handful of
/// transitions.
#[test]
fn pushing_a_frame_costs_no_more_than_opening_a_prompt() {
    let prompt = Rc::new(Prompt {
        clauses: Rc::new(Vec::new()),
        effects: Rc::new(Vec::new()),
        ret: None,
        env: Env::empty(),
        module: 0,
        span: Span::DUMMY,
    });

    let mut stack = Stack::new();
    for _ in 0..64 {
        stack = stack.push(call_frame());
    }

    const N: usize = 1_000;
    let (_, prompts) = allocations_of(|| {
        let mut s = stack.clone();
        for _ in 0..N {
            s = s.push_prompt(prompt.clone());
        }
        s
    });
    let (_, frames) = allocations_of(|| {
        let mut s = stack.clone();
        for _ in 0..N {
            s = s.push(call_frame());
        }
        s
    });

    println!(
        "per operation: push_prompt = {}, push = {}",
        prompts as f64 / N as f64,
        frames as f64 / N as f64
    );
    assert!(prompts > 0);
    assert!(
        frames <= prompts,
        "pushing a frame allocated {frames} against {prompts} for opening a prompt, so the frequent operation is again the dearer one"
    );
}

fn corpus(root: &Path) {
    let spec = CorpusSpec {
        seed: 9,
        modules: 6,
        defs_per_module: 8,
        tests: 24,
        depth: 3,
        ..CorpusSpec::default()
    };
    write(root, &spec, &generate(&spec)).expect("the corpus must be writable");
}

/// The same programs, on both engines, counted rather than timed.
#[test]
fn the_machine_allocates_more_per_test_than_the_tree_walker() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let root = dir.path().join("corpus");
    corpus(&root);
    let front = front(&root).expect("the corpus must compile");

    let mut counts = Vec::new();
    for engine in [Engine::Treewalk, Engine::Machine] {
        let mut worker: Box<dyn Evaluator> = match engine {
            Engine::Treewalk => {
                Box::new(Interp::new(&front.program, &front.resolved, &front.check))
            }
            Engine::Machine => {
                Box::new(Machine::new(&front.program, &front.resolved, &front.check))
            }
        };
        // A first pass so lazily-built state is not charged to the count.
        for index in 0..worker.test_count() {
            worker.eval_test(index).expect("the corpus passes");
        }
        let (_, allocs) = allocations_of(|| {
            for index in 0..worker.test_count() {
                worker.eval_test(index).expect("the corpus passes");
            }
        });
        counts.push((engine, allocs, worker.test_count()));
    }

    for (engine, allocs, tests) in &counts {
        println!(
            "{}: {} allocations over {} tests ({:.0} per test)",
            engine.as_str(),
            allocs,
            tests,
            *allocs as f64 / *tests as f64
        );
    }

    let tree = counts[0].1;
    let machine = counts[1].1;
    assert!(tree > 0 && machine > 0);
    // A ceiling rather than an equality: this documents the gap the profile found and fails if it
    // widens, without pretending the current ratio is a target.
    assert!(
        machine < tree * 4,
        "the machine allocated {machine} against the tree-walker's {tree}, which is worse than the gap this test was written over"
    );
}
