//! The emitter written in Ply, as the C tier's producer. A recipe is installed per process and
//! each thread builds its own, since the loaded unit holds `Rc`s.

use super::build::Native;
use super::tables::{Defined, Tables};
use crate::source::Source;
use anyhow::{Context, Result, anyhow, bail};
use ply_eval::decode::{self, At};
use ply_eval::{DefHash, Diagnostic, Fields, Front, Severity, SourceId, Symbol, Value, codes};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

/// How a thread builds its producer.
pub type Recipe = Arc<dyn Fn() -> Result<PlyProducer, String> + Send + Sync>;

static RECIPE: OnceLock<Recipe> = OnceLock::new();
/// A digest of the emitter's program, folded into every cache key.
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

/// The module path each `import` line of `text` opens with. Read here rather than asked of the
/// front end: the identity has to exist before there is a compiler to ask for it.
fn imports_of(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| line.strip_prefix("import "))
        .map(|rest| {
            rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .next()
                .unwrap_or("")
        })
        .collect()
}

/// The shipped modules `own` imports, transitively, in [`ply_std::sources`]' order. A reserved
/// name nothing ships is left out for the front end to report where it is written.
fn shipped_closure(own: &[(String, String)]) -> Vec<(String, String)> {
    let mut wanted: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = own
        .iter()
        .flat_map(|(_, text)| imports_of(text))
        .map(str::to_string)
        .collect();
    while let Some(name) = frontier.pop() {
        if !ply_std::is_std(&name) {
            continue;
        }
        let Some(text) = ply_std::source(&name) else {
            continue;
        };
        if wanted.insert(name) {
            frontier.extend(imports_of(text).into_iter().map(str::to_string));
        }
    }
    ply_std::sources()
        .filter(|(name, _)| wanted.contains(*name))
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect()
}

/// The emitter's program: its own modules, then the shipped modules they import, transitively and
/// placed after them, as the front end places the ones it pulls for a user program. A shipped
/// module nothing here imports is in neither the identity, the bundle nor a cache key. A directory
/// is sorted like the embedded list.
pub fn modules_of(src: &Sources) -> Vec<(String, String)> {
    let mut program = own_sources(src);
    program.extend(shipped_closure(&program));
    program
}

/// The program's own modules, before the shipped closure is pulled in.
fn own_sources(src: &Sources) -> Vec<(String, String)> {
    match src {
        Sources::Embedded => ply_compiler::sources()
            .map(|(m, t)| (m.to_string(), t.to_string()))
            .collect(),
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
            found
        }
    }
}

