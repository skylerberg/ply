use crate::unit::build::{
    bin, block, callv, clause, discard, effect_def, handle, int, lam, letv, list, perform,
    standalone, var, with_cell,
};
use ply_eval::builtins::*;
use ply_eval::evaluator::Machine;
use ply_eval::task_regions::TaskRegions;
use ply_eval::{Frame, Value};
use ply_span::{Diagnostic, Span, codes};
use ply_syntax::ast::{BinOp, Expr, Item, Mode};

fn ints(xs: &[i64]) -> Value {
    Value::list(xs.iter().copied().map(Value::Int).collect())
}

fn f() -> Value {
    Value::builtin(Builtin::IntToString)
}

/// Drives the protocol the way an engine does, answering every callback.
fn drive(
    b: Builtin,
    args: Vec<Value>,
    mut answer: impl FnMut(&[Value]) -> Value,
) -> Result<Value, Diagnostic> {
    let mut cells: TaskRegions = TaskRegions::new();
    let mut step = call(b, args, cells.arena_mut(), Span::DUMMY)?;
    loop {
        match step {
            Step::Done(v) => return Ok(v),
            Step::Apply { args, frame, .. } => {
                let v = answer(&args);
                step = advance(frame, v)?;
            }
        }
    }
}

/// A builtin that cannot suspend, called the way an engine calls it.
fn done(b: Builtin, args: Vec<Value>) -> Result<Value, Diagnostic> {
    let mut cells: TaskRegions = TaskRegions::new();
    match call(b, args, cells.arena_mut(), Span::DUMMY)? {
        Step::Done(v) => Ok(v),
        Step::Apply { .. } => panic!("`{}` suspended", b.name()),
    }
}

fn bytes(b: &[u8]) -> Value {
    Value::bytes(b)
}

/// `Some(i)` or `None`, rendered, which is what a Ply program sees.
fn found(b: Builtin, args: Vec<Value>) -> String {
    done(b, args).unwrap().render()
}

fn some(i: i64) -> String {
    format!("Some({i})")
}

/// `Some(i)` as `i` and `None` as `-1`, which is the shape W1's folds answered in and therefore
/// the shape a comparison against them needs.
fn at(v: &Value) -> i64 {
    match v {
        Value::Ctor { args, .. } if !args.is_empty() => {
            args[0].as_int(Span::DUMMY, "test").unwrap()
        }
        _ => -1,
    }
}

/// Deterministic and dependency-free, so a failing case is a seed a reader can reproduce rather
/// than a number that moves between runs.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

#[test]
fn index_of_covers_empty_absent_at_the_start_at_the_end_and_overlapping() {
    let hay = bytes(b"aaabaaab");
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"")]),
        some(0)
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![bytes(b""), bytes(b"")]),
        some(0)
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![bytes(b""), bytes(b"a")]),
        "None"
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"z")]),
        "None"
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOf,
            vec![hay.clone(), bytes(b"aaabaaabx")]
        ),
        "None",
        "a needle longer than the haystack cannot occur"
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"aaa")]),
        some(0)
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"aab")]),
        some(1)
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"b")]),
        some(3)
    );

    // Overlapping occurrences: `aa` sits at 0, 1, 4 and 5, and the first is the answer.
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"aa")]),
        some(0)
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOfFrom,
            vec![hay.clone(), bytes(b"aa"), Value::Int(1)]
        ),
        some(1)
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOfFrom,
            vec![hay.clone(), bytes(b"aa"), Value::Int(2)]
        ),
        some(4)
    );
}

/// The index a `_from` search answers is absolute, so it feeds straight back into
/// `bytes_slice`.
#[test]
fn index_of_from_answers_an_absolute_index_and_admits_the_end() {
    let hay = bytes(b"GET / HTTP/1.1");
    assert_eq!(
        found(
            Builtin::BytesIndexOfFrom,
            vec![hay.clone(), bytes(b" "), Value::Int(0)]
        ),
        some(3)
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOfFrom,
            vec![hay.clone(), bytes(b" "), Value::Int(4)]
        ),
        some(5)
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOfFrom,
            vec![hay.clone(), bytes(b""), Value::Int(14)]
        ),
        some(14),
        "an empty needle occurs where the search started, the end included"
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOfFrom,
            vec![hay.clone(), bytes(b" "), Value::Int(14)]
        ),
        "None"
    );
}

#[test]
fn a_start_outside_the_buffer_is_named_rather_than_clamped() {
    for (b, args) in [
        (
            Builtin::BytesIndexOfFrom,
            vec![bytes(b"abc"), bytes(b"a"), Value::Int(4)],
        ),
        (
            Builtin::BytesIndexOfFrom,
            vec![bytes(b"abc"), bytes(b"a"), Value::Int(-1)],
        ),
        (
            Builtin::BytesScan,
            vec![bytes(b"abc"), Value::Int(9), bytes(b"a"), Value::Int(1)],
        ),
    ] {
        let d = done(b, args).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR, "{}", b.name());
        assert!(
            d.message.contains("outside a value of 3 bytes"),
            "{}",
            d.message
        );
    }
}

#[test]
fn index_of_byte_takes_a_byte_and_refuses_anything_else() {
    assert_eq!(
        found(
            Builtin::BytesIndexOfByte,
            vec![bytes(b"a\r\nb"), Value::Int(13)]
        ),
        some(1)
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOfByte,
            vec![bytes(b"abc"), Value::Int(255)]
        ),
        "None"
    );
    for out_of_range in [-1, 256] {
        let d = done(
            Builtin::BytesIndexOfByte,
            vec![bytes(b"abc"), Value::Int(out_of_range)],
        )
        .unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("not a byte"), "{}", d.message);
    }
}

