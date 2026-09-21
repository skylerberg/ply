//! The explicit control stack, and the prompts that delimit it.

use crate::pool::{self, Free, Link, Pooled};
use crate::value::{List, Value};
use ply_span::{Span, Symbol};
use ply_ty::BinOp;
use std::rc::Rc;

#[derive(Clone)]
pub enum Frame {
    BinaryApply {
        op: BinOp,
        lhs: Value,
        lhs_span: Span,
        rhs_span: Span,
        span: Span,
    },

    Call {
        name: Option<Symbol>,
        call_site: Span,
        /// First evaluation of a nullary pure definition; the answer is its constant.
        memo: bool,
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

pub struct Prompt {
    pub span: Span,
}

/// A `simulate` region: its ordinal among the regions one entry point has entered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SimId(pub u32);

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
    prompt: Option<Rc<Prompt>>,
}

pub enum Next {
    Frame(Frame, Stack),
    Leave(Rc<Prompt>, Stack),
    Done,
}

#[derive(Clone, Default)]
pub struct Stack {
    /// The innermost segment, held by value rather than as the head of `under`.
    top: Segment,
    under: Chain<Segment>,
    frames: usize,
}

impl Stack {
    pub fn new() -> Stack {
        Stack::default()
    }

    pub fn frames(&self) -> usize {
        self.frames
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
        self.top.frames = std::mem::take(&mut self.top.frames).push(frame);
        self.frames += 1;
        self
    }

    pub fn push_prompt(&self, prompt: Rc<Prompt>) -> Stack {
        let mut out = self.clone();
        let displaced = std::mem::replace(
            &mut out.top,
            Segment {
                frames: Chain::new(),
                prompt: Some(prompt),
            },
        );
        out.under = std::mem::take(&mut out.under).push(displaced);
        out
    }

    pub fn next(&self) -> Next {
        self.clone().into_next()
    }

    pub fn into_next(mut self) -> Next {
        if let Some(frame) = self.top.frames.pop_front() {
            self.frames -= 1;
            return Next::Frame(frame, self);
        }
        match self.top.prompt.take() {
            Some(prompt) => {
                self.top = self
                    .under
                    .pop_front()
                    .expect("only the base segment has no prompt, and it is the outermost");
                Next::Leave(prompt, self)
            }
            None => Next::Done,
        }
    }
}
