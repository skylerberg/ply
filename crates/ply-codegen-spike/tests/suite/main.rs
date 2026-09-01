//! Every integration test in this crate, as one binary: cargo links one per
//! `.rs` directly under `tests/`, and against cranelift at `opt-level = 3` the
//! link is most of what the job costs.
//!
//! Nothing here reads process-global state — no `#[global_allocator]`, no
//! `static` accumulator — so unlike `crates/ply-eval` and `crates/ply-corpus`
//! this crate has no test that has to keep a binary of its own beside this
//! one. Every fixture path comes from `env!("CARGO_MANIFEST_DIR")`, so the
//! move down a directory changes none of them.

mod entry_cost;
mod hazards;
mod mcts_kernel;
mod mutations;
mod spike;