/// Required test 36.
#[test]
fn index_of_agrees_with_a_naive_search_over_ten_thousand_pairs() {
    fn naive(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        if needle.is_empty() {
            return Some(from);
        }
        (from..=hay.len().checked_sub(needle.len())?).find(|&i| &hay[i..i + needle.len()] == needle)
    }

    let mut rng = Xorshift(0x5eed_1234_9abc_def1);
    for case in 0..10_000 {
        // A three-letter alphabet, so needles hit often and overlap.
        let hay: Vec<u8> = (0..rng.below(40))
            .map(|_| b'a' + rng.below(3) as u8)
            .collect();
        let needle: Vec<u8> = (0..rng.below(4))
            .map(|_| b'a' + rng.below(3) as u8)
            .collect();
        let from = if hay.is_empty() {
            0
        } else {
            rng.below(hay.len() + 1)
        };

        assert_eq!(
            done(Builtin::BytesIndexOf, vec![bytes(&hay), bytes(&needle)])
                .unwrap()
                .render(),
            match naive(&hay, &needle, 0) {
                Some(i) => some(i as i64),
                None => "None".to_string(),
            },
            "case {case}: {hay:?} / {needle:?}"
        );
        assert_eq!(
            done(
                Builtin::BytesIndexOfFrom,
                vec![bytes(&hay), bytes(&needle), Value::Int(from as i64)]
            )
            .unwrap()
            .render(),
            match naive(&hay, &needle, from) {
                Some(i) => some(i as i64),
                None => "None".to_string(),
            },
            "case {case}: {hay:?} / {needle:?} from {from}"
        );
    }
}

#[test]
fn starts_with_and_ends_with_agree_with_the_empty_and_whole_cases() {
    let b = bytes(b"HTTP/1.1");
    for (builtin, hits, misses) in [
        (
            Builtin::BytesStartsWith,
            [&b""[..], b"H", b"HTTP/1.1"],
            [&b"HTTP/1.10"[..], b"T", b"1.1"],
        ),
        (
            Builtin::BytesEndsWith,
            [&b""[..], b"1", b"HTTP/1.1"],
            [&b"0HTTP/1.1"[..], b"T", b"HTTP"],
        ),
    ] {
        for hit in hits {
            assert_eq!(
                done(builtin, vec![b.clone(), bytes(hit)]).unwrap().render(),
                "true",
                "{} {hit:?}",
                builtin.name()
            );
        }
        for miss in misses {
            assert_eq!(
                done(builtin, vec![b.clone(), bytes(miss)])
                    .unwrap()
                    .render(),
                "false",
                "{} {miss:?}",
                builtin.name()
            );
        }
    }
    assert_eq!(
        done(Builtin::BytesStartsWith, vec![bytes(b""), bytes(b"")])
            .unwrap()
            .render(),
        "true"
    );
}

#[test]
fn split_keeps_the_empty_pieces_a_join_needs_to_round_trip() {
    let split = |hay: &[u8], sep: &[u8]| done(Builtin::BytesSplit, vec![bytes(hay), bytes(sep)]);
    assert_eq!(
        split(b"a,b,c", b",").unwrap().render(),
        "[b\"a\", b\"b\", b\"c\"]"
    );
    assert_eq!(split(b"", b",").unwrap().render(), "[b\"\"]");
    assert_eq!(split(b",", b",").unwrap().render(), "[b\"\", b\"\"]");
    assert_eq!(split(b"abc", b",").unwrap().render(), "[b\"abc\"]");
    assert_eq!(
        split(b"a\r\n\r\nb", b"\r\n").unwrap().render(),
        "[b\"a\", b\"\", b\"b\"]"
    );
    // Non-overlapping, left to right: the second `aa` starts after the first one's last byte.
    assert_eq!(
        split(b"aaaa", b"aa").unwrap().render(),
        "[b\"\", b\"\", b\"\"]"
    );
}

/// The limits's builtin.
#[test]
fn concat_all_joins_every_piece_in_order() {
    let empty = done(Builtin::BytesConcatAll, vec![Value::list(vec![])]).unwrap();
    assert_eq!(empty, Value::bytes([]));

    let pieces = Value::list(vec![
        bytes(b"GET "),
        bytes(b""),
        bytes(b"/x"),
        bytes(b" HTTP"),
    ]);
    assert_eq!(
        done(Builtin::BytesConcatAll, vec![pieces]).unwrap(),
        Value::bytes(b"GET /x HTTP")
    );

    let mut rng = Xorshift(0x00c0_ffee_0bad_f00d);
    for _ in 0..500 {
        let raw: Vec<Vec<u8>> = (0..rng.below(12))
            .map(|_| {
                (0..rng.below(9))
                    .map(|_| b'a' + rng.below(4) as u8)
                    .collect()
            })
            .collect();
        let expected: Vec<u8> = raw.concat();
        let list = Value::list(raw.iter().map(|p| bytes(p)).collect());
        assert_eq!(
            done(Builtin::BytesConcatAll, vec![list]).unwrap(),
            Value::bytes(expected)
        );
    }

    let mixed = Value::list(vec![bytes(b"a"), Value::Int(1)]);
    assert_eq!(
        done(Builtin::BytesConcatAll, vec![mixed])
            .expect_err("an Int is not a piece")
            .code,
        codes::RUNTIME_ERROR
    );
}

