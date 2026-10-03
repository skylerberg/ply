//! One test, or one interleaving of one, run on whichever thread asks: a fresh machine over the
//! unit attached to this thread, the binding it may reach, and a Rust unwind out of it caught and
//! reported as Ply's defect at the test's source. What the run comes to is the program's to say.

use ply_eval::host::{HostBinding, HostUse};
use ply_eval::{Diagnostic, Interleaving, Seed, Span, codes};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a test may reach: the binding, and the runtime a pending host answer is polled on. A
/// runtime handle belongs to one thread, so it crosses as a factory.
#[derive(Clone, Default)]
pub struct Hosting {
    pub binding: Option<Arc<HostBinding>>,
    pub runtime: Option<ply_eval::RuntimeFactory>,
}

pub struct Executor<'a> {
    pub front: &'a ply_eval::Analysis,
    pub hosting: Hosting,
    pub provider: &'static dyn ply_eval::Provider,
}

impl<'a> Executor<'a> {
    /// The backend this thread runs the unit on, attached once per thread: a test is a fresh
    /// machine over it, never a fresh attachment.
    fn backend(&self) -> Rc<dyn ply_eval::Compiled> {
        thread_local! {
            static ATTACHED: std::cell::RefCell<Vec<(usize, Rc<dyn ply_eval::Compiled>)>> =
                const { std::cell::RefCell::new(Vec::new()) };
        }
        let key = std::ptr::from_ref(self.provider).cast::<()>() as usize;
        ATTACHED.with(|attached| {
            if let Some((_, backend)) = attached.borrow().iter().find(|(k, _)| *k == key) {
                return Rc::clone(backend);
            }
            let backend = self.provider.attach();
            attached.borrow_mut().push((key, Rc::clone(&backend)));
            backend
        })
    }

    fn machine(&self, index: usize) -> Result<ply_eval::Machine<'a>, Diagnostic> {
        let mut machine = ply_eval::Machine::new(self.front, self.backend())?;
        if let Some(binding) = &self.hosting.binding {
            machine.set_host_binding(Arc::clone(binding));
        }
        if let Some(runtime) = &self.hosting.runtime {
            machine.set_host_runtime(Arc::clone(runtime));
        }
        // The entry point's footprint claim, so a host answer outside it is `E0427`.
        if let Some(test) = self.front.check.tests.get(index) {
            machine.set_declared_footprint(test.footprint.clone());
        }
        Ok(machine)
    }
}

/// What running a test, or one interleaving of it, cost and reached, whatever it decided.
#[derive(Clone, Debug, Default)]
pub struct Use {
    pub duration: Duration,
    /// The run reached a nondeterministic host handler.
    pub host: bool,
    /// Bodies the test ran natively, and calls the backend was offered and declined.
    pub entries: u64,
    pub declines: u64,
    /// The operations the test performed, handled ones included.
    pub performs: u64,
    /// What the entry left behind: cycles teardown collected, and its own warnings.
    pub teardown: Vec<Diagnostic>,
}

/// One machine's entry, before anything is made of it.
struct Entered {
    outcome: Result<(), Diagnostic>,
    interleaving: Option<Interleaving>,
    usage: Use,
}

fn entered(
    executor: &Executor<'_>,
    index: usize,
    seeded: Option<(&Seed, u32, bool)>,
) -> Result<Entered, Diagnostic> {
    let mut machine = executor.machine(index)?;
    if let Some((seed, steps, re_executed)) = seeded {
        machine.set_re_executed(re_executed);
        machine.set_seed(seed.clone(), steps);
    }
    let (outcome, warnings) = machine.eval_test(index).into_parts();
    let (entries, declines) = machine.compiled_counts();
    let mut teardown = ply_eval::rc::take_cycles();
    teardown.extend(warnings);
    let interleaving = seeded.and_then(|_| {
        machine
            .simulated()
            .map(|record| record.interleaving(&outcome))
    });
    Ok(Entered {
        usage: Use {
            duration: Duration::ZERO,
            host: machine.host_use().is_some_and(HostUse::acted),
            entries,
            declines,
            performs: machine.trace().performs(),
            teardown,
        },
        interleaving,
        outcome,
    })
}

