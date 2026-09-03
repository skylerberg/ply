//! Building a unit: which bodies the tier takes, the C it emits for them, and the tables the
//! runtime reads them against.
//!
//! The admitted set is a fixpoint, exactly as the Cranelift tier's is: a body is taken when it
//! emits *and* every definition it calls is taken, so a set that compiles cannot call out of
//! itself. The difference from the other tier is only what comes out the end — text, then one
//! process and one link.

use super::emit::{Emit, Unit, mangle};
use super::load::{Library, compile_and_load};
use super::{HELPERS, PRELUDE, helper_addresses, runtime_decls};
use crate::heap::{Heap, Word, mark_immortal};
use crate::jit::{Entry, Opts, Refused};
use crate::rt::{Ctx, Tables};
use crate::source::Source;
use anyhow::{Result, bail};
use ply_eval::code::lower_fn;
use ply_span::Symbol;
use std::collections::HashMap;
use std::rc::Rc;

/// A loaded C unit, with the surface `crate::backend::Bodies` asks a compiled unit for.
pub struct Native {
    /// Kept alive because every [`Entry`] below points into its pages.
    lib: Library,
    entries: HashMap<String, (Entry, usize)>,
    tables: Rc<Tables>,
}

impl Native {
    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries.get(name).map(|(e, _)| *e)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.entries.get(name).map(|(_, a)| *a)
    }

    /// The C tier has no memo of pure nullary roots yet; every call runs.
    pub fn constant_index(&self, _name: &str) -> Option<usize> {
        None
    }

    pub fn tables(&self) -> &Rc<Tables> {
        &self.tables
    }

    pub fn context(&self) -> Ctx {
        Ctx::new(self.tables.clone())
    }

    pub fn source_path(&self) -> &std::path::Path {
        self.lib.path()
    }
}

/// Emit, compile and load `names` as one unit, with what it refused.
pub fn build(
    loaded: &'static Source,
    names: &[&str],
    _opts: Opts,
) -> Result<(Native, Vec<Refused>)> {
    let ctors = loaded.ctors();
    let mut taken: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    let mut refusals: Vec<Refused> = Vec::new();

    // The fixpoint: emit everything, drop what refused, and go round again, because dropping a
    // body can refuse the ones that call it.
    let (unit, bodies) = loop {
        let mut unit = Unit::new(ctors.clone(), taken.clone());
        let mut bodies: Vec<(String, String)> = Vec::new();
        let mut round: Vec<Refused> = Vec::new();
        for name in &taken {
            match emit_one(loaded, &mut unit, name) {
                Ok(text) => bodies.push((name.clone(), text)),
                Err(e) => match e.downcast::<Refused>() {
                    Ok(r) => round.push(r),
                    Err(other) => return Err(other),
                },
            }
        }
        if round.is_empty() {
            break (unit, bodies);
        }
        for r in &round {
            taken.retain(|n| n != &r.function);
        }
        refusals.extend(round);
    };

    let arities: Vec<(String, usize)> = taken
        .iter()
        .filter_map(|n| {
            loaded
                .definition(n)
                .map(|(d, _)| (n.clone(), d.params.len()))
        })
        .collect();
    if std::env::var("PLY_C_REFUSALS").is_ok() {
        for r in &refusals {
            eprintln!("c tier refused `{}`: {}", r.function, r.construct);
        }
        eprintln!("c tier took {} of {} definitions", taken.len(), names.len());
    }
    let text = assemble(&bodies, &arities);
    if let Ok(want) = std::env::var("PLY_C_DUMP") {
        if want == "*" {
            let mut sizes: Vec<(usize, &str)> = bodies
                .iter()
                .map(|(n, b)| (b.lines().count(), n.as_str()))
                .collect();
            sizes.sort_by(|a, b| b.0.cmp(&a.0));
            eprintln!("unit: {} lines over {} bodies", text.len(), bodies.len());
            for (n, name) in sizes.iter().take(8) {
                eprintln!("  {n:6} lines  {name}");
            }
        }
        for (name, body) in &bodies {
            if *name == want {
                eprintln!("--- {name} ---\n{body}");
            }
        }
    }
    let lib = compile_and_load(&text, "unit")?;
    bind(&lib)?;

    let mut entries = HashMap::new();
    for name in &taken {
        let Some((def, _)) = loaded.definition(name) else {
            continue;
        };
        let symbol = format!("{}_entry", mangle(name));
        let Some(p) = lib.symbol(&symbol) else {
            bail!("the unit the C tier built has no `{symbol}`");
        };
        entries.insert(
            name.clone(),
            (
                unsafe { std::mem::transmute::<*mut std::ffi::c_void, Entry>(p) },
                def.params.len(),
            ),
        );
    }
    let tables = Rc::new(tables_of(unit, &ctors));
    Ok((
        Native {
            lib,
            entries,
            tables,
        },
        refusals,
    ))
}

