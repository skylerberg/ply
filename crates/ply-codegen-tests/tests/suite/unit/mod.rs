//! `ply-codegen`'s unit tests; the module tree mirrors `crates/ply-codegen/src`.

// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod backend;
mod blocks;
mod c;
mod heap;
mod linked;
mod list;
mod map;
mod stack;
mod stored;
