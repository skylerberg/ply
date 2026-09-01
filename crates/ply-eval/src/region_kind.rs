//! Which of ADR 0017 §3's two kinds each region in a program is.
//!
//! A region is `unique` when the compiler can prove no continuation is captured
//! across it, and `shared` otherwise. `unique` costs a bump pointer and nothing
//! else, because nothing can reach its slots after its close; `shared` keeps its
//! slots alive past that close, because a continuation may be resumed there and
//! read them.
//!
//! The two kinds do **not** decide what a resumption observes. State is threaded
//! — ADR 0005 §3, and ADR 0017 §3 as amended — so resumption *n* observes
//! resumption *n−1*'s writes under either kind.
//!
//! # The direction the imprecision runs
//!
//! Inferring `shared` where `unique` would have done retains memory that could
//! have been freed at the region's close. Inferring `unique` where a capture is
//! reachable *frees* memory a continuation can still reach, and what a reader
//! then sees is a resumption reading a slot another region has since been handed
//! — silent, and unrecoverable. So every case this analysis cannot decide lands
//! on `shared`:
//!
//! - a call whose callee is not a statically known definition, because the value
//!   could be any closure in the program — and a **local binding shadowing a
//!   definition's name is not that definition**. `fn go(helper: (Int) -> Int)`
//!   in a module that also declares `fn helper(n: Int)` calls whatever the
//!   caller passed, so the region holding the call is `shared`, not `unique`
//!   over `helper`'s body. [`Analysis::locals`] is what keeps the two apart, and
//!   it over-approximates on purpose: a pattern cannot be told from a nullary
//!   constructor without resolution, and reading a constructor as a local costs
//!   only precision;
//! - an argument to a callback builtin (`map`, `filter`, `fold`, `map_fold`,
//!   `bytes_position`) that is not a lambda written at the call site or a
//!   definition named there, for the same reason;
//! - a `perform` no `handle` **inside the region** answers, because the handler
//!   that answers it is outside and the capture therefore crosses the region's
//!   boundary — which is exactly what `across` means. A `handle` that lexically
//!   encloses the region does not make its operations local: `handle { with_cell
//!   [r](0) { c -> .. amb.flip() .. } } with { .. resume k -> k(true) + k(false) }`
//!   allocates one cell before the capture and writes it from both resumptions,
//!   so `r` is `shared`;
//! - **a tail-resumptive clause, not only a general one.** ADR 0005 §1.3 runs
//!   `K.capture(n)` for both clause forms, so the continuation exists either way
//!   and either way it can outlive the region's close. A rule that counted only
//!   `resume` binders would call `state.put[s](v) -> cell_set(c, v)`'s region
//!   `unique` and free memory that continuation still reaches;
//! - `simulate`, and every `task` operation, because the scheduler parks and
//!   resumes a task and that is a capture whoever wrote it;
//! - a region whose span this analysis never saw, which [`Regions::kind`]
//!   answers `shared` for.
//!
//! Two cases are precise in the useful direction, and together they are ADR
//! 0017 §3's "common case":
//!
//! - a region that performs nothing and installs no handler is `unique`,
//!   however much it allocates and however deeply it nests. `cell_get` and
//!   `cell_set` are builtins rather than operations, so a region that is pure
//!   computation over its own cells stays `unique`;
//! - a program with **no** handler, `simulate` or `task` operation anywhere has
//!   no capture for an unknown callee to reach, so every region in it is
//!   `unique` however higher-order it is.
//!
//! [`Cause`] records *which* of these fired, so a later refinement — a clause
//! that provably resumes at most once, say — can raise precision without
//! changing what the safe default is.
//!
//! # What that costs, measured
//!
//! Over `examples/` and the `std` modules they import: **113 regions, 0
//! `unique`, 113 `shared`, every one of them because of a tail-resumptive
//! clause.** The canonical Ply region is `with_cell[s](0) { c -> handle .. }` —
//! a cell backing a handler — so the clause rule alone accounts for all of
//! them, and ADR 0017 §3's "this is the common case and it is free" does not
//! survive contact with the corpus under the conservative reading.
//!
//! That is a finding rather than a bug in this module. What would move it is a
//! rule that decided a continuation *cannot outlive the region* — a clause that
//! provably resumes at most once and does not park its continuation — which is
//! a linearity analysis this milestone does not have. `Cause::TailClause` is
//! recorded separately so that analysis has something to refine.
//! `the_split_over_the_repositorys_own_examples` is where the number comes
//! from and is what a relaxation would move.
//!
//! # What this does not decide
//!
//! Escape. A value outliving its region is a type error under ADR 0017 §2,
//! carried by the brand in its type; this module is about the *cost* of the
//! region and says nothing about what may leave it.
//!
//! # Where the answer is computed, and why it is not keyed by a definition hash
//!
//! A region's kind is a static property of a **program**, so it belongs off the
//! runtime — [`Kinds`] is what carries one program's answer to every engine
//! built from that program, so the analysis runs at most once however many
//! machines a run builds.
//!
//! It is *not* a property of a definition, and it may not be cached under a
//! definition's hash the way a scheme or a footprint is. Two of the inputs
//! below are whole-program and neither is inside the hashed dependency closure
//! of the region's own definition:
//!
//! - [`Analysis::promote_indirect`] asks whether **any** capture is written
//!   **anywhere** in the program. With none, an unknown callee has nothing to
//!   reach and every [`Cause::Indirect`] is dropped. So adding a `handle` to an
//!   unrelated module — one this definition neither names nor transitively
//!   reaches, so one that moves no hash of its own — flips a region holding an
//!   indirect call from `unique` to `shared`.
//! - [`Analysis::new`] decides whether a name denotes a definition or a local
//!   from the program-wide set of definition names.
//!
//! A cache keyed by the region's definition hash would therefore answer
//! `unique` where a fresh inference answers `shared`, which frees an arena a
//! continuation can still reach. `a_capture_in_an_unrelated_module_makes_a_region_shared`
//! in `crates/ply-eval/tests/suite/region_kind_inference.rs` is that program, written
//! down so the key cannot be narrowed by accident. The sound key is the whole
//! `(Program, Resolved)` pair, which is what [`Kinds`] is keyed by:
//! construction, one per loaded program.

