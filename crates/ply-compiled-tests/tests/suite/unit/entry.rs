//! A unit entered at a root on a context of its own, with no front end: the answer as the bytes its
//! `R#entered` wrote, a failure as the entry's own, and what can never leave its run refused. Each
//! unit here is written by hand (`crate::units`), so no program is compiled.

use crate::units::{self, Def, def, int_entry, lifting, native};
use ply_compiled::entry::{self, Ended, Opened, Run};
use ply_core::codec;
use ply_core::{Symbol, Value};
use std::sync::Arc;

/// A string read from the lifting into `buf`, its length `n`.
pub fn string_read() -> String {
    format!(
        "  char buf[64];\n  Word n = {}(ctx);\n  if (ctx->failed || n > 64) return 0;\n  {}(ctx, (Word)(intptr_t)buf, n);",
        native("?in_str"),
        native("?taken"),
    )
}

/// The definitions the tests enter: each root `R` beside the `R#entered` it is entered through, and
/// `m.helper`, which has none. `m.ladder` nests a call `n` deep.
pub fn encoded_defs() -> Vec<Def> {
    let ladder = units::symbol("m.ladder");
    vec![
        def("m.double", 1, "  return ply_imm(ply_imm_value(p0) * 2);"),
        def("m.double#entered", 0, &int_entry("m.double")),
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
        def("m.ladder#entered", 0, &int_entry("m.ladder")),
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
pub fn entering(e: &mut units::Laid) {
    e.entered = [
        "m.double",
        "m.ladder",
        "m.greet",
        "m.closure",
        "m.keep",
        "m.fail",
    ]
    .map(str::to_string)
    .to_vec();
    e.builtins = vec![ply_core::Builtin::Panic];
}

/// The unit of [`encoded_defs`], opened with no front end; none where no C compiler is found.
fn opened(dir: &std::path::Path) -> Option<Arc<Opened>> {
    let defs = encoded_defs();
    let mut exports = units::exports_of(&defs);
    entering(&mut exports);
    let lib = units::library(dir, &units::text(&defs, &exports))?;
    Some(Opened::open(&lib).expect("the unit opens"))
}

/// `values` as the list a root's `R#entered` reads its arguments from.
fn arguments(values: Vec<Value>) -> Vec<u8> {
    codec::encode(&Value::list(values)).expect("the arguments are written")
}

fn run() -> Run {
    Run::unbounded(10_000)
}

/// What `root` answered over `values`, read back.
fn answered(opened: &Opened, root: &str, values: Vec<Value>, run: &Run) -> Result<Value, String> {
    match entry::enter(opened, root, arguments(values), run, None, None).ended {
        Ended::Answered(bytes) => Ok(codec::decode(&bytes).expect("the answer reads")),
        Ended::Failed(d) => Err(format!("failed: {d}")),
        Ended::Panicked(why) => Err(format!("panicked: {why}")),
        Ended::NoRoot => Err("no root".to_string()),
        Ended::Unanswerable(why) => Err(format!("unanswerable: {why}")),
    }
}

/// A root is entered through its `R#entered`: the arguments cross in as their list's encoding, and
/// the answer out as the bytes the entry wrote, whatever its type.
#[test]
fn a_root_answers_the_bytes_its_entry_wrote() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    assert_eq!(
        answered(&opened, "m.double", vec![Value::Int(21)], &run()),
        Ok(Value::Int(42))
    );
    assert_eq!(
        answered(&opened, "m.greet", vec![Value::str("héllo")], &run()),
        Ok(Value::Record(Arc::new(ply_core::Fields::from_unsorted(
            vec![
                (Symbol::new("length"), Value::Int(6)),
                (Symbol::new("text"), Value::str("héllo")),
            ]
        ))))
    );
}

/// Only a root the unit says it is entered at is entered, and an `R#entered` is no root of its own.
#[test]
fn a_unit_is_entered_only_at_its_roots() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    for root in ["m.helper", "m.double#entered", "m.nothing"] {
        assert_eq!(
            answered(&opened, root, vec![Value::Int(1)], &run()),
            Err("no root".to_string()),
            "{root}"
        );
    }
    assert_eq!(opened.roots(), 6);
}

/// A function the program wrote, or a handle, cannot leave the entry that holds it.
#[test]
fn an_answer_holding_a_function_or_a_handle_stays_in_its_run() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    assert!(
        answered(&opened, "m.closure", vec![], &run())
            .is_err_and(|why| why.starts_with("unanswerable")),
    );
}

/// A raise inside an entry is the entry's alone, whatever crossings it left open, and the next entry
/// of the unit runs as if it had not happened.
#[test]
fn a_raise_ends_its_entry_and_no_other() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    for _ in 0..2 {
        let raised = answered(&opened, "m.fail", vec![Value::str("no")], &run())
            .expect_err("a panic raises");
        assert!(raised.contains("no"), "{raised}");
        assert_eq!(
            answered(&opened, "m.double", vec![Value::Int(5)], &run()),
            Ok(Value::Int(10))
        );
    }
}

/// How deeply the entry's calls may nest is the run's, and a call past it fails the entry.
#[test]
fn a_recursion_past_the_runs_depth_fails_rather_than_running_on() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    let shallow = Run::unbounded(8);
    assert!(
        answered(&opened, "m.ladder", vec![Value::Int(100)], &shallow)
            .is_err_and(|why| why.contains("nested calls")),
    );
    assert_eq!(
        answered(&opened, "m.ladder", vec![Value::Int(100)], &run()),
        Ok(Value::Int(100)),
        "the same call answers on a depth that fits it, so the failure above says nothing else"
    );
}

/// An entry may begin while another runs on the same thread: it runs on a context of its own and
/// leaves the outer one's as it found it.
#[test]
fn an_entry_inside_another_runs_on_a_context_of_its_own() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    let mut outer = crate::bare::context();
    outer.begin(1_000);
    assert_eq!(
        answered(&opened, "m.double", vec![Value::Int(8)], &run()),
        Ok(Value::Int(16))
    );
    assert_eq!(outer.heap.live(), 0);
    outer.end();
    assert_eq!(
        answered(&opened, "m.double", vec![Value::Int(9)], &run()),
        Ok(Value::Int(18))
    );
}

/// An entry on a program thread nests as deeply as its run allows, however little stack the thread
/// that asks for it has: 100,000 nested calls from a thread of 256 KiB.
#[test]
fn an_entry_on_a_program_thread_nests_past_its_callers_stack() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    let deep = Run::unbounded(200_000);
    let asked = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            entry::on_program_thread(|| {
                let arguments = arguments(vec![Value::Int(100_000)]);
                match entry::enter(&opened, "m.ladder", arguments, &deep, None, None).ended {
                    Ended::Answered(bytes) => Some(bytes),
                    _ => None,
                }
            })
        })
        .expect("a thread of little stack")
        .join()
        .expect("the asking thread ends");
    let answer = asked
        .flatten()
        .map(|bytes| codec::decode(&bytes).expect("the answer reads"));
    assert_eq!(answer, Some(Value::Int(100_000)));
}
