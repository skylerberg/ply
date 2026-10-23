//! The machine entering natively compiled code, from a command a user runs.

use crate::rt::Entry;
use crate::source::Source;
use anyhow::{Result, bail};
use ply_core::{
    Carry, Compilation, Counters, CtorCarries, DefHash, Diagnostic, Entered, Provider, Symbol,
    Value,
};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

/// The widest arity this boundary carries without allocating an argument array.
const MAX_ARITY: usize = 16;

/// One admitted definition: what an entry calls (its own code, or in an encoded unit its
/// `R#entered`), how many arguments it takes, what may answer an entry instead of running it, and
/// how its arguments and answer read.
struct Admitted {
    entry: Entry,
    arity: usize,
    memo: Memo,
    params: Vec<Carry>,
    answer: Carry,
    witnesses: Vec<usize>,
}

impl Admitted {
    /// How this entry's answer reads: its type, each variable bound by what the arguments show of
    /// it. What no argument shows stays open, since only a value of that type could show it, except
    /// a witnessed variable, which [`Admitted::entered_with`] passed as `Int`.
    fn reading(&self, args: &[Value], ctors: &CtorCarries) -> Cow<'_, Carry> {
        if !self.answer.mentions_var() {
            return Cow::Borrowed(&self.answer);
        }
        let mut vars = Vec::new();
        for (param, arg) in self.written().iter().zip(args) {
            param.bind(arg, &mut vars, ctors);
        }
        for &var in &self.witnesses {
            if vars.len() <= var {
                vars.resize(var + 1, Carry::Open);
            }
            if vars[var] == Carry::Open {
                vars[var] = Carry::Plain;
            }
        }
        Cow::Owned(self.answer.instantiate(&vars))
    }

    /// The parameters a caller from outside passes: those after the witnesses.
    fn written(&self) -> &[Carry] {
        self.params.get(self.witnesses.len()..).unwrap_or(&[])
    }

    /// `args` behind the witness of each variable it is entered with: the type the arguments show
    /// for it, or `Int`'s when none does.
    fn entered_with<'a>(&self, args: &'a [Value], ctors: &CtorCarries) -> Cow<'a, [Value]> {
        if self.witnesses.is_empty() {
            return Cow::Borrowed(args);
        }
        let shown = |var: usize| {
            self.written()
                .iter()
                .zip(args)
                .find_map(|(param, arg)| param.value_at(var, arg, ctors))
                .map_or(
                    ply_core::builtins::INT_WITNESS,
                    ply_core::builtins::witness_of,
                )
        };
        Cow::Owned(
            self.witnesses
                .iter()
                .map(|&var| Value::Int(shown(var)))
                .chain(args.iter().cloned())
                .collect(),
        )
    }
}

/// Decided once, from the purity the compiler published: an impure root has no memo to consult.
#[derive(Clone, Copy)]
enum Memo {
    Never,
    /// A pure root of no arguments, and its memo slot.
    Constant(usize),
    /// A pure root, whose entries over memo words alone are remembered.
    Calls,
}

/// Why an offered call was not taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Declines {
    /// The machine offered a name this unit did not compile.
    pub not_compiled: u64,
    /// It compiled the name and the call had the wrong number of arguments.
    pub arity: u64,
    /// An entry arrived while another was running.
    pub reentered: u64,

    /// The answer held a closure or a secret, which cannot cross out; a handle raises `E0449`.
    pub answer: u64,
    /// The answer held an `Int` word where neither its type nor the arguments say whether it is
    /// an `Int` or a width, or one that is no value of the width its type says.
    pub unread: u64,
}

impl Declines {
    pub fn total(&self) -> u64 {
        self.not_compiled + self.arity + self.reentered + self.answer + self.unread
    }
}

thread_local! {
    /// The unit a pre-flight on this thread loaded, beside the `Unit` it is for: the first worker
    /// on the thread takes it rather than mapping the same object a second time.
    static PREFLIGHT: RefCell<Option<(Weak<Unit>, crate::c::Native)>> = const { RefCell::new(None) };
}

