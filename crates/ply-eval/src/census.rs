//! A count of what the compiled seam is offered and what refuses it.
//!
//! Measurement scaffolding, off unless `PLY_SEAM_CENSUS` is set in the
//! environment, and it answers one question `compiled.rs`'s counters cannot:
//! `Machine::compiled_counts` counts entries and declines *after* `admit` has
//! already cleared a call, so the denominator it reports is the admitted set
//! rather than the program. What fraction of a real program's calls can cross
//! is therefore not a number this tree has ever printed.
//!
//! Every field is a count, so a run is deterministic and a second run is a
//! check rather than a sample.
//!
//! # What the ladder was built to decide, and what it decided
//!
//! `compiled::crossable` carries `Int`, `Bool` and `Bytes`. Everything above
//! that is a container, and a container cannot be admitted on its discriminant:
//! it can hold a `Closure` or a `Cell` one field down and `compiled.rs`'s
//! effects gate rests on no code crossing. So the question "what would the next
//! widening buy" has three candidate answers, and they are three different
//! numbers rather than one. Measured 2026-08-30, `examples/desk.ply` through
//! the ported Ply front end (`spikes/ply-parser`) and `ply test examples -j 1`,
//! as a share of body calls:
//!
//! | design | Ply parser | Ply lexer | `ply test examples` |
//! | --- | ---: | ---: | ---: |
//! | today (`Int\|Bool\|Bytes`) | 12.52% | 24.99% | 25.44% |
//! | shallow kind test — **unsound**, the number an optimistic census prints | 82.58% | 68.67% | 91.92% |
//! | deep value walk, budget 256 — sound | **18.47%** | **25.03%** | 90.81% |
//! | declared parameter types — sound | **82.58%** | **68.67%** | 84.22% |
//!
//! Two things fall out of that table and neither was predictable from the
//! shallow rung alone.
//!
//! **The deep walk is the wrong instrument, and on the workload the
//! bootstrapping case rests on it is catastrophically wrong.** Unbounded it does
//! not finish: the parser passes a state record that transitively holds the
//! token list and the source bytes, so the walk is O(program state) per call
//! over ~1,000,000 calls — **no output and no exit in 9 minutes 30 seconds**
//! against **1.2 s** for the shallow test, killed rather than waited out.
//! Bounded it reaches 18.47%, and `deep_blockers` says why in one line:
//! **660,255 of 660,255** refusals in the gap are `<budget>`, not `Closure` and
//! not `Cell`. Raising the budget does not fix it — 16 / 256 / 4096 nodes give
//! 18.33% / 18.47% / 18.79% — because the values really are that large.
//!
//! The same three runs price the walk, and the price predicts the hang rather
//! than merely being consistent with it. User CPU for the whole probe was
//! 1.83 / 3.38 / 27.53 s, and 660,255 calls exhaust their budget on each of the
//! two deep rungs at every budget, so the marginal cost is
//! `(27.53 - 3.38) / (660,255 x 2 x 3,840)` = **4.8 ns per node visited** and
//! `(3.38 - 1.83) / (660,255 x 2 x 240)` = **4.9 ns** — linear, which is the
//! signature of a walk that never finishes early. `desk.ply` lexes to 19,563
//! tokens and each is a constructor over a span record, so the state record the
//! parser passes reaches on the order of 1e5 nodes; at 4.8 ns that is
//! `660,255 x 2 x 1e5 x 4.8 ns` = **630 s**, against the 570 s the unbounded
//! run was killed at with no output. The extrapolation lands on the
//! observation, which is the only reason either is worth quoting. Wall-clock
//! figures in this block are observations rather than figures in the project's
//! sense — they were taken at a load average of 4 to 6 — but a 475x ratio is
//! not a load artefact.
//!
//! On `examples/` the same walk costs almost nothing (90.81% against 91.92%) and
//! the blockers there are genuine: `Closure` 2,452, `Secret` 20, `<budget>` 249.
//! A census taken only on `examples/` would have reported that a deep walk is
//! affordable, and it is not.
//!
//! **The type-level gate is the right instrument and it is free.** Deciding the
//! arguments from the definition's *declared parameter types* is a property of a
//! published scheme, so it is computed once per definition and is O(1) per call
//! — it needs no walk at all. It reaches **exactly the shallow rung's number**
//! on both Ply front-end probes, 82.58% and 68.67%, because a front end's
//! parameters are declared at concrete first-order types. Its conservatism shows
//! up on `examples/` instead, at 84.22% against the shallow 91.92%: it refuses a
//! `Type::Var` parameter, which can be instantiated at a closure however
//! first-order the value in front of it happens to be.
//!
//! What that costs is an ordering, not a walk. `compiled::admit` puts the shape
//! gate ahead of the `CheckOutput` lookup on purpose, and
//! `the_shape_gate_is_reached_before_the_row_is_looked_up` asserts it; a
//! type-level argument test needs the name first. Whoever takes that widening
//! owes a re-measurement of that ordering and a per-definition cache, and owes
//! it before the first line of a backend.
//!
//! > **Taken, and the debt discharged (2026-08-31).** The widening this table
//! > chose is `compiled::CarriedTypes`, reached through
//! > `compiled::Gate::ArgumentType`. Both obligations above were paid: the
//! > per-definition cache is that type, built once per `CheckOutput` behind a
//! > `OnceCell` on the machine, and the ordering was re-measured rather than
//! > argued — the type gate sits **below** the row and effects gates, not above
//! > the shape gate, because above them it masks their refusal of an unpublished
//! > name and two of this seam's tripwires went red saying so.
//! >
//! > **Two of the four numbers in the table above are now *measurements* of the
//! > shipping gate rather than counterfactuals, and they do not agree with the
//! > counterfactual.** Re-taken on the same two workloads on 2026-08-31, as a
//! > share of body calls with no backend attached (so the denominator is the
//! > whole program in every row):
//! >
//! > | | Ply parser (W1) | `ply test examples` (W2) |
//! > | --- | ---: | ---: |
//! > | before — `Int\|Bool\|Bytes` on the value | 12.205% | 25.442% |
//! > | **after — declared parameter types, shipping** | **84.014%** | **26.235%** |
//! > | the counterfactual this table predicted (`type_gated`) | 84.014% | 84.135% |
//! >
//! > On the front end the gate lands exactly on its own counterfactual. On
//! > `examples/` it lands **58 pp short**, and the census now says why in one
//! > line rather than leaving it to be guessed: the new `blocking_types`
//! > histogram attributes every `ArgumentType` refusal to the first uncarried
//! > head in the declared parameter type, and over `examples/` it reads
//! > **`String` 108,925 · `Var` 10,867 · `Decimal` 1,504 · `Fn` 346**. The
//! > shortfall is **89.5% `String`**, which is not the type test being
//! > conservative — it is `compiled::crossable`'s leaf set, deliberately
//! > unchanged at `Int | Bool | Bytes` so that this widening moves containers
//! > and no hazard (ADR 0019 §5 item 4). The `Var` row is the whole cost of
//! > refusing generics rather than resolving them at the call site: **4.4% of
//! > body calls on `examples/`, and 0 on the front end**.
//! >
//! > The counterfactual row is kept and is not withdrawn: it is the ladder's
//! > answer to a different question — "what would a rung with *these value
//! > kinds* allow" — and the gap between it and the shipping row is exactly the
//! > leaf set, which is the next thing anyone widening this will reach for.
//!
//! > **The ARGUMENT number stopped being the interesting one on the same day
//! > (2026-08-31).** Everything above is about what the seam **admits**. What a
//! > backend can *answer* is a second question this module has always counted
//! > and nobody had read: `admitted_carried_sig`, which additionally requires
//! > the declared **return** type to be carried. On the front end the two were
//! > **2,028,230 and 411,216** — 84.014% admitted, **17.033% answerable**. Four
//! > of every five admitted calls were offered and declined on the return, and
//! > ADR 0030 §1 had already named the most valuable one: `lex(Bytes) -> Scan`,
//! > offered thirteen times and declined thirteen times.
//! >
//! > `Machine::compiled_answer` now decides an answer the way `admit` decides an
//! > argument, and on this workload the two numbers meet: **admitted 2,028,230,
//! > answerable 2,028,230**. The `offered and declined` line this module prints
//! > goes 1,617,014 -> 0 there, and over `examples/` from 4,690 to 3,330 —
//! > `String`, `Float` and `Decimal` returns, which are deliberately outside the
//! > fragment in both directions.
//! >
//! > **The number to read after that change is not a share at all, it is the
//! > entry count, and it FALLS.** `ply test <W1> --backend reference` goes from
//! > **306,931 of 1,580,763 offers entered** to **26 of 26**, because
//! > `items.parse` is entered once per file and its whole subtree runs inside
//! > that entry. That is PR #30's shape — crossings 721 -> 1 when a fragment
//! > widened until one entry swallowed an MCTS search — and it is the win rather
//! > than a regression: a call share cannot see it, because entering a call
//! > removes its subtree from this module's denominator as well as from its
//! > numerator. Every share quoted in this header is therefore taken **with no
//! > backend attached**, and after this change that is not a convention but a
//! > requirement: with one attached the front-end denominator collapses from
//! > 2,414,170 body calls to 26.
//! >
//! > `carried_sig_walked` was added in the same change and is the one counter
//! > here that checks the seam against itself rather than against a
//! > counterfactual: it asks `admitted_carried_sig`'s question by **walking**
//! > the declared types at every call, where the shipping predicate reads a
//! > per-definition table built once. `seam_census.rs` asserts they are equal
//! > over the whole corpus, because a precompute can be right about a rule and
//! > stale about a program.
//!
//! # Every number above is a CALL share. One time share exists, and it is here
//!
//! This module counts calls, and a call share is not a time share.
//! [`ADR 0030`](../../../docs/adr/0030-compiled-code-on-the-front-end.md)
//! measured the one bridge that exists: on `spikes/ply-parser` parsing
//! `examples/` — the workload the two front-end figures above were taken on —
//! today's rung's **8.63% of body calls accounted for 10.78% of run time**, a
//! ratio of 1.25, and an infinitely fast backend over it has a **ceiling of
//! 1.121×**. That is one point and it does not extrapolate: 1.25 x 82.9%
//! exceeds 100%, so the relation saturates somewhere between the two rungs and
//! nothing here says where. Whoever quotes the 82.855% as what a widening is
//! worth is quoting a call share; the time it buys has not been measured and
//! this module cannot measure it.
//!
//! > **One of those three clauses is withdrawn (2026-08-31), and the other two
//! > stand.** Withdrawn: *"the time it buys has not been measured"*. It has been
//! > measured twice since — ADR 0030 §6.3 for the answer widening (`f`
//! > 22.97% -> 97.53%) and §10 for the lambda wall (a registry narrowed to the
//! > callback-free fragment reads `f` = 51.77%, ceiling **2.074×**, against
//! > 99.65% for one that can enter the root). Standing: *"this module cannot
//! > measure it"*, and it is why both readings were taken from **outside the
//! > process**, by the two-binary method §4 built because `ply-eval` may not
//! > carry a clock.
//! >
//! > **What stands, and it is what the paragraph was for: a call share is not a
//! > time share, and the saturation it warned about is now on the record.** §10
//! > registered a prediction that the callback-free fragment would cover under
//! > 30% of body calls; it covers **61.06%**, and its ceiling is 2.074× rather
//! > than anything near the 99.65% the full fragment reaches. 61% of the calls,
//! > 52% of the time, and 2.07x rather than 282x — the relation between the two
//! > columns saturates exactly as this paragraph said it must, and the ratio
//! > that governs is the one measured on the arm in front of you rather than
//! > the 1.25 above.
//! >
//! > > **And the ceiling `1/(1-f)` could not be resolved at `f` = 97.53%, which
//! > > [ADR 0031](../../../docs/adr/0031-the-closed-fragment.md) §3 fixes with a
//! > > second instrument rather than a better estimate.** A **floor arm** — the
//! > > same command with a `--filter` that selects no test, which still
//! > > typechecks all seven modules and runs no body — measures the residue
//! > > directly instead of inferring it: `F` = **0.05 s** against `A` = 2.84 s,
//! > > so `A/F` = **56.8x**, and the two instruments agree on the linear
//! > > quantity `t` to **0.36%** (2.780 against 2.790) while their multipliers
//! > > sit 17% apart. Both readings of this module's own arm are reproduced
//! > > there: the callback-free ceiling re-measures at **2.104x** against §10's
//! > > 2.074x, and the entry line `495152 of 1049245 offers entered` to the
//! > > call.

