use ply_eval::builtins::*;
use ply_eval::{Diagnostic, Span, Value, codes};

fn ints(xs: &[i64]) -> Value {
    Value::list(xs.iter().copied().map(Value::Int).collect())
}

fn done(b: Builtin, args: Vec<Value>) -> Result<Value, Diagnostic> {
    call(b, args, Span::DUMMY)
}

fn bytes(b: &[u8]) -> Value {
    Value::bytes(b)
}

fn found(b: Builtin, args: Vec<Value>) -> Value {
    done(b, args).unwrap()
}

fn some(i: i64) -> Value {
    Value::ctor("Some", vec![Value::Int(i)])
}

fn none() -> Value {
    Value::ctor("None", Vec::new())
}

/// `Some(i)` as `i` and `None` as `-1`, the shape the folds it is compared against answer in.
fn at(v: &Value) -> i64 {
    match v {
        Value::Ctor { args, .. } if !args.is_empty() => {
            args[0].as_int(Span::DUMMY, "test").unwrap()
        }
        _ => -1,
    }
}

/// Deterministic, so a failing case is a seed a reader can reproduce.
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
        none()
    );
    assert_eq!(
        found(Builtin::BytesIndexOf, vec![hay.clone(), bytes(b"z")]),
        none()
    );
    assert_eq!(
        found(
            Builtin::BytesIndexOf,
            vec![hay.clone(), bytes(b"aaabaaabx")]
        ),
        none(),
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
        none()
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
        none()
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
            done(Builtin::BytesIndexOf, vec![bytes(&hay), bytes(&needle)]).unwrap(),
            match naive(&hay, &needle, 0) {
                Some(i) => some(i as i64),
                None => none(),
            },
            "case {case}: {hay:?} / {needle:?}"
        );
        assert_eq!(
            done(
                Builtin::BytesIndexOfFrom,
                vec![bytes(&hay), bytes(&needle), Value::Int(from as i64)]
            )
            .unwrap(),
            match naive(&hay, &needle, from) {
                Some(i) => some(i as i64),
                None => none(),
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
                done(builtin, vec![b.clone(), bytes(hit)]).unwrap(),
                Value::Bool(true),
                "{} {hit:?}",
                builtin.name()
            );
        }
        for miss in misses {
            assert_eq!(
                done(builtin, vec![b.clone(), bytes(miss)]).unwrap(),
                Value::Bool(false),
                "{} {miss:?}",
                builtin.name()
            );
        }
    }
    assert_eq!(
        done(Builtin::BytesStartsWith, vec![bytes(b""), bytes(b"")]).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn split_keeps_the_empty_pieces_a_join_needs_to_round_trip() {
    let split = |hay: &[u8], sep: &[u8]| done(Builtin::BytesSplit, vec![bytes(hay), bytes(sep)]);
    assert_eq!(
        split(b"a,b,c", b",").unwrap(),
        Value::list(vec![bytes(b"a"), bytes(b"b"), bytes(b"c")])
    );
    assert_eq!(split(b"", b",").unwrap(), Value::list(vec![bytes(b"")]));
    assert_eq!(
        split(b",", b",").unwrap(),
        Value::list(vec![bytes(b""), bytes(b"")])
    );
    assert_eq!(
        split(b"abc", b",").unwrap(),
        Value::list(vec![bytes(b"abc")])
    );
    assert_eq!(
        split(b"a\r\n\r\nb", b"\r\n").unwrap(),
        Value::list(vec![bytes(b"a"), bytes(b""), bytes(b"b")])
    );
    // Non-overlapping, left to right: the second `aa` starts after the first one's last byte.
    assert_eq!(
        split(b"aaaa", b"aa").unwrap(),
        Value::list(vec![bytes(b""), bytes(b""), bytes(b"")])
    );
}

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

    // A run that reaches the end answers the end, not a sentinel.
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

/// The four paths are `memchr`, `memchr2`, `memchr3`, then the bitmap.
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
fn the_scans_agree_with_the_folds_they_replace() {
    fn fold_index_of(hay: &[u8], byte: u8, from: usize) -> i64 {
        // Visits every remaining byte even after it has the answer, as the fold did.
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

        // The bounded scan's "absent" is the end of the window, not a sentinel.
        let stopped = scan(Builtin::BytesScanUntil, &head, 0, &[byte], len).unwrap();
        assert_eq!(
            if stopped == len { -1 } else { stopped },
            fold_index_of(&head, byte, 0),
            "case {case}: {head:?}"
        );

        // `head_end`.
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
        done(Builtin::StringFind, vec![Value::str(text), Value::str("=")]).unwrap(),
        Value::Int(5)
    );

    // A byte search may cut a character, and `string_of_bytes` refuses the piece.
    let cut = done(
        Builtin::BytesSlice,
        vec![bytes(text.as_bytes()), Value::Int(0), Value::Int(2)],
    )
    .unwrap();
    assert_eq!(
        done(Builtin::BytesIsUtf8, vec![cut.clone()]).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        done(Builtin::StringOfBytes, vec![cut]).unwrap_err().code,
        codes::RUNTIME_ERROR
    );

    // A multi-byte needle is matched whole, never split.
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
fn every_builtin_is_reachable_by_the_name_it_reports() {
    for b in Builtin::all() {
        assert_eq!(Builtin::from_name(b.name()), Some(*b));
    }
}

/// Each builtin's parameter count in the scheme the port's checker binds it to: its `tycore.Type`,
/// which is a `TyFn` whose `params` are the parameters.
fn prelude_arities() -> std::collections::BTreeMap<String, usize> {
    ply_codegen::c::producer::ensure_default();
    let answer = ply_codegen::c::producer::call("front.builtin_rows", &[])
        .expect("the port publishes its builtins");
    let rows = ply_eval::decode::At::new("`front.builtin_rows`' answer", &answer);
    let mut out = std::collections::BTreeMap::new();
    for row in rows.list().unwrap() {
        let name = row.field("name").and_then(|n| n.utf8()).unwrap();
        let documented = row.field("params").and_then(|p| p.list()).unwrap().len();
        let ty = row
            .field("scheme")
            .and_then(|s| s.field("ty"))
            .and_then(|t| t.ctor())
            .unwrap();
        assert_eq!(ty.name(), "TyFn", "`{name}`'s scheme is not a function");
        let params = ty
            .arg(0)
            .and_then(|f| f.field("params"))
            .and_then(|p| p.list())
            .unwrap()
            .len();
        assert_eq!(
            documented, params,
            "`{name}` is documented with {documented} parameter names for {params} parameters"
        );
        let twice = out.insert(name.to_string(), params).is_some();
        assert!(!twice, "the prelude binds `{name}` twice");
    }
    out
}

#[test]
fn every_builtin_agrees_on_its_arity_everywhere() {
    let prelude = prelude_arities();
    for b in Builtin::all() {
        let (min, max) = b.arity();
        assert_eq!(
            min,
            max,
            "`{}` has a variable arity; every builtin is exactly applied, and a call \
             that leaves an argument out is filled by the front end's defaults pass \
             before anything here sees it",
            b.name()
        );

        if let Some(&typed) = prelude.get(b.name()) {
            assert_eq!(
                typed,
                max,
                "`{}` takes {max} arguments here and {typed} in the prelude's scheme. \
                 Whichever is larger, the extra arm is unreachable from source.",
                b.name()
            );
        }
    }
}

/// Whether each builtin's scheme in the port's prelude carries `abort.raise` in its row.
fn prelude_raises() -> std::collections::BTreeMap<String, bool> {
    ply_codegen::c::producer::ensure_default();
    let answer = ply_codegen::c::producer::call("front.builtin_rows", &[])
        .expect("the port publishes its builtins");
    let rows = ply_eval::decode::At::new("`front.builtin_rows`' answer", &answer);
    let mut out = std::collections::BTreeMap::new();
    for row in rows.list().unwrap() {
        let name = row.field("name").and_then(|n| n.utf8()).unwrap();
        let raises = row
            .field("scheme")
            .and_then(|s| s.field("ty"))
            .and_then(|t| t.ctor())
            .and_then(|f| f.arg(0))
            .and_then(|f| f.field("effects"))
            .and_then(|r| r.field("atoms"))
            .and_then(|a| a.list())
            .unwrap()
            .any(|a| a.field("effect").and_then(|e| e.utf8()).unwrap() == "abort");
        out.insert(name.to_string(), raises);
    }
    out
}

/// A failure of a builtin whose row says it may raise is one a `handle` answers; any other ends
/// the run. The runtime decides by `Builtin::raises`, so it has to say what the scheme says.
#[test]
fn a_builtin_raises_exactly_when_its_scheme_says_it_may() {
    let prelude = prelude_raises();
    for b in Builtin::all() {
        if let Some(&typed) = prelude.get(b.name()) {
            assert_eq!(
                b.raises(),
                typed,
                "`{}` {} in its scheme but `Builtin::raises` says {}",
                b.name(),
                if typed { "raises" } else { "does not raise" },
                b.raises()
            );
        }
    }
}

/// A builtin with no scheme cannot be called; a scheme with no builtin checks and then fails.
#[test]
fn the_runtime_implements_exactly_the_builtins_the_prelude_types() {
    let mut names: Vec<&str> = Builtin::all().iter().map(|b| b.name()).collect();
    names.sort_unstable();
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(names, unique, "`Builtin::all()` lists a builtin twice");

    let prelude = prelude_arities();
    let untyped: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !prelude.contains_key(*n))
        .collect();
    assert!(
        untyped.is_empty(),
        "builtins with no scheme in the prelude, so no program can call them: {untyped:?}"
    );
    let unimplemented: Vec<&String> = prelude
        .keys()
        .filter(|n| names.binary_search(&n.as_str()).is_err())
        .collect();
    assert!(
        unimplemented.is_empty(),
        "prelude schemes `Builtin::all()` has no builtin for: a call checks and then fails, \
         or `all()` is missing an enum variant and every table driven by it skips it: \
         {unimplemented:?}"
    );
}

#[test]
fn list_set_replaces_one_element_keeps_the_original_and_raises_outside_the_list() {
    let xs = ints(&[10, 20, 30]);
    let ys = done(
        Builtin::ListSet,
        vec![xs.clone(), Value::Int(1), Value::Int(99)],
    )
    .unwrap();
    assert_eq!(ys, ints(&[10, 99, 30]));
    assert_eq!(xs, ints(&[10, 20, 30]), "the list given is unchanged");

    let long: Vec<i64> = (0..100).collect();
    let big = ints(&long);
    let set = done(
        Builtin::ListSet,
        vec![big.clone(), Value::Int(40), Value::Int(-1)],
    )
    .unwrap();
    let mut want = long.clone();
    want[40] = -1;
    assert_eq!(set, ints(&want));
    assert_eq!(big, ints(&long), "a write below the tail shares the trie");

    for i in [-1, 3] {
        let d = done(
            Builtin::ListSet,
            vec![xs.clone(), Value::Int(i), Value::Int(0)],
        )
        .unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR, "list_set(xs, {i}, 0)");
        assert!(
            d.message.contains("outside a value of 3 elements"),
            "{}",
            d.message
        );
    }
}

