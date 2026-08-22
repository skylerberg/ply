//! The other direction: a `Machine` entering compiled code.
//!
//! ADR 0018 §0 is the reason this file exists. The fragment reaches **52.58×**
//! where it runs and **0.998×** end to end, and the gap is not the boundary —
//! 102 crossings cost 0.017% of the run. It is that calls only ever went one
//! way. A function the fragment accepted whose *callers* it refused was compiled
//! and then never entered, so twenty compiled arithmetic functions moved a
//! 57,700 µs program to 57,582 µs.
//!
//! [`SpikeBodies`] is the inverse: it implements `ply_eval::Compiled`, the
//! machine's generic entry hook, so the interpreter runs the tree machinery and
//! drops into native code at the leaves.
//!
//! # What the machine is promised, and who enforces it
//!
//! `Compiled::enter` hands over a name, some scalars and a call budget, and
//! takes back at most one scalar. It is handed no arena, no stack, no handler
//! stack, no host binding, no `&mut Machine` and no callback — so the promises
//! below are kept by there being no route to break them, not by this file
//! remembering to:
//!
//! | promise | kept by |
//! | --- | --- |
//! | a native body reaches no Ply function outside the compiled set | [`crate::jit::Denotes::Uncompiled`] refuses the caller at compile time |
//! | it performs no effect and captures no continuation | there is no machine to perform into, and `perform`/`handle` are outside the fragment |
//! | it touches no cell and opens no region | `cell_get`/`cell_set` refused at compile time; [`crate::rt::Ctx::touched_cells`] is the armed check |
//! | it calls no user code from a builtin | `Builtin::higher_order` refused at compile time |
//! | it cannot outrun `ply_eval::limit` | the fuel prologue, seeded from `budget` |
//! | it raises nothing | a failure answers `None` and the machine raises its own diagnostic |
//! | it cannot be entered from inside itself | one `Ctx` behind a `RefCell`, which declines rather than resetting |
//!
//! The one thing **not** structural, and it is worth saying plainly: a compiled
//! body that computes the wrong `Int` is a wrong answer this boundary cannot
//! detect. `values_equal` against the machine over generated inputs, and
//! `differential::compare_answers` against the tree-walker, are what catch that;
//! nothing here does.

use crate::jit::{Compiled, Entry, Jit, Opts};
use crate::program::Loaded;
use crate::rt::{Ctx, FAILED_OUT_OF_FUEL};
use anyhow::{Result, anyhow, bail};
use ply_eval::Value;
use ply_span::Symbol;
use ply_syntax::ast::{Program, TypeExpr};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

/// The widest arity this boundary carries without allocating an argument array.
/// A wider function declines; nothing in the standard library or the kernel is
/// near it, and a heap allocation per native call would price the hook rather
/// than the code.
const MAX_ARITY: usize = 16;

/// One admitted definition: where its code is, how many arguments it takes, and
/// how often the machine has entered it.
struct Admitted {
    entry: Entry,
    arity: usize,
    entered: Cell<u64>,
}

/// Why an offered call was not taken. R4's null result was a speedup reported
/// with zero entries and nothing in the harness that could say so, so a decline
/// is counted by its reason rather than counted at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Declines {
    /// The machine offered a name this unit did not compile.
    pub not_compiled: u64,
    /// It compiled the name and the call had the wrong number of arguments.
    pub arity: u64,
    /// The body ran and failed — an overflow, a division by zero, a `match` with
    /// no arm, a type the fragment's `Int` lowering could not unbox.
    pub failed: u64,
    /// The body would have nested past the budget the machine handed it. The
    /// machine then re-evaluates and raises its own bound.
    ///
    /// **Measured cost, 2026-08-21.** A runaway recursion is now a diagnostic in
    /// both engines rather than a `SIGABRT` in one — which is the point — but it
    /// is a *slow* diagnostic. `mcts.playouts(0, 1, 5_000_000)` takes 7.9 s with
    /// a backend attached against 0.11 s without, over two runs at load 2.3
    /// (`mcts --dir benches/kernel --probe compiled`, release). The machine
    /// re-offers the same function at every one of the ten thousand interpreted
    /// depths and each attempt burns its whole remaining fuel before declining,
    /// so the work is O(`max_calls`²) native frames: 19,992 entries and 10,000
    /// fuel declines on that run.
    ///
    /// Not fixed, deliberately. The obvious fix — remember the budget a function
    /// last ran out of fuel at and decline anything smaller — is wrong on its
    /// face, because whether a body fits its budget depends on the *arguments*,
    /// so a wall set by one runaway call would silently decline the fast calls
    /// underneath it and there would be nothing to say so. That is a lost
    /// speedup with no symptom, which is the failure mode this milestone is
    /// supposed to be avoiding. The cost is bounded, it is paid only by a
    /// program that is about to die anyway, and no shipping command can install
    /// a backend.
    pub out_of_fuel: u64,
    /// An entry arrived while another was running. Structurally impossible —
    /// nothing a compiled body can call re-enters a machine — and counted rather
    /// than asserted, because the day it stops being impossible the count is the
    /// only thing that would say so.
    pub reentered: u64,
    /// A builtin allocated in the fragment's private arena, which means the
    /// compile-time refusal of `cell_get`/`cell_set` has a hole in it.
    pub touched_cells: u64,
}

