mod dump;
mod sweep;

use crate::units::{self, def};
use ply_codegen::c::exports::HelperShape;
use ply_codegen::c::{PRELUDE, builtin_helper_name, helpers};
use ply_codegen::heap::{imm, imm_value};

/// A declaration with no address is a null call at run time: a crash rather than a decline.
#[test]
fn every_declared_helper_has_an_address() {
    for h in helpers() {
        assert!(!h.address.is_null(), "`{}` has no address", h.name);
    }
}

/// A builtin's helper takes the words a call of it passes, under the name the emitter writes.
#[test]
fn every_builtin_has_a_helper_of_its_own() {
    for b in ply_eval::Builtin::all() {
        let name = builtin_helper_name(b.name());
        assert!(
            name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
            "`{name}` is no C name"
        );
        let found: Vec<_> = helpers().iter().filter(|h| h.name == name).collect();
        assert_eq!(found.len(), 1, "`{name}`");
        assert_eq!(found[0].args, b.arity(), "`{name}`");
        assert!(found[0].answers, "`{name}`");
    }
}

#[test]
fn the_prelude_agrees_with_the_layouts_it_mirrors() {
    assert_eq!(ply_codegen::heap::HEADER, 16, "PLY_HEADER");
    assert_eq!(std::mem::size_of::<ply_codegen::heap::Obj>(), 16, "PlyObj");
    assert_eq!(ply_codegen::heap::FLAT, 1, "PLY_FLAT");
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, failed),
        0,
        "PlyCtx.failed"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, fuel),
        8,
        "PlyCtx.fuel"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, stack_floor),
        16,
        "PlyCtx.stack_floor"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, ticks),
        48,
        "PlyCtx.ticks"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, next_tick),
        56,
        "PlyCtx.next_tick"
    );
    assert!(PRELUDE.contains("#define PLY_HEADER 16"));
}

/// A unit binds the helpers it records by name, in its own order, each to this runtime's own.
#[test]
fn a_units_helpers_bind_by_name_to_the_runtimes_own() {
    let mut exports = units::exports_of(&[]);
    exports.helpers.reverse();
    let read = ply_codegen::c::Exports::decode(&exports.encode()).expect("the table reads back");
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

/// The prologue the emitter writes on every body: the fuel it spends, and the stack it moves to
/// when this thread's is nearly out, re-entering through `entry` with the arguments in `args`.
fn prologue(entry: &str, args: &[&str]) -> String {
    let grown = if args.is_empty() {
        format!("return rt_grow_p(ctx, (Word)(intptr_t)&{entry}, 0);")
    } else {
        let mut held = format!("Word ply_grown[{}];", args.len());
        for (i, a) in args.iter().enumerate() {
            held.push_str(&format!(" ply_grown[{i}] = {a};"));
        }
        format!(
            "{held} return rt_grow_p(ctx, (Word)(intptr_t)&{entry}, (Word)(intptr_t)ply_grown);"
        )
    };
    format!(
        "  if (ctx->fuel <= 0) {{ rt_no_fuel_p(ctx); return 0; }}\n\
         \x20 {{ char probe; if ((uintptr_t)&probe < ctx->stack_floor) {{ {grown} }} }}\n\
         \x20 ctx->fuel -= 1;\n"
    )
}

/// `ladder(n)` is `n` calls deep, none of them a tail call; `spin` has no base case.
fn deep_calls() -> [units::Def; 2] {
    let ladder = units::symbol("m.ladder");
    let spin = units::symbol("m.spin");
    [
        def(
            "m.ladder",
            1,
            &format!(
                "{}  int64_t n = ply_imm_value(p0);\n\
                 \x20 Word r = ply_imm(0);\n\
                 \x20 if (n > 0) {{ r = {ladder}(ctx, ply_imm(n - 1)); if (ctx->failed) return 0; r = ply_imm(ply_imm_value(r) + 1); }}\n\
                 \x20 ctx->fuel += 1;\n\
                 \x20 return r;",
                prologue(&format!("{ladder}_entry"), &["p0"])
            ),
        ),
        def(
            "m.spin",
            1,
            &format!(
                "{}  Word r = {spin}(ctx, ply_imm(ply_imm_value(p0) + 1));\n\
                 \x20 if (ctx->failed) return 0;\n\
                 \x20 ctx->fuel += 1;\n\
                 \x20 return ply_imm(ply_imm_value(r) + 1);",
                prologue(&format!("{spin}_entry"), &["p0"])
            ),
        ),
    ]
}

/// A frame that would cross the floor gets a stack of its own, so how deep a program nests is what
/// its fuel says and not what the thread it runs on happens to have been given. The floor is moved
/// up to just under this frame, so the guard fires on any machine rather than on a lucky one.
#[test]
fn a_recursion_past_what_the_stack_holds_grows_onto_another_and_answers() {
    const DEEP: i64 = 20_000;
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &deep_calls(), |_| {}) else {
        return;
    };
    let (native, _) = loaded.expect("the unit loads");
    let entry = native.entry("m.ladder").expect("`m.ladder` is taken");
    let mut ctx = native.context();
    ctx.begin(DEEP * 2);
    let here = 0u8;
    ctx.stack_floor = std::ptr::from_ref(&here) as usize - 64 * 1024;
    let args = [imm(DEEP)];
    let answer = unsafe { entry(&mut ctx, args.as_ptr()) };
    let (failed, grown) = (ctx.failed, ctx.grown);
    let raised = ctx.take_failure().map(|d| d.message);
    ctx.end();
    assert_eq!(failed, 0, "`m.ladder({DEEP})` raised: {raised:?}");
    assert_eq!(imm_value(answer), DEEP);
    assert!(
        grown > 0,
        "the call never crossed the floor, so nothing grew"
    );
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
        ply_codegen::rt::FAILED_OUT_OF_FUEL,
        "a runaway recursion ended some other way: {raised}"
    );
    assert!(raised.contains("bound on nested calls"), "{raised}");
}

