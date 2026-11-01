mod stage;
mod sweep;

use crate::units::{self, def};
use ply_compiled::c::exports::HelperShape;
use ply_compiled::c::{builtin_helper_name, helpers};
use ply_compiled::heap::{imm, imm_value};
use std::sync::atomic::Ordering::Relaxed;

#[test]
fn every_declared_helper_has_an_address() {
    for h in helpers() {
        assert!(!h.address.is_null(), "`{}` has no address", h.name);
    }
}

#[test]
fn a_native_function_signature_is_derived_from_its_rust_function_type() {
    use ply_compiled::emitted::{Handed, PlyType};
    use ply_compiled::heap::Word;
    use ply_compiled::native::MachineType::{self, Address, Handed as Moved, I64, Type, Words};
    use ply_compiled::native::{Native, Signature};
    use ply_compiled::rt::Context;
    let of = |params: &'static [MachineType], result| Signature { params, result };
    assert_eq!(
        <unsafe extern "C" fn(*mut Context) as Native>::SIGNATURE,
        of(&[], None)
    );
    assert_eq!(
        <unsafe extern "C" fn(*mut Context, Word, *const Word) -> Word as Native>::SIGNATURE,
        of(&[MachineType::Word, Words], Some(MachineType::Word))
    );
    type NativeTakingType = unsafe extern "C" fn(
        *mut Context,
        ply_compiled::native::Address,
        ply_compiled::native::I64,
        Handed,
        *const PlyType,
    ) -> ply_compiled::native::I64;
    assert_eq!(
        <NativeTakingType as Native>::SIGNATURE,
        of(&[Address, I64, Moved, Type], Some(I64))
    );
}

#[test]
fn each_native_function_that_takes_raw_addresses_or_integers_has_exactly_one_runtime_helper_with_the_same_signature()
 {
    use ply_compiled::c::runtime_natives;
    use ply_compiled::native::MachineType;
    let machine = runtime_natives();
    assert!(machine.len() > 30, "only {} machine natives", machine.len());
    let declared: Vec<&str> = ply_compiled::hosted::natives()
        .iter()
        .chain(ply_compiled::secret::natives().iter())
        .chain(ply_compiled::entry::natives().iter())
        .chain(ply_compiled::tasks::natives().iter())
        .map(|n| n.name)
        .collect();
    for n in machine {
        assert!(
            n.name.starts_with('?') || declared.contains(&n.name),
            "`{}` is no name the compiler or a shipped module declares",
            n.name
        );
        let name = builtin_helper_name(n.name);
        let found: Vec<_> = helpers().iter().filter(|h| h.name == name).collect();
        assert_eq!(found.len(), 1, "`{name}`");
        assert_eq!(found[0].signature, n.signature, "`{name}`");
        assert!(
            !n.signature.params.contains(&MachineType::Word),
            "`{}` takes a uniform word",
            n.name
        );
    }
    let out_str = machine
        .iter()
        .find(|n| n.name == "?out_str")
        .expect("`?out_str`");
    assert_eq!(
        out_str.signature.params,
        [MachineType::Address, MachineType::I64]
    );
    assert_eq!(out_str.signature.result, None);
}

