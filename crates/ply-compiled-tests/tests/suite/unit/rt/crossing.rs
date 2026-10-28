//! Values crossing between emitted code and the runtime a part at a time, as emitted code calls the
//! natives: written as the codec writes them, read back part by part, and crossings begun inside
//! others leaving the outer as they found it.

use crate::bare::{context, context_calling, helper};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Carried, Handed, PlyType};
use ply_compiled::heap::{HEADER, Word, imm, word_at};
use ply_compiled::native::Answered;
use ply_compiled::rt::{Ctx, Lifted};
use ply_core::codec::{decode, encode, tag};
use ply_core::{Decimal, Fields, Fixed, INT_TYPES, IntTy, Symbol, Value};
use std::sync::Arc;

type N0 = unsafe extern "C" fn(*mut Ctx);
type A0 = unsafe extern "C" fn(*mut Ctx) -> Word;
type N1 = unsafe extern "C" fn(*mut Ctx, Word);
type A1 = unsafe extern "C" fn(*mut Ctx, Word) -> Word;
type N2 = unsafe extern "C" fn(*mut Ctx, Word, Word);
type A2 = unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word;
type N3 = unsafe extern "C" fn(*mut Ctx, Word, Word, Word);
type A3 = unsafe extern "C" fn(*mut Ctx, Word, Word, Word) -> Word;

/// The native the compiler declares as `name`, at `F`, as a unit binds it.
fn native<F: Copy>(name: &str) -> F {
    helper(&builtin_helper_name(name))
}

fn text(ctx: *mut Ctx, native_name: &str, s: &[u8]) {
    unsafe { native::<N2>(native_name)(ctx, s.as_ptr() as Word, s.len() as Word) }
}

/// What `write` writes between opening a lowering and closing it, copied out as emitted code copies
/// the taken bytes.
fn lowered(ctx: &mut Ctx, again: bool, write: impl FnOnce(*mut Ctx)) -> Vec<u8> {
    let ctx: *mut Ctx = ctx;
    unsafe {
        native::<N1>("?lower_open")(ctx, Word::from(again));
        write(ctx);
        let n = native::<A0>("?lower_close")(ctx);
        let mut out = vec![0u8; n as usize];
        native::<N2>("?taken")(ctx, out.as_mut_ptr() as Word, n);
        out
    }
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(k, v)| (Symbol::new(k), v))
            .collect(),
    )))
}

fn width(ty: IntTy) -> Word {
    INT_TYPES.iter().position(|t| *t == ty).expect("a width") as Word
}

fn decimal_words(d: Decimal) -> (Word, Word) {
    let b = d.serialize();
    (
        i64::from_le_bytes(b[..8].try_into().unwrap()),
        i64::from_le_bytes(b[8..].try_into().unwrap()),
    )
}

