//! A backend a shipping command can attach, and eight ways of being wrong.

use crate::compiled::Compiled;
use crate::value::Value;
use ply_span::Symbol;
use ply_syntax::ast::Program;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What one run's backend was asked, summed over every worker.
#[derive(Default)]
pub struct Counters {
    offered: AtomicU64,
    offered_target: AtomicU64,
    fired: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    str_in: AtomicU64,
    str_out: AtomicU64,
    containers_out: AtomicU64,
    converted_in: AtomicU64,
    converted_out: AtomicU64,
}

impl Counters {
    /// One offer, counted, and whether it carried a `Bytes` or a `String` in.
    pub fn note_offer(&self, args: &[Value]) {
        self.offered.fetch_add(1, Ordering::Relaxed);
        if args.iter().any(|a| matches!(a, Value::Bytes(_))) {
            self.bytes_in.fetch_add(1, Ordering::Relaxed);
        }
        if args.iter().any(|a| matches!(a, Value::Str(_))) {
            self.str_in.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// An answered [`Value::Bytes`], counted before any mutation touches it.
    pub fn note_bytes_out(&self) {
        self.bytes_out.fetch_add(1, Ordering::Relaxed);
    }

    /// An answered [`Value::Str`], counted before any mutation touches it.
    pub fn note_str_out(&self) {
        self.str_out.fetch_add(1, Ordering::Relaxed);
    }

    /// An offer naming the one definition a targeted mutation corrupts.
    pub fn note_offered_target(&self) {
        self.offered_target.fetch_add(1, Ordering::Relaxed);
    }

    /// An answer a mutation actually changed.
    pub fn note_fired(&self) {
        self.fired.fetch_add(1, Ordering::Relaxed);
    }

    pub fn offers(&self) -> Offers {
        Offers {
            offered: self.offered.load(Ordering::Relaxed),
            offered_target: self.offered_target.load(Ordering::Relaxed),
            fired: self.fired.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            str_in: self.str_in.load(Ordering::Relaxed),
            str_out: self.str_out.load(Ordering::Relaxed),
            containers_out: self.containers_out.load(Ordering::Relaxed),
            converted_in: self.converted_in.load(Ordering::Relaxed),
            converted_out: self.converted_out.load(Ordering::Relaxed),
        }
    }

    /// The seam's census (ADR 0035 Decision 6): the objects one entry built from its arguments
    /// and the objects it read back out of its answer.
    pub fn note_converted(&self, inward: u64, outward: u64) {
        self.converted_in.fetch_add(inward, Ordering::Relaxed);
        self.converted_out.fetch_add(outward, Ordering::Relaxed);
    }

    /// One answer that carried a container out.
    pub fn note_container_out(&self) {
        self.containers_out.fetch_add(1, Ordering::Relaxed);
    }
}

/// What a run's backend was asked and what it did with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offers {
    /// Calls the machine offered this backend, over every worker.
    pub offered: u64,
    /// Offers naming the one definition a targeted mutation corrupts.
    pub offered_target: u64,
    /// Calls whose answer a mutation actually changed.
    pub fired: u64,
    /// Offers carrying at least one [`Value::Bytes`] argument.
    pub bytes_in: u64,
    /// Entered calls that answered a [`Value::Bytes`], counted before any mutation touches the
    /// answer.
    pub bytes_out: u64,
    /// Offers carrying at least one [`Value::Str`] argument.
    pub str_in: u64,
    /// Entered calls that answered a [`Value::Str`], counted before any mutation touches the
    /// answer.
    pub str_out: u64,
    /// Entered calls that answered a `List`, `Map`, `Record` or `Ctor`, counted before any mutation
    /// touches the answer.
    pub containers_out: u64,
    /// Objects the entries built from their arguments — every value converted at the root that
    /// was not an immediate — and objects read back out of their answers: the seam's census,
    /// so a run whose conversion dominates is visible rather than inferred.
    pub converted_in: u64,
    pub converted_out: u64,
}

/// A run's source of backends: one per run, shared by every worker, and the only route a shipping
/// command has to install one.
pub trait Provider: Send + Sync {
    /// This provider's backend for one worker, corrupted as `spec` asks.
    fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled>;

    /// What `--backend` calls this, for a report a user reads.
    fn name(&self) -> &'static str;

