//! One binary. A test that reads process-global state (`#[global_allocator]`, `ply_eval::census`)
//! needs a binary of its own.
//!
//! What belongs here: a test of `ply` as shipped — the binary as a process, the artifact and the
//! module sources `ply build` commits and the launcher lays out, and the standard library and the
//! examples as `ply` presents them. A test whose subject is a workspace library's own API belongs
//! to that library's `-tests` package, which is where `ply-machine-tests` keeps the machine's
//! modules, the prover's soundness audit and the two front-end conformance suites.
//!
//! Every command a test runs here comes from [`harness`], which is also the only place the `ply`
//! binary is named, so no test can quietly inherit the machine's environment or forget a flag.

mod artifact;
mod artifact_program;
mod backend;
mod bootstrap_archive;
mod cache_cli;
mod cli;
mod config_cli;
mod corpus;
mod derivable;
mod derivation_determinism_audit;
mod derive;
mod desk_operations;
mod determinism_audit;
mod diagnostic_text;
mod doc;
mod effect_set_selection;
mod effect_sets;
mod explain;
mod failure_classification_audit;
mod fmt;
mod grants;
mod harness;
mod http_audit;
mod http_endpoint;
mod incremental;
mod json_endpoint;
mod lang_fixtures;
mod manifest;
mod map_cache;
mod map_law;
mod mutate;
mod nesting;
mod new;
mod numerics;
mod packages;
mod process_cli;
mod prove;
mod refcount_counters;
mod regressions;
mod replace;
mod routing_audit;
mod shutdown;
mod stdlib;
mod stdlib_audit;
mod surface;
mod text;
mod tls_cli;
mod trace_audit;
mod tree;
