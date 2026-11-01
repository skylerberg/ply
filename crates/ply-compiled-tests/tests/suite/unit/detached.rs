use super::frames::{boxed, count, every_kind, freed, release_box};
use crate::bare::{context_of, helper};
use ply_compiled::detached::{INSTANCE_DETACHED, INSTANCE_HEADER_WORDS, INSTANCE_VARIABLES};
use ply_compiled::emitted::{Block, OwnedWord, PlyFunctionDescriptor, PlyOp};
use ply_compiled::frames::{PlyFrame, call_recording_slots};
use ply_compiled::heap::{HEADER, Word, imm, imm_value};
use ply_compiled::native::{I64, Returned};
use ply_compiled::rt::{Context, rt_detached_open, rt_detached_perform};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

const CHOICE_EFFECT_ID: i64 = 0;
const FLIP_OPERATION: i64 = 1;

const BOX_PAYLOADS: [Word; 5] = [1, 2, 3, 4, 5];

thread_local! {
    static TEXT: Arc<str> = Arc::from("held across a stop");
    static SEEN: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static STACK_EMPTY_AT_CLAUSE: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    static CONTINUATION_UNTYPED_LOCAL_FLAGS: RefCell<Vec<(bool, bool)>> = const { RefCell::new(Vec::new()) };
    static HANDLER_RECORD_POINTER: Cell<*const Word> = const { Cell::new(std::ptr::null()) };
}

fn seen(n: u32) {
    SEEN.with(|s| s.borrow_mut().push(n));
}

fn taken<T>(cell: &'static std::thread::LocalKey<RefCell<Vec<T>>>) -> Vec<T> {
    cell.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

fn text_holders() -> usize {
    TEXT.with(Arc::strong_count)
}

#[repr(C)]
struct Linked {
    h: PlyFrame,
    slot: Word,
}

unsafe extern "C" fn box_each(context: *mut Context, slots: *mut Word, _d: i64) {
    release_box(context, unsafe { *slots });
}

unsafe fn with_box_in_frame(
    context: *mut Context,
    marker: Word,
    call: impl FnOnce() -> Word,
) -> Word {
    let mut frame = Linked {
        h: PlyFrame {
            previous: std::ptr::null_mut(),
            each: box_each,
        },
        slot: boxed(context, marker),
    };
    let record = (&raw mut frame).cast::<PlyFrame>();
    unsafe {
        if !(*context).frames.is_null() {
            (*record).previous = (*context).frames;
            (*context).frames = record;
        }
        let r = call();
        if (*context).frames == record {
            (*context).frames = (*record).previous;
        }
        let b = (*record.cast::<Linked>()).slot;
        if (*context).failed == 0 {
            seen(count(b));
        }
        release_box(context, b);
        r
    }
}

static FLIP_OP: PlyOp = PlyOp {
    effect: CHOICE_EFFECT_ID,
    op: FLIP_OPERATION,
    answer: std::ptr::null(),
    own: 0,
    params_len: 0,
    params: std::ptr::null(),
    variables_length: 0,
    variables: std::ptr::null(),
};

unsafe extern "C" fn body(context: *mut Context, _block: Word, _args: *const Word) -> Word {
    unsafe {
        with_box_in_frame(context, BOX_PAYLOADS[0], || {
            through_a_runtime_frame(context)
        })
    }
}

unsafe fn through_a_runtime_frame(context: *mut Context) -> Word {
    let (held, words) = TEXT.with(|t| {
        every_kind(
            context,
            [BOX_PAYLOADS[1], BOX_PAYLOADS[2], BOX_PAYLOADS[3]],
            t,
        )
    });
    let (r, back) = unsafe {
        call_recording_slots(context, held, || {
            with_box_in_frame(context, BOX_PAYLOADS[4], || {
                let flip = rt_detached_perform(
                    context,
                    HANDLER_RECORD_POINTER.get(),
                    &FLIP_OP,
                    I64(-1),
                    std::ptr::null(),
                );
                let flip = std::mem::transmute::<OwnedWord, Word>(flip);
                if (*context).failed != 0 {
                    0
                } else {
                    imm(if imm_value(flip) == 1 { 1 } else { 2 })
                }
            })
        })
    };
    unsafe {
        if (*context).failed == 0 {
            for w in words {
                seen(count(w));
            }
        }
        for one in back {
            one.release(context);
        }
    }
    r
}

unsafe fn clause_ran(context: *mut Context, k: Word) {
    let unrecorded = unsafe { (*context).frames.is_null() };
    STACK_EMPTY_AT_CLAUSE.with(|c| c.borrow_mut().push(unrecorded));
    let descriptor = unsafe { &**(k as *const *const PlyFunctionDescriptor).byte_add(HEADER) };
    let crosses =
        unsafe { (descriptor.share)(context, k) != 0 || (descriptor.keep)(context, k) != 0 };
    let seen = (descriptor.typed.is_none(), !crosses);
    CONTINUATION_UNTYPED_LOCAL_FLAGS.with(|t| t.borrow_mut().push(seen));
}

unsafe extern "C" fn once(context: *mut Context, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(context, *args);
        let k = Block::of(*args);
        let first = k.call(context, &[imm(1)]);
        k.release_function_if_last(context);
        if (*context).failed != 0 {
            return 0;
        }
        imm(imm_value(first) * 10)
    }
}

