//! Whether an append copies, decided before the program runs.
//!
//! Since ADR 0034 the machine moves a binding's value out of its slot at its last use, so the
//! positional rule this checker once transcribed no longer exists: nothing is decided by where an
//! expression sits in its enclosing call or literal. What is left — and what this reports — is the
//! *residue*: the copies the semantics require, because something else genuinely owns the value
//! when the append runs. A cell's arena, a map, a closure's capture, a caller that keeps reading
//! what it passed, a binding read again later.

use crate::builtins::Builtin;
use crate::code::{self, Code, NodeKind, Stmt};
use crate::rc::Own;
use ply_span::{Span, Symbol};
use ply_syntax::ast::{Item, Program, QName};
use ply_syntax::resolve::{Namespace, Resolved};
use rustc_hash::{FxHashMap, FxHashSet};

/// What the checker says one `push` site will cost.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Reuses,
    Copies,
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Reuses => "reuses",
            Verdict::Copies => "COPIES",
            Verdict::Unknown => "unknown",
        }
    }
}

/// Why a site is not `Reuses`, in the form a diagnostic can dispatch on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Cause {
    /// The binding is read again after this use, so this use clones the value.
    ReadAgain,
    /// The list came out of a cell, whose region arena holds it for the whole of the append.
    Cell,
    /// The list came out of a map, which still holds it.
    MapEntry,
    /// A closure captured the binding, and holds its own copy of the value.
    Capture,
    /// The list is a parameter and a caller keeps what it passes.
    CallerKeeps,
    /// The list is an element of a list being walked, which still holds it.
    Element,
    /// A top-level definition named outside callee position, so the program holds it.
    Program,
    /// The value a call answered, which the callee may also hold.
    Call,
    /// The value a handler, a `handle` or a `simulate` answered.
    Handler,
    /// A free variable of a closure, where whether the closure's copy is still held when the
    /// append runs is not a property of this body.
    Closure,
}

impl Cause {
    /// The source edit that removes the copy, or `None` where no edit does.
    pub fn fix(self) -> Option<&'static str> {
        match self {
            Cause::ReadAgain => Some(
                "make the append the binding's last use: bind whatever you still need out of it \
                 first",
            ),
            Cause::Cell => Some("`cell_update`"),
            Cause::MapEntry => Some("`map_update`"),
            Cause::Capture => {
                Some("bind the list before the closure captures it, or capture less")
            }
            Cause::CallerKeeps => Some("the edit is at the call site: the caller keeps the list"),
            Cause::Element | Cause::Program | Cause::Call | Cause::Handler | Cause::Closure => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Cause::ReadAgain => "read-again",
            Cause::Cell => "cell",
            Cause::MapEntry => "map",
            Cause::Capture => "capture",
            Cause::CallerKeeps => "caller",
            Cause::Element => "element",
            Cause::Program => "program",
            Cause::Call => "call",
            Cause::Handler => "handler",
            Cause::Closure => "closure",
        }
    }
}

/// A cause and the sentence a diagnostic would print for it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Why {
    cause: Cause,
    text: String,
}

impl Why {
    fn new(cause: Cause, text: impl Into<String>) -> Why {
        Why {
            cause,
            text: text.into(),
        }
    }

    /// The same cause with a sentence that says where it was met.
    fn reworded(&self, text: String) -> Why {
        Why {
            cause: self.cause,
            text,
        }
    }
}

/// One `push` in one definition.
#[derive(Clone, Debug)]
pub struct Site {
    pub span: Span,
    pub verdict: Verdict,
    /// Why, in the words a diagnostic would use.
    pub reason: String,
    /// `None` at a [`Verdict::Reuses`] site, which has no cause to name.
    pub cause: Option<Cause>,
    /// Whether the lowering marked this append's **list argument** [`Own::Owned`] — the machine's
    /// own move decision, recorded so the checker can be judged against it.
    pub own_marked: bool,
}

impl Site {
    /// The source edit that would remove this copy, if one would.
    pub fn fix(&self) -> Option<&'static str> {
        self.cause.and_then(Cause::fix)
    }
}

/// What kind of item a [`Definition`] came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefKind {
    Fn,
    Test,
    Law,
}

