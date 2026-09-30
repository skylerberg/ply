//! Where a natively compiled body may be entered in place of evaluating one.

use crate::host::{HostBinding, HostRuntime, HostUse};
use crate::region::Record;
use crate::sim::Seed;
use crate::value::Value;
use crate::{DefHash, Diagnostic, EffectAtom, Footprint, Symbol};
use std::rc::Rc;
use std::sync::Arc;

pub trait Compiled {
    /// Whether this was built over the program [`crate::Front::hashes_digest`] names.
    fn describes(&self, program: DefHash) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;

    /// Runs a test whole, telling a raise from a decline, which [`Compiled::enter`] conflates.
    fn enter_test(&self, _name: &Symbol, _budget: usize) -> Entered {
        Entered::Declined
    }

    /// A definition entered whole, for an engine with no machine behind it to fall back to.
    fn enter_whole(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Entered {
        Entered::Declined
    }

    /// The atoms the last entry's body performed, handled ones included.
    fn take_performed(&self) -> Vec<EffectAtom> {
        Vec::new()
    }

    fn set_seed(&self, _seed: Seed, _steps: u32) {}

    /// The calls the last entry's body made: 0 when no body ran, as when a memo answered it.
    fn steps(&self) -> u64 {
        0
    }

    /// What the `simulate` regions of the last entry's body did; none when no body ran.
    fn simulated(&self) -> Option<Record> {
        None
    }

    fn set_host(&self, _binding: Arc<HostBinding>, _runtime: Option<Rc<dyn HostRuntime>>) {}

    fn set_declared(&self, _declared: Option<Footprint>) {}

    fn set_re_executed(&self, _re_executed: bool) {}

    /// What the entries since the last take asked of the host, and how many linear operations.
    fn take_host_use(&self) -> (HostUse, u64) {
        (HostUse::default(), 0)
    }

    /// What the host runtime said as the entries since the last take ended.
    fn take_teardown(&self) -> Vec<Diagnostic> {
        Vec::new()
    }
}

#[derive(Debug)]
pub enum Entered {
    Answered(Value),
    Raised(Diagnostic),
    Declined,
}
