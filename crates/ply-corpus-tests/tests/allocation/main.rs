//! One binary, because a `#[global_allocator]` is whole-binary; `counting` is thread-local, so tests still run in parallel.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod frame_cost;
mod w6_report_allocations;
mod w6_request_cost;
