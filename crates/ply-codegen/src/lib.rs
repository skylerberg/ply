//! A cranelift code generator behind `ply test --backend cranelift`.
//!
//! This crate is what ADR 0026 §4.1 decided and what six milestones deferred: a
//! real code generator that a shipping command installs, rather than a
//! measurement harness in a quarantined workspace on a second toolchain.
//!
//! # What it is, and what it is not
//!
//! It compiles the fragment ADR 0016 §3.2 pins — `Int` and `Bool` arithmetic,
//! comparison, the short-circuiting operators, `if`, `let`, `block`, `match` on
//! literal patterns, and calls between members of the compiled set. Everything
//! else is **refused by name**, and a definition whose body holds one refused
//! construct refuses its callers on the next round of [`backend::Cranelift`]'s
//! fixpoint. Values stay boxed; the win is the dispatch loop, not unboxing.
//!
//! It is not the fast path for a Ply program in general. ADR 0030 measured the
//! front end end to end at **1.0887×** with `ply_eval::backend::Reference`
//! attached, against a ceiling of **1.121×** for an *infinitely fast* backend —
//! so on that workload a real code generator can add at most about 3% over what
//! already ships. ADR 0018 measured **6.199×** on a compute kernel, because a
//! compute loop is almost entirely inside the fragment. Both numbers are about
//! the same seam and the difference between them is the workload.
//!
//! # Its relationship to `crates/ply-codegen-spike`
//!
//! `crate::jit` and `crate::rt` are ported from the spike's files of the same
//! names and are close to unchanged; the spike's `program.rs` loaders are gone,
//! because a shipping backend is handed a program a command already loaded, and
//! its `entry.rs` became [`backend`], which implements `ply_eval::Policed` so
//! that the eight wrong backends wrap it.
//!
//! The spike is **not** deleted by this crate existing and is not depended on by
//! it. ADR 0026 §4.7 makes its deletion conditional on all eight of its wrong
//! backends being reproduced inside the workspace, and the eighth — an
//! unbounded runaway reported from outside the process — is the one this crate
//! makes reachable rather than the one it discharges. See that section.

// `Value` is `Arc` in five of its variants and a `Value` is not `Send`, so
// every construction of one trips `arc_with_non_send_sync`. The `Arc` is
// `ply_eval::Value`'s own representation and not a choice this crate makes;
// `crates/ply-eval/src/lib.rs` carries the identical allow for the identical
// reason, and this crate constructs `Value`s in `rt.rs` for the same seam.
#![allow(clippy::arc_with_non_send_sync)]

pub mod backend;
pub mod jit;
pub mod rt;
pub mod source;

pub use backend::{Bodies, Cranelift, Declines};
pub use jit::{Jit, Opts, Refused, Unit};
pub use source::Source;
