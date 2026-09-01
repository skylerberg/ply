# ADR 0025 — Ownership: the cliff, not the count

Status: accepted — **not implemented**, except the one change named as P2 below,
which was built and measured while this document was written and then reverted.
Implements the decision recorded as ADR 0024. Amends ADR 0017 §4 and
`crates/ply-eval/src/rc.rs`'s module contract. **Re-sequenced by
[ADR 0033](0033-perceus-over-slots.md)**, which lands P2, subsumes P1 and P3 into
a slot-based environment, and re-poses the `Vector<T>` gate; every measurement
here is accepted by it and none re-taken.

> **ADR 0024 is not in this repository, and this document was written without
> it.** `docs/adr/` holds `0001`–`0021` and no `0022`, `0023` or `0024`;
> `find . -iname '*ownership*'` answers nothing; a grep for "ownership" across
> `*.md` reaches ADRs 0003, 0006, 0017, 0018 and 0019 only. Four separate design
> proposals were prepared against ADR 0024 and all four reported the same
> absence, so every claim any of them attributes to it — its §3 theorem, its §5
> promise that the copy be visible in the source, its §6 refusal of a `Builder`
> type, its §9 hash budget, its §10 ergonomic risk, its §11 open question about
> regions — is a paraphrase carried in a task brief, not a quotation.
>
> This document therefore does not cite ADR 0024 as authority for anything. It
> cites the tree. Where the brief's paraphrase is used it is named as such, and
> the two decisions the paraphrase asks for that this ADR **declines** are
> called out in §Decision 1 rather than buried, precisely because the document
> that would have argued the other side cannot be read. If ADR 0024 is recovered
> and says something else, §Decision 1 is the section to re-open.

## Context

### The defect, measured on the shipped standard library

`Value::List` is `Arc<Vec<Value>>` — `crates/ply-eval/src/value.rs`, whose own
comment calls it *"whole-list sharing rather than structural sharing: `push`
copies, which is fine at v0 sizes and costs no persistent-vector dependency."*
`builtins.rs` asks `Arc::get_mut`; when it answers `None` the whole vector is
copied. So an append is O(1) when the caller is the sole owner and O(n) when
anything else holds a reference, and which one a program gets is not visible in
its source.

Each standard-library module, run over its own test suite, with
`rc::Stats::updates` against `rc::Stats::updates_in_place` (taken 2026-08-28 on
this tree; the probe is described in §Provenance):

| module | updates | in place | copies | in place |
| --- | ---: | ---: | ---: | ---: |
| `std.json` | 3,290 | 3,277 | 13 | 99.6% |
| `std.http` | 6,299 | 12 | **6,287** | **0.2%** |
| `std.router` | 6,368 | 35 | **6,333** | **0.5%** |
| `std.db` | 6,989 | 6,040 | 949 | 86.4% |
| `std.trace` | 3,343 | 3,295 | 48 | 98.6% |
| `spikes/ply-lexer/lexer.ply` | 67 | 47 | 20 | 70.1% |

The two modules on the request path are the two that reuse nothing. ADR 0020 §4
priced the same defect end to end on the shipped `json::encode_string` —
0.03 / 0.07 / 0.22 / 0.79 s for k = 1,000 / 2,000 / 4,000 / 8,000 escapes,
ratios approaching 4× per doubling — and `README.md` records `std.trace`'s
sink as O(N²) for the same reason. This is not a hypothetical.

### Where the copies actually are

Attributed to source lines by instrumenting `push`'s copying arm with its span.
This is the measurement the design turns on and no proposal had it:

| copies | site | shape |
| ---: | --- | --- |
| 4,100 | `http.ply` `body_more` | `let taken = push(out, step.out);` and more statements — `out` is a **parameter** |
| 2,062 | `http.ply` `absorb` | growing field **first** in a record literal |
| 34 | `http.ply` | argument position |
| 32 | `router.ply` `numbered` | growing field first |
| 21 | `http.ply` | argument position |
| 20 | `lexer.ply` `emit`/`err` | projection out of a record parameter still held |
| 13 | `json.ply` `escape_runs` | `push(push(acc, x), y)` |

Two causes, and neither is the one the language would be changed for:

1. **A parameter is never released from a block continuation's scope.**
   `code.rs` seeds `cumulative` with an empty vector and grows it only from
   `stmt_binders(stmts[i])`, so no function parameter can ever appear in a `Dead`
   set. An accumulator threaded as a `let` binding is reused; the identical
   accumulator threaded as a parameter is not. Measured directly:
   `{ let t = push(xs, 1); let u = 7; len(t) + u }` is **1 of 1 in place** when
   `xs` is a `let` and **0 of 1** when `xs` is a parameter.

2. **`rc::carry` is all-or-nothing.** It is
   `carry(env, remaining: bool) -> if remaining { env.clone() } else { Env::empty() }`
   (`rc.rs`), called at exactly eight sites — `frame.rs`,
   `machine.rs`, `handler.rs` — each passing a boolean of the
   form `next + 1 < args.len()`. A frame with *any* sub-expression left carries
   the *whole* scope, so every binding in it sits at two owners for the whole of
   that sub-expression, even when the sub-expression is a literal that reads
   nothing.

### The positional rule is worse than the record says

`GAPS.md` §1 and ADR 0020 §4 both state the rule as *position in the enclosing
node*. That is right and it is not the whole rule. The rule **compounds at every
enclosing node on the path from the `push` up**. Measured, n = 200:

