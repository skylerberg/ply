//! The trusted computing base: the Rust handlers a Ply program's effect operations may resolve to.

// `Value` holds `Rc`, so the `Arc`s the db driver builds around values can never be `Send`.
#![allow(clippy::arc_with_non_send_sync)]

pub mod config;
pub mod db;
pub mod fs;
pub mod pool;
pub mod registry;
pub mod sched;
pub mod signal;
pub mod tcp;
pub mod tls;
pub mod trace;

pub use registry::{Host, registry, registry_over, registry_with_database};
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
