mod dump;
mod stage;
mod sweep;

use crate::units::{self, def};
use ply_compiled::c::exports::HelperShape;
use ply_compiled::c::{PRELUDE, builtin_helper_name, helpers};
use ply_compiled::heap::{imm, imm_value};

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
    use ply_compiled::native::{Native, Raising, Signature};
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
        <unsafe extern "C" fn(*mut Ctx, Word, Word, Word, Word, Word, Word) -> Raising as Native>::SIGNATURE,
        of(&[Machine::Word; 6], Some(Machine::Word), true)
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
    let concat = ply_core::Builtin::from_name("Bytes::concat").expect("a builtin");
    let words = of(&[Machine::Word; 2], Some(Machine::Word), false);
    assert_eq!(ply_compiled::rt::signature(concat), words);
    let at = ply_core::Builtin::from_name("Bytes::at").expect("a builtin");
    assert_eq!(
        ply_compiled::rt::signature(at),
        Signature {
            raises: true,
            ..words
        }
    );
}

/// A native over machine data is bound as a builtin's helper is, under the name the compiler gives
/// it, which neither a builtin nor another of them has, at the words and kinds its function takes.
#[test]
fn every_native_over_machine_data_has_a_helper_of_its_own() {
    use ply_compiled::c::machine_natives;
    use ply_compiled::native::Machine;
    let machine = machine_natives();
    assert!(machine.len() > 30, "only {} machine natives", machine.len());
    for n in machine {
        assert!(
            n.name.starts_with('?'),
            "`{}` is no name the compiler declares",
            n.name
        );
        assert!(
            ply_core::Builtin::from_name(n.name).is_none(),
            "`{}`",
            n.name
        );
        let name = builtin_helper_name(n.name);
        let found: Vec<_> = helpers().iter().filter(|h| h.name == name).collect();
        assert_eq!(found.len(), 1, "`{name}`");
        assert_eq!(found[0].args, n.signature.words(), "`{name}`");
        assert_eq!(found[0].answers, n.signature.answers(), "`{name}`");
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

#[test]
fn the_prelude_agrees_with_the_layouts_it_mirrors() {
    assert_eq!(ply_compiled::heap::HEADER, 8, "PLY_HEADER");
    assert_eq!(std::mem::size_of::<ply_compiled::heap::Obj>(), 8, "PlyObj");
    assert_eq!(
        std::mem::offset_of!(ply_compiled::rt::Ctx, failed),
        0,
        "PlyCtx.failed"
    );
    assert_eq!(
        std::mem::offset_of!(ply_compiled::rt::Ctx, fuel),
        8,
        "PlyCtx.fuel"
    );
    assert_eq!(
        std::mem::offset_of!(ply_compiled::rt::Ctx, stack_floor),
        16,
        "PlyCtx.stack_floor"
    );
    assert_eq!(
        std::mem::offset_of!(ply_compiled::rt::Ctx, ticks),
        48,
        "PlyCtx.ticks"
    );
    assert_eq!(
        std::mem::offset_of!(ply_compiled::rt::Ctx, next_tick),
        56,
        "PlyCtx.next_tick"
    );
    assert!(PRELUDE.contains("#define PLY_HEADER 8"));
    assert_eq!(ply_compiled::heap::SHARED, 0x8000_0000, "ply_inc");
    assert!(ply_compiled::c::runtime_header().contains("rc < UINT32_C(0x80000000)"));
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

/// A table a unit whose values the runtime laid out wrote reads as its helpers, constants, pools and
/// entered roots: the constructors, shapes, instances and layout lines it says besides read as
/// nothing.
#[test]
fn a_units_table_reads_past_what_it_says_of_values_the_runtime_laid_out() {
    let said = "helpers 0\nctors 2\nNone 0\nm.Pair 2 a000 a001\nconstants 1\nm.table 4\nmodules 3\n\
                refused 0\nbuiltins 0\nfields 1\nop\nshapes 1\na b\ninstances 1\nm.Box m.area !\n\
                ints held raw\nentered m.main\n";
    let read = ply_compiled::c::Exports::decode(said).expect("the table reads");
    assert_eq!(read.constants, [("m.table".to_string(), 4)]);
    assert_eq!(read.modules, 3);
    assert_eq!(read.fields, [ply_core::Symbol::new("op")]);
    assert_eq!(read.entered, ["m.main"]);
    assert_eq!(
        read.encode(),
        "helpers 0\nconstants 1\nm.table 4\nmodules 3\nrefused 0\nbuiltins 0\nfields 1\nop\n\
         entered m.main\n"
    );
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
    let (native, refused) = loaded.expect("the unit loads");
    assert!(refused.is_empty());
    assert_eq!(native.names(), ["m.probe"]);
    assert_eq!(native.arity("m.probe"), Some(2));
    let answer = units::entered(&native, "m.probe", 10, &[imm(20), imm(22)]).expect("it answers");
    assert_eq!(imm_value(answer), 42);
}

/// A C row of each case of `op` over `values`: the operands, whether it overflows, and the
/// wrapped answer.
fn overflow_rows(op: &str, values: &[i64], f: fn(i64, i64) -> (Option<i64>, i64)) -> String {
    let lit = |v: i64| {
        if v == i64::MIN {
            "INT64_MIN".to_string()
        } else {
            format!("INT64_C({v})")
        }
    };
    let mut rows = String::new();
    for &a in values {
        for &b in values {
            let (exact, wrapped) = f(a, b);
            rows.push_str(&format!(
                "    {{ {op}, {}, {}, {}, {} }},\n",
                lit(a),
                lit(b),
                i32::from(exact.is_none()),
                lit(wrapped)
            ));
        }
    }
    rows
}

/// The checked arithmetic the C prelude gives compiled code, the compiler's overflow builtins and
/// the fallback a compiler without them (tcc) reads, each against Rust's over the edges of the range.
#[test]
fn the_preludes_checked_arithmetic_agrees_with_and_without_the_overflow_builtins() {
    let values = [
        0,
        1,
        -1,
        2,
        -2,
        3,
        i64::MAX,
        i64::MIN,
        i64::MAX / 2,
        i64::MIN / 2,
        3_037_000_499,
        3_037_000_500,
        -3_037_000_500,
        1 << 32,
        -(1 << 32),
        1 << 62,
        -(1 << 62),
    ];
    let rows = [
        overflow_rows("0", &values, |a, b| (a.checked_add(b), a.wrapping_add(b))),
        overflow_rows("1", &values, |a, b| (a.checked_sub(b), a.wrapping_sub(b))),
        overflow_rows("2", &values, |a, b| (a.checked_mul(b), a.wrapping_mul(b))),
    ]
    .concat();
    let body = format!(
        "  static const struct {{ int op; int64_t a, b; int over; int64_t r; }} cases[] = {{\n{rows}  }};\n\
         \x20 int64_t wrong = 0;\n\
         \x20 for (unsigned i = 0; i < sizeof cases / sizeof cases[0]; i++) {{\n\
         \x20   int64_t r = 0;\n\
         \x20   int over = cases[i].op == 0 ? ply_add_ov(cases[i].a, cases[i].b, &r)\n\
         \x20     : cases[i].op == 1 ? ply_sub_ov(cases[i].a, cases[i].b, &r) : ply_mul_ov(cases[i].a, cases[i].b, &r);\n\
         \x20   if (over != cases[i].over || r != cases[i].r) wrong++;\n\
         \x20 }}\n\
         \x20 return ply_imm(wrong);"
    );
    let defs = [def("m.checked", 0, &body)];
    for head in ["", "#define PLY_NO_OV_BUILTIN\n"] {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let text = format!("{head}{}", units::text(&defs, &units::exports_of(&defs)));
        let Some(lib) = units::library(dir.path(), &text) else {
            return;
        };
        let (native, _) = ply_compiled::c::load_opened(&lib, None).expect("the unit loads");
        let wrong = units::entered(&native, "m.checked", 10, &[]).expect("it answers");
        assert_eq!(imm_value(wrong), 0, "with {head:?}");
    }
}

/// The prologue a body opens with: the fuel it spends, and room asked for on the stack it runs on
/// where its frame lies below the floor.
const PROLOGUE: &str = "  if (ctx->fuel <= 0) { rt_no_fuel_p(ctx); return 0; }\n\
     \x20 { char probe; if ((uintptr_t)&probe < ctx->stack_floor && !rt_stack_more_p(ctx)) return 0; }\n\
     \x20 ctx->fuel -= 1;\n";

/// `ladder(n)` is `n` calls deep, none of them a tail call, each frame a kilobyte and more;
/// `spin` has no base case.
fn deep_calls() -> [units::Def; 2] {
    let ladder = units::symbol("m.ladder");
    let spin = units::symbol("m.spin");
    [
        def(
            "m.ladder",
            1,
            &format!(
                "{PROLOGUE}  volatile char pad[1024];\n\
                 \x20 int64_t n = ply_imm_value(p0);\n\
                 \x20 pad[n & 1023] = 1;\n\
                 \x20 Word r = ply_imm(0);\n\
                 \x20 if (n > 0) {{ r = {ladder}(ctx, ply_imm(n - 1)); if (ctx->failed) return 0; r = ply_imm(ply_imm_value(r) + pad[n & 1023]); }}\n\
                 \x20 ctx->fuel += 1;\n\
                 \x20 return r;"
            ),
        ),
        def(
            "m.spin",
            1,
            &format!(
                "{PROLOGUE}  Word r = {spin}(ctx, ply_imm(ply_imm_value(p0) + 1));\n\
                 \x20 if (ctx->failed) return 0;\n\
                 \x20 ctx->fuel += 1;\n\
                 \x20 return ply_imm(ply_imm_value(r) + 1);"
            ),
        ),
    ]
}

/// Twenty megabytes of frames and more: past a stack's first two chunks.
const DEEP: i64 = 20_000;

/// An entry begun where a Rust recursion grew onto a stacker segment, or anywhere else the runtime
/// does not know, as `Bodies::run` begins one.
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
    let (native, _) = loaded.expect("the unit loads");
    let mut ctx = native.context();
    ctx.begin(DEEP * 2);
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
/// it, so how deep a program nests is what its fuel says. An entry begun where the runtime knows no
/// stack is given one of its own.
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
            let (native, _) = loaded.expect("the unit loads");
            let entry = native.entry("m.ladder").expect("`m.ladder` is taken");
            let mut ctx = native.context();
            ctx.begin(DEEP * 2);
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

/// Growing is not a licence to recurse for ever: depth is the fuel's to bound.
#[test]
fn a_recursion_with_no_base_case_still_stops_at_the_fuel() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &deep_calls(), |_| {}) else {
        return;
    };
    let (native, _) = loaded.expect("the unit loads");
    let entry = native.entry("m.spin").expect("`m.spin` is taken");
    let mut ctx = native.context();
    ctx.begin(10_000);
    let here = 0u8;
    ctx.stack_floor = std::ptr::from_ref(&here) as usize - 64 * 1024;
    let args = [imm(0)];
    let _ = unsafe { entry(&mut ctx, args.as_ptr()) };
    let failed = ctx.failed;
    let raised = ctx.take_failure().map(|d| d.message).unwrap_or_default();
    ctx.end();
    assert_eq!(
        failed,
        ply_compiled::rt::FAILED_OUT_OF_FUEL,
        "a runaway recursion ended some other way: {raised}"
    );
    assert!(raised.contains("bound on nested calls"), "{raised}");
}

/// A unit says each constant with its row of the code table.
#[test]
fn a_unit_says_its_constants_slots() {
    let with = "helpers 0\nconstants 1\nm.table 4\nmodules 0\nrefused 0\nbuiltins 0\nfields 0\n";
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
    let tables = "helpers 0\nconstants 0\nmodules 0\nrefused 0\nbuiltins 0\nfields 0\n";
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

/// `m.double`, compiled and opened as a program's unit entered through `m.double#entered`.
fn doubling(dir: &std::path::Path) -> Option<(ply_core::Analysis, std::path::PathBuf)> {
    let defs = [
        def("m.double", 1, "  return ply_imm(ply_imm_value(p0) * 2);"),
        def("m.double#entered", 0, &units::int_entry("m.double")),
    ];
    let mut exports = units::exports_of(&defs);
    exports.entered = vec!["m.double".to_string()];
    let lib = units::library(dir, &units::text(&defs, &exports))?;
    Some((units::front(b"doubling", &defs[..1], &[]), lib))
}

/// The unit is loaded once to read what it holds, and every backend over it answers alike.
#[test]
fn every_backend_over_a_unit_answers_what_it_holds() {
    use ply_core::{Provider, Symbol, Value};
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, lib)) = doubling(dir.path()) else {
        return;
    };
    let unit = ply_compiled::Unit::opened(&front, &lib).expect("the unit opens");
    assert_eq!(unit.compiled(), ["m.double", "m.double#entered"]);
    let first = std::sync::Arc::clone(&unit).attach();
    let second = std::sync::Arc::clone(&unit).attach();
    assert_eq!(unit.compilation().units, 2);
    for backend in [&first, &second] {
        assert!(backend.describes(front.hashes_digest));
        assert_eq!(
            backend.enter(&Symbol::new("m.double"), &[Value::Int(21)], 10_000),
            Some(Value::Int(42))
        );
    }
}

/// A program loaded and let go leaves nothing behind: its unit, the front end's answer it holds and
/// its loaded image go with the last backend over it.
#[test]
fn a_unit_nothing_holds_is_freed() {
    use ply_core::Provider;
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, lib)) = doubling(dir.path()) else {
        return;
    };
    let unit = ply_compiled::Unit::opened(&front, &lib).expect("the unit opens");
    let held = std::sync::Arc::downgrade(&unit);
    let backend = unit.attach();
    assert!(held.upgrade().is_some(), "a backend holds its unit");
    drop(backend);
    assert!(held.upgrade().is_none(), "a unit nothing holds was kept");
}