/// Each bucket's table reads back as the loader fills it, and a table whose C writes the unit's
/// positions itself names none.
#[test]
fn a_units_bucket_tables_read_back_and_a_table_without_them_names_none() {
    let tables = "helpers 0\nctors 0\ntaken 0\nconstants 0\nmodules 0\nrefused 0\n\
                  consts 0\nbuiltins 0\nfields 0\nshapes 0\nlambdas 0\n";
    let with = format!("{tables}buckets 2\n3 0 2\n58 1\n");
    let read = ply_codegen::c::Exports::decode(&with).expect("the table reads");
    assert_eq!(read.buckets, vec![(3, vec![0, 2]), (58, vec![1])]);
    assert_eq!(read.encode(), with);
    let without = ply_codegen::c::Exports::decode(tables).expect("the table reads");
    assert!(without.buckets.is_empty());
    assert!(without.instances.is_empty());
    // What a type's module states its values are read through follows the buckets, in a unit
    // that holds such a type: `-` for none, `!` for a function the unit does not take.
    let stating = format!("{with}instances 2\nm.Box m.area !\nm.Date - m.written\n");
    let read = ply_codegen::c::Exports::decode(&stating).expect("the table reads");
    assert_eq!(
        read.instances,
        vec![
            ("m.Box".into(), "m.area".to_string(), "!".to_string()),
            ("m.Date".into(), "-".to_string(), "m.written".to_string()),
        ]
    );
    assert!(read.numerics.is_empty());
    assert_eq!(read.encode(), stating);
    // A type that states `numeric` lists its five functions after those two, in the roles' order.
    let computing = format!("{with}instances 1\nm.N m.held - m.add m.sub m.mul ! m.of\n");
    let read = ply_codegen::c::Exports::decode(&computing).expect("the table reads");
    let names = ["m.add", "m.sub", "m.mul", "!", "m.of"].map(str::to_string);
    assert_eq!(read.numerics, vec![("m.N".into(), names)]);
    assert_eq!(read.encode(), computing);
    assert!(
        ply_codegen::c::Exports::decode(&format!("{with}instances 1\nm.N - - m.add\n")).is_none()
    );
}

