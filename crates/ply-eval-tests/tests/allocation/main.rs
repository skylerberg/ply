//! A binary of its own: it installs a `#[global_allocator]`.

mod counting;

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

mod bridge_reuse;
