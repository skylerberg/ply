# ADR 0033 — The append cliff is a calculus mismatch: Perceus over slots, and a bounded worst case

**Status:** proposed. §4's diagnosis is confirmed on four of the five shapes
§10's gate holds and narrowed by the fifth; §5 is unmeasured; §6 and §7 are
untouched. **Date:** 2026-08-31.

Continues [ADR 0024](0024-ownership-as-a-checked-property.md) and
[ADR 0025](0025-ownership-design.md), whose findings it accepts entire and whose
sequencing it re-orders. Supersedes nothing.

> **What this decides.** That the positional rule
> (`spikes/ply-lexer/GAPS.md` §1) is **not a language-design defect and not a
> property to be checked, warned about or annotated**. It is one implementation
> decision — ownership tracked at *scope* granularity over a shared `Rc` chain —
> and the fix is to give the machine the calculus Perceus is stated over. Paired
> with a representation whose worst case is bounded, the rule stops existing
> rather than becoming better documented.
>
> **What it does not decide.** Whether the change is worth its size. That is
> §10's gate, which is armed and partly answered. Nor the surface of `fip`, nor
> the chunked vector's branching factor, nor anything about `Value::Bytes`.

---

## §1 The defect, which is one row of a table `rc.rs` already prints

`crates/ply-eval/src/rc.rs`'s module header maps Perceus' operations onto Ply's
and has a fourth row the calculus does not: `carry`, *"because a frame holding a
scope it will not read is an owner Perceus' calculus has no name for"*.

That row is the field-order rule. Perceus is stated over stack slots; Ply runs it
over a persistent `Rc` chain that a closure, a continuation frame and the current
evaluation share by pointer. Because the unit of ownership is *everything in
scope* rather than *this binding*, a frame that will never read `s` still holds
`s`, and whether `s.toks` is at one owner is decided by what syntactically
follows it.

Three consequences, all already in the record:

1. **The rule as documented is too weak.** `docs/GUIDE.md` §6.7 stated it
   locally — last field of its record literal. ADR 0025 §Context measured a
   program that obeys that and is quadratic anyway, because the literal is not
   last in the *call*. An author who learned the rule and applied it correctly
   still got the quadratic.
2. **A lint over it is a partial oracle.** Built as `W0611` on PR #41 and
   refuted: it fired on a `push` that copies nothing and was silent on one that
   is fully quadratic (ADR 0021 §4 item 1).
3. **A mode on the arrow is checkable-and-useless or useful-and-uncheckable.**
   ADR 0025 §Decision 1 measured it: under a multi-shot resumption a parameter
   with one occurrence, its last use, free in no closure, has two owners.

ADR 0025 answered (2) and (3) correctly and then treated the residue as a cost to
be *reported*. This ADR's disagreement is narrow and is the whole of it: **the
residue is not a cost to report, it is a representation to replace**, and (2) and
(3) failed because they aimed at a language-level property the defect does not
live at.

---

## §2 What the surveyed languages do, and the line all of them hold

Survey, not measurement — API and literature.

| | aliasing | append cost | which axis surprises you |
| --- | --- | --- | --- |
| Rust | in the type, checked, mandatory | always amortized O(1) | neither — `Rc::make_mut` / `.clone()` are typed by hand |
| Go | invisible | always amortized O(1) | **semantics** — `append` can clobber an alias |
| Java / C# / JS | invisible | always amortized O(1) | semantics, and observably |
| Clojure / Haskell / Scala | irrelevant (persistent) | always ~O(log n) | neither |
| Erlang | none (per-process heaps) | O(1) prepend | neither |
| Swift | mostly provable | **O(1) or O(n)** | cost — a known hazard |
| Koka | invisible but *precise* | O(1) when the count is truly 1 | neither; `fip` makes it checkable |
| **Ply** | invisible **and imprecise** | **O(1) or O(n)** | **cost, asymptotically, on syntactic position** |

Four readings, and the third decides this ADR.