impl Declines {
    pub fn total(&self) -> u64 {
        self.not_compiled
            + self.arity
            + self.failed
            + self.out_of_fuel
            + self.reentered
            + self.touched_cells
    }
}

/// Compiled bodies for one program, offered to a `Machine` through
/// `ply_eval::Compiled`.
pub struct SpikeBodies {
    /// Identity, never dereferenced. `Compiled::describes` is a pointer
    /// comparison for the reason `code::Lowering::describes` is one: a bisection
    /// builds programs whose definitions carry the names of the ones they
    /// replace, and a registry keyed on a bare name would answer for the wrong
    /// body.
    program: *const Program,
    compiled: Compiled,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry, and the `RefCell` is the proof rather than a
    /// comment: [`crate::rt::Ctx::slots`] is a bump arena with no pop, so an
    /// entry that began inside another would have to either reset it — leaving
    /// the outer activation's handles indexing different values of the same type
    /// — or let it grow for the life of the program. Both are silent. Declining
    /// is neither.
    ctx: RefCell<Ctx>,
    entered: Cell<u64>,
    declines: Cell<Declines>,
}

impl SpikeBodies {
    /// Registers `compiled`'s functions as enterable bodies for `loaded`'s
    /// program.
    ///
    /// `names` must be closed under calls — every dynamic callee of an admitted
    /// function is itself admitted — which is what [`admissible`] computes and
    /// what [`crate::jit::Denotes::Uncompiled`] enforces. Passing a set that is
    /// not closed is not unsound, because the compile would have refused it; it
    /// simply cannot happen.
    pub fn new(
        loaded: &'static Loaded,
        compiled: Compiled,
        names: &[String],
    ) -> Result<SpikeBodies> {
        if let Some(what) = compiled.tables().retains_a_handle() {
            bail!(
                "the constant pool holds {what}, which must not outlive the call that made it; \
                 refusing the whole registration rather than entering anything"
            );
        }
        let mut admitted = HashMap::new();
        for name in names {
            let entry = compiled
                .entry(name)
                .ok_or_else(|| anyhow!("`{name}` was admitted and not compiled"))?;
            let arity = compiled
                .arity(name)
                .ok_or_else(|| anyhow!("`{name}` was compiled without an arity"))?;
            if arity > MAX_ARITY {
                bail!(
                    "`{name}` takes {arity} arguments and this boundary carries {MAX_ARITY}; \
                     refusing the registration rather than leaving one name that declines \
                     every call and no reason recorded against it"
                );
            }
            admitted.insert(
                Symbol::new(name),
                Admitted {
                    entry,
                    arity,
                    entered: Cell::new(0),
                },
            );
        }
        let ctx = RefCell::new(compiled.context());
        Ok(SpikeBodies {
            program: &loaded.ast as *const Program,
            compiled,
            admitted,
            ctx,
            entered: Cell::new(0),
            declines: Cell::new(Declines::default()),
        })
    }

    pub fn compiled(&self) -> &Compiled {
        &self.compiled
    }

    /// Native bodies actually run, over this provider's whole life.
    ///
    /// The number R5 exists to move. A ratio reported beside a zero here is a
    /// null result whatever it says.
    pub fn entered(&self) -> u64 {
        self.entered.get()
    }

    pub fn declines(&self) -> Declines {
        self.declines.get()
    }

    /// Runs `f` with the entry context borrowed, which is the state an entry in
    /// progress leaves behind it.
    ///
    /// There is no way to produce a genuinely nested entry: nothing a native
    /// body can call reaches a `Machine`, and `Denotes::Uncompiled` refuses any
    /// caller that would try. So the reentrancy guard in [`SpikeBodies::enter`]
    /// has no route that can fire it, and a counter that can never move is the
    /// kind of armed-looking check `CONTRIBUTING.md` §"Do not state a guarantee
    /// you have not armed" is about. This is the closest thing there is: the
    /// borrow the guard actually tests, held while the machine is offered calls.
    /// A test that does this and sees the interpreter answer correctly has
    /// checked the guard's behaviour rather than its existence.
    pub fn while_entered<T>(&self, f: impl FnOnce() -> T) -> T {
        let _held = self.ctx.borrow_mut();
        f()
    }

