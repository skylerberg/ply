//! A second emitter standing in for this one, body by body.
//!
//! ADR 0042's second step: the emitter written in Ply is held to *behaviour*, which means the C
//! tier has to be able to take its bodies. This is the seam. A producer is installed once per
//! process as a way of *building* one -- the compiled Ply emitter is a loaded unit holding `Rc`s,
//! so a worker thread builds its own from the same recipe -- and `emit_one` asks it before it
//! emits. What the producer answers for a body is the same pair the reference emits, the C text
//! and the tables it names by its own positions, in the cache's own encoding; what it declines
//! falls to the reference. The count of what it answered is the ratchet.
//!
//! The producer's own unit is built by the reference, never by itself: `building` is raised
//! around that build, and `current` answers nothing while it is.

use super::build::Native;
use super::emit::Tables;
use crate::source::Source;
use anyhow::{Context, Result, anyhow, bail};
use ply_eval::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// How a thread builds its producer.
pub type Recipe = Arc<dyn Fn() -> Result<PlyProducer, String> + Send + Sync>;

static RECIPE: OnceLock<Recipe> = OnceLock::new();
/// A digest of the emitter's own sources, folded into every cache key a produced body or unit
/// is kept under: a body the last version of the emitter wrote is not this version's.
static IDENTITY: OnceLock<String> = OnceLock::new();

