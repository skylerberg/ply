//! Parse, resolve and check a source: what every module in this binary needs before it can assert
//! anything, and what twenty-three of them used to spell out for themselves.
//!
//! The module name a fixture compiles under is observable — assertions name `m.foo` or `t.foo` —
//! so it is a parameter here rather than a constant, and [`Compiled::new`] fixes only the `m` that
//! most of them want.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Machine, Provider};
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::collections::HashMap;

pub struct Compiled {
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    /// Each module's source text, keyed by `m.name.to_string()` — what the whole Ply emitter
    /// re-parses to produce bodies, since it is a front end rather than an AST consumer.
    pub texts: HashMap<String, String>,
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
        let texts = sources
            .iter()
            .map(|(name, src)| {
                (
                    ModuleName::from_dotted(name).to_string(),
                    (*src).to_string(),
                )
            })
            .collect();
        Compiled {
            program,
            resolved,
            check,
            texts,
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

    /// A machine running on a real compiled tier — the only evaluator under tier-only (ADR 0048).
    /// Every eval-test that runs a program uses it, since a bare machine holds no evaluator.
    ///
    /// The unit is built with the module source texts so the whole Ply emitter — installed by
    /// [`ply_codegen::c::producer::ensure_default`] — can re-parse them into bodies; `Unit::over`
    /// alone gets only the reference fragment, which holds no `perform`/`handle`/`simulate`.
    pub fn machine(&self) -> Machine<'_> {
        ply_codegen::c::producer::ensure_default();
        let mut m = Machine::new(&self.program, &self.resolved, &self.check);
        let unit = ply_codegen::Unit::over_with_texts(
            &self.program,
            &self.resolved,
            &self.check,
            self.texts.clone(),
        )
        .expect("this host has a C compiler");
        let spec = ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
            ..Default::default()
        };
        m.set_compiled(unit.attach(&spec));
        m
    }

    pub fn machine_on_tier(&self) -> Machine<'_> {
        self.machine()
    }

    pub fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }
}