    /// What decides this provider's answers beyond its name: anything that would make a result it
    /// earned untrue of another run under the same name. Empty when the name is the whole
    /// identity. A cached result is namespaced by it, so a knob that changes which definitions run
    /// natively belongs here.
    fn variant(&self) -> String {
        String::new()
    }

    /// How many definitions this provider has a body for.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What every worker's backend was asked, summed.
    fn offers(&self) -> Offers;

    /// What this provider spent compiling, or `None` for one that compiles nothing.
    fn compilation(&self) -> Option<Compilation> {
        None
    }

    /// Workers this provider could not build a backend for.
    fn unbuilt(&self) -> u64 {
        0
    }
}

/// What a run spent turning a program into compiled code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compilation {
    /// Nanoseconds deciding *what* to compile.
    pub analysis_nanos: u64,
    /// Nanoseconds inside the code generator, summed over every backend built.
    pub codegen_nanos: u64,
    /// Backends built.
    pub units: u64,
}

/// A backend the eight corruptions can wrap.
pub trait Policed: Compiled {
    /// The run's counters, which every backend over one provider shares.
    fn counters(&self) -> &'static Counters;

    /// Whether this backend has a body for `name` at all.
    fn holds(&self, name: &Symbol) -> bool;

    /// The honest answer for a call, **without** counting an offer.
    fn answer(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;

    /// The body on an arbitrary bound, whatever the machine allowed.
    fn run_with_fuel(&self, name: &Symbol, args: &[Value], fuel: usize) -> Option<Value>;
}

/// One worker's backend, corrupted as `spec` asks — the one place a [`Mutant`] is built, so that no
/// provider can install a wrapper the others do not.
pub fn wrap(inner: Rc<dyn Policed>, spec: &Spec) -> Rc<dyn Compiled> {
    match (&spec.mutation, &spec.target) {
        (Mutation::None, None) => inner,
        _ => Rc::new(Mutant {
            inner,
            mutation: spec.mutation.clone(),
            target: spec.target.clone(),
            previous: RefCell::new(None),
        }),
    }
}

/// One way of being wrong.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mutation {
    /// The honest answer, so a harness can check that the wrapper itself changes nothing.
    #[default]
    None,
    /// `Int(n)` becomes `Int(n + 1)`: the arithmetic is off by one.
    OffByOne,
    /// `Bool(b)` becomes `Bool(!b)`: the comparison is inverted.
    Inverted,
    /// This call answers what the *previous* entered call answered.
    Stale,
    /// The same information in the wrong kind — `Bool` where the definition returns `Int`, `Int`
    /// where it returns `Bool`, and the length where it returns `Bytes`.
    WrongType,
    /// Answers for a name this backend has no body for, instead of declining.
    Unoffered,
    /// Runs the body with more fuel than the machine allowed instead of declining.
    ExceedsBudget(Option<u32>),
    /// Answers this value for the target name whatever the machine asked, and whether or not a body
    /// exists.
    Answers(i64),
    /// A handle into this run's world, inside an otherwise honest container answer: the first field
    /// of a `Record`, the first argument of a `Ctor`, or the head of a `List` is replaced by a
    /// `Value::Cell`.
    Handle,
}

impl Mutation {
    pub fn describe(&self) -> String {
        match self {
            Mutation::None => "nothing".to_string(),
            Mutation::OffByOne => "every `Int` answer is one too high".to_string(),
            Mutation::Inverted => "every `Bool` answer is inverted".to_string(),
            Mutation::Stale => "every answer is the previous call's".to_string(),
            Mutation::WrongType => {
                "`Int` answers come back as `Bool` and back, and `Bytes` as its length".to_string()
            }
            Mutation::Unoffered => {
                "names with no compiled body are answered rather than declined".to_string()
            }
            Mutation::ExceedsBudget(None) => {
                "the machine's call budget is ignored entirely".to_string()
            }
            Mutation::ExceedsBudget(Some(k)) => {
                format!("bodies run with {k}x the budget the machine allowed")
            }
            Mutation::Answers(v) => format!("the target is answered `{v}` unconditionally"),
            Mutation::Handle => {
                "container answers come back holding a cell from this run's world".to_string()
            }
        }
    }
}

/// Which of the backends a command can install.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// `ply_codegen::c`: the program emitted as C and handed to `cc` — under tier-only, the one
    /// evaluator.
    #[default]
    C,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::C => "c",
        }
    }
}

