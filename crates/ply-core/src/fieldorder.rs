//! `W0611` — a `push` the machine has to perform by copying, because when it
//! runs something else still holds the list.
//!
//! ## The rule, stated over the mechanism rather than over a position
//!
//! `builtins.rs`'s `push` is `Arc::get_mut(list)`: it rewrites the list when it
//! is that list's only owner and copies the whole thing when it is not. Run the
//! copying branch in a fold and the fold is quadratic. So the question this
//! pass answers is one question — **when this `push` runs, can anything else
//! still see the list?** — and there are exactly two ways for the answer to be
//! yes.
//!
//! - **A pending frame is holding the scope.** [`ply_eval::rc::carry`]
//!   (`crates/ply-eval/src/rc.rs:98`) is `if remaining { env.clone() } else {
//!   Env::empty() }`, and all eight of its call sites pass `remaining` = *is
//!   there another sub-expression after this one*. When there is, the frame
//!   keeps the scope, `Env::take_unique` refuses at the first shared link, the
//!   read clones instead of moving, and the list is at two owners. This is
//!   positional, and it is the route [`Slot`] models.
//! - **An earlier sibling already read the list.** The pending frames
//!   accumulate the values they have — `Frame::AppArgs::done`
//!   (`crates/ply-eval/src/frame.rs:132`), `Frame::RecordField::done` (`:255`),
//!   `Frame::ListItem::done` (`:296`), `Frame::PerformArgs::done` (`:328`) — so
//!   a sibling evaluated *before* this one holds the list directly, whatever
//!   `carry` did. This route is not positional at all: it fires on a `push` in
//!   the **last** sub-expression of its node, which is the spelling the first
//!   route calls correct. [`Keeps`] models it.
//!
//! > **Corrected 2026-08-28. The account above used to run only to the first
//! > route, and this module claimed the resulting analysis was an
//! > over-approximation. Both are withdrawn. The summary's doc comment used to
//! > read:** *"Deliberately an **over**-approximation: any `push` anywhere in
//! > the body counts, whether or not its result reaches the return value. The
//! > direction is the whole point. An under-approximation would answer 'does
//! > not grow' for something that does, and a caller composing it in non-final
//! > position would get silence over exactly the space nobody looked at ... An
//! > over-approximation costs a false positive, which the flag, the note and
//! > the counter between them make cheap."*
//! >
//! > It was false in both directions at once, and each was measured with
//! > `rc::Stats` at n = 200 and n = 400 before it was written down here.
//! > Under-approximating: `fn step(s: S, i: Int) -> S = { a: s.a, b: push(s.a,
//! > i) }` measured `in_place` **0.0000** at both sizes and the pass was
//! > silent, because `s.a` is read into the record's first field and is still
//! > in `Frame::RecordField::done` when the `push` runs. Over-approximating in
//! > the other direction: `fn small(i: Int) -> Int = len(push([], i))` called
//! > at argument 0 of 2 measured **1.0000** at both sizes and the pass fired.
//! > An analysis that advertises a direction it does not have is worse than no
//! > analysis, so the claim is gone rather than repaired: see
//! > [What it is not](#what-it-is-not) for what stands in its place.
//!
//! ## It is not local
//!
//! ADR 0020 §5.2 measured a correctly written callee made quadratic by its
//! caller, where the trailing sub-expression that does it is a literal
//! constant: `carry` never asks what the remaining sub-expression *reads*. So
//! no property of a definition alone decides whether its `push` copies, and
//! this pass computes an interprocedural [`summarize`] summary before it walks
//! anything.
//!
//! ## What it is not
//!
//! **Neither sound nor complete, and the shapes it gets wrong are named here
//! with the number each one measures.** The first three rows were taken with
//! `ply run --json`'s `counters` on the machine engine at n = 200 and n = 400
//! and each is a row of `field_order_oracle.rs`'s probe table, which asserts
//! the disagreement — so closing one of these gaps is a red test and an edit to
//! this table rather than a silent improvement nobody records. The last two
//! rows have no counter that could measure them and say so.
//!
//! | shape | `in_place` | this pass | why |
//! | --- | ---: | --- | --- |
//! | `{ n: id(s.toks), toks: push(s.toks, i) }`, `fn id(x) = x` | 0.0000 | **silent — a miss** | the earlier sibling is a call. Deciding that `id` answers its argument is an interprocedural value analysis this pass does not have |
//! | `match map_get(m, k) { Some(vs) -> map_insert(m, k, push(vs, i)), .. }` | 0.0000 | **silent — a miss** | `map_get` clones the value out of the tree, so the list is at two owners before the `push` is written at all. `std.http`'s `add_field` is exactly this, and the same call-shaped blindness is the cause |
//! | `sink(push(mk(i), i), i)`, `fn mk(i) = [i]` | 1.0000 | **fires — a false positive** | the container is a call result. The carried scope may or may not be able to reach it, the pass cannot tell, and it prefers the firing |
//! | a `Map`, a `Bytes`, a `String` as the container | no counter exists | silent | `rc::note_update` is called from `push` and nowhere else, so `updates` counts `List` and nothing else. A firing on a `Map` would be a claim with no counter able to check it |
//! | a container that stays short | high | fires | `grouped_ten` in `spikes/ply-lexer-nesting` is that shape: O(bound) per step is linear. The pass cannot bound a list statically and the diagnostic says so, pointing at `counters.in_place` |
//!
//! The first two rows are one gap seen from two sides, and it is the honest
//! shape of the whole thing: the retention test is a **definite**-alias test.
//! It answers yes only when the same syntactic place is read twice, or read
//! into a literal, or captured by a closure — cases where immutability makes
//! the two mentions provably the same allocation. Anything that goes through a
//! call is invisible to it, whether the call is the sibling or the read.
//!
//! The false positive on the third row is the same blindness once more: `mk(i)` is
//! a call, so the pass cannot say the list is fresh, and between silence and a
//! firing it takes the firing — which is the one place it still chooses a
//! direction on purpose.
//!
//! The three measured rows are **asserted** rather than described:
//! `an_alias_through_a_call_is_a_known_miss` and
//! `a_push_onto_a_list_read_out_of_a_map_is_a_known_miss` in
//! `crates/ply-core/tests/field_order.rs`, and rows `F1`, `K1` and `M1` of
//! `field_order_oracle.rs`'s table, which measure each one at two sizes and
//! fail if the lint quietly starts agreeing.
//!
//! ## Handler clauses
//!
//! A `handle` body is analyzed as carried, because `handler::enter_handle`
//! puts `env.clone()` in the `Prompt` (`crates/ply-eval/src/handler.rs:156`)
//! and the prompt outlives the body. A clause body and a `return` clause body
//! are analyzed as **their own roots**: `handler::leave_handle` evaluates the
//! return arm under `prompt.env.bind(...)`, a scope built for it rather than
//! the enclosing expression's. `crates/ply-core/tests/field_order.rs` probes
//! all three; no counter covers them, and this sentence is the extent of the
//! claim.

