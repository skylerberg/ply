//! Building a unit. The admitted set is a fixpoint: a body is taken when it emits *and* every
//! definition it calls is taken, so a set that compiles cannot call out of itself.

use super::Refused;
use super::exports::Exports;
use super::load::{Library, compile_and_load};
use super::tables::{Unit, mangle};
use super::{HELPERS, PRELUDE, helper_addresses, runtime_decls};
use crate::heap::{Heap, Word, mark_immortal};
use crate::rt::Entry;
use crate::rt::{Ctx, Tables};
use crate::source::Source;
use anyhow::{Result, bail};
use ply_span::{SourceId, Symbol};
use std::collections::HashMap;
use std::rc::Rc;

/// A loaded C unit, with the surface `crate::backend::Bodies` asks a compiled unit for.
pub struct Native {
    /// Kept alive because every [`Entry`] below points into its pages.
    lib: Library,
    entries: HashMap<String, (Entry, usize)>,
    constants: HashMap<String, usize>,
    tables: Rc<Tables>,
}

impl Native {
    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries.get(name).map(|(e, _)| *e)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.entries.get(name).map(|(_, a)| *a)
    }

    /// The memo index of a pure nullary root, if `name` is one.
    pub fn constant_index(&self, name: &str) -> Option<usize> {
        self.constants.get(name).copied()
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

/// The definitions offered after `PLY_C_ONLY`/`PLY_C_SKIP`, and the digest refusals are cached by.
fn offered_set<'a>(names: &[&'a str]) -> (Vec<&'a str>, String) {
    let mut offered: Vec<&str> = names.to_vec();
    if let Ok(only) = std::env::var("PLY_C_ONLY") {
        let want: Vec<&str> = only.split(',').filter(|s| !s.is_empty()).collect();
        offered.retain(|n| want.iter().any(|w| n == w));
    }
    if let Ok(skip) = std::env::var("PLY_C_SKIP") {
        let drop: Vec<&str> = skip.split(',').filter(|s| !s.is_empty()).collect();
        offered.retain(|n| !drop.iter().any(|d| n.starts_with(d)));
    }
    // Digest after filtering: refusals depend on the offered set, so narrowed runs must not share.
    let fragment = super::cache::fragment_digest(&offered);
    (offered, fragment)
}

struct Emitted {
    bodies: Vec<(String, String)>,
    unit: Unit,
    taken: Vec<String>,
    refusals: Vec<Refused>,
}

