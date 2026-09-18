//! Building a unit: which bodies the tier takes, the C it emits for them, and the tables the
//! runtime reads them against.
//!
//! The admitted set is a fixpoint: a body is taken when it emits *and* every definition it calls
//! is taken, so a set that compiles cannot call out of itself.

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
    ///
    /// The same rule the in-process tier applies, and the same reason: a nullary function whose
    /// published row says it is pure answers the same thing every time, so the seam remembers it
    /// rather than running it. Without this the gate's kernel rebuilds a sixty-five-kilobyte byte
    /// literal on every call, which the other tier does not.
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

/// The definitions actually offered, after the two bisecting instruments, and the digest a refusal
/// is cached against. Both callers need the same answer: a refusal is cached against this digest,
/// so an instrument that narrows the offered set has to move it.
fn offered_set<'a>(names: &[&'a str]) -> (Vec<&'a str>, String) {
    let mut offered: Vec<&str> = names.to_vec();
    // A bisecting instrument: compile only the definitions named, so that a wrong answer can be
    // narrowed to the body that produces it. The fixpoint then refuses whatever calls the rest.
    if let Ok(only) = std::env::var("PLY_C_ONLY") {
        let want: Vec<&str> = only.split(',').filter(|s| !s.is_empty()).collect();
        offered.retain(|n| want.iter().any(|w| n == w));
    }
    // The other half of `PLY_C_ONLY`, and the usable one at corpus scale: an allow-list of 1400
    // names does not fit in an environment variable, and a truncated one silently compiles a
    // different program than the one asked for. A prefix to *drop* is short whatever the corpus.
    if let Ok(skip) = std::env::var("PLY_C_SKIP") {
        let drop: Vec<&str> = skip.split(',').filter(|s| !s.is_empty()).collect();
        offered.retain(|n| !drop.iter().any(|d| n.starts_with(d)));
    }
    // *Then* the digest, over what is actually offered rather than over what the caller asked
    // for. A refusal is cached against this, because a body is refused when something it calls was
    // not offered -- so an instrument that narrows the offered set has to move the digest with it.
    // Filtering after it meant a bisecting run's refusals were served back to an unfiltered one,
    // which built a unit neither run would produce and crashed in it. That cost most of a day.
    let fragment = super::cache::fragment_digest(&offered);
    (offered, fragment)
}

/// Everything the fixpoint settles on: every body's resolved C, the unit its tables ended in, the
/// definitions it kept, and the ones it refused.
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

    // The fixpoint: emit everything, drop what refused, and go round again, because dropping a
    // body can refuse the ones that call it. A body's text names the unit's tables by its own
    // positions, so nothing here touches the unit and a round is the emitting alone.
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
        // A compiled `perform` searches the compiled handler frames, which is complete for an
        // operation while every body handling it that can run is compiled (ADR 0043). Under
        // tier-only nothing outside the unit runs, so a handler outside it -- a test's, when a unit
        // is offered an artifact's closure -- is never on the stack, and the performer compiles.
        // A `perform` no compiled handler answers reaches the host binding from the runtime, with
        // the machine's checks.
        if round.is_empty() {
            break emitted;
        }
        for r in &round {
            taken.retain(|n| n != &r.function);
        }
        refusals.extend(round);
    };
    // The tables the settled set actually needs, filled once rather than once per round.
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

/// The whole unit as C, without compiling it, and what it says about itself.
///
/// What an artifact embeds and what a bootstrap archives: `build` compiles and loads; this stops
/// one step earlier and hands back the text, so a tree can keep its front end as a C file that
/// any C compiler turns into a working front end -- the only form of "check in the compiler" that
/// is neither a per-platform binary nor a dependency on the compiler being replaced. The text
/// carries `exports` as its last declaration, so it runs from its own definitions with no source
/// to re-parse.
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
    // An emitter that raised answered nothing, and a unit over that silence would be cached as
    // the program's bodies: the failure is the answer, and the next run asks again.
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

/// What the unit says about itself, from the program it was emitted from: the only place a
/// source is read for it. Everything a loader needs is in here, so that loading reads none.
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
    // A nullary function whose published row says it is pure answers the same thing every time,
    // so the seam remembers it rather than running it. Without this the gate's kernel rebuilds a
    // sixty-five-kilobyte byte literal on every call.
    let constants: Vec<String> = taken
        .iter()
        .filter(|n| {
            loaded.arity_of(n) == Some(0)
                && ply_eval::memo::pure_by_published_row(Some(loaded.check), &Symbol::new(n))
        })
        .cloned()
        .collect();
    // For its effect on the code table, whose rows are recorded just below: `finish` reads the
    // same slots back out of the table this completes.
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

/// A unit produced elsewhere -- an artifact's, a bundle's -- compiled, loaded and finished against
/// what it says about itself: its constructor table rather than any program's, so the tags baked
/// into its C still name its shapes, and no source at all. `sources` is the `SourceId` of each
/// module it was emitted from, in module order, for the spans its bodies store; `None` numbers
/// them from zero, which is what the bootstrap assigns.
pub fn load_unit(
    text: &str,
    sources: Option<Vec<SourceId>>,
    stem: &str,
) -> Result<(Native, Vec<Refused>)> {
    finish_unit(compile_and_load(text, stem)?, sources)
}

