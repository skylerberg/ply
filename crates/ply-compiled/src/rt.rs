//! The helpers compiled code calls into and the context an entry runs in. A helper that takes an
//! argument owns it, one that reads leaves its count alone, and every result is the caller's.

use crate::emitted::{Block, Carried, Handed, KeepRoot, PlyOp, PlyType};
use crate::heap::{self, CLOSURE_CAPTURES, CLOSURE_CODE, Heap, Word, obj, set_word, word_at};
use crate::native::I64;
use crate::stack::{Stack, switch};
use ply_core::builtins::scoped_released;
use ply_core::{Diagnostic, Mode, Span, Symbol, codes};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};

mod budget;
mod calls;
mod ctx;
mod data;
mod effects;
mod frames;
mod marshalling;
mod memory;
mod natives;
mod tables;

pub use budget::*;
pub use calls::*;
pub use ctx::*;
pub use data::*;
pub use effects::*;
pub use frames::*;
pub use marshalling::*;
pub use memory::*;
pub use natives::*;
pub use tables::*;
