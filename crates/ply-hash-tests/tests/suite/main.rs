//! One binary for every integration test, because the link dominates the build. A test that
//! reads process-global state (a `#[global_allocator]`, `ply_eval::census`) needs its own binary.

mod audit;
mod bodies;
mod derivation;
mod effect_sets;
mod effect_sets_audit;
mod fixture;
mod map;
mod modules;
mod modules_audit;
mod unit;
