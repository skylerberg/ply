//! The `std.random` facility: entropy from the operating system.
//!
//! The prelude's `random` is the scheduler's, answered with values a seed decides so a simulation
//! is reproducible; a run that is not simulated has this instead, and it is the host's because
//! `/dev/urandom` is the only source a run has.

use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, Linearity,
    Span, Symbol, Value, codes,
};
// `fill` is the trait's method, not the struct's.
use ring::rand::SecureRandom;
use std::sync::Arc;

pub const MODULE: &str = "std.random";

pub const EFFECT: &str = "std.random.entropy";

pub struct RandomHost {
    rng: ring::rand::SystemRandom,
}

impl Default for RandomHost {
    fn default() -> RandomHost {
        RandomHost::new()
    }
}

impl RandomHost {
    pub fn new() -> RandomHost {
        RandomHost {
            rng: ring::rand::SystemRandom::new(),
        }
    }

    /// Eight bytes, with the sign bit clear, so a draw is never negative: an `Int` that a program
    /// can put in a list or a bound without asking what its sign means.
    pub fn next(&self) -> Result<i64, Diagnostic> {
        let mut bytes = [0u8; 8];
        self.rng.fill(&mut bytes).map_err(|_| no_entropy())?;
        Ok((u64::from_be_bytes(bytes) >> 1) as i64)
    }

    /// Uniformly below `n`: a value at or past the largest whole multiple of `n` is drawn again,
    /// because folding it into range with a remainder would make the low values likelier.
    pub fn below(&self, n: i64) -> Result<i64, Diagnostic> {
        if n <= 0 {
            return Err(bad_bound(n));
        }
        let bound = n as u64;
        let limit = u64::MAX - (u64::MAX % bound);
        loop {
            let mut bytes = [0u8; 8];
            self.rng.fill(&mut bytes).map_err(|_| no_entropy())?;
            let drawn = u64::from_be_bytes(bytes);
            if drawn < limit {
                return Ok((drawn % bound) as i64);
            }
        }
    }

    /// `size` bytes drawn straight into a `Secret`, so no plain value of the program ever holds
    /// them: a key's material.
    pub fn secret(&self, size: i64) -> Result<Value, Diagnostic> {
        let size = usize::try_from(size)
            .ok()
            .filter(|n| *n <= MAX_SECRET)
            .ok_or_else(|| bad_size(size))?;
        let mut drawn = zeroize::Zeroizing::new(vec![0u8; size]);
        self.rng.fill(&mut drawn).map_err(|_| no_entropy())?;
        Ok(Value::secret_bytes(&drawn))
    }
}

/// The most bytes one `entropy.secret` draws: far past any key, and short of a mistake in a size.
pub const MAX_SECRET: usize = 1 << 16;

/// The handler both operations share: one source of entropy for the run, not one per call.
struct Entropy {
    host: Arc<RandomHost>,
}

impl HostHandler for Entropy {
    fn call(
        &self,
        _: &dyn ply_eval::HostRuntime,
        req: &HostRequest<'_>,
    ) -> Result<HostAnswer, Diagnostic> {
        match req.op.op.as_str() {
            "next" => Ok(HostAnswer::Value(Value::Int(self.host.next()?))),
            "below" => {
                let n = req.args[0].as_int(req.span, "`entropy.below`")?;
                Ok(HostAnswer::Value(Value::Int(self.host.below(n)?)))
            }
            "secret" => {
                let n = req.args[0].as_int(req.span, "`entropy.secret`")?;
                Ok(HostAnswer::Value(self.host.secret(n)?))
            }
            other => Err(unknown_op(other, req.span)),
        }
    }
}

/// `std.random`'s operations, as the declarations `std.random` writes them.
pub fn registrations() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let host = Arc::new(RandomHost::new());
    Op::ALL
        .iter()
        .map(|op| {
            (
                op.declaration(),
                Arc::new(Entropy {
                    host: Arc::clone(&host),
                }) as Arc<dyn HostHandler>,
            )
        })
        .collect()
}

pub fn register(registry: &mut HostRegistry) {
    for (op, handler) in registrations() {
        registry.register(op, handler);
    }
}

operations! {
    what "random";
    path "random";
    Next = "next",
    Below = "below",
    Secret = "secret",
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // `Any`, like every other facility: a program that does not import `std.random` does
            // not declare the effect, and a registration for an undeclared effect is skipped rather
            // than refused.
            resource: HostResource::Any,
            // What a draw is worth is not a function of what the program has done.
            determinism: Determinism::Nondeterministic,
            // Every draw is independent, so a run may take as many as it likes.
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

fn no_entropy() -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        "the operating system gave this run no entropy",
    )
    .note("`std.random` reads the system's, and there is no second source")
}

fn bad_bound(n: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`entropy.below` needs a bound above zero, but got {n}"),
    )
    .primary(Span::DUMMY, "this bound names no range")
    .note("a bound of zero has no value below it, and a negative one has no meaning here")
}

fn bad_size(n: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`entropy.secret` draws 0 to {MAX_SECRET} bytes, and was asked for {n}"),
    )
    .primary(Span::DUMMY, "this size is no key's")
}

fn unknown_op(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` reached a host that cannot answer it"),
    )
    .primary(span, "performed here")
    .note("a defect in Ply's registration rather than in the program")
}
