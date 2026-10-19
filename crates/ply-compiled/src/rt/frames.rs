//! How an entry fails, and the handler frames it runs under: their clauses, the stacks a suspension keeps, arguments and aborts.

use super::*;

/// `Ctx::failed` when the fuel ran out.
pub const FAILED_OUT_OF_FUEL: i64 = 2;

/// A call would nest past what its stack holds, or a body or task could not be given a stack.
pub const FAILED_OUT_OF_STACK: i64 = 3;

/// A clause answered without resuming: `Ctx::unwind` carries its value to its `handle`.
pub const FAILED_UNWIND: i64 = 4;

/// The harness stopped the entry at its wall clock, which is not a verdict on the program.
pub const FAILED_ABANDONED: i64 = 5;

/// The entry spent its step budget without finishing.
pub const FAILED_OUT_OF_STEPS: i64 = 6;

/// The task was cancelled: it unwinds to its entry, which reports it cancelled rather than failed.
pub const FAILED_CANCELLED: i64 = 7;

/// A raise: `Ctx::aborting` names the `handle` whose clause answers it.
pub const FAILED_ABORT: i64 = 8;

/// The mode a compiled `perform` of a `raise` operation passes, beside 0 for a read and 1 for a
/// write.
pub(super) const MODE_RAISE: i64 = 2;

/// Calls a function value the runtime holds for a handler: through its descriptor where emitted
/// code made it, else as the release's code lays it out. Takes the arguments.
pub(crate) fn call_fun(ctx: *mut Ctx, emitted: bool, f: Word, args: &[Word]) -> Word {
    if emitted {
        unsafe { Block::of(f).call(ctx, args) }
    } else {
        call_value(ctx, f, args)
    }
}

/// One holder more of a function value the runtime holds for a handler.
pub(crate) fn dup_fun(emitted: bool, f: Word) {
    if emitted {
        Block::of(f).dup();
    } else {
        heap::inc(f);
    }
}

/// One holder fewer of a function value the runtime holds for a handler.
pub(crate) fn release_fun(emitted: bool, f: Word) {
    if emitted {
        unsafe { Block::of(f).let_go_function(running()) };
    } else {
        heap::dec(f);
    }
}

/// An installed handler: pushed by a `handle` site, searched innermost-out by a `perform`.
pub struct HandlerFrame {
    pub(super) clauses: Vec<FrameClause>,
    /// The `return` clause's closure, or zero.
    pub(super) ret: Word,
    /// Whether `ret` is a closure emitted code made.
    pub(super) emitted: bool,
    /// A `simulate` region's clause-less frame, answering `task`, `clock`, `random` and `sim`.
    pub(super) simulate: bool,
    /// For a `handle` resuming off the tail: the detached body whose own stack this frame bottoms.
    pub(super) detached: Option<usize>,
    /// The calls its stack had left when this frame went on, which its `handle` lands with: a
    /// frame a failure returns through gives its call back nowhere else.
    pub(super) fuel: i64,
    /// Which `handle` this frame is of, the same in every copy a task inherits or a continuation
    /// captures, so a resumption finds the variables of a clause of its own handler.
    pub(crate) handler: u64,
    /// The handler's variables, from when its body starts.
    pub(super) vars: Option<SharedVars>,
}

/// A handler's variables, held. A task's copy of the frame shares them, so tasks under one handler
/// see one set; a continuation's copy holds its own, so a resumption starts from them as they were
/// where it was captured. Given back by [`release_vars`], never by a drop, which could come after
/// the entry's heap is gone.
pub(crate) struct HandlerVars(pub(super) Vec<Var>);

/// One handler variable: a uniform word the release's code set, or a value emitted code set with
/// its type.
pub(crate) enum Var {
    Word(Word),
    Carried(Carried),
}

impl Var {
    fn copied(&self) -> Var {
        match self {
            Var::Word(w) => {
                heap::inc(*w);
                Var::Word(*w)
            }
            Var::Carried(v) => Var::Carried(v.dup(running())),
        }
    }

