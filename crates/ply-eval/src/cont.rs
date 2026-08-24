//! The explicit control stack, and the delimited continuations cut out of it.
//!
//! The stack is a list of **segments**. A segment is a persistent list of
//! frames sitting on top of the `handle` that delimits it; the outermost
//! segment has no delimiter. The innermost segment is held by value and the
//! rest as a persistent list, because every push and every pop rewrites the
//! innermost one and nothing else can be looking at that rewrite. Capturing a
//! continuation is taking the segments down to and including the segment whose
//! prompt matches — a `Vec` of two pointers each, one entry per enclosing
//! handler crossed, and never one entry per pending frame. That is what makes a second resumption cost the same as
//! the first, and it is the reason multi-shot is affordable rather than
//! theoretical.
//!
//! Resuming pushes those segments back onto whatever stack is current. Because
//! the captured slice carries its own prompt, the handler is reinstalled by the
//! act of resuming: handlers are **deep**, and a clause runs on the stack
//! *below* its own handler so that a clause performing the operation it handles
//! reaches the next handler out rather than itself.

use crate::arena::{Pin, RegionId};
use crate::code::{Clause, Code, ReturnArm, Stmt};
use crate::env::Env;
use crate::pool::{self, Free, Link, Pooled};
use crate::value::{Value, Vector};
use ply_span::{Span, Symbol};
use ply_syntax::ast::{BinOp, Ident, UnOp};
use std::cell::Cell;
use std::rc::Rc;

/// One suspended step. Every variant names the value it is waiting for and what
/// it will do with it; together they cover `NodeKind` exhaustively, plus the
/// three prelude builtins that call back into user code.
#[derive(Clone)]
pub enum Frame {
    Unary {
        op: UnOp,
        operand_span: Span,
        span: Span,
    },

    /// Waiting for the left operand; the right one is still code.
    BinaryRhs {
        op: BinOp,
        rhs: Code,
        env: Env,
        module: usize,
        lhs_span: Span,
        span: Span,
    },

    /// Waiting for the right operand, holding the evaluated left one.
    BinaryApply {
        op: BinOp,
        lhs: Value,
        lhs_span: Span,
        rhs_span: Span,
        span: Span,
    },

    /// `&&` and `||`: the left operand decided nothing, so evaluate the right.
    ShortCircuit {
        op: BinOp,
        rhs: Code,
        env: Env,
        module: usize,
        rhs_span: Span,
    },

    AppCallee {
        args: Rc<Vec<Code>>,
        env: Env,
        module: usize,
        span: Span,
    },

    /// Waiting for `args[next - 1]`, holding the callee and the arguments
    /// already evaluated.
    AppArgs {
        callee: Value,
        done: Vec<Value>,
        args: Rc<Vec<Code>>,
        next: usize,
        env: Env,
        module: usize,
        span: Span,
    },

    /// A user function's body is running. It transforms nothing: every frame
    /// that holds pending code carries its own module, so returning from a call
    /// restores the caller's scope with no help. It exists to bound recursion
    /// and to give the causal-slice tracer its enter and exit events.
    ///
    /// Every call pushes one, tail position included: eliding it for a tail
    /// call leaves a tail-recursive runaway unbounded here while the
    /// tree-walker diagnoses it in milliseconds.
    Call {
        name: Option<Symbol>,
        call_site: Span,
        /// This call is the one evaluating a nullary pure definition for the
        /// first time, so the value it receives is that definition's constant.
        /// See [`crate::memo`].
        memo: bool,
    },

    /// Hands the value it receives to a captured continuation. This is the
    /// whole of the tail-resumptive clause: `op(x) -> e` is `op(x) resume k ->
    /// k(e)`, and this frame is the `k(_)`.
    Resume {
        k: Rc<Continuation>,
    },

    If {
        then_branch: Code,
        else_branch: Code,
        env: Env,
        module: usize,
        cond_span: Span,
    },