**Invisible constant factors are ubiquitous and tolerated; invisible asymptotes
are shipped by nobody.** Go's escape analysis is genuinely unpredictable and
Go's answer was a reporting flag, `-gcflags=-m` — the precedent for
`ply check --costs`. But escape analysis decides heap versus stack, a constant. A
reporting flag is proportionate to an invisible constant and is not proportionate
to an invisible complexity class. **That is why ADR 0025 §Decision 2 is a good
instrument aimed at the wrong tier.**

**Every solution above is one of two families and there is no third.** Either
ownership is visible and checked (Rust; Swift is adding `borrowing`/`consuming`
and non-copyable types for exactly this), or cost is independent of ownership
(Go, Java, Clojure, Haskell, by very different means). Koka does neither and gets
away with it by making the *count* precise.

**Ply cannot take Rust's route, and multi-shot is why.** Rust's ownership works
because there is exactly one continuation. Under `resume k -> k(true) + k(false)`
one syntactic occurrence legitimately yields two uses — a semantic fact, and what
ADR 0025 measured. **Ply chose effects; effects cost you Rust's answer.** It
cannot take Go's either: Go lets hidden aliasing corrupt *meaning* rather than
cost, and ADR 0017's governing property forbids that trade. Koka's is what is
left, and it is the one designed for a language with handlers.

**The in-house proof.** `Map` is `rpds::RedBlackTreeMap` — persistent, so an
insert costs the same whoever holds it. It has produced no rule, no lint, no ADR
and no paragraph in the guide. `List` is `Arc<Vec<Value>>` and has produced all
four. Same language, same evaluator, same effects, same multi-shot handlers. The
only difference is that one container has a cliff.

---

## §3 Koka's mechanism, in four parts

Literature: *Perceus* (PLDI 2021), *Generalized Evidence Passing for Effect
Handlers* (ICFP 2021), *FP²* (ICFP 2023). Anyone implementing §6 or §7 should
read the current Koka documentation rather than this summary, which is written to
be checked and not trusted.

**1. An ownership-passing IR.** Every binding is owned by exactly one place and
consumed once along every path: `dup` at a use with a later read, otherwise a
move; `drop` at the end of a branch for what was owned and not consumed; closures
`dup` their free variables. Two properties, both stronger than RAII: **drops land
at the last use, not at scope end**, and **ownership is per-variable — nothing
owns "the scope"**. The second is the row `rc.rs` has no name for.

**2. Borrowed parameters.** Own-everything puts a `dup`/`drop` pair around every
read; borrowing marks callees that do not consume. This is the *weak* half of ADR
0025 §Decision 1's dilemma — "the callee does not keep this" — the half that ADR
was right to say does not buy the append. It buys RC traffic, a constant factor.
Koka needs both halves and gets the append from part 3.

**3. Drop-reuse.** A pattern match emits `drop-reuse`, yielding a **reusable
memory token** when the count was one. A later constructor of the same size
allocates *at* that token. So `map`, `filter`, `reverse` and red-black insert over
uniquely-owned data allocate nothing.

**Ply's in-place `push` is not this.** It is `Arc::get_mut` on a `Vec` — Swift's
mechanism. Perceus reuse recycles a *dying* value's memory into a *newly
constructed* one, which covers a class Ply does not touch at all: `{..s, f: e}`
expands at parse time to a full field list (ADR 0023), so at runtime it builds a
fresh record and nothing recycles the dead `s`. ADR 0019 §R4 attributes the bulk
of request-path allocation to value construction, so this is aimed at the
measured profile and not only at the append.

**4. Drop and reuse specialization.** `drop` at a known constructor skips the tag
test; a statically non-null token makes a constructor into direct field writes
into memory already held.

**How it survives multi-shot**, which decides whether any of this is importable.
Koka distinguishes clause forms statically: a tail-resumptive `fun` clause is a
**direct call through the evidence vector with no capture**; a `ctl` clause
captures for real and `dup`s honestly, so under `resume k -> k(true) + k(false)`
the count is genuinely two and the copy is *correct* — two futures need two
lists.