/// Every kind of part, written by the natives, is the bytes the codec writes of the same value.
#[test]
fn the_natives_write_what_the_codec_writes() {
    let mut ctx = context();
    let d = Decimal::new(-12345, 3);
    let bytes = lowered(&mut ctx, false, |ctx| unsafe {
        let (lo, hi) = decimal_words(d);
        native::<N2>("?out_count")(ctx, tag::RECORD as Word, 8);
        text(ctx, "?out_name", b"a_decimal");
        native::<N2>("?out_decimal")(ctx, lo, hi);
        text(ctx, "?out_name", b"b_narrow");
        // A narrow width however its holder extends it: here zero-extended.
        native::<N3>("?out_fixed")(ctx, width(IntTy::I8), 0xff, 0);
        text(ctx, "?out_name", b"c_wide");
        native::<N3>("?out_fixed")(ctx, width(IntTy::U128), -1, 7);
        text(ctx, "?out_name", b"d_int");
        native::<N1>("?out_int")(ctx, -5);
        text(ctx, "?out_name", b"e_text");
        text(ctx, "?out_str", "né".as_bytes());
        text(ctx, "?out_name", b"f_some");
        native::<N3>("?out_ctor")(ctx, b"Some".as_ptr() as Word, 4, 1);
        native::<N1>("?out_tag")(ctx, tag::UNIT as Word);
        text(ctx, "?out_name", b"g_flag");
        native::<N1>("?out_tag")(ctx, tag::TRUE as Word);
        text(ctx, "?out_name", b"h_list");
        native::<N2>("?out_count")(ctx, tag::LIST as Word, 3);
        native::<N1>("?out_float")(ctx, ply_core::float::bits(1.5) as Word);
        native::<N1>("?out_char")(ctx, 'c' as Word);
        text(ctx, "?out_bytes", b"\x00\xff");
    });
    assert_eq!(ctx.failed, 0);
    let wide = (7u128 << 64) | u128::from(u64::MAX);
    let v = record(vec![
        ("a_decimal", Value::Decimal(d)),
        (
            "b_narrow",
            Value::Fixed(Fixed::new(IntTy::I8, -1i128 as u128)),
        ),
        ("c_wide", Value::Fixed(Fixed::new(IntTy::U128, wide))),
        ("d_int", Value::Int(-5)),
        ("e_text", Value::str("né")),
        ("f_some", Value::ctor("Some", vec![Value::Unit])),
        ("g_flag", Value::Bool(true)),
        (
            "h_list",
            Value::list(vec![
                Value::Float(1.5),
                Value::Char('c'),
                Value::bytes(b"\x00\xff"),
            ]),
        ),
    ]);
    assert_eq!(bytes, encode(&v).unwrap());
}

/// Asked of a value written before, a lowering that shares writes it again as `AGAIN`, which reads
/// back as a copy; one that does not writes it whole each time.
#[test]
fn a_value_written_twice_is_written_again_only_where_the_lowering_shares() {
    let mut ctx = context();
    let (shared, inner) = (0x1000 as Word, 0x2000 as Word);
    let write = |ctx: *mut Ctx| unsafe {
        let again: A1 = native("?out_again");
        native::<N2>("?out_count")(ctx, tag::LIST as Word, 3);
        for _ in 0..2 {
            if again(ctx, shared) == 0 {
                native::<N2>("?out_count")(ctx, tag::LIST as Word, 1);
                if again(ctx, inner) == 0 {
                    text(ctx, "?out_str", b"a string long enough to share");
                }
            }
        }
        if again(ctx, inner) == 0 {
            text(ctx, "?out_str", b"a string long enough to share");
        }
    };
    let with = lowered(&mut ctx, true, write);
    let without = lowered(&mut ctx, false, write);
    assert_eq!(ctx.failed, 0);
    let s = Value::str("a string long enough to share");
    let whole = Value::list(vec![
        Value::list(vec![s.clone()]),
        Value::list(vec![s.clone()]),
        s,
    ]);
    assert_eq!(without, encode(&whole).unwrap());
    assert_eq!(decode(&with).unwrap(), whole);
    assert!(
        with.len() + 50 < without.len(),
        "{} bytes shared, {} not",
        with.len(),
        without.len()
    );
}

