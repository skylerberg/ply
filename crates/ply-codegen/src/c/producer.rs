//! The emitter written in Ply, as the C tier's producer (ADR 0042).
//!
//! A producer is installed once per process as a way of *building* one -- the compiled Ply emitter
//! is a loaded unit holding `Rc`s, so a worker thread builds its own from the same recipe -- and
//! `emit_one` asks it for every body. What it answers for a body is the C text and the tables it
//! names by its own positions, in the cache's own encoding.
//!
//! The producer's own unit comes from a bootstrap bundle, never from itself: `BUILDING` is raised
//! around that build, and [`with_current`] answers nothing on the thread while it is.

use super::build::Native;
use super::tables::Tables;
use crate::source::Source;
use anyhow::{Context, Result, anyhow, bail};
use ply_eval::{Fields, Value};
use ply_span::{Severity, SourceId, Symbol};
use ply_ty::{Front, read_front};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// How a thread builds its producer.
pub type Recipe = Arc<dyn Fn() -> Result<PlyProducer, String> + Send + Sync>;

static RECIPE: OnceLock<Recipe> = OnceLock::new();
/// A digest of the emitter's own sources, folded into every cache key a produced body or unit
/// is kept under: a body the last version of the emitter wrote is not this version's.
static IDENTITY: OnceLock<String> = OnceLock::new();
/// What the port's answers are a function of besides what it is asked: the units that may serve
/// and the helper table they bind.
static EMITTER: OnceLock<String> = OnceLock::new();

thread_local! {
    static MINE: RefCell<Option<Result<PlyProducer, String>>> = const { RefCell::new(None) };
    static BUILDING: Cell<bool> = const { Cell::new(false) };
    /// A producer handed over for the duration of a call, with the identity the caches must key
    /// its bodies under. This is how one emitter emits another's sources: the thread-locals above
    /// carry one emitter per run.
    static HANDED: RefCell<Option<(PlyProducer, String)>> = const { RefCell::new(None) };
}

/// Installs the recipe every thread's producer is built from. The first installation wins; a
/// second is ignored, because a run has one emitter.
pub fn install(recipe: Recipe, identity: String) {
    let _ = IDENTITY.set(identity);
    let _ = RECIPE.set(recipe);
}

/// Where the emitter's own source comes from.
///
/// [`Sources::Embedded`] is `ply-compiler`, which this binary carries: no file system, so a
/// shipped `ply` compiles through the self-hosted emitter wherever it is run from. A directory is
/// a working copy, which `PLY_C_EMITTER=ply:<dir>` names -- the way to run a change to the emitter
/// before it has been bootstrapped into the bundle.
#[derive(Clone, Debug)]
pub enum Sources {
    Embedded,
    Directory(std::path::PathBuf),
}

/// Install the self-hosted Ply emitter as the producer when none is installed: the working copy
/// `PLY_C_EMITTER=ply:<dir>` names, or the one this binary carries.
///
/// [`with_current`] calls this, so the override is read in one place rather than wired through
/// each consumer.
pub fn ensure_default() {
    if installed() {
        return;
    }
    install_sources(match std::env::var("PLY_C_EMITTER") {
        Ok(spec) => match spec.strip_prefix("ply:") {
            Some(dir) => Sources::Directory(std::path::PathBuf::from(dir)),
            None => {
                eprintln!(
                    "PLY_C_EMITTER is `{spec}`; the spelling is `ply:<dir>`, the directory the \
                     emitter's own `.ply` sources are in"
                );
                Sources::Embedded
            }
        },
        Err(_) => Sources::Embedded,
    });
}

/// The same, from a working copy on disk.
pub fn install_sources(src: Sources) {
    if installed() {
        return;
    }
    let identity = identity_of(&src);
    let _ = EMITTER.set(emitter_of(&src, &identity));
    install(Arc::new(move || build(&src)), identity);
}

/// The digest the emitter `src` holds keys its bodies under.
pub fn identity_of(src: &Sources) -> String {
    digest_of(&modules_of(src))
}