/// Required test 39, both halves.
#[test]
fn split_round_trips_against_a_join_and_refuses_an_empty_separator() {
    let mut rng = Xorshift(0xfeed_face_dead_b0d1);
    for case in 0..2_000 {
        let hay: Vec<u8> = (0..rng.below(30))
            .map(|_| b'a' + rng.below(3) as u8)
            .collect();
        let sep: Vec<u8> = (0..1 + rng.below(3))
            .map(|_| b'a' + rng.below(3) as u8)
            .collect();
        let split = done(Builtin::BytesSplit, vec![bytes(&hay), bytes(&sep)]).unwrap();
        let Value::List(pieces) = &split else {
            panic!("`bytes_split` answers a list");
        };
        let mut joined: Vec<u8> = Vec::new();
        for (i, piece) in pieces.iter().enumerate() {
            if i > 0 {
                joined.extend_from_slice(&sep);
            }
            let Value::Bytes(p) = piece else {
                panic!("`bytes_split` answers a list of Bytes");
            };
            joined.extend_from_slice(p);
        }
        assert_eq!(joined, hay, "case {case}: separator {sep:?}");
    }

    let d = done(Builtin::BytesSplit, vec![bytes(b"abc"), bytes(b"")]).unwrap_err();
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("empty"), "{}", d.message);
}

fn scan(b: Builtin, hay: &[u8], from: i64, set: &[u8], max: i64) -> Result<i64, Diagnostic> {
    Ok(done(
        b,
        vec![bytes(hay), Value::Int(from), bytes(set), Value::Int(max)],
    )?
    .as_int(Span::DUMMY, "test")
    .unwrap())
}

#[test]
fn a_scan_stops_on_the_class_and_the_other_stops_off_it() {
    let head = b"GET /orders?id=7 HTTP/1.1";
    let digits = b"0123456789";
    let big = head.len() as i64;

    assert_eq!(
        scan(Builtin::BytesScanUntil, head, 0, b" ", big).unwrap(),
        3
    );
    assert_eq!(
        scan(Builtin::BytesScanUntil, head, 4, b" ", big).unwrap(),
        16
    );
    assert_eq!(
        scan(Builtin::BytesScan, head, 4, b"/ordes?=i", big).unwrap(),
        15,
        "the target's own bytes run out at the `7`"
    );
    assert_eq!(
        scan(Builtin::BytesScan, head, 15, digits, big).unwrap(),
        16,
        "a digit run ends at the space after it"
    );
    assert_eq!(
        scan(Builtin::BytesScan, head, 14, digits, big).unwrap(),
        14,
        "`=` is not a digit, so the scan stops where it started"
    );

    // The whole point of `bytes_scan` over a fold: the answer for a run that reaches the end is
    // the end, not a sentinel.
    assert_eq!(
        scan(Builtin::BytesScanUntil, head, 0, b"z", big).unwrap(),
        big
    );
    assert_eq!(scan(Builtin::BytesScan, head, 0, b"", big).unwrap(), 0);
    assert_eq!(
        scan(Builtin::BytesScanUntil, head, 0, b"", big).unwrap(),
        big,
        "an empty class is never entered"
    );
    assert_eq!(scan(Builtin::BytesScanUntil, b"", 0, b"a", 10).unwrap(), 0);
    assert_eq!(scan(Builtin::BytesScan, head, big, b"a", big).unwrap(), big);
}

/// Every set size takes a different path — `memchr`, `memchr2`, `memchr3`, then the bitmap — so
/// the four have to agree with each other.
#[test]
fn every_set_size_takes_its_own_path_and_they_all_agree() {
    fn naive(hay: &[u8], from: usize, set: &[u8], max: usize, want: bool) -> i64 {
        let limit = hay.len().min(from + max);
        for (i, b) in hay.iter().enumerate().take(limit).skip(from) {
            if set.contains(b) == want {
                return i as i64;
            }
        }
        limit as i64
    }

    let mut rng = Xorshift(0x0123_4567_89ab_cdef);
    for case in 0..5_000 {
        let hay: Vec<u8> = (0..rng.below(50))
            .map(|_| b'a' + rng.below(6) as u8)
            .collect();
        let set: Vec<u8> = (0..rng.below(7))
            .map(|_| b'a' + rng.below(6) as u8)
            .collect();
        let from = if hay.is_empty() {
            0
        } else {
            rng.below(hay.len() + 1)
        };
        let max = rng.below(60);
        for (builtin, want) in [(Builtin::BytesScan, false), (Builtin::BytesScanUntil, true)] {
            assert_eq!(
                scan(builtin, &hay, from as i64, &set, max as i64).unwrap(),
                naive(&hay, from, &set, max, want),
                "case {case}: {} over {hay:?} from {from} set {set:?} max {max}",
                builtin.name()
            );
        }
    }
}

/// Required test 37.
#[test]
fn a_scan_examines_at_most_max_bytes() {
    for max in 0..40usize {
        assert!(scan_window(&[0u8; 64], 3, max).len() <= max);
        assert!(scan_window(&[0u8; 8], 3, max).len() <= max);
    }

    let mut hay = vec![b'a'; 1024];
    hay[100] = b'!';
    assert_eq!(
        scan(Builtin::BytesScanUntil, &hay, 0, b"!", 100).unwrap(),
        100,
        "the budget ran out exactly at the marker, which is one byte too far"
    );
    assert_eq!(
        scan(Builtin::BytesScanUntil, &hay, 0, b"!", 101).unwrap(),
        100
    );
    assert_eq!(
        scan(Builtin::BytesScanUntil, &hay, 0, b"!", 20).unwrap(),
        20,
        "a caller tells this from a hit by comparing against `from + max`"
    );
    assert_eq!(
        scan(Builtin::BytesScan, &hay, 0, b"a", 7).unwrap(),
        7,
        "the same bound applies to the complement scan"
    );
}

#[test]
fn a_negative_budget_is_a_bug_in_the_caller_and_is_named() {
    let d = scan(Builtin::BytesScan, b"abc", 0, b"a", -1).unwrap_err();
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("negative budget"), "{}", d.message);
}

