// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod arena;
mod argv;
mod build;
mod builtins;
mod compiled;
mod cont;
mod escape;
mod explore;
mod host;
mod list;
mod memo;
mod numerics;
mod rc;
mod region;
mod sched;
mod sim;
mod task_regions;
mod value;
