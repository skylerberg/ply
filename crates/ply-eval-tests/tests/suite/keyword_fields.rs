//! A keyword names a record field wherever a field is named — a type, a literal, a pattern, a
//! `.` read and an update — and the program checks, runs and renders as any other record does.

use crate::fixture::Compiled;

#[test]
fn a_record_with_keyword_fields_is_built_matched_read_updated_and_rendered() {
    let compiled = Compiled::new(
        r#"
type EffectDef = { effect: Bytes, nondet: Bool, type: Int }

fn declare(e: Bytes) -> EffectDef = { effect: e, nondet: false, type: 1 }

fn mark(d: EffectDef) -> EffectDef = {..d, nondet: true}

fn describe(d: EffectDef) -> Int = match d {
  { nondet: true, type: t, .. } -> t + 100,
  { effect: e, .. } -> if e == b"io" { 2 } else { 0 }
}

test "keyword fields" {
  let d = declare(b"io");
  assert_eq(d.effect, b"io");
  assert_eq(d.type, 1);
  assert_eq(describe(d), 2);
  assert_eq(describe(mark(d)), 101);
  assert_eq(mark(d), { effect: b"io", nondet: true, type: 1 })
}
"#,
    );
    let index = compiled.index_of("keyword fields");
    if let Err(d) = compiled.machine().eval_test(index) {
        panic!("the test must pass: {d:#?}");
    }
}
