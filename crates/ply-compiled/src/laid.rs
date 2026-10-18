//! Where a list's and a map's parts lie, for the code the emitter writes over them in place, and
//! where the fields of the context and of the descriptors emitted code lays out lie: what the
//! runtime's description reports, each fact read off the constant or the type the runtime's own
//! code uses.

use crate::emitted::{PlyFnDesc, PlyOp, PlyType};
use crate::heap::{KIND_BRANCH, KIND_LEAF, KIND_MBRANCH, KIND_MLEAF};
use crate::rt::Ctx;
use crate::{list, map};
use std::mem::{offset_of, size_of};

/// The fields of the context compiled code reads and writes itself, each with its offset.
pub fn ctx_fields() -> Vec<(&'static str, usize)> {
    vec![
        ("failed", offset_of!(Ctx, failed)),
        ("fuel", offset_of!(Ctx, fuel)),
        ("stack_floor", offset_of!(Ctx, stack_floor)),
        ("site_root", offset_of!(Ctx, site_root)),
        ("site_start", offset_of!(Ctx, site_start)),
        ("site_end", offset_of!(Ctx, site_end)),
        ("ticks", offset_of!(Ctx, ticks)),
        ("next_tick", offset_of!(Ctx, next_tick)),
        ("release_depth", offset_of!(Ctx, release_depth)),
        ("release_waiting", offset_of!(Ctx, release_waiting)),
    ]
}

/// A struct emitted code lays out and the runtime reads: its size, and each field with its offset.
pub struct Described {
    pub name: &'static str,
    pub size: usize,
    pub fields: Vec<(&'static str, usize)>,
}

/// Each descriptor emitted code lays out, under the name `compiler.emit.layout` gives it.
pub fn descriptors() -> [Described; 3] {
    [
        Described {
            name: "PlyType",
            size: size_of::<PlyType>(),
            fields: vec![
                ("identity", offset_of!(PlyType, identity)),
                ("spelled", offset_of!(PlyType, spelled)),
                ("spelled_len", offset_of!(PlyType, spelled_len)),
                ("inc", offset_of!(PlyType, inc)),
                ("dec", offset_of!(PlyType, dec)),
                ("lower", offset_of!(PlyType, lower)),
                ("lift", offset_of!(PlyType, lift)),
            ],
        },
        Described {
            name: "PlyOp",
            size: size_of::<PlyOp>(),
            fields: vec![
                ("effect", offset_of!(PlyOp, effect)),
                ("op", offset_of!(PlyOp, op)),
                ("answer", offset_of!(PlyOp, answer)),
                ("own", offset_of!(PlyOp, own)),
                ("params_len", offset_of!(PlyOp, params_len)),
                ("params", offset_of!(PlyOp, params)),
                ("vars_len", offset_of!(PlyOp, vars_len)),
                ("vars", offset_of!(PlyOp, vars)),
            ],
        },
        Described {
            name: "PlyFnDesc",
            size: size_of::<PlyFnDesc>(),
            fields: vec![
                ("uniform", offset_of!(PlyFnDesc, uniform)),
                ("typed", offset_of!(PlyFnDesc, typed)),
                ("convention", offset_of!(PlyFnDesc, convention)),
                ("release", offset_of!(PlyFnDesc, release)),
                ("share", offset_of!(PlyFnDesc, share)),
                ("keep", offset_of!(PlyFnDesc, keep)),
            ],
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