use crate::arena::RegionKind;
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Expr, ExprKind, Item, Pattern, PatternKind, Program, QName, Stmt};
use ply_syntax::resolve::{Namespace, Resolved};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, OnceLock};

/// Why a continuation capture is reachable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Cause {
    /// A handler clause with a `resume` binder, which may resume any number of
    /// times.
    Clause { effect: Symbol, op: Symbol },
    /// A tail-resumptive clause, which resumes exactly once — and still
    /// captures, so its continuation can still outlive the region.
    TailClause { effect: Symbol, op: Symbol },
    /// A `perform` no `handle` inside the region answers. The handler is the
    /// caller's, so the capture crosses the region boundary.
    Escapes { effect: Symbol, op: Symbol },
    /// `task.spawn`, `task.join` or `task.yield`: the scheduler parks the
    /// performing task and resumes it later.
    Task { op: Symbol },
    /// `simulate { .. }`, whose scheduler does the same to every task in it.
    Simulate,
    /// A call whose callee is not a statically known definition.
    Indirect,
    /// An argument to a callback builtin that this analysis cannot name.
    Callback { builtin: &'static str },
}

impl Cause {
    /// What the label under the capture site says.
    pub fn describe(&self) -> String {
        match self {
            Cause::Clause { effect, op } => {
                format!("`{effect}.{op}` binds its continuation with `resume`")
            }
            Cause::TailClause { effect, op } => format!(
                "`{effect}.{op}` is tail-resumptive, which still captures: `op(x) -> e` is \
                 `op(x) resume k -> k(e)`"
            ),
            Cause::Escapes { effect, op } => format!(
                "`{effect}.{op}` is answered outside this region, so the capture crosses its \
                 boundary"
            ),
            Cause::Task { op } => {
                format!("`task.{op}` parks this task, and the scheduler resumes it")
            }
            Cause::Simulate => "a simulated region parks and resumes its tasks".to_string(),
            Cause::Indirect => {
                "the callee is not known here, so it may be any function in the program".to_string()
            }
            Cause::Callback { builtin } => {
                format!("`{builtin}` calls a function this analysis cannot name")
            }
        }
    }
}

/// Where a capture becomes reachable from a region, and how it was reached.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CaptureSite {
    pub span: Span,
    pub cause: Cause,
    /// The definitions between the region body and the site, outermost first.
    /// Empty when the site is written inside the region itself.
    pub through: Vec<Symbol>,
}

impl CaptureSite {
    /// `reached through `a` → `b``, or `None` when the site is in the region.
    pub fn chain(&self) -> Option<String> {
        if self.through.is_empty() {
            return None;
        }
        let names: Vec<String> = self.through.iter().map(|n| format!("`{n}`")).collect();
        Some(format!("reached through {}", names.join(" → ")))
    }
}

/// One region and what was decided about it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Region {
    /// The whole region expression, which is what a diagnostic points at.
    pub span: Span,
    /// The brand: `r` of `with_region[r]` or of a `with_cell[r]` that opens its
    /// own region.
    pub brand: Symbol,
    pub kind: RegionKind,
    /// `Some` exactly when `kind` is [`RegionKind::Shared`] by inference. A
    /// region *declared* `shared` over no reachable capture carries `None`.
    pub capture: Option<CaptureSite>,
    /// Whether the kind was declared rather than inferred.
    pub declared: bool,
}