    fn release(self) {
        match self {
            Var::Word(w) => heap::dec(w),
            Var::Carried(v) => v.release(running()),
        }
    }
}

impl HandlerVars {
    /// The same values, each held once more.
    pub(crate) fn copied(&self) -> HandlerVars {
        HandlerVars(self.0.iter().map(Var::copied).collect())
    }

    /// Takes `v` as variable `i`, letting go of what it held.
    pub(super) fn set(&mut self, i: usize, v: Var) {
        if self.0.len() <= i {
            self.0.resize_with(i + 1, || Var::Word(heap::unit()));
        }
        std::mem::replace(&mut self.0[i], v).release();
    }

    /// Variable `i`, held once more for its reader: a uniform word, or a carried value's word.
    pub(super) fn get(&self, i: usize) -> Word {
        match self.0.get(i) {
            Some(Var::Carried(v)) => v.dup(running()).handed().word(),
            Some(Var::Word(w)) => {
                heap::inc(*w);
                *w
            }
            None => heap::unit(),
        }
    }

    /// Takes `w` as variable `i`, of the type the variable was given at its `handle`.
    pub(super) fn put(&mut self, i: usize, w: Handed) -> Result<(), &'static str> {
        let Some(Var::Carried(old)) = self.0.get(i) else {
            return Err("a handler's variable was set before its `handle` gave it a type");
        };
        let ty = old.ty();
        self.set(i, Var::Carried(unsafe { Carried::new(w, ty) }));
        Ok(())
    }
}

pub(crate) type SharedVars = Rc<std::cell::RefCell<HandlerVars>>;

pub(crate) fn shared(vars: HandlerVars) -> SharedVars {
    Rc::new(std::cell::RefCell::new(vars))
}

/// Lets go of one holder of `v`; the last gives back the values.
pub(crate) fn release_vars(v: SharedVars) {
    if let Ok(only) = Rc::try_unwrap(v) {
        for var in only.into_inner().0 {
            var.release();
        }
    }
}

impl HandlerFrame {
    /// The clause a raise is bound for, taken out of the frame, which keeps the rest.
    pub(crate) fn take_clause(&mut self, at: usize) -> FrameClause {
        self.clauses.swap_remove(at)
    }

    pub(super) fn simulate(fuel: i64) -> HandlerFrame {
        HandlerFrame {
            clauses: Vec::new(),
            ret: 0,
            emitted: false,
            simulate: true,
            detached: None,
            fuel,
            handler: 0,
            vars: None,
        }
    }

    /// The bottom of a detached body's own stack, which starts with the calls its opener had left.
    pub(crate) fn detached(
        clauses: Vec<FrameClause>,
        id: usize,
        fuel: i64,
        handler: u64,
    ) -> HandlerFrame {
        HandlerFrame {
            clauses,
            ret: 0,
            emitted: false,
            simulate: false,
            detached: Some(id),
            fuel,
            handler,
            vars: None,
        }
    }

    /// The variables a clause or the `return` clause of this frame runs with, by its handler.
    pub(crate) fn held(&self) -> Option<(u64, SharedVars)> {
        self.vars.as_ref().map(|v| (self.handler, Rc::clone(v)))
    }

    /// A copy of the frame's variables of its own, for a clause that works on one, by its handler.
    pub(crate) fn vars_copied(&self) -> Option<(u64, SharedVars)> {
        self.vars
            .as_ref()
            .map(|v| (self.handler, shared(v.borrow().copied())))
    }

    /// The frame's variables set to `values`, each held once more.
    pub(crate) fn set_vars(&mut self, values: &HandlerVars) {
        match &self.vars {
            Some(v) => {
                let old = std::mem::replace(&mut *v.borrow_mut(), values.copied());
                for var in old.0 {
                    var.release();
                }
            }
            None => self.vars = Some(shared(values.copied())),
        }
    }

