//! The explicit control stack, and the delimited continuations cut out of it.
//!
//! Since ADR 0034 a frame holds no scope: the machine owns one slot stack and every frame records
//! only the *relative* quantities needed to undo its window effect — sizes and offsets from the
//! top, never an absolute index. That is what lets a captured extent splice back onto any stack at
//! any height without a single frame being rewritten, which multi-shot resumption requires of a
//! structure shared by `Rc`.

use crate::arena::{Pin, RegionId};
use crate::code::{Clause, Code, ReturnArm, Stmt};
use crate::pool::{self, Free, Link, Pooled};
use crate::value::{List, Value};
use crate::window::SlotVal;
use ply_span::{Span, Symbol};
use ply_syntax::ast::{BinOp, Ident, UnOp};
use std::cell::Cell;
use std::rc::Rc;

/// One suspended step.
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
        module: usize,
        rhs_span: Span,
    },

    AppCallee {
        args: Rc<Vec<Code>>,
        module: usize,
        span: Span,
    },

    /// Waiting for `args[next - 1]`, holding the callee and the arguments already evaluated.
    AppArgs {
        callee: Value,
        done: Vec<Value>,
        args: Rc<Vec<Code>>,
        next: usize,
        module: usize,
        span: Span,
    },

    /// A user function's body is running.
    Call {
        name: Option<Symbol>,
        call_site: Span,
        /// This call is the one evaluating a nullary pure definition for the first time, so the
        /// value it receives is that definition's constant.
        memo: bool,
        /// The callee's window size, truncated away when the call returns.
        callee_window: u32,
        /// The caller's window size, which is what re-derives its base from the top.
        caller_window: u32,
    },

    /// A window boundary that is not a call: a handler clause's body, or a `return` arm. Undoes
    /// its window exactly as [`Frame::Call`] does, without counting against the call budget.
    Exit {
        callee_window: u32,
        caller_window: u32,
    },

    /// Hands the value it receives to a captured continuation.
    Resume {
        k: Rc<Continuation>,
    },

    /// The frame a resumption pushes under the segments it splices: when the value passes back
    /// down out of the restored extent, drop the extent's windows and restore the base the
    /// resumer had. Relative on both counts, so a later capture can carry it anywhere.
    Restore {
        /// What is left of the restored extent's windows when this dispatches: the window of the
        /// activation that pushed the captured prompt.
        spill: u32,
        /// The resuming activation's window size.
        base_offset: u32,
    },

    If {
        then_branch: Code,
        else_branch: Code,
        module: usize,
        cond_span: Span,
    },

    /// Waiting for the scrutinee.
    MatchArms {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        next: usize,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for an arm's guard. The arm's pattern bindings are already in their slots, and a
    /// failing guard falls through to the next arm, whose slots are its own.
    MatchGuard {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        at: usize,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for `stmts[next - 1]`.
    BlockStep {
        stmts: Rc<Vec<Stmt>>,
        next: usize,
        tail: Option<Code>,
        module: usize,
    },

    RecordField {
        done: Vec<(Symbol, Value)>,
        fields: Rc<Vec<(Symbol, Code)>>,
        next: usize,
        module: usize,
    },

    /// A record update's written fields, waiting for `sets[next - 1]`; the base comes last. The
    /// values go into a pooled vector — the names are `sets`' — so an update allocates nothing
    /// of its own.
    UpdateField {
        base: Code,
        copies: Rc<Vec<Ident>>,
        sets: Rc<Vec<(Symbol, Code)>>,
        done: Vec<Value>,
        next: usize,
        module: usize,
        span: Span,
    },

    /// Waiting for a record update's base, with the written fields already evaluated.
    UpdateApply {
        copies: Rc<Vec<Ident>>,
        sets: Rc<Vec<(Symbol, Code)>>,
        done: Vec<Value>,
        span: Span,
    },

    FieldAccess {
        field: Ident,
        base_span: Span,
    },

    ListItem {
        done: Vec<Value>,
        items: Rc<Vec<Code>>,
        next: usize,
        module: usize,
    },

    /// Waiting for `args[next - 1]` of a `perform`.
    PerformArgs {
        effect: Symbol,
        op: Symbol,
        resource: Option<Symbol>,
        done: Vec<Value>,
        args: Rc<Vec<Code>>,
        next: usize,
        module: usize,
        span: Span,
    },

    /// The cell is allocated only once the initial value lands.
    WithCellBody {
        resource: Symbol,
        binder: Symbol,
        /// The binder's slot in the enclosing activation.
        slot: Option<u32>,
        body: Code,
        module: usize,
        /// The whole `with_cell` expression, which is the key [`crate::region_kind`] filed its
        /// decision about this region under.
        region: Span,
    },

    /// A region's lexical close.
    CloseRegion {
        region: RegionId,
    },

    /// `map`, `filter` and `fold` call user code, so their loops are frames rather than host
    /// recursion — otherwise a continuation captured inside the function passed to `map` would be
    /// captured across a native frame that cannot be re-entered.
    MapStep {
        f: Value,
        items: List,
        next: usize,
        done: Vec<Value>,
        span: Span,
    },

    FilterStep {
        f: Value,
        items: List,
        next: usize,
        done: Vec<Value>,
        span: Span,
    },

    FoldStep {
        f: Value,
        items: List,
        next: usize,
        span: Span,
    },

    /// `map_fold`'s loop.
    MapFoldStep {
        f: Value,
        entries: crate::map::Entries,
        next: usize,
        span: Span,
    },

    /// `bytes_position`'s loop.
    BytesPositionStep {
        f: Value,
        bytes: std::sync::Arc<[u8]>,
        next: usize,
        span: Span,
    },

    /// `iterate`'s loop.
    IterateStep {
        f: Value,
        budget: i64,
        left: i64,
        span: Span,
    },

    /// `cell_update` is waiting for its function's answer, which goes back into the cell.
    CellUpdateStep {
        slot: crate::arena::Slot,
        span: Span,
    },

    /// `map_update` is waiting for its function's answer, which goes back under the key.
    MapUpdateStep {
        map: Value,
        key: Value,
        span: Span,
    },
}

/// How many slots sit between the state just below this pending frame and the state just above
/// it — what a capture walks to find the height at a delimiter's push.
fn frame_delta(frame: &Frame) -> usize {
    match frame {
        Frame::Call { callee_window, .. } | Frame::Exit { callee_window, .. } => {
            *callee_window as usize
        }
        Frame::Restore { spill, .. } => *spill as usize,
        _ => 0,
    }
}

pub struct Prompt {
    pub clauses: Rc<Vec<Clause>>,
    /// Each clause's effect under its program-wide name, resolved where the `handle` was written.
    pub effects: Rc<Vec<Symbol>>,
    pub ret: Option<Rc<ReturnArm>>,
    /// Per clause, the values its body's free variables were bound to where the handler was
    /// installed — copied at handle entry, written into the clause's window at each perform.
    pub clause_captures: Vec<Rc<[Value]>>,
    /// The same, for the `return` arm.
    pub ret_captures: Rc<[Value]>,
    pub module: usize,
    pub span: Span,
}

impl Prompt {
    /// The index of the clause handling this operation, innermost clause order.
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

/// Which simulated region a [`Delimiter::Sim`] belongs to: its ordinal among the regions one entry
/// point has entered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SimId(pub u32);

/// What delimits a segment.
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
    /// The seeded scheduler: a `task.*`, `clock.*` or `random.*` perform that reached a `simulate`
    /// region's delimiter before any `handle` that names it.
    Sim(SimId),
}

/// A persistent stack, shared by pointer.
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
    /// Moves the head out when this chain is its only owner, which is every pop no captured
    /// continuation is sharing.
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

/// Iterative, because nothing bounds the frames pending on a stack at a depth the native stack
/// could survive unwinding recursively.
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
    /// The window size of the activation that pushed this segment's delimiter — what a capture
    /// subtracts to find the floor its snapshot starts at. A size, not a position, so a spliced
    /// segment needs no rebasing.
    window: u32,
    /// The summed [`frame_delta`] of this segment's pending frames, kept incrementally so a
    /// capture reads its slot height in O(segments) rather than walking every frame.
    deltas: usize,
}