/// A working copy is served by its own bundle, or by the committed one as is or emitting it.
fn emitter_of(src: &Sources, identity: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(super::exports::helpers_digest().as_bytes());
    if let Some(carried) = super::bundle::of(&Sources::Embedded) {
        h.update(carried.unit());
    }
    if let Sources::Directory(_) = src {
        h.update(identity.as_bytes());
        if let Some(own) = super::bundle::of(src) {
            h.update(own.unit());
        }
    }
    h.finalize().to_hex().to_string()
}

/// The emitter's modules -- the standard library, then the emitter's own -- as `(name, text)`
/// pairs, for the identity digest and for the parse.
///
/// **The order is the identity**, and it is `ply_compiler::MODULES`'s: a directory walk is not
/// ordered, and this digest keys every body the emitter produces, so reading a working copy sorts
/// what it finds into the same order the embedded list is in.
fn modules_of(src: &Sources) -> Vec<(String, String)> {
    let mut modules: Vec<(String, String)> = ply_std::sources()
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    match src {
        Sources::Embedded => {
            modules.extend(ply_compiler::sources().map(|(m, t)| (m.to_string(), t.to_string())))
        }
        Sources::Directory(dir) => {
            let mut found: Vec<(String, String)> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "ply")
                        && let Ok(text) = std::fs::read_to_string(&p)
                    {
                        found.push((
                            p.file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            text,
                        ));
                    }
                }
            }
            found.sort();
            modules.extend(found);
        }
    }
    modules
}

/// Build the emitter as a producer: the native emitter from these sources' own bundle, which
/// reads none of them -- or, when they have no bundle or it does not serve, emitted by the
/// **committed** emitter this binary carries, handed over for the build. A working copy is the
/// ordinary case: `PLY_C_EMITTER=ply:<dir>` names one before a bundle is bootstrapped for it, and
/// what stands it up is the emitter already in hand, not the Rust chain (ADR 0052 §2).
pub fn build(src: &Sources) -> Result<PlyProducer, String> {
    let from_committed = || -> Result<(super::Native, Vec<super::Refused>), String> {
        let carried = super::bundle::of(&Sources::Embedded)
            .ok_or_else(|| "this binary carries no bootstrap bundle".to_string())?;
        let (native, refused) = super::bundle::build(&carried).map_err(|e| {
            format!(
                "the committed bundle does not serve this runtime either: {e:#}. Check out an \
                 older bundle this runtime serves from git history, then refresh it with \
                 `PLY_C_BOOTSTRAP_REFRESH=1 cargo nextest run -p ply-codegen-tests --test bootstrap`"
            )
        })?;
        // A working copy holding the sources the bundle was emitted from *is* that bundle, which
        // the fixpoint test asserts, so emitting them again answers the same unit for a minute of
        // work. Only a working copy that differs is worth standing an emitter up for.
        let theirs = identity_of(src);
        if carried.sources_digest() == Some(theirs.as_str()) {
            return Ok((native, refused));
        }
        let first = PlyProducer::new(native).map_err(|e| format!("{e:#}"))?;
        let identity = identity_of(&Sources::Embedded);
        with_producer(first, identity, || {
            let source = front_end(src)?;
            let names: Vec<String> = source.functions();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let (native, refused) = super::build(source, &refs).map_err(|e| format!("{e:#}"))?;
            // `PlyProducer::new` would say only that the entry is missing, which does not say
            // whether the front end never offered it or the fragment refused its body.
            if native.entry(ENTRY).is_none() {
                let head: Vec<String> = refused.iter().take(5).map(|r| r.to_string()).collect();
                return Err(format!(
                    "the committed emitter emitted no `{ENTRY}`: {} roots offered, `{ENTRY}` \
                     {} among them, {} refused in all{}",
                    names.len(),
                    if names.iter().any(|n| n == ENTRY) {
                        "was"
                    } else {
                        "was NOT"
                    },
                    refused.len(),
                    if head.is_empty() {
                        String::new()
                    } else {
                        format!("; first refusals: {}", head.join(" | "))
                    },
                ));
            }
            Ok((native, refused))
        })
    };
    let (native, _refused) = match super::bundle::of(src) {
        Some(bundle) => match super::bundle::build(&bundle) {
            Ok(built) => built,
            Err(e) if e.downcast_ref::<super::exports::Unserved>().is_some() => {
                eprintln!(
                    "the bootstrap bundle does not serve: {e:#}; the committed emitter builds it"
                );
                from_committed()?
            }
            Err(e) => return Err(format!("{e:#}")),
        },
        None => from_committed()?,
    };
    PlyProducer::new(native).map_err(|e| format!("{e:#}"))
}