/// A program's compiled unit, shared by every worker's backend, and freed with the last of them.
pub struct Unit {
    /// [`ply_core::Analysis::hashes_digest`] of the program this was built over, for
    /// `Compiled::describes`.
    identity: DefHash,
    source: Arc<Source>,
    /// The set the emitter compiles as one unit, closed under calls.
    compiled: Vec<String>,
    /// The subset of `compiled` the machine may be offered.
    members: BTreeSet<Symbol>,
    /// Definitions the emitter refused, with the construct that refused each.
    refusals: Vec<(String, String)>,
    counters: Counters,
    load_nanos: AtomicU64,
    compiles: AtomicU64,
    /// Workers whose load failed after the first load of the unit succeeded.
    poisoned: AtomicU64,
    /// What a worker that finds no load on its thread loads again.
    code: Code,
}

/// Where a unit's code comes from.
enum Code {
    /// The program this executable was linked with.
    Linked,
    /// A library the unit's C was compiled and linked into.
    Opened(std::path::PathBuf),
}

impl Code {
    fn load(&self, source: &Source) -> Result<(crate::c::Native, Vec<crate::c::Refused>)> {
        match self {
            Code::Linked => crate::c::load_linked(Some(source)),
            Code::Opened(path) => crate::c::load_opened(path, Some(source)),
        }
    }
}

impl Unit {
    /// The unit compiled into the library at `path`, loaded once: the first backend on this thread
    /// takes that load, and one that does not serve this runtime is an [`crate::c::Unserved`].
    pub fn opened(front: &ply_core::Analysis, path: &std::path::Path) -> Result<Arc<Unit>> {
        Unit::over(front, Code::Opened(path.to_path_buf()))
    }

    /// The program this executable was linked with, over `front`, what its front end answered of
    /// the roots it is entered by: the machine is offered those and no other definition it holds.
    pub fn linked(front: &ply_core::Analysis) -> Result<Arc<Unit>> {
        Unit::over(front, Code::Linked)
    }

    fn over(front: &ply_core::Analysis, code: Code) -> Result<Arc<Unit>> {
        let identity = front.hashes_digest;
        let source = Arc::new(Source::from_analysis(Arc::new(front.clone())));
        let (native, refused) = code.load(&source)?;
        let compiled = native.names();
        let offered = |name: &&String| match code {
            Code::Linked => source.row(name).is_some(),
            Code::Opened(_) => true,
        };
        let members: BTreeSet<Symbol> = (compiled.iter())
            .filter(offered)
            .filter(|name| native.enterable(name))
            .map(Symbol::new)
            .collect();
        let unit = Unit {
            identity,
            source,
            compiled,
            members,
            refusals: refused
                .into_iter()
                .map(|r| (r.function, r.construct))
                .collect(),
            counters: Counters::default(),
            load_nanos: AtomicU64::new(0),
            compiles: AtomicU64::new(0),
            poisoned: AtomicU64::new(0),
            code,
        };
        let unit = Arc::new(unit);
        PREFLIGHT.with(|slot| *slot.borrow_mut() = Some((Arc::downgrade(&unit), native)));
        Ok(unit)
    }

    /// The constructor table a unit over this program is emitted against.
    pub fn ctors(&self) -> Vec<(Symbol, usize)> {
        self.source.ctors()
    }

    /// The bodies this unit builds, as the concrete type rather than `dyn Compiled`, for tests.
    pub fn bodies(self: &Arc<Self>) -> Result<Rc<Bodies>> {
        self.build().map(Rc::new)
    }

    /// The definitions the emitter compiles, closed under calls.
    pub fn compiled(&self) -> &[String] {
        &self.compiled
    }

    /// What the emitter refused and the construct that refused it.
    pub fn refusals(&self) -> &[(String, String)] {
        &self.refusals
    }

    /// What this unit has spent compiling, in its two halves.
    pub fn compilation(&self) -> Compilation {
        Compilation {
            load_nanos: self.load_nanos.load(Ordering::Relaxed),
            units: self.compiles.load(Ordering::Relaxed),
        }
    }

    pub fn poisoned(&self) -> u64 {
        self.poisoned.load(Ordering::Relaxed)
    }

    fn build(self: &Arc<Self>) -> Result<Bodies> {
        let started = std::time::Instant::now();
        let preflown = PREFLIGHT.with(|slot| {
            let mut slot = slot.borrow_mut();
            match slot.take() {
                Some((unit, native)) if Weak::ptr_eq(&unit, &Arc::downgrade(self)) => Some(native),
                // A load left for a unit no one holds any more is let go.
                Some((unit, native)) if unit.strong_count() > 0 => {
                    *slot = Some((unit, native));
                    None
                }
                _ => None,
            }
        });
        let native = match preflown {
            Some(native) => native,
            None => self.code.load(&self.source)?.0,
        };
        self.load_nanos.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.compiles.fetch_add(1, Ordering::Relaxed);
        Bodies::new(Arc::clone(self), native)
    }
}

