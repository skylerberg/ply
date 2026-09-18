/// One shape, written the two ways.
struct Pair {
    name: &'static str,
    canonical: &'static str,
    pessimal: &'static str,
}

/// The members differ only in parameter order: the growing argument moves and nothing else does.
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

/// Field last, record first: the position rule compounds at every enclosing node.
const COMPOUNDING_PESSIMAL: &str = r#"
fn go(s: {k: Int, out: List<Int>}, i: Int) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go({k: s.k + 1, out: push(s.out, i)}, i + 1) }

test "the growing field is last and its record is first in the call" {
  let s = go({k: 0, out: []}, 0);
  assert_eq(len(s.out) + s.k, 400)
}
"#;

/// The members differ only in how `xs` arrives: a statement binder here, a parameter there.
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

fn digest(src: &str) -> String {
    format!("b3:{}", &blake3::hash(src.as_bytes()).to_hex()[..12])
}

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

    // The second and third rows share a canonical digest on purpose: only the outer node moved.
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