/// The emitter's own program through the front end, keyed as the cache keys it, with the modules
/// as `SourceId(0..n)` in `modules_of`'s order. The check and the hashes come from the emitter
/// handed over by [`build`]: the port answers for the program it is about to become.
fn front_end(src: &Sources) -> Result<&'static Source, String> {
    let modules = modules_of(src);
    let ids: Vec<SourceId> = (0..modules.len()).map(|i| SourceId(i as u32)).collect();
    let inputs: Vec<_> = modules
        .iter()
        .enumerate()
        .map(|(i, (module, text))| {
            let text: &'static str = Box::leak(text.clone().into_boxed_str());
            (
                SourceId(i as u32),
                ply_syntax::ast::ModuleName::from_dotted(module),
                text,
            )
        })
        .collect();
    let first = |ds: Vec<ply_span::Diagnostic>| {
        ds.first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "no diagnostic".to_string())
    };
    let mut ast = ply_syntax::parse_program(inputs).map_err(first)?;
    let expanded = ply_derive::expand_program(&mut ast);
    if !expanded.is_empty() {
        return Err(first(expanded));
    }
    let resolved = ply_syntax::resolve::resolve(&mut ast).map_err(first)?;
    let program: &'static ply_syntax::ast::Program = Box::leak(Box::new(ast));
    let resolved = Box::leak(Box::new(resolved));
    // The handed-over emitter's answer. This used to be the Rust chain's, on the argument that
    // the port cannot answer for the program it is itself compiled from -- true of the emitter
    // being built, and not of the one already in hand.
    let front = Box::leak(Box::new(
        front(&modules, &ids).map_err(|e| format!("{e:#}"))?,
    ));
    let keys = crate::source::emit_keys(front);
    let texts: HashMap<String, String> = modules.iter().cloned().collect();
    Ok(Box::leak(Box::new(
        Source::from_front(program, resolved, front, keys).with_texts(texts),
    )))
}

pub fn reset_thread() {
    MINE.with(|mine| *mine.borrow_mut() = None);
}

/// The digest the emitter answering on this thread keys its bodies under. Installs the default
/// first, as [`with_current`] does: a key taken before the emitter is known would file its bodies
/// under no emitter at all.
pub fn identity() -> String {
    if let Some(id) = HANDED.with(|h| h.borrow().as_ref().map(|(_, id)| id.clone())) {
        return id;
    }
    ensure_default();
    IDENTITY.get().cloned().unwrap_or_default()
}

/// What the emitter answering on this thread is, as a cache of its answers keys on it.
pub fn emitter() -> String {
    if HANDED.with(|h| h.borrow().is_some()) {
        return identity();
    }
    ensure_default();
    EMITTER.get().cloned().unwrap_or_else(identity)
}