impl Provider for Unit {
    fn attach(self: Arc<Self>) -> Rc<dyn ply_core::Compiled> {
        if self.members.is_empty() {
            return Rc::new(Absent { unit: self });
        }
        match self.build() {
            Ok(bodies) => Rc::new(bodies),
            Err(e) => {
                eprintln!("the C backend built no unit for this program: {e:#}");
                self.poisoned.fetch_add(1, Ordering::Relaxed);
                Rc::new(Absent { unit: self })
            }
        }
    }

    fn name(&self) -> &'static str {
        "c"
    }

    fn len(&self) -> usize {
        self.members.len()
    }

    fn offers(&self) -> ply_core::Offers {
        self.counters.offers()
    }

    fn compilation(&self) -> Option<Compilation> {
        Some(Unit::compilation(self))
    }

    fn unbuilt(&self) -> u64 {
        self.poisoned()
    }
}

/// A worker whose compile failed: it declines everything and is counted.
struct Absent {
    unit: Arc<Unit>,
}

impl ply_core::Compiled for Absent {
    fn describes(&self, program: DefHash) -> bool {
        self.unit.identity == program
    }

    fn enter(&self, _name: &Symbol, args: &[Value], _budget: usize) -> Option<Value> {
        self.unit.counters.note_offer(args);
        None
    }
}

/// What an entry leaves for the machine to read after it, all of it from the body it ran.
#[derive(Default)]
struct LastEntry {
    steps: u64,
    held: ply_core::Held,
    record: Option<ply_core::region::Record>,
    performed: Vec<ply_core::EffectAtom>,
}

/// One worker's compiled bodies, offered to a `Machine` through `ply_core::Compiled`.
pub struct Bodies {
    unit: Arc<Unit>,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry; the `RefCell` forbids nesting, since `Ctx::slots` has no pop.
    ctx: RefCell<crate::rt::Ctx>,
    entered: Cell<u64>,
    declines: Cell<Declines>,
    /// `run` clears it first, so an entry no body ran for reports none of the one before it.
    last: RefCell<LastEntry>,
    /// Whether the unit is entered through its roots' `R#entered`, its arguments and answers
    /// crossing encoded.
    encoded: bool,
    /// What an encoded unit's pure roots answered.
    kept: RefCell<Kept>,
    /// Kept alive because every [`Entry`] above points into its executable pages. Last, so the
    /// context and the memo, which may hold a word of its static data, go before it is unmapped.
    _code: crate::c::Native,
}

/// What an encoded unit's pure roots answered, as values, each under its root and its arguments'
/// encoding; and the identity of each answer and of its direct parts, by which an argument the
/// memo answered is known.
#[derive(Default)]
struct Kept {
    answers: HashMap<(Symbol, Vec<u8>), Value>,
    calls: usize,
    known: HashSet<crate::rt::Identity>,
}

impl Kept {
    /// Whether every one of `args` is a value the memo answered, or a direct part of one.
    fn knows_all(&self, args: &[Value]) -> bool {
        args.iter()
            .all(|v| crate::rt::identity(v).is_some_and(|id| self.known.contains(&id)))
    }

    /// Keeps `value` as the answer of `key`, a call over arguments the memo answered only while
    /// fewer than [`crate::rt::CALL_MEMO_LIMIT`] are kept.
    fn keep(&mut self, key: (Symbol, Vec<u8>), value: &Value, call: bool) {
        if call {
            if self.calls >= crate::rt::CALL_MEMO_LIMIT {
                return;
            }
            self.calls += 1;
        }
        self.known.extend(crate::rt::identities(value));
        self.answers.insert(key, value.clone());
    }
}

