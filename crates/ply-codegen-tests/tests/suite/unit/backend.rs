//! The seam a machine enters a unit through: what it declines, what it counts, and the program it
//! describes. Each unit here is written by hand (`crate::units`), so no program is compiled.

use crate::units::{self, Def, def};
use ply_codegen::{Bodies, Unit};
use ply_eval::{Analysis, Compiled, DefHash, Machine, Provider, Span, Symbol, Value, codes};
use std::rc::Rc;
use std::sync::Arc;

/// The definitions every test here enters: arithmetic, a nested recursion `n` deep, a raise of
/// its argument's text, a nullary answer, and a `Secret` minted from its argument's text.
fn defs() -> Vec<Def> {
    let ladder = units::symbol("m.ladder");
    vec![
        def("m.double", 1, "  return ply_imm(ply_imm_value(p0) * 2);"),
        def(
            "m.ladder",
            1,
            &format!(
                "  if (ctx->fuel <= 0) {{ rt_no_fuel_p(ctx); return 0; }}\n\
                 \x20 ctx->fuel -= 1;\n\
                 \x20 int64_t n = ply_imm_value(p0);\n\
                 \x20 Word r = ply_imm(0);\n\
                 \x20 if (n > 0) {{ r = {ladder}(ctx, ply_imm(n - 1)); if (ctx->failed) return 0; r = ply_imm(ply_imm_value(r) + 1); }}\n\
                 \x20 ctx->fuel += 1;\n\
                 \x20 return r;"
            ),
        ),
        def("m.fail", 1, "  builtin_panic_p(ctx, p0);\n  return 0;"),
        def("m.seeded", 0, "  return ply_imm(80);"),
        def(
            "m.keyed",
            1,
            "  return builtin_secret_of_string_p(ctx, p0);",
        ),
    ]
}

/// The unit over [`defs`], opened for a program whose hashes digest to `program`'s.
fn opened(dir: &std::path::Path, program: &[u8]) -> Option<(&'static Analysis, Arc<Unit>)> {
    let defs = defs();
    let lib = units::library(dir, &units::text(&defs, &units::exports_of(&defs)))?;
    let front: &'static Analysis = Box::leak(Box::new(units::front(program, &defs, &[])));
    Some((front, Unit::opened(front, &lib).expect("the unit opens")))
}

fn bodies(unit: &Arc<Unit>) -> Rc<Bodies> {
    unit.bodies().expect("the unit builds")
}

fn enter(bodies: &Bodies, name: &str, args: &[Value], budget: usize) -> Option<Value> {
    Compiled::enter(bodies, &Symbol::new(name), args, budget)
}

#[test]
fn a_name_the_unit_has_no_body_for_is_declined() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let b = bodies(&unit);
    assert_eq!(enter(&b, "m.absent", &[Value::Int(1)], 10_000), None);
    assert_eq!(
        enter(&b, "m.double", &[Value::Int(21)], 10_000),
        Some(Value::Int(42))
    );
    assert_eq!(b.declines().not_compiled, 1, "{:?}", b.declines());
}

#[test]
fn a_call_of_the_wrong_arity_is_declined_and_counted() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let b = bodies(&unit);
    assert_eq!(enter(&b, "m.double", &[], 10_000), None);
    assert_eq!(
        enter(
            &b,
            "m.double",
            &[Value::Int(1), Value::Int(2), Value::Int(3)],
            10_000
        ),
        None
    );
    assert_eq!(b.declines().arity, 2, "{:?}", b.declines());
}

/// `budget` is the machine's remaining nested calls and not a hint.
#[test]
fn a_recursion_past_the_budget_raises_rather_than_running_on() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let b = bodies(&unit);
    assert_eq!(enter(&b, "m.ladder", &[Value::Int(100)], 8), None);
    assert_eq!(
        enter(&b, "m.ladder", &[Value::Int(100)], 10_000),
        Some(Value::Int(100)),
        "the same call fails on a budget that fits it, so the failure above says nothing"
    );
}

/// A bisection builds programs whose definitions reuse the names they replace, so a backend
/// answering for a name alone would answer for the wrong body.
#[test]
fn a_backend_describes_only_the_program_it_was_built_from() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let backend = Arc::clone(&unit).attach();
    assert!(backend.describes(front.hashes_digest));
    assert!(!backend.describes(DefHash::of(b"another program")));
}

/// A `Secret` never crosses out as a value: the entry is declined and counted as an answer that
/// could not cross.
#[test]
fn a_secret_never_leaves_the_entry_that_minted_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let b = bodies(&unit);
    assert!(b.admits("m.keyed"));
    assert_eq!(enter(&b, "m.keyed", &[Value::str("hunter2")], 10_000), None);
    assert_eq!(b.declines().answer, 1, "{:?}", b.declines());
}