    /// Puts its stack's count of calls left back where it stood when the frame went on: every
    /// frame above the `handle` is gone by the time it lands.
    pub(crate) fn land(&self, c: &mut Ctx) {
        debug_assert!(
            c.failed != 0 || c.fuel == self.fuel,
            "a `handle` body that returned left {} calls where it found {}",
            c.fuel,
            self.fuel
        );
        c.fuel = self.fuel;
    }
}

/// One stack's handler frames, chained to the stack it was entered from, which a `perform`
/// searches next. A depth names a frame within one stack.
pub(crate) struct Frames {
    pub(crate) list: Vec<HandlerFrame>,
    pub(crate) parent: Option<usize>,
    /// The detached body this stack is the own stack of; its frame at the bottom of `list` is
    /// hidden while one of its clauses runs, so it cannot say.
    pub(crate) body: Option<usize>,
    /// For a task's stack, the stack its region was entered from, where the task's body was
    /// written; a production task's `parent` is the scheduler loop's, which does not lead there.
    pub(crate) entered_from: Option<usize>,
    /// The variables of each clause running on this stack, the innermost last, by handler: a
    /// stack's own, since tasks run their clauses interleaved.
    pub(crate) held: Vec<(u64, SharedVars)>,
    /// What runtime frames on this stack hold while they call into the program, the innermost
    /// last: kept here rather than in their locals, so a snapshot of the stack copies it.
    pub(crate) kept: Vec<Kept>,
}

impl Frames {
    pub(crate) fn under(parent: Option<usize>) -> Frames {
        Frames {
            list: Vec::new(),
            parent,
            body: None,
            entered_from: None,
            held: Vec::new(),
            kept: Vec::new(),
        }
    }
}

/// What a runtime frame holds while it calls into the program. A frame's locals are on the stack a
/// snapshot copies byte for byte, so memory one owned would be freed once per restore; what is kept
/// in [`Frames::kept`] is copied with the snapshot instead, and a restore puts the copy back.
pub(crate) enum Kept {
    /// The frames a clause runs outside of, by the stack each came off.
    Hidden(Vec<(usize, Vec<HandlerFrame>)>),
    /// The frame of a `handle` whose `return` clause runs.
    Landing(HandlerFrame),
    /// The raise a `bracket`'s release runs during.
    Raising(Aborting),
    /// The arguments a clause emitted code made is called with, which it takes as it is entered.
    Args(Vec<Word>),
}

impl Kept {
    pub(super) fn release(self) {
        match self {
            Kept::Hidden(groups) => groups
                .into_iter()
                .flat_map(|(_, frames)| frames)
                .for_each(drop_frame),
            Kept::Landing(f) => drop_frame(f),
            Kept::Raising(a) => {
                a.into_failure();
            }
            Kept::Args(_) => {}
        }
    }
}

/// `clause` of `emitted` code's making called with `args`, which it takes, while `held` are the
/// clause's variables. A clause emitted code made reads its arguments from where a snapshot of this
/// stack copies them, so a stop inside it that is resumed more than once frees them once a resume.
pub(crate) fn call_clause(
    ctx: *mut Ctx,
    clause: Word,
    emitted: bool,
    args: Vec<Word>,
    held: Option<(u64, SharedVars)>,
) -> Word {
    let c = unsafe { &mut *ctx };
    if !emitted {
        let args = Args::of(c, args);
        let r = Ctx::holding(ctx, held, || call_value(ctx, clause, args.words()));
        args.release();
        return r;
    }
    let at = args.as_ptr();
    let n = args.len();
    let here = c.keep(Kept::Args(args));
    let r = Ctx::holding(ctx, held, || {
        call_fun(ctx, true, clause, unsafe {
            std::slice::from_raw_parts(at, n)
        })
    });
    unsafe { &mut *ctx }.kept_back(here).release();
    r
}

