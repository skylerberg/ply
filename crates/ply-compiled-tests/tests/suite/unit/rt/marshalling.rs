use crate::bare::{helper, new_context};
use crate::unit::secret::sealed;
use ply_compiled::c::builtin_helper_name;
use ply_compiled::emitted::{Handed, PlyType};
use ply_compiled::heap::{Word, imm};
use ply_compiled::native::Returned;
use ply_compiled::rt::Context;
use ply_core::codec::{Writer, fun, shown, tag};
use ply_core::float::bits;
use ply_core::{Decimal, Fixed, INT_TYPES, IntegerType};

type N0 = unsafe extern "C" fn(*mut Context);
type A0 = unsafe extern "C" fn(*mut Context) -> Word;
type N1 = unsafe extern "C" fn(*mut Context, Word);
type A1 = unsafe extern "C" fn(*mut Context, Word) -> Word;
type N2 = unsafe extern "C" fn(*mut Context, Word, Word);
type A2 = unsafe extern "C" fn(*mut Context, Word, Word) -> Word;
type N3 = unsafe extern "C" fn(*mut Context, Word, Word, Word);

fn native<F: Copy>(name: &str) -> F {
    helper(&builtin_helper_name(name))
}

fn text(context: *mut Context, native_name: &str, s: &[u8]) {
    unsafe { native::<N2>(native_name)(context, s.as_ptr() as Word, s.len() as Word) }
}

fn lowered(context: &mut Context, write: impl FnOnce(*mut Context)) -> Vec<u8> {
    let context: *mut Context = context;
    unsafe {
        native::<N0>("?shown_open")(context);
        write(context);
        let n = native::<A0>("?lower_close")(context);
        let mut out = vec![0u8; n as usize];
        native::<N2>("?taken")(context, out.as_mut_ptr() as Word, n);
        out
    }
}

fn lift_open(context: *mut Context, bytes: &[u8]) {
    unsafe { &mut *context }.open_decoding(bytes.to_vec());
}

fn written(write: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    write(&mut w);
    w.finish()
}

fn width(ty: IntegerType) -> Word {
    INT_TYPES.iter().position(|t| *t == ty).expect("a width") as Word
}

fn decimal_words(d: Decimal) -> (Word, Word) {
    let b = d.serialize();
    (
        i64::from_le_bytes(b[..8].try_into().unwrap()),
        i64::from_le_bytes(b[8..].try_into().unwrap()),
    )
}

#[test]
fn the_native_output_functions_write_the_same_bytes_as_the_encoder_for_every_kind_of_value() {
    let mut context = new_context();
    let d = Decimal::new(-12345, 3);
    let bytes = lowered(&mut context, |context| unsafe {
        let (lo, hi) = decimal_words(d);
        native::<N2>("?out_count")(context, tag::RECORD as Word, 8);
        text(context, "?out_name", b"a_decimal");
        native::<N2>("?out_decimal")(context, lo, hi);
        text(context, "?out_name", b"b_narrow");
        native::<N3>("?out_fixed")(context, width(IntegerType::I8), 0xff, 0);
        text(context, "?out_name", b"c_wide");
        native::<N3>("?out_fixed")(context, width(IntegerType::U128), -1, 7);
        text(context, "?out_name", b"d_int");
        native::<N1>("?out_int")(context, -5);
        text(context, "?out_name", b"e_text");
        text(context, "?out_str", "né".as_bytes());
        text(context, "?out_name", b"f_some");
        native::<N3>("?out_ctor")(context, b"Some".as_ptr() as Word, 4, 1);
        native::<N1>("?out_tag")(context, tag::UNIT as Word);
        text(context, "?out_name", b"g_flag");
        native::<N1>("?out_tag")(context, tag::TRUE as Word);
        text(context, "?out_name", b"h_list");
        native::<N2>("?out_count")(context, tag::LIST as Word, 3);
        native::<N1>("?out_float")(context, ply_core::float::bits(1.5) as Word);
        native::<N1>("?out_char")(context, 'c' as Word);
        text(context, "?out_bytes", b"\x00\xff");
    });
    assert_eq!(context.failed, 0);
    let wide = (7u128 << 64) | u128::from(u64::MAX);
    let expected = written(|w| {
        w.count(tag::RECORD, 8);
        w.name(b"a_decimal");
        w.decimal(d.serialize());
        w.name(b"b_narrow");
        w.fixed(Fixed::new(IntegerType::I8, -1i128 as u128));
        w.name(b"c_wide");
        w.fixed(Fixed::new(IntegerType::U128, wide));
        w.name(b"d_int");
        w.int(-5);
        w.name(b"e_text");
        w.str("né".as_bytes());
        w.name(b"f_some");
        w.ctor(b"Some", 1);
        w.tag(tag::UNIT);
        w.name(b"g_flag");
        w.tag(tag::TRUE);
        w.name(b"h_list");
        w.count(tag::LIST, 3);
        w.float(bits(1.5));
        w.char(u32::from('c'));
        w.bytes(b"\x00\xff");
    });
    assert_eq!(bytes, expected);
}