/// Every region in a program, and its kind.
#[derive(Clone, Default, Debug)]
pub struct Regions {
    /// Source order: by module, then by position. Deterministic, because an
    /// artifact that renders these must be byte-identical run to run.
    regions: Vec<Region>,
}

impl Regions {
    /// The kind of the region opened at `span`.
    ///
    /// **`Shared` for a span this analysis never saw.** A caller holding a
    /// region the inference did not produce is a caller whose program moved
    /// under it, and the safe answer to "was a capture reachable" is always yes.
    pub fn kind(&self, span: Span) -> RegionKind {
        self.at(span).map_or(RegionKind::Shared, |r| r.kind)
    }

    pub fn at(&self, span: Span) -> Option<&Region> {
        self.regions.iter().find(|r| r.span == span)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn unique(&self) -> usize {
        self.regions
            .iter()
            .filter(|r| r.kind == RegionKind::Unique)
            .count()
    }

    pub fn shared(&self) -> usize {
        self.len() - self.unique()
    }
}

/// One program's region kinds, computed at most once however many engines are
/// built from that program.
///
/// The analysis is whole-program, so its answer is a property of the
/// `(Program, Resolved)` pair and of nothing smaller. Handing one of these to
/// every engine built from one program is what keeps it off the runtime; a
/// `Machine` that is handed none infers its own, which is correct and is what
/// costs a whole-program traversal per entry point.
///
/// Still filled on first use rather than at construction, because a run that
/// opens no region must not pay for an analysis of every region in the project
/// — `ply test` on a fully cached corpus is that run.
///
/// **The program a `Kinds` is filled from is the program it is an answer
/// about.** Sharing one across two different programs answers about the wrong
/// one, and where the two happen to agree on a span it answers `unique` for a
/// region the other program captures across. Share it only where the
/// `(Program, Resolved)` pair is the same pair.
pub type Kinds = Arc<OnceLock<Regions>>;

/// Infers a kind for every region in the program.
pub fn infer(program: &Program, resolved: &Resolved) -> Regions {
    let (regions, _) = decide(program, resolved, &[]);
    regions
}

/// Infers, then holds each declared region to what it declared.
///
/// A region declared `unique` over a reachable capture is refused, naming the
/// capture site — ADR 0017 §3. A region declared `shared` is always accepted:
/// declaring the conservative kind can only cost a copy.
pub fn check(
    program: &Program,
    resolved: &Resolved,
    declared: &[(Span, RegionKind)],
) -> Result<Regions, Vec<Diagnostic>> {
    let (regions, refusals) = decide(program, resolved, declared);
    if refusals.is_empty() {
        Ok(regions)
    } else {
        Err(refusals)
    }
}

fn decide(
    program: &Program,
    resolved: &Resolved,
    declared: &[(Span, RegionKind)],
) -> (Regions, Vec<Diagnostic>) {
    let mut analysis = Analysis::new(program, resolved);
    analysis.scan_program();
    analysis.promote_indirect();
    analysis.propagate();
    analysis.settle(declared)
}

// ----------------------------------------------------------------- the scan

/// What one body contributes.
#[derive(Clone, Default)]
struct Scan {
    /// The first capture written in the body itself, in source order.
    direct: Option<CaptureSite>,
    /// The first place the body reaches code this analysis cannot name. Held
    /// apart from `direct` because it only *is* a capture if the program has one
    /// to reach; see [`Analysis::promote_indirect`].
    indirect: Option<CaptureSite>,
    /// Definitions named in the body, in source order. A mention is an edge, not
    /// only a call: a definition passed to `map` is called just the same.
    refs: Vec<Symbol>,
}

impl Scan {
    fn direct_at(&mut self, span: Span, cause: Cause) {
        if self.direct.is_none() {
            self.direct = Some(CaptureSite {
                span,
                cause,
                through: Vec::new(),
            });
        }
    }

    fn indirect_at(&mut self, span: Span, cause: Cause) {
        if self.indirect.is_none() {
            self.indirect = Some(CaptureSite {
                span,
                cause,
                through: Vec::new(),
            });
        }
    }

    fn absorb(&mut self, other: Scan) {
        if self.direct.is_none() {
            self.direct = other.direct;
        }
        if self.indirect.is_none() {
            self.indirect = other.indirect;
        }
        self.refs.extend(other.refs);
    }