use crate::scc::sccs;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, ExprKind, FnDef, Item, ModuleName, Program, QName, Stmt};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::{FxHashMap, FxHashSet};

/// The builtin whose copying branch this pass exists to predict.
const PUSH: &str = "push";

const RED_ZONE: usize = 256 * 1024;
const NEW_SEGMENT: usize = 2 * 1024 * 1024;

/// One `fn` in the program, keyed the way [`Resolved`] keys one.
struct Def<'a> {
    /// The fully-qualified name, which is what a resolved reference yields.
    qualified: Symbol,
    module: usize,
    def: &'a FnDef,
}

/// Where a sub-expression sits, in the only terms the machine cares about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Nothing follows it in its enclosing node, so `carry` hands the pending
    /// frame `Env::empty()` and the enclosing position's answer is inherited.
    Last,
    /// Another sub-expression follows, so the frame keeps the scope.
    Carried,
    /// A body entered with a scope built for it: a lambda, a handler clause.
    /// Its own analysis starts over.
    Barrier,
}

/// Which of a node's already-evaluated sub-expressions the pending frame is
/// still holding while this one runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    /// None: whatever ran before this child was consumed before it started.
    Nothing,
    /// Every earlier child, in order — the four frames that accumulate a
    /// `done` vector, plus `Frame::AppArgs::callee` and `Frame::BinaryApply`.
    Earlier,
    /// The node's first child only. `Frame::MatchGuard` keeps the scrutinee
    /// while a guard runs and drops it before the arm body; `open_cell` moves
    /// the initial value into the arena, where it outlives the body.
    First,
}

/// A binding and a path of field reads from it — `s`, `s.toks`, `s.a.b`.
///
/// Two mentions of one place answer the same allocation: a record is immutable
/// and `Frame::FieldAccess` clones the field out of it, so `s.toks` read twice
/// is one list at two owners. That is what makes [`Keeps::holds`] a *definite*
/// alias test rather than a guess, and it is why nothing here goes through a
/// call.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Place {
    root: Symbol,
    path: Vec<Symbol>,
}

impl Place {
    /// Whether a value at this place contains the value at `inner`.
    fn contains(&self, inner: &Place) -> bool {
        self.root == inner.root
            && self.path.len() <= inner.path.len()
            && self.path.iter().zip(&inner.path).all(|(a, b)| a == b)
    }

    /// Whether either place contains the other, which is the test for a call:
    /// the caller knows what it handed over but not which part of it the
    /// callee pushes onto.
    fn overlaps(&self, other: &Place) -> bool {
        self.contains(other) || other.contains(self)
    }
}

/// What the names in scope denote, as far as a syntactic pass can tell.
#[derive(Clone, Default)]
struct Names {
    /// Bound here, so a bare call to it is a local and not a definition.
    bound: FxHashSet<Symbol>,
    /// Bound directly to a place: `let ys = s.toks` records `ys -> s.toks`.
    alias: FxHashMap<Symbol, Place>,
    /// Denotes something the caller handed in. Seeded with the parameters and
    /// carried through `let` and `match` binders. A lambda's parameters clear
    /// it: whoever applies the lambda decides what they hold, and this
    /// definition's caller is not it.
    from_arg: FxHashSet<Symbol>,
}

impl Names {
    fn of(def: &FnDef) -> Names {
        let params: FxHashSet<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
        Names {
            bound: params.clone(),
            alias: FxHashMap::default(),
            from_arg: params,
        }
    }