impl Segment {
    pub fn base() -> Segment {
        Segment::default()
    }

    pub fn under(prompt: Rc<Prompt>) -> Segment {
        Segment::below(Delimiter::Ply(prompt), 0)
    }

    pub fn below(delimiter: Delimiter, window: u32) -> Segment {
        Segment {
            frames: Chain::new(),
            delimiter: Some(delimiter),
            calls: 0,
            window,
            deltas: 0,
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
    /// The delimited body finished.
    Leave(Delimiter, Stack),
    Done,
}

pub struct Handled {
    /// How many segments to capture, counting from the innermost.
    pub segments: usize,
    pub target: Target,
}

#[derive(Clone, Default)]
pub struct Stack {
    /// The innermost segment, held by value rather than as the head of `under`.
    top: Segment,
    /// The segments below `top`, head first.
    under: Chain<Segment>,
    frames: usize,
    calls: usize,
}

impl Stack {
    pub fn new() -> Stack {
        Stack::default()
    }

    /// Total pending frames.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Pending calls — the [`Frame::Call`]s among [`Stack::frames`], counted the same way a
    /// recursive evaluator counts its own nesting.
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

    /// The owned form.
    pub fn pushed(mut self, frame: Frame) -> Stack {
        let calls = is_call(&frame);
        self.top.deltas += frame_delta(&frame);
        self.top.frames = std::mem::take(&mut self.top.frames).push(frame);
        self.top.calls += calls;
        self.frames += 1;
        self.calls += calls;
        self
    }

    /// Opens a prompt's segment. `window` is the pushing activation's window size, which a
    /// capture at this prompt subtracts to find its snapshot's floor.
    pub fn push_prompt(&self, prompt: Rc<Prompt>, window: u32) -> Stack {
        self.push_delimiter(Delimiter::Ply(prompt), window)
    }

    /// Opens a segment under a scheduler's delimiter. A task's control never reaches below the
    /// region's entry height, so the window is zero and a capture snapshots the task's own slots
    /// and nothing else.
    pub fn push_sim(&self, region: SimId) -> Stack {
        self.push_delimiter(Delimiter::Sim(region), 0)
    }

    pub fn push_delimiter(&self, delimiter: Delimiter, window: u32) -> Stack {
        let mut out = self.clone();
        let displaced = std::mem::replace(&mut out.top, Segment::below(delimiter, window));
        out.under = std::mem::take(&mut out.under).push(displaced);
        out
    }

    /// Whether this stack is inside `region` — that is, whether the region's delimiter is still one
    /// of the prompts control would have to leave.
    pub fn holds_sim(&self, region: SimId) -> bool {
        self.segments_iter()
            .any(|s| matches!(s.delimiter(), Some(Delimiter::Sim(r)) if *r == region))
    }

    /// How many segments [`Stack::capture`] takes to cut out to and including the innermost region
    /// delimiter — what a task's own control is.
    pub fn sim_depth(&self) -> Option<usize> {
        self.segments_iter()
            .position(|s| matches!(s.delimiter(), Some(Delimiter::Sim(_))))
            .map(|depth| depth + 1)
    }

    /// The whole stack as one task's control, delimited by `region`. The caller owns the slot
    /// stack and seals the extent onto the continuation itself.
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
            extent: Extent::InPlace,
            base_offset: 0,
            cut_deltas: 0,
            cut_window: 0,
        }
    }