/// Every `push` site in one definition.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub span: Span,
    pub module: usize,
    pub kind: DefKind,
    pub sites: Vec<Site>,
}

impl Definition {
    pub fn copies(&self) -> usize {
        self.count(Verdict::Copies)
    }

    pub fn unknown(&self) -> usize {
        self.count(Verdict::Unknown)
    }

    pub fn reuses(&self) -> usize {
        self.count(Verdict::Reuses)
    }

    fn count(&self, v: Verdict) -> usize {
        self.sites.iter().filter(|s| s.verdict == v).count()
    }
}

/// What the checker found, whole program.
#[derive(Clone, Debug)]
pub struct Report {
    defs: Vec<Definition>,
    /// How many rounds the fixpoint took.
    pub rounds: usize,
    /// Definitions whose parameters the fixpoint could not keep sole-owned, and what spoiled each
    /// one.
    pub spoiled: Vec<(String, usize, String)>,
}

impl Report {
    pub fn all(&self) -> &[Definition] {
        &self.defs
    }

    /// One module's definitions, in source order.
    pub fn module(&self, index: usize) -> Vec<&Definition> {
        self.defs.iter().filter(|d| d.module == index).collect()
    }
}

/// Who owns a value besides the place it is standing.
#[derive(Clone, Debug)]
enum Owner {
    /// Nothing else can reach it.
    Fresh,
    /// This definition's `k`th parameter, which is sole-owned exactly when every caller hands one
    /// over and stops reading it.
    Param(usize),
    /// Provably held by something else when it is used.
    Blocked(Why),
    /// Not decidable from this body.
    Unknown(Why),
}

/// [`Owner`], settled for a site or a callee demand.
#[derive(Clone, Debug, Default)]
enum Res {
    #[default]
    Unique,
    Param(usize),
    Blocked(Why),
    Unknown(Why),
}

fn to_res(owner: Owner) -> Res {
    match owner {
        Owner::Fresh => Res::Unique,
        Owner::Param(k) => Res::Param(k),
        Owner::Blocked(why) => Res::Blocked(why),
        Owner::Unknown(why) => Res::Unknown(why),
    }
}

/// One binding in the scope the machine would have built.
#[derive(Clone, Debug)]
struct Binding {
    name: Symbol,
    /// What the value bound here is worth *besides* this binding itself.
    owner: Owner,
}

struct State {
    chain: Vec<Binding>,
    /// The module the body being walked belongs to, for name resolution.
    module: usize,
}

impl State {
    /// The innermost binding of `name`, which is the one a lookup would find.
    fn index_of(&self, name: &Symbol) -> Option<usize> {
        self.chain.iter().rposition(|b| &b.name == name)
    }
}

/// The checker over one program.
pub struct Costs<'a> {
    program: &'a Program,
    resolved: &'a Resolved,
    /// Program-wide name of every top-level `fn`, and its arity.
    defs: FxHashMap<Symbol, usize>,
}

/// What a whole-program pass concluded about one parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ParamState {
    Sole,
    Unsure(Why),
    Shared(Why),
}

impl ParamState {
    fn rank(&self) -> u8 {
        match self {
            ParamState::Sole => 2,
            ParamState::Unsure(_) => 1,
            ParamState::Shared(_) => 0,
        }
    }

    fn meet(self, other: ParamState) -> ParamState {
        if other.rank() < self.rank() {
            other
        } else {
            self
        }
    }
}

/// What a definition's body answers with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RetState {
    Fresh,
    /// The body answers its `k`th parameter, so the call answers whatever the argument at that
    /// position was worth.
    Param(usize),
    Shared(Why),
    Unsure(Why),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Tables {
    params: FxHashMap<Symbol, Vec<ParamState>>,
    rets: FxHashMap<Symbol, RetState>,
}

/// One definition, lowered once and walked once per round.
struct Body<'p> {
    label: String,
    qname: Option<Symbol>,
    module: usize,
    kind: DefKind,
    span: Span,
    params: Vec<Symbol>,
    code: Code,
    _marker: std::marker::PhantomData<&'p ()>,
}

