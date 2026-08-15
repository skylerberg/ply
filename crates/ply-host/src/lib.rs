//! The trusted computing base: the Rust handlers a Ply program's effect
//! operations may resolve to.
//!
//! A Ply program never calls the host. It performs an ordinary effect operation
//! and the runtime's handler stack may resolve it here, which is why a
//! host-backed effect and an in-memory one are indistinguishable at the type
//! level and why substitution works at all.
//!
//! Everything in this crate is registered by hand, in a function a reviewer can
//! read top to bottom. There is no attribute macro and no link-time registry: a
//! member of the trusted computing base gets in by someone writing it down.

// The db driver builds `Value`s to answer with, and `Value` pins `Arc` for its
// shared payloads and `Rc` for shared code — so none of those `Arc`s can ever be
// `Send`. That is `ply-eval`'s design rather than an oversight here, and this is
// the same allow, for the same reason, that `ply-eval` and `ply-prove` carry.
#![allow(clippy::arc_with_non_send_sync)]

pub mod config;
pub mod db;
pub mod registry;
pub mod sched;
pub mod signal;
pub mod tcp;
pub mod tls;
pub mod trace;

pub use registry::{Host, registry, registry_over, registry_with_database};
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