/// Bytes the codec wrote, read a part at a time as a value's type reads them.
#[test]
fn the_natives_read_what_the_codec_writes() {
    let mut ctx = context();
    let d = Decimal::new(271828, 5);
    let v = Value::list(vec![
        Value::ctor("m.Pair", vec![Value::Int(-9), Value::str("ok")]),
        record(vec![("x", Value::Float(-0.25)), ("y", Value::Char('é'))]),
        Value::Decimal(d),
        Value::Fixed(Fixed::new(IntTy::I16, -2i128 as u128)),
        Value::Bool(false),
    ]);
    let bytes = encode(&v).unwrap();
    let c: *mut Ctx = &mut ctx;
    unsafe {
        native::<N2>("?lift_open")(c, bytes.as_ptr() as Word, bytes.len() as Word);
        let count: A1 = native("?in_count");
        assert_eq!(count(c, tag::LIST as Word), 5);
        assert_eq!(count(c, tag::CTOR as Word), 2);
        let is: A2 = native("?in_name_is");
        assert_eq!(is(c, b"m.Pair".as_ptr() as Word, 6), 1);
        assert_eq!(is(c, b"m.Other".as_ptr() as Word, 7), 0);
        assert_eq!(native::<A0>("?in_int")(c), -9);
        let n = native::<A0>("?in_str")(c);
        let mut s = vec![0u8; n as usize];
        native::<N2>("?taken")(c, s.as_mut_ptr() as Word, n);
        assert_eq!(s, b"ok");
        assert_eq!(count(c, tag::RECORD as Word), 2);
        native::<N2>("?in_field")(c, b"x".as_ptr() as Word, 1);
        assert_eq!(
            native::<A0>("?in_float")(c),
            ply_core::float::bits(-0.25) as Word
        );
        native::<N2>("?in_field")(c, b"y".as_ptr() as Word, 1);
        assert_eq!(native::<A0>("?in_char")(c), 'é' as Word);
        let high: A0 = native("?in_high");
        let lo = native::<A0>("?in_decimal")(c);
        assert_eq!((lo, high(c)), decimal_words(d));
        let lo = native::<A1>("?in_fixed")(c, width(IntTy::I16));
        assert_eq!((lo, high(c)), (-2, -1), "sign-extended to 128 bits");
        assert_eq!(native::<A0>("?in_tag")(c), tag::FALSE as Word);
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut ctx);
}

/// What a descriptor lifts a type that can hold a function by fails the entry, saying a function
/// never crosses back.
#[test]
fn a_lifting_of_a_type_that_can_hold_a_function_fails_the_entry() {
    let mut ctx = context();
    ctx.begin(1_000);
    let refused: unsafe extern "C" fn(*mut Ctx) = helper("rt_never_lifted");
    unsafe { refused(&mut ctx) };
    assert_ne!(ctx.failed, 0, "the lifting was not refused");
    let why = format!("{:?}", ctx.take_failure());
    assert!(why.contains("never back"), "{why}");
    ctx.end();
}

/// Asserts the context did not fail, saying why where it did.
pub fn unfailed(ctx: &mut Ctx) {
    let failed = ctx.failed;
    let why = ctx.take_failure();
    assert_eq!(failed, 0, "{why:?}");
}

fn lift_and(bytes: &[u8], read: impl FnOnce(*mut Ctx)) -> String {
    let mut ctx = context();
    let c: *mut Ctx = &mut ctx;
    unsafe {
        native::<N2>("?lift_open")(c, bytes.as_ptr() as Word, bytes.len() as Word);
        read(c);
        native::<N0>("?lift_close")(c);
    }
    assert_ne!(ctx.failed, 0, "the lifting read on");
    ctx.take_failure().expect("a diagnostic").message
}

/// Bytes that do not hold what a type reads are Ply's defect, and fail the context.
#[test]
fn bytes_that_do_not_read_as_the_type_fail_the_context() {
    let int = encode(&Value::Int(3)).unwrap();
    let said = lift_and(&int, |c| unsafe {
        native::<A0>("?in_float")(c);
    });
    assert!(said.contains("tagged 3 where one tagged 4"), "{said}");
    let said = lift_and(&int, |_| {});
    assert!(said.contains("bytes follow"), "{said}");
    let rec = encode(&record(vec![("x", Value::Int(1))])).unwrap();
    let said = lift_and(&rec, |c| unsafe {
        native::<A1>("?in_count")(c, tag::RECORD as Word);
        native::<N2>("?in_field")(c, b"y".as_ptr() as Word, 1);
        native::<A0>("?in_int")(c);
    });
    assert!(said.contains("field `x` where `y` was read"), "{said}");
    let said = lift_and(b"not PLV1", |_| {});
    assert!(said.contains("not an encoded value"), "{said}");
}

