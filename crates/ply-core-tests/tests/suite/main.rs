//! One binary; a test reading process-global state (an allocator count, `census`) needs its own.

mod fixture;

mod bit_operators;
mod byte_builtins;
mod derivation;
mod effect_sets;
mod effect_sets_audit;
mod fused_update_builtins;
mod iterate_builtin;
mod list_builtins;
mod map_keys;
mod number_types;
mod record_update;
mod region_escape_audit;
mod regions;
mod secrets;
mod shipped_modules;
mod try_op;
mod unit;