#[test]
fn position_finds_the_first_byte_its_predicate_accepts() {
    let hay = bytes(b"abcXdef");
    let is_upper = |args: &[Value]| {
        let b = args[0].as_int(Span::DUMMY, "test").unwrap();
        Value::Bool((65..=90).contains(&b))
    };
    assert_eq!(
        drive(
            Builtin::BytesPosition,
            vec![hay.clone(), Value::Int(0), f()],
            is_upper
        )
        .unwrap()
        .render(),
        some(3)
    );
    assert_eq!(
        drive(
            Builtin::BytesPosition,
            vec![hay.clone(), Value::Int(4), f()],
            is_upper
        )
        .unwrap()
        .render(),
        "None"
    );
    assert_eq!(
        drive(
            Builtin::BytesPosition,
            vec![bytes(b""), Value::Int(0), f()],
            |_| panic!("an empty buffer calls no predicate")
        )
        .unwrap()
        .render(),
        "None"
    );
}

/// Required test 38.
#[test]
fn position_calls_its_predicate_once_for_a_match_at_the_start_of_a_megabyte() {
    let mut calls = 0;
    let out = drive(
        Builtin::BytesPosition,
        vec![Value::bytes(vec![7u8; 1 << 20]), Value::Int(0), f()],
        |_| {
            calls += 1;
            Value::Bool(true)
        },
    )
    .unwrap();
    assert_eq!(out.render(), some(0));
    assert_eq!(calls, 1);
}

#[test]
fn position_reports_a_non_boolean_answer_rather_than_reading_past_it() {
    let d = drive(
        Builtin::BytesPosition,
        vec![bytes(b"ab"), Value::Int(0), f()],
        |_| Value::Int(1),
    )
    .unwrap_err();
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("Bool"), "{}", d.message);
}

/// The property every builtin frame owes: a suspension point captured inside it can be advanced
/// more than once, and each resumption is its own search.
#[test]
fn one_suspension_point_inside_position_can_be_resumed_twice() {
    let mut cells: TaskRegions = TaskRegions::new();
    let start = call(
        Builtin::BytesPosition,
        vec![bytes(b"abc"), Value::Int(0), f()],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap();
    let Step::Apply { frame, .. } = start else {
        panic!("`bytes_position` suspends on its first byte");
    };

    let finish = |mut step: Step, fill: bool| loop {
        match step {
            Step::Done(v) => return v,
            Step::Apply { frame, .. } => step = advance(frame, Value::Bool(fill)).unwrap(),
        }
    };
    assert_eq!(
        finish(advance(frame.clone(), Value::Bool(true)).unwrap(), false).render(),
        some(0)
    );
    assert_eq!(
        finish(advance(frame, Value::Bool(false)).unwrap(), true).render(),
        some(1)
    );
}

/// The fold-based `index_of` from W1's `examples/hello.ply`, verbatim in Rust.
#[test]
fn the_scans_agree_with_the_folds_they_replace() {
    fn fold_index_of(hay: &[u8], byte: u8, from: usize) -> i64 {
        // The fold's shape, kept: it visits every remaining byte even after it has the answer,
        // which is the cost the builtins removed.
        let mut found: i64 = -1;
        for (i, &b) in hay.iter().enumerate().skip(from) {
            if found < 0 && b == byte {
                found = i as i64;
            }
        }
        found
    }

    fn fold_head_end(head: &[u8]) -> i64 {
        let mut found: i64 = -1;
        for i in 0..head.len().saturating_sub(3) {
            if found < 0 && &head[i..i + 4] == b"\r\n\r\n" {
                found = (i + 4) as i64;
            }
        }
        found
    }

    fn fold_all_upper(b: &[u8]) -> bool {
        b.iter().all(|c| (65..=90).contains(c))
    }

    let mut rng = Xorshift(0xabcd_ef01_2345_6789);
    for case in 0..5_000 {
        let head: Vec<u8> = (0..rng.below(60))
            .map(|_| [b'G', b'E', b'T', b' ', b'/', b'\r', b'\n', b'A'][rng.below(8)])
            .collect();
        let len = head.len() as i64;

        // `bytes_index_of_byte`, absent as `None` rather than as `-1`.
        let byte = b'\n';
        let native = at(&done(
            Builtin::BytesIndexOfByte,
            vec![bytes(&head), Value::Int(i64::from(byte))],
        )
        .unwrap());
        assert_eq!(
            native,
            fold_index_of(&head, byte, 0),
            "case {case}: {head:?}"
        );

        // The same question through the bounded scan, whose "absent" is the end of the window
        // rather than a sentinel.
        let stopped = scan(Builtin::BytesScanUntil, &head, 0, &[byte], len).unwrap();
        assert_eq!(
            if stopped == len { -1 } else { stopped },
            fold_index_of(&head, byte, 0),
            "case {case}: {head:?}"
        );

        // `head_end`, which was the most expensive of the five folds.
        let found = at(&done(
            Builtin::BytesIndexOf,
            vec![bytes(&head), bytes(b"\r\n\r\n")],
        )
        .unwrap());
        let end = if found < 0 { -1 } else { found + 4 };
        assert_eq!(end, fold_head_end(&head), "case {case}: {head:?}");

        // `all_upper`, as a complement scan that reaches the end.
        let upper = scan(
            Builtin::BytesScan,
            &head,
            0,
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            len,
        )
        .unwrap();
        assert_eq!(upper == len, fold_all_upper(&head), "case {case}: {head:?}");
    }
}

/// These are byte builtins and index in bytes, which is the whole reason a request target is
/// `Bytes`: a peer may send what is not UTF-8 at all.
#[test]
fn the_byte_searches_index_in_bytes_where_the_string_ones_index_in_characters() {
    let text = "héllo=wörld";
    assert_eq!(
        found(
            Builtin::BytesIndexOfByte,
            vec![bytes(text.as_bytes()), Value::Int(i64::from(b'='))]
        ),
        some(6),
        "`é` is two bytes, so the byte index is one past the character index"
    );
    assert_eq!(
        done(Builtin::StringFind, vec![Value::str(text), Value::str("=")])
            .unwrap()
            .render(),
        "5"
    );

    // A byte search may stop in the middle of a character, and the piece it cuts is refused by
    // `string_of_bytes` rather than silently replaced.
    let cut = done(
        Builtin::BytesSlice,
        vec![bytes(text.as_bytes()), Value::Int(0), Value::Int(2)],
    )
    .unwrap();
    assert_eq!(
        done(Builtin::BytesIsUtf8, vec![cut.clone()])
            .unwrap()
            .render(),
        "false"
    );
    assert_eq!(
        done(Builtin::StringOfBytes, vec![cut]).unwrap_err().code,
        codes::RUNTIME_ERROR
    );

    // A multi-byte needle is matched whole, so a search never reports a position that splits
    // one.
    assert_eq!(
        found(
            Builtin::BytesIndexOf,
            vec![bytes(text.as_bytes()), bytes("ö".as_bytes())]
        ),
        some(8)
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOf,
            vec![bytes(text.as_bytes()), bytes(&"é".as_bytes()[1..])]
        ),
        some(2),
        "a needle that is half a character still occurs where those bytes do"
    );
}

