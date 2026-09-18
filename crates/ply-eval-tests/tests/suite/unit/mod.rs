// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod arena;
mod argv;
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
