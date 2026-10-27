//! Where the fields of the context and of the descriptors emitted code lays out lie, and where a
//! hold keeps what it holds, in the order a released `ply`'s bootstrap compares them with its own:
//! what the runtime's JSON description reports, each read off the constant or the type the
//! runtime's own code uses.

use crate::emitted::{PlyFnDesc, PlyOp, PlyType};
use crate::frames::PlyFrame;
use crate::rt::Ctx;
use std::mem::{offset_of, size_of};

/// A field of a struct emitted code and the runtime both lay out: where it lies in the struct and
/// how many bytes it takes, as `compiler.emit.layout`'s `MachineField` says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub at: usize,
    pub bytes: usize,
}

/// The size of the field `field` borrows out of an `S`.
fn field_bytes<S, F>(_field: fn(&S) -> &F) -> usize {
    size_of::<F>()
}

/// Each named field of `$s`, in the order given, as the Rust type lays it out.
macro_rules! fields {
    ($s:ty: $($f:ident),* $(,)?) => {
        vec![$(Field {
            name: stringify!($f),
            at: offset_of!($s, $f),
            bytes: field_bytes::<$s, _>(|s| &s.$f),
        }),*]
    };
}

/// The fields of the context a released `ply`'s code reads and writes itself, in the context's
/// order; a field this `ply`'s code reads anew is `c::described`'s alone.
pub fn ctx_fields() -> Vec<Field> {
    fields!(Ctx: failed, fuel, stack_floor, site_root, site_start, site_end, ticks, next_tick,
        frames, release_depth, release_waiting)
}

/// A struct emitted code lays out and the runtime reads, as `compiler.emit.layout`'s
/// `MachineStruct` says it: its size in bytes, and each of its fields in order.
pub struct Described {
    pub name: &'static str,
    pub bytes: usize,
    pub fields: Vec<Field>,
}

/// Each struct emitted code lays out and the runtime reads, in the order
/// `compiler.emit.layout.machine_structs` lists them: each declared before one that points at it.
pub fn descriptors() -> [Described; 4] {
    [
        Described {
            name: "PlyFrame",
            bytes: size_of::<PlyFrame>(),
            fields: fields!(PlyFrame: prev, each),
        },
        Described {
            name: "PlyType",
            bytes: size_of::<PlyType>(),
            fields: fields!(PlyType: identity, spelled, spelled_len, inc, dec, lower, lift),
        },
        Described {
            name: "PlyOp",
            bytes: size_of::<PlyOp>(),
            fields: fields!(PlyOp: effect, op, answer, own, params_len, params, vars_len, vars),
        },
        Described {
            name: "PlyFnDesc",
            bytes: size_of::<PlyFnDesc>(),
            fields: fields!(PlyFnDesc: uniform, typed, convention, release, share, keep),
        },
    ]
}

/// Where a hold keeps what it holds, as `compiler.emit.layout`'s `HoldLaid` names it: the payload
/// word `held`, 0 once the hold has let go.
pub fn hold() -> Vec<(&'static str, usize)> {
    vec![("held", crate::rt::HOLD_HELD)]
}