#[test]
fn map_visits_every_element_in_order() {
    let mut seen = Vec::new();
    let out = drive(Builtin::Map, vec![ints(&[1, 2, 3]), f()], |args| {
        let n = args[0].as_int(Span::DUMMY, "test").unwrap();
        seen.push(n);
        Value::Int(n * 10)
    })
    .unwrap();
    assert_eq!(seen, [1, 2, 3]);
    assert_eq!(out.render(), "[10, 20, 30]");
}

#[test]
fn an_empty_list_never_calls_the_callback() {
    let out = drive(Builtin::Map, vec![ints(&[]), f()], |_| {
        panic!("map called its function on an empty list")
    })
    .unwrap();
    assert_eq!(out.render(), "[]");
    let out = drive(Builtin::Fold, vec![ints(&[]), Value::Int(7), f()], |_| {
        panic!("fold called its function on an empty list")
    })
    .unwrap();
    assert_eq!(out.render(), "7");
}

#[test]
fn filter_keeps_the_element_its_predicate_accepted() {
    let out = drive(Builtin::Filter, vec![ints(&[1, 2, 3, 4]), f()], |args| {
        let n = args[0].as_int(Span::DUMMY, "test").unwrap();
        Value::Bool(n % 2 == 0)
    })
    .unwrap();
    assert_eq!(out.render(), "[2, 4]");
}

#[test]
fn fold_threads_the_accumulator_leftwards() {
    let out = drive(
        Builtin::Fold,
        vec![ints(&[1, 2, 3]), Value::Int(0), f()],
        |args| {
            let acc = args[0].as_int(Span::DUMMY, "test").unwrap();
            let x = args[1].as_int(Span::DUMMY, "test").unwrap();
            Value::Int(acc * 10 + x)
        },
    )
    .unwrap();
    assert_eq!(out.render(), "123");
}