/// `N` states its arithmetic: `add`, `sub` and `mul` take two words and `neg` and `of_int` one.
fn numbered() -> Vec<units::Def> {
    vec![
        def("m.held", 1, "  return p0;"),
        def("m.add", 2, "  (void)p1; return p0;"),
        def("m.neg", 1, "  return p0;"),
        def("m.of", 1, "  return p0;"),
    ]
}

fn stating(e: &mut ply_codegen::c::Exports, functions: [&str; 5]) {
    e.ctors = vec![(ply_eval::Symbol::new("m.N"), 1)];
    e.instances = vec![(
        ply_eval::Symbol::new("m.N"),
        "m.held".to_string(),
        "-".to_string(),
    )];
    e.numerics = vec![(ply_eval::Symbol::new("m.N"), functions.map(str::to_string))];
}

/// An operator at a type that states `numeric` calls the function the unit lists for it with as
/// many words as the operation takes, so a unit that lists one of another arity is not loaded.
#[test]
fn a_unit_listing_arithmetic_of_another_arity_is_refused_by_the_function_it_names() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &numbered(), |e| {
        stating(e, ["m.add", "m.add", "m.add", "m.neg", "m.of"])
    }) else {
        return;
    };
    loaded.expect("a unit stating its arithmetic at the words each takes loads");
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(loaded) = units::opened(dir.path(), &numbered(), |e| {
        stating(e, ["m.neg", "m.add", "m.add", "m.neg", "m.of"])
    }) else {
        return;
    };
    let refused = loaded
        .err()
        .expect("an `add` that takes one word was bound");
    assert!(refused.contains("`m.neg`"), "{refused}");
}

/// A stated `key` or `show` is called with the value alone, so a unit that states one taking more
/// words is not loaded.
#[test]
fn a_unit_stating_a_function_of_more_than_the_value_is_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let defs = [def("m.scaled", 2, "  (void)p1; return p0;")];
    let Some(loaded) = units::opened(dir.path(), &defs, |e| {
        e.ctors = vec![(ply_eval::Symbol::new("m.Box"), 1)];
        e.instances = vec![(
            ply_eval::Symbol::new("m.Box"),
            "m.scaled".to_string(),
            "-".to_string(),
        )];
    }) else {
        return;
    };
    let refused = loaded.err().expect("a key that takes two words was bound");
    assert!(refused.contains("`m.scaled`"), "{refused}");
}

/// `m.double`, compiled and opened as a program's unit.
fn doubling(dir: &std::path::Path) -> Option<(ply_eval::Analysis, std::path::PathBuf)> {
    let defs = [def(
        "m.double",
        1,
        "  return ply_imm(ply_imm_value(p0) * 2);",
    )];
    let lib = units::library(dir, &units::text(&defs, &units::exports_of(&defs)))?;
    Some((units::front(b"doubling", &defs, &[]), lib))
}

/// The unit is loaded once to read what it holds, and every backend over it answers alike.
#[test]
fn every_backend_over_a_unit_answers_what_it_holds() {
    use ply_eval::{Provider, Symbol, Value};
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, lib)) = doubling(dir.path()) else {
        return;
    };
    let unit = ply_codegen::Unit::opened(&front, &lib).expect("the unit opens");
    assert_eq!(unit.compiled(), ["m.double"]);
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
    use ply_eval::Provider;
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some((front, lib)) = doubling(dir.path()) else {
        return;
    };
    let unit = ply_codegen::Unit::opened(&front, &lib).expect("the unit opens");
    let held = std::sync::Arc::downgrade(&unit);
    let backend = unit.attach();
    assert!(held.upgrade().is_some(), "a backend holds its unit");
    drop(backend);
    assert!(held.upgrade().is_none(), "a unit nothing holds was kept");
}
