//! The emitter written in Ply, as the C tier's producer. A recipe is installed per process and
//! each thread builds its own, since the loaded unit holds `Rc`s.

use super::build::Native;
use super::tables::Tables;
use crate::source::Source;
use anyhow::{Context, Result, anyhow, bail};
use ply_eval::{Fields, Value};
use ply_span::frames::Cursor;
use ply_span::{Severity, SourceId, Symbol};
use ply_ty::{Front, read_front};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// How a thread builds its producer.
pub type Recipe = Arc<dyn Fn() -> Result<PlyProducer, String> + Send + Sync>;

static RECIPE: OnceLock<Recipe> = OnceLock::new();
/// A digest of the emitter's own sources, folded into every cache key.
static IDENTITY: OnceLock<String> = OnceLock::new();
/// What the emitter's answers depend on besides its input: the serving units and helper table.
static EMITTER: OnceLock<String> = OnceLock::new();

thread_local! {
    static MINE: RefCell<Option<Result<PlyProducer, String>>> = const { RefCell::new(None) };
    static BUILDING: Cell<bool> = const { Cell::new(false) };
    /// A producer handed over for a call, with its cache identity: how one emitter emits another.
    static HANDED: RefCell<Option<(PlyProducer, String)>> = const { RefCell::new(None) };
}

/// Installs the recipe every thread's producer is built from. The first installation wins.
pub fn install(recipe: Recipe, identity: String) {
    let _ = IDENTITY.set(identity);
    let _ = RECIPE.set(recipe);
}

/// Where the emitter's own source comes from: the embedded `ply-compiler`, or a working copy.
#[derive(Clone, Debug)]
pub enum Sources {
    Embedded,
    Directory(std::path::PathBuf),
}

/// Install the working copy `PLY_C_EMITTER=ply:<dir>` names, or the embedded emitter, if none is.
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

/// The standard library, then the emitter's modules; a directory is sorted like the embedded list.
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

/// Build the emitter from its sources' own bundle, or else have the embedded emitter emit them.
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
        // Sources identical to the bundle's emit that same unit (the fixpoint), so skip the work.
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
            // Say whether the entry was never offered or was refused; `PlyProducer::new` would not.
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

/// The emitter's own program through the front end, modules as `SourceId(0..n)` in `modules_of`'s
/// order; the answer comes from the emitter handed over by [`build`].
fn front_end(src: &Sources) -> Result<&'static Source, String> {
    let modules = modules_of(src);
    let ids: Vec<SourceId> = (0..modules.len()).map(|i| SourceId(i as u32)).collect();
    let front = front(&modules, &ids).map_err(|e| format!("{e:#}"))?;
    if let Some(error) = front
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
    {
        return Err(error.message.clone());
    }
    let front: &'static Front = Box::leak(Box::new(front));
    let keys = crate::source::emit_keys(front);
    let texts: HashMap<String, String> = modules.iter().cloned().collect();
    Ok(Box::leak(Box::new(
        Source::from_front(front, keys).with_texts(texts),
    )))
}

pub fn reset_thread() {
    MINE.with(|mine| *mine.borrow_mut() = None);
}

/// The digest this thread's emitter keys its bodies under; installs the default first.
pub fn identity() -> String {
    if let Some(id) = HANDED.with(|h| h.borrow().as_ref().map(|(_, id)| id.clone())) {
        return id;
    }
    ensure_default();
    IDENTITY.get().cloned().unwrap_or_default()
}

/// This thread's emitter, as a cache of its answers keys on it.
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

/// Runs `f` with `p` as this thread's producer, its emissions keyed under `identity`.
pub fn with_producer<R>(p: PlyProducer, identity: String, f: impl FnOnce() -> R) -> R {
    let _held = hand_over(p, identity);
    f()
}

/// [`with_producer`] as a guard; handovers nest, restoring the previous one on drop.
pub fn hand_over(p: PlyProducer, identity: String) -> Handed {
    Handed(HANDED.with(|h| h.borrow_mut().replace((p, identity))))
}

pub struct Handed(Option<(PlyProducer, String)>);

impl Drop for Handed {
    fn drop(&mut self) {
        HANDED.with(|h| *h.borrow_mut() = self.0.take());
    }
}

/// Runs `f` with this thread's producer: the handed-over one, else one built from the recipe.
/// `None` while it is being built or if building failed (reported once).
pub fn with_current<T>(f: impl FnOnce(&PlyProducer) -> T) -> Option<T> {
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
    /// Every body answered, by name, keyed on the program's address; filled on first ask.
    modules: RefCell<HashMap<usize, Bodies>>,
    asked: Cell<u64>,
    answered: Cell<u64>,
    /// Why the emitter raised over a program, by the program's address.
    failed: RefCell<HashMap<usize, String>>,
}

/// Entered as `(names, srcs, ctors, builtins)` over every module at once, so they resolve together.
const ENTRY: &str = "emit.emit_unit_all";

impl PlyProducer {
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

    /// The emitter's C for `name`; `None` if unreached or failed (see [`PlyProducer::failure`]).
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