    /// The place `e` denotes, with aliases resolved. `None` for anything that
    /// is not a name or a chain of field reads from one.
    fn place(&self, e: &Expr) -> Option<Place> {
        match &e.kind {
            ExprKind::Var(q) if q.is_bare() => Some(match self.alias.get(q.symbol()) {
                Some(p) => p.clone(),
                None => Place {
                    root: q.symbol().clone(),
                    path: Vec::new(),
                },
            }),
            ExprKind::Field { base, field } => {
                let mut place = self.place(base)?;
                place.path.push(field.name.clone());
                Some(place)
            }
            _ => None,
        }
    }

    /// Whether `e` denotes something this definition's caller handed it, which
    /// is the only thing a caller's carried frame can still be holding.
    fn caller_owned(&self, e: &Expr) -> bool {
        self.place(e)
            .is_some_and(|p| self.from_arg.contains(&p.root))
    }

    /// Every parameter this definition's caller handed in that `e` mentions,
    /// anywhere inside it.
    ///
    /// Looser than [`Names::caller_owned`] on purpose, and only used to decide
    /// a call-graph edge: `g(wrap(xs))` hands `g` a record built around the
    /// caller's list, and a `push` inside `g` onto a field of it copies exactly
    /// as if `xs` had been passed directly.
    fn args_mentioned(&self, e: &Expr) -> FxHashSet<Symbol> {
        let mut found = FxHashSet::default();
        mentions(e, &mut self.clone(), &mut found);
        found
    }

    /// Binds `name` to `value`, or to nothing known when `value` is `None`.
    ///
    /// The `None` case is a *removal* as much as an addition: a lambda
    /// parameter named `s` shadows a parameter named `s`, and treating the two
    /// as one place would be an alias claim about two different values.
    fn bind(&mut self, name: &Symbol, value: Option<&Expr>) {
        let place = value.and_then(|v| self.place(v));
        self.bound.insert(name.clone());
        match &place {
            Some(p) if self.from_arg.contains(&p.root) => {
                self.from_arg.insert(name.clone());
            }
            _ => {
                self.from_arg.remove(name);
            }
        }
        match place {
            Some(p) => self.alias.insert(name.clone(), p),
            None => self.alias.remove(name),
        };
    }

    fn bind_all(&mut self, names: impl IntoIterator<Item = Symbol>) {
        for name in names {
            self.bind(&name, None);
        }
    }
}

fn mentions(e: &Expr, names: &mut Names, found: &mut FxHashSet<Symbol>) {
    // Through the alias map, so `let ys = s.toks; g(ys)` is a mention of `s`
    // rather than of a name the caller never heard of. A place's only free
    // variable is its root, so there is nothing under it to descend into.
    if let Some(place) = names.place(e) {
        if names.from_arg.contains(&place.root) {
            found.insert(place.root);
        }
        return;
    }
    let mut inner = names.clone();
    for kid in children(e, &mut inner) {
        mentions(kid.expr, &mut inner.clone(), found);
    }
}

/// What a sub-expression's *value* still owns once it has been evaluated.
#[derive(Clone, Default)]
struct Keeps {
    /// Places the value definitely contains.
    places: Vec<Place>,
    /// The value is a closure, so it captured the whole scope and every place
    /// rooted at a binding is still reachable through it. Measured rather than
    /// assumed: a lambda *literal* as a record field reads `in_place` 0.0000
    /// while the same lambda passed to a call that returns an `Int` reads
    /// 0.9950, because only the first is still a value when the next field
    /// runs.
    scope: bool,
}

impl Keeps {
    /// Whether something here is still holding the list at `p`.
    fn holds(&self, p: &Place) -> bool {
        self.scope || self.places.iter().any(|q| q.contains(p))
    }

    /// Whether something here overlaps `p`, which is the weaker test a call
    /// gets: the pass knows what was handed over, not which part of it the
    /// callee pushes onto.
    fn touches(&self, p: &Place) -> bool {
        self.scope || self.places.iter().any(|q| q.overlaps(p))
    }

    fn absorb(&mut self, other: Keeps) {
        self.scope |= other.scope;
        for p in other.places {
            if !self.places.contains(&p) {
                self.places.push(p);
            }
        }
    }

    /// What both branches of a choice keep. Definite, so an intersection: a
    /// value that is `s.a` on one path and `s.b` on the other keeps neither.
    fn common(self, other: Keeps) -> Keeps {
        Keeps {
            places: self
                .places
                .into_iter()
                .filter(|p| other.places.contains(p))
                .collect(),
            scope: self.scope && other.scope,
        }
    }
}

/// What `e`'s value definitely still owns.
///
/// Only places rooted at a name this scope can see survive. A `match` arm's
/// binder and a `let` inside a block are gone by the time the value is read,
/// and a place named after one of them would be an alias claim about a binding
/// that no longer exists.
fn keeps(names: &Names, e: &Expr) -> Keeps {
    let mut out = keeps_inner(names, e);
    out.places.retain(|p| names.bound.contains(&p.root));
    out
}

