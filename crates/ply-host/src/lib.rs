//! The trusted computing base: the Rust handlers a Ply program's effect operations may resolve to.

// The db driver builds `Value`s to answer with, and `Value` pins `Arc` for its shared payloads and
// `Rc` for shared code — so none of those `Arc`s can ever be `Send`.
#![allow(clippy::arc_with_non_send_sync)]

pub mod config;
pub mod db;
pub mod fs;
/// Not `pub`: a blocking pool is how a facility answers, never something a
/// caller composes. `tcp` and `fs` each own one, and the token ranges they mint
/// in are disjoint so a composed runtime can tell whose answer a token is.
mod pool;
pub mod registry;
pub mod sched;
pub mod signal;
pub mod tcp;
pub mod tls;
pub mod trace;

pub use registry::{Host, registry, registry_over, registry_with_database};
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
