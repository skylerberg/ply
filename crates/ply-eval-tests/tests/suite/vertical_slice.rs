use crate::fixture::Compiled;

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

fn single() -> Compiled {
    Compiled::modules(&[("m", SOURCE)])
}

#[test]
fn a_handled_effect_evaluates_through_the_checked_module() {
    let compiled = single();
    let mut machine = compiled.machine();
    assert_eq!(machine.test_count(), 2);
    machine.eval_test(0).expect("the handled test should pass");
}

/// Effect, perform and handler sit in three modules that each spell the effect differently.
#[test]
fn a_handler_discharges_an_effect_declared_in_another_module() {
    let compiled = Compiled::modules(&[
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
    assert_eq!(compiled.front.check.tests.len(), 1);
    assert_eq!(
        compiled.front.check.tests[0].key.as_str(),
        "app.the imported effect is handled here"
    );
    compiled
        .machine()
        .eval_test(0)
        .expect("the cross-module handler should discharge `store.db`");
}

#[test]
fn a_handler_clause_body_resolves_where_the_handler_was_written() {
    let compiled = Compiled::modules(&[
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
        .machine()
        .eval_test(0)
        .expect("the clause body must resolve `fixture` in `app`, not in `store`");
}

#[test]
fn same_named_definitions_in_two_modules_do_not_collide() {
    let compiled = Compiled::modules(&[
        (
            "alpha",
            "pub fn answer() -> Int = 1\npub fn wrapped() -> Int = answer()\n",
        ),
        (
            "beta",
            "pub fn answer() -> Int = 2\npub fn wrapped() -> Int = answer()\n",
        ),
    ]);
    let mut machine = compiled.machine();
    let at = ply_span::Span::DUMMY;
    assert_eq!(
        machine
            .call("alpha.wrapped", Vec::new(), at)
            .unwrap()
            .render(),
        "1"
    );
    assert_eq!(
        machine
            .call("beta.wrapped", Vec::new(), at)
            .unwrap()
            .render(),
        "2"
    );
}

#[test]
fn constructors_from_two_modules_are_distinct_values() {
    let compiled = Compiled::modules(&[
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
    let mut machine = compiled.machine();
    let at = ply_span::Span::DUMMY;
    assert_eq!(
        machine
            .call("beta.theirs", Vec::new(), at)
            .unwrap()
            .render(),
        "1"
    );
    assert_eq!(
        machine.call("beta.mine", Vec::new(), at).unwrap().render(),
        "2"
    );
    assert_eq!(
        machine.call("alpha.make", Vec::new(), at).unwrap().render(),
        "alpha.Wrapped(1)"
    );
}