    /// Enters any function of the unit (`module.name`) with `args`, in a fresh context.
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

/// What this thread's entries into the compiled compiler have cost since the last reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub entries: usize,
    /// Modules handed to the front end, summed over its entries.
    pub modules: usize,
    /// Modules handed to [`claims_dump`], summed over its entries.
    pub claimed: usize,
    pub allocated: usize,
    pub recycled: usize,
    /// The most chunk bytes any one entry held at its end.
    pub chunk_bytes: usize,
}

thread_local! {
    static CENSUS: Cell<Census> = const { Cell::new(Census { entries: 0, modules: 0, claimed: 0, allocated: 0, recycled: 0, chunk_bytes: 0 }) };
}

fn tally(f: impl FnOnce(&mut Census)) {
    CENSUS.with(|c| {
        let mut census = c.get();
        f(&mut census);
        c.set(census);
    });
}

fn note_census(ctx: &crate::rt::Ctx) {
    tally(|census| {
        census.entries += 1;
        census.allocated += ctx.heap.allocated();
        census.recycled += ctx.heap.recycled();
        census.chunk_bytes = census.chunk_bytes.max(ctx.heap.chunk_bytes());
    });
}

/// Starts this thread's census afresh, so a test reads only what it entered.
pub fn reset_census() {
    CENSUS.with(|c| c.set(Census::default()));
}

pub fn census() -> Census {
    CENSUS.with(|c| c.get())
}

/// The front end's whole answer, as `ply_ty::front` reads it.
const FRONT: &str = "front.front_dump";

/// The front end over a program; `ids[i]` is module `i`'s source. Program errors are in
/// `diagnostics`, not the `Err`; use [`checked_front`] to raise them.
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

/// The front end's raw answer, before [`read_front`].
pub fn front_dump(sources: &[(String, String)]) -> Result<String> {
    tally(|census| census.modules += sources.len());
    dump_over(FRONT, sources)
}

const CLAIMS: &str = "front.claims_dump";

/// Every body, clause and law of a program [`front`] already checked, lowered.
pub fn claims_dump(sources: &[(String, String)]) -> Result<String> {
    tally(|census| census.claimed += sources.len());
    dump_over(CLAIMS, sources)
}

fn dump_over(entry: &str, sources: &[(String, String)]) -> Result<String> {
    let answer = call(entry, &[source_list(sources)])?;
    let Value::Str(dump) = &answer else {
        bail!(
            "`{entry}` answered a {} rather than a string",
            answer.type_name()
        );
    };
    Ok(dump.to_string())
}

fn source_list(sources: &[(String, String)]) -> Value {
    Value::list(
        sources
            .iter()
            .map(|(name, src)| {
                Value::Record(Arc::new(Fields::from_unsorted(vec![
                    (Symbol::new("name"), Value::bytes(name.as_bytes())),
                    (Symbol::new("src"), Value::bytes(src.as_bytes())),
                ])))
            })
            .collect(),
    )
}

/// [`FRONT`], pulling in the shipped modules the program imports itself.
const FRONT_PULLING: &str = "front.front_pulling_std";

/// What [`front_pulling_std`] answered.
pub struct Pulled {
    /// The shipped modules pulled in, in the positions they took after the user's.
    pub modules: Vec<String>,
    /// [`front_dump`]'s answer over the user's modules followed by [`Pulled::modules`].
    pub dump: String,
}

/// [`front_dump`] over `user` plus each module of `shipped` it imports, transitively, placed
/// as the CLI driver places them: a round of newly imported modules at a time, each in byte order.
pub fn front_pulling_std(
    user: &[(String, String)],
    shipped: &[(String, String)],
) -> Result<Pulled> {
    let answer = call(FRONT_PULLING, &[source_list(user), source_list(shipped)])?;
    let Value::Str(answer) = &answer else {
        bail!(
            "`{FRONT_PULLING}` answered a {} rather than a string",
            answer.type_name()
        );
    };
    let answer: &str = answer;
    let mut frames = Cursor::new(answer.as_bytes(), "frame");
    let (words, payload) = frames
        .unit()
        .map_err(|e| anyhow!("`{FRONT_PULLING}`'s answer: {e}"))?;
    if words != ["pulled", "_"] {
        bail!(
            "`{FRONT_PULLING}` led with `{}` rather than the modules it pulled in",
            words.join(" ")
        );
    }
    let mut fields = Cursor::new(payload, "field");
    let mut modules = Vec::new();
    while !fields.done() {
        let (key, name) = fields
            .unit()
            .map_err(|e| anyhow!("the modules `{FRONT_PULLING}` pulled in: {e}"))?;
        if key != ["module"] {
            bail!(
                "the modules `{FRONT_PULLING}` pulled in hold a `{}` field",
                key.join(" ")
            );
        }
        modules.push(
            std::str::from_utf8(name)
                .context("a pulled module's name")?
                .to_string(),
        );
    }
    tally(|census| census.modules += user.len() + modules.len());
    Ok(Pulled {
        modules,
        dump: answer[frames.at()..].to_string(),
    })
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
