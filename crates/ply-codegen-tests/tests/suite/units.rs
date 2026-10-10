//! Units written by hand: C bodies over the prelude and the runtime's own object, beside the table a
//! unit says about itself, compiled by the host's C compiler into a library the runtime opens. What
//! the emitter writes is the compiler's to test; these test what the runtime does with a unit.

use ply_codegen::c::exports::{Taken, runtime_helpers};
use ply_codegen::c::{Exports, PRELUDE, runtime_header, runtime_object};
use std::path::{Path, PathBuf};

/// A definition the unit takes: its name, how many words it takes, and its body's C, which reads
/// them as `p0`, `p1`, ... and the context as `ctx`.
pub struct Def {
    pub name: &'static str,
    pub arity: usize,
    pub body: String,
}

pub fn def(name: &'static str, arity: usize, body: &str) -> Def {
    Def {
        name,
        arity,
        body: body.to_string(),
    }
}

/// The C function `name`'s body is written as.
pub fn symbol(name: &str) -> String {
    format!("ply_t_{}", name.replace('.', "_"))
}

/// The table a unit of `defs` says about itself: every helper this runtime has, and each definition
/// taken under its own symbol.
pub fn exports_of(defs: &[Def]) -> Exports {
    Exports {
        helpers: runtime_helpers(),
        ctors: Vec::new(),
        taken: defs
            .iter()
            .map(|d| Taken {
                name: d.name.to_string(),
                arity: d.arity,
                symbol: symbol(d.name),
                entry: format!("{}_entry", symbol(d.name)),
            })
            .collect(),
        constants: Vec::new(),
        modules: 1,
        refusals: Vec::new(),
        consts: Vec::new(),
        fields: Vec::new(),
        builtins: Vec::new(),
        shapes: Vec::new(),
        lambdas: Vec::new(),
        buckets: Vec::new(),
        instances: Vec::new(),
        numerics: Vec::new(),
    }
}

/// The unit's C: the prelude and the runtime declared, each definition and the entry the runtime
/// calls it through, the runtime's object, and `exports` as the loader reads it.
pub fn text(defs: &[Def], exports: &Exports) -> String {
    let mut out = format!("{PRELUDE}{}\n", runtime_header());
    for d in defs {
        let params: String = (0..d.arity).map(|i| format!(", Word p{i}")).collect();
        let args: String = (0..d.arity).map(|i| format!(", args[{i}]")).collect();
        let s = symbol(d.name);
        out.push_str(&format!(
            "Word {s}(PlyCtx *ctx{params});\n\
             Word {s}_entry(PlyCtx *ctx, const Word *args);\n\
             Word {s}(PlyCtx *ctx{params}) {{\n{}\n}}\n\
             Word {s}_entry(PlyCtx *ctx, const Word *args) {{\n  (void)args;\n  return {s}(ctx{args});\n}}\n",
            d.body
        ));
    }
    out.push_str(&runtime_object());
    out.push_str("const char ply_exports[] =\n");
    for line in exports.encode().lines() {
        out.push('"');
        for b in line.bytes() {
            match b {
                b'"' => out.push_str("\\\""),
                b'\\' => out.push_str("\\\\"),
                0x20..=0x7e if b != b'?' => out.push(b as char),
                _ => out.push_str(&format!("\\{b:03o}")),
            }
        }
        out.push_str("\\n\"\n");
    }
    out.push_str(";\n");
    out
}

/// `text` compiled into a library under `dir`, as the toolchain compiles a unit; `None` where the
/// host has no C compiler.
pub fn library(dir: &Path, text: &str) -> Option<PathBuf> {
    let c = dir.join("unit.c");
    let lib = dir.join("unit.so");
    std::fs::write(&c, text).expect("the unit's C is written");
    let cc = std::env::var("PLY_CC").unwrap_or_else(|_| "cc".to_string());
    let mut command = std::process::Command::new(cc);
    command.args(["-shared", "-fPIC", "-fno-strict-aliasing", "-O1", "-o"]);
    command.arg(&lib).arg(&c);
    let out = match command.output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("the C compiler could not run: {e}"),
    };
    assert!(
        out.status.success(),
        "the C compiler refused the unit:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(lib)
}

/// `defs` compiled and opened, its table as `exports_of` makes it and then as `adjust` leaves it.
pub fn opened(
    dir: &Path,
    defs: &[Def],
    adjust: impl FnOnce(&mut Exports),
) -> Option<Result<(ply_codegen::c::Native, Vec<ply_codegen::Refused>), String>> {
    let mut exports = exports_of(defs);
    adjust(&mut exports);
    let lib = library(dir, &text(defs, &exports))?;
    Some(ply_codegen::c::load_opened(&lib, None).map_err(|e| format!("{e:#}")))
}

/// The front end's answer a unit of `defs` is opened over, its hashes digesting to `program`'s: each
/// definition a root of plain `Int`s, none of them pure.
pub fn front(program: &[u8], defs: &[Def]) -> ply_eval::Analysis {
    ply_eval::Analysis {
        hashes_digest: ply_eval::DefHash::of(program),
        emitter_roots: defs
            .iter()
            .map(|d| ply_eval::EmitterRoot {
                root: ply_eval::Symbol::new(d.name),
                arity: d.arity,
                pure: false,
                span: ply_eval::Span::DUMMY,
                params: vec![ply_eval::Carry::Plain; d.arity],
                answer: ply_eval::Carry::Plain,
                witnesses: Vec::new(),
            })
            .collect(),
        ..ply_eval::Analysis::default()
    }
}

/// One entry of `native`'s `name`, with `fuel` nested calls to spend: what it answered as a word,
/// or what it raised.
pub fn entered(
    native: &ply_codegen::c::Native,
    name: &str,
    fuel: i64,
    args: &[ply_codegen::heap::Word],
) -> Result<ply_codegen::heap::Word, String> {
    let entry = native
        .entry(name)
        .unwrap_or_else(|| panic!("the unit does not take `{name}`"));
    let mut ctx = native.context();
    ctx.begin(fuel);
    let answer = unsafe { entry(&mut ctx, args.as_ptr()) };
    let failed = ctx.failed;
    let raised = ctx.take_failure().map(|d| d.message);
    ctx.end();
    if failed == 0 {
        Ok(answer)
    } else {
        Err(raised.unwrap_or_default())
    }
}