unsafe extern "C" fn twice(context: *mut Context, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(context, *args);
        let k = Block::of(*args);
        let first = k.call(context, &[imm(1)]);
        let second = if (*context).failed == 0 {
            k.call(context, &[imm(0)])
        } else {
            0
        };
        k.release_function_if_last(context);
        if (*context).failed != 0 {
            return 0;
        }
        imm(imm_value(first) * 10 + imm_value(second))
    }
}

unsafe extern "C" fn never(context: *mut Context, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(context, *args);
        Block::of(*args).release_function_if_last(context);
    }
    imm(42)
}

unsafe extern "C" fn parks(context: *mut Context, _block: Word, args: *const Word) -> Word {
    unsafe {
        clause_ran(context, *args);
        *args
    }
}

type Alloc = unsafe extern "C" fn(*mut Context, Word) -> Word;
type Free = unsafe extern "C" fn(*mut Context, Word, Word);

const CLOSURE: Word = (HEADER + 8) as Word;

fn closure(context: *mut Context, descriptor: &'static PlyFunctionDescriptor) -> Word {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, CLOSURE) };
    unsafe { *(w as *mut Word).byte_add(HEADER) = (&raw const *descriptor) as Word };
    w
}

unsafe extern "C" fn released(context: *mut Context, h: OwnedWord) -> Word {
    let w = unsafe { std::mem::transmute::<OwnedWord, Word>(h) };
    unsafe { helper::<Free>("rt_free")(context, w, CLOSURE) };
    0
}

unsafe extern "C" fn stays_here(_context: *mut Context, _w: Word) -> Word {
    0
}

const fn descriptor(
    uniform: unsafe extern "C" fn(*mut Context, Word, *const Word) -> Word,
) -> PlyFunctionDescriptor {
    PlyFunctionDescriptor {
        uniform,
        typed: None,
        convention: 0,
        release: released,
        share: stays_here,
        keep: stays_here,
    }
}

static BODY: PlyFunctionDescriptor = descriptor(body);
static ONCE: PlyFunctionDescriptor = descriptor(once);
static TWICE: PlyFunctionDescriptor = descriptor(twice);
static NEVER: PlyFunctionDescriptor = descriptor(never);
static RETURNS_CONTINUATION: PlyFunctionDescriptor = descriptor(parks);

fn unit() -> Context {
    context_of(&["amb", "flip"], Vec::new())
}

const RECORD: usize = 4;
const INSTANCE_BYTES: Word = (HEADER + 8 * (INSTANCE_HEADER_WORDS + RECORD)) as Word;

unsafe extern "C" fn instance_released(context: *mut Context, h: OwnedWord) -> Word {
    let w = unsafe { std::mem::transmute::<OwnedWord, Word>(h) };
    unsafe {
        let clause = *(w as *const Word)
            .byte_add(HEADER)
            .add(INSTANCE_HEADER_WORDS);
        Block::of(clause).release_function_if_last(context);
        helper::<Free>("rt_free")(context, w, INSTANCE_BYTES);
    }
    0
}

static INSTANCE_DESCRIPTOR: PlyFunctionDescriptor = PlyFunctionDescriptor {
    release: instance_released,
    ..descriptor(body)
};

