//! A unit entered at a root on a context of its own, with no front end: the answer as the bytes its
//! `R#entered` wrote, a failure as the entry's own, and what can never leave its run refused. Each
//! unit here is written by hand (`crate::units`), so no program is compiled.

use crate::units::{self, Def, def, int_entry, lifting, native};
use ply_compiled::entry::{self, Ended, Opened, Run};
use ply_core::codec::{Reader, Writer, tag};
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
                "{}  Word s = {}(ctx, (Word)(intptr_t)\"key\", 3, 0);\n  {}(ctx, s);\n  return 0;",
                lifting(""),
                native("?secret_of"),
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
        def("m.spawned", 0, "  return 0;"),
        def(
            "m.spawned#entered",
            0,
            &format!(
                "{}  Word t = {}(ctx, 0);\n  {}(ctx, t);\n  return 0;",
                lifting(""),
                native("task_made"),
                native("?out_handle"),
            ),
        ),
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
        "m.spawned",
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

/// The list a root's `R#entered` reads its arguments from, `n` of them, each written by `write`.
fn arguments(n: u64, write: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    w.count(tag::LIST, n);
    write(&mut w);
    w.finish()
}

/// The `Int`s `xs` as a root's arguments.
fn ints(xs: &[i64]) -> Vec<u8> {
    arguments(xs.len() as u64, |w| xs.iter().for_each(|x| w.int(*x)))
}

fn run() -> Run {
    Run::unbounded(10_000)
}

#[test]
fn a_root_that_takes_nothing_is_handed_the_empty_list() {
    assert_eq!(entry::NO_ARGUMENTS, ints(&[]));
}

/// The bytes `root` answered over `arguments`.
fn answered(opened: &Opened, root: &str, arguments: Vec<u8>, run: &Run) -> Result<Vec<u8>, String> {
    match entry::enter(opened, root, arguments, run, None, None).ended {
        Ended::Answered(bytes) => Ok(bytes),
        Ended::Failed(d) => Err(format!("failed: {d}")),
        Ended::Panicked(why) => Err(format!("panicked: {why}")),
        Ended::NoRoot => Err("no root".to_string()),
        Ended::Unanswerable(why) => Err(format!("unanswerable: {why}")),
    }
}

/// The `Int` the bytes of an answer are.
fn int_of(bytes: &[u8]) -> i64 {
    let mut r = Reader::new(bytes).expect("the answer opens with the format");
    r.expect(tag::INT).expect("an `Int`");
    let n = r.int().expect("an `Int`'s body");
    r.end().expect("the `Int` is the whole answer");
    n
}

/// What `root` answered over the `Int`s `xs`, read as the `Int` it answers.
fn int_answered(opened: &Opened, root: &str, xs: &[i64], run: &Run) -> Result<i64, String> {
    answered(opened, root, ints(xs), run).map(|bytes| int_of(&bytes))
}

/// A root is entered through its `R#entered`: the arguments cross in as their list's encoding, and
/// the answer out as the bytes the entry wrote, whatever its type.
#[test]
fn a_root_answers_the_bytes_its_entry_wrote() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    assert_eq!(int_answered(&opened, "m.double", &[21], &run()), Ok(42));
    let greeted = answered(
        &opened,
        "m.greet",
        arguments(1, |w| w.str("héllo".as_bytes())),
        &run(),
    )
    .expect("`m.greet` answers");
    let mut r = Reader::new(&greeted[..]).expect("the answer opens with the format");
    r.expect(tag::RECORD).unwrap();
    assert_eq!(r.count().unwrap(), 2);
    assert_eq!(r.symbol().unwrap().as_str(), "length");
    r.expect(tag::INT).unwrap();
    assert_eq!(r.int().unwrap(), 6);
    assert_eq!(r.symbol().unwrap().as_str(), "text");
    r.expect(tag::STR).unwrap();
    assert_eq!(r.str().unwrap(), "héllo");
    r.end().unwrap();
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
            int_answered(&opened, root, &[1], &run()),
            Err("no root".to_string()),
            "{root}"
        );
    }
    assert_eq!(opened.roots(), 7);
}