use crate::compiled::Gate;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOOKED: OnceLock<()> = OnceLock::new();

pub fn enabled() -> bool {
    LOOKED.get_or_init(|| {
        if std::env::var_os("PLY_SEAM_CENSUS").is_some() {
            ON.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    ON.load(std::sync::atomic::Ordering::Relaxed)
}

/// Turns the census on for the rest of the process, for a harness that cannot
/// set an environment variable before the first machine runs.
pub fn enable() {
    let _ = enabled();
    ON.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The raw counts, for a test that asserts on them rather than printing them.
pub fn snapshot() -> (u64, u64, u64, BTreeMap<&'static str, u64>) {
    let c = cell().lock().expect("census");
    (
        c.body_calls,
        c.admitted,
        c.admitted_carried_sig,
        c.gates.clone(),
    )
}

/// What the shipping type gate alone admits, for the test that reads it against
/// `admitted`. See [`Counts::type_gated_shipping`].
pub fn type_gated_shipping() -> u64 {
    cell().lock().expect("census").type_gated_shipping
}

/// [`Counts::carried_sig_walked`], for the test that reads it against
/// `admitted_carried_sig`.
pub fn carried_sig_walked() -> u64 {
    cell().lock().expect("census").carried_sig_walked
}

#[derive(Default)]
pub struct Counts {
    pub body_calls: u64,
    pub builtin_calls: u64,
    pub ctor_calls: u64,
    pub admitted: u64,
    /// Of `admitted`, those whose whole declared signature is carried by the
    /// seam — what `backend::Reference` would actually answer rather than be
    /// offered and decline.
    pub admitted_carried_sig: u64,
    /// The same question asked by a **walk** over the declared types rather
    /// than by reading `compiled::CarriedTypes`'s per-definition table.
    ///
    /// The two must be equal, and they are two routes to one predicate rather
    /// than one route counted twice: `admitted_carried_sig` goes through
    /// `backend::carried_signature` -> `CarriedTypes::signature_carried`, which
    /// reads the `Denotes` computed once per definition when the table was
    /// built; this one calls `CarriedTypes::carries` on every parameter and on
    /// the return type at the call. A gap means the precompute and the walk
    /// disagree, which is the failure mode a per-definition cache has and a
    /// per-call test does not. `seam_census.rs` reads them off a corpus.
    pub carried_sig_walked: u64,
    pub frame_ceiling: u64,
    pub gates: BTreeMap<&'static str, u64>,
    /// For `ArgumentShape`: every argument `compiled::crossable` refuses, by
    /// kind.
    pub blocking_args: BTreeMap<&'static str, u64>,
    /// For `ArgumentType`: what in the declared parameter types refused it —
    /// see `compiled::CarriedTypes::refusal`. One entry per refused call, the
    /// first offending head only.
    pub blocking_types: BTreeMap<&'static str, u64>,
    /// Admitted calls by name.
    pub admitted_names: BTreeMap<String, u64>,
    /// Refused calls by `name @ gate`.
    pub refused_names: BTreeMap<String, u64>,
    pub builtin_names: BTreeMap<&'static str, u64>,
    /// Counterfactual widenings of `compiled::crossable`, by level name:
    /// calls whose arguments all pass that level *and* clear every other gate.
    pub widened: BTreeMap<&'static str, u64>,
    /// The same, and the definition's declared return type also passes — so a
    /// native body would have something it could hand back.
    pub widened_returnable: BTreeMap<&'static str, u64>,
    /// For the widest DEEP rung: what refused each call whose arguments the
    /// *shallow* rung would have carried. The difference between the two rungs,
    /// attributed.
    pub deep_blockers: BTreeMap<&'static str, u64>,
    /// The third design: clear every gate but the shape one, and decide the
    /// arguments from the definition's **declared parameter types** instead of
    /// from the values.
    ///
    /// This is the only sound container widening that is O(1) per call — the
    /// answer is a property of a published scheme and can be computed once per
    /// definition — and it is why it is worth a counter of its own beside the
    /// deep rungs. `..._and_return` additionally requires the declared return
    /// to be carried, which is what a backend needs to hand something back.
    pub type_gated: u64,
    pub type_gated_and_return: u64,
    /// The type gate **as it ships** — `compiled::CarriedTypes` — asked with the
    /// value-kind test removed, so the two halves of `Gate::ArgumentShape` and
    /// `Gate::ArgumentType` can be separated.
    ///
    /// On a program the checker accepted this must equal `admitted`: a value's
    /// kind follows its declared type, so the kind test refuses nothing the type
    /// test admits. A gap is not a bug in the gate — it is defence in depth
    /// firing — but it is a fact worth reading off a corpus rather than
    /// assuming, which is why it is counted.
    ///
    /// It differs from `type_gated` above, which is the counterfactual at the
    /// widest LADDER rung and therefore admits `String`, `Float` and `Decimal`
    /// leaves the shipping gate refuses.
    pub type_gated_shipping: u64,
    /// Calls a backend actually *entered*, by name — the subset of
    /// `admitted_names` whose name the backend's registry also holds. Written
    /// from `backend::Reference::enter`, so it is empty unless a backend is
    /// attached, and it is the only field here that is not a fact about the
    /// program alone.
    pub entered_names: BTreeMap<String, u64>,
}

/// The widening ladder, coarsest last. Level 1 is what ships.
///
/// Level 0 is kept after the `Bytes` widening rather than dropped: it is the
/// arm every before/after number on this seam is read against, and a ladder
/// whose first rung is always "today" cannot say what a change bought.
///
/// The third column is whether the argument test **walks into containers**. It
/// exists because a shallow rung above level 2 is not a rung anything could
/// ship: a `List`, `Map`, `Record` or `Ctor` admitted on its discriminant can
/// carry a `Closure` or a `Cell` one field down, and `compiled.rs`'s effects
/// gate rests on no code crossing. So every container rung is reported twice —
/// once shallow, which is the number an optimistic census produces, and once
/// deep, which is the number a *sound* widening could actually reach. The gap
/// between the two is the size of the mistake a shallow census invites.
pub(crate) const LADDER: [(&str, &[&str], bool); 7] = [
    ("0 Int|Bool (before 2026-08-30)", &["Int", "Bool"], false),
    ("1 +Bytes (today)", &["Int", "Bool", "Bytes"], false),
    ("2 +Bytes,Str", &["Int", "Bool", "Bytes", "Str"], false),
    (
        "3 +Record,Ctor  shallow",
        &["Int", "Bool", "Bytes", "Str", "Record", "Ctor"],
        false,
    ),
    (
        "3d +Record,Ctor  DEEP",
        &["Int", "Bool", "Bytes", "Str", "Record", "Ctor"],
        true,
    ),
    (
        "4 no world-handle  shallow",
        &[
            "Int", "Bool", "Float", "Decimal", "Str", "Bytes", "Unit", "List", "Map", "Record",
            "Ctor",
        ],
        false,
    ),
    (
        "4d no world-handle  DEEP",
        &[
            "Int", "Bool", "Float", "Decimal", "Str", "Bytes", "Unit", "List", "Map", "Record",
            "Ctor",
        ],
        true,
    ),
];

fn cell() -> &'static Mutex<Counts> {
    static C: OnceLock<Mutex<Counts>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Counts::default()))
}

pub(crate) fn gate_name(gate: Gate) -> &'static str {
    match gate {
        Gate::NotLoweredCode => "NotLoweredCode",
        Gate::ArgumentShape => "ArgumentShape",
        Gate::ArgumentType => "ArgumentType",
        Gate::SimulateRegion => "SimulateRegion",
        Gate::Anonymous => "Anonymous",
        Gate::PublishedRow => "PublishedRow",
        Gate::InternalEffects => "InternalEffects",
        Gate::Budget => "Budget",
    }
}