    /// The site to attribute, preferring the one actually written over the one
    /// that is merely reachable.
    fn site(&self) -> Option<&CaptureSite> {
        self.direct.as_ref().or(self.indirect.as_ref())
    }
}

/// A region found during the scan, before its kind is known.
struct Found {
    span: Span,
    brand: Symbol,
    scan: Scan,
}

/// What a body is being walked under.
#[derive(Clone)]
struct Ctx {
    module: usize,
    /// Operations answered by a `handle` written **inside the region being
    /// walked**. A `perform` of one of them adds nothing, because that `handle`
    /// has already made the region `shared` by installing a clause. One this
    /// list does not carry is answered outside the region, so the capture
    /// crosses its boundary — [`Analysis::walk_region`] clears the list at every
    /// region boundary for that reason.
    handled: Vec<(Symbol, Symbol)>,
    /// Brands of the regions open at this point, so a `with_cell[r]` written
    /// inside `with_region[r]` allocates into that region rather than opening
    /// one of its own.
    brands: Vec<Symbol>,
}

struct Analysis<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    /// Program-wide names of the definitions, so a `Var` can be told from a
    /// constructor or a builtin.
    definitions: BTreeSet<Symbol>,
    /// The local binders in scope at the point of the walk, innermost last.
    ///
    /// `Resolved::scopes[module]` is the **module** scope, so without this a
    /// parameter, a `let` or a pattern binder that shadows a definition's name
    /// resolves to that definition — and a call through it is recorded as an
    /// edge to a body that may reach no capture, rather than as
    /// [`Cause::Indirect`] over a callee that could be any closure the caller
    /// passed. That is the one direction this module may not be wrong in.
    locals: Vec<Symbol>,
    /// The prelude's constructors, which no module declares.
    prelude_ctors: BTreeSet<Symbol>,
    /// Per definition. `BTreeMap` rather than a hash map because the propagation
    /// below iterates it and a run must not depend on a hasher's seed.
    scans: BTreeMap<Symbol, Scan>,
    /// Regions, in source order.
    found: Vec<Found>,
    /// The first capture written anywhere in the program, which is what an
    /// unknown callee may reach.
    anywhere: Option<CaptureSite>,
    /// Definition -> the site reachable from it, and the definitions between.
    reaches: BTreeMap<Symbol, CaptureSite>,
}