    /// Waiting for the scrutinee. `next` is the first arm not yet tried, so the
    /// same frame serves the initial dispatch and a guard that failed.
    MatchArms {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        next: usize,
        env: Env,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for an arm's guard, holding the bindings that arm's pattern made
    /// so a failing guard can fall through without rematching.
    MatchGuard {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        at: usize,
        arm_env: Env,
        env: Env,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for `stmts[next - 1]`. `scope` accumulates `let` bindings.
    BlockStep {
        stmts: Rc<Vec<Stmt>>,
        next: usize,
        tail: Option<Code>,
        scope: Env,
        module: usize,
    },

    RecordField {
        done: Vec<(Symbol, Value)>,
        fields: Rc<Vec<(Symbol, Code)>>,
        next: usize,
        env: Env,
        module: usize,
    },

    FieldAccess {
        field: Ident,
        base_span: Span,
    },

    ListItem {
        done: Vec<Value>,
        items: Rc<Vec<Code>>,
        next: usize,
        env: Env,
        module: usize,
    },

    /// Waiting for `args[next - 1]` of a `perform`. When the last one lands the
    /// machine moves to the `Perform` state rather than pushing another frame.
    PerformArgs {
        effect: Symbol,
        op: Symbol,
        resource: Option<Symbol>,
        done: Vec<Value>,
        args: Rc<Vec<Code>>,
        next: usize,
        env: Env,
        module: usize,
        span: Span,
    },

    /// The cell is allocated only once the initial value lands.
    WithCellBody {
        resource: Symbol,
        binder: Symbol,
        body: Code,
        env: Env,
        module: usize,
        /// The whole `with_cell` expression, which is the key
        /// [`crate::region_kind`] filed its decision about this region under.
        region: Span,
    },

    /// A region's lexical close. Pushed under the region's body, so it is
    /// reached once the body has produced its value and — being below the
    /// prompt of any `handle` the body installs — never inside a captured
    /// segment.
    ///
    /// A clause that discards its continuation discards this frame with it and
    /// the region is left open; `TaskRegions::reset` is what closes it then, and
    /// an enclosing region's close absorbs it before that.
    CloseRegion {
        region: RegionId,
    },

    /// `map`, `filter` and `fold` call user code, so their loops are frames
    /// rather than host recursion — otherwise a continuation captured inside the
    /// function passed to `map` would be captured across a native frame that
    /// cannot be re-entered.
    MapStep {
        f: Value,
        items: Vector<Value>,
        next: usize,
        done: Vec<Value>,
        span: Span,
    },

    FilterStep {
        f: Value,
        items: Vector<Value>,
        next: usize,
        done: Vec<Value>,
        span: Span,
    },

    FoldStep {
        f: Value,
        items: Vector<Value>,
        next: usize,
        span: Span,
    },

    /// `map_fold`'s loop. It carries the entries it will visit rather than the
    /// map, because the order is the contract: a resumption has to continue over
    /// the same entries in the same ascending order, and a live cursor into a
    /// persistent tree would let two resumptions of one capture disagree.
    MapFoldStep {
        f: Value,
        entries: crate::map::Entries,
        next: usize,
        span: Span,
    },

    /// `bytes_position`'s loop. It is a frame for the same reason the three
    /// above are, and it is the one byte builtin that pays a boxed `Int` and a
    /// frame per byte examined — which is why the contract calls it the escape
    /// hatch and points at `bytes_scan` first.
    BytesPositionStep {
        f: Value,
        bytes: std::sync::Arc<[u8]>,
        next: usize,
        span: Span,
    },
}

pub struct Prompt {
    pub clauses: Rc<Vec<Clause>>,
    /// Each clause's effect under its program-wide name, resolved where the
    /// `handle` was written. A perform reached from another module spells the
    /// same effect differently and the two only meet once both are qualified.
    pub effects: Rc<Vec<Symbol>>,
    pub ret: Option<Rc<ReturnArm>>,
    pub env: Env,
    pub module: usize,
    pub span: Span,
}

impl Prompt {
    /// The index of the clause handling this operation, innermost clause order.
    /// A clause without a resource label handles every resource of its
    /// operation; an operation declared without `[r]` has exactly one anyway.
    pub fn clause_for(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<usize> {
        self.clauses
            .iter()
            .zip(self.effects.iter())
            .position(|(c, e)| {
                e == effect
                    && c.op == *op
                    && match (&c.resource, resource) {
                        (None, _) => true,
                        (Some(cr), Some(r)) => cr == r,
                        (Some(_), None) => false,
                    }
            })
    }
}

/// Which simulated region a [`Delimiter::Sim`] belongs to: its ordinal among the
/// regions one entry point has entered.
///
/// An ordinal rather than a depth, because a test may run several regions in
/// sequence — only *nesting* is `E0416` — and a continuation captured in the
/// first must be recognized as foreign by the second rather than steering it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SimId(pub u32);

/// What delimits a segment. A `handle` expression, or the seeded scheduler a
/// `simulate` region installs — a native prompt whose clauses are Rust.
#[derive(Clone)]
pub enum Delimiter {
    Ply(Rc<Prompt>),
    Sim(SimId),
}

/// Where a perform was answered.
pub enum Target {
    Ply {
        prompt: Rc<Prompt>,
        clause: usize,
    },
    /// The seeded scheduler: a `task.*`, `clock.*` or `random.*` perform that
    /// reached a `simulate` region's delimiter before any `handle` that names
    /// it. A user handler nested inside the region therefore still wins, which
    /// is the ordinary innermost-first rule with no special case.
    Sim(SimId),
}

/// A persistent stack, shared by pointer. One allocation to push, because the
/// value lives in the link rather than behind a second pointer of its own, and
/// none at all to pop a link nothing else holds.
///
/// `rpds::List` is the obvious alternative and costs two of each, since it
/// boxes the value away from the link. A push and a pop are the machine's two
/// most frequent steps.
///
/// The link a pop retires is kept on [`crate::pool`]'s free list rather than
/// returned to the allocator, and a push takes one from there. That is a
/// memory-reuse change and not a representation change: the chain is still
/// persistent, a segment is still shared by pointer, and capture is still O(1)
/// in the frames it crosses.
struct Chain<T: Pooled> {
    head: Option<Rc<Link<T>>>,
    len: usize,
}

impl<T: Pooled> Chain<T> {
    fn new() -> Chain<T> {
        Chain { head: None, len: 0 }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    fn push(mut self, value: T) -> Chain<T> {
        let len = self.len + 1;
        Chain {
            head: Some(pool::link(value, self.head.take())),
            len,
        }
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        let mut cur = self.head.as_deref();
        std::iter::from_fn(move || {
            let link = cur?;
            cur = link.next.as_deref();
            link.value.as_ref()
        })
    }
}

impl<T: Pooled + Clone> Chain<T> {
    /// Moves the head out when this chain is its only owner, which is every pop
    /// no captured continuation is sharing.
    fn pop_front(&mut self) -> Option<T> {
        let mut node = self.head.take()?;
        self.len -= 1;
        match Rc::get_mut(&mut node) {
            Some(link) => {
                let value = link.value.take();
                self.head = link.next.take();
                pool::give(node);
                value
            }
            None => {
                self.head = node.next.clone();
                node.value.clone()
            }
        }
    }
}

impl<T: Pooled> Clone for Chain<T> {
    fn clone(&self) -> Chain<T> {
        Chain {
            head: self.head.clone(),
            len: self.len,
        }
    }
}

impl<T: Pooled> Default for Chain<T> {
    fn default() -> Chain<T> {
        Chain::new()
    }
}

/// Iterative, because nothing bounds the frames pending on a stack at a depth
/// the native stack could survive unwinding recursively.
///
/// > **Corrected (2026-08-24): this used to read "bounded by
/// > `DEFAULT_MAX_FRAMES` and not by anything the native stack could survive".**
/// > There is no longer a default frame ceiling — see
/// > [`crate::Machine::with_max_frames`] — so the loop below is more load-bearing
/// > than it was, not less: what used to cap the chain at a million links now
/// > caps it at nothing but memory.
impl<T: Pooled> Drop for Chain<T> {
    fn drop(&mut self) {
        let mut cur = self.head.take();
        while let Some(mut node) = cur {
            match Rc::get_mut(&mut node) {
                Some(link) => {
                    link.value = None;
                    cur = link.next.take();
                    pool::give(node);
                }
                None => break,
            }
        }
    }
}

thread_local! {
    static FRAME_LINKS: Free<Frame> = const { Free::new() };
    static SEGMENT_LINKS: Free<Segment> = const { Free::new() };
}

impl Pooled for Frame {
    fn free() -> &'static std::thread::LocalKey<Free<Frame>> {
        &FRAME_LINKS
    }
}

impl Pooled for Segment {
    fn free() -> &'static std::thread::LocalKey<Free<Segment>> {
        &SEGMENT_LINKS
    }
}

#[derive(Clone, Default)]
pub struct Segment {
    frames: Chain<Frame>,
    delimiter: Option<Delimiter>,
    calls: usize,
}

impl Segment {
    pub fn base() -> Segment {
        Segment::default()
    }

    pub fn under(prompt: Rc<Prompt>) -> Segment {
        Segment::below(Delimiter::Ply(prompt))
    }

    pub fn below(delimiter: Delimiter) -> Segment {
        Segment {
            frames: Chain::new(),
            delimiter: Some(delimiter),
            calls: 0,
        }
    }

    pub fn delimiter(&self) -> Option<&Delimiter> {
        self.delimiter.as_ref()
    }

    pub fn prompt(&self) -> Option<&Rc<Prompt>> {
        match &self.delimiter {
            Some(Delimiter::Ply(prompt)) => Some(prompt),
            _ => None,
        }
    }

    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    pub fn calls(&self) -> usize {
        self.calls
    }
}

fn is_call(frame: &Frame) -> usize {
    usize::from(matches!(frame, Frame::Call { .. }))
}

/// What the machine does with the value it is currently returning.
pub enum Next {
    Frame(Frame, Stack),
    /// The delimited body finished. Apply this handler's `return` clause, if
    /// any, and carry on with the stack the handler was installed on; for a
    /// `Sim` delimiter it is a task's body that returned.
    Leave(Delimiter, Stack),
    Done,
}

pub struct Handled {
    /// How many segments to capture, counting from the innermost. Always at
    /// least one: the segment delimited by the handler itself.
    pub segments: usize,
    pub target: Target,
}

#[derive(Clone, Default)]
pub struct Stack {
    /// The innermost segment, held by value rather than as the head of
    /// `under`. Every push and every pop rewrites exactly this, and reaching it
    /// through a persistent list meant rebuilding a shared node — two
    /// allocations to install a frame and two more to retire it — for a segment
    /// nothing else was looking at.
    top: Segment,
    /// The segments below `top`, head first. The last one is always the base,
    /// so `top.delimiter.is_none()` holds exactly when this is empty.
    under: Chain<Segment>,
    frames: usize,
    calls: usize,
}

impl Stack {
    pub fn new() -> Stack {
        Stack::default()
    }

    /// Total pending frames. O(1), and what an opt-in resource ceiling on this
    /// engine's heap is checked against — never what a program's answer turns
    /// on. That is [`Stack::calls`].
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Pending calls — the [`Frame::Call`]s among [`Stack::frames`], counted the
    /// same way the tree-walker counts its own nesting. O(1), and it is the
    /// *semantic* bound: the one number both engines answer to, so that a
    /// runaway recursion is the same diagnostic whichever engine ran it.
    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn segments(&self) -> usize {
        self.under.len() + 1
    }

    pub fn is_empty(&self) -> bool {
        self.frames == 0 && self.under.is_empty()
    }

    pub fn push(&self, frame: Frame) -> Stack {
        self.clone().pushed(frame)
    }

    /// The owned form. A machine replacing its own stack has nothing to share
    /// with, so it neither clones the segments below nor drops the copy after.
    pub fn pushed(mut self, frame: Frame) -> Stack {
        let calls = is_call(&frame);
        self.top.frames = std::mem::take(&mut self.top.frames).push(frame);
        self.top.calls += calls;
        self.frames += 1;
        self.calls += calls;
        self
    }

    pub fn push_prompt(&self, prompt: Rc<Prompt>) -> Stack {
        self.push_delimiter(Delimiter::Ply(prompt))
    }

    /// Opens a segment under the seeded scheduler. One per task: a task's body
    /// returning is `Next::Leave` of its own delimiter, which is how the machine
    /// learns the task finished rather than the region.
    pub fn push_sim(&self, region: SimId) -> Stack {
        self.push_delimiter(Delimiter::Sim(region))
    }

    pub fn push_delimiter(&self, delimiter: Delimiter) -> Stack {
        let mut out = self.clone();
        let displaced = std::mem::replace(&mut out.top, Segment::below(delimiter));
        out.under = std::mem::take(&mut out.under).push(displaced);
        out
    }

    /// Whether this stack is inside `region` — that is, whether the region's
    /// delimiter is still one of the prompts control would have to leave.
    ///
    /// A second `simulate` reached while this holds is genuine nesting. One
    /// reached while it does not is a region whose control was discarded, which
    /// is a different mistake and gets a different diagnostic.
    pub fn holds_sim(&self, region: SimId) -> bool {
        self.segments_iter()
            .any(|s| matches!(s.delimiter(), Some(Delimiter::Sim(r)) if *r == region))
    }

    /// How many segments [`Stack::capture`] takes to cut out to and including
    /// the innermost region delimiter — what a task's own control is.
    ///
    /// The same count [`Stack::find_handler`] reports for a `Sim` target, and it
    /// is here so that a caller who already knows the perform belongs to the
    /// region — the host boundary, parking a task on a pending token — does not
    /// have to search by operation name to find out where to cut.
    pub fn sim_depth(&self) -> Option<usize> {
        self.segments_iter()
            .position(|s| matches!(s.delimiter(), Some(Delimiter::Sim(_))))
            .map(|depth| depth + 1)
    }

    /// The whole stack as one task's control, delimited by `region`.
    ///
    /// This is what a **lazily opened** production region needs and no other
    /// caller does. ADR 0011 §9 opens one at the first `task.*` that reaches the
    /// host binding, so the region has no body: its root task is the computation
    /// already in progress, which is this entire stack. [`Stack::capture`] cannot
    /// express that — it never crosses the base segment, because for every other
    /// caller the base is what the capture is being cut *out of*.
    ///
    /// The delimiter lands on the base segment rather than beside it, because a
    /// segment carrying no delimiter is where [`Stack::into_next`] answers
    /// `Done`: a region delimiter pushed under a delimiter-less base would never
    /// be reached and the root task's return would end the run instead of the
    /// task.
    pub fn into_task(mut self, region: SimId, born: u64) -> Continuation {
        let (frames, calls) = (self.frames, self.calls);
        let mut taken = Vec::with_capacity(self.segments());
        loop {
            match self.under.pop_front() {
                Some(below) => taken.push(std::mem::replace(&mut self.top, below)),
                None => {
                    self.top.delimiter = Some(Delimiter::Sim(region));
                    taken.push(self.top);
                    break;
                }
            }
        }
        Continuation {
            segments: Rc::new(taken),
            frames,
            calls,
            born,
            resumes: Rc::new(Cell::new(0)),
            pin: None,
        }
    }

    pub fn prompt(&self) -> Option<&Rc<Prompt>> {
        self.top.prompt()
    }

    pub fn next(&self) -> Next {
        self.clone().into_next()
    }

    /// The owned form, which is what the machine's return transition uses: the
    /// popped frame is moved out of its link rather than cloned whenever no
    /// captured continuation is still holding it.
    pub fn into_next(mut self) -> Next {
        if let Some(frame) = self.top.frames.pop_front() {
            let calls = is_call(&frame);
            self.top.calls -= calls;
            self.frames -= 1;
            self.calls -= calls;
            return Next::Frame(frame, self);
        }
        match self.top.delimiter.take() {
            Some(delimiter) => {
                self.top = self
                    .under
                    .pop_front()
                    .expect("only the base segment has no delimiter, and it is the outermost");
                Next::Leave(delimiter, self)
            }
            None => Next::Done,
        }
    }

    fn segments_iter(&self) -> impl Iterator<Item = &Segment> {
        std::iter::once(&self.top).chain(self.under.iter())
    }

    /// Innermost first, over both delimiter kinds. A `handle` nested inside a
    /// `simulate` region answers `clock.now()` itself; the same handler written
    /// outside the region does not, because the region's delimiter is crossed
    /// first. That is the ordinary shadowing rule and there is no case for it.
    pub fn find_handler(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<Handled> {
        for (depth, segment) in self.segments_iter().enumerate() {
            let Some(delimiter) = segment.delimiter() else {
                continue;
            };
            let target = match delimiter {
                Delimiter::Ply(prompt) => {
                    prompt
                        .clause_for(effect, op, resource)
                        .map(|clause| Target::Ply {
                            prompt: prompt.clone(),
                            clause,
                        })
                }
                Delimiter::Sim(region) => crate::sim::is_scheduled(effect.as_str(), op.as_str())
                    .then_some(Target::Sim(*region)),
            };
            if let Some(target) = target {
                return Some(Handled {
                    segments: depth + 1,
                    target,
                });
            }
        }
        None
    }

    /// Cuts the innermost `segments` segments away. The returned stack is what
    /// the handler clause runs on; the continuation is what resuming puts back.
    ///
    /// `born` is the machine's at-most-once host-operation count at the moment
    /// of capture, and it is a parameter rather than a default because a
    /// continuation that does not know when it was born cannot be told apart
    /// from one captured before the last packet went out. Zero is the
    /// conservative answer and the right one for every caller with no host
    /// binding.
    pub fn capture(&self, segments: usize, born: u64) -> (Continuation, Stack) {
        let mut taken = Vec::with_capacity(segments);
        let mut rest = self.clone();
        let mut frames = 0;
        let mut calls = 0;
        for _ in 0..segments {
            let below = rest
                .under
                .pop_front()
                .expect("capture never crosses the base segment");
            let cut = std::mem::replace(&mut rest.top, below);
            frames += cut.frames();
            calls += cut.calls();
            rest.frames -= cut.frames();
            rest.calls -= cut.calls();
            taken.push(cut);
        }
        (
            Continuation {
                segments: Rc::new(taken),
                frames,
                calls,
                born,
                resumes: Rc::new(Cell::new(0)),
                pin: None,
            },
            rest,
        )
    }

    /// Splices a captured continuation on top of this stack. Every resumption
    /// of one continuation splices the same shared segments, so the second
    /// costs what the first did.
    pub fn resume(&self, k: &Continuation) -> Stack {
        self.spliced(&k.segments)
    }

    /// `segments` are innermost first, the order [`Stack::capture`] produced.
    fn spliced(&self, segments: &[Segment]) -> Stack {
        let mut out = self.clone();
        for segment in segments.iter().rev() {
            let displaced = std::mem::replace(&mut out.top, segment.clone());
            out.under = std::mem::take(&mut out.under).push(displaced);
            out.frames += segment.frames();
            out.calls += segment.calls();
        }
        out
    }
}

/// A delimited continuation: the control captured at a `perform`, from the
/// perform site down to and including the handler that answered it.
///
/// It captures **control only**. The world is threaded through the machine
/// rather than snapshotted here, so a resumption observes the handler's state
/// as of the call to `resume`.
pub struct Continuation {
    /// Innermost first — the order `capture` produced and the reverse of the
    /// order `resume` pushes them back.
    segments: Rc<Vec<Segment>>,
    frames: usize,
    calls: usize,
    /// The machine's at-most-once host-operation count when this was captured.
    born: u64,
    /// Resumptions so far, **shared across clones**. A `Continuation` is cloned
    /// by `Rc` and handed to a scheduler, a frame and a clause binder alike; two
    /// clones are one continuation, and a per-clone counter would let a second
    /// resumption launder itself through a `let k2 = k`.
    resumes: Rc<Cell<u32>>,
    /// This continuation's claim on the regions that were open when it was
    /// captured, so their lexical close retains their slots instead of handing
    /// them back to a bump pointer this continuation can still read through —
    /// ADR 0005 required test 6. Dropped with the continuation, which is what
    /// makes the claim end when the continuation does.
    pin: Option<Pin>,
}

impl Continuation {
    /// Attaches the arena claim taken at this capture. Separate from
    /// [`Stack::capture`] because a stack has no arena: only the machine driving
    /// the capture holds one.
    pub fn pinned(mut self, pin: Option<Pin>) -> Continuation {
        self.pin = pin;
        self
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// [`Machine::host_ops`] when this continuation was captured.
    ///
    /// [`Machine::host_ops`]: crate::machine::Machine::host_ops
    pub fn born(&self) -> u64 {
        self.born
    }

    pub fn resumes(&self) -> u32 {
        self.resumes.get()
    }

    /// Counts this resumption and decides whether it may proceed, answering the
    /// ordinal of the refused resumption when it may not.
    ///
    /// `resumes > 1 && host_ops > born`, and both halves matter. A first
    /// resumption is always allowed — that is ordinary tail-resumptive control.
    /// A second is allowed whenever no at-most-once host operation has happened
    /// since the capture, which is *always* in a hermetic run, where `host_ops`
    /// is zero for the life of the entry point. So nothing multi-shot currently
    /// does changes under W1.
    ///
    /// It over-approximates deliberately: it refuses when such an operation
    /// happened anywhere after the capture — in another task, or in the clause
    /// rather than inside the continuation — even though replaying this
    /// particular control would repeat nothing. The precise rule needs a
    /// per-resumption liveness scope on the control stack, interacting with
    /// capture, splice, `Next::Leave` and task start, in the one part of the
    /// system where a defect is silent and sends a packet twice.
    ///
    /// It lives on the continuation, and [`crate::handler::resume`] is the only
    /// caller, so there is no way to splice a continuation back onto a stack
    /// without passing through it. The count saturates: a program that resumes
    /// one continuation four billion times is refused for other reasons long
    /// before, and wrapping to zero would silently re-open the boundary.
    pub(crate) fn admit(&self, host_ops: u64) -> Result<(), u32> {
        let resumes = self.resumes.get().saturating_add(1);
        self.resumes.set(resumes);
        if resumes > 1 && host_ops > self.born {
            return Err(resumes);
        }
        Ok(())
    }

    /// What splicing this back costs against the call budget: a resumption
    /// re-installs the calls the capture cut away.
    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn segments(&self) -> usize {
        self.segments.len()
    }

    /// The delimiters this continuation carries, innermost first. A task spawned
    /// under a `handle` written inside a `simulate` region has to run under that
    /// handler too, and this is where the machine reads which ones those are.
    pub fn delimiters(&self) -> Vec<Delimiter> {
        self.segments
            .iter()
            .filter_map(|s| s.delimiter.clone())
            .collect()
    }

    /// The region whose delimiter this continuation carries, if any. Innermost
    /// first, so a continuation cut out of a task names that task's own region.
    pub fn sim(&self) -> Option<SimId> {
        self.sim_at().map(|(id, _)| id)
    }

    /// The stack that would sit below this continuation's `Sim` delimiter once
    /// it is spliced onto `stack`.
    ///
    /// A region's anchor is the stack under its delimiter, and resuming a task
    /// from a clause body puts the delimiter somewhere new — over whatever that
    /// clause still had pending. Without moving the anchor the region delivers
    /// its value onto the stack it was *entered* on and the pending work is lost.
    pub fn under_sim(&self, stack: &Stack) -> Option<Stack> {
        let (_, at) = self.sim_at()?;
        Some(stack.spliced(&self.segments[at + 1..]))
    }

    fn sim_at(&self) -> Option<(SimId, usize)> {
        self.segments
            .iter()
            .enumerate()
            .find_map(|(i, s)| match s.delimiter {
                Some(Delimiter::Sim(id)) => Some((id, i)),
                _ => None,
            })
    }
}

impl Clone for Continuation {
    fn clone(&self) -> Continuation {
        Continuation {
            segments: Rc::clone(&self.segments),
            frames: self.frames,
            calls: self.calls,
            born: self.born,
            resumes: Rc::clone(&self.resumes),
            pin: self.pin.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_span::Span;

    fn frame(n: i64) -> Frame {
        Frame::FieldAccess {
            field: Ident::new(format!("f{n}"), Span::DUMMY),
            base_span: Span::DUMMY,
        }
    }

    fn field_of(f: &Frame) -> String {
        match f {
            Frame::FieldAccess { field, .. } => field.name.to_string(),
            _ => panic!("expected a field frame"),
        }
    }

    fn prompt() -> Rc<Prompt> {
        Rc::new(Prompt {
            clauses: Rc::new(Vec::new()),
            effects: Rc::new(Vec::new()),
            ret: None,
            env: Env::empty(),
            module: 0,
            span: Span::DUMMY,
        })
    }

    #[test]
    fn a_new_stack_is_done_immediately() {
        assert!(matches!(Stack::new().next(), Next::Done));
        assert!(Stack::new().is_empty());
    }

    #[test]
    fn frames_come_back_innermost_first() {
        let s = Stack::new().push(frame(1)).push(frame(2));
        assert_eq!(s.frames(), 2);
        let Next::Frame(top, rest) = s.next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&top), "f2");
        let Next::Frame(under, rest) = rest.next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&under), "f1");
        assert!(matches!(rest.next(), Next::Done));
    }

    #[test]
    fn popping_a_frame_leaves_the_original_stack_intact() {
        let s = Stack::new().push(frame(1));
        let _ = s.next();
        assert_eq!(s.frames(), 1);
    }

    #[test]
    fn an_exhausted_segment_yields_its_prompt_and_then_the_stack_under_it() {
        let s = Stack::new().push(frame(1)).push_prompt(prompt());
        let Next::Leave(_, under) = s.next() else {
            panic!("expected to leave the segment");
        };
        assert_eq!(under.segments(), 1);
        assert_eq!(under.frames(), 1);
    }

    #[test]
    fn capture_takes_the_segments_above_and_including_the_handler() {
        let s = Stack::new()
            .push(frame(0))
            .push_prompt(prompt())
            .push(frame(1))
            .push(frame(2));
        assert_eq!(s.segments(), 2);

        let (k, below) = s.capture(1, 0);
        assert_eq!(k.frames(), 2);
        assert_eq!(k.segments(), 1);
        assert_eq!(below.segments(), 1);
        assert_eq!(below.frames(), 1);
    }

    #[test]
    fn resuming_reinstalls_the_handler_that_delimited_the_capture() {
        let s = Stack::new().push_prompt(prompt()).push(frame(1));
        let (k, below) = s.capture(1, 0);
        assert!(below.prompt().is_none());

        let resumed = below.resume(&k);
        assert!(resumed.prompt().is_some());
        assert_eq!(resumed.frames(), 1);
    }

    #[test]
    fn a_continuation_may_be_resumed_twice_onto_different_stacks() {
        let s = Stack::new().push_prompt(prompt()).push(frame(9));
        let (k, below) = s.capture(1, 0);

        let once = below.resume(&k);
        let twice = below.push(frame(5)).resume(&k);

        assert_eq!(once.frames(), 1);
        assert_eq!(twice.frames(), 2);

        let Next::Frame(a, _) = once.next() else {
            panic!("expected a frame");
        };
        let Next::Frame(b, _) = twice.next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&a), "f9");
        assert_eq!(field_of(&b), "f9");
    }

