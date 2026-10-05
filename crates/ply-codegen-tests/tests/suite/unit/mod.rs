//! `ply-codegen`'s unit tests; the module tree mirrors `crates/ply-codegen/src`.

// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod c;
mod detached;
mod heap;
mod list;
mod map;
mod parallel;
mod simulate;
mod stack;
mod stored;
