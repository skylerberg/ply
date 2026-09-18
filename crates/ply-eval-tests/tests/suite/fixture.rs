use ply_core::check_program;
use ply_eval::{Machine, Provider};
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use ply_ty::CheckOutput;
use std::collections::HashMap;

pub struct Compiled {
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    /// Keyed by `m.name.to_string()`: the Ply emitter re-parses source text, not the AST.
    pub texts: HashMap<String, String>,
}

impl Compiled {
    /// One module, named `m`.
    #[track_caller]
    pub fn new(source: &str) -> Compiled {
        Compiled::modules(&[("m", source)])
    }

    /// One module under the name its assertions spell.
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

    /// The checker's diagnostics, empty if it accepted; failing to parse or resolve panics.
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

    /// A machine with a compiled tier attached: a bare machine holds no evaluator.
    pub fn machine(&self) -> Machine<'_> {
        let mut m = Machine::new(&self.program, &self.resolved, &self.check);
        let unit =
            ply_codegen::Unit::over_with_texts(&self.program, &self.resolved, self.texts.clone())
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