    /// `into_next` moves the frame out of its link when nothing else holds it,
    /// so a captured segment has to be the thing that stops it.
    #[test]
    fn popping_a_captured_frame_leaves_the_continuation_able_to_splice_it_again() {
        let s = Stack::new()
            .push_prompt(prompt())
            .push(frame(1))
            .push(frame(2));
        let (k, below) = s.capture(1, 0);

        let Next::Frame(first, rest) = below.resume(&k).into_next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&first), "f2");
        let Next::Frame(second, _) = rest.into_next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&second), "f1");

        assert_eq!(k.frames(), 2);
        let again = below.resume(&k);
        assert_eq!(again.frames(), 2);
        let Next::Frame(replayed, _) = again.into_next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&replayed), "f2");
    }

    /// A stack may hold as many frames as the calls under `DEFAULT_MAX_CALLS`
    /// can pend, which no constant caps, so releasing one has to be a loop. A
    /// recursive drop through the links would abort the process on a worker's
    /// stack rather than raise a diagnostic.
    #[test]
    fn dropping_a_deep_stack_does_not_recurse_through_the_native_stack() {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let mut s = Stack::new();
                for i in 0..200_000 {
                    s = s.pushed(frame(i));
                }
                assert_eq!(s.frames(), 200_000);
                drop(s);
            })
            .expect("failed to spawn")
            .join()
            .expect("dropping the stack overflowed the thread stack");
    }

    #[test]
    fn capture_crosses_every_handler_between_the_perform_and_its_own() {
        let s = Stack::new()
            .push_prompt(prompt())
            .push(frame(1))
            .push_prompt(prompt())
            .push(frame(2))
            .push_prompt(prompt())
            .push(frame(3));

        let (k, below) = s.capture(3, 0);
        assert_eq!(k.segments(), 3);
        assert_eq!(k.frames(), 3);
        assert_eq!(below.segments(), 1);
        assert_eq!(below.frames(), 0);

        assert_eq!(below.resume(&k).segments(), 4);
    }

    #[test]
    fn find_handler_reports_the_innermost_matching_prompt() {
        let effect = Symbol::new("db");
        let op = Symbol::new("get");
        let clause = |resource: Option<&str>| Clause {
            effect: ply_syntax::ast::QName::bare(Ident::new("db", Span::DUMMY)),
            op: op.clone(),
            resource: resource.map(Symbol::new),
            params: Rc::new(Vec::new()),
            resume: None,
            body: crate::code::lower(&crate::build::int(0)),
            span: Span::DUMMY,
        };
        let with = |c: Clause| {
            Rc::new(Prompt {
                clauses: Rc::new(vec![c]),
                effects: Rc::new(vec![effect.clone()]),
                ret: None,
                env: Env::empty(),
                module: 0,
                span: Span::DUMMY,
            })
        };

        let s = Stack::new()
            .push_prompt(with(clause(Some("users"))))
            .push_prompt(with(clause(Some("orders"))));

        let users = Symbol::new("users");
        let found = s
            .find_handler(&effect, &op, Some(&users))
            .expect("the outer handler matches");
        assert_eq!(found.segments, 2);
        assert!(matches!(found.target, Target::Ply { clause: 0, .. }));

        let orders = Symbol::new("orders");
        let inner = s
            .find_handler(&effect, &op, Some(&orders))
            .expect("the inner handler matches");
        assert_eq!(inner.segments, 1);
    }

    #[test]
    fn an_unhandled_operation_finds_no_prompt() {
        let s = Stack::new().push_prompt(prompt());
        assert!(
            s.find_handler(&Symbol::new("db"), &Symbol::new("get"), None)
                .is_none()
        );
    }

    /// A `simulate` region's delimiter answers the three simulated effects and
    /// nothing else, and a `handle` nested inside one still shadows it.
    #[test]
    fn a_sim_delimiter_answers_the_scheduled_operations_only() {
        let s = Stack::new().push_sim(SimId(0));
        let now = Symbol::new("now");
        assert!(matches!(
            s.find_handler(&Symbol::new("clock"), &now, None)
                .expect("the region handles `clock.now`")
                .target,
            Target::Sim(SimId(0))
        ));
        assert!(
            s.find_handler(&Symbol::new("db"), &Symbol::new("get"), None)
                .is_none(),
            "a region must not claim an effect the language has never heard of"
        );

        let clause = Clause {
            effect: ply_syntax::ast::QName::bare(Ident::new("clock", Span::DUMMY)),
            op: now.clone(),
            resource: None,
            params: Rc::new(Vec::new()),
            resume: None,
            body: crate::code::lower(&crate::build::int(0)),
            span: Span::DUMMY,
        };
        let inner = s.push_prompt(Rc::new(Prompt {
            clauses: Rc::new(vec![clause]),
            effects: Rc::new(vec![Symbol::new("clock")]),
            ret: None,
            env: Env::empty(),
            module: 0,
            span: Span::DUMMY,
        }));
        assert!(matches!(
            inner
                .find_handler(&Symbol::new("clock"), &now, None)
                .expect("the nested handler matches")
                .target,
            Target::Ply { .. }
        ));
    }
}
