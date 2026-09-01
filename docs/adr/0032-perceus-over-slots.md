# ADR 0032 — The append cliff is a calculus mismatch: Perceus over slots, and a bounded worst case

**Status:** proposed — **a direction with a gate, and its central claim is not
measured.** §10 registers the criteria in code before the measurement, per
`CONTRIBUTING.md` §"Measure an ADR's motivating claim before accepting the ADR",
and §11 puts arming the gate ahead of every fix.
**Date:** 2026-08-31.

Amends nothing yet. Supersedes nothing. Continues
[ADR 0024](0024-ownership-as-a-checked-property.md) and
[ADR 0025](0025-ownership-design.md), whose findings it accepts entire and whose
sequencing it re-orders.

> **What this decides.** That the positional rule
> (`spikes/ply-lexer/GAPS.md` §1) is **not a language-design defect and not a
> property to be checked, warned about or annotated**. It is one implementation
> decision — that ownership is tracked at *scope* granularity over a shared `Rc`
> chain — and the fix is to give the machine the calculus Perceus is stated
> over. Paired with a representation whose worst case is bounded, the rule stops
> existing rather than becoming better documented.
>
> **What it does not decide.** Whether the change is worth its size. That is
> §10's gate and it is not yet run. Nor the surface of `fip` (§7), nor the
> chunked vector's branching factor, nor anything about `Value::Bytes`.
>
> **What it takes no measurement of, and this is the sentence to hold it to.**
> Its central claim — *slot-granular ownership removes the position dependence* —
> is **argued from the mechanism and not measured**. Every number below is either
> quoted from ADR 0024/0025 with its provenance intact, read off this tree by
> grep, or cited to published literature. ADR 0017 is what happens when a
> performance ADR is accepted on reasoning; §10 exists so this one cannot be.

---

## §1 The defect, restated as one sentence about the implementation

`crates/ply-eval/src/rc.rs`'s own module header states it and stops one step
short of the conclusion:

> Perceus is stated over a calculus whose variables are **stack slots**, so a
> last use is a *move* out of a slot and a dead binding is an explicit `drop`.
> Ply's machine has neither: a scope is a persistent `Rc` chain
> ([`Env`]) that a closure, a continuation frame and the current evaluation all
> share by pointer.
>
> | Perceus | here |
> | --- | --- |
> | `dup x` at a non-last use | the `Value` clone `Var` already does |
> | `drop x` when a binding dies | `Env::release`, at the statement whose end kills it |
> | a last use *moves* | `Env::take_unique` |
> | **there is no fourth one** | **`carry`, because a *frame* holding a scope it will not read is an owner Perceus' calculus has no name for** |

That fourth row is the field-order rule. It is not an extra mechanism Ply chose
to add; it is the artifact of running a slot calculus over a chain. Because the
unit of ownership is *everything in scope* rather than *this binding*, a frame
that will never read `s` still holds `s`, and so whether `s.toks` is at one
owner is decided by what syntactically follows it.

Three consequences follow, and all three are already in the record:

1. **The rule as documented is wrong in the safe direction, twice corrected.**
   `docs/GUIDE.md` §6.7 and the gotcha at `:2905` state the local form — last
   field of its record literal. ADR 0025 §Context measured
   `go({k: s.k + 1, out: push(s.out, i)}, i + 1)` at **0 / 200** with the field
   last, because the record is not last in the *call*. An author who learns the
   documented rule and applies it correctly still gets the quadratic.
2. **A lint over it is a partial oracle.** Built as `W0611` on PR #41 and
   refuted: it fired on `len(push([], i))`, which copies nothing, and was silent
   on `{a: s.a, b: push(s.a, i)}`, which is fully quadratic (ADR 0021 §4 item 1).
3. **A mode on the arrow is checkable-and-useless or useful-and-uncheckable.**
   ADR 0025 §Decision 1 measured it: under `resume k -> k(true) + k(false)` a
   parameter with one occurrence, its last use, free in no closure, reports
   `owners = 2`.

ADR 0025 answered (2) and (3) correctly and then treated the residue as a cost to
be *reported* (`--costs`, `W0611 COPYING_APPEND`). This ADR's disagreement is
narrow and is the whole of it: **the residue is not a cost to report, it is a
representation to replace**, and (2) and (3) both failed because they were aimed
at a language-level property that the defect does not live at.

---

## §2 What the surveyed languages do, and the line all of them hold

Recorded because "no widely used language ships this and then lints it" is the
motivating intuition and it is checkable against the field. This section is
literature and API survey, not measurement.

| | aliasing | append cost | which axis surprises you |
| --- | --- | --- | --- |
| Rust | in the type, checked, mandatory | always amortized O(1) | neither — `Rc::make_mut` / `.clone()` are typed by hand |
| Go | invisible (backing array shared) | always amortized O(1) | **semantics** — `append` can clobber an alias |
| Java / C# / JS | invisible | always amortized O(1) | semantics, and observably |
| Clojure / Haskell / Scala | irrelevant (persistent) | always ~O(log₃₂ n) | neither |
| Erlang | none (per-process heaps) | O(1) prepend | neither |
| Swift | mostly compiler-provable | **O(1) or O(n)** by refcount | **cost** — known hazard |
| Koka | invisible but *precise* | O(1) when the count is truly 1 | neither; `fip` makes it checkable |
| **Ply** | invisible **and imprecise** | **O(1) or O(n)** by *syntactic position* | **cost, asymptotically, on a non-semantic property** |

