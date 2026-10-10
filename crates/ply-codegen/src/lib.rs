//! Compiled code, the only evaluator: a program's emitted C built and loaded as a unit, and the
//! runtime that C calls into.

// `Value` holds `Arc`s but is not `Send`; raw-pointer helpers share the contract in `heap.rs`.
#![allow(clippy::arc_with_non_send_sync)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod array;
pub mod backend;
pub mod c;
pub mod detached;
pub mod heap;
pub mod host;
pub mod list;
pub mod map;
mod parallel;
pub mod rt;
pub mod simulate;
pub mod source;
pub mod stack;
pub mod stored;

pub use backend::{Bodies, Declines, Unit};
pub use c::{Profile, Refused, select_profile};
pub use source::{Source, clause_root_name};
