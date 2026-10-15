//! Where a list's and a map's parts lie, for the code the emitter writes over them in place: what
//! the runtime's description reports, each fact read off the constant the runtime's own code uses.

use crate::heap::{KIND_BRANCH, KIND_LEAF, KIND_MBRANCH, KIND_MLEAF};
use crate::{list, map};

/// A fact of a layout: a number (a word's place, a kind, a width), or the header field a count is
/// held in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    Number(usize),
    Field(&'static str),
}

/// Each object the emitter writes code over in place, with its facts, under the names
/// `compiler.layout`'s `Layouts` gives them.
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