/// A stack's frames, what its runtime frames keep and the variables its running clauses hold, as a
/// snapshot copies them.
pub(crate) struct StackCopy {
    pub(crate) list: Vec<HandlerFrame>,
    pub(crate) kept: Vec<Kept>,
    pub(crate) held: Vec<(u64, SharedVars)>,
}

impl StackCopy {
    /// Each value held once more and each handler's variables copied; a running clause holds the
    /// copy of its frame's variables where the frame is among those copied, and a copy of its own
    /// otherwise.
    pub(crate) fn of(
        list: &[HandlerFrame],
        kept: &[Kept],
        held: &[(u64, SharedVars)],
    ) -> StackCopy {
        let mut copies: Vec<(*const std::cell::RefCell<HandlerVars>, SharedVars)> = Vec::new();
        let mut frame = |f: &HandlerFrame| {
            let copy = copied_frame(f, false);
            if let (Some(old), Some(new)) = (&f.vars, &copy.vars) {
                copies.push((Rc::as_ptr(old), Rc::clone(new)));
            }
            copy
        };
        let list = list.iter().map(&mut frame).collect();
        let kept = kept
            .iter()
            .map(|k| match k {
                Kept::Hidden(groups) => Kept::Hidden(
                    groups
                        .iter()
                        .map(|(s, frames)| (*s, frames.iter().map(&mut frame).collect()))
                        .collect(),
                ),
                Kept::Landing(f) => Kept::Landing(frame(f)),
                Kept::Raising(a) => Kept::Raising(a.copied()),
                Kept::Args(words) => Kept::Args(words.clone()),
            })
            .collect();
        let held = held
            .iter()
            .map(|(h, v)| {
                let mine = copies
                    .iter()
                    .find(|(old, _)| *old == Rc::as_ptr(v))
                    .map(|(_, new)| Rc::clone(new));
                (*h, mine.unwrap_or_else(|| shared(v.borrow().copied())))
            })
            .collect();
        StackCopy { list, kept, held }
    }

    pub(crate) fn release(self) {
        self.list.into_iter().for_each(drop_frame);
        self.kept.into_iter().for_each(Kept::release);
        for (_, v) in self.held {
            release_vars(v);
        }
    }
}

/// A call's arguments held in the calling runtime frame itself, so a snapshot of its stack copies
/// them with it; past [`ARGS_INLINE`], a flat record nothing counts the words of.
pub(crate) struct Args {
    pub(super) len: usize,
    pub(super) inline: [Word; ARGS_INLINE],
    pub(super) spill: Word,
}

pub(super) const ARGS_INLINE: usize = 16;

impl Args {
    /// Takes `words` whole, so no buffer of theirs outlives this, the call's frame included.
    pub(crate) fn of(c: &mut Ctx, words: Vec<Word>) -> Args {
        let mut inline = [0; ARGS_INLINE];
        let spill = if words.len() <= ARGS_INLINE {
            inline[..words.len()].copy_from_slice(&words);
            0
        } else {
            uncounted_words(c, &words)
        };
        Args {
            len: words.len(),
            inline,
            spill,
        }
    }

    pub(crate) fn words(&self) -> &[Word] {
        if self.spill == 0 {
            &self.inline[..self.len]
        } else {
            unsafe { std::slice::from_raw_parts(heap::words(obj(self.spill)), self.len) }
        }
    }

    /// Lets go of the spill, whose words the call took.
    pub(crate) fn release(self) {
        if self.spill != 0 {
            heap::dec(self.spill);
        }
    }
}

/// `words` in a flat record, which holds no count of them.
pub(super) fn uncounted_words(c: &mut Ctx, words: &[Word]) -> Word {
    let spill = uncounted(c, words.len());
    for (i, w) in words.iter().enumerate() {
        unsafe { set_word(obj(spill), i, *w) };
    }
    spill
}

/// A flat record of `n` words, which holds no count of what is put in them.
pub(super) fn uncounted(c: &mut Ctx, n: usize) -> Word {
    let o = c.heap.alloc(KIND_RECORD, heap::FLAT, n as u32, 0);
    for i in 0..n {
        unsafe { set_word(o, i, heap::unit()) };
    }
    o as Word
}