/// The property the frames exist for: a suspension point inside `map` can be advanced twice and
/// each resumption completes its own list.
#[test]
fn one_suspension_point_inside_map_can_be_resumed_twice() {
    let mut cells: TaskRegions = TaskRegions::new();
    let start = call(
        Builtin::Map,
        vec![ints(&[1, 2, 3]), f()],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap();
    let Step::Apply { frame, .. } = start else {
        panic!("map suspends on its first element");
    };

    let finish = |mut step: Step, fill: i64| loop {
        match step {
            Step::Done(v) => return v,
            Step::Apply { frame, .. } => step = advance(frame, Value::Int(fill)).unwrap(),
        }
    };

    let a = finish(advance(frame.clone(), Value::Int(7)).unwrap(), 0);
    let b = finish(advance(frame, Value::Int(9)).unwrap(), 1);
    assert_eq!(a.render(), "[7, 0, 0]");
    assert_eq!(b.render(), "[9, 1, 1]");
}

/// `iterate` through the protocol an engine drives, with the step answered by hand: the seed is
/// threaded, `Stop` ends it, and the value `Stop` carries is the answer rather than the seed.
#[test]
fn an_iterate_threads_its_seed_and_answers_what_stop_carries() {
    let stop_at = |n: i64| {
        move |args: &[Value]| {
            let Value::Int(i) = args[0] else {
                panic!("the seed is an Int here")
            };
            if i >= n {
                Value::ctor("Stop", vec![Value::str(format!("done at {i}"))])
            } else {
                Value::ctor("Continue", vec![Value::Int(i + 1)])
            }
        }
    };
    let out = drive(
        Builtin::Iterate,
        vec![Value::Int(0), Value::Int(100), f()],
        stop_at(7),
    )
    .unwrap();
    assert_eq!(out.render(), "\"done at 7\"");

    // Stopping on the very first step costs one round, not none: the step has to run to say so.
    let out = drive(
        Builtin::Iterate,
        vec![Value::Int(9), Value::Int(1), f()],
        stop_at(0),
    )
    .unwrap();
    assert_eq!(out.render(), "\"done at 9\"");
}

/// The reason the loop is a `Frame` and not host recursion: a continuation captured inside the
/// step can be resumed more than once, and each resumption has to continue **its own** copy of
/// the countdown.
#[test]
fn one_suspension_point_inside_iterate_can_be_resumed_twice() {
    let mut cells: TaskRegions = TaskRegions::new();
    let start = call(
        Builtin::Iterate,
        vec![Value::Int(0), Value::Int(4), f()],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap();
    let Step::Apply { frame, .. } = start else {
        panic!("iterate suspends on its first round");
    };

    // Each leg runs the loop out from the same captured point.
    let run = |mut step: Step, stop_after: i64| {
        let mut seen = 0;
        loop {
            match step {
                Step::Done(v) => return Ok(v),
                Step::Apply { frame, .. } => {
                    seen += 1;
                    let answer = if seen >= stop_after {
                        Value::ctor("Stop", vec![Value::Int(seen)])
                    } else {
                        Value::ctor("Continue", vec![Value::Int(seen)])
                    };
                    match advance(frame, answer) {
                        Ok(next) => step = next,
                        Err(d) => return Err(d),
                    }
                }
            }
        }
    };

    let a = run(
        advance(frame.clone(), Value::ctor("Continue", vec![Value::Int(0)])).unwrap(),
        3,
    )
    .unwrap();
    let b = run(
        advance(frame, Value::ctor("Continue", vec![Value::Int(0)])).unwrap(),
        3,
    )
    .unwrap();
    assert_eq!(a.render(), "3");
    assert_eq!(
        b.render(),
        "3",
        "the second resumption inherited a spent budget"
    );

    // And the budget above is exactly tight, which is what makes the pair above non-vacuous:
    // one leg spends all four rounds, so two legs sharing a countdown could not both finish.
    let mut cells: TaskRegions = TaskRegions::new();
    let tight = call(
        Builtin::Iterate,
        vec![Value::Int(0), Value::Int(3), f()],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap();
    let Step::Apply { frame, .. } = tight else {
        panic!("iterate suspends on its first round");
    };
    let d = run(
        advance(frame, Value::ctor("Continue", vec![Value::Int(0)])).unwrap(),
        3,
    )
    .unwrap_err();
    assert!(d.message.contains("budget of 3 steps"), "{}", d.message);
}

/// The budget is spent per round and exhausting it is a diagnostic, because `Stop` is the only
/// source of an answer and there is none to give.
#[test]
fn an_iterate_that_never_stops_exhausts_its_budget_and_says_so() {
    let d = drive(
        Builtin::Iterate,
        vec![Value::Int(0), Value::Int(12), f()],
        |args| {
            let Value::Int(i) = args[0] else {
                panic!("the seed is an Int here")
            };
            Value::ctor("Continue", vec![Value::Int(i + 1)])
        },
    )
    .unwrap_err();
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("budget of 12 steps"), "{}", d.message);
    // Nothing nested, so nothing here may say it did.
    assert!(!d.message.contains("recursion limit"), "{}", d.message);
}

#[test]
fn an_iterate_budget_below_one_is_refused_before_the_loop_starts() {
    for budget in [0, -1] {
        let d = drive(
            Builtin::Iterate,
            vec![Value::Int(0), Value::Int(budget), f()],
            |_| panic!("the step must not run at all"),
        )
        .unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(
            d.message.contains(&format!("budget of {budget}")),
            "{}",
            d.message
        );
    }
}

/// Inference admits only `Iter<s, r>` in this position, so anything else arriving here came
/// from a host handler or a `Value` built in Rust — and treating it as a silent stop would
/// answer a value nobody asked for.
#[test]
fn an_iterate_step_answering_neither_continue_nor_stop_is_a_runtime_error() {
    let d = drive(
        Builtin::Iterate,
        vec![Value::Int(0), Value::Int(5), f()],
        |_| Value::ctor("Halt", vec![Value::Int(1)]),
    )
    .unwrap_err();
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("Continue or Stop"), "{}", d.message);
}

#[test]
fn a_non_boolean_from_a_filter_predicate_is_a_runtime_error() {
    let d = drive(Builtin::Filter, vec![ints(&[1]), f()], |_| Value::Int(1)).unwrap_err();
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("Bool"), "{}", d.message);
}

#[test]
fn advancing_a_frame_that_is_not_a_builtin_step_is_reported_not_ignored() {
    let frame = Frame::Call {
        name: None,
        call_site: Span::DUMMY,
        memo: false,
        callee_window: 0,
        caller_window: 0,
    };
    let d = advance(frame, Value::Unit).unwrap_err();
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert!(d.message.contains("internal error"), "{}", d.message);
}

#[test]
fn cell_builtins_read_and_write_the_arena_they_are_given() {
    let mut cells: TaskRegions = TaskRegions::new();
    let slot = cells.alloc_cell(Value::Int(1));
    let set = call(
        Builtin::CellSet,
        vec![Value::Cell(slot), Value::Int(2)],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap();
    assert!(matches!(set, Step::Done(Value::Unit)));

    let got = call(
        Builtin::CellGet,
        vec![Value::Cell(slot)],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap();
    let Step::Done(v) = got else {
        panic!("cell_get does not suspend");
    };
    assert_eq!(v.render(), "2");
}

/// The generation is what makes this a report rather than a read of the cell now living at that
/// position: the stale slot and the live one share an index and differ in generation.
#[test]
fn a_cell_from_another_region_stack_is_named_rather_than_silently_read() {
    let mut other = TaskRegions::new();
    other.alloc_cell(Value::Int(1));
    other.reset();
    let stale = other.alloc_cell(Value::Int(2));

    let mut cells: TaskRegions = TaskRegions::new();
    let live = cells.alloc_cell(Value::Int(3));
    assert_eq!(stale.index(), live.index());
    assert_ne!(stale.generation(), live.generation());

    let d = call(
        Builtin::CellGet,
        vec![Value::Cell(stale)],
        cells.arena_mut(),
        Span::DUMMY,
    )
    .unwrap_err();
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert!(d.message.contains("does not belong"), "{}", d.message);
}

#[test]
fn exactly_the_callback_builtins_are_higher_order() {
    let names: Vec<&str> = Builtin::all()
        .iter()
        .filter(|b| b.higher_order())
        .map(|b| b.name())
        .collect();
    let mut names = names;
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "bytes_position",
            "cell_update",
            "filter",
            "fold",
            "iterate",
            "map",
            "map_fold",
            "map_update"
        ]
    );
}

