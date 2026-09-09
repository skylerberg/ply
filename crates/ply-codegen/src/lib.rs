//! The compiled tier behind `ply test --backend c`: the machine's lowered `Code` emitted as C.

// `Value` is `Arc` in five of its variants and a `Value` is not `Send`, so every construction of
// one trips `arc_with_non_send_sync`. The runtime's helpers and the heap's accessors take the
// words compiled code holds, which are raw pointers by design; their contract is the code
// generator's, stated once in `heap.rs`, not a `# Safety` section per helper.
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
pub mod opt;
pub mod rt;
pub mod simulate;
pub mod source;
pub mod stack;

pub use backend::{Bodies, Closed, Declines, Embedded, Unit, closure};
pub use c::{Profile, Refused, select_profile};
pub use source::{
    Source, clause_root_name, emit_keys, is_spec_root, law_root_name, test_root_name,
};
