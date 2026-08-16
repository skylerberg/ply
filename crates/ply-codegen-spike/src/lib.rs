//! ADR 0016 §3's spike, and nothing else.
//!
//! Run it with `cargo +1.94.0 run --release -- --out spike.json`.
//!
//! `cranelift-jit 0.134.3` needs rustc 1.94, which is why the toolchain is
//! named. Nothing in the shipping workspace depends on this crate and the
//! workspace does not list it, so deferring M9 is `rm -r crates/ply-codegen-spike`.
//!
//! It exists to produce one number — the speedup a Cranelift backend would
//! reach on the hottest pure function of the request path — and is deleted when
//! W6 closes whatever that number turns out to be.

pub mod jit;
pub mod measure;
pub mod program;
pub mod rt;
pub mod served;