impl<'a> Analysis<'a> {
    fn new(program: &'a Program, resolved: &'a Resolved) -> Analysis<'a> {
        let mut definitions = BTreeSet::new();
        for module in &program.modules {
            for item in &module.items {
                if let Item::Fn(f) = item {
                    definitions.insert(module.name.qualify(&f.name.name));
                }
            }
        }
        Analysis {
            program,
            resolved,
            definitions,
            locals: Vec::new(),
            prelude_ctors: ply_core::prelude::ctor_arities()
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
            scans: BTreeMap::new(),
            found: Vec::new(),
            anywhere: None,
            reaches: BTreeMap::new(),
        }
    }

    /// Walks every body a definition, a test or a law holds. Nothing else can
    /// open a region: a `derive` has already been expanded into ordinary
    /// definitions by the time anything here runs.
    fn scan_program(&mut self) {
        for (m, module) in self.program.modules.iter().enumerate() {
            for item in &module.items {
                let (name, body, binders) = match item {
                    Item::Fn(f) => (
                        Some(module.name.qualify(&f.name.name)),
                        &f.body,
                        f.params.iter().map(|p| p.name.name.clone()).collect(),
                    ),
                    Item::Test(t) => (None, &t.body, Vec::new()),
                    Item::Law(l) => (
                        None,
                        &l.body,
                        l.binders.iter().map(|b| b.name.name.clone()).collect(),
                    ),
                    _ => continue,
                };
                let ctx = Ctx {
                    module: m,
                    handled: Vec::new(),
                    brands: Vec::new(),
                };
                let mut scan = Scan::default();
                self.scoped(binders, |a| a.walk(body, &ctx, &mut scan));
                if self.anywhere.is_none() {
                    self.anywhere = scan.direct.clone();
                }
                if let Some(name) = name {
                    self.scans.insert(name, scan);
                }
            }
        }
    }

    /// An unknown callee reaches whatever the program can reach. With no capture
    /// written anywhere there is nothing to reach, so a wholly higher-order
    /// program still infers `unique`.
    fn promote_indirect(&mut self) {
        if self.anywhere.is_none() {
            for scan in self.scans.values_mut() {
                scan.indirect = None;
            }
            for found in &mut self.found {
                found.scan.indirect = None;
            }
        }
    }

    /// Breadth-first over the reverse call graph from every definition with a
    /// site of its own, so each definition gets the shortest chain to one.
    fn propagate(&mut self) {
        let mut callers: BTreeMap<Symbol, BTreeSet<Symbol>> = BTreeMap::new();
        for (name, scan) in &self.scans {
            for callee in &scan.refs {
                callers
                    .entry(callee.clone())
                    .or_default()
                    .insert(name.clone());
            }
        }

        let mut queue: VecDeque<Symbol> = VecDeque::new();
        for (name, scan) in &self.scans {
            if let Some(site) = scan.site() {
                self.reaches.insert(name.clone(), site.clone());
                queue.push_back(name.clone());
            }
        }
        while let Some(callee) = queue.pop_front() {
            let site = self.reaches[&callee].clone();
            let Some(callers) = callers.get(&callee) else {
                continue;
            };
            for caller in callers {
                if self.reaches.contains_key(caller) {
                    continue;
                }
                let mut through = vec![callee.clone()];
                through.extend(site.through.iter().cloned());
                self.reaches.insert(
                    caller.clone(),
                    CaptureSite {
                        span: site.span,
                        cause: site.cause.clone(),
                        through,
                    },
                );
                queue.push_back(caller.clone());
            }
        }
    }

    fn settle(mut self, declared: &[(Span, RegionKind)]) -> (Regions, Vec<Diagnostic>) {
        let mut regions = Vec::with_capacity(self.found.len());
        let mut refusals = Vec::new();
        for found in std::mem::take(&mut self.found) {
            let capture = self.capture_of(&found.scan);
            let inferred = match capture {
                Some(_) => RegionKind::Shared,
                None => RegionKind::Unique,
            };
            let forced = declared
                .iter()
                .find(|(span, _)| *span == found.span)
                .map(|(_, kind)| *kind);
            let kind = forced.unwrap_or(inferred);
            if forced == Some(RegionKind::Unique)
                && let Some(site) = &capture
            {
                refusals.push(refuse_unique(&found, site));
            }
            regions.push(Region {
                span: found.span,
                brand: found.brand,
                kind,
                capture,
                declared: forced.is_some(),
            });
        }
        (Regions { regions }, refusals)
    }

    /// The capture a region reaches: one written inside it, or the first its
    /// body's calls lead to.
    fn capture_of(&self, scan: &Scan) -> Option<CaptureSite> {
        if let Some(site) = scan.site() {
            return Some(site.clone());
        }
        for callee in &scan.refs {
            if let Some(site) = self.reaches.get(callee) {
                let mut through = vec![callee.clone()];
                through.extend(site.through.iter().cloned());
                return Some(CaptureSite {
                    span: site.span,
                    cause: site.cause.clone(),
                    through,
                });
            }
        }
        None
    }

    fn walk(&mut self, e: &Expr, ctx: &Ctx, out: &mut Scan) {
        crate::limit::grow(|| self.walk_at(e, ctx, out));
    }

    /// Runs `f` with `names` bound as locals, and unbinds them however it
    /// returns. Shadowing is by position — the innermost binding of a name is
    /// the last one pushed — so a `Vec` and a truncation are the whole scope
    /// discipline.
    fn scoped(&mut self, names: Vec<Symbol>, f: impl FnOnce(&mut Self)) {
        let depth = self.locals.len();
        self.locals.extend(names);
        f(self);
        self.locals.truncate(depth);
    }

    fn walk_at(&mut self, e: &Expr, ctx: &Ctx, out: &mut Scan) {
        match &e.kind {
            ExprKind::Var(q) => {
                if let Some(name) = self.definition(ctx.module, q) {
                    out.refs.push(name);
                }
            }
            ExprKind::App { func, args, .. } => {
                self.walk_call(e.span, func, args, ctx, out);
            }
            ExprKind::Lambda { params, body } => {
                let bound = params.iter().map(|p| p.name.name.clone()).collect();
                self.scoped(bound, |a| a.walk(body, ctx, out));
            }
            ExprKind::Block { stmts, tail } => {
                let depth = self.locals.len();
                // A `let`'s binders are in scope for the statements after it and
                // for the tail, and not for its own right-hand side.
                for stmt in stmts {
                    match stmt {
                        Stmt::Let { pat, value, .. } => {
                            self.walk(value, ctx, out);
                            pattern_binders(pat, &mut self.locals);
                        }
                        Stmt::Expr(e) => self.walk(e, ctx, out),
                    }
                }
                if let Some(tail) = tail {
                    self.walk(tail, ctx, out);
                }
                self.locals.truncate(depth);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk(scrutinee, ctx, out);
                for arm in arms {
                    let mut bound = Vec::new();
                    pattern_binders(&arm.pat, &mut bound);
                    self.scoped(bound, |a| {
                        if let Some(guard) = &arm.guard {
                            a.walk(guard, ctx, out);
                        }
                        a.walk(&arm.body, ctx, out);
                    });
                }
            }
            ExprKind::Perform {
                effect, op, args, ..
            } => {
                for arg in args {
                    self.walk(arg, ctx, out);
                }
                self.walk_perform(e.span, effect, &op.name, ctx, out);
            }
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                let mut inner = ctx.clone();
                for clause in clauses {
                    let effect = self.effect_name(ctx.module, &clause.effect);
                    let op = clause.op.name.clone();
                    // Both forms capture. The tail-resumptive one is recorded
                    // under its own cause rather than left out, because a
                    // refinement that decided one resumption is safe would
                    // still have to see it here.
                    let cause = match clause.resume {
                        Some(_) => Cause::Clause {
                            effect: effect.clone(),
                            op: op.clone(),
                        },
                        None => Cause::TailClause {
                            effect: effect.clone(),
                            op: op.clone(),
                        },
                    };
                    out.direct_at(clause.span, cause);
                    inner.handled.push((effect, op));
                }
                self.walk(body, &inner, out);
                // A clause body and a return clause run *below* their own
                // handler, so they are walked under the enclosing context.
                for clause in clauses {
                    let mut bound: Vec<Symbol> =
                        clause.params.iter().map(|p| p.name.clone()).collect();
                    bound.extend(clause.resume.iter().map(|r| r.name.clone()));
                    self.scoped(bound, |a| a.walk(&clause.body, ctx, out));
                }
                if let Some(ret) = return_clause {
                    let bound = vec![ret.binder.name.clone()];
                    self.scoped(bound, |a| a.walk(&ret.body, ctx, out));
                }
            }
            ExprKind::Simulate { body } => {
                out.direct_at(e.span, Cause::Simulate);
                self.walk(body, ctx, out);
            }
            ExprKind::WithRegion { region, body } => {
                self.walk_region(e.span, region.name.clone(), body, ctx, out);
            }
            ExprKind::WithCell {
                resource,
                init,
                binder,
                body,
            } => {
                self.walk(init, ctx, out);
                let bound = vec![binder.name.clone()];
                // A cell written inside the region of the same brand is a value
                // allocated in *that* region — ADR 0017 §1 — so it opens nothing.
                if ctx.brands.contains(&resource.name) {
                    self.scoped(bound, |a| a.walk(body, ctx, out));
                } else {
                    let span = e.span;
                    let brand = resource.name.clone();
                    self.scoped(bound, |a| a.walk_region(span, brand, body, ctx, out));
                }
            }
            _ => children(e, &mut |child| self.walk(child, ctx, out)),
        }
    }

    fn walk_region(&mut self, span: Span, brand: Symbol, body: &Expr, ctx: &Ctx, out: &mut Scan) {
        let mut inner = ctx.clone();
        inner.brands.push(brand.clone());
        // A `handle` that encloses the region answers *across* the region's
        // boundary, which is exactly what `Cause::Escapes` means, so its
        // operations are not local to this region and must not be inherited.
        // Only a `handle` written inside the body makes one of its operations
        // local, and the `Handle` arm files those as it walks.
        inner.handled.clear();
        let mut scan = Scan::default();
        self.walk(body, &inner, &mut scan);
        // Post-order: an inner region is recorded before the region enclosing
        // it. Deterministic either way, and every consumer looks a region up by
        // its own span.
        self.found.push(Found {
            span,
            brand,
            scan: scan.clone(),
        });
        out.absorb(scan);
    }

    fn walk_call(&mut self, span: Span, func: &Expr, args: &[Expr], ctx: &Ctx, out: &mut Scan) {
        for arg in args {
            self.walk(arg, ctx, out);
        }
        match &func.kind {
            ExprKind::Var(q) if !self.is_local(q) => {
                if let Some(name) = self.definition(ctx.module, q) {
                    out.refs.push(name);
                    return;
                }
                if self.is_constructor(ctx.module, q) {
                    return;
                }
                if q.is_bare()
                    && let Some(builtin) = crate::builtins::Builtin::from_name(q.symbol())
                {
                    if builtin.higher_order() {
                        self.walk_callback(span, builtin.name(), args, ctx, out);
                    }
                    return;
                }
                out.indirect_at(span, Cause::Indirect);
            }
            // A local binding, so the callee is whatever it holds — including
            // when it shadows the name of a definition, a constructor or a
            // builtin, which is the case the module scope alone gets wrong.
            ExprKind::Var(_) => out.indirect_at(span, Cause::Indirect),
            // Written at the call site: the callee is known, and its body has
            // already been walked as an argument-free subexpression.
            ExprKind::Lambda { params, body } => {
                let bound = params.iter().map(|p| p.name.name.clone()).collect();
                self.scoped(bound, |a| a.walk(body, ctx, out));
            }
            // A field, the result of another call: nothing here names what will
            // run.
            _ => {
                self.walk(func, ctx, out);
                out.indirect_at(span, Cause::Indirect);
            }
        }
    }

    /// `map`, `filter`, `fold`, `iterate`, `map_fold` and `bytes_position` call
    /// a function this analysis has to be able to name.
    ///
    /// All six take it **last** — `map(xs, f)`, `fold(xs, init, f)`,
    /// `iterate(seed, budget, f)` — and all six have a fixed arity, so the
    /// callback is `args.last()` and the rest of the arguments are data.
    /// [`the_callback_builtins_are_the_six_this_module_knows`] is what stops a
    /// seventh from being added under this assumption, and it is why `iterate`
    /// takes its budget second: a callback in the middle would be read as data
    /// and the budget read as the callback, silently.
    ///
    /// [`the_callback_builtins_are_the_six_this_module_knows`]: tests::the_callback_builtins_are_the_six_this_module_knows
    fn walk_callback(
        &mut self,
        span: Span,
        builtin: &'static str,
        args: &[Expr],
        ctx: &Ctx,
        out: &mut Scan,
    ) {
        let nameable = match args.last().map(|arg| &arg.kind) {
            // Written at the call site, or a definition or constructor named
            // there: the `Var` arm above has already recorded the edge.
            Some(ExprKind::Lambda { .. }) => true,
            Some(ExprKind::Var(q)) => {
                !self.is_local(q)
                    && (self.definition(ctx.module, q).is_some()
                        || self.is_constructor(ctx.module, q)
                        || (q.is_bare()
                            && crate::builtins::Builtin::from_name(q.symbol()).is_some()))
            }
            _ => false,
        };
        if !nameable {
            out.indirect_at(span, Cause::Callback { builtin });
        }
    }

    fn walk_perform(&mut self, span: Span, effect: &QName, op: &Symbol, ctx: &Ctx, out: &mut Scan) {
        let declared = self.global(ctx.module, Namespace::Effect, effect);
        // The builtin `task`, which no module declares and which the scheduler
        // answers by parking the performing task.
        if declared.is_none() && effect.is_bare() && effect.symbol().as_str() == "task" {
            out.direct_at(span, Cause::Task { op: op.clone() });
            return;
        }
        let name = declared.unwrap_or_else(|| effect.symbol().clone());
        // `ctx.handled` carries only the handlers written inside this region, so
        // an operation it names is already the reason the region is `shared` and
        // adds nothing. Anything else is answered outside the region, and *that*
        // capture crosses the region's boundary.
        if ctx.handled.contains(&(name.clone(), op.clone())) {
            return;
        }
        out.direct_at(
            span,
            Cause::Escapes {
                effect: name,
                op: op.clone(),
            },
        );
    }

    /// Whether a bare name is bound by something inside the body being walked.
    /// A qualified name cannot be: only a module path reaches one.
    fn is_local(&self, q: &QName) -> bool {
        q.is_bare() && self.locals.contains(q.symbol())
    }

    /// The program-wide name of the definition a reference denotes, or `None`
    /// when it denotes anything else — a local, a constructor, a builtin.
    fn definition(&self, module: usize, q: &QName) -> Option<Symbol> {
        if self.is_local(q) {
            return None;
        }
        let name = self.global(module, Namespace::Value, q)?;
        self.definitions.contains(&name).then_some(name)
    }

    fn is_constructor(&self, module: usize, q: &QName) -> bool {
        if self.is_local(q) {
            return false;
        }
        match self.global(module, Namespace::Value, q) {
            Some(name) => !self.definitions.contains(&name),
            None => q.is_bare() && self.prelude_ctors.contains(q.symbol()),
        }
    }

    fn effect_name(&self, module: usize, effect: &QName) -> Symbol {
        self.global(module, Namespace::Effect, effect)
            .unwrap_or_else(|| effect.symbol().clone())
    }

    fn global(&self, module: usize, ns: Namespace, q: &QName) -> Option<Symbol> {
        if q.is_bare() {
            return self
                .resolved
                .scopes
                .get(module)
                .and_then(|scope| scope.get(ns, q.symbol()))
                .map(|b| b.qualified.clone());
        }
        self.resolved
            .lookup(module, ns, q)
            .ok()
            .map(|b| b.qualified.clone())
    }
}

