//! `min` and `max` over integers, and a module's own `min` winning inside that module — the
//! standard library's `hash` declares one.

use crate::fixture::Compiled;

#[test]
fn min_and_max_answer_over_integers_and_a_modules_own_min_shadows_the_builtin() {
    let compiled = Compiled::new(
        r#"
fn clamp(n: Int, lo: Int, hi: Int) -> Int = max(lo, min(n, hi))

test "min and max" {
  assert_eq(min(3, 5), 3);
  assert_eq(max(3, 5), 5);
  assert_eq(min(-7, -7), -7);
  assert_eq(max(-7, 2), 2);
  assert_eq(clamp(12, 0, 10), 10);
  assert_eq(clamp(-3, 0, 10), 0);
  assert_eq(clamp(4, 0, 10), 4)
}
"#,
    );
    let index = compiled.index_of("min and max");
    if let Err(d) = compiled.machine().eval_test(index) {
        panic!("the test must pass: {d:#?}");
    }

    let shadowing = Compiled::new(
        r#"
fn min(a: Int, b: Int) -> Int = a + b

test "a module's own min" {
  assert_eq(min(3, 5), 8);
  assert_eq(max(3, 5), 5)
}
"#,
    );
    let index = shadowing.index_of("a module's own min");
    if let Err(d) = shadowing.machine().eval_test(index) {
        panic!("the module's own `min` must win: {d:#?}");
    }
}
