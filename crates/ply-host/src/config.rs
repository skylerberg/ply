//! What `config.get` and `config.secret` answer from: the snapshot the program resolved before
//! anything was bound.

use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_eval::{Diagnostic, Span, Symbol, Value, codes};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const MODULE: &str = "std.config";

pub const EFFECT: &str = "std.config.config";

operations! {
    what "config";
    path "config";
    Get = "get",
    Secret = "secret",
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            // The environment is not a function of program state; a `det` test supplies values.
            determinism: Determinism::Nondeterministic,
            // True only because `Snapshot` is immutable.
            linearity: Linearity::Repeatable,
            // Every source was read before this handler existed.
            blocking: false,
            // `config.secret` answers a `Secret`; neither operation is handed one.
            secrets: false,
            path: self.path(),
        }
    }
}

/// One resolved key: its value, and whether the run's schema declared it secret.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub value: String,
    pub secret: bool,
}

/// The frozen configuration of one run, as the program resolved it before anything was bound.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Snapshot {
    values: BTreeMap<String, Entry>,
    /// Whether the run named a `--config-schema`: without one no key is secret.
    has_spec: bool,
}

impl Snapshot {
    /// What a run with no `--host` holds: nothing, whatever the environment has.
    pub fn unopened() -> Snapshot {
        Snapshot::default()
    }

    pub fn new(values: BTreeMap<String, Entry>, has_spec: bool) -> Snapshot {
        Snapshot { values, has_spec }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let entry = self.values.get(key)?;
        (!entry.secret).then_some(entry.value.as_str())
    }

    /// The plaintext behind `config.secret`, for the one caller that turns it into a `Secret`.
    pub fn plaintext(&self, key: &str) -> Option<&str> {
        let entry = self.values.get(key)?;
        (!self.has_spec || entry.secret).then_some(entry.value.as_str())
    }

    pub fn has_spec(&self) -> bool {
        self.has_spec
    }
}

pub fn register(registry: &mut HostRegistry, snapshot: Arc<Snapshot>) {
    for op in Op::ALL {
        registry.register(
            op.declaration(),
            Arc::new(Operation {
                op,
                snapshot: Arc::clone(&snapshot),
            }),
        );
    }
}

pub fn registry(snapshot: Arc<Snapshot>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    register(&mut registry, snapshot);
    registry
}

struct Operation {
    op: Op,
    snapshot: Arc<Snapshot>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let [key] = req.args else {
            return Err(arity(self.op, req.args.len(), span));
        };
        let key = key.as_str(span, "a configuration key")?;
        Ok(HostAnswer::Value(match self.op {
            Op::Get => option(self.snapshot.get(key).map(|v| Value::Str(v.into()))),
            Op::Secret => option(self.snapshot.plaintext(key).map(secret)),
        }))
    }
}

/// The one place in this crate a Ply-level `Secret` is built.
fn secret(plain: &str) -> Value {
    Value::secret(Value::Str(plain.into()))
}

fn option(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::Ctor {
            name: Symbol::new("Some"),
            args: Arc::new(vec![value]),
        },
        None => Value::Ctor {
            name: Symbol::new("None"),
            args: Arc::new(Vec::new()),
        },
    }
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{} was performed with {got} arguments and takes 1", op.what()),
    )
    .primary(span, "this perform reached the configuration snapshot")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}