fn block(rc: u32) -> Box<[u64; 2]> {
    Box::new([u64::from(rc), 0])
}

#[test]
fn a_value_that_appears_twice_is_serialized_completely_each_time_and_is_not_shared() {
    let mut context = new_context();
    let (a, b) = (block(1), block(1));
    let (shared, inner) = (a.as_ptr() as Word, b.as_ptr() as Word);
    let write = |context: *mut Context| unsafe {
        let again: A1 = native("?out_again");
        native::<N2>("?out_count")(context, tag::LIST as Word, 3);
        for _ in 0..2 {
            if again(context, shared) == 0 {
                native::<N2>("?out_count")(context, tag::LIST as Word, 1);
                if again(context, inner) == 0 {
                    text(context, "?out_str", b"a string long enough to share");
                }
            }
        }
        if again(context, inner) == 0 {
            text(context, "?out_str", b"a string long enough to share");
        }
    };
    let with = lowered(&mut context, write);
    assert_eq!(context.failed, 0);
    let s = b"a string long enough to share";
    let whole = written(|w| {
        w.count(tag::LIST, 3);
        for _ in 0..2 {
            w.count(tag::LIST, 1);
            w.str(s);
        }
        w.str(s);
    });
    let shared = written(|w| {
        w.count(tag::LIST, 3);
        w.count(tag::LIST, 1);
        w.str(s);
        w.again(1);
        w.again(2);
    });
    assert_eq!(with, shared);
    assert!(
        with.len() + 50 < whole.len(),
        "{} bytes shared, {} not",
        with.len(),
        whole.len()
    );
}

#[test]
fn an_immortal_value_that_appears_twice_is_serialized_completely_each_time() {
    let mut context = new_context();
    let s = block(ply_compiled::heap::IMMORTAL);
    let w = s.as_ptr() as Word;
    let bytes = lowered(&mut context, |context| unsafe {
        let again: A1 = native("?out_again");
        native::<N2>("?out_count")(context, tag::LIST as Word, 2);
        assert_eq!(again(context, w), 0);
        text(context, "?out_str", b"a");
        assert_eq!(again(context, w), 0);
        text(context, "?out_bytes", b"a");
    });
    assert_eq!(context.failed, 0);
    let whole = written(|w| {
        w.count(tag::LIST, 2);
        w.str(b"a");
        w.bytes(b"a");
    });
    assert_eq!(bytes, whole);
}

