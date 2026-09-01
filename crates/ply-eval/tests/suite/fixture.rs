//! Parse, resolve and check a source: what every module in this binary needs before it can assert
//! anything, and what twenty-three of them used to spell out for themselves.
//!
//! The module name a fixture compiles under is observable — assertions name `m.foo` or `t.foo` —
//! so it is a parameter here rather than a constant, and [`Compiled::new`] fixes only the `m` that
//! most of them want.

use ply_core::{CheckOutput, check_program};
use ply_eval::Machine;
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

pub struct Compiled {
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
}

impl Compiled {
    /// One module, named `m`.
    #[track_caller]
    pub fn new(source: &str) -> Compiled {
        Compiled::modules(&[("m", source)])
    }

    /// One module under the name its assertions spell, for the fixtures that say `t.foo`.
    #[track_caller]
    pub fn named(module: &str, source: &str) -> Compiled {
        Compiled::modules(&[(module, source)])
    }

    /// Several modules, each one's `SourceId` its position in `sources`.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let inputs: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
            .collect();
        let mut program = ply_syntax::parse_program(inputs)
            .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}"));
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    /// The diagnostics the checker refused `source` with, empty if it accepted. Parsing and
    /// resolution still have to succeed: a fixture that cannot get that far is a broken fixture,
    /// not an observation.
    #[track_caller]
    pub fn rejected(source: &str) -> Vec<Diagnostic> {
        Compiled::rejected_in("m", source)
    }

    /// [`Compiled::rejected`] under the module name the assertions spell.
    #[track_caller]
    pub fn rejected_in(module: &str, source: &str) -> Vec<Diagnostic> {
        let inputs = [(SourceId(0), ModuleName::from_dotted(module), source)];
        let mut program = ply_syntax::parse_program(inputs)
            .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}"));
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        check_program(&program, &resolved).err().unwrap_or_default()
    }

    /// Every test in `source`, run under the machine, for a fixture that asserts on what a
    /// completed run left behind rather than on the run itself.
    #[track_caller]
    pub fn ran(source: &str) -> Compiled {
        let c = Compiled::new(source);
        assert!(!c.check.tests.is_empty(), "the source declares no test");
        let mut machine = c.machine();
        for (i, t) in c.check.tests.iter().enumerate() {
            if let Err(d) = machine.eval_test(i) {
                panic!("`{}` failed under the machine: {d:#?}", t.name);
            }
        }
        c
    }

    pub fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    pub fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }
}
