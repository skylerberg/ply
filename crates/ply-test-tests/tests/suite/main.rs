//! One binary for link time; a test reading process-global state (allocator, census) needs its own.

mod fixture;

mod bisect_audit;
mod classification_audit;
mod effect_set_scheduling_audit;
mod host_scheduler_audit;
mod host_selection_audit;
mod host_trust_audit;
mod hybrid;
mod isolation_audit;
mod obligations;
mod region_fixture_cost;
mod unit;
