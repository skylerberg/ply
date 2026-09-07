//! The Ply arm of ADR 0035's gate, measured the way the Rust arm is.
//!
//! `benches/value-model/rust` takes the minimum over `REPEATS` calls **inside one process**. The
//! Ply arm used to run each kernel once per process and take the minimum over three processes, so
//! it carried the whole of a run's fixed cost -- process start, page cache, whatever the allocator
//! was holding -- where the Rust arm carried none of it. At k1's fifth of a millisecond that is
//! most of what was being timed, and it is why one day's six readings put k1 at 2.20, 2.46, 2.76,
//! 2.97, 3.41 and 3.69 against a bar of 3.0.
//!
//! There is no interpreter in what this times. `ply test --audit-backend` reports k1 as `entered 1,
//! declined 0`: one call into compiled code and never back out. So the call below is the same
//! shape the machine makes, without the machine.
//!
//!   ply-arm <project-dir> <c|cranelift>
//!
//! Prints `k1=<ms> k2=<ms> digest=<hex>`, which is the Rust arm's line exactly.

use ply_codegen::source::Source;
use ply_syntax::ast::ModuleName;
use std::time::Instant;

/// The Rust arm's, so the two statistics are the same statistic. k1 gets more because it is two
/// hundred times the shorter: twenty repeats of a fifth of a millisecond is not enough of a
/// sample for the minimum to settle, and the spread of the minimum is what the gate reads as its
/// resolution.
const K1_REPEATS: usize = 200;
const K2_REPEATS: usize = 20;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, backend) = match args.as_slice() {
        [_, d, b] => (d.clone(), b.clone()),
        _ => {
            eprintln!("usage: ply-arm <project-dir> <c|cranelift>");
            std::process::exit(2);
        }
    };

    let loaded = load(&dir);
    // The fragment the tier itself would choose: the largest subset that compiles as one unit.
    // Offering every function instead makes the in-process tier fail on the first one outside it,
    // and `std.config` has one.
    let (names, _refused) =
        ply_codegen::closure(loaded, &loaded.functions()).expect("the fragment closes");
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();

    // Both tiers hand back the same three things: an entry by name, the tables the runtime reads
    // against, and a context wired to them.
    let code: Code = match backend.as_str() {
        "c" => Code::C(
            ply_codegen::c::build(loaded, &refs, ply_codegen::Opts::default())
                .expect("the C tier builds")
                .0,
        ),
        "cranelift" => {
            Code::Jit(ply_codegen::Jit::compile(loaded, &refs).expect("the in-process tier builds"))
        }
        other => {
            eprintln!("unknown backend `{other}`");
            std::process::exit(2);
        }
    };

    let mut ctx = code.context();
    let layouts: *const ply_codegen::heap::Layouts = &code.tables().layouts;

    // The input, made once. Remaking it per repeat would put a 64KB copy inside the measurement.
    let input = call(&code, &mut ctx, "input.k1_input", &[]);
    let mut k1 = f64::MAX;
    let mut digest = 0i64;
    for _ in 0..K1_REPEATS {
        ply_codegen::heap::inc(input);
        let t = Instant::now();
        let d = call(&code, &mut ctx, "std.hash.blake3", &[input]);
        k1 = k1.min(t.elapsed().as_secs_f64() * 1000.0);
        digest = d;
    }

    let steps = ply_codegen::heap::imm(200_000);
    let mut k2 = f64::MAX;
    for _ in 0..K2_REPEATS {
        let t = Instant::now();
        let _ = call(&code, &mut ctx, "kernels.run", &[steps]);
        k2 = k2.min(t.elapsed().as_secs_f64() * 1000.0);
    }

    let answer = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, digest);
    let hex = match &answer {
        ply_eval::Value::Bytes(b) => b.iter().map(|x| format!("{x:02x}")).collect::<String>(),
        other => panic!("blake3 answered {other:?}"),
    };
    // The profile goes in the line because the C tier's default is `development` -- `cc -O0` with
    // the inliner off, forty times slower on k1 -- and a reading taken under it against a bar taken
    // under `release` is a verdict about a compiler flag wearing a verdict about a value model.
    // `run.sh` exports `release` and refuses to start otherwise; this is what puts it in the raw
    // file, where a reader who did not run the script can still see which one it was.
    println!(
        "k1={k1:.3} k2={k2:.3} digest={hex} profile={}",
        ply_codegen::Profile::current().name()
    );
}

// The same allowance `ply_codegen::backend::Code` makes for the same reason: both variants are
// held only to keep the pages their entries point into alive, and there is one of them.
#[allow(clippy::large_enum_variant)]
enum Code {
    Jit(ply_codegen::Unit),
    C(ply_codegen::c::Native),
}

impl Code {
    fn entry(&self, name: &str) -> Option<ply_codegen::jit::Entry> {
        match self {
            Code::Jit(u) => u.entry(name),
            Code::C(n) => n.entry(name),
        }
    }
    fn tables(&self) -> &std::rc::Rc<ply_codegen::rt::Tables> {
        match self {
            Code::Jit(u) => u.tables(),
            Code::C(n) => n.tables(),
        }
    }
    fn context(&self) -> ply_codegen::rt::Ctx {
        match self {
            Code::Jit(u) => u.context(),
            Code::C(n) => n.context(),
        }
    }
}

/// One call into compiled code, the way the seam makes it and with nothing else in the way.
fn call(code: &Code, ctx: &mut ply_codegen::rt::Ctx, name: &str, args: &[i64]) -> i64 {
    let entry = code
        .entry(name)
        .unwrap_or_else(|| panic!("`{name}` was not compiled by this tier"));
    ctx.fuel = 1_000_000_000;
    ctx.failed = 0;
    let w = unsafe { entry(ctx, args.as_ptr()) };
    assert_eq!(ctx.failed, 0, "`{name}` raised");
    w
}

/// The project's own modules and the standard library's, checked together.
fn load(dir: &str) -> &'static Source {
    let mut sources = ply_span::SourceMap::new();
    let mut units: Vec<(ply_span::SourceId, ModuleName, &'static str)> = Vec::new();
    // The whole standard library, under the names it ships with -- the kernels import `std.hash`,
    // and that module imports others.
    for name in ply_std::modules() {
        let text = ply_std::source(&name).expect("a listed std module has a source");
        let id = sources.add(ply_std::pseudo_path(&name), text.to_string());
        units.push((id, name, text));
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("the project directory is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("a module file has a name")
            .to_string();
        let text: &'static str = Box::leak(
            std::fs::read_to_string(&path)
                .expect("readable")
                .into_boxed_str(),
        );
        let id = sources.add(path.to_string_lossy().as_ref(), text.to_string());
        units.push((id, ModuleName::from_dotted(&stem), text));
    }
    let mut ast = ply_syntax::parse_program(units).expect("the project parses");
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the project resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("the project checks");
    Box::leak(Box::new(Source::new(
        Box::leak(Box::new(ast)),
        Box::leak(Box::new(resolved)),
        Box::leak(Box::new(check)),
    )))
}
