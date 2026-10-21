//! The helpers compiled code calls into and the context an entry runs in. A helper that takes an
//! argument owns it, one that reads leaves its count alone, and every answer is the caller's.

use crate::emitted::{Block, Carried, Handed, KeepRoot, PlyFnDesc, PlyOp, PlyType};
use crate::heap::{
    self, CLOSURE_CAPTURES, CLOSURE_CODE, CLOSURE_DESCRIBED, Heap, KIND_ARRAY, KIND_BRIDGE,
    KIND_BYTES, KIND_CLOSURE, KIND_COLUMNS, KIND_CTOR, KIND_LIST, KIND_MAP, KIND_MBRANCH,
    KIND_MLEAF, KIND_RECORD, KIND_STR, Layouts, Stated, Word, bridged, bytes_of, is_unique, obj,
    set_word, str_of, word_at,
};
use crate::host::Served;
use crate::map;
use crate::native::{Answered, I64, Native, Raises, Raising, Signature};
use crate::stack::{Stack, switch};
use crate::{array, columns, list};
use ply_core::builtins::hold_released;
use ply_core::region::StepSite;
use ply_core::sched::Shield;
use ply_core::sim::Access;
use ply_core::{
    BinOp, Builtin, Closure, ClosureKind, Diagnostic, EffectAtom, Mode, Plain, Resource, Span,
    Symbol, Value, codes, values_equal,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use std::sync::{Arc, Mutex, MutexGuard};

mod budget;
mod calls;
mod crossing;
mod ctx;
mod data;
mod effects;
mod frames;
mod memory;
mod natives;
mod numbers;
mod tables;

pub use budget::*;
pub use calls::*;
pub use crossing::*;
pub use ctx::*;
pub use data::*;
pub use effects::*;
pub use frames::*;
pub use memory::*;
pub use natives::*;
pub use numbers::*;
pub use tables::*;
