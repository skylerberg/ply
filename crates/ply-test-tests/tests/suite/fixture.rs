use ply_eval::{Exploration, host::HostUse};
use ply_hash::HashOutput;
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use ply_test::{BackendUse, Engine, Executor, InterpExecutor, Worker};
use ply_ty::CheckOutput;
use std::collections::HashMap;

#[track_caller]
pub fn port_check(sources: &[(String, String)], ids: &[SourceId]) -> CheckOutput {
    ply_codegen::c::producer::checked_front(sources, ids)
        .unwrap_or_else(|e| panic!("the fixture must typecheck: {e:#}"))
        .check
}

/// What the port raises over these modules, for a fixture meant to be refused.
#[track_caller]
pub fn port_diagnostics(sources: &[(String, String)], ids: &[SourceId]) -> Vec<Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::front(sources, ids)
        .unwrap_or_else(|e| panic!("the port answers for the fixture: {e:#}"))
        .diagnostics
}

pub struct Compiled {
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// A `Front` without bodies silently disables every hybrid rather than failing.
    pub bodies: ply_hash::body::BodySet,
    /// Keyed by `m.name.to_string()`; the Ply emitter re-parses these rather than reading the AST.
    pub texts: HashMap<String, String>,
}

impl Compiled {
    /// One module named `m`.
    #[track_caller]
    pub fn new(src: &str) -> Compiled {
        Compiled::modules(&[("m", src)])
    }

    /// Named as `ply_syntax::parse` names it, not `m`; the module name reaches every hash.
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
        let sources: Vec<(String, String)> = program
            .modules
            .iter()
            .map(|m| {
                let name = m.name.to_string();
                let text = texts
                    .get(&name)
                    .unwrap_or_else(|| panic!("no source text for module {name:?}"));
                (name, text.clone())
            })
            .collect();
        let ids: Vec<SourceId> = program.modules.iter().map(|m| m.source).collect();
        let check = port_check(&sources, &ids);
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

    pub fn front(&self) -> ply_ty::Front {
        ply_codegen::source::front_of(
            &self.program,
            &self.resolved,
            &self.check,
            self.hashes.clone(),
            Some(&self.bodies),
        )
    }

    pub fn footprints(&self) -> Vec<ply_ty::Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }

    /// Leaks the `&'static` unit. A bare machine holds no evaluator, so every run needs this.
    pub fn tier(&self) -> (&'static ply_codegen::Unit, ply_eval::BackendSpec) {
        let unit = ply_codegen::Unit::over_with_texts(&self.program, self.texts.clone())
            .expect("this host has a C compiler");
        let spec = ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
            ..Default::default()
        };
        (unit, spec)
    }
}

/// Reports `Engine::Evaluator`, the cache namespace every `select` and `store.get` here reads.
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