Four readings, and the third is the one that decides this ADR.

**Invisible constant factors are ubiquitous and tolerated; invisible asymptotes
are shipped by nobody.** Go's escape analysis is genuinely unpredictable and
Go's answer was a reporting flag, `-gcflags=-m` — the exact precedent for
`ply check --costs`. But escape analysis decides heap versus stack, which is a
constant. A reporting flag is proportionate to an invisible constant and is not
proportionate to an invisible complexity class. **This is the argument that
ADR 0025 §Decision 2 is a good instrument aimed at the wrong tier.**

**Every solution in the table is one of two families, and there is no third.**
Either ownership is visible and checked (Rust; Swift is adding `borrowing` /
`consuming` and non-copyable types for exactly this reason), or cost is
independent of ownership (Go, Java, Clojure, Haskell — by wildly different
means). Koka does neither and gets away with it by making the *count* precise.

**Ply cannot take Rust's route, and multi-shot is why.** Rust's ownership works
because there is exactly one continuation. Under `resume k -> k(true) + k(false)`
one syntactic occurrence legitimately yields two uses. That is not a gap in an
analysis, it is a semantic fact, and it is precisely what ADR 0025 measured.
**Ply chose effects; effects cost you Rust's answer.** It cannot take Go's
either: Go's trick is to let hidden aliasing corrupt *meaning* rather than cost,
and ADR 0017's governing property — inherited verbatim by ADR 0025 and by this
ADR — forbids that trade. What is left is Koka's, which is the one designed for a
language with handlers.

**Swift is the near-identical case and the cautionary one.** Copy-on-write arrays
with a runtime `isKnownUniquelyReferenced` check is `builtins.rs:457` with a
different spelling, and Swift has this bug class. Three things make Swift's
version milder and Ply has none of them: value semantics and exclusivity let the
compiler usually *prove* uniqueness; there are no multi-shot continuations; and
the retain happens at a **program-visible event** (storing into a property,
capturing in an escaping closure) rather than at a syntactic position inside an
expression.

**The in-house proof.** `Map` is `rpds::RedBlackTreeMap` — persistent, so
`map_insert` is O(log n) whoever holds it. It has produced no rule, no lint, no
ADR and no paragraph in the guide. `List` is `Arc<Vec<Value>>` and has produced
all four. Same language, same evaluator, same effects, same multi-shot handlers.
The only difference is that one container has a cliff.

---

## §3 Koka's mechanism, in four parts

Literature, cited rather than measured: *Perceus: Garbage Free Reference Counting
with Reuse* (PLDI 2021); *Generalized Evidence Passing for Effect Handlers*
(ICFP 2021); *FP²: Fully in-Place Functional Programming* (ICFP 2023). Anyone
implementing §6 or §7 should read the current Koka documentation rather than this
summary, which is written to be checked and not to be trusted.

**1. An ownership-passing IR.** The core is rewritten so every binding is owned by
exactly one place and consumed once along every path: at a use, `dup` if the
variable is live afterward, otherwise *move*; `drop` at the end of a branch what
was owned and not consumed; closures `dup` their free variables. Two properties
matter and both are stronger than RAII: **drops land at the last use, not at
scope end** (this is the "garbage free" claim), and **ownership is per-variable —
there is no object that owns "the scope"**. A frame holds the values it will use
and nothing else. That second property is the row `rc.rs` says the calculus has
no name for.

**2. Borrowed parameters.** Own-everything puts a `dup`/`drop` pair around every
read; borrowing marks callees that do not consume, and the traffic is not
emitted. Note what this is: it is the *weak* half of ADR 0025 §Decision 1's
dilemma — "the callee does not keep this" — the half that ADR was right to say
does not buy the append. It buys RC traffic, which is a constant factor. Koka
needs both halves and gets the append from part 3.

**3. Drop-reuse.** A pattern match does not emit `drop xs`. It emits
`let ru = drop-reuse(xs)`, which yields a **reusable memory token** when the
count was 1, or null. A later constructor of the same size becomes `Cons@ru(y,
ys)`: allocate *at* `ru` if non-null. So `map`, `filter`, `reverse` and red-black
insert over uniquely-owned data allocate **nothing** — each cell is deconstructed
and reconstructed in the same memory.

**Ply's in-place `push` is not this.** It is `Arc::get_mut` on a `Vec` — Swift's
mechanism. Perceus reuse recycles a *dying* value's memory into a *newly
constructed* one, which is strictly larger and covers a class Ply does not touch
at all: `{..s, f: e}` expands at parse time to a full field list (ADR 0023), so at
runtime it builds a fresh record and nothing recycles the dead `s`. Given ADR
0019 §R4 attributed the bulk of request-path allocation to value construction —
372.4 argument vectors a request, 40.9% — this is aimed at the measured profile
and not only at the append.

