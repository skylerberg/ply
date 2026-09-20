//! The emitter written in Ply, as the C tier's producer. A recipe is installed per process and
//! each thread builds its own, since the loaded unit holds `Rc`s.

use super::build::Native;
use super::tables::Tables;
use crate::source::Source;
use anyhow::{Context, Result, anyhow, bail};
use ply_eval::{Fields, Value};
use ply_span::frames::Cursor;
use ply_span::{Diagnostic, Severity, SourceId, Symbol, codes};
use ply_ty::{DefHash, Front, Scheme, parse_scheme, read_front};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
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
    let _ = EMITTER.set(emitter_of(&identity));
    install(Arc::new(move || build(&src)), identity);
}

/// The digest the emitter `src` holds keys its bodies under.
pub fn identity_of(src: &Sources) -> String {
    digest_of(&modules_of(src))
}

/// What the emitter's answers are a function of: the runtime's helper table and the sources it
/// was built from; a stage emitted for those sources answers as the fixpoint of them would.
fn emitter_of(identity: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(super::exports::helpers_digest().as_bytes());
    h.update(identity.as_bytes());
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

/// The emitter for `src`: the committed bundle when it was emitted from these very sources, else
/// the stage kept for them by an earlier process, else the committed emitter emitting them now.
/// A binary whose bundle is behind its sources therefore runs the sources, never the bundle.
pub fn build(src: &Sources) -> Result<PlyProducer, String> {
    let identity = identity_of(src);
    let carried = super::bundle::embedded();
    let (native, _refused) = if carried.sources_digest() == Some(identity.as_str()) {
        super::bundle::build(&carried).map_err(|e| format!("{e:#}"))?
    } else {
        let staged = super::bundle::from_dir(&super::bundle::stage_dir(&identity))
            .and_then(|stage| super::bundle::build(&stage).ok());
        match staged {
            Some(built) => built,
            None => emit_stage(src, &identity)?,
        }
    };
    PlyProducer::new(native).map_err(|e| format!("{e:#}"))
}

/// The committed emitter emitting `src`, written as the stage for `identity` so no later process
/// repeats the work.
fn emit_stage(
    src: &Sources,
    identity: &str,
) -> Result<(super::Native, Vec<super::Refused>), String> {
    let (native, _) = super::bundle::build(&super::bundle::embedded()).map_err(|e| {
        format!(
            "the committed bundle does not serve this runtime: {e:#}. Check out an older bundle \
             this runtime serves from git history, then refresh it with \
             `PLY_C_BOOTSTRAP_REFRESH=1 cargo nextest run -p ply-codegen-tests --test bootstrap`"
        )
    })?;
    let first = PlyProducer::new(native).map_err(|e| format!("{e:#}"))?;
    with_producer(first, identity_of(&Sources::Embedded), || {
        let source = front_end(src)?;
        let names: Vec<String> = source.functions();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let produced = super::produce(source, &refs).map_err(|e| format!("{e:#}"))?;
        // Say whether the entry was never offered or was refused; `PlyProducer::new` would not.
        if !produced.exports.names().iter().any(|n| n == ENTRY) {
            let head: Vec<String> = produced
                .refused
                .iter()
                .take(5)
                .map(|r| r.to_string())
                .collect();
            return Err(format!(
                "the committed emitter emitted no `{ENTRY}`: {} roots offered, `{ENTRY}` {} among \
                 them, {} refused in all{}",
                names.len(),
                if names.iter().any(|n| n == ENTRY) {
                    "was"
                } else {
                    "was NOT"
                },
                produced.refused.len(),
                if head.is_empty() {
                    String::new()
                } else {
                    format!("; first refusals: {}", head.join(" | "))
                },
            ));
        }
        let dir = super::bundle::stage_dir(identity);
        super::bundle::write(&dir, &produced.text, identity).map_err(|e| format!("{e:#}"))?;
        let stage =
            super::bundle::from_dir(&dir).ok_or_else(|| "the stage was not written".to_string())?;
        super::bundle::build(&stage).map_err(|e| format!("{e:#}"))
    })
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
        return Err(placed(error, &modules));
    }
    let front: &'static Front = Box::leak(Box::new(front));
    let keys = crate::source::emit_keys(front);
    let texts: HashMap<String, String> = modules.iter().cloned().collect();
    Ok(Box::leak(Box::new(
        Source::from_front(front, keys).with_texts(texts),
    )))
}