pub(crate) fn value_kind(v: &crate::value::Value) -> &'static str {
    use crate::value::Value::*;
    match v {
        Int(_) => "Int",
        Bool(_) => "Bool",
        Float(_) => "Float",
        Decimal(_) => "Decimal",
        Str(_) => "Str",
        Bytes(_) => "Bytes",
        Unit => "Unit",
        List(_) => "List",
        Map(_) => "Map",
        Record(_) => "Record",
        Ctor { .. } => "Ctor",
        Closure(_) => "Closure",
        Cell(_) => "Cell",
        Task(_) => "Task",
        Continuation(_) => "Continuation",
        Secret(_) => "Secret",
    }
}

pub(crate) fn with<F: FnOnce(&mut Counts)>(f: F) {
    if let Ok(mut c) = cell().lock() {
        f(&mut c);
    }
}

/// The whole census, as lines on stderr. Called once, from the binary.
pub fn report() -> String {
    let Ok(c) = cell().lock() else {
        return String::new();
    };
    let mut out = String::new();
    let refused = c.body_calls - c.admitted;
    out.push_str("=== PLY SEAM CENSUS ===\n");
    out.push_str(&format!(
        "body calls (enter_code)   {}\nbuiltin calls             {}\nctor calls                {}\n",
        c.body_calls, c.builtin_calls, c.ctor_calls
    ));
    out.push_str(&format!(
        "admitted (all gates)      {}  ({:.4}% of body calls)\n",
        c.admitted,
        pct(c.admitted, c.body_calls)
    ));
    out.push_str(&format!(
        "  of which carried-sig    {}  ({:.4}% of body calls)  <- what `Reference` would answer\n\
         \x20 carried-sig by walk   {}  (equal = {})\n\
         \x20 offered and declined  {}  <- admitted, but the declared RETURN type is not carried\n",
        c.admitted_carried_sig,
        pct(c.admitted_carried_sig, c.body_calls),
        c.carried_sig_walked,
        c.carried_sig_walked == c.admitted_carried_sig,
        c.admitted - c.admitted_carried_sig,
    ));
    out.push_str(&format!("refused                   {refused}\n"));
    let mut sum = 0;
    for (g, n) in &c.gates {
        sum += *n;
        out.push_str(&format!(
            "  {g:<18} {n:>12}  ({:.2}% of refusals)\n",
            pct(*n, refused)
        ));
    }
    out.push_str(&format!(
        "  {:<18} {:>12}  (machine-side, not a Gate)\n",
        "FrameCeiling", c.frame_ceiling
    ));
    out.push_str(&format!(
        "  [histogram sums to {sum}; refused is {refused}; equal = {}]\n",
        sum == refused
    ));
    out.push_str("non-crossable arguments, by kind (ArgumentShape refusals only):\n");
    let mut args: Vec<_> = c.blocking_args.iter().collect();
    args.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (k, n) in args {
        out.push_str(&format!("  {k:<12} {n:>12}\n"));
    }
    out.push_str("uncarried declared parameter types (ArgumentType refusals only, first head):\n");
    let mut tys: Vec<_> = c.blocking_types.iter().collect();
    tys.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (k, n) in tys {
        out.push_str(&format!("  {k:<14} {n:>12}\n"));
    }
    out.push_str(&format!(
        "counterfactual: what a wider `crossable` would admit (DEEP rungs walk to a budget of {} nodes)\n",
        deep_budget()
    ));
    for (label, _, _) in LADDER {
        let a = c.widened.get(label).copied().unwrap_or(0);
        let r = c.widened_returnable.get(label).copied().unwrap_or(0);
        out.push_str(&format!(
            "  {label:<40} args-only {a:>10} ({:>7.3}%)   args+return {r:>10} ({:>7.3}%)\n",
            pct(a, c.body_calls),
            pct(r, c.body_calls)
        ));
    }
    out.push_str(&format!(
        "type-level gate (declared parameter types, O(1) per call after a per-definition \
         precompute)\n  params carried {} ({:.3}%)   params+return carried {} ({:.3}%)\n  \
         SHIPPING type gate alone {} ({:.3}%)   admitted {} ({:.3}%)   equal = {}\n",
        c.type_gated,
        pct(c.type_gated, c.body_calls),
        c.type_gated_and_return,
        pct(c.type_gated_and_return, c.body_calls),
        c.type_gated_shipping,
        pct(c.type_gated_shipping, c.body_calls),
        c.admitted,
        pct(c.admitted, c.body_calls),
        c.type_gated_shipping == c.admitted
    ));
    out.push_str(
        "why the widest DEEP rung refuses what the shallow one carries (first offending kind):\n",
    );
    let mut blockers: Vec<_> = c.deep_blockers.iter().collect();
    blockers.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (k, n) in blockers {
        out.push_str(&format!("  {k:<12} {n:>12}\n"));
    }
    out.push_str("top admitted definitions:\n");
    out.push_str(&top(&c.admitted_names, 20));
    out.push_str(&format!(
        "definitions a backend actually ENTERED ({} distinct, {} entries):\n",
        c.entered_names.len(),
        c.entered_names.values().sum::<u64>()
    ));
    out.push_str(&top(&c.entered_names, 25));
    out.push_str("top refusals (name @ gate):\n");
    out.push_str(&top(&c.refused_names, 25));
    out.push_str("top builtins called:\n");
    let owned: BTreeMap<String, u64> = c
        .builtin_names
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
    out.push_str(&top(&owned, 20));
    out
}

