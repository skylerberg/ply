//! The other direction: a `Machine` entering compiled code.

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
const MAX_ARITY: usize = 16;

/// One admitted definition: where its code is, how many arguments it takes, and how often the
/// machine has entered it.
struct Admitted {
    entry: Entry,
    arity: usize,
    entered: Cell<u64>,
}

/// Why an offered call was not taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Declines {
    /// The machine offered a name this unit did not compile.
    pub not_compiled: u64,
    /// It compiled the name and the call had the wrong number of arguments.
    pub arity: u64,
    /// The body ran and failed — an overflow, a division by zero, a `match` with no arm, a type the
    /// fragment's `Int` lowering could not unbox.
    pub failed: u64,
    /// The body would have nested past the budget the machine handed it.
    pub out_of_fuel: u64,
    /// An entry arrived while another was running.
    pub reentered: u64,
    /// A builtin allocated in the fragment's private arena, which means the compile-time refusal of
    /// `cell_get`/`cell_set` has a hole in it.
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

/// Compiled bodies for one program, offered to a `Machine` through `ply_eval::Compiled`.
pub struct SpikeBodies {
    /// Identity, never dereferenced.
    program: *const Program,
    compiled: Compiled,
    admitted: HashMap<Symbol, Admitted>,
    /// One context for every entry, and the `RefCell` is the proof rather than a comment:
    /// [`crate::rt::Ctx::slots`] is a bump arena with no pop, so an entry that began inside another
    /// would have to either reset it — leaving the outer activation's handles indexing different
    /// values of the same type — or let it grow for the life of the program.
    ctx: RefCell<Ctx>,
    entered: Cell<u64>,
    declines: Cell<Declines>,
}

impl SpikeBodies {
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

    pub fn admits(&self, name: &Symbol) -> bool {
        self.admitted.contains_key(name)
    }

    /// Native bodies actually run, over this provider's whole life.
    pub fn entered(&self) -> u64 {
        self.entered.get()
    }

    pub fn declines(&self) -> Declines {
        self.declines.get()
    }

    /// Runs `f` with the entry context borrowed, which is the state an entry in progress leaves
    /// behind it.
    pub fn while_entered<T>(&self, f: impl FnOnce() -> T) -> T {
        let _held = self.ctx.borrow_mut();
        f()
    }

    /// The value arena's length and capacity, which is what grows if an entry ever stops resetting:
    /// `Ctx::slots` is a bump arena with no pop, so unbounded growth here is memory proportional to
    /// executed work rather than to live data.
    pub fn slots(&self) -> (usize, usize) {
        let ctx = self.ctx.borrow();
        (ctx.slots.len(), ctx.slots.capacity())
    }

    /// How much of the value arena the entry that just finished used — the independent variable of
    /// `mcts --carryover`, and the number `CONTRIBUTING.md` item 12's curve is drawn against.
    pub fn arena_after_entry(&self) -> usize {
        self.ctx.borrow().arena_after_entry()
    }

    /// Entries whose predecessor had not closed itself.
    pub fn unclosed_entries(&self) -> u64 {
        self.ctx.borrow().unclosed_entries()
    }

    /// Builtin calls made from inside compiled code, over this provider's whole life.
    pub fn builtin_calls(&self) -> u64 {
        self.ctx.borrow().builtin_calls
    }

    /// Entries per admitted function, descending — so a report can say *which* functions the
    /// interpreter dropped into rather than only how often.
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
            ctx.end();
            bail!(
                "compiled code raised: {}",
                d.map(|d| d.message).unwrap_or_else(|| "no message".into())
            );
        }
        let value = ctx.read(out).clone();
        ctx.end();
        Ok(value)
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
            // The fragment's own diagnostic is deliberately dropped on the floor: it is
            // `RUNTIME_ERROR` at `Span::DUMMY`, and the machine is about to evaluate the same
            // definition and raise the real one.
            let out_of_fuel = ctx.failed == FAILED_OUT_OF_FUEL;
            ctx.end();
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
            ctx.end();
            drop(ctx);
            return self.decline(|d| d.touched_cells += 1);
        }
        let value = ctx.read(out).clone();
        ctx.end();
        drop(ctx);
        self.entered.set(self.entered.get() + 1);
        admitted.entered.set(admitted.entered.get() + 1);
        Some(value)
    }
}

/// Whether every parameter and the return type are written `Int` or `Bool`.
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
pub fn admissible(loaded: &'static Loaded, candidates: &[String]) -> Result<Vec<String>> {
    Ok(closure(loaded, candidates)?.0)
}

/// The members of a compiled set the machine may be offered.
pub fn enterable(loaded: &Loaded, set: &[String]) -> Vec<String> {
    set.iter()
        .filter(|n| scalar_signature(loaded, n))
        .cloned()
        .collect()
}

/// Every function `candidates` loses, with the reason it lost, in the round it lost it.
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