| written | in place |
| --- | ---: |
| `go(i + 1, push(acc, i))` | **200 / 200** |
| `go(push(acc, i), i + 1)` | 0 / 200 |
| `go(i + 1, {k: s.k + 1, out: push(s.out, i)})` | **200 / 200** |
| `go(i + 1, {out: push(s.out, i), k: s.k + 1})` | 0 / 200 |
| `go({k: s.k + 1, out: push(s.out, i)}, i + 1)` | **0 / 200** |
| `fold(range(0, 200), [], \|acc, x\| push(acc, x))` | 200 / 200 |

Row five is the finding. The growing field is last in its record literal — the
rule as written down in `lexer.ply`, in `GAPS.md` §1 and in ADR 0020 §4 —
and the program is still quadratic, because the record is not last in the
*call*. An author who learns the documented rule and applies it correctly still
gets the quadratic. Two careful authors have already written a version of this
rule down and both were corrected; this is the third correction, and it is the
argument that no rule of this kind should have to be learned at all.

> **A smaller correction, in passing.** `lexer.ply` attributes the rule to
> `ply_eval::rc::carry`, *"called from `machine.rs` and `frame.rs`"*.
> `frame.rs` is a `carry` site. `machine.rs` is a closing brace; the
> three sites in that file are `1035`, `1092` and `1122`. The comment is right
> about the mechanism and stale about one of its two citations, which is worth a
> one-line fix when P1 rewrites that mechanism anyway.

## The property this ADR must not break

**Program meaning does not change.** This is ADR 0017's governing property and it
is inherited verbatim. Everything below alters cost, not semantics. In
particular, and stated here because it is the single decision that makes the rest
safe to ship: **`Env::take_unique`'s chain walk and `Arc::get_mut` in `push`
both stay, unconditionally, in release builds.** The static analysis may be
wrong; when it is, the program is slow and never incorrect. `rc.rs`'s module
comment already says why — *"a wrong `Owned` therefore costs a wasted walk, never
a wrong answer"* — and this ADR does **not** promote `Own` to a permission.

Everything in §Decision 1 follows from taking that sentence seriously.

## Decision 1 — Ply does not acquire a parameter mode, and the reason is measured

All four proposals put a three-point mode (`read`/`keep`/`own`,
`peek`/`share`/`own`, or `own`/`share`) on each parameter of an arrow, inferred
over SCCs the way an effect row is, carried in `Type::Fn`, and published in the
signature. **That is declined.** No `own` keyword, no mode in `Type::Fn`, no
`Scheme` field, no change to `unify`, no `DefHash` movement, no
`FRONTEND_VERSION` bump, no stdlib signature churn.

Three reasons, in descending weight. The first is a measurement taken for this
document.

### 1.1 A parameter's uniqueness is not a property the caller can promise

```ply
effect amb { read flip() -> Bool }

fn grow(acc: List<Int>, b: Bool) -> Int = len(push(acc, if b { 1 } else { 2 }))

fn probe() -> Int = {
  let xs = [1, 2, 3];
  handle { let b = amb.flip(); grow(xs, b) }
  with { amb.flip() resume k -> k(true) + k(false), return x -> x }
}
```

`grow`'s `acc` has **exactly one occurrence**; that occurrence is its last use; it
is free in no closure, stored in no cell, held by no record, and reached by no
projection. Every rule stated by every one of the four proposals infers `own`
here. Measured: `updates = 2, updates_in_place = 0`, and an instrumented `push`
reports **`owners = 2` on both resumptions**. The second owner is the
continuation's captured segment, which `rc.rs`'s own module comment describes —
*"a resumed frame is cloned out of the continuation's shared segment, so its
scope is shared, so nothing in it is ever taken"*.

The one-shot control is the part that decides it. Under
`amb.flip() resume k -> k(true)` — a single, tail resumption — the same program
measures `owners = 2` as well, and with no handler at all it measures
`updates = 1, in_place = 1`. **So it is capture, not multiplicity.** Any
`perform` in the enclosing dynamic extent puts a second owner on the value, and
`region_kind.rs`'s own corpus measurement says how common that is over this
repository: *"113 regions, 0 `unique`, 113 `shared`, every one of them because of
a tail-resumptive clause"*, because *"the canonical Ply region is
`with_cell[s](0) { c -> handle .. }` — a cell backing a handler"*.

A mode is a contract between a caller and a callee. Neither party to that
contract can discharge it: the callee cannot, because the second owner is created
outside it; and the caller cannot, because the handler that captures the
continuation may be installed by *its* caller, arbitrarily far up. The only sound
side condition is "no `perform` anywhere in the dynamic extent", which over this
corpus is 0 regions out of 113.

**The dilemma, stated as sharply as it can be, because it is the whole of
Decision 1.** A parameter mode can mean one of two things.

- *"The caller does not read this value again after the call."* This is
  syntactic, local, and genuinely checkable — and it buys **nothing**, because
  not reading a value again is not the same as being its only owner, and only the
  second licenses an in-place append. `let a = xs; grow(a); len(xs)` satisfies it
  and measures `owners = 2`.
- *"This value has one owner when the callee runs."* This is what would license
  the append, and §1.1 measures it false at a site every proposal's rules accept,
  with no `perform` written anywhere in the callee.

So the mode is either checkable and useless, or useful and uncheckable. Every one
of the four proposals wrote the first meaning into its surface and reasoned about
the second in its worked examples, which is why all four report a burden of zero
and none reports a benefit that is theirs rather than `carry`'s.

The answer stays correct today only because `Arc::get_mut` refuses. That is the
right behaviour and it is why the copy under a multi-shot handler is not a
pessimization but **the semantics**: the second resumption must not observe the
first's append. A design that promised O(1) there would be promising a wrong
answer, and a design that promises O(1) *and* keeps the guard is promising
something it silently does not deliver — the brief's own named failure mode, a
green result over unexplored space, inside the mechanism meant to cure it.