fn top(m: &BTreeMap<String, u64>, n: usize) -> String {
    let mut v: Vec<_> = m.iter().collect();
    v.sort_by_key(|(k, c)| (std::cmp::Reverse(**c), (*k).clone()));
    let mut out = String::new();
    for (k, c) in v.into_iter().take(n) {
        out.push_str(&format!("  {k:<60} {c:>12}\n"));
    }
    out
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        100.0 * a as f64 / b as f64
    }
}

/// Whether a declared type's runtime values are all inside `allowed`.
///
/// The mapping from a declared type to a `Value` kind is exact for the builtin
/// heads and approximate for a nominal one: a user's `type Scan = { .. }` is a
/// `Value::Record` and a `type Tok = A | B` is a `Value::Ctor`, and this pass
/// cannot tell the two apart from the head alone, so any nominal type is
/// admitted exactly when **both** `Record` and `Ctor` are allowed. A `Type::Var`
/// is refused: an unresolved variable can be anything, including a closure.
///
/// > **Corrected in place (2026-08-31): the nominal fallback admitted three
/// > types it must not.** `Cell`, `Task` and `Secret` are `Type::Con`s with a
/// > name and arguments exactly as `Option` is, so the `_ =>` arm read them as
/// > ordinary nominal types and carried them at any rung allowing both `Record`
/// > and `Ctor` — including the rung named **"no world-handle"**, whose whole
/// > content is that a world handle does not cross. Measured on the tree this
/// > was found on: `ply test examples --no-cache -j 1` reports 20 `Secret`
/// > refusals from the DEEP walk's blocker histogram, against a shallow
/// > `type_carries` that admitted them, so the type-level row of ADR 0030 §6 is
/// > over-counted by at most that. It moved no figure quoted in that ADR to
/// > three digits and it is corrected anyway, because the number is what the
/// > next widening is chosen on.
/// >
/// > This function stays the LADDER's instrument — it answers "what would a
/// > rung with *these value kinds* allow" — and is not what ships.
/// > `compiled::CarriedTypes` is the shipping rule, and it walks a declaration's
/// > constructors rather than approximating a nominal type by its head.
pub(crate) fn type_carries(ty: &ply_core::ty::Type, allowed: &[&str]) -> bool {
    use ply_core::ty::Type;
    let has = |k: &str| allowed.contains(&k);
    match ty {
        Type::Var(_) => false,
        Type::Fn { .. } => false,
        Type::Record(fields) => has("Record") && fields.values().all(|t| type_carries(t, allowed)),
        Type::Con(name, args) => {
            let head = match name.as_str() {
                "Int" => "Int",
                "Bool" => "Bool",
                "Float" => "Float",
                "Decimal" => "Decimal",
                "String" => "Str",
                "Bytes" => "Bytes",
                "Unit" => "Unit",
                "List" => "List",
                "Map" => "Map",
                // Not nominal types, whatever their shape says.
                "Cell" | ply_core::prelude::TASK_TYPE | ply_core::ty::SECRET => return false,
                _ => {
                    return has("Record")
                        && has("Ctor")
                        && args.iter().all(|t| type_carries(t, allowed));
                }
            };
            has(head) && args.iter().all(|t| type_carries(t, allowed))
        }
    }
}