#[test]
fn the_text_the_runtime_describes_itself_with_lists_helpers_and_native_functions_and_struct_offsets_and_sizes_matching_the_rust_code()
 {
    use ply_compiled::rt::Context;
    let text = ply_compiled::c::described();
    assert_eq!(text.lines().next(), Some("ply runtime 1"));
    for line in [
        "helper rt_release_later 0 - Handed DropRoot",
        "helper rt_alloc_bytes 0 Block Word",
        "helper rt_release_drain 0 -",
        "native ?out_str 0 - Addr I64",
        "helper rt_failed 0 - I64 Words I64 I64",
    ] {
        assert!(text.lines().any(|l| l == line), "no `{line}` in:\n{text}");
    }
    let own = ply_compiled::c::runtime_helpers();
    let helpers = text.lines().filter(|l| l.starts_with("helper ")).count();
    assert_eq!(helpers, own.len());
    assert!(own.iter().all(|h| !h.name.starts_with("builtin_")));
    let fact = |name: &str| {
        let line = format!("fact {name} ");
        let found = text.lines().find_map(|l| l.strip_prefix(line.as_str()));
        found
            .unwrap_or_else(|| panic!("no fact `{name}`"))
            .parse::<usize>()
            .expect("a fact is a number")
    };
    assert_eq!(
        fact("PlyCtx.release_depth.at"),
        std::mem::offset_of!(Context, release_depth)
    );
    assert_eq!(fact("PlyCtx.frames.bytes"), 8);
    assert_eq!(fact("PlyObj.bytes"), ply_compiled::heap::HEADER);
    assert_eq!(
        fact("PlyFnDesc.bytes"),
        size_of::<ply_compiled::emitted::PlyFunctionDescriptor>()
    );
    assert_eq!(fact("count.frame"), ply_compiled::heap::FRAME as usize);
    assert_eq!(
        fact("release_depth"),
        ply_compiled::rt::RELEASE_DEPTH as usize
    );
}

#[test]
fn helpers_recorded_in_a_unit_export_table_bind_by_name_to_the_runtime_helper_addresses() {
    let mut exports = units::exports_of(&[]);
    exports.helpers.reverse();
    let read = ply_compiled::c::Exports::decode(&exports.encode()).expect("the table reads back");
    assert_eq!(read.encode(), exports.encode());
    let bound = read.bound().expect("the unit serves this runtime");
    for (helper, address) in read.helpers.iter().zip(&bound) {
        let runtime = helpers()
            .iter()
            .find(|h| h.name == helper.name)
            .expect("the runtime has every helper it records");
        assert_eq!(*address as *const (), runtime.address, "`{}`", helper.name);
    }
}

#[test]
fn an_exports_table_decodes_to_the_values_a_unit_wrote_and_encodes_back_to_the_same_text() {
    let said = "helpers 0\nconstants 1\nm.table 4\nmodules 3\nfields 1\nop\nentered m.main\n";
    let read = ply_compiled::c::Exports::decode(said).expect("the table reads");
    assert_eq!(read.constants, [("m.table".to_string(), 4)]);
    assert_eq!(read.modules, 3);
    assert_eq!(read.fields, [ply_core::Symbol::new("op")]);
    assert_eq!(read.entered, ["m.main"]);
    assert_eq!(read.encode(), said);
}

#[test]
fn decoding_an_exports_table_drops_its_list_of_functions_that_were_not_compiled_so_encoding_omits_it()
 {
    let listed = "helpers 0\nconstants 0\nmodules 3\nrefused 1\nm.f\nwhy\nfields 1\nop\n\
                  entered m.main\n";
    let bare = "helpers 0\nconstants 0\nmodules 3\nfields 1\nop\nentered m.main\n";
    let read = ply_compiled::c::Exports::decode(listed).expect("the listing table reads");
    assert_eq!(read.encode(), bare);
}

#[test]
fn a_unit_that_records_a_helper_the_runtime_lacks_or_with_a_different_argument_count_fails_to_bind_and_the_error_message_names_the_helper()
 {
    let mut retired = units::exports_of(&[]);
    retired.helpers.push(HelperShape {
        name: "rt_retired".to_string(),
        args: 1,
        results: true,
    });
    let rejected = retired
        .bound()
        .expect_err("a helper the runtime lacks is rejected");
    assert!(rejected.to_string().contains("`rt_retired`"), "{rejected}");
    let mut reshaped = units::exports_of(&[]);
    reshaped.helpers[0].args += 1;
    let rejected = reshaped
        .bound()
        .expect_err("a helper taking other words is not the one the unit was emitted against");
    assert!(
        rejected.to_string().contains(&reshaped.helpers[0].name),
        "{rejected}"
    );
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &[], |e| {
        e.helpers.push(HelperShape {
            name: "rt_retired".to_string(),
            args: 1,
            results: true,
        })
    }) else {
        return;
    };
    let rejected = loaded
        .err()
        .expect("a unit that does not serve is not loaded");
    assert!(rejected.contains("`rt_retired`"), "{rejected}");
}

