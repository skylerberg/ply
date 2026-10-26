//! `ply-compiled`'s unit tests; the module tree mirrors `crates/ply-compiled/src`.

// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod blocks;
mod c;
mod detached;
mod emitted;
mod entry;
mod frames;
mod heap;
mod hosted;
mod linked;
mod machine;
mod parallel;
mod rt;
mod stack;
