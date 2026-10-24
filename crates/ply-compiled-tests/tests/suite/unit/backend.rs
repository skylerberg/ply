//! The seam a machine enters a unit through: what it declines, what it counts, and the program it
//! describes. Each unit here is written by hand (`crate::units`), so no program is compiled.

use crate::units::{self, Def, def};
use ply_compiled::{Bodies, Unit};
use ply_core::{Analysis, Compiled, DefHash, Machine, Provider, Span, Symbol, Value, codes};
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
    exports.constants = vec!["m.table".to_string()];
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

/// The C pointer a unit calls the native the compiler declares as `name` through.
fn native(name: &str) -> String {
    ply_compiled::c::pointer_name(&ply_compiled::c::builtin_helper_name(name))
}

/// The C that opens an `R#entered`: the arguments' list read, `read` reading each, and the
/// lifting ended, returning where the context failed.
fn lifting(read: &str) -> String {
    format!(
        "  {}(ctx, {});\n{read}\n  {}(ctx);\n  if (ctx->failed) return 0;\n",
        native("?in_count"),
        ply_core::codec::tag::LIST,
        native("?lift_close"),
    )
}

/// A string read from the lifting into `buf`, its length `n`.
fn string_read() -> String {
    format!(
        "  char buf[64];\n  Word n = {}(ctx);\n  if (ctx->failed || n > 64) return 0;\n  {}(ctx, (Word)(intptr_t)buf, n);",
        native("?in_str"),
        native("?taken"),
    )
}

/// An encoded unit's definitions: each root `R` beside the `R#entered` it is entered through, and
/// `m.helper`, which has none.
fn encoded_defs() -> Vec<Def> {
    let double = units::symbol("m.double");
    vec![
        def("m.double", 1, "  return ply_imm(ply_imm_value(p0) * 2);"),
        def(
            "m.double#entered",
            0,
            &format!(
                "{}  Word r = {double}(ctx, ply_imm(n));\n  if (ctx->failed) return 0;\n  {}(ctx, ply_imm_value(r));\n  return 0;",
                lifting(&format!("  Word n = {}(ctx);", native("?in_int"))),
                native("?out_int"),
            ),
        ),
        def("m.greet", 1, "  return p0;"),
        def(
            "m.greet#entered",
            0,
            &format!(
                "{}  {}(ctx, {}, 2);\n  {}(ctx, (Word)(intptr_t)\"length\", 6);\n  {}(ctx, n);\n  {}(ctx, (Word)(intptr_t)\"text\", 4);\n  {}(ctx, (Word)(intptr_t)buf, n);\n  return 0;",
                lifting(&string_read()),
                native("?out_count"),
                ply_core::codec::tag::RECORD,
                native("?out_name"),
                native("?out_int"),
                native("?out_name"),
                native("?out_str"),
            ),
        ),
        def("m.closure", 0, "  return 0;"),
        def(
            "m.closure#entered",
            0,
            &format!(
                "{}  {}(ctx, 0, (Word)(intptr_t)\"m.f\", 3);\n  return 0;",
                lifting(""),
                native("?out_fun"),
            ),
        ),
        def("m.keep", 1, "  return p0;"),
        def(
            "m.keep#entered",
            0,
            &format!(
                "{}  {}(ctx, s);\n  return 0;",
                lifting(&format!("  Word s = {}(ctx);", native("?in_handle"))),
                native("?out_handle"),
            ),
        ),
        def("m.fail", 1, "  return p0;"),
        def(
            "m.fail#entered",
            0,
            &format!(
                "{}  {}(ctx, 0);\n  {}(ctx, {}, 1);\n  {}(ctx, (Word)(intptr_t)buf, n);\n  rt_builtin_encoded_p(ctx, 0);\n  return 0;",
                lifting(&string_read()),
                native("?lower_open"),
                native("?out_count"),
                ply_core::codec::tag::LIST,
                native("?out_str"),
            ),
        ),
        def("m.helper", 1, "  return p0;"),
    ]
}

/// The roots of [`encoded_defs`] each entered through its `R#entered`.
fn entering(e: &mut units::Laid) {
    e.entered = ["m.double", "m.greet", "m.closure", "m.keep", "m.fail"]
        .map(str::to_string)
        .to_vec();
    e.builtins = vec![ply_core::Builtin::Panic];
}

/// The unit over `defs`, its table as `adjust` leaves it, opened for a program of them.
fn opened_with(
    dir: &std::path::Path,
    defs: &[Def],
    pure: &[&str],
    adjust: impl FnOnce(&mut units::Laid),
) -> Option<(&'static Analysis, Arc<Unit>)> {
    let mut exports = units::exports_of(defs);
    adjust(&mut exports);
    let lib = units::library(dir, &units::text(defs, &exports))?;
    let front: &'static Analysis = Box::leak(Box::new(units::front(b"p", defs, pure)));
    Some((front, Unit::opened(front, &lib).expect("the unit opens")))
}