/// The read that answers a width's or a `Decimal`'s low word keeps the word above it for one ask:
/// a narrow width's as the width's sign extends it, a 128-bit width's its own.
#[test]
fn a_wide_values_high_word_is_kept_for_one_ask_after_its_low_word() {
    let mut ctx = context();
    let wide = (5u128 << 64) | 9;
    let v = Value::list(vec![
        Value::Fixed(Fixed::new(IntTy::U128, wide)),
        Value::Fixed(Fixed::new(IntTy::I128, -3i128 as u128)),
        Value::Fixed(Fixed::new(IntTy::U8, 255)),
        Value::Fixed(Fixed::new(IntTy::U64, u128::from(u64::MAX))),
    ]);
    let bytes = encode(&v).unwrap();
    let c: *mut Ctx = &mut ctx;
    unsafe {
        native::<N2>("?lift_open")(c, bytes.as_ptr() as Word, bytes.len() as Word);
        assert_eq!(native::<A1>("?in_count")(c, tag::LIST as Word), 4);
        let (fixed, high): (A1, A0) = (native("?in_fixed"), native("?in_high"));
        assert_eq!((fixed(c, width(IntTy::U128)), high(c)), (9, 5));
        assert_eq!((fixed(c, width(IntTy::I128)), high(c)), (-3, -1));
        assert_eq!((fixed(c, width(IntTy::U8)), high(c)), (255, 0));
        assert_eq!(
            (fixed(c, width(IntTy::U64)), high(c)),
            (-1, 0),
            "zero-extended past its 64 bits"
        );
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut ctx);
    let one = encode(&Value::Fixed(Fixed::new(IntTy::U128, wide))).unwrap();
    let said = lift_and(&one, |c| unsafe {
        native::<A1>("?in_fixed")(c, width(IntTy::U128));
        native::<A0>("?in_high")(c);
        native::<A0>("?in_high")(c);
    });
    assert!(said.contains("no wide value was read"), "{said}");
}

/// A 128-bit width read as its two words is made a box of them.
#[test]
fn a_128_bit_widths_two_words_box_as_its_two_words() {
    let boxed: A3 = helper("rt_box_wide");
    let mut ctx = context();
    for (ty, bits) in [
        (IntTy::U128, (5u128 << 64) | 9),
        (IntTy::I128, -3i128 as u128),
    ] {
        let (lo, hi) = (bits as u64 as Word, (bits >> 64) as u64 as Word);
        let w = unsafe { boxed(&mut ctx, lo, hi, width(ty)) };
        let o = w as *mut ply_compiled::heap::Obj;
        assert_eq!(unsafe { (word_at(o, 0), word_at(o, 1)) }, (lo, hi));
    }
    unfailed(&mut ctx);
    unsafe { boxed(&mut ctx, 1, 0, width(IntTy::U64)) };
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("no 128-bit width"), "{said}");
}

/// `args` lowered as the list of a builtin's arguments, the builtin at `index` called over them.
fn called_encoded(c: *mut Ctx, index: Word, args: impl FnOnce(*mut Ctx)) {
    unsafe {
        native::<N1>("?lower_open")(c, 0);
        args(c);
        helper::<N1>("rt_builtin_encoded")(c, index);
    }
}

/// A builtin called over its arguments' encoding answers a lifting of its answer's, and what the
/// bytes cannot hold crosses beside them both ways.
#[test]
fn a_builtin_called_encoded_answers_a_lifting_of_its_answer() {
    let mut ctx = context_calling(&["checked_neg", "Secret::verify", "Secret::of_string"]);
    let c: *mut Ctx = &mut ctx;
    unsafe {
        called_encoded(c, 0, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 1);
            native::<N1>("?out_int")(c, 5);
        });
        assert_eq!(native::<A1>("?in_count")(c, tag::CTOR as Word), 1);
        assert_eq!(
            native::<A2>("?in_name_is")(c, b"Some".as_ptr() as Word, 4),
            1
        );
        assert_eq!(native::<A0>("?in_int")(c), -5i64 as Word);
        native::<N0>("?lift_close")(c);
        called_encoded(c, 2, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 1);
            text(c, "?out_str", b"hunter2");
        });
        let secret = native::<A0>("?in_handle")(c);
        native::<N0>("?lift_close")(c);
        called_encoded(c, 1, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 2);
            native::<N1>("?out_handle")(c, secret);
            text(c, "?out_str", b"hunter3");
        });
        assert_eq!(native::<A0>("?in_tag")(c), tag::FALSE as Word);
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut ctx);
}