fn keeps_inner(names: &Names, e: &Expr) -> Keeps {
    if let Some(place) = names.place(e) {
        return Keeps {
            places: vec![place],
            scope: false,
        };
    }
    match &e.kind {
        // A closure captures the scope it was built in, by pointer.
        ExprKind::Lambda { .. } => Keeps {
            places: Vec::new(),
            scope: true,
        },
        // A literal's value contains its elements'.
        ExprKind::List { items } => items.iter().fold(Keeps::default(), |mut acc, item| {
            acc.absorb(keeps(names, item));
            acc
        }),
        ExprKind::Record { fields } => fields.iter().fold(Keeps::default(), |mut acc, (_, f)| {
            acc.absorb(keeps(names, f));
            acc
        }),
        ExprKind::If {
            then_branch,
            else_branch,
            ..
        } => keeps(names, then_branch).common(keeps(names, else_branch)),
        ExprKind::Match { arms, .. } => arms
            .iter()
            .map(|arm| keeps(names, &arm.body))
            .reduce(Keeps::common)
            .unwrap_or_default(),
        ExprKind::Block { tail, .. } => tail
            .as_ref()
            .map_or_else(Keeps::default, |t| keeps(names, t)),
        // Everything else — a call above all — is where the gap is. See the
        // module comment's table.
        _ => Keeps::default(),
    }
}

/// Every `W0611` in the program, in source order per module.
///
/// `program` must be complete — every module parsed. A module whose AST is
/// absent contributes no definitions, and a callee this pass cannot see would
/// answer "does not grow", which is a silent miss rather than a loud one. The
/// caller is responsible for completing the parse first; `ply check
/// --field-order` does.
pub fn check(program: &Program, resolved: &Resolved) -> Vec<Diagnostic> {
    check_where(program, resolved, |_| true)
}

/// [`check`], reporting only in the modules `report` accepts.
///
/// The **summary** is still computed over the whole program, and that is not an
/// optimization detail: the trap is a property of a composition, so a caller in
/// a project and a callee in the standard library are one question. Narrowing
/// the call graph to the reported modules would answer "does not grow" for
/// every library function and silently lose every firing that crosses the
/// boundary. Only the reporting is scoped.
pub fn check_where(
    program: &Program,
    resolved: &Resolved,
    report: impl Fn(&ModuleName) -> bool,
) -> Vec<Diagnostic> {
    let defs = index(program, resolved);
    let by_key: FxHashMap<Symbol, usize> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.qualified.clone(), i))
        .collect();
    let grows = summarize(&defs, resolved, &by_key);

    let mut wanted = vec![true; program.modules.len()];
    for module in &program.modules {
        if let Some(i) = resolved.index_of(&module.name) {
            wanted[i] = report(&module.name);
        }
    }

    let mut out = Vec::new();
    for def in &defs {
        if !wanted.get(def.module).copied().unwrap_or(true) {
            continue;
        }
        walk_def(resolved, &by_key, &grows, def, &mut out);
    }
    out
}

fn walk_def(
    resolved: &Resolved,
    by_key: &FxHashMap<Symbol, usize>,
    grows: &[Vec<bool>],
    def: &Def<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let mut walk = Walk {
        resolved,
        by_key,
        grows,
        module: def.module,
        names: Names::of(def.def),
        out,
    };
    walk.expr(&def.def.body, true, &Keeps::default());
}

fn index<'a>(program: &'a Program, resolved: &Resolved) -> Vec<Def<'a>> {
    let mut defs = Vec::new();
    for module in &program.modules {
        let Some(index) = resolved.index_of(&module.name) else {
            continue;
        };
        for item in &module.items {
            // `Derive` and `EffectSet` declare nothing a reference can reach;
            // a `derive` has already been expanded into ordinary `Fn` items.
            let Item::Fn(def) = item else { continue };
            let Some(binding) = resolved
                .declared
                .get(index)
                .and_then(|b| b.get(Namespace::Value, &def.name.name))
            else {
                continue;
            };
            defs.push(Def {
                qualified: binding.qualified.clone(),
                module: index,
                def,
            });
        }
    }
    defs
}

/// Which of a definition's parameters a call to it can `push` onto.
///
/// One `bool` per parameter, and the per-parameter shape is not decoration.
/// `fn late(xs: List<Int>, i: Int) -> List<Int> = sink_snd(i, grow(xs, i))`
/// hands `grow` two arguments and keeps the value of one of them, `i`, in
/// `Frame::AppArgs::done` while the call runs. A summary that only said "`grow`
/// grows" would see a kept value overlapping an argument and fire; the measured
/// answer is `in_place` **1.0000**, because what `grow` pushes onto is `xs` and
/// nothing kept `xs`.
///
/// The condition is *caller-reachable*, not *any push at all*, and that too is
/// measured rather than argued: `fn build(k: Int) -> List<Int> = fold(range(0,
/// k), [], |a, j| push(a, j))` pushes ten times per call onto an accumulator
/// that starts at `[]` inside its own body. Its caller cannot see that list, so
/// a call to `build` in a carried position copies nothing — `in_place`
/// **1.0000** at n = 200 and n = 400 — and a summary that said "grows" would
/// fire on it.
///
/// The call-graph edge is drawn on the looser test,
/// [`Names::args_mentioned`]: what this definition hands a growing callee may
/// be a record built around the caller's list rather than the list itself.
fn summarize(
    defs: &[Def<'_>],
    resolved: &Resolved,
    by_key: &FxHashMap<Symbol, usize>,
) -> Vec<Vec<bool>> {
    let mut grows: Vec<Vec<bool>> = Vec::with_capacity(defs.len());
    let mut calls: Vec<Vec<Call>> = Vec::with_capacity(defs.len());
    for def in defs {
        let mut found = Found {
            resolved,
            by_key,
            module: def.module,
            names: Names::of(def.def),
            at: def
                .def
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| (p.name.name.clone(), i))
                .collect(),
            grows_at: vec![false; def.def.params.len()],
            calls: Vec::new(),
        };
        found.expr(&def.def.body);
        grows.push(found.grows_at);
        calls.push(found.calls);
    }

    let adj: Vec<Vec<usize>> = calls
        .iter()
        .map(|cs| cs.iter().map(|c| c.callee).collect())
        .collect();

    // Reverse topological order, so a component is reached after everything it
    // calls. A recursive or mutually recursive group is one component and is
    // iterated to a fixpoint inside itself.
    for component in sccs(defs.len(), &adj) {
        loop {
            let mut moved = false;
            for &i in &component {
                for call in &calls[i] {
                    for (at, mine) in call.per_arg.iter().enumerate() {
                        if !grows[call.callee].get(at).copied().unwrap_or(false) {
                            continue;
                        }
                        for &k in mine {
                            if !grows[i][k] {
                                grows[i][k] = true;
                                moved = true;
                            }
                        }
                    }
                }
            }
            if !moved {
                break;
            }
        }
    }
    grows
}

