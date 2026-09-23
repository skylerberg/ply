//! What a run is configured with, as plain data: the machines read these, and the shell's parsed
//! flags convert into them. Nothing here knows about clap.

use ply_host::tls::CredentialSpec;
use std::path::PathBuf;

/// TLS credentials and extra roots of trust.
#[derive(Clone, Debug, Default)]
pub struct TlsOptions {
    pub tls: Vec<CredentialSpec>,
    pub trust: Vec<PathBuf>,
}

/// What a `SIGINT` or a `SIGTERM` does to a serving run.
#[derive(Clone, Copy, Debug)]
pub struct ShutdownOptions {
    pub drain_ms: u64,
    pub drain_lead_ms: u64,
}

impl Default for ShutdownOptions {
    fn default() -> ShutdownOptions {
        ShutdownOptions {
            drain_ms: ply_host::signal::DEFAULT_DRAIN_MS,
            drain_lead_ms: ply_host::signal::DEFAULT_LEAD_MS,
        }
    }
}

impl ShutdownOptions {
    pub fn bounds(&self) -> ply_host::signal::Bounds {
        ply_host::signal::Bounds {
            lead: std::time::Duration::from_millis(self.drain_lead_ms),
            drain: std::time::Duration::from_millis(self.drain_ms),
        }
    }
}

/// A three-way switch for work a run may do on its own behalf.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum When {
    #[default]
    Auto,
    Always,
    Never,
}

impl When {
    pub fn as_str(self) -> &'static str {
        match self {
            When::Auto => "auto",
            When::Always => "always",
            When::Never => "never",
        }
    }
}