impl Bodies {
    fn new(unit: Arc<Unit>, code: crate::c::Native) -> Result<Bodies> {
        if let Some(what) = code.tables().retains_a_handle() {
            bail!(
                "the constant pool holds {what}, which must not outlive the call that made it; \
                 refusing the whole registration rather than entering anything"
            );
        }
        let mut admitted = HashMap::new();
        let encoded = code.encoded();
        for name in &unit.members {
            let entry = if encoded {
                code.entered(name.as_str())
            } else {
                code.entry(name.as_str())
            };
            let Some(entry) = entry else {
                bail!("`{name}` was admitted and cannot be entered");
            };
            let Some(arity) = code.arity(name.as_str()) else {
                bail!("`{name}` was compiled without an arity");
            };
            if !encoded && arity > MAX_ARITY {
                bail!(
                    "`{name}` takes {arity} arguments and this boundary carries {MAX_ARITY}; \
                     refusing the registration rather than leaving one name that declines \
                     every call and no reason recorded against it"
                );
            }
            // An embedded unit's slots were chosen when it was built; this program's purity decides.
            let memo = match (
                unit.source.pure(name.as_str()),
                code.constant_index(name.as_str()),
            ) {
                (false, _) => Memo::Never,
                (true, Some(slot)) => Memo::Constant(slot),
                (true, None) => Memo::Calls,
            };
            let row = unit.source.row(name.as_str());
            admitted.insert(
                name.clone(),
                Admitted {
                    entry,
                    arity,
                    memo,
                    params: row.map_or_else(Vec::new, |r| r.params.clone()),
                    answer: row.map_or(Carry::Open, |r| r.answer.clone()),
                    witnesses: row.map_or_else(Vec::new, |r| r.witnesses.clone()),
                },
            );
        }
        let mut ctx = code.context();
        ctx.program = Some(Arc::clone(&unit.source.front));
        let ctx = RefCell::new(ctx);
        Ok(Bodies {
            unit,
            _code: code,
            admitted,
            ctx,
            entered: Cell::new(0),
            declines: Cell::new(Declines::default()),
            last: RefCell::new(LastEntry::default()),
            encoded,
            kept: RefCell::default(),
        })
    }

    /// Entries the backend answered or raised, a memo's answers among them, over this backend's
    /// whole life.
    pub fn entered(&self) -> u64 {
        self.entered.get()
    }

    /// Runs `f` with the context borrowed, so a test can reach the reentrancy decline.
    pub fn while_entered<T>(&self, f: impl FnOnce() -> T) -> T {
        let _held = self.ctx.borrow_mut();
        f()
    }

    /// Whether this backend would be offered `name` at all.
    pub fn admits(&self, name: &str) -> bool {
        self.admitted.contains_key(&Symbol::new(name))
    }

    pub fn reset_counts(&self) {
        self.entered.set(0);
        self.declines.set(Declines::default());
    }

    pub fn declines(&self) -> Declines {
        self.declines.get()
    }

    fn decline(&self, mut f: impl FnMut(&mut Declines)) -> EntryOutcome {
        let mut d = self.declines.get();
        f(&mut d);
        self.declines.set(d);
        EntryOutcome::Declined
    }

    fn run(&self, name: &Symbol, args: &[Value], fuel: usize) -> EntryOutcome {
        *self.last.borrow_mut() = LastEntry::default();
        let Some(admitted) = self.admitted.get(name) else {
            return self.decline(|d| d.not_compiled += 1);
        };
        if admitted.arity != admitted.witnesses.len() + args.len() {
            return self.decline(|d| d.arity += 1);
        }
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return self.decline(|d| d.reentered += 1);
        };
        let filled = admitted.entered_with(args, &self.unit.source.front.ctor_carries);
        if self.encoded {
            return self.run_encoded(name, admitted, &mut ctx, &filled, fuel);
        }

