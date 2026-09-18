//! The explicit control stack, and the delimited continuations cut out of it.
//! Frames record only relative sizes, so a captured extent splices back at any stack height.

use crate::arena::{Pin, RegionId};
use crate::code::{Clause, Code, ReturnArm, Stmt};
use crate::pool::{self, Free, Link, Pooled};
use crate::value::{List, Value};
use crate::window::SlotVal;
use ply_span::{Span, Symbol};
use ply_syntax::ast::Ident;
use ply_ty::{BinOp, UnOp};
use std::cell::Cell;
use std::rc::Rc;

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

    BinaryApply {
        op: BinOp,
        lhs: Value,
        lhs_span: Span,
        rhs_span: Span,
        span: Span,
    },

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

    Call {
        name: Option<Symbol>,
        call_site: Span,
        /// First evaluation of a nullary pure definition; the answer is its constant.
        memo: bool,
        /// The callee's window size, truncated away when the call returns.
        callee_window: u32,
        /// The caller's window size, which re-derives its base from the top.
        caller_window: u32,
    },

    /// A non-call window boundary (handler clause body or `return` arm), off the call budget.
    Exit {
        callee_window: u32,
        caller_window: u32,
    },

    Resume {
        k: Rc<Continuation>,
    },

    /// Pushed under a resumption's segments; drops the extent's windows and restores the base.
    Restore {
        /// The window of the activation that pushed the captured prompt.
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

    MatchArms {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        next: usize,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for an arm's guard; the arm's bindings are already in their slots.
    MatchGuard {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        at: usize,
        module: usize,
        scrutinee_span: Span,
    },

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

    /// Waiting for `sets[next - 1]` of a record update; the base comes last.
    UpdateField {
        base: Code,
        copies: Rc<Vec<Ident>>,
        sets: Rc<Vec<(Symbol, Code)>>,
        done: Vec<Value>,
        next: usize,
        module: usize,
        span: Span,
    },

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
        slot: Option<u32>,
        body: Code,
        module: usize,
        /// The `with_cell` expression, the key [`crate::region_kind`] files this region under.
        region: Span,
    },

    CloseRegion {
        region: RegionId,
    },

    /// Loops are frames, not host recursion, so a capture inside `f` spans no native frame.
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

    MapFoldStep {
        f: Value,
        entries: crate::map::Entries,
        next: usize,
        span: Span,
    },

    BytesPositionStep {
        f: Value,
        bytes: std::sync::Arc<[u8]>,
        next: usize,
        span: Span,
    },

    IterateStep {
        f: Value,
        budget: i64,
        left: i64,
        span: Span,
    },

    CellUpdateStep {
        slot: crate::arena::Slot,
        span: Span,
    },

    MapUpdateStep {
        map: Value,
        key: Value,
        span: Span,
    },
}

/// Slots between the state below this pending frame and the state above it.
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
    /// Per clause, its free variables' values at handler install, written in at each perform.
    pub clause_captures: Vec<Rc<[Value]>>,
    /// The same, for the `return` arm.
    pub ret_captures: Rc<[Value]>,
    pub module: usize,
    pub span: Span,
}

