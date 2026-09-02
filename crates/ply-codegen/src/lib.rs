//! A cranelift code generator behind `ply test --backend cranelift`.

// `Value` is `Arc` in five of its variants and a `Value` is not `Send`, so every construction of
// one trips `arc_with_non_send_sync`. The runtime's helpers and the heap's accessors take the
// words compiled code holds, which are raw pointers by design; their contract is the code
// generator's, stated once in `heap.rs`, not a `# Safety` section per helper.
#![allow(clippy::arc_with_non_send_sync)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod backend;
pub mod heap;
pub mod jit;
pub mod list;
pub mod opt;
pub mod rt;
pub mod source;

pub use backend::{Bodies, Cranelift, Declines};
pub use jit::{Jit, Opts, Refused, Unit};
pub use source::Source;