thread_local! {
    static MINE: RefCell<Option<Result<PlyProducer, String>>> = const { RefCell::new(None) };
    static BUILDING: Cell<bool> = const { Cell::new(false) };
    static REFERENCE_ONLY: Cell<bool> = const { Cell::new(false) };
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
/// Every consumer -- the CLI, the corpus, the tests -- calls this, so the override is read in one
/// place rather than wired through each of them. Under tier-only (ADR 0048) the Rust reference
/// emitter is a fragment, so the language runs only when this produces.
pub fn ensure_default() {
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
    let identity = digest_of(&modules_of(&src));
    install(Arc::new(move || build_from(&src)), identity);
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

/// Build the emitter as a producer: parse and check its modules, then build the native emitter
/// from the bootstrap bundle -- or, when `PLY_C_BOOTSTRAP=off` or the bundle does not serve, with
/// the reference emitter, which is how a bundle is refreshed.
fn build_from(src: &Sources) -> Result<PlyProducer, String> {
    use ply_span::SourceId;
    let inputs: Vec<_> = modules_of(src)
        .into_iter()
        .enumerate()
        .map(|(i, (module, text))| {
            let text: &'static str = Box::leak(text.into_boxed_str());
            (
                SourceId(i as u32),
                ply_syntax::ast::ModuleName::from_dotted(&module),
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
    let check = ply_core::check_program(&ast, &resolved).map_err(first)?;
    let program: &'static ply_syntax::ast::Program = Box::leak(Box::new(ast));
    let resolved = Box::leak(Box::new(resolved));
    let check = Box::leak(Box::new(check));
    let keys = ply_hash::hash_program(program, resolved, check)
        .map(|h| crate::source::emit_keys(program, &h))
        .unwrap_or_default();
    let source: &'static Source =
        Box::leak(Box::new(Source::keyed(program, resolved, check, keys)));
    let bundle = (std::env::var("PLY_C_BOOTSTRAP").as_deref() != Ok("off"))
        .then(|| super::bundle::of(src))
        .flatten();
    let (native, _refused) = match bundle {
        Some(bundle) => super::bundle::build(source, &bundle).map_err(|e| format!("{e:#}"))?,
        None => {
            let names: Vec<String> = source.functions();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            super::build(source, &refs).map_err(|e| format!("{e:#}"))?
        }
    };
    PlyProducer::new(native).map_err(|e| format!("{e:#}"))
}

pub fn reset_thread() {
    MINE.with(|mine| *mine.borrow_mut() = None);
}

pub fn identity() -> &'static str {
    IDENTITY.get().map_or("", String::as_str)
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

/// The producer's mode, as the caches key on it. While the producer's own unit is being built
/// the reference is the emitter, whatever was asked for: the producer cannot answer for itself.
///
/// There were two producing modes while ADR 0042 was being walked: body-by-body, where the
/// reference emitted and the port stood in for the bodies the reference had already accepted,
/// and the whole unit. Under tier-only (ADR 0048) the reference is a fragment that refuses
/// `perform`, so body-by-body could only ever offer what the fragment already covered -- less
/// than the fragment alone, since it added no body and could refuse one. The unit is the mode.
pub fn mode() -> &'static str {
    if !installed() || BUILDING.with(Cell::get) || REFERENCE_ONLY.with(Cell::get) {
        "ref"
    } else {
        "ply"
    }
}

/// Runs `f` with the reference emitter forced, whatever producer is installed: [`mode`] answers
/// `ref` and the producer is not consulted or built. The bisection's mixtures are reconstructed
/// ASTs with no source text, which the whole Ply emitter — a front end — cannot re-parse; the
/// reference is an AST consumer and emits the identical C for the effect-free programs a mixture
/// reconstructs. The thread-local is restored on unwind, so a panicking mixture leaves no residue.
pub fn reference_only<R>(f: impl FnOnce() -> R) -> R {
    struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            REFERENCE_ONLY.with(|c| c.set(self.0));
        }
    }
    let _guard = Guard(REFERENCE_ONLY.with(|c| c.replace(true)));
    f()
}

/// The mode with the emitter's identity, as the caches key on it.
pub fn who() -> String {
    let mode = mode();
    if mode == "ref" {
        mode.to_string()
    } else {
        format!("{mode}\0{}", identity())
    }
}

/// Runs `f` with this thread's producer, building it first if the recipe is installed and this
/// thread has not built one. `None` when there is no producer, when it is being built, or when
/// building it failed -- the failure is reported once, and the reference emits everything.
/// Whether the producer's own unit is being built on this thread.
pub fn building() -> bool {
    BUILDING.with(Cell::get)
}

pub fn with_current<T>(f: impl FnOnce(&PlyProducer) -> T) -> Option<T> {
    let recipe = RECIPE.get()?;
    if BUILDING.with(Cell::get) || REFERENCE_ONLY.with(Cell::get) {
        return None;
    }
    MINE.with(|mine| {
        if mine.borrow().is_none() {
            BUILDING.with(|b| b.set(true));
            let built = recipe();
            BUILDING.with(|b| b.set(false));
            if let Err(e) = &built {
                eprintln!(
                    "the Ply emitter could not be built, so the reference emits everything: {e}"
                );
            }
            *mine.borrow_mut() = Some(built);
        }
        match mine.borrow().as_ref() {
            Some(Ok(p)) => Some(f(p)),
            _ => None,
        }
    })
}

/// What the emitter answered for one module: each body's C and tables, by program-wide name.
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

    /// The emitter's C for `name`, or nothing: it did not reach the body, or the program has no
    /// source texts to hand the emitter.
    pub fn body(&self, loaded: &Source, name: &str, module_index: usize) -> Option<Answer> {
        self.asked.set(self.asked.get() + 1);
        loaded.program.modules.get(module_index)?;
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
        for m in &loaded.program.modules {
            let name = m.name.to_string();
            let Some(text) = loaded.texts.get(&name) else {
                return Ok(HashMap::new());
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
        let mut ctx = self.native.context();
        ctx.begin(i64::MAX / 2);
        let layouts: *const crate::heap::Layouts = &self.native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let entry = self
            .native
            .entry(ENTRY)
            .ok_or_else(|| anyhow!("no entry"))?;
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        if ctx.failed != 0 {
            let why = ctx
                .take_failure()
                .map(|d| d.message)
                .unwrap_or_else(|| "no diagnostic".to_string());
            ctx.end();
            bail!("the emitter raised: {why}");
        }
        let value = crate::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        let Value::Str(dump) = &value else {
            bail!("the emitter answered something that is not a string");
        };
        parse(dump).context("reading the emitter's answer")
    }
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
