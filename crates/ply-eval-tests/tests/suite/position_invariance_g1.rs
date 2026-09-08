//! **the gate G1 — position invariance, registered before the measurement.**




/// One shape, written the two ways.
struct Pair {
    /// What the pair is about, printed in the table.
    name: &'static str,
    canonical: &'static str,
    pessimal: &'static str,
}

/// the ownership design rows 1 and 2 — `go(i + 1, push(acc, i))` at 200 / 200
/// against `go(push(acc, i), i + 1)` at 0 / 200. The two functions differ in
/// parameter order because that is the shape: the growing argument moves, and
/// nothing else about the computation does.
const CALL_ARG_CANONICAL: &str = r#"
fn go(i: Int, acc: List<Int>) -> List<Int> =
  if i >= 200 { acc } else { go(i + 1, push(acc, i)) }

test "the growing argument is last in the call" {
  assert_eq(len(go(0, [])), 200)
}
"#;

const CALL_ARG_PESSIMAL: &str = r#"
fn go(acc: List<Int>, i: Int) -> List<Int> =
  if i >= 200 { acc } else { go(push(acc, i), i + 1) }

test "the growing argument is first in the call" {
  assert_eq(len(go([], 0)), 200)
}
"#;

/// the ownership design rows 3 and 4 — the same loop with the accumulator inside a
/// record, the growing field last against first in the literal. This is the
/// rule as `docs/GUIDE.md` §6.7 states it.
const RECORD_FIELD_CANONICAL: &str = r#"
fn go(i: Int, s: {k: Int, out: List<Int>}) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go(i + 1, {k: s.k + 1, out: push(s.out, i)}) }

test "the growing field is last in its literal" {
  let s = go(0, {k: 0, out: []});
  assert_eq(len(s.out) + s.k, 400)
}
"#;

const RECORD_FIELD_PESSIMAL: &str = r#"
fn go(i: Int, s: {k: Int, out: List<Int>}) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go(i + 1, {out: push(s.out, i), k: s.k + 1}) }

test "the growing field is first in its literal" {
  let s = go(0, {k: 0, out: []});
  assert_eq(len(s.out) + s.k, 400)
}
"#;

/// the ownership design row **five**, which is the finding: the growing field is
/// last in its literal — the documented rule, applied correctly — and the
/// record is not last in the enclosing call, so the program is quadratic
/// anyway. The rule compounds at every enclosing node on the path from the
/// `push` up, and this is the pair that says so: its canonical member is
/// byte-identical to [`RECORD_FIELD_CANONICAL`] and only the *outer* node moved.
const COMPOUNDING_PESSIMAL: &str = r#"
fn go(s: {k: Int, out: List<Int>}, i: Int) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go({k: s.k + 1, out: push(s.out, i)}, i + 1) }

test "the growing field is last and its record is first in the call" {
  let s = go({k: 0, out: []}, 0);
  assert_eq(len(s.out) + s.k, 400)
}
"#;

/// the ownership design cause 1 — *"an accumulator threaded as a `let` binding is
/// reused; the identical accumulator threaded as a parameter is not"*, measured
/// there as 1 of 1 against 0 of 1 on
/// `{ let t = push(xs, 1); let u = 7; len(t) + u }`. Those three statements are
/// reproduced verbatim in both members — with the binding that produces `xs`
/// added ahead of them, since the ownership design's fragment does not say where `xs` comes
/// from and that is the whole of what this pair varies: a statement binder here,
/// a parameter there.
const PARAM_VS_LET_CANONICAL: &str = r#"
fn probe(n: Int) -> Int = {
  let xs = range(0, n);
  let t = push(xs, 1);
  let u = 7;
  len(t) + u
}

fn drive(i: Int, tot: Int) -> Int =
  if i >= 200 { tot } else { drive(i + 1, tot + probe(3)) }

test "the accumulator arrives as a let binding" {
  assert_eq(drive(0, 0), 2200)
}
"#;

const PARAM_VS_LET_PESSIMAL: &str = r#"
fn inner(xs: List<Int>) -> Int = {
  let t = push(xs, 1);
  let u = 7;
  len(t) + u
}

fn probe(n: Int) -> Int = inner(range(0, n))

fn drive(i: Int, tot: Int) -> Int =
  if i >= 200 { tot } else { drive(i + 1, tot + probe(3)) }

test "the accumulator arrives as a parameter" {
  assert_eq(drive(0, 0), 2200)
}
"#;

