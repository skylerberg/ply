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

pub mod registry;
pub mod sched;
pub mod tcp;
pub mod tls;

pub use registry::{Host, registry};
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
