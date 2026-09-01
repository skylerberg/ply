//! Exercises the contract entry point on real source: parse, resolve, check, evaluate.

use ply_core::{CheckOutput, check_program};
use ply_eval::Interp;
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

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

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(files: &[(&str, &str)]) -> Compiled {
        let inputs: Vec<_> = files
            .iter()
            .enumerate()
            .map(|(i, (name, text))| (SourceId(i as u32), ModuleName::from_dotted(name), *text))
            .collect();
        let mut program = ply_syntax::parse_program(inputs).expect("the sample should parse");
        let resolved = resolve(&mut program).expect("the sample should resolve");
        let check = check_program(&program, &resolved).expect("the sample should typecheck");
        Compiled {
            program,
            resolved,
            check,
        }
    }

    fn interp(&self) -> Interp<'_> {
        Interp::new(&self.program, &self.resolved, &self.check)
    }
}

fn single() -> Compiled {
    Compiled::new(&[("m", SOURCE)])
}

#[test]
fn a_handled_effect_evaluates_through_the_checked_module() {
    let compiled = single();
    let mut interp = compiled.interp();
    assert_eq!(interp.test_count(), 2);
    interp.eval_test(0).expect("the handled test should pass");
}

#[test]
fn a_failing_test_yields_an_assertion_diagnostic() {
    let compiled = single();
    let diag = compiled
        .interp()
        .eval_test(1)
        .expect_err("the test should fail");
    assert_eq!(diag.code, ply_span::codes::ASSERTION_FAILED);
    assert_eq!(diag.message, "assertion failed: expected 3, found 2");
    assert!(
        !diag.primary_span().unwrap().is_dummy(),
        "the failure must point at real source"
    );
}

/// The effect, the code that performs it and the handler that discharges it are each in a different
/// module and spell the effect differently.
#[test]
fn a_handler_discharges_an_effect_declared_in_another_module() {
    let compiled = Compiled::new(&[
        (
            "store",
            "pub effect db {\n  read get[r](key: Int) -> Int\n}\n\
             pub fn total(keys: List<Int>) -> Int / {db.read[users]} =\n\
             \x20 fold(keys, 0, |acc, k| acc + db.get[users](k))\n",
        ),
        (
            "app",
            "import store\n\
             import store (db)\n\
             test \"the imported effect is handled here\" {\n\
             \x20 handle {\n\
             \x20   assert_eq(store::total([1, 2, 3]), 21)\n\
             \x20 } with {\n\
             \x20   db.get[users](k) -> 7,\n\
             \x20 }\n\
             }\n",
        ),
    ]);
    assert_eq!(compiled.check.tests.len(), 1);
    assert_eq!(
        compiled.check.tests[0].key.as_str(),
        "app.the imported effect is handled here"
    );
    compiled
        .interp()
        .eval_test(0)
        .expect("the cross-module handler should discharge `store.db`");
}

/// A clause body runs when a perform deep inside another module reaches it, but its bare names
/// still mean what they meant where the `handle` was written.
#[test]
fn a_handler_clause_body_resolves_where_the_handler_was_written() {
    let compiled = Compiled::new(&[
        (
            "store",
            "pub effect db {\n  read all[t]() -> Int\n}\n\
             pub fn fixture() -> Int = 1\n\
             pub fn reading() -> Int / {db.read[users]} = db.all[users]() + fixture()\n",
        ),
        (
            "app",
            "import store\n\
             import store (db)\n\
             fn fixture() -> Int = 100\n\
             test \"the clause body sees `app.fixture`\" {\n\
             \x20 handle {\n\
             \x20   assert_eq(store::reading(), 101)\n\
             \x20 } with {\n\
             \x20   db.all[users]() -> fixture(),\n\
             \x20 }\n\
             }\n",
        ),
    ]);
    compiled
        .interp()
        .eval_test(0)
        .expect("the clause body must resolve `fixture` in `app`, not in `store`");
}

/// Two modules declaring the same simple names is the case a flat global table would silently get
/// wrong, so each definition has to reach its own.
#[test]
fn same_named_definitions_in_two_modules_do_not_collide() {
    let compiled = Compiled::new(&[
        (
            "alpha",
            "pub fn answer() -> Int = 1\npub fn wrapped() -> Int = answer()\n",
        ),
        (
            "beta",
            "pub fn answer() -> Int = 2\npub fn wrapped() -> Int = answer()\n",
        ),
    ]);
    let mut interp = compiled.interp();
    let at = ply_span::Span::DUMMY;
    assert_eq!(
        interp
            .call("alpha.wrapped", Vec::new(), at)
            .unwrap()
            .render(),
        "1"
    );
    assert_eq!(
        interp
            .call("beta.wrapped", Vec::new(), at)
            .unwrap()
            .render(),
        "2"
    );
}

/// A constructor's identity is its program-wide name: two modules may each declare a `Wrapped`, and
/// a value built by one must not match the other's pattern.
#[test]
fn constructors_from_two_modules_are_distinct_values() {
    let compiled = Compiled::new(&[
        (
            "alpha",
            "pub type A = Wrapped(Int)\npub fn make() -> A = Wrapped(1)\n",
        ),
        (
            "beta",
            "import alpha\n\
             type B = Wrapped(Int)\n\
             pub fn theirs() -> Int = match alpha::make() { alpha::Wrapped(n) -> n }\n\
             pub fn mine() -> Int = match Wrapped(2) { Wrapped(n) -> n }\n",
        ),
    ]);
    let mut interp = compiled.interp();
    let at = ply_span::Span::DUMMY;
    assert_eq!(
        interp.call("beta.theirs", Vec::new(), at).unwrap().render(),
        "1"
    );
    assert_eq!(
        interp.call("beta.mine", Vec::new(), at).unwrap().render(),
        "2"
    );
    assert_eq!(
        interp.call("alpha.make", Vec::new(), at).unwrap().render(),
        "alpha.Wrapped(1)"
    );
}
