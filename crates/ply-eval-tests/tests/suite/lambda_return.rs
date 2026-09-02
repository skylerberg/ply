//! A lambda with a written return type: `?` inside it expands and exits the lambda, and the body
//! must fit the type.

use crate::fixture::Compiled;

#[test]
fn a_try_inside_a_lambda_with_a_written_return_type_runs() {
    let compiled = Compiled::new(
        r#"
fn parse_all(xs: List<Result<Int, String>>) -> Result<Int, String> = {
  let bump = |r: Result<Int, String>| -> Result<Int, String> { Ok(r? + 1) };
  fold(xs, Ok(0), |acc: Result<Int, String>, r: Result<Int, String>| -> Result<Int, String> {
    Ok(acc? + bump(r)?)
  })
}

test "all ok" { assert_eq(parse_all([Ok(1), Ok(2)]), Ok(5)) }

test "one err stops at the err" { assert_eq(parse_all([Ok(1), Err("no"), Ok(3)]), Err("no")) }
"#,
    );
    for test in ["all ok", "one err stops at the err"] {
        let index = compiled.index_of(test);
        if let Err(d) = compiled.machine().eval_test(index) {
            panic!("{test:?} must pass: {d:#?}");
        }
    }
}