/// The error with its place: the module, the line and column of its primary label, and what the
/// label says, since nothing else about the emitter's own sources reaches a reader.
fn placed(error: &ply_span::Diagnostic, modules: &[(String, String)]) -> String {
    let mut out = error.message.clone();
    if let Some(label) = error
        .labels
        .iter()
        .find(|l| l.primary)
        .or_else(|| error.labels.first())
        && let Some((name, text)) = modules.get(label.span.source.0 as usize)
    {
        let start = label.span.start as usize;
        let line = text[..start.min(text.len())].matches('\n').count() + 1;
        let column = start
            - text[..start.min(text.len())]
                .rfind('\n')
                .map_or(0, |i| i + 1)
            + 1;
        out.push_str(&format!(
            " at {name}.ply:{line}:{column}: {}",
            label.message
        ));
    }
    for note in &error.notes {
        out.push_str(&format!("; {note}"));
    }
    out
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

/// The emitter's answers over one program.
#[derive(Default)]
struct Memo {
    answers: Bodies,
    /// Every root the emitter was entered for: one it did not answer is not asked for again.
    asked: HashSet<String>,
}

impl Memo {
    fn knows(&self, name: &str) -> bool {
        self.asked.contains(name)
    }
}

/// The compiled Ply emitter, entered over the whole program for the roots asked of it.
pub struct PlyProducer {
    native: Native,
    /// By the program's address.
    memos: RefCell<HashMap<usize, Memo>>,
    asked: Cell<u64>,
    answered: Cell<u64>,
    /// Why the emitter raised over a program, by the program's address.
    failed: RefCell<HashMap<usize, String>>,
}

/// Entered as `(names, srcs, ctors, builtins, wanted)`: every module at once, so they resolve
/// together, emitting the roots `wanted` names.
const ENTRY: &str = "emit.emit_roots";

impl PlyProducer {
    pub fn new(native: Native) -> Result<PlyProducer> {
        if native.entry(ENTRY).is_none() {
            bail!("the unit has no `{ENTRY}`, so it is not the Ply emitter");
        }
        Ok(PlyProducer {
            native,
            memos: RefCell::new(HashMap::new()),
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

    /// Roots handed to the emitter and bodies it answered, over this thread's life.
    pub fn counts(&self) -> (u64, u64) {
        (self.asked.get(), self.answered.get())
    }

    /// Enters the emitter once for the roots of `wanted` it has not been asked for over `loaded`.
    pub fn ask(&self, loaded: &Source, wanted: &[String]) {
        let program = std::ptr::from_ref(loaded) as usize;
        let missing: Vec<String> = {
            let memos = self.memos.borrow();
            let known = |name: &str| memos.get(&program).is_some_and(|m| m.knows(name));
            wanted
                .iter()
                .filter(|name| !known(name.as_str()))
                .cloned()
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        self.asked.set(self.asked.get() + missing.len() as u64);
        let entered = self.enter(loaded, &missing);
        let mut memos = self.memos.borrow_mut();
        let memo = memos.entry(program).or_default();
        match entered {
            Ok(answers) => {
                memo.answers.extend(answers);
                let answered = missing
                    .iter()
                    .filter(|name| {
                        matches!(memo.answers.get(name.as_str()), Some(Answer::Body(..)))
                    })
                    .count();
                self.answered.set(self.answered.get() + answered as u64);
            }
            Err(e) => {
                self.failed.borrow_mut().insert(program, format!("{e:#}"));
            }
        }
        memo.asked.extend(missing);
    }

    /// The emitter's C for `name`, asked for on its own unless [`PlyProducer::ask`] already did;
    /// `None` if unanswered or failed (see [`PlyProducer::failure`]).
    pub fn body(&self, loaded: &Source, name: &str) -> Option<Answer> {
        self.ask(loaded, &[name.to_string()]);
        let program = std::ptr::from_ref(loaded) as usize;
        self.memos
            .borrow()
            .get(&program)?
            .answers
            .get(name)
            .cloned()
    }

    /// One entry over the whole program, emitting `wanted`'s roots.
    fn enter(&self, loaded: &Source, wanted: &[String]) -> Result<Bodies> {
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
        let roots = wanted.iter().map(|n| Value::bytes(n.as_bytes())).collect();
        let args = vec![
            Value::list(names),
            Value::list(srcs),
            ctors,
            builtins,
            Value::list(roots),
        ];
        tally(|census| census.wanted.push(wanted.to_vec()));
        let value = self.call(ENTRY, &args)?;
        let Value::Str(dump) = &value else {
            bail!("the emitter answered something that is not a string");
        };
        parse(dump).context("reading the emitter's answer")
    }

    /// Enters any function of the unit (`module.name`) with `args`, in a fresh context and with
    /// no time budget: the compiler's own work is never bounded by the program's.
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value> {
        crate::rt::with_time_budget(0, || self.enter_own(name, args))
    }

    fn enter_own(&self, name: &str, args: &[Value]) -> Result<Value> {
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// The roots each entry into the emitter was asked for, in entry order.
    pub wanted: Vec<Vec<String>>,
}

thread_local! {
    static CENSUS: RefCell<Census> = const {
        RefCell::new(Census {
            entries: 0,
            modules: 0,
            claimed: 0,
            allocated: 0,
            recycled: 0,
            chunk_bytes: 0,
            wanted: Vec::new(),
        })
    };
}

fn tally(f: impl FnOnce(&mut Census)) {
    CENSUS.with(|c| f(&mut c.borrow_mut()));
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
    CENSUS.with(|c| *c.borrow_mut() = Census::default());
}

pub fn census() -> Census {
    CENSUS.with(|c| c.borrow().clone())
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

const BUILTINS: &str = "front.builtins_dump";

/// A builtin as the port's checker binds it: its scheme, the names of its parameters and a note.
#[derive(Clone, Debug)]
pub struct BuiltinInfo {
    pub name: Symbol,
    pub scheme: Scheme,
    pub params: Vec<String>,
    pub note: String,
}

/// Every builtin, in the prelude's order.
pub fn builtins() -> Result<Vec<BuiltinInfo>> {
    let dump = string_answer(BUILTINS, call(BUILTINS, &[])?)?;
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    let mut out = Vec::new();
    while !frames.done() {
        let (words, payload) = frames
            .unit()
            .map_err(|e| anyhow!("`{BUILTINS}`'s answer: {e}"))?;
        let ["builtin", name] = words[..] else {
            bail!("`{BUILTINS}` framed a `{}`", words.join(" "));
        };
        let mut fields = Cursor::new(payload, "field");
        let mut scheme = None;
        let mut params = Vec::new();
        let mut note = String::new();
        while !fields.done() {
            let (key, text) = fields
                .unit()
                .map_err(|e| anyhow!("`{name}`'s frame: {e}"))?;
            let text = std::str::from_utf8(text).context("a builtin's field")?;
            match key[..] {
                ["scheme"] => {
                    scheme = Some(
                        parse_scheme(text)
                            .map_err(|e| anyhow!("`{name}`'s scheme `{text}`: {e}"))?,
                    );
                }
                ["param"] => params.push(text.to_string()),
                ["note"] => note = text.to_string(),
                _ => bail!("`{name}`'s frame has a `{}` field", key.join(" ")),
            }
        }
        let Some(scheme) = scheme else {
            bail!("`{name}`'s frame has no `scheme` field");
        };
        out.push(BuiltinInfo {
            name: Symbol::new(name),
            scheme,
            params,
            note,
        });
    }
    Ok(out)
}

fn dump_over(entry: &str, sources: &[(String, String)]) -> Result<String> {
    string_answer(entry, call(entry, &[source_list(sources)])?)
}

fn source_list(sources: &[(String, String)]) -> Value {
    Value::list(
        sources
            .iter()
            .map(|(name, src)| {
                record(vec![
                    ("name", Value::bytes(name.as_bytes())),
                    ("src", Value::bytes(src.as_bytes())),
                ])
            })
            .collect(),
    )
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    )))
}

fn string_answer(entry: &str, answer: Value) -> Result<String> {
    let Value::Str(text) = &answer else {
        bail!(
            "`{entry}` answered a {} rather than a string",
            answer.type_name()
        );
    };
    Ok(text.to_string())
}

const REHASH: &str = "front.rehash_dump";

/// Every definition and test re-hashed with each reference to a `pins` name written as its pin.
pub fn rehash_dump(
    sources: &[(String, String)],
    pins: &[(String, bool, DefHash)],
) -> Result<String> {
    let pins = Value::list(
        pins.iter()
            .map(|(name, decl, hash)| {
                record(vec![
                    ("name", Value::bytes(name.as_bytes())),
                    ("decl", Value::Bool(*decl)),
                    ("hash", Value::bytes(hash.0)),
                ])
            })
            .collect(),
    );
    string_answer(REHASH, call(REHASH, &[source_list(sources), pins])?)
}

const PRINT: &str = "front.print_dump";

/// `(module, text)` in byte order, then `tests` as `ply_tests.t<i>`; `shipped` is only imported.
pub fn print_bodies(
    bodies: &[&[u8]],
    names: &[(&str, DefHash)],
    tests: &[&[u8]],
    relink: &[(DefHash, DefHash)],
    shipped: &[&str],
) -> std::result::Result<Vec<(String, String)>, Diagnostic> {
    let refused = |message: String| Diagnostic::error(codes::ARTIFACT_INVALID, message);
    let args = [
        Value::list(bodies.iter().map(Value::bytes).collect()),
        Value::list(
            names
                .iter()
                .map(|(name, hash)| {
                    record(vec![
                        ("name", Value::bytes(name.as_bytes())),
                        ("hash", Value::bytes(hash.0)),
                    ])
                })
                .collect(),
        ),
        Value::list(tests.iter().map(Value::bytes).collect()),
        Value::list(
            relink
                .iter()
                .map(|(from, to)| {
                    record(vec![
                        ("from", Value::bytes(from.0)),
                        ("to", Value::bytes(to.0)),
                    ])
                })
                .collect(),
        ),
        Value::list(shipped.iter().map(|m| Value::bytes(m.as_bytes())).collect()),
    ];
    let dump = call(PRINT, &args)
        .and_then(|answer| string_answer(PRINT, answer))
        .map_err(|e| refused(format!("the stored bodies do not decode: {e:#}")))?;
    let unreadable = |e: String| refused(format!("`{PRINT}`'s answer does not read: {e}"));
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    let mut modules = Vec::new();
    while !frames.done() {
        let (words, payload) = frames.unit().map_err(unreadable)?;
        let text = std::str::from_utf8(payload)
            .map_err(|e| unreadable(e.to_string()))?
            .to_string();
        match words[..] {
            ["module", name] => modules.push((name.to_string(), text)),
            ["refused", _] => {
                let mut fields = Cursor::new(payload, "field");
                let mut diagnostic = refused(String::new());
                while !fields.done() {
                    let (key, body) = fields.unit().map_err(unreadable)?;
                    let body = String::from_utf8_lossy(body).into_owned();
                    match key[..] {
                        ["message"] => diagnostic.message = body,
                        _ => diagnostic = diagnostic.note(body),
                    }
                }
                return Err(diagnostic);
            }
            _ => return Err(unreadable(format!("a `{}` frame", words.join(" ")))),
        }
    }
    Ok(modules)
}

/// `entry`'s answer, one frame, handed to `read` as its header words and payload.
fn framed<T>(
    entry: &str,
    args: &[Value],
    read: impl FnOnce(&[&str], &[u8]) -> Result<T>,
) -> Result<T> {
    ensure_default();
    let dump = string_answer(entry, call(entry, args)?)?;
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    let (words, payload) = frames
        .unit()
        .map_err(|e| anyhow!("`{entry}`'s answer: {e}"))?;
    read(&words, payload)
}

/// The `message` a `refused` frame's payload carries.
fn refusal(entry: &str, payload: &[u8]) -> Result<String> {
    let mut fields = Cursor::new(payload, "field");
    let (key, body) = fields
        .unit()
        .map_err(|e| anyhow!("`{entry}`'s refusal: {e}"))?;
    if key != ["message"] {
        bail!("`{entry}`'s refusal holds a `{}` field", key.join(" "));
    }
    Ok(String::from_utf8_lossy(body).into_owned())
}

const FMT: &str = "front.fmt_dump";

/// `src` formatted. The outer error is the emitter failing; the inner one is the text the
/// formatter refused, with the diagnostic that stopped it.
pub fn fmt_source(src: &str) -> Result<Result<String, String>> {
    framed(
        FMT,
        &[Value::bytes(src.as_bytes())],
        |words, payload| match words {
            ["formatted", _] => Ok(Ok(std::str::from_utf8(payload)
                .context("the formatted text")?
                .to_string())),
            ["refused", _] => Ok(Err(refusal(FMT, payload)?)),
            _ => bail!("`{FMT}` framed a `{}`", words.join(" ")),
        },
    )
}

const ITEM_RANGE: &str = "front.item_range_dump";

/// The byte range of the `fn`, `type` or `effect` named `name` in `src`: its comment lines,
/// `pub` and `reuse` through the end of its last line. The inner error is the front end's refusal.
pub fn item_range(src: &str, name: &str) -> Result<Result<(usize, usize), String>> {
    let args = [Value::bytes(src.as_bytes()), Value::bytes(name.as_bytes())];
    framed(ITEM_RANGE, &args, |words, payload| match words {
        ["range", _] => {
            let mut fields = Cursor::new(payload, "field");
            let (mut start, mut end) = (None, None);
            while !fields.done() {
                let (key, body) = fields
                    .unit()
                    .map_err(|e| anyhow!("`{ITEM_RANGE}`'s range: {e}"))?;
                let n: usize = std::str::from_utf8(body)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| {
                        anyhow!("`{ITEM_RANGE}`'s `{}` is not a number", key.join(" "))
                    })?;
                match key[..] {
                    ["start"] => start = Some(n),
                    ["end"] => end = Some(n),
                    _ => bail!("`{ITEM_RANGE}`'s range holds a `{}` field", key.join(" ")),
                }
            }
            match (start, end) {
                (Some(start), Some(end)) => Ok(Ok((start, end))),
                _ => bail!("`{ITEM_RANGE}`'s range lacks a bound"),
            }
        }
        ["refused", _] => Ok(Err(refusal(ITEM_RANGE, payload)?)),
        _ => bail!("`{ITEM_RANGE}` framed a `{}`", words.join(" ")),
    })
}

const REPLACE: &str = "front.replace_dump";

/// `src` with the item named `name` replaced by `item` formatted, every other byte kept. The
/// inner error is the front end's refusal: an absent name, a replacement of another kind or
/// name, or text that does not parse.
pub fn replace_item(src: &str, name: &str, item: &str) -> Result<Result<String, String>> {
    let args = [
        Value::bytes(src.as_bytes()),
        Value::bytes(name.as_bytes()),
        Value::bytes(item.as_bytes()),
    ];
    framed(REPLACE, &args, |words, payload| match words {
        ["replaced", _] => Ok(Ok(std::str::from_utf8(payload)
            .context("the replaced text")?
            .to_string())),
        ["refused", _] => Ok(Err(refusal(REPLACE, payload)?)),
        _ => bail!("`{REPLACE}` framed a `{}`", words.join(" ")),
    })
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