### 1.2 The mode cannot be cached under a `DefHash`, and a mode that is re-derived per program is not a signature

`region_kind.rs` settles this against itself, for the analysis that asks
the closest available question:

> A cache keyed by the region's definition hash would therefore answer `unique`
> where a fresh inference answers `shared` … The sound key is the whole
> `(Program, Resolved)` pair.

Two of its inputs are whole-program and neither is inside the hashed dependency
closure of the definition: whether *any* capture is written *anywhere*, and
whether a name denotes a definition or a local. `a_capture_in_an_unrelated_module_makes_a_region_shared`
in `crates/ply-eval/tests/suite/region_kind_inference.rs` is the program that pins it.
Adding a `handle` to a module a definition neither names nor reaches flips the
answer — and moves no hash.

Every proposal put the mode in `Type::Fn`, hence in `Scheme`, hence in
`DefInfo`/`KnownDef`, hence in the content-addressed store, and argued from the
effect row's precedent that a warm check would then never walk the callee's body.
A published row is sound to cache because a footprint is a function of the
definition's own hashed closure. Uniqueness is not, for exactly the reason
`region_kind` gives. The precedent does not transfer.

### 1.3 The surface cannot describe the file the milestone exists for

`spikes/ply-lexer/lexer.ply` has **zero** `List`-typed function parameters.
Counted: its three `List<` occurrences are at lines 91, 107 and 108, and all
three are record *fields* (`Lexed`, `Scan`). `GAPS.md` records that Ply's
recursion caps at 10,000 frames, so a lexer threads its state through a fold as a
record — which is the shape the whole spike is about, and the shape `absorb`
(2,062 copies), `numbered` (32) and `trace.ply::append` (35) all take.

A mode on a *parameter* has nothing to say about any of them. Every proposal's
worked examples are pure-functional accumulators passed as parameters, which is
the one shape a last-use analysis already handles at zero cost, and the two
largest measured copy blocks in the tree are not that shape.

## Decision 2 — The property Ply checks is "this append did not copy", and it is checked by counting, not by proving

`rc::Own` stays what its doc says it is: *"an optimization hint and never a
permission"*. What changes is that the hint becomes **reported** and
**falsifiable**.

This is the direct answer to the brief's paraphrase of ADR 0024 §5 — that the
absence of reuse be a compile-time event rather than a production one — in the
only form §1.1 leaves standing. Ply cannot prove a value has one owner. It can
say where it expects one, print that expectation, and fail a test when the
expectation was wrong. Three parts:

**2a. `ply check --costs`.** `crates/ply-cli/src/signature.rs` already renders
`ply check --types` at a pinned width because *"`ply check --types` is diffed in
review and pinned in tests"*. It gains a per-definition line naming each `push`
and whether the lowering marked its list argument `Owned`:

```
std.json.escape_runs
  json.ply:LL  push(acc, ..)                      reused
  json.ply:LL  push(push(acc, ..), ..)  inner     COPIES — acc is carried for the
                                                  trailing argument
```

The legibility mechanism is grafted from the **regions** proposal, which took it
from `Cell[users]<Int>`: `infer.rs` gives a written `Cell<T>` a fresh
region variable nobody typed and `print.rs` fills the brand back into the
printed signature, so a property is inferred and *shown* without being *asked
for*. The **param-marks** proposal supplied the correction that matters: what
belongs in that slot is a **count**, not a type, because the honest form of this
guarantee is a cost.

**2b. The falsifier, and this is the part that stops the result being vacuous.**
`crates/ply-eval/tests/suite/reference_counting_cost.rs` already prints in-place rates
per corpus with a floor. It gains a per-site assertion: **every `push` whose list
argument the lowering marked `Own::Owned` must be counted in place, or the test
fails, naming the site.** This is grafted from the **regions** proposal's
soundness judge and it is the single best idea produced in the round. It costs an
afternoon, it needs no language change, and it converts a checker that asserts a
property it has not established into one that fails loudly on the existing
corpus. `rc::Stats` is already documented as diagnostics-only — *"nothing here
enters a value, a hash, a cache key, a footprint or a seeded choice"* — so
nothing about it can move a program's meaning.

Registering it now, before it is built, per CONTRIBUTING §"Measure an ADR's
motivating claim before accepting the ADR": **the assertion will fail on the
tree as it stands.** The multi-shot program in §1.1 marks `acc` `Owned` and
copies. That is the point. Sites that cannot be reused are to be reclassified —
by making `Live` refuse to mark them (§Decision 4) — not exempted.

**2c. `copy(x)`.** One new builtin, `<a>(a) -> a`, semantically the identity:
`copy(x) == x` for every `x`, and no test, no `law`, no spec and no
`--engine both` divergence check can tell a program with one from the same
program without it. It is `builtins.rs`'s existing copying branch promoted
from a silent fallback to a written word. Grafted from **Sole**, whose judges
were right that identity-ness is the property that makes an escape hatch safe for
code a model writes at speed: every diagnostic has a mechanical fix that provably
cannot change what the program means.

**Sole's own weakness 8 is adopted with it**: because inserting `copy` cannot
change meaning, it is exactly how a model discharges a cost signal without
understanding it. So there is deliberately **no `--fix-copies`**, and `copy` is
never the fix a diagnostic *recommends* — §Decision 4's one warning names the
reordering or the builtin, and mentions `copy` last.

## Decision 3 — Four changes to the evaluator, in this order, each gated on the counter

No syntax, no type change, no hash movement, no annotation. This is where every
measured win is.

### P2 — a parameter may appear in a `Dead` set. **Built and measured.**

