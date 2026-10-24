//! Units written by hand: C bodies over the prelude and the runtime's own object, beside the table a
//! unit says about itself, compiled by the host's C compiler into a library the runtime opens. What
//! the emitter writes is the compiler's to test; these test what the runtime does with a unit.

use ply_compiled::c::exports::runtime_helpers;
use ply_compiled::c::{Exports, PRELUDE, runtime_header, runtime_object};
use std::ops::{Deref, DerefMut};
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

/// The C pointer a unit calls the native the compiler declares as `name` through.
pub fn native(name: &str) -> String {
    ply_compiled::c::pointer_name(&ply_compiled::c::builtin_helper_name(name))
}

/// The C that opens an `R#entered`: the arguments' list read, `read` reading each, and the
/// lifting ended, returning where the context failed.
pub fn lifting(read: &str) -> String {
    format!(
        "  {}(ctx, {});\n{read}\n  {}(ctx);\n  if (ctx->failed) return 0;\n",
        native("?in_count"),
        ply_core::codec::tag::LIST,
        native("?lift_close"),
    )
}

/// The C of an `R#entered` that reads one `Int`, calls `root` over it and writes the `Int` it answers.
pub fn int_entry(root: &str) -> String {
    format!(
        "{}  Word r = {}(ctx, ply_imm(n));\n  if (ctx->failed) return 0;\n  {}(ctx, ply_imm_value(r));\n  return 0;",
        lifting(&format!("  Word n = {}(ctx);", native("?in_int"))),
        symbol(root),
        native("?out_int"),
    )
}

/// The C function `name`'s body is written as.
pub fn symbol(name: &str) -> String {
    format!("ply_t_{}", name.replace(['.', '#'], "_"))
}

/// One definition a unit holds: its Ply name, how many words it takes, and the C symbol of the
/// entry the runtime calls it through.
pub struct Taken {
    pub name: String,
    pub arity: usize,
    pub entry: String,
}

/// A unit as a test writes one: what it says about itself, which it stands in for, and what its C
/// lays out for the loader by address.
pub struct Laid {
    says: Exports,
    /// The code table's rows, each by the C symbol of its function.
    pub lambdas: Vec<String>,
    pub taken: Vec<Taken>,
    /// The definitions the seam memoizes, each of which takes the row of the code table its entry
    /// is at.
    pub constants: Vec<String>,
}

impl Deref for Laid {
    type Target = Exports;

    fn deref(&self) -> &Exports {
        &self.says
    }
}

impl DerefMut for Laid {
    fn deref_mut(&mut self) -> &mut Exports {
        &mut self.says
    }
}

/// A unit of `defs`: it says every helper this runtime has, and lays out each definition under
/// its own symbol.
pub fn exports_of(defs: &[Def]) -> Laid {
    Laid {
        says: Exports {
            helpers: runtime_helpers(),
            constants: Vec::new(),
            modules: 1,
            refusals: Vec::new(),
            fields: Vec::new(),
            builtins: Vec::new(),
            entered: Vec::new(),
        },
        lambdas: Vec::new(),
        taken: defs
            .iter()
            .map(|d| Taken {
                name: d.name.to_string(),
                arity: d.arity,
                entry: format!("{}_entry", symbol(d.name)),
            })
            .collect(),
        constants: Vec::new(),
    }
}

/// The unit's C: the prelude and the runtime declared, each definition and the entry the runtime
/// calls it through, the runtime's object, and what the unit lays out for its loader: the table
/// `exports` says, and the code table `exports.lambdas` names and the definitions `exports.taken`
/// names, each by address.
pub fn text(defs: &[Def], exports: &Laid) -> String {
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
    let slot_of = |name: &String| {
        let entry = &(exports.taken.iter().find(|t| t.name == *name))
            .unwrap_or_else(|| panic!("the unit does not take the constant `{name}`"))
            .entry;
        (exports.lambdas.iter().position(|l| l == entry))
            .unwrap_or_else(|| panic!("the code table has no `{entry}`"))
    };
    let said = Exports {
        constants: (exports.constants.iter())
            .map(|name| (name.clone(), slot_of(name)))
            .collect(),
        ..exports.says.clone()
    };
    out.push_str("const char ply_exports[] =\n");
    for line in said.encode().lines() {
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
    out.push_str("void *const ply_code[] = {");
    for lambda in &exports.lambdas {
        out.push_str(&format!(" (void *){lambda},"));
    }
    out.push_str(" 0 };\n");
    let mut taken: Vec<&Taken> = exports.taken.iter().collect();
    taken.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    out.push_str(
        "struct ply_taken { const char *name; void *entry; unsigned long arity; };\n\
         const struct ply_taken ply_taken[] = {",
    );
    for t in &taken {
        out.push_str(&format!(
            " {{ \"{}\", (void *){}, {} }},",
            t.name, t.entry, t.arity
        ));
    }
    out.push_str(&format!(
        " {{ 0, 0, 0 }} }};\n\
         struct ply_descriptor {{ const char *exports; unsigned long exports_len; void *bind; \
         void *bind_singletons; void *const *code; unsigned long code_count; \
         const struct ply_taken *taken; unsigned long taken_count; }};\n\
         const struct ply_descriptor ply_descriptor = {{ ply_exports, sizeof ply_exports - 1, \
         (void *)ply_bind, (void *)0, ply_code, {}, ply_taken, {} }};\n",
        exports.lambdas.len(),
        taken.len()
    ));
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
    adjust: impl FnOnce(&mut Laid),
) -> Option<Result<(ply_compiled::c::Native, Vec<ply_compiled::Refused>), String>> {
    let mut exports = exports_of(defs);
    adjust(&mut exports);
    let lib = library(dir, &text(defs, &exports))?;
    Some(ply_compiled::c::load_opened(&lib, None).map_err(|e| format!("{e:#}")))
}

/// The front end's answer a unit of `defs` is opened over, its hashes digesting to `program`'s: each
/// definition a root of plain words, pure where `pure` names it.
pub fn front(program: &[u8], defs: &[Def], pure: &[&str]) -> ply_core::Analysis {
    ply_core::Analysis {
        hashes_digest: ply_core::DefHash::of(program),
        emitter_roots: defs
            .iter()
            .map(|d| ply_core::EmitterRoot {
                root: ply_core::Symbol::new(d.name),
                arity: d.arity,
                pure: pure.contains(&d.name),
                span: ply_core::Span::DUMMY,
                params: vec![ply_core::Carry::Plain; d.arity],
                answer: ply_core::Carry::Plain,
                witnesses: Vec::new(),
            })
            .collect(),
        ..ply_core::Analysis::default()
    }
}

/// One entry of `native`'s `name`, with `fuel` nested calls to spend: what it answered as a word,
/// or what it raised.
pub fn entered(
    native: &ply_compiled::c::Native,
    name: &str,
    fuel: i64,
    args: &[ply_compiled::heap::Word],
) -> Result<ply_compiled::heap::Word, String> {
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