So ADR 0025 §Decision 1's sentence — *any `perform` in the enclosing dynamic
extent puts a second owner on the value* — **is not a fact about effect handlers.
It is a fact about handlers that capture.** §8 is what follows in this tree.

---

## §4 Decision 1 — the environment becomes slots

Replace `Env` — a persistent `Rc` chain looked up by `Symbol` — with **flat
frames of slots resolved at lowering**: every `Var` a slot index, every binding a
computed last use, `dup`/`drop` per slot. Closures capture their **free
variables** rather than the chain (`Value::Closure` holds the whole scope today,
which `costs.rs` records as its third blindness).

Traced against the shapes that refuted the documented rule: a projection that is
its binding's last use *moves* out of its slot, which is then empty, so a later
sibling sub-expression reads a different slot and the pending frame holds nothing
that reaches the list. A parameter is a slot like any other, so if the caller
passed *its* last use the value arrives at one owner — the chain composes across
calls with no annotation, which is why Koka needs no ownership in its surface
types.

**Measured, by §11's S4 probe rather than argued.** The probe is ADR 0025's P1 at
the `App`-argument and `Record`-field carry sites, behind `PLY_ADR0033_PROBE=1`,
at `rc::carry_released`: the frame carries the scope *minus* what the
sub-expression just started is the last reader of. Armed, **four of §10 G1's five
pairs go to a gap of 0.000**, including ADR 0025 §Context row five — the case
that refuted the documented rule.

**The fifth does not move, and the reason narrows this section's claim.** Its
pessimal spelling is `{out: push(s.out, i), k: s.k + 1}`, and field 1 genuinely
reads `s`. No release keyed by a *name* can free `s.out` there, because `s` is not
dead — only `s.out` is. That is **path-granular liveness**, the general form of
ADR 0025's P3, which that ADR records the regions proposal as having declined to
build because a wrong answer there is a wrong program rather than a slow one.

So: one slot per binding removes position dependence wherever the enclosing
node's other sub-expressions do not read the same binding. **Where they read a
different *field* of it, the slot has to be finer than the binding** — which puts
§6's flat record representation upstream of finishing §4 rather than after it,
and §11 is sequenced accordingly.

**This subsumes ADR 0025's P1, P2 and P3 rather than competing with them.** P1
computes what slot frames compute — a frame holding exactly the live bindings —
but pays `Env::release`'s O(scope-depth) chain rebuild at every sub-expression to
get it, on the path ADR 0017's census puts at the largest share of marginal
allocations. ADR 0025 separately concedes that P1 is a `Code` IR change plus
lowering rather than an edit to eight call sites. **If the IR is changing anyway,
change it to slots and get an O(1) answer instead of an O(depth) one.**

**The cost is owed for a second reason.** `Env::lookup` walking an `Rc` chain
comparing `Symbol`s is not a runtime a bootstrapped compiler can keep, and ADR
0030 puts the Ply front end far enough behind the Rust one that the representation
under it is on ADR 0021's critical path for reasons independent of `push`.

**What does not change.** `Env::take_unique`'s dynamic guard and `Arc::get_mut` in
`push` both stay in release builds. ADR 0025's governing property is inherited
verbatim: the static analysis may be wrong, and when it is the program is slow and
never incorrect. `Own` is **not** promoted to a permission.

---

## §5 Decision 2 — the worst case becomes bounded

Even with a perfect count, the count is honestly ≥ 2 sometimes: a `ctl`
resumption, the memo table, a real alias. Today that costs O(n), and the penalty
against the good case **grows without bound** with n (ADR 0025 §The
persistent-vector fallback has the ratios).

`Vector<T>` becomes a **chunked persistent vector with an in-place fast path when
unique**: tail-chunk append, `Arc::get_mut` on the tail at one owner. Unique
appends stay a `Vec::push` into a small array; shared appends become O(log₃₂ n).

