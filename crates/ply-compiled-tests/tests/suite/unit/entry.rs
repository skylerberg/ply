use crate::units::{self, Def, def, int_entry, lifting, native};
use ply_compiled::entry::{self, Ended, Opened, Run, overdue_lines};
use ply_core::codec::{Reader, Writer, tag};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn string_read() -> String {
    format!(
        "  char buf[64];\n  Word n = {}(ctx);\n  if (ctx->failed || n > 64) return 0;\n  {}(ctx, (Word)(intptr_t)buf, n);",
        native("?in_str"),
        native("?taken"),
    )
}

pub fn encoded_defs() -> Vec<Def> {
    let ladder = units::symbol("m.ladder");
    vec![
        def("m.double", 1, "  return ply_imm(ply_imm_value(p0) * 2);"),
        def("m.double#entered", 0, &int_entry("m.double")),
        def(
            "m.ladder",
            1,
            &format!(
                "  int64_t n = ply_imm_value(p0);\n\
                 \x20 Word r = ply_imm(0);\n\
                 \x20 if (n > 0) {{ r = {ladder}(ctx, ply_imm(n - 1)); if (ctx->failed) return 0; r = ply_imm(ply_imm_value(r) + 1); }}\n\
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
                "{}  Word texts[] = {{ (Word)(intptr_t)\"E0501\", 5, (Word)(intptr_t)\"assertion failed: condition is false\", 36, (Word)(intptr_t)\"here\", 4, (Word)(intptr_t)buf, n }};\n  rt_failed_p(ctx, 1, (Word)(intptr_t)texts, 1, 0);\n  return 0;",
                lifting(&string_read()),
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
}

fn opened(dir: &std::path::Path) -> Option<Arc<Opened>> {
    let defs = encoded_defs();
    let mut exports = units::exports_of(&defs);
    entering(&mut exports);
    let lib = units::library(dir, &units::text(&defs, &exports))?;
    Some(Opened::open(&lib).expect("the unit opens"))
}

fn arguments(n: u64, write: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    w.count(tag::LIST, n);
    write(&mut w);
    w.finish()
}

fn ints(xs: &[i64]) -> Vec<u8> {
    arguments(xs.len() as u64, |w| xs.iter().for_each(|x| w.int(*x)))
}

fn run() -> Run {
    Run::unbounded()
}

#[test]
fn the_argument_list_passed_to_an_entry_function_that_takes_no_arguments_is_empty() {
    assert_eq!(entry::NO_ARGUMENTS, ints(&[]));
}

fn returned(opened: &Opened, root: &str, arguments: Vec<u8>, run: &Run) -> Result<Vec<u8>, String> {
    match entry::enter(opened, root, arguments, run, None, None).ended {
        Ended::Returned(bytes) => Ok(bytes),
        Ended::Failed(d) => Err(format!("failed: {d}")),
        Ended::Panicked(reason) => Err(format!("panicked: {reason}")),
        Ended::NoRoot => Err("no root".to_string()),
        Ended::NotReturnable(reason) => Err(format!("not returnable: {reason}")),
    }
}

fn int_of(bytes: &[u8]) -> i64 {
    let mut r = Reader::new(bytes).expect("the result opens with the format");
    r.expect(tag::INT).expect("an `Int`");
    let n = r.int().expect("an `Int`'s body");
    r.end().expect("the `Int` is the whole result");
    n
}

fn int_returned(opened: &Opened, root: &str, xs: &[i64], run: &Run) -> Result<i64, String> {
    returned(opened, root, ints(xs), run).map(|bytes| int_of(&bytes))
}

#[test]
fn calling_an_exported_function_returns_its_result_as_encoded_bytes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    assert_eq!(int_returned(&opened, "m.double", &[21], &run()), Ok(42));
    let greeted = returned(
        &opened,
        "m.greet",
        arguments(1, |w| w.str("héllo".as_bytes())),
        &run(),
    )
    .expect("`m.greet` returns");
    let mut r = Reader::new(&greeted[..]).expect("the result opens with the format");
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

#[test]
fn calling_a_function_that_is_not_exported_fails_with_a_no_root_error() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    for root in ["m.helper", "m.double#entered", "m.nothing"] {
        assert_eq!(
            int_returned(&opened, root, &[1], &run()),
            Err("no root".to_string()),
            "{root}"
        );
    }
    assert_eq!(opened.roots(), 7);
}

#[test]
fn a_result_containing_a_function_or_a_runtime_handle_is_not_returned_and_fails_instead() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    assert_eq!(
        returned(&opened, "m.closure", ints(&[]), &run()),
        Err("not returnable: its result holds a function the program wrote".to_string())
    );
    assert_eq!(
        returned(&opened, "m.keep", ints(&[]), &run()),
        Err("not returnable: its result holds a handle of its run".to_string())
    );
}

#[test]
fn a_failed_assertion_fails_only_that_call_and_later_calls_still_succeed() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    for _ in 0..2 {
        let raised = returned(&opened, "m.fail", arguments(1, |w| w.str(b"no")), &run())
            .expect_err("a failed assertion raises");
        assert!(raised.contains("no"), "{raised}");
        assert_eq!(int_returned(&opened, "m.double", &[5], &run()), Ok(10));
    }
}

