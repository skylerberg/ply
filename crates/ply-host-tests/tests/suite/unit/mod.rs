//! `ply-host`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-host/src` without the `tests` level.

// `Value` pins `Arc` for its shared payloads, as the library's own allow explains.
#![allow(clippy::arc_with_non_send_sync)]

mod config;
mod db;
mod fs;
mod pool;
mod registry;
mod sched;
mod signal;
mod tcp;
mod tls;
mod trace;
