//! Parse, resolve, check and hash a source: what every module in this binary needs before it can
//! assert anything about selection, scheduling or a cache.
//!
//! Two entry points, because they do not agree: [`Compiled::anonymous`] parses one module the way
//! `ply_syntax::parse` names it, and [`Compiled::new`] names it `m`. A module name reaches the
//! hashes through every program-wide symbol, so the two are not interchangeable.

use ply_core::{CheckOutput, check_program};
use ply_hash::HashOutput;
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;

pub struct Compiled {
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    pub hashes: HashOutput,
}

impl Compiled {
    /// One module named `m`.
    #[track_caller]
    pub fn new(src: &str) -> Compiled {
        Compiled::modules(&[("m", src)])
    }

    /// One module under the name `ply_syntax::parse` gives it, which is not `m`.
    #[track_caller]
    pub fn anonymous(src: &str) -> Compiled {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        Compiled::of(ply_syntax::ast::Program::single(module))
    }

    /// Several modules, each one's `SourceId` its position in `sources`.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let inputs: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
            .collect();
        Compiled::of(
            ply_syntax::parse_program(inputs)
                .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}")),
        )
    }

    #[track_caller]
    fn of(mut program: Program) -> Compiled {
        let resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes = ply_hash::hash_program(&program, &resolved, &check)
            .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
            hashes,
        }
    }

    /// Every test's footprint, owned, so a caller may take one from a temporary.
    pub fn footprints(&self) -> Vec<ply_core::Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }
}
