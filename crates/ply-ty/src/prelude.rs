//! The prelude declarations the Rust runtime still reads.
//!
//! The effects the language declares, the atoms `simulate` discharges, the seeded handlers and the
//! `Task`/`Secret`/`Cell` types are the compiler's (`infer.ply`), and reach Rust in the front end's
//! answer. What is left here is the one table that answer does not carry grouped: the ADTs the
//! language declares without a file, which the prover and the code generator need by name.

use ply_span::Symbol;

/// The handle `task.spawn` answers with.
pub const TASK_TYPE: &str = "Task";

/// An ADT the language declares rather than a module.
pub struct Adt {
    pub name: &'static str,
    /// Each constructor's name and how many fields it carries.
    pub variants: &'static [(&'static str, usize)],
}

/// Every ADT the language declares without a file.
pub const ADTS: &[Adt] = &[
    Adt {
        name: "Option",
        variants: &[("None", 0), ("Some", 1)],
    },
    Adt {
        name: "Result",
        variants: &[("Ok", 1), ("Err", 1)],
    },
    Adt {
        name: "Ordering",
        variants: &[("Less", 0), ("Equal", 0), ("Greater", 0)],
    },
    Adt {
        name: "Rounding",
        variants: &[
            ("HalfEven", 0),
            ("HalfUp", 0),
            ("Down", 0),
            ("Up", 0),
            ("Ceiling", 0),
            ("Floor", 0),
        ],
    },
    // Two parameters so `Stop` can carry a result of a different type than the seed.
    Adt {
        name: "Iter",
        variants: &[("Continue", 1), ("Stop", 1)],
    },
];

/// Every prelude constructor and its arity, in declaration order.
pub fn ctor_arities() -> Vec<(Symbol, usize)> {
    ADTS.iter()
        .flat_map(|adt| adt.variants)
        .map(|(name, arity)| (Symbol::new(*name), *arity))
        .collect()
}
