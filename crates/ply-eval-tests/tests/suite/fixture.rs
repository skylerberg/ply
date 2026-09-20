use ply_eval::{Machine, Provider};
use ply_span::{Diagnostic, SourceId};
use ply_ty::ModuleName;
use ply_ty::{CheckOutput, Front};
use std::collections::HashMap;

/// `sources[i]` is `(module name, text)` for `SourceId(i)`.
#[track_caller]
pub fn port_front(sources: &[(&str, &str)]) -> Front {
    let (named, ids) = inputs(sources);
    ply_codegen::c::producer::checked_front(&named, &ids)
        .unwrap_or_else(|e| panic!("the fixture must typecheck: {e:#}"))
}

#[track_caller]
pub fn port_check(sources: &[(&str, &str)]) -> CheckOutput {
    port_front(sources).check
}

/// Every diagnostic when any is an error, as a refusing checker answers; empty otherwise.
#[track_caller]
pub fn port_errors(sources: &[(&str, &str)]) -> Vec<Diagnostic> {
    let (named, ids) = inputs(sources);
    ply_codegen::c::producer::ensure_default();
    let front = ply_codegen::c::producer::front(&named, &ids)
        .unwrap_or_else(|e| panic!("the port answers for the fixture: {e:#}"));
    if front.has_error() {
        front.diagnostics
    } else {
        Vec::new()
    }
}

fn inputs(sources: &[(&str, &str)]) -> (Vec<(String, String)>, Vec<SourceId>) {
    let named = sources
        .iter()
        .map(|(name, src)| {
            (
                ModuleName::from_dotted(name).to_string(),
                (*src).to_string(),
            )
        })
        .collect();
    let ids = (0..sources.len()).map(|i| SourceId(i as u32)).collect();
    (named, ids)
}

pub struct Compiled {
    pub front: Front,
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
        let front = port_front(sources);
        let texts = sources
            .iter()
            .map(|(name, src)| {
                (
                    ModuleName::from_dotted(name).to_string(),
                    (*src).to_string(),
                )
            })
            .collect();
        Compiled { front, texts }
    }

    /// The port's diagnostics, empty if it accepted.
    #[track_caller]
    pub fn rejected(source: &str) -> Vec<Diagnostic> {
        Compiled::rejected_in("m", source)
    }

    /// [`Compiled::rejected`] under the module name the assertions spell.
    #[track_caller]
    pub fn rejected_in(module: &str, source: &str) -> Vec<Diagnostic> {
        port_errors(&[(module, source)])
    }

    /// A machine with a compiled tier attached: a bare machine holds no evaluator.
    pub fn machine(&self) -> Machine<'_> {
        ply_codegen::c::producer::ensure_default();
        let mut m = Machine::new(&self.front);
        let unit = ply_codegen::Unit::over_front(&self.front, self.texts.clone())
            .expect("this host has a C compiler");
        let spec = ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
        };
        m.set_compiled(unit.attach(&spec));
        m
    }

    pub fn machine_on_tier(&self) -> Machine<'_> {
        self.machine()
    }

    pub fn index_of(&self, name: &str) -> usize {
        self.front
            .check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }
}
