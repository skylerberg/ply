//! One binary. A test that reads process-global state (`#[global_allocator]`, `ply_eval::census`) needs a binary of its own.

mod fragment;
mod hazards;
mod kernel;
mod number_types;
mod unit;
