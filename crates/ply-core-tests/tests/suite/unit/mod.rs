// `Value` pins `Arc` for its shared payloads.
#![allow(clippy::arc_with_non_send_sync)]

mod argv;
mod builtins;
mod codec;
mod crypto;
mod escape;
mod float;
mod footprint;
mod host;
mod list;
mod memo;
mod numerics;
mod rc;
mod region;
mod sched;
mod sim;
mod span;
mod sqlite;
mod value;