/// A failure is the entry's alone: the context the next entry runs on holds nothing of it.
#[test]
fn a_failed_entry_does_not_poison_the_one_after_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let mut machine = Machine::new(front, bodies(&unit)).expect("the unit describes the program");
    for _ in 0..2 {
        let raised = machine
            .call("m.fail", vec![Value::str("no")], Span::DUMMY)
            .into_parts()
            .0
            .expect_err("a panic raises");
        assert!(raised.message.contains("no"), "{raised}");
        let answered = |m: &mut Machine<'_>, name: &str, args: Vec<Value>| {
            m.call(name, args, Span::DUMMY).into_parts().0
        };
        assert_eq!(
            answered(&mut machine, "m.seeded", vec![]).ok(),
            Some(Value::Int(80))
        );
        assert_eq!(
            answered(&mut machine, "m.double", vec![Value::Int(5)]).ok(),
            Some(Value::Int(10))
        );
    }
}

/// `Ctx` is one flat frame, so a nested entry would alias the outer one's words.
#[test]
fn an_entry_that_arrives_while_another_is_running_is_declined_and_reported() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, unit)) = opened(dir.path(), b"p") else {
        return;
    };
    let b = bodies(&unit);
    let mut machine = Machine::new(front, b.clone()).expect("the unit describes the program");
    let call = |m: &mut Machine<'_>| {
        m.call("m.double", vec![Value::Int(8)], Span::DUMMY)
            .into_parts()
            .0
    };
    assert_eq!(call(&mut machine).ok(), Some(Value::Int(16)));
    b.reset_counts();
    let before = b.entered();
    let inside = b.while_entered(|| call(&mut machine));
    assert_eq!(
        b.entered(),
        before,
        "an entry was taken while the context was held"
    );
    assert_eq!(b.declines().reentered, 1, "{:?}", b.declines());
    let declined = inside.expect_err("a reentrant entry is declined, not served");
    assert_eq!(
        declined.code,
        codes::INTERNAL_ERROR,
        "a decline is Ply's own defect, never the program's runtime error: {declined}"
    );
    assert_eq!(
        declined.message,
        "the C backend declined to enter `m.double`"
    );
    assert_eq!(call(&mut machine).ok(), Some(Value::Int(16)));
    assert_eq!(b.entered(), before + 1);
}

/// A constant's answer crosses back in as memo words, so a pure root entered over it again is
/// answered from the memo and runs nothing; a root that is not pure runs every time it is entered.
#[test]
fn a_pure_root_over_memo_words_is_answered_from_the_memo() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let counted = "  ctx->ticks += 1;\n  return ply_imm(7);";
    let defs = vec![
        def(
            "m.table",
            0,
            "  ctx->ticks += 1;\n  return builtin_int_to_string_p(ctx, ply_imm(123));",
        ),
        def("m.weigh", 1, counted),
        def("m.post", 1, counted),
    ];
    let mut exports = units::exports_of(&defs);
    exports.constants = vec![("m.table".to_string(), None)];
    exports.lambdas = vec![format!("{}_entry", units::symbol("m.table"))];
    let Some(lib) = units::library(dir.path(), &units::text(&defs, &exports)) else {
        return;
    };
    let front: &'static Analysis =
        Box::leak(Box::new(units::front(b"p", &defs, &["m.table", "m.weigh"])));
    let unit = Unit::opened(front, &lib).expect("the unit opens");
    let b = bodies(&unit);
    let mut machine = Machine::new(front, b.clone()).expect("the unit describes the program");
    let mut call = |name: &str, args: Vec<Value>| {
        let answer = machine
            .call(name, args, Span::DUMMY)
            .into_parts()
            .0
            .unwrap_or_else(|d| panic!("`{name}` raised: {d}"));
        (answer, b.steps())
    };
    let (table, made) = call("m.table", vec![]);
    assert_eq!(table, Value::str("123"));
    assert!(made > 0, "the constant's first entry did not run its body");
    assert_eq!(call("m.table", vec![]).1, 0, "the constant ran again");

    let (first, ran) = call("m.weigh", vec![table.clone()]);
    assert!(ran > 0, "the first entry did not run the body");
    let (second, again) = call("m.weigh", vec![table.clone()]);
    assert_eq!(second, first);
    assert_eq!(again, 0, "the second entry ran the body again");

    for entry in 1..=2 {
        assert!(
            call("m.post", vec![table.clone()]).1 > 0,
            "entry {entry} of a root that is not pure was answered by an earlier one"
        );
    }
}