/// What one round of walking found.
#[derive(Default)]
struct Found {
    sites: Vec<(Span, Res, bool)>,
    /// `(callee, parameter, what this site hands it)`.
    demands: Vec<(Symbol, usize, Res)>,
    escaped: FxHashSet<Symbol>,
    ret: Res,
}

/// How many rounds the fixpoint is allowed.
const MAX_ROUNDS: usize = 24;

impl<'a> Costs<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved) -> Costs<'a> {
        let mut defs = FxHashMap::default();
        for (index, module) in program.modules.iter().enumerate() {
            for item in &module.items {
                if let Item::Fn(def) = item
                    && let Some(name) = qualified(resolved, index, &def.name.name)
                {
                    defs.insert(name, def.params.len());
                }
            }
        }
        Costs {
            program,
            resolved,
            defs,
        }
    }

    /// Walk every definition to a fixpoint, then read the verdicts off it.
    pub fn check(&self) -> Report {
        let bodies = self.bodies();
        let mut tables = Tables {
            params: self
                .defs
                .iter()
                .map(|(name, arity)| (name.clone(), vec![ParamState::Sole; *arity]))
                .collect(),
            rets: self
                .defs
                .keys()
                .map(|name| (name.clone(), RetState::Fresh))
                .collect(),
        };
        let mut rounds = 0;
        let mut found;
        loop {
            found = self.round(&bodies, &tables);
            let next = self.settle(&bodies, &found, &tables);
            rounds += 1;
            if next == tables || rounds >= MAX_ROUNDS {
                tables = next;
                break;
            }
            tables = next;
        }
        // One more walk under the settled tables, so every verdict is read off the answer rather
        // than off the round that produced it.
        let found = self.round(&bodies, &tables);

        let mut spoiled: Vec<(String, usize, String)> = Vec::new();
        for (name, slots) in &tables.params {
            for (j, slot) in slots.iter().enumerate() {
                match slot {
                    ParamState::Sole => {}
                    ParamState::Unsure(why) | ParamState::Shared(why) => {
                        spoiled.push((name.as_str().to_string(), j, why.text.clone()))
                    }
                }
            }
        }
        spoiled.sort();

        let defs = bodies
            .iter()
            .zip(found.iter())
            .map(|(body, found)| Definition {
                sites: found
                    .sites
                    .iter()
                    .map(|(span, res, own)| finish(*span, res, *own, body.qname.as_ref(), &tables))
                    .collect(),
                name: body.label.clone(),
                span: body.span,
                module: body.module,
                kind: body.kind,
            })
            .collect();
        Report {
            defs,
            spoiled,
            rounds,
        }
    }

    /// Every definition, test and law, lowered once.
    fn bodies(&self) -> Vec<Body<'a>> {
        let mut out = Vec::new();
        for (index, module) in self.program.modules.iter().enumerate() {
            for item in &module.items {
                let (label, qname, kind, params, body, span) = match item {
                    Item::Fn(def) => (
                        format!("{}.{}", module.name.as_str(), def.name.name),
                        qualified(self.resolved, index, &def.name.name),
                        DefKind::Fn,
                        def.params
                            .iter()
                            .map(|p| p.name.name.clone())
                            .collect::<Vec<_>>(),
                        &def.body,
                        def.span,
                    ),
                    Item::Test(def) => (
                        format!("{}: test {:?}", module.name.as_str(), def.name),
                        None,
                        DefKind::Test,
                        Vec::new(),
                        &def.body,
                        def.span,
                    ),
                    Item::Law(def) => (
                        format!("{}: law {:?}", module.name.as_str(), def.name),
                        None,
                        DefKind::Law,
                        def.binders.iter().map(|b| b.name.name.clone()).collect(),
                        &def.body,
                        def.span,
                    ),
                    _ => continue,
                };
                let code = code::lower_fn(&params, body).code;
                out.push(Body {
                    label,
                    qname,
                    module: index,
                    kind,
                    span,
                    params,
                    code,
                    _marker: std::marker::PhantomData,
                });
            }
        }
        out
    }

    fn round(&self, bodies: &[Body<'_>], tables: &Tables) -> Vec<Found> {
        bodies
            .iter()
            .map(|body| {
                let mut walk = Walk {
                    costs: self,
                    tables,
                    found: Found::default(),
                };
                let chain = body
                    .params
                    .iter()
                    .enumerate()
                    .map(|(k, name)| Binding {
                        name: name.clone(),
                        owner: match body.qname {
                            // A test or a law has no caller: its binders are whatever the harness
                            // built.
                            None => Owner::Fresh,
                            Some(_) => Owner::Param(k),
                        },
                    })
                    .collect();
                let mut st = State {
                    chain,
                    module: body.module,
                };
                let ret = walk.walk(&body.code, &mut st);
                walk.found.ret = to_res(ret);
                walk.found
            })
            .collect()
    }

    /// One step of the fixpoint: what the round just walked says the tables should be.
    fn settle(&self, bodies: &[Body<'_>], found: &[Found], tables: &Tables) -> Tables {
        let mut params: FxHashMap<Symbol, Vec<ParamState>> = self
            .defs
            .iter()
            .map(|(name, arity)| (name.clone(), vec![ParamState::Sole; *arity]))
            .collect();
        let mut rets: FxHashMap<Symbol, RetState> = FxHashMap::default();

        for (body, found) in bodies.iter().zip(found.iter()) {
            let caller = body.qname.clone();
            for name in &found.escaped {
                if let Some(slots) = params.get_mut(name) {
                    for slot in slots.iter_mut() {
                        *slot = std::mem::replace(slot, ParamState::Sole).meet(ParamState::Unsure(
                            Why::new(
                                Cause::Program,
                                format!(
                                    "`{}` is used as a value in {}, so not every caller of it is known",
                                    name.as_str(),
                                    body.label
                                ),
                            ),
                        ));
                    }
                }
            }
            for (callee, j, res) in &found.demands {
                let state = interpret(res, caller.as_ref(), tables, &body.label);
                if let Some(slots) = params.get_mut(callee)
                    && let Some(slot) = slots.get_mut(*j)
                {
                    *slot = std::mem::replace(slot, ParamState::Sole).meet(state);
                }
            }
            if let Some(name) = &body.qname {
                rets.insert(
                    name.clone(),
                    match &found.ret {
                        Res::Unique => RetState::Fresh,
                        Res::Param(k) => RetState::Param(*k),
                        Res::Blocked(why) => RetState::Shared(why.clone()),
                        Res::Unknown(why) => RetState::Unsure(why.clone()),
                    },
                );
            }
        }
        Tables { params, rets }
    }
}

/// What one call site's argument says about the callee's parameter.
fn interpret(res: &Res, caller: Option<&Symbol>, tables: &Tables, at: &str) -> ParamState {
    match res {
        Res::Unique => ParamState::Sole,
        Res::Blocked(why) => {
            ParamState::Shared(why.reworded(format!("{at} passes a value where {}", why.text)))
        }
        Res::Unknown(why) => {
            ParamState::Unsure(why.reworded(format!("{at} passes a value where {}", why.text)))
        }
        Res::Param(k) => match caller
            .and_then(|c| tables.params.get(c))
            .and_then(|slots| slots.get(*k))
        {
            Some(ParamState::Sole) => ParamState::Sole,
            Some(ParamState::Unsure(why)) => ParamState::Unsure(why.reworded(format!(
                "{at} passes its own parameter {k}, which is unsettled"
            ))),
            Some(ParamState::Shared(why)) => ParamState::Shared(why.reworded(format!(
                "{at} passes its own parameter {k}, whose callers keep what they pass"
            ))),
            None => ParamState::Unsure(Why::new(
                Cause::CallerKeeps,
                format!("{at} passes a parameter this pass cannot name"),
            )),
        },
    }
}

fn finish(
    span: Span,
    res: &Res,
    own_marked: bool,
    owner: Option<&Symbol>,
    tables: &Tables,
) -> Site {
    let (verdict, reason, cause) = match res {
        Res::Unique => (
            Verdict::Reuses,
            "the list reaches `push` at one owner".to_string(),
            None,
        ),
        Res::Blocked(why) => (Verdict::Copies, why.text.clone(), Some(why.cause)),
        Res::Unknown(why) => (Verdict::Unknown, why.text.clone(), Some(why.cause)),
        Res::Param(k) => {
            let state = owner
                .and_then(|name| tables.params.get(name))
                .and_then(|slots| slots.get(*k));
            match state {
                Some(ParamState::Sole) => (
                    Verdict::Reuses,
                    format!(
                        "the list is parameter {k}, and every call site hands one over and \
                         stops reading it"
                    ),
                    None,
                ),
                Some(ParamState::Shared(why)) => (
                    Verdict::Copies,
                    format!(
                        "the list is parameter {k}, and a caller keeps it: {}",
                        why.text
                    ),
                    Some(Cause::CallerKeeps),
                ),
                Some(ParamState::Unsure(why)) => (
                    Verdict::Unknown,
                    format!(
                        "the list is parameter {k}, and a call site is undecided: {}",
                        why.text
                    ),
                    Some(why.cause),
                ),
                None => (
                    Verdict::Unknown,
                    format!("the list is parameter {k} of a definition this pass cannot name"),
                    Some(Cause::CallerKeeps),
                ),
            }
        }
    };
    Site {
        span,
        verdict,
        reason,
        cause,
        own_marked,
    }
}

/// One definition's traversal, under the tables the last round settled.
struct Walk<'c, 'a> {
    costs: &'c Costs<'a>,
    tables: &'c Tables,
    found: Found,
}

impl Walk<'_, '_> {
    fn walk(&mut self, code: &Code, st: &mut State) -> Owner {
        crate::limit::grow(|| self.walk_node(code, st))
    }

    fn walk_node(&mut self, code: &Code, st: &mut State) -> Owner {
        match &code.kind {
            NodeKind::Lit(..) => Owner::Fresh,

            NodeKind::Var { name, .. } => self.var_owner(name, code.own, st),

            NodeKind::Unary { operand, .. } => {
                self.walk(operand, st);
                Owner::Fresh
            }

            NodeKind::Binary { lhs, rhs, .. } => {
                self.walk(lhs, st);
                self.walk(rhs, st);
                Owner::Fresh
            }

            NodeKind::Lambda {
                params,
                body,
                captures,
                ..
            } => {
                self.barrier(params, body, st, &[]);
                self.poison_captures(captures, st);
                Owner::Fresh
            }

            NodeKind::App { func, args } => {
                let builtin = self.builtin_of(func, st);
                let callee = self.callee_of(func, st);
                // A callee written as a name is not the definition *escaping*: it is the call.
                if !matches!(func.kind, NodeKind::Var { .. }) {
                    self.walk(func, st);
                }
                let mut resolved: Vec<Res> = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    let owner = match self.callback_owners(builtin, i, args.len(), &resolved) {
                        Some(owners) => self.walk_callback(&owners, arg, st),
                        None => self.walk(arg, st),
                    };
                    resolved.push(to_res(owner));
                }
                if builtin == Some(Builtin::Push) && args.len() == 2 {
                    self.found.sites.push((
                        code.span,
                        resolved[0].clone(),
                        args[0].own == Own::Owned,
                    ));
                }
                if let Some(name) = &callee {
                    for (j, res) in resolved.iter().enumerate() {
                        if !matches!(res, Res::Unique) {
                            self.found.demands.push((name.clone(), j, res.clone()));
                        }
                    }
                }
                self.result_owner(builtin, callee.as_ref(), &resolved)
            }

            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.walk(cond, st);
                let a = self.walk(then_branch, st);
                let b = self.walk(else_branch, st);
                join(a, b)
            }

            NodeKind::Match { scrutinee, arms } => {
                let scrutinee_owner = self.walk(scrutinee, st);
                let mut joined: Option<Owner> = None;
                for arm in arms.iter() {
                    let depth = st.chain.len();
                    let mut bound = Vec::new();
                    arm.pat.binders(&mut bound);
                    for name in bound {
                        st.chain.push(Binding {
                            name,
                            owner: scrutinee_owner.clone(),
                        });
                    }
                    if let Some(guard) = &arm.guard {
                        self.walk(guard, st);
                    }
                    let owner = self.walk(&arm.body, st);
                    st.chain.truncate(depth);
                    joined = Some(match joined {
                        None => owner,
                        Some(prev) => join(prev, owner),
                    });
                }
                joined.unwrap_or(Owner::Fresh)
            }

            NodeKind::Block { stmts, tail } => {
                let depth = st.chain.len();
                for stmt in stmts.iter() {
                    let owner = self.walk(stmt.code(), st);
                    if let Stmt::Let { pat, .. } = stmt {
                        let mut bound = Vec::new();
                        pat.binders(&mut bound);
                        for name in bound {
                            st.chain.push(Binding {
                                name,
                                owner: owner.clone(),
                            });
                        }
                    }
                }
                let answer = match tail {
                    Some(t) => self.walk(t, st),
                    None => Owner::Fresh,
                };
                st.chain.truncate(depth);
                answer
            }

            NodeKind::Record { fields } => {
                for (_, value) in fields.iter() {
                    self.walk(value, st);
                }
                Owner::Fresh
            }

            NodeKind::Field { base, field } => {
                // A projection takes the field out when the record arrives at one owner — the
                // machine probes uniqueness at the access — and clones it otherwise, so the field
                // is worth what the record was.
                match self.walk(base, st) {
                    Owner::Blocked(why) => Owner::Blocked(why.reworded(format!(
                        "`.{}` was read out of a record something else still holds: {}",
                        field.name, why.text
                    ))),
                    other => other,
                }
            }

            NodeKind::List { items } => {
                for item in items.iter() {
                    self.walk(item, st);
                }
                Owner::Fresh
            }

            NodeKind::Perform { args, .. } => {
                for arg in args.iter() {
                    self.walk(arg, st);
                }
                Owner::Unknown(Why::new(
                    Cause::Handler,
                    "the value a handler answered, which the handler may still hold",
                ))
            }

            NodeKind::Handle { body, clauses, ret } => {
                // Clause captures are copied at handle entry, before the body runs.
                for clause in clauses.iter() {
                    self.poison_captures(&clause.captures, st);
                }
                if let Some(arm) = ret {
                    self.poison_captures(&arm.captures, st);
                }
                self.walk(body, st);
                for clause in clauses.iter() {
                    let mut params: Vec<Symbol> = clause.params.as_ref().clone();
                    params.extend(clause.resume.clone());
                    self.barrier(&params, &clause.body, st, &[]);
                }
                if let Some(arm) = ret {
                    self.barrier(std::slice::from_ref(&arm.binder), &arm.body, st, &[]);
                }
                Owner::Unknown(Why::new(Cause::Handler, "the value a `handle` answered"))
            }

            NodeKind::WithCell {
                init, binder, body, ..
            } => {
                self.walk(init, st);
                let depth = st.chain.len();
                st.chain.push(Binding {
                    name: binder.clone(),
                    owner: Owner::Blocked(Why::new(
                        Cause::Cell,
                        "a cell, whose region arena holds its contents",
                    )),
                });
                let answer = self.walk(body, st);
                st.chain.truncate(depth);
                answer
            }

            NodeKind::Simulate { body, captures, .. } => {
                self.poison_captures(captures, st);
                self.barrier(&[], body, st, &[]);
                Owner::Unknown(Why::new(Cause::Handler, "the value a `simulate` answered"))
            }

            NodeKind::WithRegion { body } => self.walk(body, st),
        }
    }

    /// A construct whose body may run again, later, or beside another task.
    fn barrier(&mut self, params: &[Symbol], body: &Code, st: &mut State, owners: &[Owner]) {
        let held = std::mem::take(&mut st.chain);
        st.chain = params
            .iter()
            .enumerate()
            .map(|(i, name)| Binding {
                name: name.clone(),
                owner: owners.get(i).cloned().unwrap_or_else(|| {
                    Owner::Unknown(Why::new(
                        Cause::Closure,
                        "a parameter of a closure whose caller this body does not name",
                    ))
                }),
            })
            .collect();
        self.walk(body, st);
        st.chain = held;
    }

    /// A capture that clones keeps a copy for the closure's whole life, so the binding's value has
    /// a second owner from here on; a capture that moves leaves the binding dead, which the
    /// liveness pass already guarantees nothing reads.
    fn poison_captures(&mut self, captures: &code::Captures, st: &mut State) {
        for (j, name) in captures.names.iter().enumerate() {
            if captures.owns.get(j) == Some(&Own::Owned) {
                continue;
            }
            if let Some(i) = st.index_of(name)
                && !matches!(st.chain[i].owner, Owner::Blocked(_))
            {
                st.chain[i].owner = Owner::Blocked(Why::new(
                    Cause::Capture,
                    format!("a closure captured `{name}` and holds its own copy of the value"),
                ));
            }
        }
    }

    /// What a name is worth where it is read.
    fn var_owner(&mut self, q: &QName, own: Own, st: &mut State) -> Owner {
        let bare = q.is_bare();
        if let Some(i) = bare.then(|| st.index_of(q.symbol())).flatten() {
            // A move takes the binding's value with whatever other owners it already had; a clone
            // is itself the second owner.
            return match own {
                Own::Owned => st.chain[i].owner.clone(),
                _ => Owner::Blocked(Why::new(
                    Cause::ReadAgain,
                    format!(
                        "`{}` is read again after this point, so this use clones it",
                        q.symbol()
                    ),
                )),
            };
        }
        // A definition named outside callee position is a definition applied by something this
        // analysis cannot see.
        if let Some(name) = self.costs.resolve_name(st.module, q)
            && self.costs.defs.contains_key(&name)
        {
            self.found.escaped.insert(name);
            return Owner::Blocked(Why::new(
                Cause::Program,
                "a top-level definition, which the program holds",
            ));
        }
        if !bare {
            return Owner::Blocked(Why::new(
                Cause::Program,
                "a module-qualified name denotes something the program holds",
            ));
        }
        // A free variable of the enclosing closure.
        Owner::Unknown(Why::new(
            Cause::Closure,
            format!(
                "`{}` is free in this closure; whether the closure's copy is the only other \
                 owner when the append runs is not a property of this body",
                q.symbol()
            ),
        ))
    }

    /// The builtin a callee names, or `None` when it names something else.
    fn builtin_of(&self, func: &Code, st: &State) -> Option<Builtin> {
        let NodeKind::Var { name: q, .. } = &func.kind else {
            return None;
        };
        if !q.is_bare() {
            return None;
        }
        let name = q.symbol();
        // A local binding or a definition of the same name shadows the builtin, exactly as
        // `Machine::lookup` orders them.
        if st.index_of(name).is_some() || self.costs.global(st.module, name).is_some() {
            return None;
        }
        Builtin::from_name(name)
    }

    /// The definition a callee names, when it names one.
    fn callee_of(&self, func: &Code, st: &State) -> Option<Symbol> {
        let NodeKind::Var { name: q, .. } = &func.kind else {
            return None;
        };
        if q.is_bare() && st.index_of(q.symbol()).is_some() {
            return None;
        }
        let name = self.costs.resolve_name(st.module, q)?;
        self.costs.defs.contains_key(&name).then_some(name)
    }

    /// What the value a call answers is worth.
    fn result_owner(
        &self,
        builtin: Option<Builtin>,
        callee: Option<&Symbol>,
        args: &[Res],
    ) -> Owner {
        let Some(b) = builtin else {
            return match callee.and_then(|name| self.tables.rets.get(name)) {
                Some(RetState::Fresh) => Owner::Fresh,
                Some(RetState::Param(k)) => match args.get(*k) {
                    Some(Res::Unique) => Owner::Fresh,
                    Some(Res::Param(j)) => Owner::Param(*j),
                    Some(Res::Blocked(why)) => Owner::Blocked(why.clone()),
                    Some(Res::Unknown(why)) => Owner::Unknown(why.clone()),
                    None => Owner::Unknown(Why::new(
                        Cause::Call,
                        "a call answering an argument it was not given",
                    )),
                },
                Some(RetState::Shared(why)) => Owner::Blocked(why.clone()),
                Some(RetState::Unsure(why)) => Owner::Unknown(why.clone()),
                None => Owner::Unknown(Why::new(
                    Cause::Call,
                    "the value a call answered, which the callee may also hold",
                )),
            };
        };
        match b {
            // Each builds a fresh `Vec` and hands it back; `push`'s two arms both answer a list
            // nothing else has seen.
            Builtin::Push | Builtin::Map | Builtin::Filter | Builtin::Range => Owner::Fresh,
            Builtin::CellGet => Owner::Blocked(Why::new(
                Cause::Cell,
                "`cell_get` answers a clone the region's arena still holds — the append cannot \
                 be in place however the call is written; `cell_update` is the fix",
            )),
            Builtin::MapGet => Owner::Blocked(Why::new(
                Cause::MapEntry,
                "`map_get` answers a clone the map still holds",
            )),
            // The same fact as `map_get`'s, over a list.
            Builtin::ListAt => Owner::Blocked(Why::new(
                Cause::Element,
                "`list_at` answers a clone the list still holds",
            )),
            Builtin::Fold => Owner::Unknown(Why::new(
                Cause::Call,
                "`fold` answers whatever its callback answered",
            )),
            _ => Owner::Fresh,
        }
    }

    /// What the callback builtins hand the function they call.
    fn callback_owners(
        &self,
        builtin: Option<Builtin>,
        at: usize,
        arity: usize,
        done: &[Res],
    ) -> Option<Vec<Owner>> {
        if at + 1 != arity {
            return None;
        }
        let element = Owner::Blocked(Why::new(
            Cause::Element,
            "an element the list being walked still holds",
        ));
        match builtin? {
            Builtin::Fold => {
                let seed = match done.get(1) {
                    Some(Res::Unique) => Owner::Fresh,
                    Some(Res::Param(k)) => Owner::Param(*k),
                    Some(Res::Blocked(why)) => Owner::Blocked(why.clone()),
                    Some(Res::Unknown(why)) => Owner::Unknown(why.clone()),
                    None => Owner::Unknown(Why::new(Cause::Call, "the seed of this fold")),
                };
                Some(vec![seed, element])
            }
            Builtin::Map | Builtin::Filter => Some(vec![element]),
            _ => None,
        }
    }

    /// A callback written at the call site, entered with the owners the builtin is known to hand it
    /// rather than with the `Unknown` a closure of unknown provenance gets.
    fn walk_callback(&mut self, owners: &[Owner], arg: &Code, st: &mut State) -> Owner {
        let NodeKind::Lambda {
            params,
            body,
            captures,
            ..
        } = &arg.kind
        else {
            // Named elsewhere: its body is checked where it is defined.
            return self.walk(arg, st);
        };
        self.barrier(params, body, st, owners);
        self.poison_captures(captures, st);
        Owner::Fresh
    }
}