/// A runtime object is read as it is written into a lowering, so the code that wrote it may let it
/// go before the builtin the lowering is read for runs.
#[test]
fn a_runtime_object_written_into_a_lowering_outlives_the_count_that_wrote_it() {
    let mut ctx = context_calling(&["Secret::verify", "Secret::of_string"]);
    let c: *mut Ctx = &mut ctx;
    unsafe {
        called_encoded(c, 1, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 1);
            text(c, "?out_str", b"hunter2");
        });
        let secret = native::<A0>("?in_handle")(c);
        native::<N0>("?lift_close")(c);
        called_encoded(c, 0, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 2);
            native::<N1>("?out_handle")(c, secret);
            helper::<N1>("rt_handle_free")(c, secret);
            text(c, "?out_str", b"hunter3");
        });
        assert_eq!(native::<A0>("?in_tag")(c), tag::FALSE as Word);
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut ctx);
}

/// A hold written into a lowering reads as a hold, whatever it holds, which no crossing reads.
#[test]
fn a_hold_written_into_a_lowering_reads_as_a_hold() {
    let mut ctx = context_calling(&["reflect"]);
    let c: *mut Ctx = &mut ctx;
    let name = b"std.value.VHold";
    unsafe {
        let hold = native::<A1>("?hold_made")(c, imm(42));
        called_encoded(c, 0, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 1);
            native::<N1>("?out_handle")(c, hold);
        });
        assert_eq!(native::<A1>("?in_count")(c, tag::CTOR as Word), 0);
        assert_eq!(
            native::<A2>("?in_name_is")(c, name.as_ptr() as Word, name.len() as Word),
            1
        );
        native::<N0>("?lift_close")(c);
        helper::<N1>("rt_handle_free")(c, hold);
    }
    unfailed(&mut ctx);
}

/// A builtin that raises ends the lowering of its arguments and opens nothing, so the crossing it
/// was called inside goes on where it was.
#[test]
fn a_builtin_called_encoded_that_raises_opens_nothing() {
    let mut ctx = context_calling(&["panic"]);
    let outer = lowered(&mut ctx, false, |c| unsafe {
        native::<N2>("?out_count")(c, tag::LIST as Word, 2);
        native::<N1>("?out_int")(c, 1);
        called_encoded(c, 0, |c| {
            native::<N2>("?out_count")(c, tag::LIST as Word, 1);
            text(c, "?out_str", b"no such file");
        });
        native::<N1>("?out_int")(c, 2);
    });
    let said = ctx.take_failure().expect("the builtin raised").message;
    assert!(said.contains("no such file"), "{said}");
    assert_eq!(
        decode(&outer).unwrap(),
        Value::list(vec![Value::Int(1), Value::Int(2)])
    );
}

/// A builtin is called over a lowering of its arguments and nothing else.
#[test]
fn a_builtin_called_encoded_with_no_lowering_or_outside_its_pool_fails_the_context() {
    let mut ctx = context_calling(&["checked_neg"]);
    unsafe { helper::<N1>("rt_builtin_encoded")(&mut ctx, 0) };
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("no lowering open"), "{said}");
    let mut ctx = context_calling(&["digest"]);
    called_encoded(&mut ctx, 1, |c| unsafe {
        native::<N2>("?out_count")(c, tag::LIST as Word, 0);
    });
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("builtin 1 of a pool of 1"), "{said}");
}