#[test]
fn the_native_input_functions_read_back_every_kind_of_value_the_encoder_writes() {
    let mut context = new_context();
    let d = Decimal::new(271828, 5);
    let bytes = written(|w| {
        w.count(tag::LIST, 5);
        w.ctor(b"m.Pair", 2);
        w.int(-9);
        w.str(b"ok");
        w.count(tag::RECORD, 2);
        w.name(b"x");
        w.float(bits(-0.25));
        w.name(b"y");
        w.char(u32::from('é'));
        w.decimal(d.serialize());
        w.fixed(Fixed::new(IntegerType::I16, -2i128 as u128));
        w.tag(tag::FALSE);
    });
    let c: *mut Context = &mut context;
    lift_open(c, &bytes);
    unsafe {
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
        let lo = native::<A1>("?in_fixed")(c, width(IntegerType::I16));
        assert_eq!((lo, high(c)), (-2, -1), "sign-extended to 128 bits");
        assert_eq!(native::<A0>("?in_tag")(c), tag::FALSE as Word);
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut context);
}

#[test]
fn helpers_that_reject_a_type_descriptor_make_the_current_entry_fail_with_a_reason() {
    for (rejection, said) in [
        ("rt_never_lifted", "lifts only a `String`"),
        ("rt_never_shown", "does not lower it"),
    ] {
        let mut context = new_context();
        context.begin();
        let rejected: unsafe extern "C" fn(*mut Context) = helper(rejection);
        unsafe { rejected(&mut context) };
        assert_ne!(context.failed, 0, "{rejection} rejected nothing");
        let reason = format!("{:?}", context.take_failure());
        assert!(reason.contains(said), "{reason}");
        context.end();
    }
}

pub fn unfailed(context: &mut Context) {
    let failed = context.failed;
    let reason = context.take_failure();
    assert_eq!(failed, 0, "{reason:?}");
}

fn lift_and(bytes: &[u8], read: impl FnOnce(*mut Context)) -> String {
    let mut context = new_context();
    let c: *mut Context = &mut context;
    lift_open(c, bytes);
    unsafe {
        read(c);
        native::<N0>("?lift_close")(c);
    }
    assert_ne!(context.failed, 0, "the lifting read on");
    context.take_failure().expect("a diagnostic").message
}

#[test]
fn reading_bytes_that_do_not_match_the_expected_type_fails_with_a_diagnostic() {
    let int = written(|w| w.int(3));
    let said = lift_and(&int, |c| unsafe {
        native::<A0>("?in_float")(c);
    });
    assert!(said.contains("tagged 3 where one tagged 4"), "{said}");
    let said = lift_and(&int, |_| {});
    assert!(said.contains("bytes follow"), "{said}");
    let rec = written(|w| {
        w.count(tag::RECORD, 1);
        w.name(b"x");
        w.int(1);
    });
    let said = lift_and(&rec, |c| unsafe {
        native::<A1>("?in_count")(c, tag::RECORD as Word);
        native::<N2>("?in_field")(c, b"y".as_ptr() as Word, 1);
        native::<A0>("?in_int")(c);
    });
    assert!(said.contains("field `x` where `y` was read"), "{said}");
    let said = lift_and(b"not PLV1", |_| {});
    assert!(said.contains("not an encoded value"), "{said}");
}

#[test]
fn a_128_bit_value_is_read_as_a_low_word_and_its_high_word_is_read_right_after() {
    let mut context = new_context();
    let wide = (5u128 << 64) | 9;
    let bytes = written(|w| {
        w.count(tag::LIST, 4);
        w.fixed(Fixed::new(IntegerType::U128, wide));
        w.fixed(Fixed::new(IntegerType::I128, -3i128 as u128));
        w.fixed(Fixed::new(IntegerType::U8, 255));
        w.fixed(Fixed::new(IntegerType::U64, u128::from(u64::MAX)));
    });
    let c: *mut Context = &mut context;
    lift_open(c, &bytes);
    unsafe {
        assert_eq!(native::<A1>("?in_count")(c, tag::LIST as Word), 4);
        let (fixed, high): (A1, A0) = (native("?in_fixed"), native("?in_high"));
        assert_eq!((fixed(c, width(IntegerType::U128)), high(c)), (9, 5));
        assert_eq!((fixed(c, width(IntegerType::I128)), high(c)), (-3, -1));
        assert_eq!((fixed(c, width(IntegerType::U8)), high(c)), (255, 0));
        assert_eq!(
            (fixed(c, width(IntegerType::U64)), high(c)),
            (-1, 0),
            "zero-extended past its 64 bits"
        );
        native::<N0>("?lift_close")(c);
    }
    unfailed(&mut context);
    let one = written(|w| w.fixed(Fixed::new(IntegerType::U128, wide)));
    let said = lift_and(&one, |c| unsafe {
        native::<A1>("?in_fixed")(c, width(IntegerType::U128));
        native::<A0>("?in_high")(c);
        native::<A0>("?in_high")(c);
    });
    assert!(said.contains("no wide value was read"), "{said}");
}