/// A call this definition makes, and which of *its* parameters each argument
/// carries.
struct Call {
    callee: usize,
    per_arg: Vec<Vec<usize>>,
}

/// What a definition's body hands its callees, and which of its parameters it
/// pushes onto.
struct Found<'a> {
    resolved: &'a Resolved,
    by_key: &'a FxHashMap<Symbol, usize>,
    module: usize,
    names: Names,
    /// The parameters, by name, in declaration order.
    at: FxHashMap<Symbol, usize>,
    grows_at: Vec<bool>,
    calls: Vec<Call>,
}

impl Found<'_> {
    fn expr(&mut self, e: &Expr) {
        stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || {
            if let ExprKind::App { func, args } = &e.kind
                && let ExprKind::Var(q) = &func.kind
            {
                match classify(self.resolved, self.by_key, &self.names, self.module, q) {
                    Callee::Push => {
                        if let Some(root) = args
                            .first()
                            .and_then(|c| self.names.place(c))
                            .map(|p| p.root)
                            && self.names.from_arg.contains(&root)
                            && let Some(&k) = self.at.get(&root)
                        {
                            self.grows_at[k] = true;
                        }
                    }
                    Callee::Def(callee) => {
                        let per_arg: Vec<Vec<usize>> = args
                            .iter()
                            .map(|a| {
                                self.names
                                    .args_mentioned(a)
                                    .iter()
                                    .filter_map(|n| self.at.get(n).copied())
                                    .collect()
                            })
                            .collect();
                        if per_arg.iter().any(|m| !m.is_empty()) {
                            self.calls.push(Call { callee, per_arg });
                        }
                    }
                    Callee::Opaque => {}
                }
            }
            let mut names = self.names.clone();
            for kid in children(e, &mut names) {
                let outer = std::mem::replace(&mut self.names, names.clone());
                self.expr(kid.expr);
                self.names = outer;
            }
            self.names = names;
        });
    }
}

/// The positional walk.
struct Walk<'a> {
    resolved: &'a Resolved,
    by_key: &'a FxHashMap<Symbol, usize>,
    grows: &'a [Vec<bool>],
    module: usize,
    names: Names,
    out: &'a mut Vec<Diagnostic>,
}

