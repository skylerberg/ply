//! The trusted computing base: the OS calls behind the natives of the real handlers, and the
//! language's `clock` and `task` as the runtime serves them.

pub mod certgen;
pub mod clock;
pub mod config;
pub mod dns;
pub mod fs;
pub mod jobs;
pub mod native;
pub mod observe;
pub mod os;
pub mod password;
pub mod pool;
pub mod process;
pub mod random;
pub mod registry;
pub mod signal;
pub mod sqlite;
pub mod stdio;
pub mod tcp;
pub mod time;
pub mod tls;
pub mod trace;
pub mod udp;

pub use registry::{Host, registry};
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
