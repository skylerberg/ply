//! The trusted computing base: the OS calls behind the natives of the host handlers, and the run's
//! clocks and pools a production scheduler waits on.

pub mod certgen;
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

pub use registry::Host;
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