impl Walk<'_> {
    /// `reached_last` is cumulative: it is true only when **every** node
    /// between here and the definition's body put this sub-expression last.
    /// One carried frame anywhere up the chain is enough, which is why the flag
    /// is threaded rather than recomputed — `f(g(0, push(xs, i)), 1)` copies
    /// even though the `push` is last inside `g`.
    ///
    /// `held` is cumulative for the same reason and is a different fact: the
    /// values every enclosing node has already computed and not yet consumed.
    /// A barrier resets both, because a lambda's body runs under a scope built
    /// for it and after those frames are gone.
    fn expr(&mut self, e: &Expr, reached_last: bool, held: &Keeps) {
        stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || {
            self.report(e, reached_last, held);

            let mut names = self.names.clone();
            let kids = children(e, &mut names);
            let mut earlier = Keeps::default();
            let mut first: Option<Keeps> = None;
            for (at, kid) in kids.iter().enumerate() {
                let (child_last, mut child_held) = match kid.slot {
                    Slot::Last => (reached_last, held.clone()),
                    Slot::Carried => (false, held.clone()),
                    Slot::Barrier => (true, Keeps::default()),
                };
                if kid.slot != Slot::Barrier {
                    child_held.absorb(match kid.held {
                        Held::Nothing => Keeps::default(),
                        Held::Earlier => earlier.clone(),
                        Held::First => first.clone().unwrap_or_default(),
                    });
                }
                let outer = std::mem::replace(&mut self.names, names.clone());
                self.expr(kid.expr, child_last, &child_held);
                self.names = outer;

                let mine = keeps(&names, kid.expr);
                if at == 0 {
                    first = Some(mine.clone());
                }
                earlier.absorb(mine);
            }
            self.names = names;
        });
    }

    fn report(&mut self, e: &Expr, reached_last: bool, held: &Keeps) {
        let ExprKind::App { func, args } = &e.kind else {
            return;
        };
        let ExprKind::Var(q) = &func.kind else {
            return;
        };
        match classify(self.resolved, self.by_key, &self.names, self.module, q) {
            Callee::Push => {
                let container = args.first();
                // A list this expression just built has exactly one owner, so
                // `Arc::get_mut` succeeds however many frames hold the scope:
                // the carried scope holds bindings, and a fresh list is in
                // none of them. Firing there would be a false positive with a
                // mechanism behind it.
                let carried = !reached_last && !container.is_some_and(fresh);
                let aliased = container
                    .and_then(|c| self.names.place(c))
                    .is_some_and(|p| held.holds(&p));
                if aliased {
                    self.out.push(self.alias_copy(e.span, "this `push`"));
                } else if carried {
                    self.out.push(self.carried_copy(e.span, "this `push`"));
                }
            }
            Callee::Def(i) if self.grows[i].iter().any(|&g| g) => {
                // Only the arguments the callee can actually push onto. An
                // argument it merely reads is in the pending frame too, and
                // asking about that one is what would fire on `sink_snd(i,
                // grow(xs, i))`, whose measured `in_place` is 1.0000.
                let aliased = args
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| self.grows[i].get(*at).copied().unwrap_or(false))
                    .filter_map(|(_, a)| self.names.place(a))
                    .any(|p| held.touches(&p));
                let what = format!("`{q}`");
                if aliased {
                    self.out.push(self.alias_copy(e.span, &what));
                } else if !reached_last {
                    self.out.push(self.carried_copy(e.span, &what));
                }
            }
            Callee::Def(_) | Callee::Opaque => {}
        }
    }

    /// Route one: a pending frame is holding the scope the list is read from.
    fn carried_copy(&self, span: Span, what: &str) -> Diagnostic {
        Diagnostic::warning(
            codes::FIELD_ORDER_COPY,
            format!("{what} copies the list instead of growing it"),
        )
        .primary(span, "not the last sub-expression of its enclosing node")
        .note(
            "a pending frame holds the scope while anything after this one is evaluated, so the \
             list is at two owners and `push` copies it; in a loop that is quadratic",
        )
        .note(
            "move it into the last position of its enclosing node — last record field, last call \
             argument, last list element",
        )
        .note(
            "`ply run --json --engine machine` reports `counters.in_place`: it is near 0 when this \
             is costing what it looks like it costs, and near 1 when the list is short enough not \
             to matter. Under `--engine treewalk` it is `null`, because that evaluator never moves \
             a value out of a scope and the ratio would be about the evaluator",
        )
    }

    /// Route two: an earlier sub-expression of the enclosing node read the
    /// same list and its value is still in the pending frame.
    fn alias_copy(&self, span: Span, what: &str) -> Diagnostic {
        Diagnostic::warning(
            codes::FIELD_ORDER_COPY,
            format!("{what} copies the list instead of growing it"),
        )
        .primary(
            span,
            "an earlier sub-expression of this node is still holding the list",
        )
        .note(
            "the pending frame accumulates the values it already has, so a sibling evaluated \
             before this one holds the list directly and `push` copies; moving this one last does \
             not help, because it is already last",
        )
        .note(
            "read the list once: build the new one first and derive the other use from it, or \
             accept the copy where both uses are genuinely wanted",
        )
        .note(
            "`ply run --json --engine machine` reports `counters.in_place`: it is near 0 when this \
             is costing what it looks like it costs, and near 1 when the list is short enough not \
             to matter. Under `--engine treewalk` it is `null`, because that evaluator never moves \
             a value out of a scope and the ratio would be about the evaluator",
        )
    }
}

/// What a call's callee is.
enum Callee {
    /// The `push` builtin, not shadowed.
    Push,
    /// A definition of this program, by index.
    Def(usize),
    /// A local, another builtin, or a name that does not resolve. Nothing is
    /// claimed about it.
    Opaque,
}

fn classify(
    resolved: &Resolved,
    by_key: &FxHashMap<Symbol, usize>,
    names: &Names,
    module: usize,
    q: &QName,
) -> Callee {
    // A local wins unconditionally, and `Resolved::lookup`'s contract is that
    // it is only asked about names local lookup already missed.
    if q.is_bare() && names.bound.contains(q.symbol()) {
        return Callee::Opaque;
    }
    if let Ok(binding) = resolved.lookup(module, Namespace::Value, q)
        && let Some(&i) = by_key.get(&binding.qualified)
    {
        return Callee::Def(i);
    }
    // Only now: a module that declares its own `push` shadows the prelude, so
    // the resolved answer above is consulted first.
    if q.is_bare() && q.symbol().as_str() == PUSH {
        return Callee::Push;
    }
    Callee::Opaque
}

/// A container this expression just built, which nothing else can reach.
///
/// `push(push([], a), b)` is the builder idiom and both of its pushes are in
/// place whatever frame holds the scope, so neither is a firing.
fn fresh(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::List { .. } => true,
        ExprKind::App { func, args } => {
            matches!(&func.kind, ExprKind::Var(q) if q.is_bare() && q.symbol().as_str() == PUSH)
                && args.first().is_some_and(fresh)
        }
        _ => false,
    }
}

