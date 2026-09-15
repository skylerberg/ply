//! `ply-codegen`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-codegen/src` without the `tests` level.

// `Value` pins `Arc` for its shared payloads, as the library's own allow explains.
#![allow(clippy::arc_with_non_send_sync)]

mod c;
mod heap;
mod list;
mod map;
mod opt;
mod stack;