/// ADR 0017 §3: forcing `unique` where a capture is reachable is a compile
/// error naming the capture site.
fn refuse_unique(found: &Found, site: &CaptureSite) -> Diagnostic {
    let mut d = Diagnostic::error(
        codes::REGION_KIND_REFUSED,
        format!(
            "region `{}` is declared `unique`, but a continuation is captured across it",
            found.brand
        ),
    )
    .primary(
        found.span,
        format!("`{}` is declared `unique` here", found.brand),
    )
    .secondary(site.span, site.cause.describe());
    if let Some(chain) = site.chain() {
        d = d.note(chain);
    }
    d.note(
        "a `unique` region is a bump pointer with no snapshot, so one resumption would observe \
         another resumption's writes",
    )
    .note(
        "remove the annotation and the region is inferred `shared`, which snapshots at the capture",
    )
}

/// Every name a pattern can bind.
///
/// A bare `Var` pattern naming a nullary constructor binds nothing, and telling
/// the two apart needs resolution this walk does not do per pattern. The
/// over-approximation is safe in the direction that matters: reading a
/// constructor as a local turns a call through it into [`Cause::Indirect`] and a
/// callback argument into [`Cause::Callback`], both of which land on `shared`.
fn pattern_binders(p: &Pattern, out: &mut Vec<Symbol>) {
    crate::limit::grow(|| match &p.kind {
        PatternKind::Wildcard | PatternKind::Lit(_) => {}
        PatternKind::Var(id) => out.push(id.name.clone()),
        PatternKind::Ctor { args, .. } => {
            for arg in args {
                pattern_binders(arg, out);
            }
        }
        PatternKind::Record { fields, .. } => {
            for (_, pat) in fields {
                pattern_binders(pat, out);
            }
        }
        PatternKind::List { items, rest } => {
            for item in items {
                pattern_binders(item, out);
            }
            if let Some(rest) = rest {
                pattern_binders(rest, out);
            }
        }
    });
}