`code.rs` seeds `cumulative` from statement binders only. Seed it instead
from the enclosing barrier's bindings, which `rc::Live` already tracks in
`ownable` (`rc.rs`, *"one frame per barrier … holding every name bound
anywhere inside it"*), so the statement that is a parameter's last reader names
it in its `dead` set and the block's continuation carries a scope without it.

Implemented for this ADR as roughly ten lines behind an environment flag, run,
and reverted. Measured:

| module | before | after | copies removed |
| --- | ---: | ---: | ---: |
| `std.http` | 0.2% | **65.7%** | 4,126 |
| `std.router` | 0.5% | **65.3%** | 4,126 |
| `std.db` | 86.4% | **96.2%** | 683 |
| `std.json`, `std.trace`, lexer | — | unchanged | 0 |

All of `cargo test -p ply-eval` passes with it enabled, `differential_corpus`,
`exploration_soundness`, the twelve `region_*`/`reference_*` files and
`reference_counting_audit` included. Eight adversarial programs — a parameter
captured by a closure, by a handler clause, by a cell, read in a later `match`
arm, read in the tail after statements, and shadowed by an inner binder of the
same name, each after its last *direct* read — all answer correctly, and
`takes_moved` rises rather than falls. The safety argument is `Live::close`'s:
a closure's free variables *"become reads at the construct that captured them —
never last ones"*, so a name a later construct captures is live at that statement
and is not released. **That argument is not yet written as a case analysis and
that is the condition on landing P2** (§What would make this wrong, item 2).

This is the **regions** proposal's C3 finding, which was the most valuable single
diagnosis in the round and which its author reached by reading `code.rs` rather
than by measuring. It is confirmed here at 4,126 + 4,126 + 683 = **8,935 copies**.

### P1 — `carry` takes a dead set, not a boolean

Replace `carry(env, remaining: bool)` with `carry_released(env, dead_here)`,
where `dead_here` names the bindings this sub-expression is the last reader of, so
a pending frame carries the scope *minus* them. All four judging lenses on all
four proposals named this as the graft to take regardless of which design won,
and they were right; it is entered here as P1 rather than first only because P2 is
built and this is not.

Its ceiling is measured rather than argued: the same programs, reordered so the
`push` already sits in last position at every enclosing node, are **200 of 200 in
place** (§Context, rows 1, 3 and 6). P1 is what makes that hold without the
reordering. It is the fix for `json.ply` `escape_runs`, for the argument-position
sites at `http.ply`, and for the nested `emit(err(s, ..), ..)` shape in
the lexer that the current boolean cannot express — "release `s`, keep `so_far`"
is not a value a `bool` has.

**Two conditions, both from judges who checked the mechanism against `env.rs`.**
`Env::release` clones the value of every binding *above* the deepest released one
into a fresh chain and shares the tail below it (`env.rs`), so a release at
every sub-expression replaces an `Rc` bump on the machine's hottest path with an
O(scope-depth) operation, **and can newly refuse a `take_unique` that succeeds
today** by introducing a shared link. `frame::dispatch < Machine::step <
Machine::call` is already 45.5% of marginal allocations by ADR 0017's own census.
So: (i) the released shape is precomputed at lowering and the common case
(`dead_here` empty) must remain the existing `clone()`; (ii) **P1 does not land
until `w6_report_allocations` has been re-taken**, because ADR 0017's standing
correction is that this milestone's premise moved that number the wrong way once
already. `Env::keep_only(live)`, built up from empty, is the alternative
primitive if `release` measures badly, and the **param-marks** implementation
judge is owed the observation that it is probably the right one.

Scope note the proposals understated: `Dead` lives only on `Stmt`
(`code.rs`). `App`, `Record`, `List` and `Perform` carry no per-argument
dead sets, so P1 is a `Code` IR change plus lowering, not an edit to eight call
sites.

### P3 — a projection may move a field out of a record that is dying

`emit` and `err` (`lexer.ply`) write the growing field **last**, exactly
as the documented rule asks, and still copy 20 times. The record parameter `s` is
read for `s.diags` and then for `s.toks`, and `Frame::FieldAccess`
(`frame.rs`) answers `v.clone()` unconditionally — it never moves the field
out even when this frame is the record's only owner. `absorb` is the same defect
with the fields in the other order, and it is 2,062 copies.

**A negative result, measured here, that should save the next implementer a
day.** The five-line half of P3 — take the field out with `Arc::get_mut` when the
frame owns the record — was built and measured for this document and changes
**nothing**: not alone, and not with P2 enabled. The record base is still carried
by the enclosing literal's own frame, so it is never at one owner when the
projection runs. **P3 is gated behind P1** and has no value before it. Sequenced
accordingly.

The general form — path-granular liveness, so `r.pieces` can be moved while
`r.total` is still to be read — is what `absorb` needs, and it is the piece the
**regions** proposal explicitly declined to build, on the grounds that a wrong
answer there is a wrong program rather than a slow one. That judgement is
accepted. P3 is therefore scoped to the case where the *whole record* is dead at
the literal, which is `absorb`, `numbered`, `append`, `emit` and `err`; a record
still live afterwards keeps its clone. **Sole**'s Step 5 is the source, minus its
ambition.

### P4 — `cell_update` and `map_update`

`push(cell_get(c), x)` is **63 of the tree's 171 `push` sites — 37%** (40 in
`db.ply`, 21 across `examples/hello.ply`, `orders.ply`, `desk.ply`, `bank.ply`,
`echo.ply`, 2 in fixtures). Measured at **0% in place, unconditionally**: the
arena still holds the value while `push` runs, so `Arc::get_mut` cannot succeed
whatever any analysis says. No ownership design fixes this, and every proposal
that claimed otherwise did so via a syntactic peephole that one judge showed to
be unsound — a `perform` or a `task.yield()` between the take and the set exposes
the emptied slot.

