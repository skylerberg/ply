//! One binary. A test that reads process-global state (`#[global_allocator]`, `ply_eval::census`) needs a binary of its own.

mod constant_memo_service;
mod http_cost;
mod region_isolation_cost;
mod support;
mod tier_audit;
mod unit;
mod w1_baseline;
mod w6_report_integrity;