        let tables = std::sync::Arc::clone(&ctx.tables);
        if let Memo::Constant(index) = admitted.memo
            && let Some(kept) = tables.memoized(index)
            && let Some(value) = tables.memo_value(kept)
        {
            drop(ctx);
            self.unit.counters.note_converted(0, 0);
            self.entered.set(self.entered.get() + 1);
            return EntryOutcome::Answered(value);
        }
        let calls = i64::try_from(fuel).unwrap_or(i64::MAX);
        ctx.begin(calls);
        // Values are deep-converted in and out: nothing outside the entry ever holds a word.
        let mut handles = [0i64; MAX_ARITY];
        let before = ctx.heap.allocated();
        // A call whose arguments are all memoized words is itself memoized.
        let mut all_memo = matches!(admitted.memo, Memo::Calls) && !filled.is_empty();
        for (slot, value) in handles.iter_mut().zip(filled.iter()) {
            *slot = match tables.memo_word(value) {
                Some(w) => w,
                None => {
                    all_memo = false;
                    ctx.heap.to_word(&tables.layouts, value)
                }
            };
        }
        let words = &handles[..filled.len()];
        if all_memo && let Some(value) = tables.memo_call(name, words) {
            ctx.end();
            drop(ctx);
            self.unit.counters.note_converted(0, 0);
            self.entered.set(self.entered.get() + 1);
            return EntryOutcome::Answered(value);
        }
        let inward = (ctx.heap.allocated() - before) as u64;
        let entering = &mut *ctx as *mut crate::rt::Ctx;
        // SAFETY: `self._code` owns the entry's pages, `ctx` is uniquely borrowed, and
        // `handles` is `MAX_ARITY` wide, which `Bodies::new` refused to exceed.
        let out = crate::rt::on_known_stack(entering, || unsafe {
            (admitted.entry)(entering, handles.as_ptr())
        });
        let out = self.returned(&mut ctx, Some(out));
        if ctx.failed != 0 {
            return self.failure(&mut ctx, fuel);
        }
        debug_assert_eq!(
            ctx.fuel, calls,
            "`{name}` returned without every call it made given back, or with more"
        );