/// The C for one body, or the refusal that stopped it.
fn emit_one(loaded: &'static Source, unit: &mut Unit, name: &str) -> Result<String> {
    let Some((def, module_index)) = loaded.definition(name) else {
        return Err(Refused {
            function: name.to_string(),
            construct: "no definition".to_string(),
        }
        .into());
    };
    let body = crate::opt::optimize(loaded, module_index, def, crate::opt::Inlining::EMITTED);
    let params: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
    let lowered = lower_fn(&params, &body);
    let mut e = Emit::new(loaded, unit, name, module_index);
    // Not `static`: an exported body carries a symbol, and a symbol is what lets a
    // sampling profiler attribute time to a Ply definition. The Cranelift tier cannot be read
    // this way at all, which is a real difference between the two and not a small one.
    let mut head = format!("Word {}(PlyCtx *ctx", mangle(name));
    let declared: Vec<super::emit::CTy> = match loaded
        .check
        .defs
        .get(&ply_span::Symbol::new(name))
        .map(|d| &d.scheme.ty)
    {
        Some(ply_core::ty::Type::Fn { params, .. }) => {
            params.iter().map(super::emit::CTy::of).collect()
        }
        _ => vec![super::emit::CTy::Unknown; params.len()],
    };
    for (i, p) in params.iter().enumerate() {
        head.push_str(&format!(", Word p{i}"));
        e.param(
            p,
            format!("p{i}"),
            declared
                .get(i)
                .cloned()
                .unwrap_or(super::emit::CTy::Unknown),
        );
    }
    head.push_str(") {\n");
    // The prologue `ply_eval::limit` needs: one nested call spent here and given back on the
    // normal return, so a compiled recursion is bounded by the number the machine bounds an
    // interpreted one by.
    head.push_str("  if (ctx->fuel <= 0) { rt_no_fuel_p(ctx); return 0; }\n  ctx->fuel -= 1;\n");
    let answer = e.expr(&lowered.code)?;
    let word = e.word(&answer);
    let mut out = head;
    out.push_str(&e.token_decls());
    out.push_str(&e.out);
    out.push_str(&format!("  ctx->fuel += 1;\n  return {word};\n}}\n"));
    // The entry the seam and a closure reach the body through, over the handle ABI.
    out.push_str(&format!(
        "Word {0}_entry(PlyCtx *ctx, const Word *args) {{\n  return {0}(ctx{1});\n}}\n",
        mangle(name),
        (0..params.len())
            .map(|i| format!(", args[{i}]"))
            .collect::<Vec<_>>()
            .join("")
    ));
    Ok(out)
}

/// The whole translation unit: the prelude, the runtime, every body's prototype, then the bodies.
fn assemble(bodies: &[(String, String)], taken: &[(String, usize)]) -> String {
    let mut out = String::from(PRELUDE);
    out.push_str(&runtime_decls());
    out.push_str("\n/* --- prototypes, so a call between two bodies resolves --- */\n");
    for (name, arity) in taken {
        let params = std::iter::once("PlyCtx*".to_string())
            .chain((0..*arity).map(|_| "Word".to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("Word {}({params});\n", mangle(name)));
    }
    out.push('\n');
    for (_, text) in bodies {
        out.push_str(text);
        out.push('\n');
    }
    out
}

/// Hand the loaded unit the runtime's addresses.
fn bind(lib: &Library) -> Result<()> {
    let Some(p) = lib.symbol("ply_bind") else {
        bail!("the unit the C tier built has no `ply_bind`");
    };
    let bind: unsafe extern "C" fn(*const *mut std::ffi::c_void) =
        unsafe { std::mem::transmute(p) };
    let addrs = helper_addresses();
    debug_assert_eq!(addrs.len(), HELPERS.len());
    unsafe { bind(addrs.as_ptr()) };
    Ok(())
}

/// The tables the runtime reads the unit against: the constant pool made immortal, the shapes, the
/// field names and the builtins the bodies named.
fn tables_of(unit: Unit, ctors: &[(Symbol, usize)]) -> Tables {
    let mut immortals = Heap::persistent();
    let mut const_words = Vec::with_capacity(unit.consts.len());
    for v in &unit.consts {
        const_words.push(immortals.immortal(&unit.layouts, v));
    }
    let mut nullaries = Vec::with_capacity(ctors.len());
    for (index, (_, arity)) in ctors.iter().enumerate() {
        let w = if *arity == 0 {
            let w = immortals.alloc(crate::heap::KIND_CTOR, 0, 0, index as u32) as Word;
            mark_immortal(w);
            w
        } else {
            0
        };
        nullaries.push(w);
    }
    let empty_list = immortals.list_from(&[]);
    mark_immortal(empty_list);
    let empty_map = immortals.map_new();
    mark_immortal(empty_map);
    Tables {
        consts: unit.consts,
        const_words,
        layouts: unit.layouts,
        fields: unit.fields,
        builtins: unit.builtins,
        functions: Vec::new(),
        memo: Default::default(),
        immortals: std::cell::RefCell::new(immortals),
        bytes: std::cell::RefCell::new([0; 256]),
        nullaries,
        empty_list,
        empty_map,
        memo_values: Default::default(),
        memo_words: Default::default(),
        calls: Default::default(),
    }
}