/// the ownership design row 6 — the `fold` accumulator, which is the shape the
/// standard library is written in, at 200 / 200.
const FOLD_CLOSURE_CANONICAL: &str = r#"
fn keep_last(i: Int, a: List<Int>) -> List<Int> = a

fn build() -> List<Int> = fold(range(0, 200), [], |acc, x| keep_last(x, push(acc, x)))

test "a fold accumulator appended in last position" {
  assert_eq(len(build()), 200)
}
"#;

const FOLD_CLOSURE_PESSIMAL: &str = r#"
fn keep_first(a: List<Int>, i: Int) -> List<Int> = a

fn build() -> List<Int> = fold(range(0, 200), [], |acc, x| keep_first(push(acc, x), x))

test "a fold accumulator appended in first position" {
  assert_eq(len(build()), 200)
}
"#;

/// How many pairs the corpus has, and therefore how many shapes G1 is taken
/// over.
const EXPECTED_PAIRS: usize = 5;

fn corpus() -> Vec<Pair> {
    vec![
        Pair {
            name: "call argument",
            canonical: CALL_ARG_CANONICAL,
            pessimal: CALL_ARG_PESSIMAL,
        },
        Pair {
            name: "record field",
            canonical: RECORD_FIELD_CANONICAL,
            pessimal: RECORD_FIELD_PESSIMAL,
        },
        Pair {
            name: "compounding: field last, record first",
            canonical: RECORD_FIELD_CANONICAL,
            pessimal: COMPOUNDING_PESSIMAL,
        },
        Pair {
            name: "let binding against parameter",
            canonical: PARAM_VS_LET_CANONICAL,
            pessimal: PARAM_VS_LET_PESSIMAL,
        },
        Pair {
            name: "fold closure accumulator",
            canonical: FOLD_CLOSURE_CANONICAL,
            pessimal: FOLD_CLOSURE_PESSIMAL,
        },
    ]
}



/// A member's identity, as twelve hex characters of BLAKE3 over its source.
fn digest(src: &str) -> String {
    format!("b3:{}", &blake3::hash(src.as_bytes()).to_hex()[..12])
}








/// The corpus is five shapes, each pair is two different programs, and each is
/// the program it was pinned as.
#[test]
fn the_corpus_is_the_five_shapes_it_says_it_is() {
    let pairs = corpus();
    assert_eq!(
        pairs.len(),
        EXPECTED_PAIRS,
        "the corpus lost or gained a pair"
    );

    for pair in &pairs {
        assert_ne!(
            digest(pair.canonical),
            digest(pair.pessimal),
            "`{}` is one program written twice, so it measures no difference in position",
            pair.name,
        );
    }
    for (i, a) in pairs.iter().enumerate() {
        for b in &pairs[i + 1..] {
            assert_ne!(
                digest(a.pessimal),
                digest(b.pessimal),
                "`{}` and `{}` are the same pessimal program, so one of the two shapes the slot rewrite \
                 the gate reports a row for is not being measured",
                a.name,
                b.name,
            );
        }
    }

    let digests: Vec<(String, String)> = pairs
        .iter()
        .map(|p| (digest(p.canonical), digest(p.pessimal)))
        .collect();
    let got: Vec<(&str, &str, &str)> = pairs
        .iter()
        .zip(&digests)
        .map(|(p, d)| (p.name, d.0.as_str(), d.1.as_str()))
        .collect();

    // Taken on this tree 2026-08-31. The second and third rows share a
    // canonical digest on purpose: the compounding pair is the record-field
    // canonical with only the *outer* node moved, which is the whole of what
    // the ownership design row five says.
    let expected: [(&str, &str, &str); EXPECTED_PAIRS] = [
        ("call argument", "b3:2a168234d2a1", "b3:dc587bf7bfb2"),
        ("record field", "b3:031f75989bca", "b3:a52314fd4b69"),
        (
            "compounding: field last, record first",
            "b3:031f75989bca",
            "b3:adf0a898c53a",
        ),
        (
            "let binding against parameter",
            "b3:e242c2b60c30",
            "b3:5c6b5d559209",
        ),
        (
            "fold closure accumulator",
            "b3:9416752c4c7a",
            "b3:71e06e69eacd",
        ),
    ];

    assert_eq!(
        got.as_slice(),
        expected.as_slice(),
        "a corpus member is not the program it was pinned as; re-pin it here if you edited it \
         deliberately, and if you did not, a member moved under you — see this test's \
         documentation"
    );
}