        // Memoize the word and its converted value, so later entries skip both run and conversion.
        let mut walked = crate::heap::Walked::default();
        let ctors = &self.unit.source.front.ctor_carries;
        let value = crate::heap::Heap::read(
            &tables.layouts,
            out,
            &admitted.reading(args, ctors),
            ctors,
            &mut walked,
        );
        let kept = match admitted.memo {
            Memo::Constant(index) if crate::heap::world_independent(out) => {
                Some(tables.memoize(index, out))
            }
            Memo::Calls if all_memo && crate::heap::world_independent(out) => {
                tables.memoize_call(name, words, out)
            }
            _ => None,
        };
        ctx.end();
        drop(ctx);
        // Covers every handle `escape` refuses; a closure or secret that holds none declines.
        if walked.handle {
            return self.holding_a_handle(name, &value);
        }
        if walked.unread {
            return self.decline(|d| d.unread += 1);
        }
        if let Some(kept) = kept {
            tables.remember(kept, &value);
        }
        self.unit.counters.note_converted(inward, walked.read);
        self.entered.set(self.entered.get() + 1);
        EntryOutcome::Answered(value)
    }

    /// [`Bodies::run`] of an encoded unit: the arguments cross in as one encoded list, which the
    /// root's `R#entered` reads before it calls the root and writes the answer's encoding, and the
    /// memo keeps values under the arguments' encoding.
    fn run_encoded(
        &self,
        name: &Symbol,
        admitted: &Admitted,
        ctx: &mut crate::rt::Ctx,
        filled: &[Value],
        fuel: usize,
    ) -> EntryOutcome {
        let (bytes, held) = crate::rt::encoded(&Value::list(filled.to_vec()));
        // A call whose arguments are all answers of the memo, or their parts, is itself memoized.
        let keeps = held.is_empty()
            && match admitted.memo {
                Memo::Never => false,
                Memo::Constant(_) => true,
                Memo::Calls => !filled.is_empty() && self.kept.borrow().knows_all(filled),
            };
        let key = keeps.then(|| (name.clone(), bytes.clone()));
        if let Some(key) = &key
            && let Some(value) = self.kept.borrow().answers.get(key)
        {
            self.entered.set(self.entered.get() + 1);
            return EntryOutcome::Answered(value.clone());
        }
        let calls = i64::try_from(fuel).unwrap_or(i64::MAX);
        ctx.begin(calls);
        ctx.open_entry(bytes, held);
        let entering = std::ptr::from_mut(ctx);
        let none: [i64; 0] = [];
        // SAFETY: `self._code` owns the entry's pages, `ctx` is uniquely borrowed, and an
        // `R#entered` takes no words.
        crate::rt::on_known_stack(entering, || unsafe {
            (admitted.entry)(entering, none.as_ptr())
        });
        self.returned(ctx, None);
        if ctx.failed != 0 {
            return self.failure(ctx, fuel);
        }
        debug_assert_eq!(
            ctx.fuel, calls,
            "`{name}` returned without every call it made given back, or with more"
        );
        let answer = crate::rt::close_entry(ctx);
        ctx.end();
        let value = match answer {
            Ok(crate::rt::EntryAnswer::Read {
                value,
                holding: false,
            }) => value,
            Ok(crate::rt::EntryAnswer::Read {
                value,
                holding: true,
            }) => return self.holding_a_handle(name, &value),
            Ok(crate::rt::EntryAnswer::Function) => return self.decline(|d| d.answer += 1),
            Err(why) => {
                self.entered.set(self.entered.get() + 1);
                return EntryOutcome::Raised(crate::rt::err_crossing(&why));
            }
        };
        if let Some(key) = key {
            let call = matches!(admitted.memo, Memo::Calls);
            self.kept.borrow_mut().keep(key, &value, call);
        }
        let inward = filled.iter().map(objects_of).sum();
        self.unit
            .counters
            .note_converted(inward, objects_of(&value));
        self.entered.set(self.entered.get() + 1);
        EntryOutcome::Answered(value)
    }

    /// What an entry that returned `out` leaves for the machine, kept for it to read: its
    /// production region's other tasks run to their ends first, which may answer another word.
    /// `out` is none for an entry that wrote its answer's encoding itself.
    fn returned(
        &self,
        ctx: &mut crate::rt::Ctx,
        out: Option<crate::heap::Word>,
    ) -> crate::heap::Word {
        let mut answer = out.unwrap_or(0);
        let production = ctx.sims.last().filter(|sim| sim.is_production());
        let held = ply_core::Held {
            bridges: ctx.heap.bridges() as u64,
            scheduled: production.map_or(0, |sim| sim.scheduled() as u64),
            task_stacks: production.map_or(0, |sim| sim.task_slots() as u64),
            frame_tables: ctx.stack_slots() as u64,
        };
        if ctx.sims.last().is_some_and(|sim| sim.is_production()) {
            answer = unsafe { crate::simulate::finish_root(std::ptr::from_mut(ctx), out) };
        }
        if let Some(rt) = ctx.runtime.clone() {
            let warned = rt.end_entry_point(ctx.id);
            ctx.teardown.extend(warned);
        }
        *self.last.borrow_mut() = LastEntry {
            steps: u64::try_from(ctx.ticks).unwrap_or(0),
            held,
            record: ctx.record.take(),
            performed: std::mem::take(&mut ctx.performed),
        };
        answer
    }

    /// How an entry that failed ends: what it raised, or a decline where it raised nothing.
    fn failure(&self, ctx: &mut crate::rt::Ctx, fuel: usize) -> EntryOutcome {
        let raised = if ctx.failed == crate::rt::FAILED_OUT_OF_FUEL {
            // The backend alone: no machine follows, so the budget is reported from here.
            Some(
                ply_core::Diagnostic::error(
                    ply_core::codes::RUNTIME_ERROR,
                    format!("recursion limit of {fuel} nested calls exceeded"),
                )
                .primary(ctx.site(), "the call that overran it"),
            )
        } else {
            ctx.take_failure().or_else(|| match ctx.failed {
                crate::rt::FAILED_UNWIND => Some(ply_core::Diagnostic::error(
                    ply_core::codes::RUNTIME_ERROR,
                    "a `handle` clause unwound past the compiled fragment's entry",
                )),
                // Every failure records why; one that did not is a fault to find, not a decline.
                crate::rt::FAILED => Some(
                    ply_core::Diagnostic::error(
                        ply_core::codes::INTERNAL_ERROR,
                        "the compiled body failed without recording why",
                    )
                    .primary(ctx.site(), "the last site it reached"),
                ),
                _ => None,
            })
        };
        ctx.end();
        self.entered.set(self.entered.get() + 1);
        match raised {
            Some(raised) => EntryOutcome::Raised(raised),
            None => EntryOutcome::Declined,
        }
    }

    /// An answer holding a handle: refused where the handle is bound to a region the entry closed,
    /// else declined, since nothing outside the entry can hold it.
    fn holding_a_handle(&self, name: &Symbol, value: &Value) -> EntryOutcome {
        let boundary = ply_core::Boundary::EntryAnswer {
            name: name.as_str(),
        };
        let span = self
            .unit
            .source
            .span_of(name.as_str())
            .unwrap_or(ply_core::Span::DUMMY);
        if let Err(refused) = ply_core::escape::check(&boundary, value, span) {
            self.entered.set(self.entered.get() + 1);
            return EntryOutcome::Raised(refused);
        }
        self.decline(|d| d.answer += 1)
    }
}