ADR 0025 priced this against `rpds::Vector` and it failed two of four
pre-registered criteria. **Two corrections to how that result should be read,
neither of which moves the bar it failed:**

1. `rpds::Vector` does **not** do the unique-path in-place mutation; the `im` /
   `imbl` family does. So its unique-path overhead is an upper bound on a cost a
   tail chunk mostly removes, and the measurement should be re-taken against a
   vector that has the fast path before the gate is applied.
2. **The gate's shape is wrong**, and this is a disagreement rather than a
   re-measurement. It reads "take the representation change if the analysis
   fails". They are not alternatives: the analysis makes the common case free and
   the representation makes the uncommon case bounded, and no amount of the first
   removes the need for the second under multi-shot. §10 G3 re-poses it as a
   property of the language: **no core operation may have a cost ratio that grows
   with n on a property the source does not show.**

The index cost is real and is now measurable via `list_at`. ADR 0027 §7 warns that
a peek is almost all interpreter dispatch, so it must be priced through the
backend or not at all; G3 says so rather than waiving it.

---

## §6 Decision 3 — reuse, and the record representation it needs first

Adopt drop-reuse (§3 part 3) for constructors and record literals.
**Prerequisite:** `Value::Record` is `Arc<BTreeMap<Symbol, Value>>`. Reuse
recycles cells of known size and shape; recycling a `BTreeMap` is neither easy nor
worth much.

So reuse is gated behind a **flat record representation** — a sorted field vector
with offsets resolved at lowering. Record types are structural and already printed
sorted, so the layout is statically known wherever the type is, and this is
independently a win: a `BTreeMap` per record value is allocation and pointer
chasing where an array index would do.

**§4's fifth G1 pair lands here too.** Field-granular liveness needs a field to be
addressable, which is the same prerequisite.

---

## §7 Decision 4 — `fip`, the checked promise ADR 0024 §5 asked for

ADR 0024 §5's surviving requirement — that the absence of reuse become visible
where an author cannot miss it — is met by a **callee-side obligation**, not a
promise about callers. A `fip fn` fails to compile if it could allocate: owned
parameters must be consumed, every construction matched by a deconstruction of
the same size in the same branch, and it may only call other `fip` functions.

This escapes ADR 0025 §Decision 1's dilemma structurally: it never states anything
about the caller, so the multi-shot counterexample does not reach it. ADR 0024 §7
already located it. **Opt-in and scoped to the standard library's hot paths**; it
is not an annotation burden on user code and §10's gate does not depend on it.

---

## §8 Decision 5 — a tail-resumptive clause is not a capture that outlives a region

**Done.** `region_kind` called every region holding a tail-resumptive clause
`shared`. ADR 0005 §1.3's two rules are why it need not: a general clause
evaluates its body under an environment binding `κ ↦ Continuation(k)`, so `k` is a
value the body can store, close over or return; a tail-resumptive clause binds no
`κ` and `k` goes into the stack as `K′·Resume(k)` — a frame above the region's
close, consumed before it, reachable from no binder. `region_kind`'s header
carries the case analysis and `region_meaning_adversarial` runs the shape on both
engines with the clause writing the cell of the region it is now `unique` over.

**It is worth zero regions on this corpus, and that is the finding.** Every region
the corpus opens has another, independent reason to be `shared`. The estimate that
said otherwise came from reading a first-cause tally as a lower bound:
`Scan::direct_at` keeps only the first cause in source order, so **a row of that
tally is an upper bound on what relaxing that one rule would move**, which is the
kind of error a re-take cannot catch because every figure in it stands.

What the refinement does buy is that the causes the analysis reports are the
load-bearing ones, and that a `TailClause` in the `direct` slot can no longer
*hide* the `Escapes` its own clause body contributes — which is the one thing that
makes such a clause unsafe. **This item was sequenced first because it is cheap,
and being cheap is what let its estimate be refuted before anything expensive was
built on it.**