fn instance(context: *mut Context, clause: Word) -> (Word, *const Word) {
    let w = unsafe { helper::<Alloc>("rt_alloc_bytes")(context, INSTANCE_BYTES) };
    let head = unsafe { (w as *mut Word).byte_add(HEADER) };
    let record = unsafe { head.add(INSTANCE_HEADER_WORDS) };
    let mut words = [0; INSTANCE_HEADER_WORDS + RECORD];
    words[0] = (&raw const INSTANCE_DESCRIPTOR) as Word;
    words[1] = RECORD as Word;
    words[INSTANCE_DETACHED] = -1;
    words[INSTANCE_VARIABLES] = RECORD as Word;
    words[INSTANCE_HEADER_WORDS..].copy_from_slice(&[clause, record as Word, 0, 0]);
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr(), head, words.len()) };
    (w, record)
}

unsafe fn handled(context: *mut Context, clause: &'static PlyFunctionDescriptor) -> Word {
    let body = closure(context, &BODY);
    let (instance, record) = instance(context, closure(context, clause));
    HANDLER_RECORD_POINTER.set(record);
    let result =
        unsafe { rt_detached_open(context, Block::of(body), Block::of(0), Block::of(instance)) };
    unsafe { Block::of(instance).release_function_if_last(context) };
    unsafe { std::mem::transmute::<OwnedWord, Word>(result) }
}

fn failure(context: *mut Context) -> String {
    unsafe { (*context).take_failure() }.map_or_else(String::new, |d| d.message)
}

const TOKEN: (bool, bool) = (true, true);

#[test]
fn resuming_a_suspended_computation_once_lets_it_run_to_the_end_and_releases_every_resource_exactly_once()
 {
    let mut owned = unit();
    let context = &raw mut owned;
    unsafe {
        (*context).begin();
        let result = handled(context, &ONCE);
        assert_eq!((*context).failed, 0, "{}", failure(context));
        assert_eq!(imm_value(result), 10);
        assert_eq!(taken(&SEEN), [1; 5]);
        assert_eq!(taken(&STACK_EMPTY_AT_CLAUSE), [true]);
        assert_eq!(taken(&CONTINUATION_UNTYPED_LOCAL_FLAGS), [TOKEN]);
        assert_eq!(freed(), BOX_PAYLOADS);
        assert_eq!(text_holders(), 1);
        assert_eq!((*context).heap.live(), 0);
        (*context).end();
    }
}

#[test]
fn resuming_a_suspended_computation_a_second_time_fails_and_every_resource_is_still_released_exactly_once()
 {
    let mut owned = unit();
    let context = &raw mut owned;
    unsafe {
        (*context).begin();
        handled(context, &TWICE);
        assert_ne!((*context).failed, 0);
        let said = failure(context);
        assert!(said.contains("resumed a second time"), "{said}");
        assert_eq!(taken(&SEEN), [1; 5]);
        assert_eq!(freed(), BOX_PAYLOADS);
        assert_eq!(text_holders(), 1);
        assert_eq!((*context).heap.live(), 0);
        (*context).end();
    }
}

#[test]
fn dropping_a_suspended_computation_without_resuming_it_releases_its_resources_exactly_once() {
    let mut owned = unit();
    let context = &raw mut owned;
    unsafe {
        (*context).begin();
        let result = handled(context, &NEVER);
        assert_eq!((*context).failed, 0, "{}", failure(context));
        assert_eq!(imm_value(result), 42);
        assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
        assert_eq!(taken(&CONTINUATION_UNTYPED_LOCAL_FLAGS), [TOKEN]);
        assert_eq!(freed(), BOX_PAYLOADS);
        assert_eq!(text_holders(), 1);
        assert_eq!((*context).heap.live(), 0);
        (*context).end();
    }
}

#[test]
fn ending_an_entry_call_while_a_computation_is_still_suspended_releases_the_resources_of_that_computation_exactly_once()
 {
    let mut owned = unit();
    let context = &raw mut owned;
    unsafe {
        (*context).begin();
        handled(context, &RETURNS_CONTINUATION);
        assert_eq!((*context).failed, 0, "{}", failure(context));
        assert!(freed().is_empty());
        assert!(text_holders() > 1);
        (*context).end();
        assert!(taken(&SEEN).is_empty(), "a frame ran past the stop");
        assert_eq!(freed(), BOX_PAYLOADS);
        assert_eq!(text_holders(), 1);
    }
}