#[test]
fn an_entry_inside_another_runs_on_a_context_of_its_own() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    let mut outer = crate::bare::new_context();
    outer.begin();
    assert_eq!(int_returned(&opened, "m.double", &[8], &run()), Ok(16));
    assert_eq!(outer.heap.live(), 0);
    outer.end();
    assert_eq!(int_returned(&opened, "m.double", &[9], &run()), Ok(18));
}

#[test]
fn a_call_made_from_a_small_stack_thread_still_recurses_100000_levels_deep_and_returns() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    let deep = Run::unbounded();
    let asked = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            entry::on_program_thread(|| {
                match entry::enter(&opened, "m.ladder", ints(&[100_000]), &deep, None, None).ended {
                    Ended::Returned(bytes) => Some(bytes),
                    _ => None,
                }
            })
        })
        .expect("a thread of little stack")
        .join()
        .expect("the asking thread ends");
    assert_eq!(asked.flatten().map(|bytes| int_of(&bytes)), Some(100_000));
}

#[test]
fn a_call_in_test_mode_reports_how_many_objects_it_allocated_and_never_freed() {
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
        assert!(matches!(o.ended, Ended::Returned(_)), "`{root}` returned");
        o.usage.live
    };
    assert_eq!([live("m.kept", true), live("m.freed", true)], [1, 0]);
    assert_eq!(
        live("m.kept", false),
        0,
        "an entry that is no test's counts nothing"
    );
}

#[test]
fn an_exported_function_that_returns_a_task_fails_with_a_diagnostic_naming_the_task() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(opened) = opened(dir.path()) else {
        return;
    };
    match entry::enter(&opened, "m.spawned", ints(&[]), &run(), None, None).ended {
        Ended::Failed(d) => {
            assert_eq!(d.code, ply_core::codes::SCHEDULER_ESCAPE_AT_BOUNDARY, "{d}");
            assert!(d.message.contains("`m.spawned` returned a `Task`"), "{d}");
        }
        _ => panic!("the boundary did not reject the task"),
    }
}

#[test]
fn the_heartbeat_names_only_tests_past_its_interval_longest_first() {
    let now = Instant::now();
    let tests = [
        ("quick".to_string(), now - Duration::from_secs(5)),
        ("slow".to_string(), now - Duration::from_secs(40)),
        ("slowest".to_string(), now - Duration::from_secs(95)),
    ];
    assert_eq!(
        overdue_lines(&tests, now, Duration::from_secs(30)),
        ["   running for 95s: slowest", "   running for 40s: slow"]
    );
}