---

## §9 What this does not do

- **No parameter mode, ownership row, or surface annotation for ownership.** ADR
  0025 §Decision 1 settled that on a measurement. `fip` (§7) is not a
  counterexample: it is an obligation on a body, not a claim about a caller.
- **No linear or uniqueness types.** They conflict with multi-shot handlers — a
  linear value captured by a twice-resumed continuation is used twice — so
  adopting them means forbidding multi-shot or splitting the world along
  linearity in the effect row. Koka is the existence proof that the performance
  does not require it.
- **No second lint.** `W0611` was built and refuted.
- **`Own` is not promoted to a permission.**
- **`ply check --costs` is not made unnecessary** — but after §4 it reports a
  residue rather than a rule, which is the tier a reporting flag fits.
- **No `Bytes` quadratic is touched.** ADR 0025's item 7 stands unaddressed.

---

## §10 The gate, registered before the measurement

Registered in code so a measurement file cannot supply a threshold, per
`CONTRIBUTING.md` §"Measure an ADR's motivating claim before accepting the ADR".

**G1 — the central claim, and the only one that can kill this ADR early.**
*Position invariance*, in `crates/ply-eval/tests/suite/position_invariance_g1.rs`. Five
paired programs, each pair computing the same value with the growing
sub-expression last and not-last; counted with `rc::sites()`, not a clock.
`Criteria::default()` holds both bars: a per-pair gap of at most 0.02, and a
canonical rate of at least 0.95 where the canonical form is linear today. Red on
the shipped evaluator, `#[ignore]`d so it does not redden CI before §11 S4, and
armed by having been seen red. `every_pair_is_pinned_to_what_it_costs_today` pins
today's numbers, and `the_corpus_is_the_five_shapes_it_says_it_is` stops the
corpus being narrowed — it exists because a member was once written into the file
and never referenced, leaving four shapes reported as five.

**Answered so far: four of five pairs meet it under the S4 probe, one does not.**
G1 stays armed and red. What it has already done is separate a confirmed claim
from an over-broad one before the region-track-sized rewrite was started.

**G2 — the corpus, after §4.** Over each module's own test suite, by
`reference_counting_cost.rs`'s harness: `std.http` and `std.router` at or above
0.90 in place, ADR 0025's own fallback bar adopted deliberately so the two ADRs
are comparable; no module regresses; and **`w6_report_allocations` does not
increase**. ADR 0017's lesson is that this is the number a milestone of this shape
moves the wrong way, so it is a gate rather than a report.

**G3 — the representation, after §5.** The property, not the ratio: the
shared-append cost ratio against the unique one is **flat in n** across at least
two doublings, and bounded; and `list_at` stays within 2× of today's **measured
through the backend**, per ADR 0027 §7's warning that dispatch will otherwise hide
the term. This supersedes ADR 0025's `Vector<T>` gate, for §5's reason.

---

## §11 Sequence

| | item | state |
| --- | --- | --- |
| **S0** | arm G1 | done — red on five shapes, non-vacuity shown by mutation |
| **S1** | §8, the tail-resumptive refinement | done — sound, and worth zero regions |
| **S2** | wire `ply check --costs` (ADR 0025 §2a, never built) | done |
| **S3** | ADR 0025's P2 — a parameter may appear in a `Dead` set | done — G1's first pair to gap 0.000 |
| **S4′** | §4's probe — P1 at the `App` and `Record` carry sites, behind `PLY_ADR0033_PROBE=1` | done — four of five pairs to 0.000; §4 has the narrowed claim |
| **S6′** | §6's flat record representation, which the fifth pair needs | **next**, and ahead of S4 rather than behind it |
| **S4** | §4, slot frames and flat closure conversion | gated on **G1**, then **G2** |
| **S5** | §5, the chunked vector | gated on **G3** |
| **S6** | §6, reuse | G2 does not regress |
| **S7** | §7, `fip` | — |

