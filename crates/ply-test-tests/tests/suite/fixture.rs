//! Parse, resolve, check and hash a source: what every module in this binary needs before it can
//! assert anything about selection, scheduling or a cache.
//!
//! Two entry points, because they do not agree: [`Compiled::anonymous`] parses one module the way
//! `ply_syntax::parse` names it, and [`Compiled::new`] names it `m`. A module name reaches the
//! hashes through every program-wide symbol, so the two are not interchangeable.
//!
//! A run needs one more thing under tier-only (ADR 0048): the compiled tier is the only evaluator,
//! so a fixture that is *run* — as opposed to only scheduled — carries its module source texts and
//! hands them to the whole Ply emitter through [`Compiled::tier`].

use ply_eval::{Exploration, host::HostUse};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use ply_test::{BackendUse, Engine, Executor, InterpExecutor, Worker};
use ply_ty::CheckOutput;
use std::collections::HashMap;

pub struct Compiled {
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// Kept rather than dropped: a `Front` without them disables every hybrid silently, since
    /// `bodies_available` answers false and the bisection reports no mixture instead of failing.
    pub bodies: ply_hash::body::BodySet,
    /// Each module's source text, keyed by `m.name.to_string()` — what the whole Ply emitter
    /// re-parses to produce bodies, since it is a front end rather than an AST consumer.
    pub texts: HashMap<String, String>,
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
        let name = module.name.to_string();
        Compiled::of(
            ply_syntax::ast::Program::single(module),
            HashMap::from([(name, src.to_string())]),
        )
    }

    /// Several modules, each one's `SourceId` its position in `sources`.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let inputs: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
            .collect();
        let texts = sources
            .iter()
            .map(|(name, src)| {
                (
                    ModuleName::from_dotted(name).to_string(),
                    (*src).to_string(),
                )
            })
            .collect();
        Compiled::of(
            ply_syntax::parse_program(inputs)
                .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}")),
            texts,
        )
    }

    #[track_caller]
    fn of(mut program: Program, texts: HashMap<String, String>) -> Compiled {
        let resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        // Each module under the name it was parsed with, in the program's own order: `anonymous`
        // keeps the empty name `ply_syntax::parse` gives it and `modules` keeps `m`, which is the
        // distinction this file's header names and the reason the two are not interchangeable.
        let named: Vec<(String, String)> = program
            .modules
            .iter()
            .map(|m| {
                let name = m.name.to_string();
                let text = texts.get(&name).cloned().unwrap_or_default();
                (name, text)
            })
            .collect();
        let ids: Vec<_> = (0..named.len()).map(|i| SourceId(i as u32)).collect();
        ply_codegen::c::producer::ensure_default();
        let front = ply_codegen::c::producer::front(&named, &ids)
            .unwrap_or_else(|e| panic!("the port answers for the fixture: {e:#}"));
        assert!(
            front.diagnostics.is_empty(),
            "the fixture must typecheck: {:#?}",
            front.diagnostics
        );
        let check = front.check;
        let (hashes, bodies) = ply_hash::hash_program_with_bodies(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
            hashes,
            bodies,
            texts,
        }
    }

    /// The Rust chain's answer as a [`ply_ty::Front`], which is what `diagnose_failures` takes.
    pub fn front(&self) -> ply_ty::Front {
        ply_codegen::source::front_of(
            &self.program,
            &self.resolved,
            &self.check,
            self.hashes.clone(),
            Some(&self.bodies),
        )
    }

    /// Every test's footprint, owned, so a caller may take one from a temporary.
    pub fn footprints(&self) -> Vec<ply_ty::Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }

    /// The whole Ply emitter's unit for this program, and the C backend spec to install it with —
    /// what a run needs under tier-only, since a bare machine holds no evaluator. `Unit::over`
    /// alone gets only the reference fragment, which holds no `perform`/`handle`/`simulate`; the
    /// texts are what the emitter re-parses into bodies. Leaks a `&'static` unit, which a test may.
    pub fn tier(&self) -> (&'static ply_codegen::Unit, ply_eval::BackendSpec) {
        ply_codegen::c::producer::ensure_default();
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
        (unit, spec)
    }
}

/// Wraps an [`InterpExecutor`] carrying the whole Ply tier so it presents as the evaluator. Under
/// tier-only the C tier is the sole engine; a run's cache is keyed by the bare test hash in the
/// [`Engine::Evaluator`] namespace, which is what every `select(.., Engine::Evaluator)` and
/// `store.get(hash)` in these tests reads. Reporting the backend's own engine would move each pass
/// into a namespace nothing here reads, so the engine the tier stands in for is the one it names.
pub struct TierExecutor<'a>(pub InterpExecutor<'a>);

impl<'a> Executor for TierExecutor<'a> {
    type Worker = Worker<'a>;

    fn worker(&self) -> Self::Worker {
        self.0.worker()
    }

    fn execute(&self, worker: &mut Self::Worker, index: usize) -> Result<(), Diagnostic> {
        self.0.execute(worker, index)
    }

    fn engine(&self) -> Engine {
        Engine::Evaluator
    }

    fn exploration(&self, worker: &Self::Worker) -> Option<Exploration> {
        self.0.exploration(worker)
    }

    fn host_use(&self, worker: &Self::Worker) -> Option<HostUse> {
        self.0.host_use(worker)
    }

    fn backend_use(&self, worker: &Self::Worker) -> Option<BackendUse> {
        self.0.backend_use(worker)
    }

    fn teardown(&self, worker: &mut Self::Worker) -> Vec<Diagnostic> {
        self.0.teardown(worker)
    }
}