/// A raise on its way to the `handle` that answers it: that frame's stack and depth, which of its
/// clauses, what the clause is given, and what the entry fails with should the frame be gone.
pub(crate) struct Aborting {
    pub(crate) stack: usize,
    pub(crate) depth: usize,
    pub(crate) clause: usize,
    /// Held for the clause, which takes them.
    pub(crate) args: Raised,
    pub(crate) diagnostic: Diagnostic,
}

/// What a raise carries: uniform words the release's code raised, or values emitted code raised
/// with their types.
pub(crate) enum Raised {
    Words(Vec<Word>),
    Carried(Vec<Carried>),
}

impl Raised {
    fn copied(&self) -> Raised {
        match self {
            Raised::Words(ws) => {
                for w in ws {
                    heap::inc(*w);
                }
                Raised::Words(ws.clone())
            }
            Raised::Carried(vs) => {
                let ctx = running();
                Raised::Carried(vs.iter().map(|v| v.dup(ctx)).collect())
            }
        }
    }

    pub(crate) fn release(self) {
        match self {
            Raised::Words(ws) => ws.into_iter().for_each(heap::dec),
            Raised::Carried(vs) => {
                let ctx = running();
                vs.into_iter().for_each(|v| v.release(ctx));
            }
        }
    }

    /// The words the clause the raise is bound for is called with, which it takes.
    pub(crate) fn handed(self) -> Vec<Word> {
        match self {
            Raised::Words(ws) => ws,
            Raised::Carried(vs) => vs.into_iter().map(|v| v.handed().word()).collect(),
        }
    }
}

impl Aborting {
    /// The same raise, what it carries held once more.
    pub(super) fn copied(&self) -> Aborting {
        Aborting {
            stack: self.stack,
            depth: self.depth,
            clause: self.clause,
            args: self.args.copied(),
            diagnostic: self.diagnostic.clone(),
        }
    }

    /// The failure a raise no clause will now answer carries, with what it held let go.
    pub(crate) fn into_failure(self) -> Diagnostic {
        self.args.release();
        self.diagnostic
    }
}

/// One clause, under program-wide effect and resource names.
pub(crate) struct FrameClause {
    pub(super) effect: Symbol,
    pub(super) resource: Option<Symbol>,
    pub(super) op: Symbol,
    pub(crate) closure: Word,
    /// 0: never resumes; 1: resumes in tail position; 3: answers a raise, once its `handle`'s body
    /// is abandoned. Off the tail, only in a detached frame: 2 resumes at most once, 4 may resume
    /// again.
    pub(super) resumes: u8,
    /// `[*t]`: the clause's first slot after the parameters is the label the call site named.
    pub(super) binds_label: bool,
    /// Whether `closure` is a closure emitted code made.
    pub(crate) emitted: bool,
}

/// `abort.raise`, which the runtime itself raises for what fails on a value.
pub(super) fn is_abort(effect: &str, op: &str) -> bool {
    effect == "abort" && op == "raise"
}

impl FrameClause {
    /// Lets go of the clause's closure.
    pub(crate) fn release(self) {
        release_fun(self.emitted, self.closure);
    }

    /// `abort.raise` is a raise whatever its clause is marked: the runtime raises it itself.
    pub(super) fn raises(&self) -> bool {
        self.resumes == 3 || is_abort(self.effect.as_str(), self.op.as_str())
    }

    /// Resumes off the tail: its `handle`'s body runs detached, and a `perform` it answers stops it.
    pub(super) fn off_tail(&self) -> bool {
        matches!(self.resumes, 2 | 4)
    }

    /// May resume a stop more than once, so each is snapshotted.
    pub(super) fn again(&self) -> bool {
        self.resumes == 4
    }

    pub(super) fn answers_raise(&self, effect: &str, op: &str) -> bool {
        self.raises() && self.effect.as_str() == effect && self.op.as_str() == op
    }