/// The emitter for `src`: the committed bundle when it was emitted from these very sources, else
/// the stage kept for them by an earlier process, else the committed emitter emitting them now.
/// A binary whose bundle is behind its sources therefore runs the sources, never the bundle.
pub fn build(src: &Sources) -> Result<PlyProducer, String> {
    let identity = identity_of(src);
    let carried = super::bundle::embedded();
    let started = std::time::Instant::now();
    let phases = std::env::var_os("PLY_C_PHASES").is_some();
    let (native, _refused) = if carried.sources_digest() == Some(identity.as_str()) {
        let built = super::bundle::build(&carried).map_err(|e| format!("{e:#}"))?;
        if phases {
            eprintln!(
                "phases: emitter {identity} from the committed bundle, {}ms",
                started.elapsed().as_millis()
            );
        }
        built
    } else {
        let dir = super::bundle::stage_dir(&identity);
        let staged = match super::bundle::from_dir(&dir) {
            None => Err("there is none".to_string()),
            Some(stage) => super::bundle::build(&stage).map_err(|e| format!("{e:#}")),
        };
        match staged {
            Ok(built) => {
                super::sweep::used(&dir);
                if phases {
                    eprintln!(
                        "phases: emitter {identity} from its stage, {}ms",
                        started.elapsed().as_millis()
                    );
                }
                built
            }
            Err(why) => {
                let built = emit_stage(src, &identity)?;
                if phases {
                    eprintln!(
                        "phases: emitter {identity} emitted, since its stage at {} would not \
                         build ({why}), {}ms",
                        dir.display(),
                        started.elapsed().as_millis()
                    );
                }
                built
            }
        }
    };
    PlyProducer::new(native)
        .map(|p| p.keeping(emitter_of(&identity)))
        .map_err(|e| format!("{e:#}"))
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
             `ply bootstrap crates/ply-compiler/ply --out crates/ply-compiler/bootstrap`"
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
    let own = own_sources(src);
    let shipped = shipped_closure(&own);
    // The shipped modules are pulled, not inlined: inlined they would be root modules named
    // `std.*`, which the front end's built-in-package check refuses.
    let pulled = front_pulling_std(&own, &shipped).map_err(|e| format!("{e:#}"))?;
    let ids: Vec<SourceId> = (0..own.len() + pulled.modules.len())
        .map(|i| SourceId(i as u32))
        .collect();
    let front = super::dump::read(&pulled.dump, &ids)
        .map_err(|e| format!("the front end's answer does not read: {e}"))?;
    let mut texts: HashMap<String, String> = own.iter().cloned().collect();
    texts.extend(shipped.iter().cloned());
    // Diagnostics index modules in the dump's order: the program's own, then the pulled ones.
    let modules: Vec<(String, String)> = own
        .iter()
        .map(|(name, _)| name.clone())
        .chain(pulled.modules.iter().cloned())
        .map(|name| {
            let text = texts.get(&name).cloned().unwrap_or_default();
            (name, text)
        })
        .collect();
    if let Some(error) = front
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
    {
        return Err(placed(error, &modules));
    }
    let front: &'static Front = Box::leak(Box::new(front));
    Ok(Box::leak(Box::new(
        Source::from_front(front).with_texts(texts),
    )))
}