/// The identity of an emitter given as its modules' sources, in any order.
pub fn digest_of(modules: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = modules.iter().collect();
    sorted.sort();
    let mut h = blake3::Hasher::new();
    for (name, text) in sorted {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(text.as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex()[..16].to_string()
}

pub fn installed() -> bool {
    RECIPE.get().is_some()
}

/// Runs `f` with `p` as this thread's producer and `identity` as the digest its emissions key
/// under, whatever is installed. `with_current` answers `p` and [`identity`] answers the string,
/// so a body emitted here is filed under the emitter that emitted it rather than the one being
/// built (ADR 0052 §2).
pub fn with_producer<R>(p: PlyProducer, identity: String, f: impl FnOnce() -> R) -> R {
    let _held = hand_over(p, identity);
    f()
}

/// The same handover held by a guard rather than wrapped around a closure, for a caller whose
/// body is a test rather than an expression. `p` is this thread's producer until [`Handed`]
/// drops, and what was handed over before comes back then -- so two handovers nest the way
/// [`with_producer`] does, and neither is a `OnceLock` that a second caller loses to.
pub fn hand_over(p: PlyProducer, identity: String) -> Handed {
    Handed(HANDED.with(|h| h.borrow_mut().replace((p, identity))))
}

pub struct Handed(Option<(PlyProducer, String)>);

impl Drop for Handed {
    fn drop(&mut self) {
        HANDED.with(|h| *h.borrow_mut() = self.0.take());
    }
}

/// Runs `f` with this thread's producer: the one handed over, else the one built from the
/// installed recipe, the default installed first so that a caller cannot reach an emitter that
/// answers nothing by forgetting [`ensure_default`]. `None` while it is being built, or when
/// building it failed -- the failure is reported once.
pub fn with_current<T>(f: impl FnOnce(&PlyProducer) -> T) -> Option<T> {
    // An emitter handed over serves before anything else: this is how one emitter emits another's
    // sources, which a nested ask could not otherwise do.
    if HANDED.with(|h| h.borrow().is_some()) {
        return HANDED.with(|h| h.borrow().as_ref().map(|(p, _)| f(p)));
    }
    if BUILDING.with(Cell::get) {
        return None;
    }
    ensure_default();
    let recipe = RECIPE.get()?;
    MINE.with(|mine| {
        if mine.borrow().is_none() {
            BUILDING.with(|b| b.set(true));
            let built = recipe();
            BUILDING.with(|b| b.set(false));
            if let Err(e) = &built {
                eprintln!("the Ply emitter could not be built, so every body is refused: {e}");
            }
            *mine.borrow_mut() = Some(built);
        }
        match mine.borrow().as_ref() {
            Some(Ok(p)) => Some(f(p)),
            _ => None,
        }
    })
}

/// What the emitter said about one definition.
#[derive(Clone)]
pub enum Answer {
    Body(String, Tables),
    /// The reason, and the operations the body would have handled, as `effect#op`.
    Refused(String, Vec<String>),
}

type Bodies = HashMap<String, Answer>;

/// The compiled Ply emitter, entered once per module of the program being compiled.
pub struct PlyProducer {
    native: Native,
    /// Every body the emitter answered, by program-wide name, filled the first time any body of a
    /// program is asked for; keyed on the program's address, since one producer serves a thread.
    modules: RefCell<HashMap<usize, Bodies>>,
    asked: Cell<u64>,
    answered: Cell<u64>,
    /// Why the emitter raised over a program, by the program's address: a unit built over its
    /// silence would cache the failure as the program's bodies.
    failed: RefCell<HashMap<usize, String>>,
}

/// The entry the emitter is entered through: `emit_bodies_all(names, srcs, ctors, builtins)`,
/// over every module of the program at once, so that it resolves them together.
const ENTRY: &str = "emit.emit_unit_all";

impl PlyProducer {
    /// Over a unit that holds the Ply emitter: the front end and `emit.ply`, compiled.
    pub fn new(native: Native) -> Result<PlyProducer> {
        if native.entry(ENTRY).is_none() {
            bail!("the unit has no `{ENTRY}`, so it is not the Ply emitter");
        }
        Ok(PlyProducer {
            native,
            modules: RefCell::new(HashMap::new()),
            asked: Cell::new(0),
            answered: Cell::new(0),
            failed: RefCell::new(HashMap::new()),
        })
    }

    /// Why the emitter raised over `loaded`, when it did.
    pub fn failure(&self, loaded: &Source) -> Option<String> {
        let program = std::ptr::from_ref(loaded) as usize;
        self.failed.borrow().get(&program).cloned()
    }

    /// Bodies asked for and bodies answered, over this thread's life.
    pub fn counts(&self) -> (u64, u64) {
        (self.asked.get(), self.answered.get())
    }

    /// The emitter's C for `name`, or nothing: it did not reach the body, or it failed over the
    /// program, which [`PlyProducer::failure`] then says.
    pub fn body(&self, loaded: &Source, name: &str) -> Option<Answer> {
        self.asked.set(self.asked.get() + 1);
        let program = std::ptr::from_ref(loaded) as usize;
        if !self.modules.borrow().contains_key(&program) {
            let bodies = match self.bodies_of(loaded) {
                Ok(b) => b,
                Err(e) => {
                    self.failed.borrow_mut().insert(program, format!("{e:#}"));
                    HashMap::new()
                }
            };
            self.modules.borrow_mut().insert(program, bodies);
        }
        let found = self
            .modules
            .borrow()
            .get(&program)
            .and_then(|m| m.get(name))
            .cloned();
        if matches!(found, Some(Answer::Body(..))) {
            self.answered.set(self.answered.get() + 1);
        }
        found
    }

    /// Every body of the program at once: the emitter resolves the modules together, so a call's
    /// default arguments are filled and every signature is in reach.
    fn bodies_of(&self, loaded: &Source) -> Result<Bodies> {
        let mut names = Vec::new();
        let mut srcs = Vec::new();
        for module in loaded.module_names() {
            let name = module.to_string();
            let Some(text) = loaded.texts.get(&name) else {
                bail!("no source text for module `{name}`, and the emitter reads a program's text");
            };
            names.push(Value::bytes(name.as_bytes()));
            srcs.push(Value::bytes(text.as_bytes()));
        }
        let ctors = Value::list(
            loaded
                .ctors()
                .into_iter()
                .map(|(n, _)| Value::bytes(n.as_str().as_bytes()))
                .collect(),
        );
        let builtins = Value::list(
            ply_eval::Builtin::all()
                .iter()
                .map(|b| Value::bytes(b.name().as_bytes()))
                .collect(),
        );
        let args = [Value::list(names), Value::list(srcs), ctors, builtins];
        let value = self.call(ENTRY, &args)?;
        let Value::Str(dump) = &value else {
            bail!("the emitter answered something that is not a string");
        };
        parse(dump).context("reading the emitter's answer")
    }

    /// Enters any function of the unit -- `module.name`, as the emitter's sources spell it -- with
    /// `args` as values, in a context of its own, and answers what it returned.
    ///
    /// This is how the differentials hold the self-hosted front end to the reference: the same
    /// unit that emits every program's C also parses, resolves, checks and hashes, and each of
    /// those phases dumps its answer as a string. Entering it here, once per input, is what
    /// replaced generating a program around every input and running `ply` over it.
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value> {
        let entry = self
            .native
            .entry(name)
            .ok_or_else(|| anyhow!("the unit has no `{name}`"))?;
        let mut ctx = self.native.context();
        ctx.begin(i64::MAX / 2);
        let layouts: *const crate::heap::Layouts = &self.native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        if ctx.failed != 0 {
            let why = ctx
                .take_failure()
                .map(|d| d.message)
                .unwrap_or_else(|| "no diagnostic".to_string());
            ctx.end();
            bail!("`{name}` raised: {why}");
        }
        let value = crate::heap::Heap::to_value(unsafe { &*layouts }, answer);
        note_census(&ctx);
        ctx.end();
        Ok(value)
    }
}

/// What this thread's entries into the compiled compiler have cost since the census was last
/// reset: the compiled compiler's own cost, apart from any harness or reference around it, in
/// the units the value model is about (ADR 0051 §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub entries: usize,
    pub allocated: usize,
    pub recycled: usize,
    /// The most chunk bytes any one entry held at its end.
    pub chunk_bytes: usize,
}

thread_local! {
    static CENSUS: Cell<Census> = const { Cell::new(Census { entries: 0, allocated: 0, recycled: 0, chunk_bytes: 0 }) };
}

fn note_census(ctx: &crate::rt::Ctx) {
    CENSUS.with(|c| {
        let mut census = c.get();
        census.entries += 1;
        census.allocated += ctx.heap.allocated();
        census.recycled += ctx.heap.recycled();
        census.chunk_bytes = census.chunk_bytes.max(ctx.heap.chunk_bytes());
        c.set(census);
    });
}

/// Starts this thread's census afresh, so a test reads only what it entered.
pub fn reset_census() {
    CENSUS.with(|c| c.set(Census::default()));
}

pub fn census() -> Census {
    CENSUS.with(|c| c.get())
}

/// The entry the whole front end answers through: the diagnostics, the load order, the checker's
/// output, the hashes, the item ordinals and the stored bodies, as `ply_ty::front` reads them.
const FRONT: &str = "front.front_dump";

/// The port's whole answer over a program — `ids` naming the source each module's spans point
/// into, in the same order the modules are handed over.
///
/// **A refusal of the program is in the answer, not in the `Err`.** The driver asks this before it
/// has an opinion of its own, so a type error is `front.diagnostics` and reaches the terminal like
/// any other; `Err` is the seam failing — no emitter on this thread, a dump that does not read.
/// A caller that expects the program to check asks [`checked_front`] instead.
pub fn front(sources: &[(String, String)], ids: &[SourceId]) -> Result<Front> {
    if sources.len() != ids.len() {
        bail!(
            "{} module(s) handed over with {} source id(s)",
            sources.len(),
            ids.len()
        );
    }
    let dump = front_dump(sources)?;
    read_front(&dump, ids).map_err(|e| anyhow!("the front end's answer does not read: {e}"))
}

/// The port's whole answer as it gives it, before [`read_front`] reads it.
pub fn front_dump(sources: &[(String, String)]) -> Result<String> {
    let records: Vec<Value> = sources
        .iter()
        .map(|(name, src)| {
            Value::Record(Arc::new(Fields::from_unsorted(vec![
                (Symbol::new("name"), Value::bytes(name.as_bytes())),
                (Symbol::new("src"), Value::bytes(src.as_bytes())),
            ])))
        })
        .collect();
    let answer = call(FRONT, &[Value::list(records)])?;
    let Value::Str(dump) = &answer else {
        bail!(
            "`{FRONT}` answered a {} rather than a string",
            answer.type_name()
        );
    };
    Ok(dump.to_string())
}

/// [`front`] over the default producer, with the program's errors raised rather than answered.
pub fn checked_front(sources: &[(String, String)], ids: &[SourceId]) -> Result<Front> {
    ensure_default();
    let front = front(sources, ids)?;
    let errors: Vec<String> = front
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} [{}]", d.message, d.code))
        .collect();
    if !errors.is_empty() {
        bail!("the program does not check: {}", errors.join("; "));
    }
    Ok(front)
}

