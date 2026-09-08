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
use std::sync::atomic::{AtomicBool, Ordering};
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
}

/// Installs the recipe every thread's producer is built from. The first installation wins; a
/// second is ignored, because a run has one emitter.
pub fn install(recipe: Recipe, identity: String) {
    let _ = IDENTITY.set(identity);
    let _ = RECIPE.set(recipe);
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

static WHOLE: AtomicBool = AtomicBool::new(false);

/// Whether the producer's answer is the unit's: its bodies taken and its refusals dropped by the
/// fixpoint, with the reference emitter not run at all. ADR 0042's third step.
pub fn set_whole(whole: bool) {
    WHOLE.store(whole, Ordering::Relaxed);
}

pub fn whole() -> bool {
    mode() == "ply-whole"
}

/// The producer's mode, as the caches key on it. While the producer's own unit is being built
/// the reference is the emitter, whatever was asked for: the producer cannot answer for itself.
pub fn mode() -> &'static str {
    if !installed() || BUILDING.with(Cell::get) {
        "ref"
    } else if WHOLE.load(Ordering::Relaxed) {
        "ply-whole"
    } else {
        "ply"
    }
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
pub fn with_current<T>(f: impl FnOnce(&PlyProducer) -> T) -> Option<T> {
    let recipe = RECIPE.get()?;
    if BUILDING.with(Cell::get) {
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
        })
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
                    eprintln!("the Ply emitter failed over the program: {e:#}");
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
    /// Every body of the program that handles each operation, answered or refused: the unit's
    /// fixpoint drops a performer while any of its operation's handlers is not taken.
    pub fn handlers_of(&self, loaded: &Source) -> HashMap<String, Vec<String>> {
        let program = std::ptr::from_ref(loaded) as usize;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        if let Some(bodies) = self.modules.borrow().get(&program) {
            for (name, answer) in bodies {
                let handled = match answer {
                    Answer::Body(_, tables) => &tables.handles,
                    Answer::Refused(_, handles) => handles,
                };
                for op in handled {
                    out.entry(op.clone()).or_default().push(name.clone());
                }
            }
        }
        out
    }

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
