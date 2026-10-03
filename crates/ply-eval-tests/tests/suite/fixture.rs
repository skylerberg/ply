use ply_eval::{Analysis, CheckOutput, Diagnostic, Machine, Provider};
use std::rc::Rc;

/// What the builder makes of `sources`, `(module name, text)` each written to the file its name
/// spells: the front end's answer, a refusal's included, and the unit's C.
#[track_caller]
fn answered(sources: &[(&str, &str)]) -> ply_machine::runnable::Runnable {
    let files: Vec<(String, String)> = sources
        .iter()
        .map(|(name, src)| {
            (
                format!("{}.ply", name.replace('.', "/")),
                (*src).to_string(),
            )
        })
        .collect();
    let bytes = ply_machine::builds::answered(&files)
        .unwrap_or_else(|d| panic!("the builder answers for the fixture: {}", d.message));
    ply_machine::runnable::decode(&bytes)
        .unwrap_or_else(|why| panic!("the builder's answer reads: {why}"))
}

#[track_caller]
fn accepted(sources: &[(&str, &str)]) -> ply_machine::runnable::Runnable {
    let answer = answered(sources);
    let errors: Vec<String> = answer
        .front
        .answer
        .diagnostics
        .iter()
        .filter(|d| d.severity == ply_eval::Severity::Error)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    assert!(errors.is_empty(), "the fixture must typecheck: {errors:?}");
    answer
}

#[track_caller]
pub fn port_front(sources: &[(&str, &str)]) -> Analysis {
    accepted(sources).front.answer
}

#[track_caller]
pub fn port_check(sources: &[(&str, &str)]) -> CheckOutput {
    port_front(sources).check
}

/// Every diagnostic when any is an error, as a refusing checker answers; empty otherwise.
#[track_caller]
pub fn port_errors(sources: &[(&str, &str)]) -> Vec<Diagnostic> {
    let front = answered(sources).front.answer;
    if front.has_error() {
        front.diagnostics
    } else {
        Vec::new()
    }
}

pub struct Compiled {
    pub front: Analysis,
    /// The unit's C, every definition offered.
    unit: String,
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

    /// Several modules.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let answer = accepted(sources);
        Compiled {
            front: answer.front.answer,
            unit: answer.unit,
        }
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

    /// A machine on the backend compiled from this program.
    pub fn machine(&self) -> Machine<'_> {
        self.machine_on(self.unit().attach())
    }

    /// The unit compiled from this program; each [`Provider::attach`] is a backend of its own.
    pub fn unit(&self) -> &'static ply_codegen::Unit {
        ply_codegen::Unit::handed(&self.front, self.unit.clone())
            .expect("this host has a C compiler")
    }

    /// A machine on `backend`, attached from [`Compiled::unit`].
    pub fn machine_on(&self, backend: Rc<dyn ply_eval::Compiled>) -> Machine<'_> {
        Machine::new(&self.front, backend).expect("the backend was compiled from this program")
    }

    /// The unit over every definition, loaded bare, so a test enters its bodies without a machine.
    pub fn native(&self) -> ply_codegen::c::Native {
        let front: &'static Analysis = Box::leak(Box::new(self.front.clone()));
        let source: &'static ply_codegen::Source =
            Box::leak(Box::new(ply_codegen::Source::from_analysis(front)));
        ply_codegen::c::load_unit(&self.unit, Some(source), "unit")
            .expect("the unit loads")
            .0
    }

    pub fn machine_on_backend(&self) -> Machine<'_> {
        self.machine()
    }

    /// [`Compiled::machine`], and the backend it runs on, which counts what it declined and why.
    pub fn machine_and_backend(&self) -> (Machine<'_>, Rc<ply_codegen::Bodies>) {
        let backend = self.unit().bodies().expect("the unit builds");
        (self.machine_on(backend.clone()), backend)
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