/// How one run of a test ended.
pub struct Executed {
    pub failure: Option<Diagnostic>,
    /// Ply unwound rather than the program failing.
    pub panicked: bool,
    pub usage: Use,
}

impl Executed {
    /// A test nothing could run: only the refusal is known.
    pub fn refused(refusal: Diagnostic) -> Executed {
        Executed {
            failure: Some(refusal),
            panicked: false,
            usage: Use::default(),
        }
    }
}

/// One test run once on this thread, an unwind out of it reported as Ply's defect at its source.
pub fn executed(executor: &Executor<'_>, index: usize) -> Executed {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| entered(executor, index, None)));
    let duration = started.elapsed();
    match result {
        Ok(Ok(e)) => Executed {
            failure: e.outcome.err(),
            panicked: false,
            usage: Use {
                duration,
                ..e.usage
            },
        },
        Ok(Err(refused)) => Executed::refused(refused),
        Err(payload) => Executed {
            failure: Some(panic_diagnostic(payload, executor.front, index)),
            panicked: true,
            usage: Use {
                duration,
                ..Use::default()
            },
        },
    }
}

/// One interleaving of a seeded test, the one `seed` names, on this thread.
pub struct Interleaved {
    pub interleaving: Interleaving,
    /// The test entered a `simulate` region, and so had a schedule to vary.
    pub observed: bool,
    pub panicked: bool,
    pub usage: Use,
}

impl Interleaved {
    pub fn refused(refusal: Diagnostic) -> Interleaved {
        Interleaved {
            interleaving: Interleaving::failed(Vec::new(), refusal),
            observed: false,
            panicked: false,
            usage: Use::default(),
        }
    }
}

pub fn interleaved(
    executor: &Executor<'_>,
    index: usize,
    seed: &Seed,
    steps: u32,
    re_executed: bool,
) -> Interleaved {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| {
        entered(executor, index, Some((seed, steps, re_executed)))
    }));
    let duration = started.elapsed();
    match result {
        Ok(Ok(e)) => Interleaved {
            observed: e.interleaving.is_some(),
            interleaving: match e.interleaving {
                Some(interleaving) => interleaving,
                None => match e.outcome {
                    Ok(()) => Interleaving::passed(Vec::new()),
                    Err(diagnostic) => Interleaving::failed(Vec::new(), diagnostic),
                },
            },
            panicked: false,
            usage: Use {
                duration,
                ..e.usage
            },
        },
        Ok(Err(refused)) => Interleaved {
            usage: Use {
                duration,
                ..Use::default()
            },
            ..Interleaved::refused(refused)
        },
        Err(payload) => Interleaved {
            panicked: true,
            usage: Use {
                duration,
                ..Use::default()
            },
            ..Interleaved::refused(panic_diagnostic(payload, executor.front, index))
        },
    }
}

fn panic_diagnostic(
    payload: Box<dyn Any + Send>,
    front: &ply_eval::Analysis,
    index: usize,
) -> Diagnostic {
    let message = if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_string()
    };
    let (name, span) = match front.check.tests.get(index) {
        Some(t) => (t.key.to_string(), t.span),
        None => (format!("test {index}"), Span::DUMMY),
    };
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("test `{name}` panicked: {message}"),
    )
    .primary(span, "the interpreter panicked while running this test")
    .note("a panic is a defect in Ply itself, not in the test; please report it with this source")
    .note("the other tests still ran, and this one was not cached")
}

/// How a report classes a run that ended with `failure`.
pub fn status_word(failure: Option<&Diagnostic>, panicked: bool) -> &'static str {
    match failure {
        None => "passed",
        Some(d) if d.code == codes::RUN_ABANDONED => "abandoned",
        Some(d) if panicked || codes::is_defect(d.code) => "panicked",
        Some(_) => "failed",
    }
}
