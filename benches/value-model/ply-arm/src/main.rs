//! The value-model bench's Ply arm, timed in-process like the Rust arm so fixed costs drop out.
//! Usage: `ply-arm <project-dir> c`; prints the Rust arm's line plus `profile=`.

use ply_codegen::source::Source;
use ply_syntax::ast::ModuleName;
use std::time::Instant;

/// k1 is far shorter than k2, so it needs more repeats for its minimum to settle.
const K1_REPEATS: usize = 200;
const K2_REPEATS: usize = 20;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, backend) = match args.as_slice() {
        [_, d, b] => (d.clone(), b.clone()),
        _ => {
            eprintln!("usage: ply-arm <project-dir> c");
            std::process::exit(2);
        }
    };

    let loaded = load(&dir);
    // Offering every function instead fails the tier on the first one outside the fragment.
    let (names, _refused) =
        ply_codegen::closure(loaded, &loaded.functions()).expect("the fragment closes");
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    if backend != "c" {
        eprintln!("unknown backend `{backend}`");
        std::process::exit(2);
    }
    let code = ply_codegen::c::build(loaded, &refs)
        .expect("the C tier builds")
        .0;

    let mut ctx = code.context();
    let layouts: *const ply_codegen::heap::Layouts = &code.tables().layouts;

    // Made once, so building the input stays out of the measurement.
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
    // A `development` (`cc -O0`) reading is not comparable to a `release` bar, so say which.
    println!(
        "k1={k1:.3} k2={k2:.3} digest={hex} profile={}",
        ply_codegen::Profile::current().name()
    );
}

fn call(
    code: &ply_codegen::c::Native,
    ctx: &mut ply_codegen::rt::Ctx,
    name: &str,
    args: &[i64],
) -> i64 {
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
    // The protocol reads a span's module as its position in this list.
    let mut modules: Vec<(String, String)> = Vec::new();
    let mut ids: Vec<ply_span::SourceId> = Vec::new();
    // The whole standard library: the kernels import `std.hash`, which imports others.
    for name in ply_std::modules() {
        let text = ply_std::source(&name).expect("a listed std module has a source");
        let id = sources.add(ply_std::pseudo_path(&name), text.to_string());
        modules.push((name.to_string(), text.to_string()));
        ids.push(id);
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
        modules.push((stem.clone(), text.to_string()));
        ids.push(id);
        units.push((id, ModuleName::from_dotted(&stem), text));
    }
    let mut ast = ply_syntax::parse_program(units).expect("the project parses");
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the project resolves");
    let front = Box::leak(Box::new(
        ply_codegen::c::producer::checked_front(&modules, &ids).expect("the project checks"),
    ));
    let texts: std::collections::HashMap<String, String> = modules.into_iter().collect();
    let program = Box::leak(Box::new(ast));
    let resolved = Box::leak(Box::new(resolved));
    let keys = ply_codegen::emit_keys(front);
    Box::leak(Box::new(
        Source::from_front(program, resolved, front, keys).with_texts(texts),
    ))
}