/// A crossing begun inside another works on a frame of its own, and the outer one goes on where it
/// was.
#[test]
fn a_crossing_inside_a_crossing_leaves_the_outer_as_it_was() {
    let mut ctx = context();
    let inner_bytes = encode(&Value::Int(7)).unwrap();
    let mut written_inside = Vec::new();
    let outer = lowered(&mut ctx, false, |c| unsafe {
        native::<N2>("?out_count")(c, tag::LIST as Word, 3);
        native::<N1>("?out_int")(c, 1);
        let mut inner = context();
        written_inside = lowered(&mut inner, false, |i| native::<N1>("?out_int")(i, 0));
        // On the same context: a lowering, then a lifting, each inside the outer lowering.
        native::<N1>("?lower_open")(c, 0);
        native::<N1>("?out_int")(c, 2);
        let n = native::<A0>("?lower_close")(c);
        let mut b = vec![0u8; n as usize];
        native::<N2>("?taken")(c, b.as_mut_ptr() as Word, n);
        assert_eq!(decode(&b).unwrap(), Value::Int(2));
        native::<N2>("?lift_open")(c, inner_bytes.as_ptr() as Word, inner_bytes.len() as Word);
        let seven = native::<A0>("?in_int")(c);
        native::<N0>("?lift_close")(c);
        native::<N1>("?out_int")(c, seven);
        native::<N1>("?out_int")(c, 3);
    });
    unfailed(&mut ctx);
    assert_eq!(
        decode(&outer).unwrap(),
        Value::list(vec![Value::Int(1), Value::Int(7), Value::Int(3)])
    );
    assert_eq!(decode(&written_inside).unwrap(), Value::Int(0));
}

#[test]
fn a_value_written_with_no_lowering_open_fails_the_context() {
    let mut ctx = context();
    unsafe { native::<N1>("?out_int")(&mut ctx, 1) };
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("no lowering open"), "{said}");
}

/// What a carried value is: a pair whose second is carried again, each lowered through its own
/// type, the inner in a lowering of its own inside the outer's.
static INNER: PlyType = PlyType {
    identity: 2,
    spelled: b"m.Inner".as_ptr(),
    spelled_len: 7,
    inc: unread,
    dec: unread,
    lower: lower_inner,
    lift: nothing,
};

static PAIR: PlyType = PlyType {
    identity: 3,
    spelled: b"m.Pair".as_ptr(),
    spelled_len: 6,
    inc: unread,
    dec: unread,
    lower: lower_pair,
    lift: nothing,
};

unsafe extern "C" fn unread(_ctx: *mut Ctx, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_ctx: *mut Ctx) -> Word {
    0
}

/// The inner value's word is the number it holds.
unsafe extern "C" fn lower_inner(ctx: *mut Ctx, w: Word) -> Word {
    unsafe {
        native::<N3>("?out_ctor")(ctx, b"m.Inner".as_ptr() as Word, 7, 1);
        native::<N1>("?out_int")(ctx, w);
    }
    0
}

/// A pair's word is its first number; its second is the inner value one more.
unsafe extern "C" fn lower_pair(ctx: *mut Ctx, w: Word) -> Word {
    unsafe {
        native::<N2>("?out_count")(ctx, tag::RECORD as Word, 2);
        text(ctx, "?out_name", b"first");
        native::<N1>("?out_int")(ctx, w);
        text(ctx, "?out_name", b"second");
        native::<N2>("?out_carried")(ctx, w + 1, &raw const INNER as Word);
    }
    0
}

#[test]
fn a_carried_value_lowers_through_its_type_and_what_it_carries_through_theirs() {
    let mut ctx = context();
    let pair = unsafe { Carried::new(Handed::of(40), &PAIR) };
    let v = pair.lower(&mut ctx).expect("it lowers");
    assert_eq!(
        v,
        record(vec![
            ("first", Value::Int(40)),
            ("second", Value::ctor("m.Inner", vec![Value::Int(41)])),
        ])
    );
    assert_eq!(ctx.failed, 0);
    pair.release(&mut ctx);
}

/// A runtime object crosses as itself.
#[test]
fn a_bridge_lowered_as_a_handle_reads_as_what_it_bridges() {
    static BRIDGED: PlyType = PlyType {
        identity: 4,
        spelled: b"Secret<String>".as_ptr(),
        spelled_len: 14,
        inc: unread,
        dec: unread,
        lower: lower_handle,
        lift: nothing,
    };
    unsafe extern "C" fn lower_handle(ctx: *mut Ctx, w: Word) -> Word {
        unsafe { native::<N1>("?out_handle")(ctx, w) };
        0
    }
    let mut ctx = context();
    let b = ctx.heap.bridge(Value::secret_text("hunter2"));
    let held = unsafe { Carried::new(Handed::of(b), &BRIDGED) };
    let v = held.lower(&mut ctx).expect("it lowers");
    assert!(matches!(v, Value::Secret(_)), "{v:?}");
    held.release(&mut ctx);
}

