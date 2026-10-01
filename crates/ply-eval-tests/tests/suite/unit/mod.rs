// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod arena;
mod argv;
mod builtins;
mod carry;
mod codec;
mod compiled;
mod cont;
mod decode;
mod escape;
mod evaluator;
mod footprint;
mod hash;
mod host;
mod list;
mod memo;
mod numerics;
mod rc;
mod region;
mod sched;
mod sim;
mod span;
mod task_regions;
mod value;