```ply
cell_update : <a | e> (Cell<a>, (a) -> a / e) -> Unit / {cell.write[r] | e}
map_update  : <k, v | e> (Map<k, v>, k, (v) -> v / e) -> Map<k, v> / e
```

Both take the value out *inside one builtin*, which establishes soleness at
runtime rather than proving it statically — which is why they are sound under
multi-shot, and why they are the right shape. Grafted from **Sole** and
**ownership-row**, which proposed them independently.

The 63 sites are a **library migration, not a compile error**. The old idiom keeps
working and keeps its quadratic; `ply check --costs` names it and §Decision 4's
warning points at it. Making it an error, as two proposals did, turns every
shipped example program into a build failure and prices `copy` — the
pessimization — as the fix.

The implementation judges are owed the note they raised: `cell_get`/`cell_set` are
special call forms in `infer_cell_op` rather than schemes, because of region-brand
inference, so a closure-taking `cell_update` needs a third call form with row
joining, a new machine `Frame`, a tree-walker twin, and an entry in
`region_kind.rs`'s hardcoded callback-builtin list or every region containing one
silently becomes `shared`. Budget it at 400–700 lines, not a line item.

## Decision 4 — One warning, no new error

`W0611 COPYING_APPEND`, raised where the lowering can show the list argument of a
`push` **cannot** be `Owned`: it is `cell_get(c)`, or `map_get(..)`, or a name
with a later read in the same body, or a name free in a closure. It names the
reordering, `cell_update`/`map_update`, or `copy` — in that order.

It is a warning and not an error because the measured false-positive rate of any
analysis of this shape is not zero and the direction is wrong: `json.ply`'s
`collect_prices` captures its accumulator in a lambda, which `Live::close` must
call unownable forever, and it is **100% in place at runtime**. A hard error there
forces `push(copy(acc), p)` and makes a linear loop quadratic — which the
**param-marks** ergonomics judge identified as that proposal's disqualifying
defect. The same trap is avoided here by not building the error.

`E0450`–`E0499` and `E0506`+ are free (`E0505` is the highest live code); none is
taken. `W0610` is the highest live warning, so `W0611` is next. The retired
field-order lint discussed in ADR 0020 §4 lives in
`spikes/ply-lexer-rc/fieldorder.ply` and was never a `W` code, so nothing is
being reused under a name that once meant something else.

## The annotation burden, as a number

**Zero, on all four functions, and that number is worthless on its own** — this
design has no annotation, so its burden is zero by construction, and reporting it
as a result would be exactly the vacuous green the brief warns about. Three
numbers that are worth something:

| | `json.ply` `escape_runs` | `http.ply` `chunk_trailers` | `router.ply` (all) | `lexer.ply` (all) |
| --- | ---: | ---: | ---: | ---: |
| annotations required | 0 | 0 | 0 | 0 |
| **forced source edits** | **0** | **0** | **0** | **0** |
| copies today | 13 | 0 | 32 + 12 | 20 |
| copies after P2 (**measured**) | 13 | 0 | 32 | 20 |
| copies after P1+P3 (**projected**) | 0 | 0 | 0 | 0 |

Read the third row before the first. **`chunk_trailers` contains no `push` at
all.** It takes `out: List<Bytes>`, passes it to `waiting` and hands it to
`bytes_concat_all`; its thirteen-field `Limits` is thirteen `Int`s. It has no
cliff, this design does nothing for it, and any proposal reporting a win on it is
reporting one it did not get. What builds the `out` it receives is `body_more`,
and that is P2's 4,100.

The last row is a **projection, not a measurement**, and is labelled so. P2 is
measured because it was built. P1 and P3 are not built; what is measured for them
is their *ceiling* — the same shapes, reordered by hand, reach 200 of 200 in
place. Anyone budgeting from this table should budget from row four.

**Tree-wide the honest number is 63 forced edits, all of one shape**
(`cell_set(c, push(cell_get(c), x))` → `cell_update`), all mechanical, all
optional in the sense that the old form keeps compiling, and all in test doubles
and example programs where N is small — but the shape is the one a production
handler would use, and `README.md` already records `std.trace`'s sink as
quadratic for exactly it.

**And the number nobody should take from this document**: `Value::Bytes` is
`Arc<[u8]>` with no spare capacity and no in-place append, so `bytes_concat`
cannot reuse anything whatever the analysis says. `bytes_concat(` occurs 35 times
in `json.ply`, 21 in `http.ply` and 29 in `lexer.ply`. `json.ply`'s
`string_chunks` and `lexer.ply`'s `string_lit`/`bytes_body` are quadratic on
`Bytes` and their own comments say so. **Nothing in this ADR touches any of
them.** If a reader takes "ownership fixes the quadratics" away from this
document, that reader has been misled. Making `Bytes` an `Arc<Vec<u8>>` would fix
it, is exactly the kind of `Value` change ADR 0019 §4 refuses on measured
grounds, and is not scoped here.

## Soundness

There is no soundness obligation to discharge, and that is the design. `Own` is
not promoted to a permission; `take_unique`'s chain walk and `Arc::get_mut` both
stay in release builds; a wrong answer from the analysis costs a copy. The four
hazards the brief names are therefore all *precision* questions, and each is
answered by a measurement rather than an argument.

**Multi-shot continuations.** Two owners, measured, at an occurrence every
proposal's rules call unique (§1.1). Under P1–P4 that site copies and answers
correctly, which is the required behaviour: the second resumption must not
observe the first's append. What this ADR adds is that `ply check --costs` will
*say* the site copies, and §Decision 2b's assertion forces `Live` to stop marking
it `Owned` rather than letting the discrepancy sit. `reference_counting_audit.rs`
records that the tree-walker answers `E0504` for a clause binding a continuation,
so `--engine both` is blind here; the falsifier is the only instrument that is
not, which is why it is a decision and not a nicety.