/// The error with its place: the module, the line and column of its primary label, and what the
/// label says, since nothing else about the emitter's own sources reaches a reader.
fn placed(error: &ply_eval::Diagnostic, modules: &[(String, String)]) -> String {
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

/// What the emitter said about one definition. A body's tables are nine tables wide, so they are
/// held behind a pointer rather than in every `Answer` a refusal fills.
#[derive(Clone)]
pub enum Answer {
    Body(String, Box<Tables>),
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
    /// The emitter its answers are kept under: set only where the unit was emitted from the sources
    /// it names, which a committed emitter emitting a stage of other sources is not.
    kept: Option<String>,
}

/// Entered as `(names, srcs, ctors, builtins, wanted, pkgs, mod_pkg, embeds, rows, walked)`: the
/// modules the roots `wanted` names are in and what they import, so they resolve together.
const ENTRY: &str = "emit.emit_roots_answer";

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
            kept: None,
        })
    }

    /// Its answers kept under `emitter`, which names the sources its unit was emitted from.
    pub fn keeping(mut self, emitter: String) -> PlyProducer {
        self.kept = Some(emitter);
        self
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
                if answers.is_empty() {
                    // The emitter ran and answered no body at all. That is not "these bodies were
                    // refused": it is that the emitter could not *answer* for the program -- a
                    // module it cannot parse, a package it cannot resolve, or a check of its own it
                    // does not pass -- and every root would otherwise report "did not answer",
                    // which says nothing about why.
                    self.failed.borrow_mut().insert(
                        program,
                        format!(
                            "it answered nothing for the {} root(s) it was asked for, so no body \
                             was emitted: a module it cannot parse, a package it cannot resolve, or \
                             a program its own check refuses",
                            missing.len()
                        ),
                    );
                }
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
        let front = loaded.front;
        let lowered: HashSet<&str> = wanted.iter().map(|r| module_of_root(r)).collect();
        let needed = imported_closure(front, lowered.iter().copied());
        // A module no wanted root is in is read for its declarations, its rows standing for its
        // bodies; an answer that published no rows leaves every body to be walked.
        let stubbing = !front.rows.is_empty();
        let mut names = Vec::new();
        let mut srcs = Vec::new();
        let mut placed = Vec::new();
        for (at, module) in loaded.module_names().enumerate() {
            let name = module.to_string();
            if !needed.contains(name.as_str()) {
                continue;
            }
            let Some(text) = loaded.texts.get(&name) else {
                bail!("no source text for module `{name}`, and the emitter reads a program's text");
            };
            let cuts = front.check.modules.get(module).map(|m| m.cuts.as_slice());
            let text = match cuts {
                Some(cuts) if stubbing && !lowered.contains(name.as_str()) => stubbed(text, cuts),
                _ => text.clone(),
            };
            names.push(Value::bytes(name.as_bytes()));
            srcs.push(Value::bytes(text.as_bytes()));
            placed.push(at);
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
        let (pkgs, mod_pkg) = if front.packages.is_empty() {
            (
                vec![record(vec![
                    ("prefix", Value::bytes(b"")),
                    ("deps", Value::list(Vec::new())),
                ])],
                Value::list(names.iter().map(|_| Value::Int(0)).collect()),
            )
        } else {
            (
                front
                    .packages
                    .iter()
                    .map(|(prefix, deps)| {
                        record(vec![
                            ("prefix", Value::bytes(prefix.as_bytes())),
                            (
                                "deps",
                                Value::list(
                                    deps.iter().map(|d| Value::bytes(d.as_bytes())).collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
                Value::list(
                    placed
                        .iter()
                        .map(|&at| {
                            Value::Int(front.mod_pkg.get(at).copied().unwrap_or_default() as i64)
                        })
                        .collect(),
                ),
            )
        };
        let args = vec![
            Value::list(names),
            Value::list(srcs),
            ctors,
            builtins,
            Value::list(roots),
            Value::list(pkgs),
            mod_pkg,
            embeds_of(front)?,
            rows_of(front)?,
            Value::bytes(&front.walked),
        ];
        tally(|census| census.wanted.push(wanted.to_vec()));
        let value = self.call(ENTRY, &args)?;
        read_answers(&value).context("reading the emitter's answer")
    }

    /// Enters any function of the unit (`module.name`) with `args`, in a fresh context and under
    /// no budget: the compiler's own work is never bounded by the program's.
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value> {
        crate::rt::unbounded(|| self.enter_own(name, args))
    }

    fn enter_own(&self, name: &str, args: &[Value]) -> Result<Value> {
        let entry = self
            .native
            .entry(name)
            .ok_or_else(|| anyhow!("the unit has no `{name}`"))?;
        // A foreign caller builds the argument array, so the entry's arity is checked before the
        // entry is entered: reading one argument too few reads past the array the caller wrote.
        let arity = self
            .native
            .arity(name)
            .ok_or_else(|| anyhow!("`{name}` was compiled without an arity"))?;
        if arity > args.len() {
            bail!(
                "`{name}` takes {arity} argument{} and was entered with {}",
                if arity == 1 { "" } else { "s" },
                args.len()
            );
        }
        // An entry gains arguments only at its end, and the committed emitter emitting a stage may
        // predate the last: the compiler's own sources need none of them.
        let args = &args[..arity];
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
    pub allocated: usize,
    pub recycled: usize,
    /// The most chunk bytes any one entry held at its end.
    pub chunk_bytes: usize,
    /// The roots each entry into the emitter was asked for, in entry order.
    pub wanted: Vec<Vec<String>>,
}

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static CENSUS: RefCell<Census> = const {
        RefCell::new(Census {
            entries: 0,
            modules: 0,
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

/// Starts this thread's census afresh, so a test reads only what it entered. A thread that counts
/// works out every answer, so it reads none that was kept.
pub fn reset_census() {
    CENSUS.with(|c| *c.borrow_mut() = Census::default());
    COUNTING.with(|c| c.set(true));
}

pub fn census() -> Census {
    CENSUS.with(|c| c.borrow().clone())
}

/// The front end over a program that ships its own modules, pulling none; `ids[i]` is module
/// `i`'s source. Program errors are in `diagnostics`, not the `Err`; use [`checked_front`] to
/// raise them.
pub fn front(sources: &[(String, String)], ids: &[SourceId]) -> Result<Front> {
    if sources.len() != ids.len() {
        bail!(
            "{} module(s) handed over with {} source id(s)",
            sources.len(),
            ids.len()
        );
    }
    let pulled = front_pulling_std(sources, &[])?;
    read_front(&pulled.dump, ids)
}

/// [`super::dump::read`], its failure the front end's.
fn read_front(dump: &Value, ids: &[SourceId]) -> Result<Front> {
    super::dump::read(dump, ids).map_err(|e| anyhow!("the front end's answer does not read: {e}"))
}

/// What the front end embedded, as the compiler's passes take it back.
pub fn embeds_of(front: &Front) -> Result<Value> {
    if front.embeds.is_empty() {
        return Ok(Value::list(Vec::new()));
    }
    ply_eval::codec::decode(&front.embeds).map_err(|e| anyhow!("the answer's embeds: {e}"))
}

/// The module a root is in: a clause's, test's or law's root is its owner's name then `#`, and a
/// module is a name less its last segment. The emitter's `module_of_root` reads roots the same way.
pub fn module_of_root(root: &str) -> &str {
    let owner = root.split_once('#').map_or(root, |(owner, _)| owner);
    owner.rsplit_once('.').map_or("", |(module, _)| module)
}

/// `modules` and every module they import, directly or not, by what the answer says each imports:
/// all the emitter reads to lower a body of one of them.
pub fn imported_closure<'a>(
    front: &Front,
    modules: impl IntoIterator<Item = &'a str>,
) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = modules.into_iter().map(str::to_string).collect();
    while let Some(module) = stack.pop() {
        if !seen.insert(module.clone()) {
            continue;
        }
        if let Some(info) = front.check.modules.get(&Symbol::new(&module)) {
            stack.extend(info.imports.iter().map(|m| m.as_str().to_string()));
        }
    }
    seen
}

/// `text` with each of `cuts` blanked, every byte but a newline a space and a braced cut keeping its
/// braces: the module's declarations at the offsets they have, over empty bodies. The front end's
/// `stub_module` blanks a compiled package's modules the same way.
pub fn stubbed(text: &str, cuts: &[ply_eval::Cut]) -> String {
    let mut out = text.as_bytes().to_vec();
    for cut in cuts {
        let Some(span) = out.get_mut(cut.start..cut.end) else {
            continue;
        };
        for byte in span.iter_mut() {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        if cut.braced && span.len() >= 2 {
            let last = span.len() - 1;
            span[0] = b'{';
            span[last] = b'}';
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// What every definition and test published, as the emitter takes it; none seeds nothing.
pub fn rows_of(front: &Front) -> Result<Value> {
    if front.rows.is_empty() {
        return Ok(record(vec![
            ("defs", Value::list(Vec::new())),
            ("tests", Value::list(Vec::new())),
        ]));
    }
    ply_eval::codec::decode(&front.rows).map_err(|e| anyhow!("the answer's rows: {e}"))
}

/// The package tables a caller passes to a resolving entry, as values: what [`Front`]
/// publishes, or empty lists for a program without packages.
pub fn package_tables(
    packages: &[(String, Vec<String>)],
    mod_pkg: &[usize],
) -> (Value, Value, Value) {
    if packages.is_empty() {
        (
            Value::list(Vec::new()),
            Value::list(Vec::new()),
            Value::Int(0),
        )
    } else {
        (
            Value::list(
                packages
                    .iter()
                    .map(|(prefix, deps)| {
                        record(vec![
                            ("prefix", Value::bytes(prefix.as_bytes())),
                            (
                                "deps",
                                Value::list(
                                    deps.iter().map(|d| Value::bytes(d.as_bytes())).collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
            Value::list(mod_pkg.iter().map(|i| Value::Int(*i as i64)).collect()),
            Value::Int(packages.len() as i64),
        )
    }
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

const REHASH: &str = "front.rehash";

/// Every definition and test re-hashed with each reference to a `pins` name written as its pin,
/// as a `hash.Rehashed`. The packages are the ones the program was analysed with: without them a
/// rehash resolves a dependency's own modules as root ones.
pub fn rehash(
    sources: &[(String, String)],
    pins: &[(String, bool, DefHash)],
    packages: &[(String, Vec<String>)],
    mod_pkg: &[usize],
) -> Result<Value> {
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
    let (pkgs, mods, shelf) = package_tables(packages, mod_pkg);
    call(REHASH, &[source_list(sources), pins, pkgs, mods, shelf])
}

const PRINT: &str = "hash.print_bodies";

/// One name a body may be printed under: the program-wide name, its body's hash, and whether the
/// module it is in exports it. A reference from another module is printed `binder::name`, so a name
/// its own module keeps private is one no other module's body can be printed under.
pub struct PrintedName<'a> {
    pub name: &'a str,
    pub hash: DefHash,
    pub public: bool,
}

/// `(module, text)` in byte order, then `tests` as `ply_tests.t<i>`; `shipped` is only imported.
pub fn print_bodies(
    bodies: &[&[u8]],
    names: &[PrintedName<'_>],
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
                .map(|named| {
                    record(vec![
                        ("name", Value::bytes(named.name.as_bytes())),
                        ("hash", Value::bytes(named.hash.0)),
                        ("public", Value::Bool(named.public)),
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
    let answer = call(PRINT, &args)
        .map_err(|e| refused(format!("the stored bodies do not decode: {e:#}")))?;
    let what = format!("`{PRINT}`'s answer");
    let read =
        || -> Result<std::result::Result<Vec<(String, String)>, Diagnostic>, decode::Error> {
            let printing = At::new(&what, &answer).ctor()?;
            match printing.name() {
                "Printed" => Ok(Ok(printing.arg(0)?.items(|m| {
                    Ok((
                        m.field("module")?.utf8()?.to_string(),
                        m.field("text")?.utf8()?.to_string(),
                    ))
                })?)),
                "Unprintable" => {
                    let why = printing.arg(0)?;
                    let lossy = |text: At<'_>| -> Result<String, decode::Error> {
                        Ok(String::from_utf8_lossy(text.bytes()?).into_owned())
                    };
                    let notes = why.field("notes")?.items(lossy)?;
                    let diagnostic = refused(lossy(why.field("message")?)?);
                    Ok(Err(notes.into_iter().fold(diagnostic, Diagnostic::note)))
                }
                _ => Err(printing.unknown()),
            }
        };
    read().map_err(|e| refused(format!("the answer does not read: {e}")))?
}

/// The front end over a program, pulling in the shipped modules it imports.
const ANSWER: &str = "front.answer_pulling_std_with";

/// What [`front_pulling_std`] answered.
pub struct Pulled {
    /// The shipped modules pulled in, in the positions they took after the user's.
    pub modules: Vec<String>,
    /// The `front.Dump` over the user's modules followed by [`Pulled::modules`], which
    /// [`super::dump::read`] reads.
    pub dump: Value,
}

const WANTS: &str = "pkg.wants";

/// What the manifests on hand ask for beyond `known`: the walk's next reads, as root keys.
pub fn pkg_wants(known: &[String], manifests: &[SuppliedPackage]) -> Result<Vec<String>> {
    let answer = call(
        WANTS,
        &[
            Value::list(known.iter().map(|r| Value::bytes(r.as_bytes())).collect()),
            Value::list(
                manifests
                    .iter()
                    .map(|s| {
                        record(vec![
                            ("root", Value::bytes(s.root.as_bytes())),
                            (
                                "manifest",
                                match &s.manifest {
                                    Some(src) => {
                                        Value::ctor("Some", vec![Value::bytes(src.as_bytes())])
                                    }
                                    None => Value::ctor("None", Vec::new()),
                                },
                            ),
                            ("modules", source_list(&s.modules)),
                        ])
                    })
                    .collect(),
            ),
        ],
    )?;
    let what = format!("`{WANTS}`'s answer");
    Ok(strings(At::new(&what, &answer))?)
}

/// [`front_pulling_std_with`] over a project without packages.
pub fn front_pulling_std(
    user: &[(String, String)],
    shipped: &[(String, String)],
) -> Result<Pulled> {
    front_pulling_std_with(user, shipped, &Packages::anonymous(String::new()), &[])
}

/// A dependency package the walk read: its root, its manifest text when the root holds one,
/// and its modules named relative to the root.
pub struct SuppliedPackage {
    pub root: String,
    pub manifest: Option<String>,
    pub modules: Vec<(String, String)>,
}

/// The packages of one load, as `pkg.Pkgs` takes them.
pub struct Packages {
    pub root: String,
    pub manifest: Option<String>,
    pub supplied: Vec<SuppliedPackage>,
}

impl Packages {
    /// The root package only, with no manifest read: what a project without packages is.
    pub fn anonymous(root: String) -> Packages {
        Packages {
            root,
            manifest: None,
            supplied: Vec::new(),
        }
    }

    fn value(&self) -> Value {
        record(vec![
            ("root", Value::bytes(self.root.as_bytes())),
            (
                "manifest",
                match &self.manifest {
                    Some(src) => Value::ctor("Some", vec![Value::bytes(src.as_bytes())]),
                    None => Value::ctor("None", Vec::new()),
                },
            ),
            (
                "supplied",
                Value::list(
                    self.supplied
                        .iter()
                        .map(|s| {
                            record(vec![
                                ("root", Value::bytes(s.root.as_bytes())),
                                (
                                    "manifest",
                                    match &s.manifest {
                                        Some(src) => {
                                            Value::ctor("Some", vec![Value::bytes(src.as_bytes())])
                                        }
                                        None => Value::ctor("None", Vec::new()),
                                    },
                                ),
                                ("modules", source_list(&s.modules)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

/// One embed a load read, as `front.Embedded` takes it: the module by its package's root and its
/// own name there, what it asked for, and the files read (`embed`'s one file unnamed) or why none
/// were.
pub struct ReadEmbed {
    pub root: String,
    pub module: String,
    pub path: String,
    pub dir: bool,
    pub read: std::result::Result<Vec<(String, Vec<u8>)>, String>,
}

impl ReadEmbed {
    fn value(&self) -> Value {
        record(vec![
            ("root", Value::bytes(self.root.as_bytes())),
            ("module", Value::bytes(self.module.as_bytes())),
            (
                "want",
                record(vec![
                    ("path", Value::bytes(self.path.as_bytes())),
                    ("dir", Value::Bool(self.dir)),
                ]),
            ),
            (
                "read",
                match &self.read {
                    Ok(files) => Value::ctor(
                        "Ok",
                        vec![Value::list(
                            files
                                .iter()
                                .map(|(name, bytes)| {
                                    record(vec![
                                        ("name", Value::bytes(name.as_bytes())),
                                        ("bytes", Value::bytes(bytes)),
                                    ])
                                })
                                .collect(),
                        )],
                    ),
                    Err(why) => Value::ctor("Err", vec![Value::bytes(why.as_bytes())]),
                },
            ),
        ])
    }
}

const WANTED: &str = "front.embeds_wanted";

/// What the modules of the package at `root` ask to embed, as `(module, path, dir)`.
pub fn embeds_wanted(
    root: &str,
    modules: &[(String, String)],
) -> Result<Vec<(String, String, bool)>> {
    let answer = call(
        WANTED,
        &[Value::bytes(root.as_bytes()), source_list(modules)],
    )?;
    let what = format!("`{WANTED}`'s answer");
    Ok(At::new(&what, &answer).items(|e| {
        let want = e.field("want")?;
        Ok((
            e.field("module")?.utf8()?.to_string(),
            want.field("path")?.utf8()?.to_string(),
            want.field("dir")?.bool()?,
        ))
    })?)
}

/// The front end over `user` plus each module of `shipped` it imports, transitively, placed as
/// the CLI driver places them: a round of newly imported modules at a time, each in byte order.
/// Nothing is handed in: the rows a previous answer published are the CLI's to seed it with.
pub fn front_pulling_std_with(
    user: &[(String, String)],
    shipped: &[(String, String)],
    packages: &Packages,
    embeds: &[ReadEmbed],
) -> Result<Pulled> {
    let answer = call(
        ANSWER,
        &[
            source_list(user),
            source_list(shipped),
            Value::list(Vec::new()),
            Value::list(Vec::new()),
            packages.value(),
            Value::list(embeds.iter().map(ReadEmbed::value).collect()),
        ],
    )?;
    let what = format!("`{ANSWER}`'s answer");
    let at = At::new(&what, &answer);
    let modules = strings(at.field("pulled")?)?;
    let dump = at.field("dump")?.value().clone();
    tally(|census| census.modules += user.len() + modules.len());
    Ok(Pulled { modules, dump })
}

/// [`front`] over the default producer, with the program's errors raised rather than answered.
pub fn checked_front(sources: &[(String, String)], ids: &[SourceId]) -> Result<Front> {
    ensure_default();
    checked(front(sources, ids)?)
}

fn checked(front: Front) -> Result<Front> {
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

/// A checked front end, and every module of the program in program order.
pub struct FrontWithStd {
    pub front: Front,
    /// The caller's modules, then the pulled std modules, as `(name, text)` in program order.
    pub modules: Vec<(String, String)>,
}

/// [`checked_front`] over the caller's own sources, with the standard library pulled as the
/// built-in package rather than inlined: the pattern every harness that composes a program
/// out of its own modules and the toolchain's shares.
pub fn checked_front_with_std(user: &[(String, String)]) -> Result<FrontWithStd> {
    let shipped: Vec<(String, String)> = ply_std::sources()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    let pulled = front_pulling_std(user, &shipped)?;
    let mut modules: Vec<(String, String)> = user.to_vec();
    for name in &pulled.modules {
        let Some(text) = ply_std::source(name) else {
            bail!("the front end pulled `{name}`, which does not ship");
        };
        modules.push((name.clone(), text.to_string()));
    }
    let ids: Vec<SourceId> = (0..modules.len()).map(|i| SourceId(i as u32)).collect();
    let front = checked(read_front(&pulled.dump, &ids)?)?;
    Ok(FrontWithStd { front, modules })
}

/// Enters `name` in this thread's compiled emitter, building it first when the thread has none.
/// An answer is kept under the emitter, the entry and the arguments, and read back when asked again.
pub fn call(name: &str, args: &[Value]) -> Result<Value> {
    let key = with_current(|p| p.kept.clone())
        .flatten()
        .filter(|_| !COUNTING.with(Cell::get))
        .and_then(|emitter| super::answers::key(&emitter, name, args));
    if let Some(answer) = key.as_deref().and_then(super::answers::read) {
        return Ok(answer);
    }
    let answer = with_current(|p| p.call(name, args)).unwrap_or_else(|| {
        bail!("no Ply emitter serves on this thread: it is being built, or building it failed")
    })?;
    if let Some(key) = &key {
        super::answers::write(key, &answer);
    }
    Ok(answer)
}

/// Each root's `emit.RootAnswer`. A body whose tables list members is a group's, and answers for
/// every one of them.
fn read_answers(answer: &Value) -> Result<Bodies, decode::Error> {
    let what = format!("`{ENTRY}`'s answer");
    let mut out = HashMap::new();
    for root in At::new(&what, answer).list()? {
        let name = root.field("name")?.utf8()?;
        let answer = root.field("answer")?.ctor()?;
        match answer.name() {
            "Body" => {
                let body = answer.arg(0)?;
                let text = body.field("text")?.utf8()?.to_string();
                let tables = Box::new(read_tables(body.field("tables")?)?);
                if tables.members.is_empty() {
                    out.insert(name.to_string(), Answer::Body(text, tables));
                } else {
                    for member in &tables.members {
                        out.insert(member.clone(), Answer::Body(text.clone(), tables.clone()));
                    }
                }
            }
            "Refused" => {
                let refused = answer.arg(0)?;
                let why = refused.field("why")?.utf8()?.to_string();
                out.insert(
                    name.to_string(),
                    Answer::Refused(why, strings(refused.field("handles")?)?),
                );
            }
            _ => return Err(answer.unknown()),
        }
    }
    Ok(out)
}

fn read_tables(t: At<'_>) -> Result<Tables, decode::Error> {
    let symbols = |list: At<'_>| list.items(|s| Ok(Symbol::new(s.utf8()?)));
    Ok(Tables {
        consts: t.field("consts")?.items(read_const)?,
        builtins: t.field("builtins")?.items(|b| {
            let name = b.utf8()?;
            ply_eval::Builtin::from_name(name)
                .ok_or_else(|| b.error(format!("`{name}` is no builtin this runtime has")))
        })?,
        fields: symbols(t.field("fields")?)?,
        shapes: t.field("shapes")?.items(symbols)?,
        calls: strings(t.field("calls")?)?,
        lambdas: strings(t.field("lambdas")?)?,
        performs: strings(t.field("performs")?)?,
        handles: strings(t.field("handles")?)?,
        members: strings(t.field("members")?)?,
        symbols: t.field("symbols")?.items(|d| {
            Ok(Defined {
                symbol: d.field("symbol")?.utf8()?.to_string(),
                entry: d.field("entry")?.utf8()?.to_string(),
            })
        })?,
    })
}

/// An `emit.Const`. A float or a decimal crosses as its literal, which is read as the lexer reads it.
fn read_const(c: At<'_>) -> Result<Value, decode::Error> {
    let c = c.ctor()?;
    Ok(match c.name() {
        "ConstStr" => Value::str(c.arg(0)?.utf8()?),
        "ConstBytes" => Value::bytes(c.arg(0)?.bytes()?),
        "ConstFixed" => {
            let fixed = c.arg(0)?;
            let width = fixed.field("width")?;
            let ty = ply_eval::INT_TYPES
                .get(width.number::<usize>()?)
                .ok_or_else(|| width.error("a width the runtime does not number"))?;
            // The bits are the `Int` the width reads, so a negative one is the same pattern.
            Value::Fixed(ply_eval::Fixed::new(
                *ty,
                fixed.field("bits")?.int()? as u128,
            ))
        }
        "ConstChar" => {
            let point = c.arg(0)?;
            Value::Char(
                u32::try_from(point.int()?)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| point.error("a character that is not a Unicode scalar value"))?,
            )
        }
        // A 128-bit literal, as the two words an `Int` each holds.
        "ConstWide" => {
            let wide = c.arg(0)?;
            let width = wide.field("width")?;
            let ty = ply_eval::INT_TYPES
                .get(width.number::<usize>()?)
                .ok_or_else(|| width.error("a width the runtime does not number"))?;
            let high = u128::from(wide.field("high")?.int()? as u64);
            let low = u128::from(wide.field("low")?.int()? as u64);
            Value::Fixed(ply_eval::Fixed::new(*ty, high << 64 | low))
        }
        "ConstFloat" => {
            let text = c.arg(0)?;
            let literal = text.utf8()?.replace('_', "");
            Value::Float(
                literal
                    .parse()
                    .map_err(|_| text.error(format!("the float `{literal}` does not read")))?,
            )
        }
        "ConstDecimal" => {
            let text = c.arg(0)?;
            let literal = text.utf8()?.replace('_', "");
            Value::Decimal(
                literal
                    .trim_end_matches('m')
                    .parse()
                    .map_err(|_| text.error(format!("the decimal `{literal}` does not read")))?,
            )
        }
        "ConstUnit" => Value::Unit,
        _ => return Err(c.unknown()),
    })
}

fn strings(list: At<'_>) -> Result<Vec<String>, decode::Error> {
    list.items(|s| Ok(s.utf8()?.to_string()))
}