    pub(super) fn answers(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>) -> bool {
        self.effect == *effect
            && self.op == *op
            && match (&self.resource, resource) {
                (None, _) => true,
                // A clause written `[*]` answers every label of the operation it names.
                (Some(mine), _) if mine.as_str() == "*" => true,
                (Some(mine), Some(theirs)) => mine == theirs,
                (Some(_), None) => false,
            }
    }

    /// Whether some operation `reached` stands for could be this clause's to answer: one of its
    /// effect, on its resource or on one `reached` does not name.
    pub(super) fn may_answer(&self, reached: &Reached) -> bool {
        self.effect == reached.effect
            && match (&self.resource, &reached.resource) {
                (None, _) | (_, None) => true,
                (Some(mine), _) if mine.as_str() == "*" => true,
                (Some(mine), Some(theirs)) => mine == theirs,
            }
    }
}

/// What a `parallel` block's branches may perform of one effect: on the resource named, or on any
/// where the block's row names none or leaves it to a label parameter.
pub(crate) struct Reached {
    pub(crate) effect: Symbol,
    pub(crate) resource: Option<Symbol>,
}

impl HandlerFrame {
    /// Whether a branch performing what `reach` lists could reach this frame.
    pub(super) fn within(&self, reach: &[Reached]) -> bool {
        self.simulate
            || self
                .clauses
                .iter()
                .any(|cl| reach.iter().any(|reached| cl.may_answer(reached)))
    }
}

/// `f` again, each closure held once more, and its handler's variables shared with it where
/// `sharing_vars`, else copied.
pub(super) fn copied_frame(f: &HandlerFrame, sharing_vars: bool) -> HandlerFrame {
    for cl in &f.clauses {
        dup_fun(cl.emitted, cl.closure);
    }
    if f.ret != 0 {
        dup_fun(f.emitted, f.ret);
    }
    HandlerFrame {
        clauses: f
            .clauses
            .iter()
            .map(|cl| FrameClause {
                effect: cl.effect.clone(),
                resource: cl.resource.clone(),
                op: cl.op.clone(),
                closure: cl.closure,
                resumes: cl.resumes,
                binds_label: cl.binds_label,
                emitted: cl.emitted,
            })
            .collect(),
        ret: f.ret,
        emitted: f.emitted,
        simulate: f.simulate,
        detached: f.detached,
        fuel: f.fuel,
        handler: f.handler,
        vars: f.vars.as_ref().map(|v| {
            if sharing_vars {
                Rc::clone(v)
            } else {
                shared(v.borrow().copied())
            }
        }),
    }
}

pub(super) fn copied_frames(list: &[HandlerFrame], sharing_vars: bool) -> Vec<HandlerFrame> {
    list.iter().map(|f| copied_frame(f, sharing_vars)).collect()
}

/// A spawned task's copies of the handlers around its spawn, at the bottom of its own stack; none
/// may name a detached body, whose clause would capture the spawner's stack. They share the
/// handlers' variables.
pub(crate) fn inherit_frames(list: &[HandlerFrame]) -> Vec<HandlerFrame> {
    copied_frames(list, true)
        .into_iter()
        .map(|f| {
            // The `handle` a copy stands for may be over before the task raises, so a raise the task
            // does not answer itself leaves for the region's own surroundings instead.
            let (raises, clauses): (Vec<FrameClause>, Vec<FrameClause>) =
                f.clauses.into_iter().partition(FrameClause::raises);
            raises.into_iter().for_each(FrameClause::release);
            HandlerFrame {
                clauses,
                detached: None,
                ..f
            }
        })
        .collect()
}

pub(crate) fn drop_frame(f: HandlerFrame) {
    f.clauses.into_iter().for_each(FrameClause::release);
    if f.ret != 0 {
        release_fun(f.emitted, f.ret);
    }
    if let Some(v) = f.vars {
        release_vars(v);
    }
}