#[test]
fn a_function_the_program_wrote_crosses_by_name_which_only_its_own_run_reads_back() {
    let mut ctx = context();
    let name = b"m.double";
    let named = lowered(&mut ctx, false, |c| unsafe {
        native::<N3>("?out_fun")(c, 0, name.as_ptr() as Word, name.len() as Word)
    });
    let refused = decode(&named).unwrap_err();
    assert!(refused.contains("only its own run"), "{refused}");
}

/// A value of `m.Box`, a block of one word, counted at its offset 0 and freed by size.
const BOX_BYTES: Word = (HEADER + 8) as Word;

unsafe extern "C" fn box_inc(_ctx: *mut Ctx, w: Word) -> Word {
    unsafe { *(w as *mut u32) += 1 };
    0
}

unsafe extern "C" fn box_dec(ctx: *mut Ctx, w: Word) -> Word {
    unsafe {
        if *(w as *const u32) > 1 {
            *(w as *mut u32) -= 1;
        } else {
            helper::<N2>("rt_free")(ctx, w, BOX_BYTES);
        }
    }
    0
}

static BOX: PlyType = PlyType {
    identity: 5,
    spelled: b"m.Box".as_ptr(),
    spelled_len: 5,
    inc: box_inc,
    dec: box_dec,
    lower: unread,
    lift: nothing,
};

/// A lifting of a list of `handles`, each a position beside the bytes, with a runtime object at
/// position 0 and a box carried with its type at position 1; answers the box.
fn lifting_beside(ctx: &mut Ctx, handles: &[u64]) -> Word {
    let mut w = ply_core::codec::Writer::new();
    w.count(tag::LIST, handles.len() as u64);
    for at in handles {
        w.handle(*at);
    }
    let b = unsafe { helper::<A1>("rt_alloc_bytes")(ctx, BOX_BYTES) };
    let bridge = ctx.heap.bridge(Value::secret_text("hunter2"));
    let carried = unsafe { Carried::new(Handed::of(b), &BOX) };
    ctx.lift_from(
        w.finish(),
        vec![Lifted::Runtime(bridge), Lifted::Carried(carried)],
    );
    b
}

/// A value the runtime kept with its type is handed to compiled code with the count the lifting
/// held; what compiled code leaves of the lifting is let go through its type when it ends.
#[test]
fn a_carried_value_lifted_moves_its_count_to_compiled_code_and_one_left_is_let_go() {
    let mut ctx = context();
    ctx.begin(1_000);
    let c: *mut Ctx = &mut ctx;
    let b = lifting_beside(&mut ctx, &[1]);
    unsafe {
        assert_eq!(native::<A1>("?in_count")(c, tag::LIST as Word), 1);
        assert_eq!(native::<A0>("?in_carried")(c), b);
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut ctx);
    assert_eq!((ctx.heap.live(), unsafe { *(b as *const u32) }), (1, 1));
    unsafe { box_dec(&mut ctx, b) };
    assert_eq!(ctx.heap.live(), 0);
    lifting_beside(&mut ctx, &[]);
    unsafe {
        assert_eq!(native::<A1>("?in_count")(c, tag::LIST as Word), 0);
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut ctx);
    assert_eq!(
        ctx.heap.live(),
        0,
        "the box left beside the bytes was not let go"
    );
    assert_eq!(ctx.heap.bridges(), 0);
    ctx.end();
}