`docs/GUIDE.md` §6.7 and its §19 gotcha are **deleted, not corrected**, when G1 is
green. That is the test of whether the rule is gone: a rule that still needs
stating is still there.

---

## §12 What would make this wrong

1. **If the four confirmed pairs regress under real slot frames**, or if the fifth
   proves unreachable without the path-granular analysis ADR 0025 declined on
   soundness grounds. Then §1's diagnosis does not carry the whole rule and §5 is
   what survives.
2. **If slot frames cost more on the hot path than they save.** The change removes
   an `Rc` bump per binding read and adds a frame allocation per call with a known
   slot count. `w6_report_allocations` is the instrument, it is G2, and ADR 0017
   is the precedent for this exact failure. **The S4 probe is off by default for
   this reason** — `Env::release` is O(scope depth) on the machine's hottest path
   and that number has not been taken.
3. **If flat closure conversion is unsound against multi-shot resumption.** A
   closure capturing free variables rather than the chain changes what a resumed
   continuation can reach. `resumption_semantics_audit`,
   `resumption_snapshot_audit` and `exploration_soundness` are where it would
   show, and none is currently written against a flat closure.
4. **If §8's refinement is unsound.** The failure mode is freeing memory a
   continuation still reaches — a wrong program, not a slow one, and the only item
   here that is not cost-only. `region_reclamation_audit` and
   `region_meaning_adversarial` are the guards.
5. **If the tree-walker's divergence stops being acceptable.** `interp.rs` runs no
   reference counting, so none of this reaches it and `--engine both` compares
   answers rather than cost. This ADR widens that gap further than any milestone
   before it.
6. **If the request path is `Bytes`-bound rather than `List`-bound.** ADR 0025
   counted the `bytes_concat` sites and two documented quadratics. If that
   dominates, S4–S6 are the wrong milestone and `Value::Bytes` is the right one.
   **Not measured, and it should be measured before S4.**
7. **If S4's size estimate is wrong by the margin this record's estimates usually
   are.** It touches `env.rs`, `code.rs`, `frame.rs`, `machine.rs`, `handler.rs`,
   `rc.rs`, `value.rs`, the arena and region interaction, and the backend seam.

---

## §13 Relationship to the rest of the record

- **ADR 0024** — §1's defect, §2's refutation of the lint and §3's argument are
  accepted entire. Its §5 stays superseded by ADR 0025 §Decision 1. Its §7 pointer
  to `fip` is taken up in §7 here.
- **ADR 0025** — every measurement is accepted and none is re-taken. What changes
  is sequencing (P1/P2/P3 subsumed by §4 rather than shipped as patches to the
  chain) and the shape of the `Vector<T>` gate. §Decision 2's checker stays and is
  finally wired; §Decision 4's `W0611` is **not built**, because after §4 there is
  no rule for it to warn about.
- **ADR 0021 §4 item 1** — already superseded by ADR 0024. This is the third entry
  in that chain and the first that does not propose telling the author something.
- **ADR 0017** — its governing property is inherited verbatim; its cautionary role
  is why §10 exists.
- **ADR 0030** — §4 is upstream of the front-end gap for reasons independent of
  this ADR, which is most of why it is worth its size.

---

## §14 Provenance

Every figure attributed to ADR 0024 or ADR 0025 is quoted from those documents and
was **not** re-taken. What was measured here, on this branch: G1's five pairs,
with and without the S4 probe; the module in-place rates before and after S3,
against ADR 0025's P2 figures; and the `region_kind` split before and after S1.
All of it is in the tests named above rather than only here, which is the point of
arming it.

**§2's survey is literature and API, not measurement. §3 is three published
papers, summarised to be checked rather than trusted.** §5, §6 and §7 carry no
measurement of their own, and §6's `BTreeMap` prerequisite is the only claim in
them read off this tree.

`0031` was taken by an open pull request when this was numbered.