impl Prompt {
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

/// A [`Delimiter::Sim`]'s region: its ordinal among the regions one entry point has entered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SimId(pub u32);

#[derive(Clone)]
pub enum Delimiter {
    Ply(Rc<Prompt>),
    Sim(SimId),
}

pub enum Target {
    Ply {
        prompt: Rc<Prompt>,
        clause: usize,
    },
    /// A scheduled perform that reached a `simulate` delimiter before any `handle` naming it.
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
    /// Moves the head out when this chain is its only owner.
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

/// Iterative: pending frames can be deeper than the native stack could unwind recursively.
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
    /// The delimiter-pushing activation's window size; a size, so splices need no rebasing.
    window: u32,
    /// The summed [`frame_delta`] of pending frames, kept so a capture need not walk them.
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

pub enum Next {
    Frame(Frame, Stack),
    Leave(Delimiter, Stack),
    Done,
}

pub struct Handled {
    pub segments: usize,
    pub target: Target,
}

#[derive(Clone, Default)]
pub struct Stack {
    /// The innermost segment, held by value rather than as the head of `under`.
    top: Segment,
    under: Chain<Segment>,
    frames: usize,
    calls: usize,
}

impl Stack {
    pub fn new() -> Stack {
        Stack::default()
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

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

    pub fn pushed(mut self, frame: Frame) -> Stack {
        let calls = is_call(&frame);
        self.top.deltas += frame_delta(&frame);
        self.top.frames = std::mem::take(&mut self.top.frames).push(frame);
        self.top.calls += calls;
        self.frames += 1;
        self.calls += calls;
        self
    }

    /// Opens a prompt's segment; `window` is the pushing activation's window size.
    pub fn push_prompt(&self, prompt: Rc<Prompt>, window: u32) -> Stack {
        self.push_delimiter(Delimiter::Ply(prompt), window)
    }

    /// Opens a scheduler's segment at window zero: a task never reaches below its region.
    pub fn push_sim(&self, region: SimId) -> Stack {
        self.push_delimiter(Delimiter::Sim(region), 0)
    }

    pub fn push_delimiter(&self, delimiter: Delimiter, window: u32) -> Stack {
        let mut out = self.clone();
        let displaced = std::mem::replace(&mut out.top, Segment::below(delimiter, window));
        out.under = std::mem::take(&mut out.under).push(displaced);
        out
    }

    pub fn holds_sim(&self, region: SimId) -> bool {
        self.segments_iter()
            .any(|s| matches!(s.delimiter(), Some(Delimiter::Sim(r)) if *r == region))
    }

    /// Segments [`Stack::capture`] takes to cut through the innermost region delimiter.
    pub fn sim_depth(&self) -> Option<usize> {
        self.segments_iter()
            .position(|s| matches!(s.delimiter(), Some(Delimiter::Sim(_))))
            .map(|depth| depth + 1)
    }

    /// The whole stack as one task's control; the caller seals the slot extent on.
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

    /// Cuts away the innermost `segments`, totalling what the caller needs to seal the extent.
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

#[derive(Clone)]
pub enum Extent {
    /// Slots stay in place: the consuming splice runs before anything can touch them.
    InPlace,
    /// Slots from the capturing prompt's activation floor, cloned once per resumption.
    Saved { slots: Rc<Vec<SlotVal>> },
}

/// The control captured at a `perform`, down to and including the handler that answered it.
pub struct Continuation {
    /// Innermost first.
    segments: Rc<Vec<Segment>>,
    frames: usize,
    calls: usize,
    /// The machine's at-most-once host-operation count when this was captured.
    born: u64,
    resumes: Rc<Cell<u32>>,
    /// Keeps regions open at capture from recycling slots this continuation can still read.
    pin: Option<Pin>,
    /// The captured windows, or nothing for a tail-resumptive capture.
    extent: Extent,
    /// The innermost captured activation's base, as an offset back from the extent's top.
    base_offset: u32,
    /// Slots above the outermost cut delimiter's push height.
    cut_deltas: usize,
    /// The outermost cut segment's activation window, shared with the activation below.
    cut_window: u32,
}

impl Continuation {
    pub fn pinned(mut self, pin: Option<Pin>) -> Continuation {
        self.pin = pin;
        self
    }

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

    pub fn cut_deltas(&self) -> usize {
        self.cut_deltas
    }

    pub fn cut_window(&self) -> usize {
        self.cut_window as usize
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn born(&self) -> u64 {
        self.born
    }

    pub fn resumes(&self) -> u32 {
        self.resumes.get()
    }

    /// The calls a resumption re-installs against the call budget.
    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn segments(&self) -> usize {
        self.segments.len()
    }

    pub fn delimiters(&self) -> Vec<Delimiter> {
        self.segments
            .iter()
            .filter_map(|s| s.delimiter.clone())
            .collect()
    }

    pub fn sim(&self) -> Option<SimId> {
        self.sim_at().map(|(id, _)| id)
    }

    /// The stack below this continuation's `Sim` delimiter once spliced onto `stack`.
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

    /// Slots above the `Sim` delimiter's push height, which locates it after a splice.
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