/// Every subexpression, in source order, for the forms that bind nothing and
/// open nothing. Every form that binds a name or opens a region has its own arm
/// in [`Analysis::walk_at`] and does not reach here — a scope has to be pushed
/// around the child rather than beside it. The match stays exhaustive so that
/// adding a form to the AST is a compile error in both places.
fn children(e: &Expr, f: &mut impl FnMut(&Expr)) {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Unary { operand, .. } => f(operand),
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::App { func, args, .. } => {
            f(func);
            for arg in args {
                f(arg);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    f(guard);
                }
                f(&arm.body);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for stmt in stmts {
                match stmt {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(e) => f(e),
                }
            }
            if let Some(tail) = tail {
                f(tail);
            }
        }
        ExprKind::Record { fields } => {
            for (_, value) in fields {
                f(value);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            f(base);
            for (_, value) in fields {
                f(value);
            }
        }
        ExprKind::Field { base, .. } => f(base),
        ExprKind::Try { operand } => f(operand),
        ExprKind::List { items } => {
            for item in items {
                f(item);
            }
        }
        ExprKind::Perform { args, .. } => {
            for arg in args {
                f(arg);
            }
        }
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            f(body);
            for clause in clauses {
                f(&clause.body);
            }
            if let Some(ret) = return_clause {
                f(&ret.body);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            f(init);
            f(body);
        }
        ExprKind::WithRegion { body, .. } | ExprKind::Simulate { body } => f(body),
    }
}

