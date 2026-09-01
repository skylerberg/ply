//! A cranelift code generator behind `ply test --backend cranelift`.

// `Value` is `Arc` in five of its variants and a `Value` is not `Send`, so every construction of
// one trips `arc_with_non_send_sync`.
#![allow(clippy::arc_with_non_send_sync)]

pub mod backend;
pub mod jit;
pub mod rt;
pub mod source;

pub use backend::{Bodies, Cranelift, Declines};
pub use jit::{Jit, Opts, Refused, Unit};
pub use source::Source;
