//! What a builtin resumes with once a closure it called has answered.

use crate::Span;
use crate::value::{List, Value};

#[derive(Clone)]
pub enum Frame {
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

/// A `simulate` region: its ordinal among the regions one entry point has entered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SimId(pub u32);