#[cfg(test)]
mod tests {
    use crate::builtins::Builtin;

    /// [`Analysis::walk_callback`] reads the callback out of the **last**
    /// argument, which is true of all six and is not a rule the type system
    /// enforces. A seventh callback builtin has to be checked against that
    /// assumption before it is added, and this is where it is stopped.
    #[test]
    fn the_callback_builtins_are_the_six_this_module_knows() {
        let mut names: Vec<&str> = Builtin::all()
            .iter()
            .filter(|b| b.higher_order())
            .map(|b| b.name())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "bytes_position",
                "filter",
                "fold",
                "iterate",
                "map",
                "map_fold"
            ],
            "a callback builtin was added or removed; `walk_callback` reads the function out of \
             the last argument and has to be checked against the new one"
        );
        for b in Builtin::all().iter().filter(|b| b.higher_order()) {
            let (min, max) = b.arity();
            assert_eq!(
                min,
                max,
                "`{}` has a variable arity, so its last argument is not always its callback",
                b.name()
            );
        }
    }

    /// A run must be a function of its definitions and nothing else, so nothing
    /// here may reach a hash-ordered collection or a clock. The same rule
    /// `sim`, `sched`, `explore` and `region` are held to.
    #[test]
    fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
        let source = include_str!("region_kind.rs");
        let body = source
            .split_once("mod tests {")
            .map(|(body, _)| body)
            .unwrap_or(source);
        for banned in [
            "HashMap",
            "HashSet",
            "FxHashMap",
            "FxHashSet",
            "SystemTime",
            "Instant",
            "thread::",
            "rayon",
        ] {
            assert!(
                !body.contains(banned),
                "`{banned}` appears in ply_eval::region_kind; an inferred region kind is part of \
                 what a program means and may not depend on a hasher's seed"
            );
        }
    }
}
