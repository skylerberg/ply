//! Exercises the contract entry point on real source: parse, check, evaluate.

use ply_core::check_module;
use ply_eval::Interp;
use ply_span::SourceId;
use ply_syntax::parse;

const SOURCE: &str = r#"
effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Unit
}

fn total(keys: List<Int>) -> Int =
  fold(keys, 0, |acc, k| acc + db.get[users](k))

test "a cell-backed handler stands in for the database" {
  with_cell[users](7) { cell ->
    handle {
      assert_eq(total([1, 2]), 14)
    } with {
      db.get[users](k) -> cell_get(cell),
      db.put[users](k, v) -> cell_set(cell, v)
    }
  }
}

test "a failing assertion is reported, not swallowed" {
  assert_eq(1 + 1, 3)
}
"#;

fn interp_source() -> (ply_syntax::ast::Module, ply_core::CheckOutput) {
    let module = parse(SourceId(0), SOURCE).expect("the sample program should parse");
    let check = check_module(&module).expect("the sample program should typecheck");
    (module, check)
}

#[test]
fn a_handled_effect_evaluates_through_the_checked_module() {
    let (module, check) = interp_source();
    let mut interp = Interp::new(&module, &check);
    assert_eq!(interp.test_count(), 2);
    interp.eval_test(0).expect("the handled test should pass");
}

#[test]
fn a_failing_test_yields_an_assertion_diagnostic() {
    let (module, check) = interp_source();
    let mut interp = Interp::new(&module, &check);
    let diag = interp.eval_test(1).expect_err("the test should fail");
    assert_eq!(diag.code, ply_span::codes::ASSERTION_FAILED);
    assert_eq!(diag.message, "assertion failed: expected 3, found 2");
    assert!(
        !diag.primary_span().unwrap().is_dummy(),
        "the failure must point at real source"
    );
}