fn array(xs: &[i64]) -> Value {
    Value::array(xs.iter().copied().map(Value::Int).collect())
}

#[test]
fn an_array_is_read_by_index_and_written_through_its_last_holder() {
    assert_eq!(
        found(Builtin::ArrayNew, vec![Value::Int(3), Value::Int(7)]),
        array(&[7, 7, 7])
    );
    let xs = found(Builtin::ArrayOfList, vec![ints(&[10, 20, 30])]);
    assert_eq!(xs, array(&[10, 20, 30]));
    assert_eq!(
        found(Builtin::ArrayToList, vec![xs.clone()]),
        ints(&[10, 20, 30])
    );
    assert_eq!(found(Builtin::ArrayLen, vec![xs.clone()]), Value::Int(3));
    assert_eq!(
        found(Builtin::ArrayGet, vec![xs.clone(), Value::Int(2)]),
        Value::Int(30)
    );
    assert_eq!(
        found(Builtin::ArrayAt, vec![xs.clone(), Value::Int(1)]),
        some(20)
    );
    for i in [-1, 3, i64::MAX] {
        assert_eq!(
            found(Builtin::ArrayAt, vec![xs.clone(), Value::Int(i)]),
            none()
        );
    }

    let ys = found(
        Builtin::ArraySet,
        vec![xs.clone(), Value::Int(1), Value::Int(99)],
    );
    assert_eq!(ys, array(&[10, 99, 30]));
    assert_eq!(
        xs,
        array(&[10, 20, 30]),
        "a held array is copied, not written"
    );

    for (b, args) in [
        (Builtin::ArrayGet, vec![xs.clone(), Value::Int(3)]),
        (Builtin::ArrayGet, vec![xs.clone(), Value::Int(-1)]),
        (
            Builtin::ArraySet,
            vec![xs.clone(), Value::Int(3), Value::Int(0)],
        ),
    ] {
        let d = done(b, args).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR, "{}", b.name());
        assert!(
            d.message.contains("outside a value of 3 elements"),
            "{}",
            d.message
        );
    }
    for n in [-1, MAX_ARRAY_LEN + 1] {
        let d = done(Builtin::ArrayNew, vec![Value::Int(n), Value::Unit]).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR, "array_new({n})");
    }
}