fn emit_all(
    loaded: &'static Source,
    offered: &[&str],
    fragment: &str,
    ctors: &[(Symbol, usize)],
    ctors_digest: &str,
) -> Result<Emitted> {
    let mut taken: Vec<String> = offered.iter().map(|n| (*n).to_string()).collect();
    let mut refusals: Vec<Refused> = Vec::new();

    // Dropping a body can refuse its callers, so repeat until a round refuses nothing.
    let emitted = loop {
        let mut emitted: Vec<(String, String, super::tables::Tables)> = Vec::new();
        let mut round: Vec<Refused> = Vec::new();
        for name in &taken {
            match emit_one(loaded, &taken, name, ctors_digest, fragment) {
                Ok((text, tables)) => emitted.push((name.clone(), text, tables)),
                Err(e) => match e.downcast::<Refused>() {
                    Ok(r) => round.push(r),
                    Err(other) => return Err(other),
                },
            }
        }
        if round.is_empty() {
            break emitted;
        }
        for r in &round {
            taken.retain(|n| n != &r.function);
        }
        refusals.extend(round);
    };
    let mut unit = Unit::new(ctors.to_vec());
    let bodies: Vec<(String, String)> = emitted
        .into_iter()
        .map(|(name, text, tables)| {
            let text = resolve(&text, &tables, &mut unit);
            (name, text)
        })
        .collect();

    if std::env::var("PLY_C_REFUSALS").is_ok() {
        for r in &refusals {
            eprintln!("c tier refused `{}`: {}", r.function, r.construct);
        }
        eprintln!(
            "c tier took {} of {} definitions",
            taken.len(),
            offered.len()
        );
        if let Some((asked, answered)) = super::producer::with_current(|p| p.counts()) {
            eprintln!("ply emitter answered {answered} of {asked} bodies asked of it");
        }
    }
    if let Ok(want) = std::env::var("PLY_C_DUMP") {
        if want == "*" {
            let mut sizes: Vec<(usize, &str)> = bodies
                .iter()
                .map(|(n, b)| (b.lines().count(), n.as_str()))
                .collect();
            sizes.sort_by(|a, b| b.0.cmp(&a.0));
            let lines: usize = sizes.iter().map(|(n, _)| n).sum();
            eprintln!("unit: {lines} lines over {} bodies", bodies.len());
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
    Ok(Emitted {
        bodies,
        unit,
        taken,
        refusals,
    })
}

/// The whole unit as C, uncompiled; the text embeds `exports` so it loads with no source.
pub struct Produced {
    pub text: String,
    pub exports: Exports,
    pub refused: Vec<Refused>,
}

pub fn produce(loaded: &'static Source, names: &[&str]) -> Result<Produced> {
    let ctors = loaded.ctors();
    let ctors_digest = super::cache::ctors_digest(&ctors);
    let (offered, fragment) = offered_set(names);
    produce_in(loaded, &offered, &fragment, &ctors, &ctors_digest)
}

fn produce_in(
    loaded: &'static Source,
    offered: &[&str],
    fragment: &str,
    ctors: &[(Symbol, usize)],
    ctors_digest: &str,
) -> Result<Produced> {
    // Up front, not only when a body misses the cache: a warm cache would otherwise hide it.
    if !offered.is_empty() && loaded.texts.is_empty() {
        bail!("no source text for this program, and the emitter reads a program's text");
    }
    let Emitted {
        bodies,
        unit,
        taken,
        refusals,
    } = emit_all(loaded, offered, fragment, ctors, ctors_digest)?;
    // Never cache a unit over an emitter that raised: fail, and the next run asks again.
    if let Some(why) = super::producer::with_current(|p| p.failure(loaded)).flatten() {
        bail!("the Ply emitter failed over the program: {why}");
    }
    let exports = describe(loaded, unit, taken, &refusals, ctors);
    let text = assemble(&bodies, &exports);
    Ok(Produced {
        text,
        exports,
        refused: refusals,
    })
}

/// What the unit says about itself: the only place a source is read, so loading reads none.
fn describe(
    loaded: &'static Source,
    mut unit: Unit,
    taken: Vec<String>,
    refusals: &[Refused],
    ctors: &[(Symbol, usize)],
) -> Exports {
    let arities: Vec<(String, usize)> = taken
        .iter()
        .map(|n| (n.clone(), loaded.arity_of(n).unwrap_or(0)))
        .collect();
    // A pure nullary function answers the same every time, so the seam memoizes it.
    let constants: Vec<String> = taken
        .iter()
        .filter(|n| {
            loaded.arity_of(n) == Some(0)
                && ply_eval::memo::pure_by_published_row(Some(loaded.check), &Symbol::new(n))
        })
        .cloned()
        .collect();
    // For its effect on the code table: `finish` reads the same slots back.
    let _ = constants_of(&constants, &mut unit);
    Exports {
        helpers: super::exports::runtime_helpers(),
        ctors: ctors.to_vec(),
        taken: arities,
        constants,
        modules: loaded.module_count(),
        refusals: refusals
            .iter()
            .map(|r| (r.function.clone(), r.construct.clone()))
            .collect(),
        consts: unit.consts,
        fields: unit.fields,
        builtins: unit.builtins,
        shapes: unit.layouts.all_shape_names(),
        lambdas: unit.lambdas,
    }
}

/// Load a unit produced elsewhere against its own constructor table. `sources` maps its modules
/// to `SourceId`s for spans; `None` numbers them from zero, as the bootstrap does.
pub fn load_unit(
    text: &str,
    sources: Option<Vec<SourceId>>,
    stem: &str,
) -> Result<(Native, Vec<Refused>)> {
    let lib = compile_and_load(text, stem)?;
    let exports = Exports::read(&lib)?;
    let refused = refused_of(&exports);
    let native = finish(lib, exports, sources)?;
    Ok((native, refused))
}

/// Whether a unit produced elsewhere serves this runtime; refusal is an [`Unserved`].
pub fn served(text: &str, stem: &str) -> Result<()> {
    let lib = compile_and_load(text, stem)?;
    match Exports::read(&lib)?.unserved() {
        Some(why) => Err(why.into()),
        None => Ok(()),
    }
}

fn refused_of(exports: &Exports) -> Vec<Refused> {
    exports
        .refusals
        .iter()
        .map(|(function, construct)| Refused {
            function: function.clone(),
            construct: construct.clone(),
        })
        .collect()
}

pub fn build(loaded: &'static Source, names: &[&str]) -> Result<(Native, Vec<Refused>)> {
    super::sweep::once();
    let started = std::time::Instant::now();
    let ctors = loaded.ctors();
    let ctors_digest = super::cache::ctors_digest(&ctors);
    let (offered, fragment) = offered_set(names);
    // Shared via the file system: `Value` holds `Rc`, so a unit cannot cross rayon workers.
    let unit_key = super::cache::unit_key(
        &loaded.keys,
        &offered,
        &ctors_digest,
        &super::producer::identity(),
    );
    // A unit entry that will not reconstruct is a reason to build one, never to fail.
    if let Some(k) = &unit_key
        && let Some(object) = super::cache::read_unit(k)
        && let Some(lib) = super::load::open_by_key(&object)
        && let Ok(exports) = Exports::read(&lib)
    {
        let refused = refused_of(&exports);
        if let Ok(native) = finish(lib, exports, Some(loaded.module_sources())) {
            super::cache::UNITS_REUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if std::env::var("PLY_C_PHASES").is_ok() {
                eprintln!(
                    "phases: whole unit from cache, {}ms",
                    started.elapsed().as_millis()
                );
            }
            return Ok((native, refused));
        }
    }
    let Produced {
        text,
        refused: refusals,
        ..
    } = produce_in(loaded, &offered, &fragment, &ctors, &ctors_digest)?;
    let t_emit = started.elapsed();
    let t_assemble = started.elapsed();
    let lib = compile_and_load(&text, "unit")?;
    let t_cc = started.elapsed();
    if let Some(k) = &unit_key {
        super::cache::write_unit(k, &super::load::object_key(&text));
    }
    // Read back from the object, so cold and warm builds share one path.
    let exports = Exports::read(&lib)?;
    let native = finish(lib, exports, Some(loaded.module_sources()))?;
    if std::env::var("PLY_C_PHASES").is_ok() {
        eprintln!(
            "phases: emit+resolve {}ms, assemble {}ms, cc+load {}ms, tables {}ms, source {}MB",
            t_emit.as_millis(),
            (t_assemble - t_emit).as_millis(),
            (t_cc - t_assemble).as_millis(),
            (started.elapsed() - t_cc).as_millis(),
            text.len() / 1_000_000,
        );
    }
    Ok((native, refusals))
}

/// A loaded object plus its `Exports`, made into an enterable `Native`.
/// Invariant: every field of `Unit` must be recorded in `Exports`, or the ids move.
fn finish(lib: Library, exports: Exports, sources: Option<Vec<SourceId>>) -> Result<Native> {
    // Before binding: the C reads the first `n` helpers of the table it is handed.
    if let Some(why) = exports.unserved() {
        return Err(why.into());
    }
    let Exports {
        helpers: _,
        ctors,
        taken,
        constants,
        modules,
        refusals: _,
        consts,
        fields,
        builtins,
        shapes,
        lambdas,
    } = exports;
    let sources = sources.unwrap_or_else(|| (0..modules).map(|i| SourceId(i as u32)).collect());
    bind(&lib)?;
    let mut unit = Unit::new(ctors.clone());
    unit.consts = consts;
    unit.fields = fields;
    unit.builtins = builtins;
    unit.lambdas = lambdas;
    // In id order, so the numbers baked into the emitted C still name these shapes.
    for (id, names) in shapes.iter().enumerate() {
        let got = unit.layouts.shape(names.clone());
        if got as usize != id {
            bail!("a unit's shapes do not intern to the ids its C was emitted against");
        }
    }
    // Before the addresses: an uncalled root still needs a row in the code table for its memo slot.
    let constants = constants_of(&constants, &mut unit);
    let mut functions = Vec::with_capacity(unit.lambdas.len());
    for symbol in &unit.lambdas {
        let Some(p) = lib.symbol(symbol) else {
            bail!("the unit the C tier built has no `{symbol}`");
        };
        functions.push(p as usize);
    }
    let mut entries = HashMap::new();
    for (name, arity) in &taken {
        let symbol = format!("{}_entry", mangle(name));
        let Some(p) = lib.symbol(&symbol) else {
            bail!("the unit the C tier built has no `{symbol}`");
        };
        entries.insert(
            name.clone(),
            (
                unsafe { std::mem::transmute::<*mut std::ffi::c_void, Entry>(p) },
                *arity,
            ),
        );
    }
    let mut tables = tables_of(unit, &ctors);
    tables.functions = functions;
    tables.sources = sources;
    Ok(Native {
        lib,
        entries,
        constants,
        tables: Rc::new(tables),
    })
}

/// Each pure nullary root's memo slot: its code-table row, so the seam and `rt_constant` agree.
fn constants_of(constants: &[String], unit: &mut Unit) -> HashMap<String, usize> {
    constants
        .iter()
        .map(|name| {
            let slot = unit.lambda(&format!("{}_entry", mangle(name)));
            (name.clone(), slot)
        })
        .collect()
}

/// The C for one body, or its refusal. `taken` is what a body may call this round.
fn emit_one(
    loaded: &'static Source,
    taken: &[String],
    name: &str,
    ctors_digest: &str,
    fragment: &str,
) -> Result<(String, super::tables::Tables)> {
    let refused = |construct: String| -> anyhow::Error {
        Refused {
            function: name.to_string(),
            construct,
        }
        .into()
    };
    // Keyed by name too: equal-content definitions share a hash but emit their own symbol.
    // A cached body is only reused if all its callees are still taken, or it would not link.
    let identity = super::producer::identity();
    let key = loaded
        .keys
        .get(name)
        .map(|h| super::cache::key(&format!("{name}\0{h}\0{identity}"), ctors_digest));
    let refusal = loaded.keys.get(name).map(|h| {
        super::cache::refusal_key(&format!("{name}\0{h}\0{identity}"), ctors_digest, fragment)
    });
    if let Some(k) = &refusal
        && let Some(reason) = super::cache::read_refusal(k)
    {
        return Err(refused(reason));
    }
    if let Some(k) = &key
        && let Some((text, tables)) = super::cache::read(k)
        && tables.calls.iter().all(|c| taken.contains(c))
    {
        return Ok((text, tables));
    }
    match super::producer::with_current(|p| p.body(loaded, name)).flatten() {
        Some(super::producer::Answer::Body(text, tables)) => {
            if let Some(missing) = tables.calls.iter().find(|c| !taken.contains(c)) {
                return Err(refused(format!(
                    "`{missing}`, which is not in this compiled unit"
                )));
            }
            if let Some(k) = &key {
                super::cache::write(k, &text, &tables);
            }
            Ok((text, tables))
        }
        Some(super::producer::Answer::Refused(why, _)) => {
            if let Some(k) = &refusal {
                super::cache::write_refusal(k, &why);
            }
            Err(refused(why))
        }
        None => Err(refused("which the Ply emitter did not answer".to_string())),
    }
}

/// Rewrite a body's `@@kN@@` placeholders from its own table positions to the unit's.
fn resolve(text: &str, tables: &super::tables::Tables, unit: &mut Unit) -> String {
    let consts: Vec<usize> = tables
        .consts
        .iter()
        .map(|v| unit.constant(v.clone()))
        .collect();
    let builtins: Vec<usize> = tables.builtins.iter().map(|b| unit.builtin(*b)).collect();
    let fields: Vec<usize> = tables.fields.iter().map(|f| unit.field(f)).collect();
    let shapes: Vec<u32> = tables.shapes.iter().map(|n| unit.shape(n)).collect();
    let lambdas: Vec<usize> = tables.lambdas.iter().map(|l| unit.lambda(l)).collect();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("@@") {
        out.push_str(&rest[..at]);
        let body = &rest[at + 2..];
        let end = body
            .find("@@")
            .expect("an emitted placeholder always closes");
        let (kind, digits) = body[..end].split_at(1);
        let i: usize = digits.parse().expect("an emitted placeholder is numbered");
        let resolved = match kind {
            "c" => consts[i],
            "b" => builtins[i],
            "f" => fields[i],
            "s" => shapes[i] as usize,
            "l" => lambdas[i],
            other => unreachable!("an emitted placeholder is one of five kinds, not `{other}`"),
        };
        out.push_str(&resolved.to_string());
        rest = &body[end + 2..];
    }
    out.push_str(rest);
    out
}

fn assemble(bodies: &[(String, String)], exports: &Exports) -> String {
    let mut out = String::from(PRELUDE);
    out.push_str(&runtime_decls());
    out.push_str("\n/* --- prototypes, so a call between two bodies resolves --- */\n");
    for (name, arity) in &exports.taken {
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
    out.push_str(&exports.embed());
    out
}

fn bind(lib: &Library) -> Result<()> {
    let Some(p) = lib.symbol("ply_bind") else {
        bail!("the unit the C tier built has no `ply_bind`");
    };
    let bind: unsafe extern "C" fn(*const *mut std::ffi::c_void) =
        unsafe { std::mem::transmute(p) };
    let addrs = helper_addresses();
    debug_assert_eq!(addrs.len(), HELPERS.len());
    unsafe { bind(addrs.as_ptr()) };
    let Some(p) = lib.symbol("ply_bind_singletons") else {
        bail!("the unit the C tier built has no `ply_bind_singletons`");
    };
    let singletons: unsafe extern "C" fn(Word, Word, Word) = unsafe { std::mem::transmute(p) };
    unsafe {
        singletons(
            crate::heap::bool(true),
            crate::heap::bool(false),
            crate::heap::unit(),
        )
    };
    Ok(())
}

fn tables_of(mut unit: Unit, ctors: &[(Symbol, usize)]) -> Tables {
    unit.layouts.index_fields(&unit.fields);
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
        sources: Vec::new(),
    }
}