#[test]
fn a_compiled_unit_loads_binds_its_helpers_and_returns_the_right_result_when_called() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let probe = def(
        "m.probe",
        2,
        "  return ply_imm(ply_imm_value(p0) + ply_imm_value(p1));",
    );
    let Some(loaded) = units::opened(dir.path(), &[probe], |_| {}) else {
        return;
    };
    let native = loaded.expect("the unit loads");
    assert_eq!(native.names(), ["m.probe"]);
    assert_eq!(native.arity("m.probe"), Some(2));
    let result = units::entered(&native, "m.probe", &[imm(20), imm(22)]).expect("it returns");
    assert_eq!(imm_value(result), 42);
}

const PROLOGUE: &str = "  { char probe; if ((uintptr_t)&probe < ctx->stack_floor && !rt_stack_more_p(ctx)) return 0; }\n";

fn deep_calls() -> [units::Def; 1] {
    let ladder = units::symbol("m.ladder");
    [def(
        "m.ladder",
        1,
        &format!(
            "{PROLOGUE}  volatile char pad[1024];\n\
             \x20 int64_t n = ply_imm_value(p0);\n\
             \x20 pad[n & 1023] = 1;\n\
             \x20 Word r = ply_imm(0);\n\
             \x20 if (n > 0) {{ r = {ladder}(ctx, ply_imm(n - 1)); if (ctx->failed) return 0; r = ply_imm(ply_imm_value(r) + pad[n & 1023]); }}\n\
             \x20 return r;"
        ),
    )]
}

const DEEP: i64 = 20_000;

struct Unknown {
    context: *mut ply_compiled::rt::Context,
    entry: ply_compiled::rt::Entry,
    args: [ply_compiled::heap::Word; 1],
    result: ply_compiled::heap::Word,
    main: usize,
}

extern "C" fn enter_from_an_unknown_stack(arg: usize) {
    let u = arg as *mut Unknown;
    unsafe {
        (*u).result = ply_compiled::rt::on_known_stack((*u).context, || {
            ((*u).entry)((*u).context, (*u).args.as_ptr())
        });
        let mut spent = 0;
        ply_compiled::stack::switch(&mut spent, (*u).main);
    }
    std::process::abort();
}

fn ladder_from_an_unknown_stack() -> Option<(i64, u64, i64, Option<String>)> {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let loaded = units::opened(dir.path(), &deep_calls(), |_| {})?;
    let native = loaded.expect("the unit loads");
    let mut context = native.context();
    context.begin();
    let mut u = Unknown {
        context: &raw mut context,
        entry: native.entry("m.ladder").expect("`m.ladder` is taken"),
        args: [imm(DEEP)],
        result: 0,
        main: 0,
    };
    let u = &raw mut u;
    let unknown = ply_compiled::stack::Stack::take().expect("the system maps a stack");
    let stack_pointer = unknown.prepare(enter_from_an_unknown_stack, u as usize);
    let result = unsafe {
        ply_compiled::stack::switch(&mut (*u).main, stack_pointer);
        (*u).result
    };
    let (failed, grown) = (context.failed, context.grown);
    let raised = context.take_failure().map(|d| d.message);
    context.end();
    Some((imm_value(result), grown, failed, raised))
}

#[test]
fn a_recursion_deeper_than_the_first_stack_chunk_grows_the_stack_in_place_and_returns() {
    let Some((result, grown, failed, raised)) = ladder_from_an_unknown_stack() else {
        return;
    };
    assert_eq!(failed, 0, "`m.ladder({DEEP})` raised: {raised:?}");
    assert_eq!(result, DEEP);
    assert!(
        grown >= 2,
        "the call never crossed the floor twice: {grown}"
    );
}

