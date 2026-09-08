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

thread_local! {
    static MINE: RefCell<Option<Result<PlyProducer, String>>> = const { RefCell::new(None) };
    static BUILDING: Cell<bool> = const { Cell::new(false) };
}

/// Installs the recipe every thread's producer is built from. The first installation wins; a
/// second is ignored, because a run has one emitter.
pub fn install(recipe: Recipe) {
    let _ = RECIPE.set(recipe);
}

pub fn installed() -> bool {
    RECIPE.get().is_some()
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
type Bodies = HashMap<String, (String, Tables)>;

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
const ENTRY: &str = "emit.emit_bodies_all";

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
    pub fn body(
        &self,
        loaded: &Source,
        name: &str,
        module_index: usize,
    ) -> Option<(String, Tables)> {
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
        if found.is_some() {
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
        let (Some("body"), Some(name), Some(n), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!("a frame header that is not `body <name> <n>`: {header:?}");
        };
        let n: usize = n.parse().context("a frame's length")?;
        let start = line_end + 1;
        let end = start + n;
        if end > bytes.len() {
            bail!("a frame of {n} bytes past the end of the answer");
        }
        let chunk = std::str::from_utf8(&bytes[start..end]).context("a frame that is not UTF-8")?;
        let (text, tables) = super::cache::decode(chunk)
            .ok_or_else(|| anyhow!("`{name}`'s frame does not decode as a body"))?;
        out.insert(name.to_string(), (text, tables));
        at = end;
    }
    Ok(out)
}