/// A unit's object, however it was compiled, finished against what it says about itself.
pub(super) fn finish_unit(
    lib: Library,
    sources: Option<Vec<SourceId>>,
) -> Result<(Native, Vec<Refused>)> {
    let exports = Exports::read(&lib)?;
    let refused = refused_of(&exports);
    let native = finish(lib, exports, sources)?;
    Ok((native, refused))
}

/// Whether a unit produced elsewhere serves this runtime: it compiles, its table reads back, and
/// the runtime's helper table starts with its own. The refusal is an [`Unserved`] a caller can
/// tell from a unit that is broken.
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
    // Before the build rather than after it, and once in the process: what this bounds is what the
    // cache is left holding, and a build that starts by making room needs no second pass over a
    // directory it has just filled.
    super::sweep::once();
    let started = std::time::Instant::now();
    let ctors = loaded.ctors();
    let ctors_digest = super::cache::ctors_digest(&ctors);
    let (offered, fragment) = offered_set(names);
    // A unit this binary already built, against this program, this constructor table and this
    // emitter. Sharing the built unit in process is not open to us -- `ply_eval::Value` holds
    // `Rc`, so nothing containing one crosses a rayon worker -- so it is shared through the same
    // file system the objects are.
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
    // Read back from the object rather than kept from the emit, so the two doors are one path: a
    // unit whose table will not read back fails every test rather than only a warm one.
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

/// A loaded object plus what it says about itself, made into the `Native` a caller can enter.
///
/// Every door reaches it -- the build that just emitted the unit, the whole-unit cache, an
/// artifact, the bootstrap bundle -- with an `Exports` read from the object, so the reconstruction
/// is not a second implementation to be kept in step: there is one, and every build exercises it.
///
/// A `Unit` is exactly what this rebuilds: its consts, fields, builtins and lambdas are recorded
/// as they are, and its `Layouts` is the constructor table plus the shapes interned in id order. Nothing else is in a `Unit`, which is why a recording of those
/// is faithful; if a field is ever added to one, it has to be added to `Exports` too or the ids
/// move.
fn finish(lib: Library, exports: Exports, sources: Option<Vec<SourceId>>) -> Result<Native> {
    // Before anything is bound: the C reads the first `n` positions of the table it is handed, so
    // the table has to start with the one it was emitted against.
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
    // For the spans bodies store, by the index of the module they were emitted from. An
    // artifact's program is rebuilt from its definitions, so its modules need not be the ones the
    // unit was emitted from; a span then names the module at that index, or nothing.
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
    // Before the addresses, because it can add a row: a root nothing calls still needs a slot for
    // the seam to remember it in, and the slot is a row of the same table.
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

/// The memo slot of every pure nullary root, which is also the row of the unit's code table that
/// `rt_constant` enters it through.
///
/// One numbering, not two. The seam reads a slot to answer without entering, and `rt_constant`
/// reads the same slot to answer without calling; they have to agree or a value remembered by one
/// is invisible to the other. Interning the entry symbol into the code table is what makes them
/// agree, and it is why this runs before the addresses are looked up: a root the compiled code
/// never calls is not in the table yet and still needs a slot.
///
/// The emitter's own test is narrower -- it also asks that the answer be a handle, since a root
/// that answers a register is cheaper to call than to look up. A root listed here and not emitted
/// against simply keeps a slot only the seam uses, which is what the in-process tier does too.
///
/// Which roots are constants is `describe`'s answer, carried in the unit's `Exports`.
fn constants_of(constants: &[String], unit: &mut Unit) -> HashMap<String, usize> {
    constants
        .iter()
        .map(|name| {
            let slot = unit.lambda(&format!("{}_entry", mangle(name)));
            (name.clone(), slot)
        })
        .collect()
}

/// The C for one body, or the refusal that stopped it. `taken` is the set the fixpoint holds this
/// round: what a body may call.
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
    // Kept from a previous run, when the caller said what this body is a function of and the
    // definitions it calls are still ones this unit has. The check on the calls is what makes a
    // restored body safe: the fixpoint may have dropped a callee since, and a body that names one
    // it no longer has would not link.
    // The *name* as well as the hash. A definition's hash is over its content, deliberately, so
    // two definitions that say the same thing share one -- and an emitted body carries its own
    // mangled name, so serving one for the other puts two definitions of the same symbol in the
    // unit. `lexer.hex1` and `lexer.hex2` are that pair, and the C compiler said so.
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

/// A body's placeholders, resolved against the unit it is going into.
///
/// The text names a constant, a builtin, a field or a shape by *its own* position, so that the
/// text is a function of the body alone. This is where those become the unit's positions.
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

/// The whole translation unit: the prelude, the runtime, every body's prototype, the bodies, and
/// last what the unit says about itself.
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

/// The tables the runtime reads the unit against: the constant pool made immortal, the shapes, the
/// field names and the builtins the bodies named.
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
