//! The compiled tier behind `ply test --backend c`: the machine's lowered `Code` emitted as C.

// `Value` holds `Arc`s but is not `Send`; raw-pointer helpers share the contract in `heap.rs`.
#![allow(clippy::arc_with_non_send_sync)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod backend;
pub mod c;
pub mod detached;
pub mod heap;
pub mod host;
pub mod list;
pub mod map;
pub mod rt;
pub mod simulate;
pub mod source;
pub mod stack;

pub use backend::{Bodies, Closed, Declines, Unit, closure};
pub use c::{Profile, Refused, select_profile};
pub use source::{Source, clause_root_name, emit_keys, law_root_name, test_root_name};
