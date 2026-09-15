//! `ply-eval`'s unit tests, in a package of their own so they compile at
//! opt-level 0 against the optimised rlib. The module tree mirrors
//! `crates/ply-eval/src` without the `tests` level; `evaluator` holds what
//! `src/tests.rs` held.

// `Value` pins `Arc` for its shared payloads, as the library's own allow explains.
#![allow(clippy::arc_with_non_send_sync)]

mod arena;
mod argv;
mod backend;
mod build;
mod builtins;
mod code;
mod compiled;
mod cont;
mod differential;
mod escape;
mod evaluator;
mod explore;
mod handler;
mod host;
mod list;
mod memo;
mod numerics;
mod rc;
mod region;
mod region_kind;
mod sched;
mod semantics;
mod sim;
mod task_regions;
mod window;