#[test]
fn a_recursion_deeper_than_the_thread_stack_limit_fails_with_an_out_of_stack_error() {
    let ran = std::thread::Builder::new()
        .stack_size(4 << 20)
        .spawn(|| {
            let dir = tempfile::tempdir().expect("a scratch directory");
            let loaded = units::opened(dir.path(), &deep_calls(), |_| {})?;
            let native = loaded.expect("the unit loads");
            let entry = native.entry("m.ladder").expect("`m.ladder` is taken");
            let mut context = native.context();
            context.begin();
            let args = [imm(DEEP)];
            let _ = unsafe { entry(&mut context, args.as_ptr()) };
            let failed = context.failed;
            let raised = context
                .take_failure()
                .map(|d| d.message)
                .unwrap_or_default();
            context.end();
            Some((failed, raised))
        })
        .expect("a thread starts")
        .join()
        .expect("the thread ran to its end");
    let Some((failed, raised)) = ran else {
        return;
    };
    assert_eq!(
        failed,
        ply_compiled::rt::FAILED_OUT_OF_STACK,
        "a recursion past the thread's stack ended some other way: {raised}"
    );
    assert!(raised.contains("what the native stack holds"), "{raised}");
}

const RESTART: &str = "  ply_loop: { char probe; if ((uintptr_t)&probe < (uintptr_t)*(volatile int64_t *)&ctx->stack_floor && !rt_stack_more_p(ctx)) return 0; }\n";

#[test]
fn a_loop_running_past_the_time_limit_stops_with_a_timed_out_failure_at_its_next_loop_restart() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let forever = def(
        "m.forever",
        1,
        &format!("{PROLOGUE}  int64_t n = 0;\n{RESTART}  n += 1;\n  goto ply_loop;"),
    );
    let Some(loaded) = units::opened(dir.path(), &[forever], |_| {}) else {
        return;
    };
    let native = loaded.expect("the unit loads");
    let entry = native.entry("m.forever").expect("`m.forever` is taken");
    let mut context = native.context();
    context.begin_with_time_limit(20);
    let args = [imm(0)];
    let _ = unsafe { entry(&mut context, args.as_ptr()) };
    let failed = context.failed;
    let said = context
        .take_failure()
        .map(|d| d.message)
        .unwrap_or_default();
    context.end();
    assert_eq!(failed, ply_compiled::rt::FAILED_TIMED_OUT, "{said}");
    assert!(said.contains("timed out after 20 ms"), "{said}");
}

#[test]
fn a_time_limit_on_an_entry_that_already_ended_does_not_change_the_context_stack_floor_value_later()
{
    let native = units::opened(
        tempfile::tempdir().expect("a scratch directory").path(),
        &[],
        |_| {},
    );
    let Some(loaded) = native else {
        return;
    };
    let mut context = loaded.expect("the unit loads").context();
    context.begin_with_time_limit(1);
    context.end();
    context.stack_floor.store(7, Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(context.stack_floor.load(Relaxed), 7);
}

#[test]
fn decoding_an_exports_table_reads_a_number_of_slots_for_each_constant_and_fails_if_a_constant_has_none()
 {
    let with = "helpers 0\nconstants 1\nm.table 4\nmodules 0\nfields 0\n";
    let read = ply_compiled::c::Exports::decode(with).expect("the table reads");
    assert_eq!(read.constants, [("m.table".to_string(), 4)]);
    assert_eq!(read.encode(), with);
    assert!(
        ply_compiled::c::Exports::decode(&with.replace("m.table 4", "m.table")).is_none(),
        "a constant with no slot is no row"
    );
}

#[test]
fn decoding_an_exports_table_reads_the_entered_function_lines_after_the_other_sections_and_ignores_unknown_lines_after_them()
 {
    let tables = "helpers 0\nconstants 0\nmodules 0\nfields 0\n";
    let said = format!("{tables}entered m.main\nentered m.run\n");
    let read = ply_compiled::c::Exports::decode(&said).expect("the table reads");
    assert_eq!(read.entered, ["m.main", "m.run"]);
    assert_eq!(read.encode(), said);
    let unknown = format!("{tables}entered m.main\na line a later unit says\n");
    let read = ply_compiled::c::Exports::decode(&unknown).expect("the table reads");
    assert_eq!(read.entered, ["m.main"]);
    let words = ply_compiled::c::Exports::decode(tables).expect("the table reads");
    assert!(words.entered.is_empty());
}