/// An encoded unit's root is entered through its `R#entered`: the arguments cross in as their list's
/// encoding and the answer out as its own, whatever its type.
#[test]
fn an_encoded_units_roots_are_entered_through_their_entered_roots() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened_with(dir.path(), &encoded_defs(), &[], entering) else {
        return;
    };
    let b = bodies(&unit);
    assert_eq!(
        enter(&b, "m.double", &[Value::Int(21)], 10_000),
        Some(Value::Int(42))
    );
    let greeted = enter(&b, "m.greet", &[Value::str("héllo")], 10_000).expect("it answers");
    assert_eq!(
        greeted,
        Value::Record(Arc::new(ply_core::Fields::from_unsorted(vec![
            (Symbol::new("length"), Value::Int(6)),
            (Symbol::new("text"), Value::str("héllo")),
        ])))
    );
    assert_eq!(b.declines().total(), 0, "{:?}", b.declines());
}

/// Only a root with an `R#entered` is offered, and no `R#entered` is offered as a root of its own.
#[test]
fn an_encoded_unit_offers_only_the_roots_it_is_entered_at() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened_with(dir.path(), &encoded_defs(), &[], entering) else {
        return;
    };
    let b = bodies(&unit);
    assert!(b.admits("m.double"));
    assert!(!b.admits("m.helper"));
    assert!(!b.admits("m.double#entered"));
    assert_eq!(enter(&b, "m.helper", &[Value::Int(1)], 10_000), None);
    assert_eq!(b.declines().not_compiled, 1, "{:?}", b.declines());
}

/// A function the program wrote, or a handle, cannot leave the entry: the entry is declined and
/// counted as an answer that could not cross, as a unit entered with words is.
#[test]
fn an_encoded_answer_holding_a_function_or_a_handle_is_declined() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((_, unit)) = opened_with(dir.path(), &encoded_defs(), &[], entering) else {
        return;
    };
    let b = bodies(&unit);
    assert_eq!(enter(&b, "m.closure", &[], 10_000), None);
    let secret = Value::secret_text("hunter2");
    assert_eq!(enter(&b, "m.keep", &[secret], 10_000), None);
    assert_eq!(b.declines().answer, 2, "{:?}", b.declines());
}

/// A raise inside an encoded entry is the entry's alone, whatever crossings it left open.
#[test]
fn a_raise_in_an_encoded_entry_does_not_poison_the_one_after_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, unit)) = opened_with(dir.path(), &encoded_defs(), &[], entering) else {
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
        let doubled = machine
            .call("m.double", vec![Value::Int(5)], Span::DUMMY)
            .into_parts()
            .0;
        assert_eq!(doubled.ok(), Some(Value::Int(10)));
    }
}

/// A unit that says it is entered at a root through an `R#entered` it does not take, or one that
/// takes words, is refused by that root.
#[test]
fn a_unit_entered_through_what_it_does_not_take_is_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &encoded_defs(), |e| {
        entering(e);
        e.entered.push("m.helper".to_string());
    }) else {
        return;
    };
    let refused = loaded
        .err()
        .expect("a root with no `R#entered` was entered");
    assert!(refused.contains("`m.helper#entered`"), "{refused}");
    let dir = tempfile::tempdir().expect("a scratch directory");
    let mut defs = encoded_defs();
    defs.push(def("m.helper#entered", 1, "  return p0;"));
    let Some(loaded) = units::opened(dir.path(), &defs, |e| {
        entering(e);
        e.entered.push("m.helper".to_string());
    }) else {
        return;
    };
    let refused = loaded
        .err()
        .expect("an `R#entered` taking a word was bound");
    assert!(refused.contains("takes 1 words"), "{refused}");
}

/// An encoded unit's memo keeps values: a pure constant runs once, a pure root over what the memo
/// answered runs once for those arguments, and a root that is not pure runs every time.
#[test]
fn an_encoded_pure_root_over_what_the_memo_answered_is_answered_from_the_memo() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let counted = format!(
        "{}  ctx->ticks += 1;\n  {}(ctx, 7);\n  return 0;",
        lifting(&string_read()),
        native("?out_int"),
    );
    let defs = vec![
        def("m.table", 0, "  return 0;"),
        def(
            "m.table#entered",
            0,
            &format!(
                "{}  ctx->ticks += 1;\n  {}(ctx, (Word)(intptr_t)\"123\", 3);\n  return 0;",
                lifting(""),
                native("?out_str"),
            ),
        ),
        def("m.weigh", 1, "  return p0;"),
        def("m.weigh#entered", 0, &counted),
        def("m.post", 1, "  return p0;"),
        def("m.post#entered", 0, &counted),
    ];
    let Some((front, unit)) = opened_with(dir.path(), &defs, &["m.table", "m.weigh"], |e| {
        e.entered = ["m.table", "m.weigh", "m.post"]
            .map(str::to_string)
            .to_vec();
        e.constants = vec!["m.table".to_string()];
        e.lambdas = vec![format!("{}_entry", units::symbol("m.table"))];
    }) else {
        return;
    };
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
    assert_eq!(
        call("m.table", vec![]),
        (table.clone(), 0),
        "the constant ran again"
    );

    let (first, ran) = call("m.weigh", vec![table.clone()]);
    assert_eq!(first, Value::Int(7));
    assert!(ran > 0, "the first entry did not run the body");
    assert_eq!(
        call("m.weigh", vec![table.clone()]),
        (first, 0),
        "the second entry ran the body again"
    );
    assert!(
        call("m.weigh", vec![Value::str("123")]).1 > 0,
        "an argument the memo did not answer was answered from it"
    );
    for entry in 1..=2 {
        assert!(
            call("m.post", vec![table.clone()]).1 > 0,
            "entry {entry} of a root that is not pure was answered by an earlier one"
        );
    }
}