/// A function the program wrote, or a runtime object written as a handle, cannot leave the entry
/// that holds it.
#[test]
fn an_answer_holding_a_function_or_a_handle_stays_in_its_run() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    assert_eq!(
        answered(&opened, "m.closure", ints(&[]), &run()),
        Err("unanswerable: its answer holds a function the program wrote".to_string())
    );
    assert_eq!(
        answered(&opened, "m.keep", ints(&[]), &run()),
        Err("unanswerable: its answer holds a handle of its run".to_string())
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
        let raised = answered(&opened, "m.fail", arguments(1, |w| w.str(b"no")), &run())
            .expect_err("a panic raises");
        assert!(raised.contains("no"), "{raised}");
        assert_eq!(int_answered(&opened, "m.double", &[5], &run()), Ok(10));
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
        int_answered(&opened, "m.ladder", &[100], &shallow)
            .is_err_and(|why| why.contains("nested calls")),
    );
    assert_eq!(
        int_answered(&opened, "m.ladder", &[100], &run()),
        Ok(100),
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
    assert_eq!(int_answered(&opened, "m.double", &[8], &run()), Ok(16));
    assert_eq!(outer.heap.live(), 0);
    outer.end();
    assert_eq!(int_answered(&opened, "m.double", &[9], &run()), Ok(18));
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
                match entry::enter(&opened, "m.ladder", ints(&[100_000]), &deep, None, None).ended {
                    Ended::Answered(bytes) => Some(bytes),
                    _ => None,
                }
            })
        })
        .expect("a thread of little stack")
        .join()
        .expect("the asking thread ends");
    assert_eq!(asked.flatten().map(|bytes| int_of(&bytes)), Some(100_000));
}

/// A test's entry that answered says how many of its objects nothing let go of, and any other entry
/// counts none.
#[test]
fn a_tests_entry_counts_the_objects_it_left_live() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let defs = vec![
        def("m.kept", 1, "  rt_alloc_bytes_p(ctx, 16);\n  return p0;"),
        def("m.kept#entered", 0, &int_entry("m.kept")),
        def(
            "m.freed",
            1,
            "  Word b = rt_alloc_bytes_p(ctx, 16);\n  rt_free_p(ctx, b, 16);\n  return p0;",
        ),
        def("m.freed#entered", 0, &int_entry("m.freed")),
    ];
    let mut exports = units::exports_of(&defs);
    exports.entered = ["m.kept", "m.freed"].map(str::to_string).to_vec();
    let Some(lib) = units::library(dir.path(), &units::text(&defs, &exports)) else {
        return;
    };
    let opened = Opened::open(&lib).expect("the unit opens");
    let live = |root: &str, tested: bool| {
        let run = Run { tested, ..run() };
        let o = entry::enter(&opened, root, ints(&[1]), &run, None, None);
        assert!(matches!(o.ended, Ended::Answered(_)), "`{root}` answered");
        o.usage.live
    };
    assert_eq!([live("m.kept", true), live("m.freed", true)], [1, 0]);
    assert_eq!(
        live("m.kept", false),
        0,
        "an entry that is no test's counts nothing"
    );
}

/// A task in an entry's answer is refused at the boundary as a continuation is: the region it is a
/// key into ended before the entry did.
#[test]
fn a_task_in_an_entrys_answer_is_refused_at_the_boundary() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    match entry::enter(&opened, "m.spawned", ints(&[]), &run(), None, None).ended {
        Ended::Failed(d) => {
            assert_eq!(d.code, ply_core::codes::REGION_ESCAPE_AT_BOUNDARY, "{d}");
            assert!(d.message.contains("`m.spawned` answered a `Task`"), "{d}");
        }
        _ => panic!("the boundary did not refuse the task"),
    }
}
