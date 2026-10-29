mod stage;
mod sweep;

use crate::units::{self, def};
use ply_compiled::c::exports::HelperShape;
use ply_compiled::c::{builtin_helper_name, helpers};
use ply_compiled::heap::{imm, imm_value};
use std::sync::atomic::Ordering::Relaxed;

/// A declaration with no address is a null call at run time: a crash rather than a decline.
#[test]
fn every_declared_helper_has_an_address() {
    for h in helpers() {
        assert!(!h.address.is_null(), "`{}` has no address", h.name);
    }
}

/// What the runtime reports of a native is read off the type of the function it binds: the words
/// past the context, whether it answers, and whether what it answers may be a raise.
#[test]
fn a_natives_signature_is_its_functions_type() {
    use ply_compiled::emitted::{Handed, PlyType};
    use ply_compiled::heap::Word;
    use ply_compiled::native::Machine::{self, Addr, Handed as Moved, I64, Type, Words};
    use ply_compiled::native::{Native, Raises, Signature};
    use ply_compiled::rt::Ctx;
    let of = |params: &'static [Machine], answer, raises| Signature {
        params,
        answer,
        raises,
    };
    assert_eq!(
        <unsafe extern "C" fn(*mut Ctx) as Native>::SIGNATURE,
        of(&[], None, false)
    );
    assert_eq!(
        <unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word as Native>::SIGNATURE,
        of(&[Machine::Word, Words], Some(Machine::Word), false)
    );
    assert_eq!(
        <unsafe extern "C" fn(*mut Ctx, Word, Word) -> Raises as Native>::SIGNATURE,
        of(&[Machine::Word; 2], None, true)
    );
    type Typed = unsafe extern "C" fn(
        *mut Ctx,
        ply_compiled::native::Addr,
        ply_compiled::native::I64,
        Handed,
        *const PlyType,
    ) -> ply_compiled::native::I64;
    assert_eq!(
        <Typed as Native>::SIGNATURE,
        of(&[Addr, I64, Moved, Type], Some(I64), false)
    );
}

/// A native over machine data is bound as a builtin's helper is, under the name the compiler or a
/// shipped module's `extern fn` gives it, which no other native has, at the
/// words and kinds its function takes.
#[test]
fn every_native_over_machine_data_has_a_helper_of_its_own() {
    use ply_compiled::c::machine_natives;
    use ply_compiled::native::Machine;
    let machine = machine_natives();
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
            !n.signature.params.contains(&Machine::Word),
            "`{}` takes a uniform word",
            n.name
        );
    }
    let out_str = machine
        .iter()
        .find(|n| n.name == "?out_str")
        .expect("`?out_str`");
    assert_eq!(out_str.signature.params, [Machine::Addr, Machine::I64]);
    assert_eq!(out_str.signature.answer, None);
}

/// What the runtime says of itself to a compiler is read off its own functions and structs: each
/// helper of its own and each native with what its function's type takes and answers, and where it
/// lays out what emitted code reads in place.
#[test]
fn the_runtime_describes_its_functions_and_layouts_as_its_types_say() {
    use ply_compiled::rt::Ctx;
    let text = ply_compiled::c::described();
    assert_eq!(text.lines().next(), Some("ply runtime 1"));
    for line in [
        "helper rt_release_later 0 - Handed DropRoot",
        "helper rt_alloc_bytes 0 Block Word",
        "helper rt_no_match 0 -",
        "native ?out_str 0 - Addr I64",
        "native ?refused 1 - I64 I64 I64 I64",
    ] {
        assert!(text.lines().any(|l| l == line), "no `{line}` in:\n{text}");
    }
    let own = ply_compiled::c::own_helpers();
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
        std::mem::offset_of!(Ctx, release_depth)
    );
    assert_eq!(fact("PlyCtx.frames.bytes"), 8);
    assert_eq!(fact("PlyObj.bytes"), ply_compiled::heap::HEADER);
    assert_eq!(
        fact("PlyFnDesc.bytes"),
        size_of::<ply_compiled::emitted::PlyFnDesc>()
    );
    assert_eq!(fact("count.frame"), ply_compiled::heap::FRAME as usize);
    assert_eq!(
        fact("release_depth"),
        ply_compiled::rt::RELEASE_DEPTH as usize
    );
}

