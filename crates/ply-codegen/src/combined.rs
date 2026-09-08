//! The engine that makes the machine unnecessary (ADR 0047): the two front ends over one runtime,
//! chosen per body. The interpreted front end runs what it carries — the first-order language,
//! cells, tail-resumptive effects — with no C compiler; anything it declines, the compiled front
//! end runs, since the C tier already carries `simulate`, a region's tasks and multi-shot
//! `resume` (ADR 0044). Neither is the machine, and between them they answer every body.
//!
//! It is a `Provider` like `Unit` and `Interpreter`, so a consumer attaches it exactly as it
//! attaches either alone, and `--audit-backend` pairs the pair against the machine while the
//! machine still exists to be the oracle.

use crate::backend::Unit;
use anyhow::Result;
use ply_core::Footprint;
use ply_core::ty::EffectAtom;
use ply_eval::host::{HostBinding, HostRuntime, HostUse};
use ply_eval::interp::Interpreter;
use ply_eval::region::Record;
use ply_eval::{BackendSpec, Offers, Provider, Seed, Value};
use ply_eval::{Compiled, Entered};
use ply_span::{Diagnostic, Symbol};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// The interpreter and the compiled unit over one program.
pub struct Combined {
    interp: &'static Interpreter,
    unit: &'static Unit,
}

impl Combined {
    pub fn build(
        program: &ply_syntax::ast::Program,
        resolved: &ply_syntax::resolve::Resolved,
        check: &ply_core::CheckOutput,
        keys: std::collections::HashMap<String, String>,
        texts: std::collections::HashMap<String, String>,
    ) -> Result<&'static Combined> {
        let interp = Interpreter::over(program, resolved, check);
        let unit = Unit::keyed(program, resolved, check, keys, texts)?;
        Ok(Box::leak(Box::new(Combined { interp, unit })))
    }
}

impl Provider for Combined {
    fn attach(&'static self, spec: &BackendSpec) -> Rc<dyn Compiled> {
        Rc::new(Pair {
            interp: self.interp.attach(spec),
            compiled: self.unit.attach(spec),
            last: Cell::new(Ran::Neither),
            audit: std::env::var("PLY_COMBINED_AUDIT").is_ok(),
        })
    }

    fn name(&self) -> &'static str {
        "combined"
    }

    fn len(&self) -> usize {
        self.unit.len()
    }

    fn offers(&self) -> Offers {
        self.unit.offers()
    }
}

/// The two front ends must answer a body the same way; a difference is the oracle firing.
fn disagree(name: &Symbol, interp: &Entered, compiled: &Entered) -> Option<Diagnostic> {
    use ply_span::codes;
    let same = match (interp, compiled) {
        (Entered::Answered(a), Entered::Answered(b)) => {
            ply_eval::values_equal(a, b, ply_span::Span::DUMMY).unwrap_or(false)
        }
        (Entered::Raised(_), Entered::Raised(_)) => true,
        (Entered::Declined, Entered::Declined) => true,
        _ => false,
    };
    if same {
        None
    } else {
        Some(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("the interpreted and compiled front ends disagree on `{name}`"),
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ran {
    Neither,
    Interp,
    Compiled,
}

/// One attached pair. An entry tries the interpreter first; a body it declines runs on the
/// compiled front end. The ambient state — seed, host, declared footprint — is set on both, and
/// what an entry produced afterwards (its performed atoms, its record, its host use) is read from
/// whichever front end ran it.
struct Pair {
    interp: Rc<dyn Compiled>,
    compiled: Rc<dyn Compiled>,
    last: Cell<Ran>,
    /// `PLY_COMBINED_AUDIT`: run a body both ways and compare, the oracle that replaces
    /// pairing against the machine once the machine is gone.
    audit: bool,
}

impl Pair {
    fn route_test(&self, name: &Symbol, budget: usize) -> Entered {
        let interp = self.interp.enter_test(name, budget);
        if let Entered::Declined = interp {
            self.last.set(Ran::Compiled);
            return self.compiled.enter_test(name, budget);
        }
        self.last.set(Ran::Interp);
        if self.audit {
            let compiled = self.compiled.enter_test(name, budget);
            if let Some(d) = disagree(name, &interp, &compiled) {
                return Entered::Raised(d);
            }
        }
        interp
    }

    fn route_whole(&self, name: &Symbol, args: &[Value], budget: usize) -> Entered {
        let interp = self.interp.enter_whole(name, args, budget);
        if let Entered::Declined = interp {
            self.last.set(Ran::Compiled);
            return self.compiled.enter_whole(name, args, budget);
        }
        self.last.set(Ran::Interp);
        if self.audit {
            let compiled = self.compiled.enter_whole(name, args, budget);
            if let Some(d) = disagree(name, &interp, &compiled) {
                return Entered::Raised(d);
            }
        }
        interp
    }

    fn ran(&self) -> &Rc<dyn Compiled> {
        match self.last.get() {
            Ran::Interp => &self.interp,
            _ => &self.compiled,
        }
    }
}

impl Compiled for Pair {
    fn describes(&self, program: &ply_syntax::ast::Program) -> bool {
        self.compiled.describes(program)
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        // The per-call seam, used by the reference path; the compiled unit is the authority.
        self.compiled.enter(name, args, budget)
    }

    fn enter_test(&self, name: &Symbol, budget: usize) -> Entered {
        self.route_test(name, budget)
    }

    fn enter_whole(&self, name: &Symbol, args: &[Value], budget: usize) -> Entered {
        self.route_whole(name, args, budget)
    }

    fn take_performed(&self) -> Vec<EffectAtom> {
        self.ran().take_performed()
    }

    fn set_seed(&self, seed: Seed, steps: u32) {
        self.interp.set_seed(seed.clone(), steps);
        self.compiled.set_seed(seed, steps);
    }

    fn simulated(&self) -> Option<Record> {
        self.ran().simulated()
    }

    fn set_host(&self, binding: Arc<HostBinding>, runtime: Option<Rc<dyn HostRuntime>>) {
        self.interp.set_host(Arc::clone(&binding), runtime.clone());
        self.compiled.set_host(binding, runtime);
    }

    fn set_declared(&self, declared: Option<Footprint>) {
        self.interp.set_declared(declared.clone());
        self.compiled.set_declared(declared);
    }

    fn set_re_executed(&self, re_executed: bool) {
        self.interp.set_re_executed(re_executed);
        self.compiled.set_re_executed(re_executed);
    }

    fn take_host_use(&self) -> (HostUse, u64) {
        self.ran().take_host_use()
    }

    fn take_teardown(&self) -> Vec<Diagnostic> {
        self.ran().take_teardown()
    }

    fn tier_only(&self) -> bool {
        self.compiled.tier_only()
    }
}
