//! A count of what the compiled seam is offered and what refuses it.

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

/// Turns the census on for the rest of the process, for a harness that cannot set an environment
/// variable before the first machine runs.
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

/// What the shipping type gate alone admits, for the test that reads it against `admitted`.
pub fn type_gated_shipping() -> u64 {
    cell().lock().expect("census").type_gated_shipping
}

/// [`Counts::carried_sig_walked`], for the test that reads it against `admitted_carried_sig`.
pub fn carried_sig_walked() -> u64 {
    cell().lock().expect("census").carried_sig_walked
}

#[derive(Default)]
pub struct Counts {
    pub body_calls: u64,
    pub builtin_calls: u64,
    pub ctor_calls: u64,
    pub admitted: u64,
    /// Of `admitted`, those whose whole declared signature is carried by the seam — what
    /// `backend::Reference` would actually answer rather than be offered and decline.
    pub admitted_carried_sig: u64,
    /// The same question asked by a **walk** over the declared types rather than by reading
    /// `compiled::CarriedTypes`'s per-definition table.
    pub carried_sig_walked: u64,
    pub frame_ceiling: u64,
    pub gates: BTreeMap<&'static str, u64>,
    /// For `ArgumentShape`: every argument `compiled::crossable` refuses, by kind.
    pub blocking_args: BTreeMap<&'static str, u64>,
    /// For `ArgumentType`: what in the declared parameter types refused it — see
    /// `compiled::CarriedTypes::refusal`.
    pub blocking_types: BTreeMap<&'static str, u64>,
    /// Admitted calls by name.
    pub admitted_names: BTreeMap<String, u64>,
    /// Refused calls by `name @ gate`.
    pub refused_names: BTreeMap<String, u64>,
    pub builtin_names: BTreeMap<&'static str, u64>,
    /// Counterfactual widenings of `compiled::crossable`, by level name: calls whose arguments all
    /// pass that level *and* clear every other gate.
    pub widened: BTreeMap<&'static str, u64>,
    /// The same, and the definition's declared return type also passes — so a native body would
    /// have something it could hand back.
    pub widened_returnable: BTreeMap<&'static str, u64>,
    /// For the widest DEEP rung: what refused each call whose arguments the *shallow* rung would
    /// have carried.
    pub deep_blockers: BTreeMap<&'static str, u64>,
    /// The third design: clear every gate but the shape one, and decide the arguments from the
    /// definition's **declared parameter types** instead of from the values.
    pub type_gated: u64,
    pub type_gated_and_return: u64,
    /// The type gate **as it ships** — `compiled::CarriedTypes` — asked with the value-kind test
    /// removed, so the two halves of `Gate::ArgumentShape` and `Gate::ArgumentType` can be
    /// separated.
    pub type_gated_shipping: u64,
    /// Calls a backend actually *entered*, by name — the subset of `admitted_names` whose name the
    /// backend's registry also holds.
    pub entered_names: BTreeMap<String, u64>,
}

/// The widening ladder, coarsest last.
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

/// The whole census, as lines on stderr.
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

/// The budget a deep argument test walks under, in `Value` nodes visited per argument.
pub fn deep_budget() -> u32 {
    static B: OnceLock<u32> = OnceLock::new();
    *B.get_or_init(|| {
        std::env::var("PLY_SEAM_DEEP_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256)
    })
}

/// The same question asked soundly: is every value reachable from `v` inside `allowed`?
pub(crate) fn kind_in_deep(v: &crate::value::Value, allowed: &[&str]) -> bool {
    let mut fuel = deep_budget();
    walk(v, allowed, &mut fuel)
}

/// What refused a deep walk: the kind of the first value outside `allowed`, or `"<budget>"` if the
/// walk ran out of fuel first.
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