#[test]
fn every_builtin_is_reachable_by_the_name_it_reports() {
    for b in Builtin::all() {
        assert_eq!(Builtin::from_name(b.name()), Some(*b));
    }
}

/// **The test this repository went its whole history without.**
#[test]
fn every_builtin_agrees_on_its_arity_everywhere() {
    for b in Builtin::all() {
        let (min, max) = b.arity();
        assert_eq!(
            min,
            max,
            "`{}` has a variable arity; every builtin is exactly applied, and a call \
             that leaves an argument out is filled by `ply_syntax::defaults` before \
             anything here sees it",
            b.name()
        );

        // A builtin the prelude does not type cannot be called at all, so the two tables have
        // to cover the same set.
        let typed = ply_core::prelude_arity(b.name()).unwrap_or_else(|| {
            panic!(
                "`{}` is a builtin with no scheme in the prelude: no program can call it",
                b.name()
            )
        });
        assert_eq!(
            typed,
            max,
            "`{}` takes {max} arguments here and {typed} in the prelude's scheme. \
             Whichever is larger, the extra arm is unreachable from source.",
            b.name()
        );

        if let Some((params, defaults)) = ply_syntax::defaults::builtin_shape(b.name()) {
            assert_eq!(
                params,
                max,
                "`{}`'s defaults table describes {params} parameters, its arity {max}",
                b.name()
            );
            assert!(
                defaults > 0,
                "`{}` is in the defaults table with no default in it",
                b.name()
            );
        }
    }
}

/// What [`Builtin::all`] lists, pinned — because until this was written, **nothing checked that
/// it was complete**.
#[test]
fn builtin_all_is_complete_and_lists_each_name_once() {
    let mut names: Vec<&str> = Builtin::all().iter().map(|b| b.name()).collect();
    names.sort_unstable();
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(names, unique, "`Builtin::all()` lists a builtin twice");
    assert_eq!(
        names,
        [
            "assert",
            "assert_eq",
            "bits_of_float",
            "byte_of_int",
            "bytes_at",
            "bytes_concat",
            "bytes_concat_all",
            "bytes_ends_with",
            "bytes_index_of",
            "bytes_index_of_byte",
            "bytes_index_of_from",
            "bytes_is_utf8",
            "bytes_len",
            "bytes_of_string",
            "bytes_position",
            "bytes_scan",
            "bytes_scan_until",
            "bytes_slice",
            "bytes_split",
            "bytes_starts_with",
            "bytes_u32_le",
            "cell_get",
            "cell_set",
            "cell_update",
            "compare",
            "compare_values",
            "decimal_div",
            "decimal_of_float",
            "decimal_of_int",
            "decimal_of_string",
            "decimal_round",
            "decimal_to_string",
            "filter",
            "float_of_bits",
            "float_of_decimal",
            "fold",
            "i16_of_int",
            "i32_of_int",
            "i64_of_int",
            "i8_of_int",
            "int_of_decimal",
            "int_of_i16",
            "int_of_i32",
            "int_of_i64",
            "int_of_i8",
            "int_of_u16",
            "int_of_u32",
            "int_of_u64",
            "int_of_u8",
            "int_to_string",
            "iterate",
            "len",
            "list_at",
            "map",
            "map_contains",
            "map_entries",
            "map_fold",
            "map_get",
            "map_insert",
            "map_keys",
            "map_len",
            "map_merge",
            "map_new",
            "map_of_entries",
            "map_remove",
            "map_update",
            "map_values",
            "max",
            "min",
            "panic",
            "push",
            "range",
            "rotr",
            "rotr32",
            "secret_is_empty",
            "secret_of_string",
            "secret_verify",
            "string_concat",
            "string_contains",
            "string_ends_with",
            "string_find",
            "string_len",
            "string_lower",
            "string_of_bytes",
            "string_of_bytes_lossy",
            "string_slice",
            "string_split",
            "string_starts_with",
            "string_trim",
            "string_upper",
            "u16_of_int",
            "u32_of_int",
            "u64_of_int",
            "u8_of_int",
            "wrap_add",
            "wrap_mul",
            "wrap_sub",
        ],
        "a builtin was added to or removed from the enum without `Builtin::all()` being \
         updated — every table driven by `all()` silently skips it until this list agrees"
    );
}

/// The low word rotated: bits leaving the right come back on the left of a thirty-two-bit
/// word, whatever the `Int` above that word held and whatever the count's sign.
#[test]
fn rotr32_turns_the_low_word_and_answers_it_non_negative() {
    let cases: &[(i64, i64, i64)] = &[
        (1, 1, 0x8000_0000),
        (0x8000_0000, 31, 1),
        (0x1234_5678, 0, 0x1234_5678),
        (0x1234_5678, 32, 0x1234_5678),
        (0x1234_5678, 4, 0x8123_4567),
        (0x1_0000_0001, 1, 0x8000_0000),
        (-1, 7, 0xFFFF_FFFF),
        (2, -1, 4),
    ];
    for &(x, n, want) in cases {
        assert_eq!(
            done(Builtin::Rotr32, vec![Value::Int(x), Value::Int(n)]).unwrap(),
            Value::Int(want),
            "rotr32({x}, {n})"
        );
    }
}

