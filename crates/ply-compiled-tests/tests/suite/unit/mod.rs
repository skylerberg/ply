//! `ply-compiled`'s unit tests; the module tree mirrors `crates/ply-compiled/src`.

// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod backend;
mod blocks;
mod c;
mod heap;
mod linked;
mod list;
mod map;
mod parallel;
mod stack;
mod stored;