/// One sub-expression of a node.
struct Kid<'a> {
    expr: &'a Expr,
    slot: Slot,
    held: Held,
}

fn carried(expr: &Expr) -> Kid<'_> {
    Kid {
        expr,
        slot: Slot::Carried,
        held: Held::Nothing,
    }
}

fn last(expr: &Expr) -> Kid<'_> {
    Kid {
        expr,
        slot: Slot::Last,
        held: Held::Nothing,
    }
}

/// Every sub-expression of `e`, with the slot the machine evaluates it in,
/// which earlier siblings are still in the pending frame while it runs, and the
/// binders `e` adds to the scope its children see.
///
/// **Exhaustive, with no `_` arm on purpose.** A node kind added later must not
/// default to "last, holding nothing": that would be silence over a shape
/// nobody considered, which is the defect class this pass exists to close. It
/// must fail to compile instead.
///
/// Every row is read off the evaluator rather than off the language reference,
/// and every anchor below was re-checked against the file it names:
///
/// | node | slot | held | where the evaluator says so |
/// | --- | --- | --- | --- |
/// | `Binary` | `lhs` carried, `rhs` last | `rhs` holds `lhs` | `machine.rs:1009` pushes `Frame::BinaryRhs` with `env.clone()`; `frame.rs:60`/`:73` then push `Frame::BinaryApply { lhs: value }` while `rhs` runs. No `BinOp` answers a `List` — `Concat` is `String -> String` (`infer.rs:3311`) — so the held value is never one |
/// | `App` | `func` carried when there are arguments, every argument but the last carried | each argument holds the callee and the arguments before it | `machine.rs:1035` `carry(&env, !args.is_empty())`; `frame.rs:107`, `:142`; `frame.rs:132` `done.push(value)` |
/// | `If` | `cond` carried, branches last | nothing | `machine.rs:1054` pushes `Frame::If` with `env.clone()`; `frame.rs:169` consumes the condition's value before evaluating a branch |
/// | `Match` | `scrutinee` carried, each guard carried, arm bodies last | a guard holds the scrutinee | `machine.rs:1068` `Frame::MatchArms`; `machine.rs:2207` `Frame::MatchGuard { scrutinee, .. }`, dropped before `arms[at].body` |
/// | `Block` | every statement carried, the tail last | nothing — a statement's value is discarded or bound, and a binding is tracked by [`Names`] | `machine.rs:2162` pushes `Frame::BlockStep` holding `scope.release(dead)`, which is why GAPS.md §1 column 4 measures a `let` failing to rescue it |
/// | `Record` | every field but the last carried | each field holds the fields before it | `machine.rs:1092`, `frame.rs:263`; `frame.rs:255` `done.push(..)` |
/// | `List` | every item but the last carried | each item holds the items before it | `machine.rs:1122`, `frame.rs:301`; `frame.rs:296` `done.push(value)` |
/// | `Perform` | every argument but the last carried | each argument holds the ones before it | `handler.rs:208`; `frame.rs:328` `done.push(value)` |
/// | `Handle` | the body carried, every clause a barrier | nothing | `handler.rs:156` puts `env.clone()` in the `Prompt`, which outlives the body; `leave_handle` builds a clause's scope from `prompt.env` |
/// | `WithCell` | `init` carried, the body last | the body holds `init` | `handler.rs:392` pushes `Frame::WithCellBody` with `env.clone()`; `handler.rs:429` moves the initial value into the arena, where the cell owns it for the whole body |
/// | `Unary`, `Field`, `WithRegion`, `Simulate` | last | nothing | `machine.rs:997` `Frame::Unary` and `machine.rs:1109` `Frame::FieldAccess` carry neither a scope nor a value, and no frame on the region or simulation paths does either |
fn children<'a>(e: &'a Expr, names: &mut Names) -> Vec<Kid<'a>> {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => Vec::new(),

        ExprKind::Binary { lhs, rhs, .. } => vec![
            carried(lhs),
            Kid {
                expr: rhs,
                slot: Slot::Last,
                held: Held::Earlier,
            },
        ],

        ExprKind::Unary { operand, .. } => vec![last(operand)],

        ExprKind::Lambda { params, body } => {
            names.bind_all(params.iter().map(|p| p.name.name.clone()));
            vec![Kid {
                expr: body,
                slot: Slot::Barrier,
                held: Held::Nothing,
            }]
        }

        ExprKind::App { func, args } => {
            let mut out = vec![if args.is_empty() {
                last(func)
            } else {
                carried(func)
            }];
            out.extend(positional(args));
            out
        }

        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => vec![carried(cond), last(then_branch), last(else_branch)],

        ExprKind::Match { scrutinee, arms } => {
            let mut out = vec![carried(scrutinee)];
            for arm in arms {
                // A pattern's binders are in scope for the guard and the body.
                // They are added to the shared set rather than per arm: a name
                // wrongly believed local only ever yields `Callee::Opaque`,
                // which under-reports on that one call, and arms cannot see
                // each other's binders in any program that type-checks.
                let mut binders = FxHashSet::default();
                pattern_binders(&arm.pat, &mut binders);
                let from_scrutinee = names.caller_owned(scrutinee);
                for b in binders {
                    names.bind(&b, None);
                    if from_scrutinee {
                        names.from_arg.insert(b);
                    }
                }
                if let Some(guard) = &arm.guard {
                    out.push(Kid {
                        expr: guard,
                        slot: Slot::Carried,
                        held: Held::First,
                    });
                }
                out.push(last(&arm.body));
            }
            out
        }

        ExprKind::Block { stmts, tail } => {
            let mut out = Vec::with_capacity(stmts.len() + 1);
            for stmt in stmts {
                match stmt {
                    Stmt::Let { pat, value, .. } => {
                        out.push(carried(value));
                        bind_pattern(names, pat, value);
                    }
                    Stmt::Expr(value) => out.push(carried(value)),
                }
            }
            if let Some(tail) = tail {
                out.push(last(tail));
            }
            out
        }

        ExprKind::Record { fields } => positional(fields.iter().map(|(_, e)| e)),

        ExprKind::Field { base, .. } => vec![last(base)],

        ExprKind::List { items } => positional(items),

        ExprKind::Perform { args, .. } => positional(args),

        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            let mut out = vec![carried(body)];
            for clause in clauses {
                names.bind_all(clause.params.iter().map(|p| p.name.clone()));
                if let Some(resume) = &clause.resume {
                    names.bind(&resume.name, None);
                }
                out.push(Kid {
                    expr: &clause.body,
                    slot: Slot::Barrier,
                    held: Held::Nothing,
                });
            }
            if let Some(ret) = return_clause {
                names.bind(&ret.binder.name, None);
                out.push(Kid {
                    expr: &ret.body,
                    slot: Slot::Barrier,
                    held: Held::Nothing,
                });
            }
            out
        }

        ExprKind::WithCell {
            init, binder, body, ..
        } => {
            let init = carried(init);
            names.bind(&binder.name, None);
            vec![
                init,
                Kid {
                    expr: body,
                    slot: Slot::Last,
                    held: Held::First,
                },
            ]
        }

        ExprKind::WithRegion { body, .. } => vec![last(body)],

        ExprKind::Simulate { body } => vec![last(body)],
    }
}

