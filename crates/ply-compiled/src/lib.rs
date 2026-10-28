//! Compiled code, the only evaluator: a program's emitted C built and loaded as a unit, and the
//! runtime that C calls into.

// Raw-pointer helpers share the contract in `heap.rs`.
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod blocks;
pub mod c;
pub mod detached;
pub mod emitted;
pub mod entry;
pub mod flat;
pub mod frames;
pub mod heap;
pub mod host;
pub mod hosted;
pub mod machine;
pub mod native;
mod parallel;
pub mod rt;
pub mod secret;
pub mod stack;
pub mod tasks;
mod vm;