/// A unit binds the helpers it records by name, in its own order, each to this runtime's own.
#[test]
fn a_units_helpers_bind_by_name_to_the_runtimes_own() {
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

/// A table reads as a unit writes one: its helpers, constants, modules, pools and entered roots,
/// in that order.
#[test]
fn a_units_table_reads_as_a_unit_writes_it() {
    let said = "helpers 0\nconstants 1\nm.table 4\nmodules 3\nfields 1\nop\nentered m.main\n";
    let read = ply_compiled::c::Exports::decode(said).expect("the table reads");
    assert_eq!(read.constants, [("m.table".to_string(), 4)]);
    assert_eq!(read.modules, 3);
    assert_eq!(read.fields, [ply_core::Symbol::new("op")]);
    assert_eq!(read.entered, ["m.main"]);
    assert_eq!(read.encode(), said);
}

/// The last release's units list what their emitter refused after their modules: such a table reads
/// as the same table without the list, since the release's builder runs on this runtime.
#[test]
fn a_table_listing_refusals_reads_as_one_without_them() {
    let listed = "helpers 0\nconstants 0\nmodules 3\nrefused 1\nm.f\nwhy\nfields 1\nop\n\
                  entered m.main\n";
    let bare = "helpers 0\nconstants 0\nmodules 3\nfields 1\nop\nentered m.main\n";
    let read = ply_compiled::c::Exports::decode(listed).expect("the listing table reads");
    assert_eq!(read.encode(), bare);
}

/// What a unit was emitted against is what it calls: a helper the runtime lacks, or has under
/// another shape, refuses the unit by name rather than binding it to nothing.
#[test]
fn a_unit_recording_a_helper_this_runtime_lacks_or_has_otherwise_is_refused_by_name() {
    let mut retired = units::exports_of(&[]);
    retired.helpers.push(HelperShape {
        name: "rt_retired".to_string(),
        args: 1,
        answers: true,
    });
    let refused = retired
        .bound()
        .expect_err("a helper the runtime lacks is refused");
    assert!(refused.to_string().contains("`rt_retired`"), "{refused}");
    let mut reshaped = units::exports_of(&[]);
    reshaped.helpers[0].args += 1;
    let refused = reshaped
        .bound()
        .expect_err("a helper taking other words is not the one the unit was emitted against");
    assert!(
        refused.to_string().contains(&reshaped.helpers[0].name),
        "{refused}"
    );
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &[], |e| {
        e.helpers.push(HelperShape {
            name: "rt_retired".to_string(),
            args: 1,
            answers: true,
        })
    }) else {
        return;
    };
    let refused = loaded
        .err()
        .expect("a unit that does not serve is not loaded");
    assert!(refused.contains("`rt_retired`"), "{refused}");
}

#[test]
fn a_unit_compiles_loads_binds_and_answers() {
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
    let answer = units::entered(&native, "m.probe", &[imm(20), imm(22)]).expect("it answers");
    assert_eq!(imm_value(answer), 42);
}

/// The prologue a body opens with: room asked for on the stack it runs on where its frame lies below
/// the floor.
const PROLOGUE: &str = "  { char probe; if ((uintptr_t)&probe < ctx->stack_floor && !rt_stack_more_p(ctx)) return 0; }\n";

/// `ladder(n)` is `n` calls deep, none of them a tail call, each frame a kilobyte and more.
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

/// Twenty megabytes of frames and more: past a stack's first two chunks.
const DEEP: i64 = 20_000;

/// An entry begun where a Rust recursion grew onto a stacker segment, or anywhere else the runtime
/// does not know.
struct Unknown {
    ctx: *mut ply_compiled::rt::Ctx,
    entry: ply_compiled::rt::Entry,
    args: [ply_compiled::heap::Word; 1],
    answer: ply_compiled::heap::Word,
    main: usize,
}

extern "C" fn enter_from_an_unknown_stack(arg: usize) {
    let u = arg as *mut Unknown;
    unsafe {
        (*u).answer = ply_compiled::rt::on_known_stack((*u).ctx, || {
            ((*u).entry)((*u).ctx, (*u).args.as_ptr())
        });
        let mut spent = 0;
        ply_compiled::stack::switch(&mut spent, (*u).main);
    }
    std::process::abort();
}