/// A left-to-right sequence in which only the last sub-expression is evaluated
/// with no frame holding the scope, and each holds the values of the ones
/// before it.
fn positional<'a, I>(items: I) -> Vec<Kid<'a>>
where
    I: IntoIterator<Item = &'a Expr>,
    I::IntoIter: ExactSizeIterator,
{
    let items = items.into_iter();
    let last = items.len().saturating_sub(1);
    items
        .enumerate()
        .map(|(i, e)| Kid {
            expr: e,
            slot: if i == last { Slot::Last } else { Slot::Carried },
            held: Held::Earlier,
        })
        .collect()
}

/// Binds a `let`'s pattern. A plain name takes the value's place as an alias;
/// anything destructured is bound with nothing known about it.
fn bind_pattern(names: &mut Names, pat: &ply_syntax::ast::Pattern, value: &Expr) {
    use ply_syntax::ast::PatternKind;
    if let PatternKind::Var(id) = &pat.kind {
        names.bind(&id.name, Some(value));
        return;
    }
    let mut binders = FxHashSet::default();
    pattern_binders(pat, &mut binders);
    let from_value = names.caller_owned(value);
    for b in binders {
        names.bind(&b, None);
        if from_value {
            names.from_arg.insert(b);
        }
    }
}

fn pattern_binders(pat: &ply_syntax::ast::Pattern, out: &mut FxHashSet<Symbol>) {
    use ply_syntax::ast::PatternKind;
    match &pat.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(id) => {
            out.insert(id.name.clone());
        }
        PatternKind::Ctor { args, .. } => {
            for arg in args {
                pattern_binders(arg, out);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (_, p) in fields {
                pattern_binders(p, out);
            }
        }
        PatternKind::List { items, rest } => {
            for p in items {
                pattern_binders(p, out);
            }
            if let Some(rest) = rest {
                pattern_binders(rest, out);
            }
        }
    }
}

/// One firing, with the definition it sits in.
///
/// The definition is what the rule is *about* — the trap is a property of a
/// composition, and "which function do I have to rewrite" is the question a
/// firing answers. `crates/ply-eval/tests/field_order_oracle.rs` compares this
/// set against `rc::Stats` per definition, which is only possible because the
/// name comes out with the span.
#[derive(Clone, Debug)]
pub struct Firing {
    /// The fully-qualified name of the definition the site is in.
    pub definition: Symbol,
    /// The simple name, as written.
    pub simple: Symbol,
    pub span: Span,
}

/// Every `W0611` site, with the definition it is in.
pub fn firings(program: &Program, resolved: &Resolved) -> Vec<Firing> {
    let defs = index(program, resolved);
    let by_key: FxHashMap<Symbol, usize> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.qualified.clone(), i))
        .collect();
    let grows = summarize(&defs, resolved, &by_key);

    let mut out = Vec::new();
    for def in &defs {
        let mut found = Vec::new();
        walk_def(resolved, &by_key, &grows, def, &mut found);
        out.extend(found.iter().filter_map(|d| {
            Some(Firing {
                definition: def.qualified.clone(),
                simple: def.def.name.name.clone(),
                span: d.primary_span()?,
            })
        }));
    }
    out
}