**Closures capturing values.** `Live::close` already marks every free variable of
a barrier `Borrowed` forever and `Live::tracked` already restricts ownership to
bindings of the current barrier. Measured: a closure capturing `xs` and used after
a `push(xs, ..)` gives `owners = 2` and copies, against a control at 1 of 1. This
is sound today and P2 does not weaken it — that is what the eight adversarial
programs test.

**Aliases the liveness of a *name* cannot see.** This is the hole that would have
sunk a mode system and it is worth stating even though this design does not fall
in it. In

```ply
{ let xs = [1,2,3]; let a = xs; let b = len(push(a, 3)); b + len(xs) }
```

`a` is genuinely dead at the `push` — its last use, on every path — and there are
`owners = 2`, measured. Name-liveness is not value-uniqueness. Storing into a
record, a cell or a closure has the same shape and all three measure `owners = 2`
against a control at 1. Every one is correct today because of the guard.

**Regions and cells.** §Decision 3 P4 is the answer: the arena is a genuine second
owner for the whole of `push`, so `push(cell_get(c), x)` is 0% in place by
construction and only a fused builtin fixes it. Nothing here changes what a
region reclaims or when. `escape.rs`'s enumerated boundaries — a host operation's
argument, a host handler's or the runtime's answer, an entry point's argument —
are the places a value acquires an owner no analysis can see, and P1's dead sets
must not release across them; that list is already enumerated and already tested
by `region_isolation_audit.rs` layer 2, and reusing it verbatim is the one piece
of region machinery ownership should not re-derive. Grafted from
**ownership-row**, which found it.

**The two engines.** `crates/ply-eval/src/interp.rs` is 1,658 lines and contains
**zero** occurrences of `Own::`, `take_unique`, `rc::carry`, `.release(` or
`use crate::code`. The tree-walker receives none of P1–P3 and stays slow, and
`--engine both` compares answers rather than costs, so it cannot see a cost
divergence in either direction. This is a real gap and it is not closed here.
What makes it survivable is that none of P1–P3 can change an answer: P2 and P1
change which scope a *pending frame* carries and P3 changes whether a field is
moved or cloned, and `Env::release` is functional — *"it builds a new chain and
never writes through a shared link"* — so a wrong release is
`codes::INTERNAL_ERROR` naming the binding, loud, and never a different value.
P4 does add two builtins and those **do** need tree-walker twins or
`--engine both` breaks; that is in P4's 400–700 lines.

**One thing this design cannot see, recorded rather than resolved.**
`crates/ply-eval/src/memo.rs` retains a `Value` for every nullary pure definition
for the run and hands out `value.clone()`, so a memoized constant has a permanent
second owner. Both engines consult it, and its key is the *published row*, which
is whole-program information again. `push(table(), 3)` for a nullary `table` did
**not** reproduce as a copy in the harness used here (the memo needs
`CheckOutput` armed and this probe did not arm it), so the claim one judge made
about it is neither confirmed nor refuted by me. It is a fourth route to a second
owner that no local rule models, it is harmless under this design because the
guard catches it, and it would have been unsound under a mode system for the same
reason as §1.2.

## Regions — answering the open question the brief attributes to ADR 0024 §11

**No. Ownership does not build on the region machinery, and the dependency runs
the other way.** Three arguments, two of which are the runners-up' and are better
than anything this document would have reached alone.

**Regions are already downstream of the ownership analysis.** ADR 0017's
correction block says `rc.rs` *is* §4's implementation — *"`crates/ply-eval/src/rc.rs`
holds the liveness analysis (`Live`, `Own`, `Dead`) and `code.rs` runs it at
lowering"*. Building the ownership check on `brand_in` would make it depend on
the region check, which depends on the ownership analysis. Not a cycle in the
code; a cycle in the argument. (**ownership-row**.)

**They ask converse questions with opposite quantifiers, and only one is
flow-sensitive.** A region asks whether a value can reach *outside* a scope —
existential, over exits, decided on a **type**, by `brand_in` over a resolved
type, which is flow-insensitive by construction and right to be. Ownership asks
whether anything else can reach a value *here* — universal, over the rest of the
activation, decided on an **occurrence**. The measurement that settles it is
mine and it is two lines: `{k: s.k + 1, out: push(s.out, i)}` and
`{out: push(s.out, i), k: s.k + 1}` have identical types, identical values and
identical semantics, and measure 200/200 and 0/200 in place. **No predicate over
a resolved type can separate those.** (**regions** and **param-marks** both
reached this; the second stated it in this form.)

**Confinement is not a count**, and the three-line proof is the **regions**
proposal's: `with_region[r] { let xs = [1,2,3]; let ys = push(xs, 4); len(xs) }`
escapes nothing and has two owners inside the region. It must answer 3, and it
does, because `push` copies. Make region-brandedness license an in-place `push`
and it answers 4 — a change of program meaning, forbidden by the same ADR that
defines the regions.

**What regions genuinely contribute** is `escape.rs`'s enumerated boundary list
(§Soundness), the site-and-defer scheduling shape, and the phantom-slot
presentation grafted in §Decision 2a. The one-line answer, which is the
**regions** proposal's and is correct: *regions are where ownership is lost, not
where it is established.*

## What this costs

Small, and that is the strongest thing about it.