/// A runtime object and a value kept with its type are read by their own natives alone; one read
/// as the other is left beside the bytes, and let go with them.
#[test]
fn a_handle_read_as_the_other_kind_fails_the_crossing_and_is_let_go_with_it() {
    for (at, read, said) in [
        (
            1,
            "?in_handle",
            "a value kept with its type, read as a runtime object",
        ),
        (
            0,
            "?in_carried",
            "a runtime object, read as a value kept with its type",
        ),
    ] {
        let mut ctx = context();
        ctx.begin(1_000);
        let c: *mut Ctx = &mut ctx;
        lifting_beside(&mut ctx, &[at]);
        unsafe {
            native::<A1>("?in_count")(c, tag::LIST as Word);
            assert_eq!(native::<A0>(read)(c), 0);
            native::<N0>("?lift_close")(c);
        }
        let failure = ctx.take_failure().expect("the crossing failed").message;
        assert!(failure.contains(said), "{failure}");
        assert_eq!((ctx.heap.live(), ctx.heap.bridges()), (0, 0));
        ctx.failed = 0;
        ctx.end();
    }
}

/// The taken bytes are copied out whole or not at all.
#[test]
fn taken_bytes_are_copied_out_at_their_count_alone() {
    let mut ctx = context();
    let c: *mut Ctx = &mut ctx;
    let mut out = [0u8; 4];
    unsafe {
        native::<N1>("?lower_open")(c, 0);
        native::<N1>("?out_int")(c, 1);
        let n = native::<A0>("?lower_close")(c);
        assert_eq!(n, 6);
        native::<N2>("?taken")(c, out.as_mut_ptr() as Word, 4);
    }
    let said = ctx.take_failure().expect("a diagnostic").message;
    assert!(said.contains("4 bytes taken, of 6 given"), "{said}");
}

#[test]
fn handed_words_are_the_words_compiled_code_passes() {
    assert_eq!(
        unsafe { std::mem::transmute::<Handed, Word>(<Handed as Answered>::of(5)) },
        5
    );
}

/// `write` over a fresh writer, as an entry's answer is written.
fn written(write: impl FnOnce(&mut ply_core::codec::Writer)) -> Vec<u8> {
    let mut w = ply_core::codec::Writer::new();
    write(&mut w);
    w.finish()
}

fn resumes(w: &mut ply_core::codec::Writer) {
    w.fun(ply_core::codec::fun::RESUMES);
}

#[test]
fn data_holding_no_continuation_has_no_way_to_one() {
    let bytes = written(|w| {
        w.count(tag::LIST, 4);
        w.int(1);
        w.str(b"two");
        w.count(tag::MAP, 0);
        w.handle(0);
    });
    assert_eq!(ply_compiled::rt::continuation_in(&bytes), Ok(None));
}

#[test]
fn the_way_to_a_continuation_reads_outermost_first() {
    let bytes = written(|w| {
        w.count(tag::LIST, 2);
        w.int(0);
        w.count(tag::RECORD, 1);
        w.name(b"saved");
        w.ctor(b"m.Just", 1);
        resumes(w);
    });
    assert_eq!(
        ply_compiled::rt::continuation_in(&bytes),
        Ok(Some(vec![
            "item 1".to_string(),
            "field `saved`".to_string(),
            "`m.Just`'s argument 1".to_string()
        ]))
    );
    assert_eq!(
        ply_compiled::rt::continuation_in(&written(resumes)),
        Ok(Some(Vec::new()))
    );
}

#[test]
fn a_continuation_is_found_past_every_kind_of_part_before_it() {
    let bytes = written(|w| {
        w.count(tag::ARRAY, 1);
        w.count(tag::MAP, 2);
        w.float(0);
        w.char(u32::from('c'));
        w.decimal([0; 16]);
        resumes(w);
    });
    assert_eq!(
        ply_compiled::rt::continuation_in(&bytes),
        Ok(Some(vec![
            "element 0".to_string(),
            "a map entry".to_string()
        ]))
    );
    let table = written(|w| {
        w.fun(ply_core::codec::fun::TABLE);
        w.varint(1);
        w.varint(1);
        w.int(1);
        resumes(w);
        w.int(0);
    });
    assert_eq!(
        ply_compiled::rt::continuation_in(&table),
        Ok(Some(Vec::new()))
    );
}