**4. Drop and reuse specialization.** `drop x` at a known constructor skips the
tag test and inlines the decrement; where `ru` is statically non-null,
`Cons@ru(y, ys)` becomes direct field writes into memory already held.

**How it survives multi-shot, which is the part that decides whether any of this
is importable.** Koka distinguishes clause forms statically:

| clause | resumes | compiled as |
| --- | --- | --- |
| `val` / `fun op(x) -> e` | exactly once, tail | a **direct call** through the evidence vector — no capture |
| `ctl op(x) -> e` | 0, 1 or many (`resume` in scope) | real continuation capture |
| `final ctl` | never | abort |

A `fun` clause captures nothing and so adds no reference. Only `ctl` builds a
continuation, and when it does it `dup`s honestly — so under
`resume k -> k(true) + k(false)` the count is genuinely 2, the reuse fails, and
**that is correct**: two futures need two lists.

So ADR 0025 §Decision 1's sentence — *"any `perform` in the enclosing dynamic
extent puts a second owner on the value"* — **is not a fact about effect
handlers. It is a fact about handlers that capture.** §8 is what follows from
that in this tree.

---

## §4 Decision 1 — the environment becomes slots

Replace `Env` — a persistent `Rc` chain of `Binding` looked up by `Symbol`
(`crates/ply-eval/src/env.rs:7-23`) — with **flat frames of slots resolved at
lowering**. Every `Var` carries a slot index; every binding carries a computed
last use; `dup` and `drop` are emitted per slot. Closures capture their **free
variables** rather than the chain (`Value::Closure(Arc<Closure>)` today holds the
whole scope, which `costs.rs` records as its third blindness).

Traced against the two rows that refuted the documented rule:

- `{toks: push(s.toks, t), pos: p}` — the projection is `s`'s last use, so it
  moves out of its slot, which is then empty. `pos: p` reads a different slot.
  The pending frame holds slots, not a scope, so it holds nothing reaching the
  list. **Position stops being observable.**
- `go({k: s.k + 1, out: push(s.out, i)}, i + 1)` — ADR 0025's row five, 0 / 200
  today. `s.k` dups, `s.out` moves, the call frame holds arg slots and `i`.
- `body_more`'s 4,100 copies, which `costs.rs` classifies `Cause::CallerKeeps` —
  a parameter is a slot like any other; if the caller passed *its* last use, the
  value arrives at count 1. **The chain composes across calls with no annotation,
  which is why Koka needs no ownership in its surface types.**

**This subsumes ADR 0025's P1, P2 and P3 rather than competing with them, and
that is the argument for doing it instead of them.** P1 (`carry` takes a dead
set) computes *the same thing* — a frame holding exactly the live bindings — but
pays `Env::release`'s O(scope-depth) chain rebuild at every sub-expression to get
it, on a path ADR 0017's census puts at 45.5% of marginal allocations. ADR 0025
flags that as the number that could sink P1, and separately concedes that P1 "is
a `Code` IR change plus lowering, not an edit to eight call sites". **If the IR is
changing anyway, change it to slots and get an O(1) answer instead of an
O(depth) one.**

**The cost is already owed for a second reason.** `Env::lookup` walking an `Rc`
chain comparing `Symbol`s is not a runtime a bootstrapped compiler can keep. ADR
0030 puts the Ply front end at 24.8× the Rust one against a 1.121× ceiling from
compiled entry, and this is the representation under all of it. Slot frames are
on the critical path for ADR 0021's goal for reasons independent of `push`.

**What does not change.** `Env::take_unique`'s dynamic guard and `Arc::get_mut`
in `push` both stay in release builds. ADR 0025's governing property is inherited
verbatim: the static analysis may be wrong, and when it is the program is slow
and never incorrect. `Own` is **not** promoted to a permission.

---

## §5 Decision 2 — the worst case becomes bounded

Even with a perfect count, the count is honestly ≥ 2 sometimes: a `ctl`
resumption, the memo table, a genuine alias. Today that costs O(n), and the
penalty **grows without bound** — ADR 0025 measured 316× at n = 4,000 and 1,596×
at n = 32,000.

`Vector<T>` becomes a **chunked persistent vector with an in-place fast path when
unique**: tail-chunk append, `Arc::get_mut` on the tail when the count is 1.
Unique appends stay a `Vec::push` into a small array; shared appends become
O(log₃₂ n).

ADR 0025 priced this against `rpds::Vector` and it failed two of four
pre-registered criteria (`(d)/(a) < 6` measured 7.56; the index criterion was
ill-posed and is now well-posed via `list_at`). **Two corrections to how that
result should be read, and neither moves the bar it failed:**

1. `rpds::Vector` does **not** do the unique-path in-place mutation; the `im` /
   `imbl` family does. So column (c)'s 1.07–1.59× is an upper bound on a cost a
   tail chunk mostly removes, and the measurement should be re-taken against a
   vector that has the fast path before the gate is applied.