| crate | files | note |
| --- | ---: | --- |
| `ply-syntax` | **0** | no keyword, no new `ExprKind`, no parser change |
| `ply-core` | **0** for P1–P3; ~2 for P4's builtin signatures | no `Type::Fn` change, no `Scheme` change, no `unify` change |
| `ply-hash` | **0** | modes never enter syntax, so `normalize.rs` is untouched |
| `ply-store` | **0** for P1–P4 | no stored type moves |
| `ply-prove` | **0** | the 22–34 mechanical `Type::Fn` sites every mode design pays are not paid |
| `ply-eval` | ~8 | `rc.rs`, `code.rs` (dead sets on the multi-argument `NodeKind`s), the 8 `carry` sites in `frame.rs`/`machine.rs`/`handler.rs`, `frame.rs::FieldAccess`, `builtins.rs`, `interp.rs` twins for P4, `region_kind.rs`'s callback list |
| `ply-cli` | ~2 | `signature.rs` and one flag |
| stdlib | 63 optional call-site edits, 0 signature changes | |

**No `DefHash` moves. No `FRONTEND_VERSION` bump. No re-run. No review baseline
is invalidated** — `reviews.rs` is keyed by `DefHash`, so every human review
survives, which writing `own` into a signature would not have allowed. That
argument is the **Sole** implementation judge's, made against Sole's own
proposal, and it is the best implementation-cost argument available to any design
here; it is claimed for this one because this one actually earns it.

For contrast, and because a reader should see what is being declined: the mode
designs were priced by three independent implementation reviews at 28–55 files
across 6–8 of 14 crates, 2,500–3,500 lines, 128 `Type::Fn` sites in 28 files, a
mandatory `FRONTEND_VERSION` bump and a full re-run — before one quadratic is
fixed. P2 alone is ten lines and removes 8,935 copies.

## The persistent-vector fallback, priced — and a pre-registered result I did not get

The brief invites reconsidering ADR 0024 §10's fallback: make `Vector<T>`
structurally shared so `push` is O(1) whoever owns it. Nobody in the round
measured it. I pre-registered criteria (`/tmp/adr0025/prereg.md`, addendum,
written before running) and measured `rpds::Vector` — which is **already a
workspace dependency at 1.2.1**, since `Map` is `rpds::RedBlackTreeMap`, so the
dependency cost is zero — against today's `Arc<Vec<Value>>`, release build:

| n | (a) `Arc<Vec>` unique | (b) `Arc<Vec>` shared | (c) rpds unique | (d) rpds shared | b/a | c/a | d/a |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 4,000 | 0.00010 | 0.03047 | 0.00015 | 0.00096 | 316 | 1.59 | 10.02 |
| 8,000 | 0.00019 | 0.11183 | 0.00027 | 0.00170 | 587 | 1.44 | 8.93 |
| 16,000 | 0.00046 | 0.42247 | 0.00054 | 0.00345 | 927 | 1.18 | 7.56 |
| 32,000 | 0.00108 | 1.72132 | 0.00116 | 0.00815 | 1,596 | 1.07 | 7.56 |

Per-doubling: (a) 2.37×, (b) **4.07×**, (c) 2.15×, (d) 2.37×.

The shape is the whole finding. Column (b) — today's shipped behaviour whenever
anything else holds the list — is quadratic and its penalty against the good case
**grows without bound**: 316× at n = 4,000, 1,596× at n = 32,000. Column (d) is
linear and its penalty is **flat at 7.56×**. A structurally-shared vector does not
make the bad case fast; it converts an unbounded penalty into a constant one,
and it does so for *every* case this ADR cannot otherwise reach — the 63 cell
sites, the multi-shot site, the memo, every alias.

**Against my pre-registered rule, two of four criteria failed, and I do not get
to claim this result.**

| criterion, fixed in advance | measured | |
| --- | --- | --- |
| (d) non-quadratic | 2.37× per doubling | **pass** |
| (c)/(a) < 4 | 1.07 at n = 32,000 | **pass** |
| (d)/(a) < 6 | **7.56, stable** | **fail** |
| index ratio < 5 | **7.07 at n = 64,000** | **fail** |

**The index criterion was ill-posed when it was written, and that was found out
after measuring: there was no list index builtin at all.** No Ply program could
pay the index cost, so the O(1) index that `Arc<Vec<Value>>` buys was unreachable
from the language. **That made the criterion meaningless rather than met, and it
is recorded as a miss rather than quietly dropped, because a criterion rewritten
after seeing the number is not a criterion.**

**The miss stands and its reason no longer holds.** ADR 0027 adds `list_at`, so
the index cost is now payable and **the criterion is well-posed for the next
taking.** One caution for whoever takes it: ADR 0027 §7 measures a peek and finds
it is **almost all interpreter dispatch** — a `map_get` peek and a `list_at` peek
cost the same to within a couple of percent — **so the index arm will price a
small term unless the backend has landed.**

That gives this ADR's own `Vector<T>` gate below a term it did not have when the
gate was fixed: a chunked structurally-shared vector would make `list_at`
O(log₃₂ n) where it is O(1) today, **and there is now a builtin for that to be a
cost *of*.** The gate's wording is unchanged — **a bar moved after a measurement
is not a bar** — but anyone re-taking it should price the index arm rather than
**record it as meaningless a second time.**

So: **the representation change is not taken now.** It is the fallback, it is
now priced, and the gate is fixed here, before the measurement, so a number
cannot set the bar it clears:

> **After P1–P4 have landed and `w6_report_allocations` has been re-taken,
> `Vector<T>` becomes a chunked structurally-shared vector if either
> `std.http` or `std.router` is below 90% in place over its own test suite, or
> if the per-site assertion in §Decision 2b is failing at more than 5% of
> `Owned`-marked sites.** Below that bar the analysis has not delivered and the
> cliff should be removed by representation instead. Above it, the remaining
> copies are the ones the semantics require.