/// Enters `name` in this thread's compiled emitter, building it first when the thread has none.
pub fn call(name: &str, args: &[Value]) -> Result<Value> {
    with_current(|p| p.call(name, args)).unwrap_or_else(|| {
        bail!("no Ply emitter serves on this thread: it is being built, or building it failed")
    })
}

/// `body <name> <n>\n` and then exactly `n` bytes, repeated: the tables in the cache's encoding,
/// `text\n`, and the C.
fn parse(dump: &str) -> Result<Bodies> {
    let mut out = HashMap::new();
    let bytes = dump.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        let line_end = dump[at..]
            .find('\n')
            .map(|i| at + i)
            .ok_or_else(|| anyhow!("an unterminated frame header"))?;
        let header = &dump[at..line_end];
        let mut parts = header.split(' ');
        let (Some(kind), Some(name), Some(n), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!(
                "a frame header that is not `body <name> <n>` or `refused <name> <n>`: {header:?}"
            );
        };
        let n: usize = n.parse().context("a frame's length")?;
        let start = line_end + 1;
        let end = start + n;
        if end > bytes.len() {
            bail!("a frame of {n} bytes past the end of the answer");
        }
        let chunk = std::str::from_utf8(&bytes[start..end]).context("a frame that is not UTF-8")?;
        let answer = match kind {
            "body" => {
                let (text, tables) = super::cache::decode(chunk)
                    .ok_or_else(|| anyhow!("`{name}`'s frame does not decode as a body"))?;
                Answer::Body(text, tables)
            }
            "refused" => {
                let (why, handles) = chunk.split_once("\nhandles ").unwrap_or((chunk, ""));
                Answer::Refused(
                    why.to_string(),
                    handles
                        .split(' ')
                        .filter(|h| !h.is_empty())
                        .map(str::to_string)
                        .collect(),
                )
            }
            other => bail!("a frame of a kind this seam does not read: {other:?}"),
        };
        out.insert(name.to_string(), answer);
        at = end;
    }
    Ok(out)
}