/// `m.ladder(DEEP)` begun on a stack the runtime does not know: what it answered, the chunks it grew
/// by, and how it failed.
fn ladder_from_an_unknown_stack() -> Option<(i64, u64, i64, Option<String>)> {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let loaded = units::opened(dir.path(), &deep_calls(), |_| {})?;
    let native = loaded.expect("the unit loads");
    let mut ctx = native.context();
    ctx.begin();
    let mut u = Unknown {
        ctx: &raw mut ctx,
        entry: native.entry("m.ladder").expect("`m.ladder` is taken"),
        args: [imm(DEEP)],
        answer: 0,
        main: 0,
    };
    let u = &raw mut u;
    let unknown = ply_compiled::stack::Stack::take().expect("the system maps a stack");
    let sp = unknown.prepare(enter_from_an_unknown_stack, u as usize);
    let answer = unsafe {
        ply_compiled::stack::switch(&mut (*u).main, sp);
        (*u).answer
    };
    let (failed, grown) = (ctx.failed, ctx.grown);
    let raised = ctx.take_failure().map(|d| d.message);
    ctx.end();
    Some((imm_value(answer), grown, failed, raised))
}

/// A recursion past a stack's first chunk makes the stack writable further down and answers on
/// it, so a program nests as deeply as its stack's reservation holds. An entry begun where the
/// runtime knows no stack is given one of its own.
#[test]
fn a_recursion_past_a_stacks_first_chunk_grows_it_in_place_and_answers() {
    let Some((answer, grown, failed, raised)) = ladder_from_an_unknown_stack() else {
        return;
    };
    assert_eq!(failed, 0, "`m.ladder({DEEP})` raised: {raised:?}");
    assert_eq!(answer, DEEP);
    assert!(
        grown >= 2,
        "the call never crossed the floor twice: {grown}"
    );
}

/// A thread's own stack is reserved whole and cannot be made larger: a recursion past its floor
/// fails, and never runs into its guard.
#[test]
fn a_recursion_past_a_threads_floor_fails_out_of_stack() {
    let ran = std::thread::Builder::new()
        .stack_size(4 << 20)
        .spawn(|| {
            let dir = tempfile::tempdir().expect("a scratch directory");
            let loaded = units::opened(dir.path(), &deep_calls(), |_| {})?;
            let native = loaded.expect("the unit loads");
            let entry = native.entry("m.ladder").expect("`m.ladder` is taken");
            let mut ctx = native.context();
            ctx.begin();
            let args = [imm(DEEP)];
            let _ = unsafe { entry(&mut ctx, args.as_ptr()) };
            let failed = ctx.failed;
            let raised = ctx.take_failure().map(|d| d.message).unwrap_or_default();
            ctx.end();
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

/// A loop's restart probes the floor as a prologue does, reading it anew each pass.
const RESTART: &str = "  ply_loop: { char probe; if ((uintptr_t)&probe < (uintptr_t)*(volatile int64_t *)&ctx->stack_floor && !rt_stack_more_p(ctx)) return 0; }\n";

/// A loop that makes no call ends at its restart once its clock runs out.
#[test]
fn a_loop_past_its_clock_ends_at_its_next_restart() {
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
    let mut ctx = native.context();
    ctx.begin_bounded(20);
    let args = [imm(0)];
    let _ = unsafe { entry(&mut ctx, args.as_ptr()) };
    let failed = ctx.failed;
    let said = ctx.take_failure().map(|d| d.message).unwrap_or_default();
    ctx.end();
    assert_eq!(failed, ply_compiled::rt::FAILED_ABANDONED, "{said}");
    assert!(said.contains("abandoned after 20 ms"), "{said}");
}

/// The watchdog writes to a context only while its entry runs, since the context may be reused or
/// freed after.
#[test]
fn an_entry_that_ended_before_its_clock_ran_out_is_left_alone() {
    let native = units::opened(
        tempfile::tempdir().expect("a scratch directory").path(),
        &[],
        |_| {},
    );
    let Some(loaded) = native else {
        return;
    };
    let mut ctx = loaded.expect("the unit loads").context();
    ctx.begin_bounded(1);
    ctx.end();
    ctx.stack_floor.store(7, Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(ctx.stack_floor.load(Relaxed), 7);
}

/// A unit says each constant with its row of the code table.
#[test]
fn a_unit_says_its_constants_slots() {
    let with = "helpers 0\nconstants 1\nm.table 4\nmodules 0\nfields 0\n";
    let read = ply_compiled::c::Exports::decode(with).expect("the table reads");
    assert_eq!(read.constants, [("m.table".to_string(), 4)]);
    assert_eq!(read.encode(), with);
    assert!(
        ply_compiled::c::Exports::decode(&with.replace("m.table 4", "m.table")).is_none(),
        "a constant with no slot is no row"
    );
}

/// A unit entered encoded names each root it is entered at on a line of its own after its tables,
/// among lines a runtime that does not know them reads as nothing.
#[test]
fn a_unit_says_each_root_it_is_entered_at_after_its_tables() {
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