One caveat in the fallback's favour that this measurement understates: elements
here were `Value::Int`, whose clone is trivial. The shipped corpus's lists hold
`Bytes` and records, whose clone is an `Arc` bump each, so column (b)'s copy is
more expensive in the corpus than in this rig and column (d)'s advantage is
larger than 7.56×. Stated, not leaned on. One against it, and it is the one ADR
0017's correction block exists to warn about: rpds allocates spine nodes where an
in-place append allocates nothing amortized, so the change would very likely move
allocations per request **up**, which is the number this milestone's three
predecessors are judged on.

## What would make this wrong

1. **If `Env::release` on the hot path costs more than P1 saves.** This is the
   number that could sink P1 and it is not in this document. `release` clones
   every binding above the deepest released one and shares the tail below,
   `frame::dispatch < Machine::step < Machine::call` is 45.5% of marginal
   allocations, and ADR 0017's own record shows a premise of exactly this shape
   moving `/health` allocations the wrong way. `./target/release/w6-alloc --repo
   . --requests 200` against 1,082 / 127,955 is the instrument, and P1 does not
   land until it has been run.

2. **If P2's release of a parameter can outrun a capture.** The argument is
   `Live::close`'s and eight adversarial programs are consistent with it, but the
   case analysis is not written and `lower_block`'s filter does not consult
   `barrier_binders`. The failure mode is `Slot::Released` →
   `INTERNAL_ERROR` on a legal program (raised at `machine.rs`, built by
   `err_released` at `:2590`, whose note reads *"reaching this is a defect in
   Ply, not in the program"*) — loud, not silent,
   which is the right direction, but it is a new way to reach a diagnostic whose
   whole point is that it is unreachable. Write the case analysis or do not land
   P2.

3. **If the §Decision 2b assertion, once armed, is failing at a rate that cannot
   be brought down.** It will fail on the tree as it stands; that is intended. If
   after P1–P4 it still fails at a large fraction of `Owned` sites, then `Live`'s
   notion of a last use is too far from what the machine can honour, and the
   right response is the fallback in §The persistent-vector fallback, not a
   weaker assertion.

4. **If the 63-site figure is the wrong denominator.** It is a grep over a
   syntactic shape (`push(cell_get(`), not a typed analysis, and it will miss
   `push(f(cell_get(c)), x)` and anything spelled differently. 171 total `push`
   occurrences is exact.

5. **If ADR 0024 is recovered and §10 or §11 says something this contradicts.**
   §Decision 1 is where to look. Everything else here is measured against the
   tree and does not depend on it.

6. **If the tree-walker's exclusion stops being acceptable.** ADR 0005 §6 lists
   four conditions under which `interp.rs` is deleted; it still ships. Every
   milestone that puts work only on the machine widens a gap `--engine both`
   cannot see, and this is one more.

7. **If `Bytes` turns out to dominate.** 85 `bytes_concat(` sites across the
   three files examined, two documented quadratics, and this ADR fixes none of
   them. If the request path is `Bytes`-bound rather than `List`-bound, the whole
   of P1–P4 is the wrong milestone and `Value::Bytes` is the right one.

## Provenance

Every number in this document was taken on this tree on 2026-08-28 by the author,
not quoted from the four proposals, and several of theirs did not reproduce. The
pre-registration is `/tmp/adr0025/prereg.md`, written before any count or
benchmark; the results are `/tmp/adr0025/results.md`.

The instrument was a temporary integration test in `crates/ply-eval/tests/`
following `reference_counting_cost.rs`'s harness: load a module and its `std`
imports, build a `Machine`, run every `test` in it, and difference `rc::stats()`.
Copy attribution added a thread-local span counter to `push`'s copying arm. P2 was
roughly ten lines in `code.rs` and `rc.rs` behind an environment flag; P3a was
about fifteen in `frame.rs`. **All of it has been reverted**; `cargo build -p
ply-eval` and `cargo fmt --all --check` are clean, and a grep for `ADR0025`,
`note_copy_span`, `ownable_now` and `COPY_SPANS` across `crates/` finds nothing.
The full workspace suite was not run, per the standing instruction that it costs
9.5–29 minutes; `cargo test -p ply-eval` was, with P2 enabled, and passes.

What did not reproduce, recorded because the disagreements are informative:

- **`sink(7, push(acc, i))` inside a recursive loop is 0% in place**, not 100%.
  One judge reported the reordered form as fast; it is fast only when the
  reordering also holds at the *enclosing* call, which is the compounding rule in
  §Context. Isolated at a single node the reordering does work — 1 of 1 against
  0 of 1 — and both readings are in this document.
- **`push(table(), 3)` for a nullary `table` measured 1 of 1 in place**, not 0.
  The memo needs `CheckOutput` armed and this harness did not arm it, so the
  claim is untested here rather than refuted, and §Soundness says so.
- **80 versus 63 for `push(cell_get(`** was a contaminated grep on my part —
  `grep --include=*.ply` under `zsh` fell through to `.rs` files. Corrected by
  driving `grep` from `find -print0`. 63 is the figure; 171 is the total.

Four proposals were prepared and judged, and this ADR is not any of them. What it
takes from each is named where it is used: `carry_released` and the standing
dynamic guard (all four, and every judging lens); the `code.rs` parameter
diagnosis and the phantom-slot presentation (**regions**); `copy` as a semantic
identity, `cell_update`, and destructure-at-last-use (**Sole**); the boundary
list from `escape.rs` and the row post-mortem that saved this document from
trying a row (**ownership-row**); "the reportable artifact is a count, not a
type", and the demonstration that `push` is the only in-place operation a Ply
program can reach (**param-marks**). The falsifier in §Decision 2b is the
**regions** proposal's soundness judge's and is the most valuable single idea
produced in the round.