2. **The gate's shape is wrong, and that is a disagreement rather than a
   re-measurement.** It reads "take the representation change if the analysis
   fails". The two are not alternatives: the analysis makes the common case free
   and the representation makes the uncommon case bounded, and no amount of the
   first ever removes the need for the second under multi-shot. §10 re-poses it
   as a property of the language rather than as a fallback: **no core operation
   may have a cost ratio that grows with n on a property the source does not
   show.**

The index cost is real and is now measurable: ADR 0027 §7 measures a peek at
~1.7 µs, almost all interpreter dispatch, so O(1) against O(log₃₂ n) is
unresolvable today and will not be after the backend lands. It is priced in §10's
G3 rather than waived.

---

## §6 Decision 3 — reuse, and the record representation it needs first

Adopt drop-reuse (§3 part 3) for constructors and record literals. **Prerequisite,
found by reading this tree rather than inferred:** `Value::Record` is
`Arc<BTreeMap<Symbol, Value>>` (`crates/ply-eval/src/value.rs:101`). Reuse
recycles cells of known size and shape; recycling a `BTreeMap` is neither easy
nor worth much.

So reuse is gated behind a **flat record representation** — a sorted field vector
with offsets resolved at lowering. Record types are structural and already
printed sorted (`docs/GUIDE.md` §5.3), so the layout is statically known wherever
the type is, and this is independently a win: a `BTreeMap` per record value is
allocation and pointer-chasing where an array index would do.

This is the largest item here and the least measured. It is sequenced last among
the evaluator changes and is **not** gated on by anything above it.

---

## §7 Decision 4 — `fip`, the checked promise ADR 0024 §5 asked for

ADR 0024 §5's surviving requirement — that the absence of reuse become visible
somewhere an author cannot miss it — is met by a **callee-side obligation**, not
by a promise about callers. A `fip fn` fails to compile if it could allocate:
owned parameters must be consumed, every construction must be matched by a
deconstruction of the same size in the same branch, and it may only call other
`fip` functions. `fbip` relaxes the constant-stack requirement; `fip(n)` permits
*n* allocations.

This escapes ADR 0025 §Decision 1's dilemma structurally: it never states
anything about the caller, so the multi-shot counterexample does not reach it.
ADR 0024 §7 already located it — *"Koka's `fip` annotations live here"*.

**Opt-in, and scoped to the standard library's hot paths.** It is not an
annotation burden on user code and §10's gate does not depend on it.

---

## §8 Decision 5 — the tail-resumptive refinement, which is cheap and is already argued in the tree

`crates/ply-eval/src/region_kind.rs`'s header records:

> **113 regions, 0 `unique`, 113 `shared`, every one of them because of a
> tail-resumptive clause** … What would move it is a rule that decided a
> continuation *cannot outlive the region* — a clause that provably resumes at
> most once and does not park its continuation — which is a linearity analysis
> this milestone does not have.

`crates/ply-eval/src/handler.rs:310-323` makes that argument, for the closely
related question of whether a tail-resumptive clause claims a pin:

> A tail-resumptive clause takes no pin … it is consumed before any of them runs,
> and it is reachable from nothing else — there is no binder for a clause body to
> store it through. … It matters: `perform` is on the request path, a pin is an
> `Rc`, and **tail-resumptive is what essentially every handler in the standard
> library is.**

> **Corrected (2026-08-31) by taking the refinement instead of estimating it,
> before this ADR was acted on further.** The sentence below read:
>
> > **These two are in tension and the tension is worth 113 regions.**
>
> The tension is real, the case analysis closes, the refinement is implemented,
> and it is worth **0 regions**. Every one of the 113 has another, independent
> reason to be `shared` — 3 a clause binding `resume`, 75 a `perform` answered
> outside the region, 15 a `simulate`, 20 an unknown callee. The 113 figure is a
> tally over the **first** cause in source order, which is all `Scan::direct_at`
> keeps, and `the_split_over_the_repositorys_own_examples` carries a comment
> saying exactly that — *"a row is an upper bound on what relaxing that one rule
> would move"* — which both `region_kind.rs`'s header and this section read as a
> lower bound.
>
> **This is §12 item 4 firing in the cheap direction and it is why S1 was
> sequenced first.** What the refinement buys is not memory: it is that the
> causes this analysis reports are the load-bearing ones, and that a
> `TailClause` in the `direct` slot can no longer *hide* the `Escapes` its own
> clause body contributes. The claim that ADR 0017 §3's "common case" fails on
> this corpus survives; its reason is now known to be the 75 escaping performs.
>
> The estimate was mine and the measurement contradicts it. Recorded here rather
> than quietly dropped, because a §2-style survey argument and a mechanism
> argument both pointed at this item and neither was a number.

**These two are in tension.** Stated as a
tension rather than as a defect, because they answer different questions —
`handler.rs` reasons about the runtime frame layout at one capture, `region_kind`
must answer statically for a whole program — and because `handler.rs` carries its
own caveat: *"Where the clause's own body performs an operation answered further
out, the capture that answers that takes the segments this frame sits in."*