#[test]
fn encoding_or_decoding_a_value_in_the_middle_of_another_encoding_leaves_the_outer_encoding_unchanged()
 {
    let mut context = new_context();
    let inner_bytes = written(|w| w.int(7));
    let mut written_inside = Vec::new();
    let outer = lowered(&mut context, |c| unsafe {
        native::<N2>("?out_count")(c, tag::LIST as Word, 3);
        native::<N1>("?out_int")(c, 1);
        let mut inner = new_context();
        written_inside = lowered(&mut inner, |i| native::<N1>("?out_int")(i, 0));
        native::<N0>("?shown_open")(c);
        native::<N1>("?out_int")(c, 2);
        let n = native::<A0>("?lower_close")(c);
        let mut b = vec![0u8; n as usize];
        native::<N2>("?taken")(c, b.as_mut_ptr() as Word, n);
        assert_eq!(b, written(|w| w.int(2)));
        lift_open(c, &inner_bytes);
        let seven = native::<A0>("?in_int")(c);
        native::<N0>("?lift_close")(c);
        native::<N1>("?out_int")(c, seven);
        native::<N1>("?out_int")(c, 3);
    });
    unfailed(&mut context);
    let whole = written(|w| {
        w.count(tag::LIST, 3);
        w.int(1);
        w.int(7);
        w.int(3);
    });
    assert_eq!(outer, whole);
    assert_eq!(written_inside, written(|w| w.int(0)));
}

#[test]
fn writing_a_value_with_no_encoding_started_fails_with_a_diagnostic() {
    let mut context = new_context();
    unsafe { native::<N1>("?out_int")(&mut context, 1) };
    let said = context.take_failure().expect("a diagnostic").message;
    assert!(said.contains("no lowering open"), "{said}");
}

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

unsafe extern "C" fn unread(_context: *mut Context, _w: Word) -> Word {
    0
}

unsafe extern "C" fn nothing(_context: *mut Context) -> Word {
    0
}

unsafe extern "C" fn lower_inner(context: *mut Context, w: Word) -> Word {
    unsafe {
        native::<N3>("?out_ctor")(context, b"m.Inner".as_ptr() as Word, 7, 1);
        native::<N1>("?out_int")(context, w);
    }
    0
}

unsafe extern "C" fn lower_pair(context: *mut Context, w: Word) -> Word {
    unsafe {
        native::<N2>("?out_count")(context, tag::RECORD as Word, 2);
        text(context, "?out_name", b"first");
        native::<N1>("?out_int")(context, w);
        text(context, "?out_name", b"second");
        native::<N2>("?out_carried")(context, w + 1, &raw const INNER as Word);
    }
    0
}

