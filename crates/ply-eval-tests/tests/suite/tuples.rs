//! Tuples are records with positional fields: built, matched, read by field, and rendered as
//! they were written.

use crate::fixture::Compiled;

#[test]
fn a_tuple_is_built_matched_read_and_rendered_as_the_record_it_is() {
    let compiled = Compiled::new(
        r#"
fn divmod(a: Int, b: Int) -> (Int, Int) = (a / b, a % b)

fn swap(p: (Int, Bool)) -> (Bool, Int) = match p { (n, flag) -> (flag, n) }

test "built and matched" {
  let (q, r) = divmod(17, 5);
  assert_eq(q, 3);
  assert_eq(r, 2);
  assert_eq(swap((1, true)), (true, 1));
  assert_eq(divmod(9, 2)._1, 1);
  assert_eq(divmod(9, 2), {_0: 4, _1: 1})
}

test "rendered as written" {
  assert_eq(int_to_string(len([(1, 2), (3, 4)])), "2")
}
"#,
    );
    for test in ["built and matched", "rendered as written"] {
        let index = compiled.index_of(test);
        if let Err(d) = compiled.machine().eval_test(index) {
            panic!("{test:?} must pass: {d:#?}");
        }
    }
}