So the refinement is not "tail-resumptive implies `unique`". It is
**tail-resumptive *and* the clause body performs nothing answered further out**,
both of which are decidable from the clause's syntax and its body's effect row.
`Cause::TailClause` was recorded separately so that a refinement would have
something to refine, and
`region_kind::the_split_over_the_repositorys_own_examples` is pinned to the
number it would move.

This item is **independent of §4 through §7**, is a day's work, and is sequenced
first among the fixes for that reason — which is what let its estimate be
refuted before anything expensive was built on it.

---

## §9 What this does not do

- **It does not add a parameter mode, an ownership row, or any surface
  annotation for ownership.** ADR 0025 §Decision 1 settled that on a measurement
  and this ADR does not reopen it. `fip` (§7) is not a counterexample: it is an
  obligation on a body, not a claim about a caller.
- **It does not adopt linear or uniqueness types.** They conflict head-on with
  multi-shot handlers — a linear value captured by a twice-resumed continuation
  is used twice — so adopting them means forbidding multi-shot or splitting the
  world along linearity in the effect row. Koka is the existence proof that the
  performance does not require it.
- **It does not ship another lint.** `W0611` was built and refuted.
- **It does not promote `Own` to a permission.** §4.
- **It does not make `ply check --costs` unnecessary, but it changes what it is
  for.** After §4 it reports a residue rather than a rule, which is the tier a
  reporting flag is proportionate to (§2).
- **It touches no `Bytes` quadratic.** ADR 0025 §What would make this wrong item
  7 stands unchanged and unaddressed.

---

## §10 The gate, registered before the measurement

Per `CONTRIBUTING.md` §"An ADR here is expected to state the criteria *before* the
measurement, in code where possible", modelled on
`ply_corpus::w6::Criteria::default()`. The thresholds live in code so a
measurement file cannot supply them.

**G1 — the central claim, and the only one that can kill this ADR early.**
*Position invariance.* A corpus of paired programs, each pair the same
computation written in canonical and pessimal order (growing sub-expression last,
against first, at every enclosing node — record literal, call argument, and
both). Counted with `rc::stats()`, not a clock.

- `|in_place_rate(canonical) − in_place_rate(pessimal)| ≤ 0.02` for every pair.
- `in_place_rate(canonical) ≥ 0.95` for every pair whose canonical form is linear
  today.

> **Corrected, 2026-08-31: the criterion above names an instrument that was not
> the one built, and describes a corpus that is two-fifths something else.** The
> two bars stand exactly as registered — no threshold moved, and neither was
> chosen after a measurement — and two of the words around them were wrong when
> they were written:
>
> - *"Counted with `rc::stats()`, not a clock"*. The test counts with
>   `rc::sites()`. It is the same counter — `rc.rs:266-282`'s `note_update`
>   bumps `Stats.updates` and the per-span entry from one call — so the totals
>   are the numbers `rc::stats()` would have answered, and the bars are over the
>   same quantity. What the split adds is a *site count per member*, pinned, and
>   that is the only thing it adds: see the correction under the table.
> - *"each pair the same computation written in canonical and pessimal order"*.
>   Three of the five pairs are exactly that. **Two are not**, and the test file
>   discloses each of them per pair where this criterion did not. The `let`
>   against parameter pair differs in where the accumulator *arrives from*
>   rather than in where the `push` sits, and its pessimal member introduces a
>   function, so it runs 200 call frames the canonical member does not. The
>   `fold` pair gives *both* members a two-argument helper, because in
>   `|acc, x| push(acc, x)` the append is the whole body and has no non-final
>   position to occupy. Both remain one computation written two ways at two
>   costs, which is what the bars are over; neither is "argument order" alone,
>   and a reader taking the sentence above at face value would think all five
>   were.

**This test must be red on the tree as it stands**, and armed by being shown red,
per `CONTRIBUTING.md` §"Do not state a guarantee you have not armed". Today's
figures are 200 / 200 against 0 / 200, so it will be. If §4 lands and G1 stays
red, the diagnosis in §1 is wrong and §5 is the whole of what survives.

**Built, and shown red — S0 is done.**
`crates/ply-eval/tests/position_invariance_g1.rs` carries the corpus and a
`Criteria::default()` holding the two bars above, with no path from a
measurement to either. `the_same_computation_costs_the_same_in_either_order`
fails on **5 of 5 pairs** and is `#[ignore]`d so it does not redden CI before
S4; `every_pair_is_pinned_to_what_it_costs_today` is not ignored and holds the
counts below, so movement in either direction fails rather than passing
unnoticed; `the_corpus_is_the_five_shapes_it_says_it_is` holds the corpus
itself, by count, by pairwise distinctness of the five pessimal programs and by
a BLAKE3 digest per member.