    /// The value arena's length and capacity, which is what grows if an entry
    /// ever stops resetting: `Ctx::slots` is a bump arena with no pop, so
    /// unbounded growth here is memory proportional to executed work rather than
    /// to live data.
    pub fn slots(&self) -> (usize, usize) {
        let ctx = self.ctx.borrow();
        (ctx.slots.len(), ctx.slots.capacity())
    }

    /// Builtin calls made from inside compiled code, over this provider's whole
    /// life. Cumulative across entries and direct calls both, because the
    /// context that counts them is shared.
    pub fn builtin_calls(&self) -> u64 {
        self.ctx.borrow().builtin_calls
    }

    /// Entries per admitted function, descending — so a report can say *which*
    /// functions the interpreter dropped into rather than only how often.
    pub fn entries_by_name(&self) -> Vec<(String, u64)> {
        let mut out: Vec<(String, u64)> = self
            .admitted
            .iter()
            .map(|(name, a)| (name.to_string(), a.entered.get()))
            .filter(|(_, n)| *n > 0)
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    pub fn reset_counts(&self) {
        self.entered.set(0);
        self.declines.set(Declines::default());
        for a in self.admitted.values() {
            a.entered.set(0);
        }
    }

    fn decline(&self, mut f: impl FnMut(&mut Declines)) -> Option<Value> {
        let mut d = self.declines.get();
        f(&mut d);
        self.declines.set(d);
        None
    }

    /// Runs a compiled body directly, outside any machine.
    ///
    /// This is ADR 0016's original measurement path — a whole entry, arguments
    /// boxed, `Ctx` seeded — and it is *not* what the machine uses. It is kept
    /// because `read_line` is measured this way and because a test that wants to
    /// see the fragment's own failure needs the diagnostic the boundary discards.
    pub fn call_direct(&self, name: &str, args: &[Value], fuel: i64) -> Result<Value> {
        let entry = self
            .compiled
            .entry(name)
            .ok_or_else(|| anyhow!("`{name}` was not compiled"))?;
        let arity = self
            .compiled
            .arity(name)
            .ok_or_else(|| anyhow!("`{name}` was compiled without an arity"))?;
        if args.len() != arity {
            bail!(
                "`{name}` takes {arity} arguments and was called with {}",
                args.len()
            );
        }
        if arity > MAX_ARITY {
            bail!("`{name}` takes {arity} arguments, more than this boundary carries");
        }
        let mut ctx = self
            .ctx
            .try_borrow_mut()
            .map_err(|_| anyhow!("a direct call arrived while an entry was running"))?;
        ctx.begin(fuel);
        let mut handles = [0i64; MAX_ARITY];
        for (slot, value) in handles.iter_mut().zip(args) {
            *slot = ctx.push(value.clone());
        }
        let out = unsafe { entry(&mut *ctx as *mut Ctx, handles.as_ptr()) };
        if ctx.failed != 0 {
            let d = ctx.take_failure();
            bail!(
                "compiled code raised: {}",
                d.map(|d| d.message).unwrap_or_else(|| "no message".into())
            );
        }
        Ok(ctx.read(out).clone())
    }
}

impl ply_eval::Compiled for SpikeBodies {
    fn describes(&self, program: &Program) -> bool {
        std::ptr::eq(self.program, program as *const Program)
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        let Some(admitted) = self.admitted.get(name) else {
            return self.decline(|d| d.not_compiled += 1);
        };
        if admitted.arity != args.len() {
            return self.decline(|d| d.arity += 1);
        }
        let Ok(mut ctx) = self.ctx.try_borrow_mut() else {
            return self.decline(|d| d.reentered += 1);
        };

        ctx.begin(i64::try_from(budget).unwrap_or(i64::MAX));
        let mut handles = [0i64; MAX_ARITY];
        for (slot, value) in handles.iter_mut().zip(args) {
            *slot = ctx.push(value.clone());
        }
        let out = unsafe { (admitted.entry)(&mut *ctx as *mut Ctx, handles.as_ptr()) };

        if ctx.failed != 0 {
            // The fragment's own diagnostic is deliberately dropped on the floor:
            // it is `RUNTIME_ERROR` at `Span::DUMMY`, and the machine is about to
            // evaluate the same definition and raise the real one. Which failure
            // it was is still counted, because "the budget ran out" and "the
            // arithmetic overflowed" are different facts about a run.
            let out_of_fuel = ctx.failed == FAILED_OUT_OF_FUEL;
            drop(ctx);
            return self.decline(|d| {
                if out_of_fuel {
                    d.out_of_fuel += 1;
                } else {
                    d.failed += 1;
                }
            });
        }
        if ctx.touched_cells() {
            drop(ctx);
            return self.decline(|d| d.touched_cells += 1);
        }
        let value = ctx.read(out).clone();
        drop(ctx);
        self.entered.set(self.entered.get() + 1);
        admitted.entered.set(admitted.entered.get() + 1);
        Some(value)
    }
}

/// Whether every parameter and the return type are written `Int` or `Bool`.
///
/// Necessary and not sufficient, and the machine's boundary is the authority on
/// both sides anyway. It is here so that a function which would *always* decline
/// is never registered: `std.http.read_line` takes `Bytes`, and `floaty.add`
/// takes `Float`s the fragment has no path for and compiles as `Int` arithmetic
/// regardless (ADR 0019 §5 item 4). Declining before the fact is cheaper than
/// declining 120,000 times, and a refusal that fires constantly is a bug report
/// rather than a fast path.
pub fn scalar_signature(loaded: &Loaded, name: &str) -> bool {
    let Some((def, _)) = loaded.definition(name) else {
        return false;
    };
    let scalar = |t: Option<&TypeExpr>| match t {
        Some(TypeExpr::Con { name, args, .. }) => {
            args.is_empty() && matches!(name.symbol().as_str(), "Int" | "Bool")
        }
        _ => false,
    };
    def.params.iter().all(|p| scalar(p.ty.as_ref())) && scalar(def.ret.as_ref())
}

/// The largest subset of `candidates` the fragment compiles **as one unit**.
///
/// Not a list somebody read off a census: a name survives only if every
/// construct in its body is inside the fragment *and* every Ply function it can
/// reach is also in the set. Dropping one function refuses its callers on the
/// next round, so this is a fixpoint rather than a filter, and it terminates
/// because every round that changes anything removes at least one name.
///
/// The result is what makes the promise [`SpikeBodies`] gives the machine true
/// by construction: from inside a member there is no reachable call that leaves
/// compiled code. It is **not** the set that gets registered — see
/// [`enterable`], which is the scalar-signature subset of this.
pub fn admissible(loaded: &'static Loaded, candidates: &[String]) -> Result<Vec<String>> {
    Ok(closure(loaded, candidates)?.0)
}

/// The members of a compiled set the machine may be offered.
///
/// A member whose signature is not `Int`/`Bool` throughout would decline on
/// every call, so it is never registered — but it stays *compiled*, because a
/// native body reaching it is what makes the set closed. `std.http.line_at` is
/// the example: compiled, reachable from `read_line`, never entered.
pub fn enterable(loaded: &Loaded, set: &[String]) -> Vec<String> {
    set.iter()
        .filter(|n| scalar_signature(loaded, n))
        .cloned()
        .collect()
}

/// Every function `candidates` loses, with the reason it lost, in the round it
/// lost it.
///
/// The first round has every candidate present, so a call between two of them
/// resolves as a direct call and the refusals name **constructs** — a field
/// access, a list pattern, unary `-`. That is the ranking ADR 0018 §0's roadmap
/// is read off, and it survives R5 unchanged. Later rounds name the callee that
/// was dropped, which is a different fact and is reported as one: "a call to
/// `mcts.iterate`" is not evidence that a field access is the roadmap.
pub fn refusals_over(
    loaded: &'static Loaded,
    candidates: &[String],
) -> Result<Vec<(String, String)>> {
    Ok(closure(loaded, candidates)?.1)
}

/// The surviving set, and every function that was dropped with the reason.
type Closed = (Vec<String>, Vec<(String, String)>);

fn closure(loaded: &'static Loaded, candidates: &[String]) -> Result<Closed> {
    let mut set: Vec<String> = candidates.to_vec();
    let mut lost: Vec<(String, String)> = Vec::new();
    loop {
        if set.is_empty() {
            break;
        }
        let names: Vec<&str> = set.iter().map(|s| s.as_str()).collect();
        let refusals = Jit::refusals(loaded, &names, Opts::default())?;
        if refusals.is_empty() {
            break;
        }
        let refused: HashSet<&str> = refusals.iter().map(|r| r.function.as_str()).collect();
        for r in &refusals {
            lost.push((r.function.clone(), r.construct.clone()));
        }
        let before = set.len();
        set.retain(|n| !refused.contains(n.as_str()));
        if set.len() == before {
            bail!(
                "the fragment refused {} function(s) and named none of the ones it was given: {:?}",
                refusals.len(),
                refusals
                    .iter()
                    .map(|r| r.function.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
    Ok((set, lost))
}