#[test]
fn the_encoding_of_a_value_for_a_diagnostic_never_contains_the_bytes_of_a_secret() {
    let mut context = new_context();
    let secret = sealed(&mut context, b"hunter2", true);
    let c: *mut Context = &mut context;
    let bytes = unsafe {
        let scoped = native::<A1>("?hold_made")(c, imm(42));
        native::<N0>("?shown_open")(c);
        native::<N2>("?out_count")(c, tag::LIST as Word, 3);
        native::<N1>("?out_handle")(c, secret);
        native::<N1>("?out_handle")(c, scoped);
        native::<N2>("?out_carried")(c, 40, &raw const PAIR as Word);
        let n = native::<A0>("?lower_close")(c);
        let mut out = vec![0u8; n as usize];
        native::<N2>("?taken")(c, out.as_mut_ptr() as Word, n);
        helper::<N1>("rt_handle_free")(c, scoped);
        out
    };
    unfailed(&mut context);
    let mut w = Writer::new();
    w.count(tag::LIST, 3);
    w.handle(shown::SEALED);
    w.handle(shown::SCOPED);
    w.count(tag::RECORD, 2);
    w.name(b"first");
    w.int(40);
    w.name(b"second");
    w.ctor(b"m.Inner", 1);
    w.int(41);
    assert_eq!(bytes, w.finish());
    assert!(!bytes.windows(7).any(|b| b == b"hunter2"));
}

#[test]
fn a_failure_shows_its_values_in_order_and_describes_each_by_its_kind() {
    type Failed = unsafe extern "C" fn(*mut Context, Word, *const Word, Word, Word);
    let mut context = new_context();
    let actual = written(|w| w.int(2));
    let expected = written(|w| w.str(b"x"));
    let message = format!(
        "expected {}, found {}",
        ply_core::slot(0),
        ply_core::slot(1)
    );
    let texts: [&[u8]; 5] = [
        b"E0501",
        message.as_bytes(),
        b"here",
        expected.as_slice(),
        actual.as_slice(),
    ];
    let words: Vec<Word> = texts
        .iter()
        .flat_map(|t| [t.as_ptr() as Word, t.len() as Word])
        .collect();
    unsafe { helper::<Failed>("rt_failed")(&mut context, 1, words.as_ptr(), 0, 2) };
    let d = context.take_failure().expect("the failure raised");
    assert_eq!(d.code, ply_core::codes::ASSERTION_FAILED);
    assert_eq!(d.values.iter().collect::<Vec<_>>(), [&expected, &actual]);
    assert_eq!(
        d.described(&d.message),
        "expected a `String`, found an `Int`"
    );
}

#[test]
fn a_function_written_by_the_program_is_encoded_as_its_name() {
    let mut context = new_context();
    let name = b"m.double";
    let named = lowered(&mut context, |c| unsafe {
        native::<N3>("?out_fun")(c, 0, name.as_ptr() as Word, name.len() as Word)
    });
    unfailed(&mut context);
    let by_name = written(|w| {
        w.fun(fun::NAMED);
        w.name(name);
    });
    assert_eq!(named, by_name);
}

#[test]
fn copying_out_fewer_bytes_than_were_encoded_fails_with_a_diagnostic() {
    let mut context = new_context();
    let c: *mut Context = &mut context;
    let mut out = [0u8; 4];
    unsafe {
        native::<N0>("?shown_open")(c);
        native::<N1>("?out_int")(c, 1);
        let n = native::<A0>("?lower_close")(c);
        assert_eq!(n, 6);
        native::<N2>("?taken")(c, out.as_mut_ptr() as Word, 4);
    }
    let said = context.take_failure().expect("a diagnostic").message;
    assert!(said.contains("4 bytes taken, of 6 given"), "{said}");
}

#[test]
fn the_handed_type_returned_from_helpers_is_a_plain_machine_word_like_compiled_code_uses() {
    assert_eq!(
        unsafe { std::mem::transmute::<Handed, Word>(<Handed as Returned>::of(5)) },
        5
    );
}

fn resumes(w: &mut Writer) {
    w.fun(fun::RESUMPTION);
}

#[test]
fn continuation_in_returns_no_continuation_for_encoded_data_that_has_none() {
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
fn continuation_in_returns_the_location_of_a_continuation_as_steps_listed_from_the_outside_in() {
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
fn continuation_in_finds_a_continuation_after_arrays_maps_floats_chars_decimals_and_function_tables()
 {
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
        w.fun(fun::TABLE);
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