impl Costs<'_> {
    fn resolve_name(&self, module: usize, q: &QName) -> Option<Symbol> {
        if q.is_bare() {
            return self
                .resolved
                .scopes
                .get(module)
                .and_then(|scope| scope.get(Namespace::Value, q.symbol()))
                .map(|b| b.qualified.clone());
        }
        self.resolved
            .lookup(module, Namespace::Value, q)
            .ok()
            .map(|b| b.qualified.clone())
    }

    /// The definition a bare name denotes, if it denotes one.
    fn global(&self, module: usize, name: &Symbol) -> Option<Symbol> {
        let found = self
            .resolved
            .scopes
            .get(module)
            .and_then(|scope| scope.get(Namespace::Value, name))
            .map(|b| b.qualified.clone())?;
        self.defs.contains_key(&found).then_some(found)
    }
}

/// Joins two branch answers.
fn join(a: Owner, b: Owner) -> Owner {
    match (to_res(a), to_res(b)) {
        (Res::Unique, Res::Unique) => Owner::Fresh,
        (Res::Blocked(r), _) | (_, Res::Blocked(r)) => Owner::Blocked(r),
        (Res::Unknown(r), _) | (_, Res::Unknown(r)) => Owner::Unknown(r),
        (Res::Param(k), _) | (_, Res::Param(k)) => Owner::Param(k),
    }
}

fn qualified(resolved: &Resolved, module: usize, name: &Symbol) -> Option<Symbol> {
    resolved
        .scopes
        .get(module)
        .and_then(|scope| scope.get(Namespace::Value, name))
        .map(|b| b.qualified.clone())
}