/// How one entry ended.
/// How many objects the value `v` crossing the seam is made of: each string, byte string,
/// container, record and constructor holding arguments; an immediate is none.
fn objects_of(v: &Value) -> u64 {
    let mut pending = vec![v];
    let mut n = 0u64;
    while let Some(v) = pending.pop() {
        match v {
            Value::Str(_) | Value::Bytes(_) => n += 1,
            Value::List(items) => {
                n += 1;
                pending.extend(items.iter());
            }
            Value::Array(items) => {
                n += 1;
                pending.extend(items.iter());
            }
            Value::Map(entries) => {
                n += 1;
                for (k, v) in entries.iter() {
                    pending.push(k);
                    pending.push(v);
                }
            }
            Value::Record(fields) => {
                n += 1;
                pending.extend(fields.iter().map(|(_, v)| v));
            }
            Value::Ctor { args, .. } if !args.is_empty() => {
                n += 1;
                pending.extend(args.iter());
            }
            _ => {}
        }
    }
    n
}

enum EntryOutcome {
    Answered(Value),
    Raised(Diagnostic),
    Declined,
}

impl EntryOutcome {
    fn answer(self) -> Option<Value> {
        match self {
            EntryOutcome::Answered(value) => Some(value),
            EntryOutcome::Raised(_) | EntryOutcome::Declined => None,
        }
    }
}

impl ply_core::Compiled for Bodies {
    fn describes(&self, program: DefHash) -> bool {
        self.unit.identity == program
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.unit.counters.note_offer(args);
        self.run(name, args, budget).answer()
    }

    fn enter_test(&self, name: &Symbol, budget: usize) -> Entered {
        self.unit.counters.note_offer(&[]);
        match self.run(name, &[], budget) {
            EntryOutcome::Answered(value) => Entered::Answered(value),
            EntryOutcome::Raised(raised) => Entered::Raised(raised),
            EntryOutcome::Declined => Entered::Declined,
        }
    }

    fn enter_whole(&self, name: &Symbol, args: &[Value], budget: usize) -> Entered {
        self.unit.counters.note_offer(args);
        match self.run(name, args, budget) {
            EntryOutcome::Answered(value) => Entered::Answered(value),
            EntryOutcome::Raised(raised) => Entered::Raised(raised),
            EntryOutcome::Declined => Entered::Declined,
        }
    }

    fn take_performed(&self) -> Vec<ply_core::EffectAtom> {
        std::mem::take(&mut self.last.borrow_mut().performed)
    }

    fn set_seed(&self, seed: ply_core::Seed, steps: u32) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.seed = seed;
            ctx.sim_steps = steps.max(1);
        }
    }

    fn set_machine(&self, machine: ply_core::host::MachineId) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.id = machine;
        }
    }

    fn steps(&self) -> u64 {
        self.last.borrow().steps
    }

    fn held(&self) -> ply_core::Held {
        self.last.borrow().held
    }

    fn simulated(&self) -> Option<ply_core::region::Record> {
        self.last.borrow().record.clone()
    }

    fn set_host(
        &self,
        binding: std::sync::Arc<ply_core::HostBinding>,
        runtime: Option<std::rc::Rc<dyn ply_core::HostRuntime>>,
        factory: Option<ply_core::RuntimeFactory>,
    ) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.set_host(binding, runtime, factory);
        }
    }

    fn set_declared(&self, declared: Option<ply_core::Footprint>) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.declared = declared;
        }
    }

    fn set_re_executed(&self, re_executed: bool) {
        if let Ok(mut ctx) = self.ctx.try_borrow_mut() {
            ctx.re_executed = re_executed;
        }
    }

    fn take_host_use(&self) -> (ply_core::host::HostUse, u64) {
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return Default::default();
        };
        (
            std::mem::take(&mut ctx.host_use),
            std::mem::take(&mut ctx.host_ops),
        )
    }

    fn take_teardown(&self) -> Vec<ply_core::Diagnostic> {
        self.ctx
            .try_borrow_mut()
            .map(|mut ctx| std::mem::take(&mut ctx.teardown))
            .unwrap_or_default()
    }
}