**That third test is not decoration, and the correction under the table below
is what it would have caught.** No count in this corpus identifies the program
that produced it — every canonical member reads 200 of 200 and every pessimal
one 0 of 200 — so the two bars are blind to which shapes they are taken over,
and the pin's re-pin workflow (§11 S4: re-pin, run G1 with `--ignored`, delete
`docs/GUIDE.md` §6.7 if it is green) would certify a corpus that had quietly
lost a row. The same edit closes a vacuity G1 had on its own: it asserts that a
list of failures is empty, and with `corpus()` returning nothing it reported
`ok` over nothing at all — run and watched, then guarded by a corpus-size check
both tests read.

Both bars are armed separately, and **no constant measurement passes them**:
measuring the canonical member twice turns G1 green wrongly and the pin red;
measuring the *pessimal* member twice reaches the same 0.000 gap from the other
side and leaves G1 red on the second bullet on all five pairs; and pointing one
pair's canonical member at its pessimal source leaves G1 red on that second
bullet for that pair alone. All three were run and watched, 2026-08-31. The
third is now caught by the corpus test as well — it makes a pair's two members
one program — but **the first two are held by nothing after the next edit to
that file**: no test there reads how the measurement is wired. The test file
records them as a note, and this ADR records them as history rather than as a
guarantee.

Taken on this branch at 2026-08-31 with `rc::sites()` — `rc::stats()`'s
`updates` split by span — at n = 200 per member, each member appending at
exactly one site:

| pair | canonical | pessimal | gap |
| --- | ---: | ---: | ---: |
| call argument (rows 1–2) | 200 / 200 | 0 / 200 | 1.000 |
| record field (rows 3–4) | 200 / 200 | 0 / 200 | 1.000 |
| compounding — field last, record first (rows 3, 5) | 200 / 200 | 0 / 200 | 1.000 |
| accumulator as a `let` against as a parameter (cause 1) | 200 / 200 | 0 / 200 | 1.000 |
| `fold` closure accumulator (row 6) | 200 / 200 | 0 / 200 | 1.000 |

> **Corrected, 2026-08-31: the third row reported a measurement that was not
> being taken, and the split-by-span instrument did not buy what it was said to
> buy.** Two things, and the figures above survive both:
>
> - **Row three.** The corpus member the row names —
>   `go({k: s.k + 1, out: push(s.out, i)}, i + 1)`, ADR 0025 §Context row five,
>   the shape §1 calls the finding — was written into the file as
>   `COMPOUNDING_PESSIMAL` and never referenced: the pair ran the *record field*
>   pessimal program instead, so the corpus was four distinct shapes and this
>   row was a copy of the one above it. `cargo clippy -p ply-eval --tests` said
>   so in one line (`constant COMPOUNDING_PESSIMAL is never used`) and CI runs
>   clippy with `-D warnings`, so the cheap instrument that would have caught it
>   was one command. Repaired, re-run, and the row above is now this program's
>   own measurement: **200 / 200 against 0 / 200, gap 1.000**, identical to what
>   was reported before it was taken — which is exactly why nothing noticed.
>   `the_corpus_is_the_five_shapes_it_says_it_is` is what fails now if a member
>   is replaced by another member. Rows two and three do still share a
>   *canonical* member, by construction and not by accident: the compounding
>   pair is the record-field canonical with only the enclosing call changed, so
>   the pessimal member is the whole of the difference — which is why losing it
>   cost the row everything and showed nothing.
> - **The instrument.** The withdrawn clause, verbatim: *"which is
>   `rc::stats()`'s `updates` split by span, so a member that grew a second
>   append site shows up as one instead of being folded into a total"*. The
>   first half is true of `rc::sites()`; the *consequence* was false of the test,
>   which summed every site back into one total and read nothing else off the
>   split. It is true now because the site count is pinned per member: growing
>   one member a second append site — its recursive call split over two branches
>   appending 100 times each — leaves every append count identical at 200 of 200
>   and fails the pin on the site column alone, 2 where 1 is pinned. Run and
>   watched, 2026-08-31.

These are ADR 0025 §Context's rows re-taken rather than quoted, which is the one
place this document has a number of its own — **re-taken, and two of them not
quoted programs**: the `let`-against-parameter pair and the `fold` pair
substitute programs for ADR 0025's, for the reasons in the correction above the
table and with the reasoning recorded per pair in the test file. The gap is
**1.000 against a bar of 0.02** on every pair, so nothing here is near the
criterion in either direction: G1 is not a close call today and will not become
one by accident.

**G2 — the corpus, after §4.** Over each module's own test suite, by
`reference_counting_cost.rs`'s existing harness:

- `std.http` ≥ 0.90 in place, `std.router` ≥ 0.90. (ADR 0025's own fallback bar,
  adopted deliberately so the two ADRs are comparable. Today: 0.2% and 0.5%.)
- No module regresses below its current rate.
- `./target/release/w6-alloc --repo . --requests 200` does not increase against
  the reading taken immediately before the change lands. **ADR 0017's lesson is
  that this is the number a milestone of this shape moves the wrong way, and it
  is a gate rather than a report.**

**G3 — the representation, after §5.** The property, not the ratio:

- The shared-append cost ratio against the unique one is **flat in n**: measured
  at n = 4,000 / 8,000 / 16,000 / 32,000, `max(ratio) / min(ratio) ≤ 1.5`. (Today
  this ratio is 316 → 1,596, i.e. 5.05, and unbounded above.)
- `max(ratio) ≤ 12`.
- `list_at` at n = 64,000 is within 2× of today's, **measured through the
  backend** and not through interpreter dispatch, per ADR 0027 §7's warning that
  the peek is ~1.7 µs of dispatch and will hide the term.

**G3 supersedes ADR 0025's `Vector<T>` gate**, which is conditional on P1–P4
landing first. The disagreement is §5's and is stated there.

---

## §11 Sequence

Each item is landable alone and each is gated on the one above only where said.

| | item | gate | size |
| --- | --- | --- | --- |
| **S0** | Arm G1: the criteria in code, the paired corpus, the test shown red, the corpus itself held | — | small, **done**: `crates/ply-eval/tests/position_invariance_g1.rs`, §10 |
| **S1** | §8, the tail-resumptive refinement in `region_kind` | ~~the split moves~~ — **done, and it moved 0 of 113; see §8's correction** | small |
| **S2** | Wire `ply check --costs` (ADR 0025 §2a, never built; `costs.rs` exists and is unreachable from the CLI) | §17 of the guide, `cli.rs` | small |
| **S3** | ADR 0025's P2 — a parameter may appear in a `Dead` set | its own landing condition: the case analysis ADR 0025 §What would make this wrong item 2 requires | small, **already measured**: http 0.2% → 65.7%, router 0.5% → 65.3% |
| **S4** | §4, slot frames and flat closure conversion | **G1**, then **G2** | large |
| **S5** | §5, the chunked vector | **G3** | medium |
| **S6** | §6, flat records, then reuse | G2 does not regress | large |
| **S7** | §7, `fip` | — | medium |

**S3 is kept even though S4 subsumes it**, because it is ten lines against a
measured 8,935 copies and it is the cheapest confirmation available that the
diagnosis points the right way. If S3 does not reproduce ADR 0025's figures,
stop and re-read before starting S4.

`docs/GUIDE.md` §6.7 and its §19 gotcha are **deleted, not corrected**, when G1 is
green after S4. That is the test of whether the rule is gone: a rule that still
needs stating is still there.

The re-pin that S4 asks for is a hazard as well as a step: the pin is re-taken
from the same corpus it certifies, so a corpus that had quietly shrunk would be
re-pinned shrunken and G1 would then go green over what was left. `EXPECTED_PAIRS`
and `the_corpus_is_the_five_shapes_it_says_it_is` are what stand in the way.
Re-pinning the *counts* after S4 stays one edit, which is the intended step;
dropping a shape is four, across two tests, which is the intended friction.

---

## §12 What would make this wrong

1. **If G1 stays red after S4.** Then position dependence is not scope
   granularity and §1 is a wrong diagnosis. §5 survives on its own terms; §4 does
   not.
2. **If slot frames cost more on the hot path than they save.** The change
   removes an `Rc` bump per binding read and adds a frame allocation per call
   with a known slot count. `w6-alloc` is the instrument, it is G2, and ADR 0017
   is the precedent for this exact failure.
3. **If flat closure conversion is not sound against multi-shot resumption.**
   A closure capturing free variables rather than the chain changes what a
   resumed continuation can reach. `resumption_semantics_audit.rs`,
   `resumption_snapshot_audit.rs` and `exploration_soundness.rs` are the tests
   that would say so, and none of them is currently written against a flat
   closure.
4. **If §8's refinement is unsound.** The failure mode is freeing memory a
   continuation still reaches — which is a *wrong program*, not a slow one, and
   is the one item here that is not cost-only. It does not land without the case
   analysis `region_kind.rs` says it lacks, and `region_reclamation_audit.rs`
   plus `region_meaning_adversarial.rs` are where it would show.
5. **If the tree-walker's divergence stops being acceptable.** `interp.rs` runs no
   reference counting at all, so every shape is quadratic there and none of this
   reaches it. `--engine both` compares answers and not cost, so it will stay
   silent. This ADR widens that gap further than any milestone before it.
6. **If the request path is `Bytes`-bound rather than `List`-bound.** ADR 0025
   counted 85 `bytes_concat(` sites and two documented quadratics across three
   files. If that dominates, S4–S6 are the wrong milestone and `Value::Bytes` is
   the right one. **Not measured here, and it should be measured before S4.**
7. **If the size estimate for S4 is wrong by the margin this record's estimates
   usually are.** It touches `env.rs`, `code.rs`, `frame.rs`, `machine.rs`,
   `handler.rs`, `rc.rs`, `value.rs`, the arena and region interaction, and the
   backend seam. ADR 0025 budgeted `cell_update`/`map_update` alone at 400–700
   lines; this is the size of the region track.

---

## §13 Relationship to the rest of the record

- **ADR 0024** — §1's defect, §2's refutation of the lint and §3's argument are
  accepted entire. Its §5 decision (ownership becomes a checked property carried
  in the signature) stays superseded by ADR 0025 §Decision 1. Its §7 pointer to
  `fip` is taken up in §7 here.
