//! Compiled code, the only evaluator: a program's emitted C built and loaded as a unit, and the
//! runtime that C calls into.

// `Value` holds `Arc`s but is not `Send`; raw-pointer helpers share the contract in `heap.rs`.
#![allow(clippy::arc_with_non_send_sync)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod array;
pub mod backend;
pub mod blocks;
pub mod c;
pub mod columns;
pub mod detached;
pub mod emitted;
pub mod frames;
pub mod heap;
pub mod host;
pub mod laid;
pub mod list;
pub mod machine;
pub mod map;
pub mod native;
mod parallel;
pub mod rt;
pub mod simulate;
pub mod source;
pub mod stack;
mod vm;

pub use backend::{Bodies, Declines, Unit};
pub use c::Refused;
pub use source::{Source, clause_root_name};