    pub fn prompt(&self) -> Option<&Rc<Prompt>> {
        self.top.prompt()
    }

    pub fn next(&self) -> Next {
        self.clone().into_next()
    }

    /// The owned form, which is what the machine's return transition uses: the popped frame is
    /// moved out of its link rather than cloned whenever no captured continuation is still holding
    /// it.
    pub fn into_next(mut self) -> Next {
        if let Some(frame) = self.top.frames.pop_front() {
            let calls = is_call(&frame);
            self.top.deltas -= frame_delta(&frame);
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

    /// Innermost first, over both delimiter kinds.
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

    /// Cuts the innermost `segments` segments away, computing on the way the two relative
    /// quantities the caller needs to seal the extent's windows: how many slots sit above the
    /// outermost cut delimiter's push height, and that delimiter's activation window.
    pub fn capture(&self, segments: usize, born: u64) -> (Continuation, Stack) {
        let mut taken = Vec::with_capacity(segments);
        let mut rest = self.clone();
        let mut frames = 0;
        let mut calls = 0;
        let mut cut_deltas = 0usize;
        let mut cut_window = 0u32;
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
            cut_deltas += cut.deltas;
            cut_window = cut.window;
            taken.push(cut);
        }
        crate::rc::census4::capture(frames as u64);
        (
            Continuation {
                segments: Rc::new(taken),
                frames,
                calls,
                born,
                resumes: Rc::new(Cell::new(0)),
                pin: None,
                extent: Extent::InPlace,
                base_offset: 0,
                cut_deltas,
                cut_window,
            },
            rest,
        )
    }

    /// Splices a captured continuation on top of this stack.
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

/// What a continuation carries of the slot stack.
#[derive(Clone)]
pub enum Extent {
    /// Nothing: the slots are still in place on the machine's stack, and the splice that consumes
    /// this continuation happens before anything below can touch them. The tail-resumptive path —
    /// every plain perform — so a capture there costs no slot traffic at all.
    InPlace,
    /// A snapshot from the floor of the capturing prompt's activation to the top at capture,
    /// restored — cloned — once per resumption: two futures need two copies, and the clone is a
    /// refcount bump per slot rather than a deep copy.
    Saved { slots: Rc<Vec<SlotVal>> },
}

/// A delimited continuation: the control captured at a `perform`, from the perform site down to and
/// including the handler that answered it.
pub struct Continuation {
    /// Innermost first — the order `capture` produced and the reverse of the order `resume` pushes
    /// them back.
    segments: Rc<Vec<Segment>>,
    frames: usize,
    calls: usize,
    /// The machine's at-most-once host-operation count when this was captured.
    born: u64,
    /// Resumptions so far, **shared across clones**.
    resumes: Rc<Cell<u32>>,
    /// This continuation's claim on the regions that were open when it was captured, so their
    /// lexical close retains their slots instead of handing them back to a bump pointer this
    /// continuation can still read through: the escape case, where a continuation is
    /// resumed after the region that made its cell returned.
    pin: Option<Pin>,
    /// The captured windows, or nothing for a tail-resumptive capture.
    extent: Extent,
    /// The innermost captured activation's base, as an offset back from the extent's top.
    base_offset: u32,
    /// Slots above the outermost cut delimiter's push height, summed from the cut frames'
    /// relative records.
    cut_deltas: usize,
    /// The outermost cut segment's activation window — the part of the snapshot shared with the
    /// activation continuing below the capture.
    cut_window: u32,
}

impl Continuation {
    /// Attaches the arena claim taken at this capture.
    pub fn pinned(mut self, pin: Option<Pin>) -> Continuation {
        self.pin = pin;
        self
    }

    /// Seals the captured windows and the base offset onto this continuation.
    pub fn with_extent(mut self, extent: Extent, base_offset: u32) -> Continuation {
        self.extent = extent;
        self.base_offset = base_offset;
        self
    }

    pub fn extent(&self) -> &Extent {
        &self.extent
    }

    pub fn base_offset(&self) -> usize {
        self.base_offset as usize
    }

    /// Slots above the outermost cut delimiter's push height at capture.
    pub fn cut_deltas(&self) -> usize {
        self.cut_deltas
    }

    /// The window of the activation that pushed the captured prompt.
    pub fn cut_window(&self) -> usize {
        self.cut_window as usize
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// [`crate::machine::Machine::host_ops`] when this continuation was captured.
    pub fn born(&self) -> u64 {
        self.born
    }

    pub fn resumes(&self) -> u32 {
        self.resumes.get()
    }

    /// Counts this resumption and decides whether it may proceed, answering the ordinal of the
    /// refused resumption when it may not.
    pub(crate) fn admit(&self, host_ops: u64) -> Result<(), u32> {
        let resumes = self.resumes.get().saturating_add(1);
        self.resumes.set(resumes);
        if resumes > 1 && host_ops > self.born {
            return Err(resumes);
        }
        Ok(())
    }

    /// What splicing this back costs against the call budget: a resumption re-installs the calls
    /// the capture cut away.
    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn segments(&self) -> usize {
        self.segments.len()
    }

    /// The delimiters this continuation carries, innermost first.
    pub fn delimiters(&self) -> Vec<Delimiter> {
        self.segments
            .iter()
            .filter_map(|s| s.delimiter.clone())
            .collect()
    }

    /// The region whose delimiter this continuation carries, if any.
    pub fn sim(&self) -> Option<SimId> {
        self.sim_at().map(|(id, _)| id)
    }

    /// The stack that would sit below this continuation's `Sim` delimiter once it is spliced onto
    /// `stack`.
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

    /// Slots above this continuation's `Sim` delimiter's push height, summed from the frames of
    /// the segments at and inside it — what a resumer subtracts from the extent's length to find
    /// where the delimiter lands after a splice.
    pub fn deltas_through_sim(&self) -> Option<usize> {
        let (_, at) = self.sim_at()?;
        Some(self.segments[..=at].iter().map(|s| s.deltas).sum())
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
            extent: self.extent.clone(),
            base_offset: self.base_offset,
            cut_deltas: self.cut_deltas,
            cut_window: self.cut_window,
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
            clause_captures: Vec::new(),
            ret_captures: Rc::from(Vec::new()),
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
        let s = Stack::new().push(frame(1)).push_prompt(prompt(), 0);
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
            .push_prompt(prompt(), 0)
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
        let s = Stack::new().push_prompt(prompt(), 0).push(frame(1));
        let (k, below) = s.capture(1, 0);
        assert!(below.prompt().is_none());

        let resumed = below.resume(&k);
        assert!(resumed.prompt().is_some());
        assert_eq!(resumed.frames(), 1);
    }

    #[test]
    fn a_continuation_may_be_resumed_twice_onto_different_stacks() {
        let s = Stack::new().push_prompt(prompt(), 0).push(frame(9));
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

    /// `into_next` moves the frame out of its link when nothing else holds it, so a captured
    /// segment has to be the thing that stops it.
    #[test]
    fn popping_a_captured_frame_leaves_the_continuation_able_to_splice_it_again() {
        let s = Stack::new()
            .push_prompt(prompt(), 0)
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

    /// The relative bookkeeping a capture reads off the cut: window-bearing frames say how many
    /// slots sit above the captured prompt's push height, and the segment says how many below it
    /// belong to the pushing activation.
    #[test]
    fn a_capture_reads_its_slot_metrics_off_the_frames_it_cuts() {
        let s = Stack::new()
            .push_prompt(prompt(), 3)
            .push(frame(1))
            .push(Frame::Call {
                name: None,
                call_site: Span::DUMMY,
                memo: false,
                callee_window: 4,
                caller_window: 3,
            })
            .push(Frame::Exit {
                callee_window: 2,
                caller_window: 4,
            });
        let (k, _) = s.capture(1, 0);
        assert_eq!(k.cut_deltas(), 6, "4 from the call, 2 from the exit");
        assert_eq!(k.cut_window(), 3, "the prompt was pushed with window 3");
    }

    /// A stack may hold as many frames as the calls under `DEFAULT_MAX_CALLS` can pend, which no
    /// constant caps, so releasing one has to be a loop.
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
            .push_prompt(prompt(), 0)
            .push(frame(1))
            .push_prompt(prompt(), 0)
            .push(frame(2))
            .push_prompt(prompt(), 0)
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
            body: crate::code::lower(&crate::build::int(0)).code,
            size: 0,
            captures: crate::code::no_captures(),
            span: Span::DUMMY,
        };
        let with = |c: Clause| {
            Rc::new(Prompt {
                clauses: Rc::new(vec![c]),
                effects: Rc::new(vec![effect.clone()]),
                ret: None,
                clause_captures: vec![Rc::from(Vec::new())],
                ret_captures: Rc::from(Vec::new()),
                module: 0,
                span: Span::DUMMY,
            })
        };

        let s = Stack::new()
            .push_prompt(with(clause(Some("users"))), 0)
            .push_prompt(with(clause(Some("orders"))), 0);

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
        let s = Stack::new().push_prompt(prompt(), 0);
        assert!(
            s.find_handler(&Symbol::new("db"), &Symbol::new("get"), None)
                .is_none()
        );
    }

    /// A `simulate` region's delimiter answers the three simulated effects and nothing else, and a
    /// `handle` nested inside one still shadows it.
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
            body: crate::code::lower(&crate::build::int(0)).code,
            size: 0,
            captures: crate::code::no_captures(),
            span: Span::DUMMY,
        };
        let inner = s.push_prompt(
            Rc::new(Prompt {
                clauses: Rc::new(vec![clause]),
                effects: Rc::new(vec![Symbol::new("clock")]),
                ret: None,
                clause_captures: vec![Rc::from(Vec::new())],
                ret_captures: Rc::from(Vec::new()),
                module: 0,
                span: Span::DUMMY,
            }),
            0,
        );
        assert!(matches!(
            inner
                .find_handler(&Symbol::new("clock"), &now, None)
                .expect("the nested handler matches")
                .target,
            Target::Ply { .. }
        ));
    }
}