/// Which backend a command was asked for, and which definition it corrupts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    /// Which backend answers.
    pub kind: Kind,
    pub mutation: Mutation,
    /// The one definition to corrupt, or every one of them.
    pub target: Option<Symbol>,
}

impl Spec {
    pub fn honest() -> Spec {
        Spec::default()
    }

    pub fn describe(&self) -> String {
        match &self.target {
            Some(name) => format!("{} (only `{name}`)", self.mutation.describe()),
            None => self.mutation.describe(),
        }
    }
}

/// Parses a `--backend` argument.
pub fn parse(spec: &str) -> Result<Spec, String> {
    // A bare backend name, honest.
    if spec == "c" {
        return Ok(Spec::honest());
    }
    let (backend, rest) = match spec.split_once(':') {
        Some(("c", rest)) => (Kind::C, rest),
        _ => (Kind::C, spec),
    };
    let Some(rest) = rest.strip_prefix("wrong:") else {
        return Err(format!(
            "unknown backend `{spec}`; one of `c` or `[c:]wrong:<mutation>` where <mutation> is \
             off-by-one, inverted, stale, wrong-type, unoffered, handle, exceeds-budget[={{k}}] or \
             answers={{int}}, each optionally @<definition>"
        ));
    };
    let (head, target) = match rest.split_once('@') {
        Some((head, target)) => (head, Some(Symbol::new(target))),
        None => (rest, None),
    };
    let (kind, argument) = match head.split_once('=') {
        Some((kind, argument)) => (kind, Some(argument)),
        None => (head, None),
    };
    let mutation = match (kind, argument) {
        ("off-by-one", None) => Mutation::OffByOne,
        ("inverted", None) => Mutation::Inverted,
        ("stale", None) => Mutation::Stale,
        ("wrong-type", None) => Mutation::WrongType,
        ("unoffered", None) => Mutation::Unoffered,
        ("handle", None) => Mutation::Handle,
        ("exceeds-budget", None) => Mutation::ExceedsBudget(None),
        ("exceeds-budget", Some(k)) => Mutation::ExceedsBudget(Some(
            k.parse()
                .map_err(|_| format!("`exceeds-budget={k}` needs a whole number"))?,
        )),
        ("answers", Some(v)) => Mutation::Answers(
            v.parse()
                .map_err(|_| format!("`answers={v}` needs a whole number"))?,
        ),
        _ => {
            return Err(format!(
                "unknown mutation `{head}`; one of off-by-one, inverted, stale, wrong-type, \
                 unoffered, handle, exceeds-budget[={{k}}], answers={{int}}, each optionally \
                 @<definition>"
            ));
        }
    };
    if matches!(mutation, Mutation::Answers(_)) && target.is_none() {
        return Err(
            "`wrong:answers=` needs a target: `wrong:answers=302@orders.measured`".to_string(),
        );
    }
    Ok(Spec {
        kind: backend,
        mutation,
        target,
    })
}

/// A backend that is wrong on purpose, wrapped around one that is not.
pub struct Mutant {
    inner: Rc<dyn Policed>,
    mutation: Mutation,
    target: Option<Symbol>,
    previous: RefCell<Option<Value>>,
}

impl Mutant {
    fn fire(&self, value: Value) -> Option<Value> {
        self.inner.counters().note_fired();
        Some(value)
    }
}

/// `value` with a world handle in its first position, or `None` if it has no position to put one
/// in.
fn forge_handle(value: &Value) -> Option<Value> {
    let handle = Value::Cell(crate::arena::Slot::new(0, 0));
    match value {
        Value::Record(fields) => {
            let mut fields = (**fields).clone();
            let key = fields.keys().next().cloned()?;
            fields.insert(key, handle);
            Some(Value::Record(std::sync::Arc::new(fields)))
        }
        Value::Ctor { name, args } if !args.is_empty() => {
            let mut args = (**args).clone();
            args[0] = handle;
            Some(Value::Ctor {
                name: name.clone(),
                args: std::sync::Arc::new(args),
            })
        }
        Value::List(items) if !items.is_empty() => {
            let mut items = items.to_vec();
            items[0] = handle;
            Some(Value::list(items))
        }
        _ => None,
    }
}