pub(crate) fn kind_in(v: &crate::value::Value, allowed: &[&str]) -> bool {
    allowed.contains(&value_kind(v))
}

/// The budget a deep argument test walks under, in `Value` nodes visited per
/// argument. `PLY_SEAM_DEEP_BUDGET` overrides it, because the answer depends on
/// it and a free parameter with one hard-coded value is a result nobody can
/// argue with.
pub fn deep_budget() -> u32 {
    static B: OnceLock<u32> = OnceLock::new();
    *B.get_or_init(|| {
        std::env::var("PLY_SEAM_DEEP_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256)
    })
}

/// The same question asked soundly: is every value reachable from `v` inside
/// `allowed`?
///
/// This is what a widening past `Bytes` would have to run on every argument of
/// every call, including the ones that go on to be refused. `Secret` is walked
/// rather than short-circuited so that a `Secret` nested in a record is refused
/// by the same rule as a bare one — `compiled::crossable` refuses `Secret` at
/// the top and would have to refuse it at any depth.
///
/// # Why it is bounded, measured rather than assumed
///
/// **Unbounded, it does not finish.** The ported Ply front end
/// (`spikes/ply-parser`) passes a `Scan` record whose fields transitively hold
/// the token list and the source bytes, so the walk is O(program state) per
/// call over ~1,000,000 calls. Run over `examples/desk.ply` on 2026-08-30 with
/// no budget: **no census output and no exit in 9 minutes 30 seconds**, against
/// **1.2 s** for the same probe with the shallow test — killed rather than
/// waited out. That is not a constant-factor tax that a later optimisation
/// trims; it is the wrong shape.
///
/// So the walk gives up at [`deep_budget`] nodes and answers "not carried",
/// which is the conservative direction and is what any shippable deep test
/// would also have to do. The budget is then a knob on the *fragment*, not on
/// the census: a call whose argument is one node past it is refused for a
/// reason that has nothing to do with what the argument holds.
pub(crate) fn kind_in_deep(v: &crate::value::Value, allowed: &[&str]) -> bool {
    let mut fuel = deep_budget();
    walk(v, allowed, &mut fuel)
}

/// What refused a deep walk: the kind of the first value outside `allowed`, or
/// `"<budget>"` if the walk ran out of fuel first.
///
/// "A sound deep test admits far fewer calls than a shallow one" is a number;
/// *why* is the fact a roadmap is read off, and it is a different fact. Without
/// this the collapse from the shallow rung to the deep one is attributable
/// equally to the budget and to the contents, and those two have opposite
/// remedies.
pub(crate) fn deep_blocker(v: &crate::value::Value, allowed: &[&str]) -> Option<&'static str> {
    let mut fuel = deep_budget();
    blocker(v, allowed, &mut fuel)
}