/// The three that answer where `+`, `-` and `*` raise, at the boundaries
/// that are the only reason they exist. A value below 2^32 needs none of
/// them — the shift semantics says so — so every case here is at or across the
/// 64-bit edge.
#[test]
fn the_wrapping_builtins_are_modulo_two_to_the_sixty_fourth() {
    let cases: &[(Builtin, i64, i64, i64)] = &[
        (Builtin::WrapAdd, i64::MAX, 1, i64::MIN),
        (Builtin::WrapAdd, i64::MIN, -1, i64::MAX),
        (Builtin::WrapAdd, i64::MAX, i64::MAX, -2),
        (Builtin::WrapAdd, 2, 3, 5),
        (Builtin::WrapSub, i64::MIN, 1, i64::MAX),
        (Builtin::WrapSub, i64::MAX, -1, i64::MIN),
        (Builtin::WrapSub, 0, i64::MIN, i64::MIN),
        (Builtin::WrapMul, i64::MAX, 2, -2),
        (Builtin::WrapMul, i64::MIN, -1, i64::MIN),
        (Builtin::WrapMul, 1 << 32, 1 << 32, 0),
        (Builtin::WrapMul, 6, 7, 42),
    ];
    for &(b, x, y, want) in cases {
        assert_eq!(
            done(b, vec![Value::Int(x), Value::Int(y)]).unwrap(),
            Value::Int(want),
            "`{}`({x}, {y})",
            b.name()
        );
    }
}

/// None of the three can fail, so the only diagnostic any of them produces
/// is about an argument that is not an `Int` — which the checker refused
/// before it got here, leaving this as the shape an unchecked body meets.
#[test]
fn a_wrapping_builtin_refuses_a_non_int_and_nothing_else() {
    let d = done(Builtin::WrapAdd, vec![Value::Int(1), Value::str("2")])
        .expect_err("a `String` is not an `Int`");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("wrap_add"), "{}", d.message);
}

fn run(items: Vec<Item>, e: Expr) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(items);
    Machine::for_program(&program, &resolved).eval_expr_for_test(&e)
}

fn state() -> Item {
    effect_def("state", &[("get", Mode::Read, false)])
}

/// The suspension points are where a builtin is most likely to be handed a stale arena, so the
/// handler both writes a cell and decides the answer from it: a builtin that carried its own
/// copy would keep the count at 1 and keep the wrong elements.
#[test]
fn a_predicate_that_performs_sees_every_write_the_handler_made_before_it() {
    let bump = block(
        vec![discard(callv(
            "cell_set",
            vec![
                var("c"),
                bin(BinOp::Add, callv("cell_get", vec![var("c")]), int(1)),
            ],
        ))],
        Some(bin(
            BinOp::Eq,
            bin(BinOp::Rem, callv("cell_get", vec![var("c")]), int(2)),
            int(0),
        )),
    );
    let kept = handle(
        callv(
            "filter",
            vec![
                list(vec![int(10), int(20), int(30), int(40)]),
                lam(&["x"], perform("state", "get", None, vec![])),
            ],
        ),
        vec![clause("state", "get", None, &[], bump)],
    );
    let e = with_cell(
        "s",
        int(0),
        "c",
        block(
            vec![letv("kept", kept)],
            Some(bin(
                BinOp::Add,
                bin(BinOp::Mul, callv("len", vec![var("kept")]), int(100)),
                callv("cell_get", vec![var("c")]),
            )),
        ),
    );
    assert_eq!(run(vec![state()], e).unwrap().render(), "204");
}

#[test]
fn a_fold_function_may_perform_and_the_accumulator_still_threads() {
    let e = handle(
        callv(
            "fold",
            vec![
                list(vec![int(1), int(2), int(3)]),
                int(0),
                lam(
                    &["acc", "x"],
                    bin(
                        BinOp::Add,
                        bin(BinOp::Add, var("acc"), var("x")),
                        perform("state", "get", None, vec![]),
                    ),
                ),
            ],
        ),
        vec![clause("state", "get", None, &[], int(100))],
    );
    assert_eq!(run(vec![state()], e).unwrap().render(), "306");
}

#[test]
fn an_assertion_inside_a_callback_keeps_its_structured_failure() {
    let e = callv(
        "map",
        vec![
            list(vec![int(1), int(2)]),
            lam(&["x"], callv("assert_eq", vec![var("x"), int(1)])),
        ],
    );
    let d = run(Vec::new(), e).unwrap_err();
    assert_eq!(d.code, codes::ASSERTION_FAILED);
    assert_eq!(d.message, "assertion failed: expected 1, found 2");
    assert!(
        d.notes.contains(&"actual:   2".to_string()),
        "{:?}",
        d.notes
    );
}

#[test]
fn map_update_applies_the_function_to_a_present_key_and_leaves_an_absent_one_alone() {
    let m = done(
        Builtin::MapInsert,
        vec![Value::empty_map(), Value::str("k"), Value::Int(1)],
    )
    .unwrap();
    let out = drive(
        Builtin::MapUpdate,
        vec![m.clone(), Value::str("k"), f()],
        |args| Value::Int(args[0].as_int(Span::DUMMY, "test").unwrap() + 41),
    )
    .unwrap();
    assert_eq!(out.render(), "{\"k\": 42}");
    let untouched = drive(Builtin::MapUpdate, vec![m, Value::str("z"), f()], |_| {
        panic!("`map_update` called its function on an absent key")
    })
    .unwrap();
    assert_eq!(untouched.render(), "{\"k\": 1}");
}