impl Compiled for Mutant {
    fn describes(&self, program: &Program) -> bool {
        self.inner.describes(program)
    }

    fn take_performed(&self) -> Vec<ply_ty::EffectAtom> {
        self.inner.take_performed()
    }

    fn set_seed(&self, seed: crate::sim::Seed, steps: u32) {
        self.inner.set_seed(seed, steps);
    }

    fn simulated(&self) -> Option<crate::region::Record> {
        self.inner.simulated()
    }

    fn set_host(
        &self,
        binding: std::sync::Arc<crate::host::HostBinding>,
        runtime: Option<std::rc::Rc<dyn crate::host::HostRuntime>>,
    ) {
        self.inner.set_host(binding, runtime);
    }

    fn set_declared(&self, declared: Option<ply_ty::Footprint>) {
        self.inner.set_declared(declared);
    }

    fn set_re_executed(&self, re_executed: bool) {
        self.inner.set_re_executed(re_executed);
    }

    fn take_host_use(&self) -> (crate::host::HostUse, u64) {
        self.inner.take_host_use()
    }

    fn take_teardown(&self) -> Vec<ply_span::Diagnostic> {
        self.inner.take_teardown()
    }

    fn tier_only(&self) -> bool {
        self.inner.tier_only()
    }

    /// A corruption is a wrong answer at the seam, and a test entered whole hands the seam
    /// nothing to corrupt: a mutant leaves every test to the machine, where each call crosses.
    fn enter_test(&self, _name: &Symbol, _budget: usize) -> crate::compiled::Entered {
        crate::compiled::Entered::Declined
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        let counters = self.inner.counters();
        counters.note_offer(args);
        if self.target.as_ref().is_some_and(|t| t != name) {
            return self.inner.answer(name, args, budget);
        }
        counters.note_offered_target();

        // The two that answer without an honest answer to corrupt.
        match &self.mutation {
            Mutation::Answers(value) => return self.fire(Value::Int(*value)),
            Mutation::Unoffered if !self.inner.holds(name) => {
                let invented = args
                    .iter()
                    .find(|v| matches!(v, Value::Int(_)))
                    .cloned()
                    .unwrap_or(Value::Int(0));
                return self.fire(invented);
            }
            _ => {}
        }

        let honest = self.inner.answer(name, args, budget);
        match (&self.mutation, honest) {
            (Mutation::None | Mutation::Unoffered, answer) => answer,
            (Mutation::OffByOne, Some(Value::Int(n))) => self.fire(Value::Int(n.wrapping_add(1))),
            (Mutation::Inverted, Some(Value::Bool(b))) => self.fire(Value::Bool(!b)),
            (Mutation::WrongType, Some(Value::Int(n))) => self.fire(Value::Bool(n != 0)),
            (Mutation::WrongType, Some(Value::Bool(b))) => self.fire(Value::Int(i64::from(b))),
            // The other leaf kinds the seam carries, and the arms that keep this mutation's claim
            // true of them.
            (Mutation::WrongType, Some(Value::Bytes(ref b))) => {
                self.fire(Value::Int(b.len() as i64))
            }
            (Mutation::WrongType, Some(Value::Str(ref s))) => self.fire(Value::Int(s.len() as i64)),
            (Mutation::WrongType, Some(Value::Unit)) => self.fire(Value::Int(0)),
            (Mutation::Handle, Some(value)) => match forge_handle(&value) {
                Some(forged) => self.fire(forged),
                // A scalar answer has nowhere to put one.
                None => Some(value),
            },
            (Mutation::Stale, Some(value)) => {
                let stale = self.previous.borrow_mut().replace(value.clone());
                match stale {
                    Some(stale) if stale.render() != value.render() => self.fire(stale),
                    // The first entry, or a repeat of the last answer: nothing to be stale about,
                    // so this call is honest and is not counted.
                    _ => Some(value),
                }
            }
            // A body that fits its budget answers the same either way; the mutation is only visible
            // where the honest backend declined.
            (Mutation::ExceedsBudget(times), None) if self.inner.holds(name) => {
                let fuel = match times {
                    None => usize::MAX,
                    Some(k) => budget.saturating_mul(*k as usize),
                };
                match self.inner.run_with_fuel(name, args, fuel) {
                    Some(value) => self.fire(value),
                    None => None,
                }
            }
            (_, answer) => answer,
        }
    }
}