fn blocker(v: &crate::value::Value, allowed: &[&str], fuel: &mut u32) -> Option<&'static str> {
    use crate::value::Value::*;
    if *fuel == 0 {
        return Some("<budget>");
    }
    *fuel -= 1;
    if !allowed.contains(&value_kind(v)) {
        return Some(value_kind(v));
    }
    let mut children: Vec<&crate::value::Value> = Vec::new();
    match v {
        List(items) => children.extend(items.iter()),
        Map(m) => {
            for (k, x) in m.iter() {
                children.push(k);
                children.push(x);
            }
        }
        Record(fields) => children.extend(fields.values()),
        Ctor { args, .. } => children.extend(args.iter()),
        Secret(inner) => children.push(inner),
        _ => {}
    }
    children.into_iter().find_map(|c| blocker(c, allowed, fuel))
}

fn walk(v: &crate::value::Value, allowed: &[&str], fuel: &mut u32) -> bool {
    use crate::value::Value::*;
    if *fuel == 0 {
        return false;
    }
    *fuel -= 1;
    if !allowed.contains(&value_kind(v)) {
        return false;
    }
    match v {
        List(items) => items.iter().all(|x| walk(x, allowed, fuel)),
        Map(m) => m
            .iter()
            .all(|(k, x)| walk(k, allowed, fuel) && walk(x, allowed, fuel)),
        Record(fields) => fields.values().all(|x| walk(x, allowed, fuel)),
        Ctor { args, .. } => args.iter().all(|x| walk(x, allowed, fuel)),
        Secret(inner) => walk(inner, allowed, fuel),
        _ => true,
    }
}