- **ADR 0025** — every measurement is accepted and none is re-taken. What this
  ADR changes is sequencing (P1/P2/P3 are subsumed by §4 rather than shipped as
  patches to the chain) and the shape of the `Vector<T>` gate (§5, §10 G3).
  §Decision 2's checker stays and is finally wired (S2); §Decision 4's `W0611` is
  **not built**, because after S4 there is no rule for it to warn about.
- **ADR 0021 §4 item 1** — already superseded by ADR 0024. This ADR is the third
  entry in that chain and the first that does not propose telling the author
  something.
- **ADR 0017** — its governing property is inherited verbatim (§4). Its cautionary
  role is §10's reason for existing.
- **ADR 0030** — §4 is upstream of the 24.8× front-end gap for reasons
  independent of this ADR, which is most of why it is worth its size.
- **`docs/GUIDE.md`** — §6.7 and the §19 gotcha are deleted at S4, per §11.

---

## §14 Provenance

> **Corrected: one number in it is now newly measured, and it is §10 G1's.**
> The withdrawn opening, verbatim: *"**No number in this document was newly
> measured, and that is the ADR's principal weakness.** Stated first rather than
> last."* It was true when this document was written and stopped being true when
> §11 S0 landed: §10 G1's table is
> `crates/ply-eval/tests/position_invariance_g1.rs` run on this branch,
> 2026-08-31, and it is the only measurement here that was taken rather than
> quoted. The weakness the sentence names survives it — §1's *diagnosis* is
> still argued from the mechanism and not measured, and G1 is what would measure
> it, after S4.

**Every number in this document but §10 G1's table was quoted rather than newly
measured, and that is the ADR's principal weakness.** Stated first rather than
last.

- Every figure attributed to ADR 0024 or ADR 0025 is quoted from those documents
  with their provenance intact and was **not** re-taken. That includes the
  in-place rates per module, the copy attribution table, P2's 65.7% / 65.3%, and
  the `rpds` comparison.

  > **Corrected: the position rows are no longer among them.** The withdrawn
  > list ended *"P2's 65.7% / 65.3%, the 200 / 200 against 0 / 200 position
  > rows, and the `rpds` comparison"*. §11 S0 re-took those rows on this branch
  > rather than quoting them, over a paired corpus of five shapes rather than
  > ADR 0025's six rows, and every pair reproduced 200 / 200 against 0 / 200.
  > The figures did not move; what moved is that they are now this document's
  > own and are held by a test. §10 G1 has the table.
  >
  > > **And "five shapes" was itself a claim ahead of its measurement, for one
  > > row, until 2026-08-31.** The corpus as first landed ran four distinct
  > > programs: the compounding pair's pessimal member was a duplicate of the
  > > record-field pair's, so ADR 0025 §Context row five — the row §1 calls the
  > > finding — was the one shape of the five not being measured, and this
  > > sentence and §10's third table row both reported a number for it anyway.
  > > It is measured now, it answers 200 / 200 against 0 / 200 like the rest,
  > > and §10's correction under the table has the detail. The count is held by
  > > `the_corpus_is_the_five_shapes_it_says_it_is` rather than by this
  > > sentence, which is the difference that mattered.
- Facts read off this tree by grep or by reading the file, on
  `adr/perceus-ownership` at `0ff8dfe`, 2026-08-31: `Value::Record` is
  `Arc<BTreeMap<Symbol, Value>>` (`value.rs:101`); `Value::Closure` holds
  `Arc<Closure>` (`value.rs:106`); `Env` is a persistent `Rc` chain (`env.rs:7-23`);
  `carry` is still `(env, remaining: bool)` (`rc.rs:98`); `code.rs:648` still
  seeds `cumulative` from statement binders, so **P2 has not landed**; `costs.rs`
  exists and no file under `crates/ply-cli/src/` references it, so
  `ply check --costs` **has not been built**; `W0610` is still the highest live
  warning in `ply-span`, so `W0611` **has not been taken**; `region_kind.rs`'s
  header carries the 113 / 0 / 113 split and `handler.rs:310-323` carries the
  tail-resumptive argument.
- §2's language survey is API and literature, not measurement. §3 is three
  published papers, summarised to be checked rather than trusted.
- **§0031 was taken.** `ls docs/adr/*.md | wc -l` answers 30, and
  `git ls-tree origin/feat/close-the-fragment docs/adr/` shows
  `0031-the-closed-fragment.md` on the open PR #65 — the collision
  `CONTRIBUTING.md` §"An ADR" has now recorded three times. Hence 0032.

The one thing this document adds that is not in ADR 0024 or ADR 0025 is a
*diagnosis*: that the residue those two ADRs correctly identified as
unfixable-by-annotation is fixable by representation, because the property they
were trying to state at the language level is an artifact of the machine's
environment. That diagnosis is argued in §1 and §4 and is **not measured**. §10
G1 is how it gets tested, and it is sequenced first for that reason.