#[test]
fn a_digest_agrees_with_equality_and_refuses_what_has_no_hash() {
    let digest = |v: Value| found(Builtin::Digest, vec![v]);
    let decimal = |s: &str| Value::Decimal(s.parse().unwrap());
    assert_eq!(digest(decimal("1.50")), digest(decimal("1.5")));
    let narrow = ply_eval::Fixed::of(ply_eval::IntTy::U8, 7).unwrap();
    assert_eq!(
        digest(Value::Fixed(narrow)),
        digest(Value::Int(7)),
        "compiled code holds a narrow width as the Int it reads as"
    );
    assert_eq!(
        digest(Value::ctor("orders.Shipped", vec![Value::Int(1)])),
        digest(Value::ctor("app.orders.Shipped", vec![Value::Int(1)])),
        "where a module stands is no part of a value"
    );
    assert_ne!(digest(ints(&[1, 2])), digest(ints(&[2, 1])));
    for v in [Value::Float(1.0), Value::secret(Value::str("pw"))] {
        let d = done(Builtin::Digest, vec![v]).unwrap_err();
        assert_eq!(d.code, codes::RUNTIME_ERROR);
    }
}

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

/// Every case is at or across the 64-bit edge, the only place these differ from `+`, `-`, `*`.
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

/// The checker refuses a non-`Int` first; this is what an unchecked body meets.
#[test]
fn a_wrapping_builtin_refuses_a_non_int_and_nothing_else() {
    let d = done(Builtin::WrapAdd, vec![Value::Int(1), Value::str("2")])
        .expect_err("a `String` is not an `Int`");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("wrap_add"), "{}", d.message);
}
