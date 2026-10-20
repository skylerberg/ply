//! Where a list's and a map's parts lie, for the code the emitter writes over them in place, and
//! where the fields of the context and of the descriptors emitted code lays out lie: what the
//! runtime's description reports, each fact read off the constant or the type the runtime's own
//! code uses.

use crate::emitted::{PlyFnDesc, PlyOp, PlyType};
use crate::frames::PlyFrame;
use crate::heap::{KIND_BRANCH, KIND_LEAF, KIND_MBRANCH, KIND_MLEAF};
use crate::rt::Ctx;
use crate::{list, map};
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

/// The fields of the context compiled code reads and writes itself, in the context's order.
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

/// A fact of a layout: a number (a word's place, a kind, a width), or the header field a count is
/// held in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    Number(usize),
    Field(&'static str),
}

/// Each object the emitter writes code over in place, with its facts, under the names
/// `compiler.emit.layout`'s `Layouts` gives them.
pub fn layouts() -> [(&'static str, Vec<(&'static str, Fact)>); 4] {
    use Fact::{Field, Number};
    [
        (
            "list",
            vec![
                ("root", Number(list::ROOT)),
                ("tail", Number(list::TAIL)),
                ("start", Field(list::START.name())),
                ("tail_len", Field(list::TAIL_LEN.name())),
                ("room", Field(list::ROOM.name())),
            ],
        ),
        (
            "trie",
            vec![
                ("leaf", Number(KIND_LEAF.into())),
                ("branch", Number(KIND_BRANCH.into())),
                ("width", Number(list::WIDTH)),
            ],
        ),
        (
            "map",
            vec![
                ("root", Number(map::ROOT)),
                ("height", Field(map::HEIGHT.name())),
            ],
        ),
        (
            "tree",
            vec![
                ("leaf", Number(KIND_MLEAF.into())),
                ("branch", Number(KIND_MBRANCH.into())),
                ("width", Number(map::WIDTH)),
                ("keys", Number(map::KEYS)),
                ("sizes", Number(map::SIZES)),
            ],
        ),
    ]
}
