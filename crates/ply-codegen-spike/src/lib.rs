//! ADR 0016 §3's spike, and nothing else.
//!
//! Run it with `cargo run --release -- --out spike.json`, from this directory.
//!
//! > **Two sentences here were stale and are corrected in place, 2026-08-31.**
//! > They read: *"Run it with `cargo +1.94.0 run --release -- --out
//! > spike.json`."* and *"`cranelift-jit 0.134.3` needs rustc 1.94, which is
//! > why the toolchain is named."* This crate moved to **cranelift 0.132.3**,
//! > which declares `rust-version = "1.93.0"`, so it builds on the 1.93.1 the
//! > workspace pins and no toolchain needs naming anywhere. The `Cargo.toml`
//! > three files over was updated in that change and this header was missed.
//!
//! Nothing in the shipping workspace depends on this crate and the workspace
//! does not list it, so deferring M9 is `rm -r crates/ply-codegen-spike`.
//!
//! > **The last clause is still true of *this* crate and is no longer the whole
//! > picture (2026-08-31).** `crates/ply-codegen` is a workspace member that
//! > puts a cranelift code generator behind `ply test --backend cranelift`. It
//! > does **not** depend on this crate — it ports `jit.rs` and `rt.rs` as
//! > source, with the provenance in their headers — so `rm -r` here still
//! > costs the workspace nothing to compile. What it costs is the two
//! > measurements only this crate produces: ADR 0018 §0.5's 6.199× and the
//! > agreement corpus of `CONTRIBUTING.md` item 18, which is currently red.
//! > ADR 0026 §4.7 records the deletion condition as **met** and says closing
//! > item 18 is what should carry the deletion.
//!
//! It exists to produce one number — the speedup a Cranelift backend would
//! reach on the hottest pure function of the request path — and is deleted when
//! W6 closes whatever that number turns out to be.

pub mod entry;
pub mod jit;
pub mod measure;
pub mod program;
pub mod rt;
pub mod served;
pub mod wrong;
