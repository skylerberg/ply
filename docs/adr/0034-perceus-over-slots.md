# ADR 0034 — The append cliff is a calculus mismatch: Perceus over slots, and a bounded worst case

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
the `App`-argument and `Record`-field carry sites, behind `PLY_ADR0034_PROBE`, which
takes `params` for S3's half, `carry` for this one and `1` for both: the frame carries the scope *minus* what the
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

**This subsumes ADR 0025's P1, P2 and P3 rather than competing with them, and the
size of the constant that separates them is now measured rather than asserted.**
P1 computes what slot frames compute — a frame holding exactly the live bindings
— and both of the primitives ADR 0025 offered for it **double the allocations on
the request path**, against a baseline of 773 per `/health`:

| how the frame narrows its scope | allocations | over baseline |
| --- | ---: | ---: |
| `Env::release(dead)`, rebuilding down from the head | 1,554 | +101% |
| `Env::keep_only(live)`, building up from empty | 1,451 | +88% |
| parameter releases at statement level, alone | 808 | +4.6% |

Neither is close, and the reason is the same for both: **the cost is inherent to
building a narrowed scope at runtime out of a persistent chain.** `release` pays
one link per binding above the deepest it drops, which for a parameter is the
whole scope. `keep_only` pays one per live name — and the live set that is *safe*
to use includes everything read after the call as well, because a name read both
later and by a remaining argument is still the frame's to hold. Narrowing it to
just the remaining arguments' reads is what turns a legal program into
`cannot find` at the read, which is how that version was found.

That is the argument for slot frames stated as a measurement: a frame that holds
slot indices does not *construct* anything, so "carrying less" is a fact settled
at lowering rather than a chain built per call. There is no cheaper approximation
left to try — these were the two.

**Narrowing only where it can pay is 15× cheaper and still not free.** §11 S4's probe narrowed at
every carry site, which is why it cost what it did. Narrowing only where the sub-expression being
started actually appends, on `/health` against a 742 baseline:

| what decides whether the frame narrows | allocations | G1 pairs met |
| --- | ---: | ---: |
| every carry site | 1,451 (+88%) | 3 of 5 |
| **only where the sub-expression appends** | **785 (+5.8%)** | **3 of 5** |
| only where the appended list is a bare name absent from the live set | 785 (+5.4%) | 2 of 5 |
| a precise per-argument live set instead of `Live`'s | 802 (+7.5%) | 2 of 5 |

Same benefit for a fifteenth of the cost, and the two attempts to sharpen it further both bought
less. The third fails because `push(s.out, i)` — the shape the compounding case is written in — has
no bare name to test; the fourth because a syntactic free-variable set ignores binders and so keeps
more than `Live` does, not less.

**It still fails G2**, and that is the point worth carrying forward: even narrowing *only* where it
can possibly pay, building a window per site costs 5.8% of the request path. The window has to be
free rather than cheap, which is the next block.

**And the frame's narrowed view has to be inline, which is what forces the rest of
the rewrite.** Modelled over a scope of depth *d*, allocations per carry:

| what the frame carries | d = 4 | 8 | 16 | 32 |
| --- | ---: | ---: | ---: | ---: |
| the chain, cloned — today | 0 | 0 | 0 | 0 |
| the chain minus one binding — P1 | 4 | 9 | 18 | 35 |
| a heap window of the live values | 1 | 1 | 1 | 1 |
| **an inline window of the live values** | **0** | **0** | **0** | **0** |

Today's carry is *free* — one refcount bump — so every scheme that narrows the
view is a cost on the common path, and only the inline one is free again. That
settles the shape: the window lives in the frame, not behind a pointer.

**It also settles that S4 has no cheap first step.** An inline window is only
usable if the sub-expressions that read it resolve their names to *indices into
it*, because a window that has to answer lookups by name is an `Env`, and
building one is the row above. Indexed lookup is the slot rewrite. Pooling does
not rescue the intermediate design either: a carried scope lives as long as its
frame, so deep recursion holds every link at once and outruns the free list,
which is why `keep_only` measured what it did. **The rewrite is the unit of
work.** ADR 0025 separately concedes that P1 is a `Code` IR change plus
lowering rather than an edit to eight call sites. **If the IR is changing anyway,
change it to slots and get an O(1) answer instead of an O(depth) one.**

**The trade the rewrite makes, counted rather than assumed.** A persistent chain makes *capture*
cheap — a continuation shares a pointer — and a slot stack makes *carrying* cheap, because the frame
records a base index instead of building anything. The second is only worth having if programs carry
far more often than they capture, and that is a property of real programs. Over `examples/`:

| | |
| --- | ---: |
| carries | 2,012 |
| captures | 162 |
| frames those captures took | 562 |

**12.4 carries per capture**, and the average capture is 3.5 frames deep. So the rewrite buys the
reuse S3 measured — 10,461 appends that stop copying — for on the order of 562 frame-window copies
on the same corpus. `slot_resolution::the_corpus_carries_far_more_often_than_it_captures` is the
count and fails if the ratio ever inverts, which is the condition under which §4 stops being worth
its size.

**The shape the measurements leave standing, stated so the rewrite has a target.** With a slot
stack, "the frame carries less" is not a construction at all: the slots stay where they are and the
carry *clears* the dead ones. That is O(dead) writes and no allocation, which is the only row of
§4's table that matches today's free clone. Everything else follows from it — a closure copies its
free variables out (done), a continuation copies the window it captured, and a `Var` reads an index.

`slots.rs` is the half of that which can be checked before any of it runs: a forward pass assigning
every binding an index and resolving every occurrence to one, with the runtime still answering by
name. A wrong slot costs nothing today and would cost a wrong value the moment the machine reads by
index, which is a check that cannot be written afterwards.

**The cost is owed for a second reason.** `Env::lookup` walking an `Rc` chain
comparing `Symbol`s is not a runtime a bootstrapped compiler can keep, and ADR
0030 puts the Ply front end far enough behind the Rust one that the representation
under it is on ADR 0021's critical path for reasons independent of `push`.

**What does not change.** `Env::take_unique`'s dynamic guard and `Arc::get_mut` in
`push` both stay in release builds. ADR 0025's governing property is inherited
verbatim: the static analysis may be wrong, and when it is the program is slow and
never incorrect. `Own` is **not** promoted to a permission.

### §4.1 What the machine has to become, since every alternative to it is now priced

Nine measurements narrow this to one design, and it is **not** the "frame carries a narrowed scope"
that §4 and its probes kept reaching for. That framing is what made every attempt cost something:
`release` rebuilds (+101%), `keep_only` re-collects (+88%), a heap window allocates once per carry,
and narrowing only where it can pay still costs +5.8%. A window is an allocation however rarely it
is built.

**The frame should not narrow anything. A last use should move the value out of its slot.**

That is Perceus' rule, and `Env::take_unique` is already the attempt at it — it fails today for a
reason that is now precisely stated: it "refuses at the first shared link", and a pending frame has
cloned the chain's head, so every link is shared and it refuses every time. The chain is what makes
sharing all-or-nothing.

With slots the same rule is a write:

1. **The machine owns one slot stack.** `Machine` holds `Vec<Option<Value>>`; an activation is a
   window into it. Nothing per-scope is allocated and nothing is reference counted.
2. **A frame records a base index, not a scope.** The `Env` threaded through fourteen `Frame`
   variants in `cont.rs` becomes `(base, len)`. Carrying is then free, and *narrowing disappears as
   an operation* rather than becoming cheap.
3. **A read marked [`crate::rc::Own::Owned`] takes the value out of its slot**, leaving it empty.
   `slots.rs` has already resolved which slot, verified over 12 shipped modules and 1,682
   occurrences — though see below for why that assignment is not yet an *address*. The pending frame sees the empty slot too, which is correct: `Owned` is exactly the
   claim that nothing after this point reads the binding.
4. **A closure copies its free variables** out of the window. Done — §11 S4a.
5. **A capture copies the windows it took**, and each resumption restores them. This is the cost
   the design pays, and §10.1's census is what says it is affordable: 12.4 carries per capture, and
   the average capture is 3.5 frames deep.

**Step 2 needs more than `slots.rs` currently gives it, and this is the first thing to fix.** A
handler clause's body is a barrier to `lower_barrier` — it opens a fresh `ownable`, so only its own
binders are ownable inside it — but at runtime `handler.rs` builds its scope as
`prompt.env.clone()` *extended* with the clause's parameters. The same is true of a `return` clause.
So a clause body's slot 0 is its first parameter, while the scope it actually runs in has the whole
prompt environment underneath. **A flat per-barrier index is therefore not a runtime address**, even
though `slots.rs` verifies it is internally consistent — the two passes agree with each other and
disagree with the machine.

Two ways out: give every occurrence a `(depth, index)`, or make a clause body a real frame that
copies in the free variables it needs, exactly as a closure does. **The second is built** — §11 S4c,
behind the probe — and it keeps addressing flat. Armed it costs `/health` about 2 allocations a
request and takes reuse over `examples/` from 67.3% to 68.6%; the soundness suites pass with it on,
which is the part that mattered, because a clause body that loses a name it reads is `cannot find`
at the read rather than a slow program.

It is behind the probe rather than on because G2 is a gate and +0.7% is an increase. What it removes
is the structural obstacle: with it armed a clause body's scope is its own free variables plus its
parameters, which a flat slot index can address.

**What makes step 3 work is step 1**, and only step 1: a value can be moved out of a slot exactly
when the slot array has one owner, and the array has one owner exactly when it is the machine's
rather than each frame's. That is the whole content of the rewrite, and it is why the pieces do not
land separately — steps 1 and 2 are inert without 3, and 3 is unsound without 1.

**What it must not break, and where that would show.** `resumption_semantics_audit` and
`exploration_soundness` hold the multi-shot rules: ADR 0005 §3 threads *state* across resumptions
while each resumption re-enters with the scope it captured. A persistent chain gets that for free; a
mutable stack gets it only if step 5 is right about which of the two a slot is. That boundary is the
single most likely place for this to be wrong, and it is why it wants its own change with those
suites run first rather than last.

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

1. The measurement was re-taken against both families, in §10.1. `imbl` is the
   candidate: `rpds` allocates once per append, which G2 refuses. Its time ratio
   is the better of the two and that is not the binding criterion.
2. **The gate's shape is wrong**, and this is a disagreement rather than a
   re-measurement. It reads "take the representation change if the analysis
   fails". They are not alternatives: the analysis makes the common case free and
   the representation makes the uncommon case bounded, and no amount of the first
   removes the need for the second under multi-shot. §10 G3 re-poses it as a
   property of the language: **no core operation may have a cost ratio that grows
   with n on a property the source does not show.**

**The instrument S5 needs is now in the tree.** `rc::Stats::updates_in_place` answers "did this
append copy the whole list", which is the right question only while a copy is all-or-nothing: a
chunked append that cannot rewrite copies a path, so the boolean would read `false` for something
costing O(log n) and the rate would look uniformly bad while the program got faster. Seven armed
assertions read that boolean, and S5 would have made every one of them vacuous.
`rc::Stats::elements_copied` counts what was actually copied, which is the question that survives
the representation — 190 elements for twenty whole-list appends, pinned in
`slot_resolution::a_copying_append_reports_how_much_it_copied`. Under `imbl` the same program
reports a number that shrinks rather than a boolean that stops meaning anything.

**And that is not sufficient, which is worth stating precisely because it looked sufficient.**
`elements_copied` is computable today because `push`'s copying arm knows the length it copied.
`imbl::Vector` exposes no such thing: its only sharing-related method is `ptr_eq`, which compares
two vectors rather than asking whether one is uniquely owned, so an append cannot report either
whether it rewrote or how much it moved. Under S5 the seven assertions reading those counters are
not made *vacuous*, they are made **unmeasurable**, and the difference matters — a vacuous assertion
can be re-pointed at the same quantity, an unmeasurable one has to be re-pointed at a different one.

What survives the swap is what the allocator saw — and in **bytes**, not in allocation count. A
whole-list copy is one `Vec::with_capacity`, so a quadratic accumulator makes the same O(n)
allocations a linear one does while moving O(n²) bytes doing it: counting allocations separates the
two by 2.29× against 1.46×, which is not a shape. Counting bytes separates them by **3.99× against
1.37×** per doubling — the 4×-against-2× signature — and at n = 2,000 by 64 MB against 412 KB.
`accumulator_shape::a_quadratic_accumulator_grows_faster_than_a_linear_one` is that test, and it
asks a question `imbl` can answer: a chunked append that copies a path instead of an array moves the
quadratic ratio toward the linear one, which is visible where a boolean is not.

**That is one site of forty-five, and counting them changes what S5 is.** The copy counters are read
**45 times across 8 files** — 16 of them in `ownership_checker_oracle`, 9 in
`stdlib_accumulator_cost`, 7 in `ownership_checker_armed`. The whole ownership-checking edifice is
built on "did this append copy the whole list", including `costs.rs`'s own oracle, which judges the
checker's verdicts against exactly that counter.

So S5 is not a representation change with an instrument problem attached. **It is an instrument
change with a representation change attached**, and the instrument half is the larger one: the
checker's notion of a correct verdict has to be restated in a quantity that survives, before the
representation it describes can move. This ADR has called S5 "ready" or "mechanical" more than once
and it has not been either time; the number above is why, and it is recorded so the next reader does
not have to find it the same way. So S5's remaining prerequisite is those assertions moving from "did this
append copy" to "what did this append allocate", and that is a change to what the record
*guarantees* about reuse rather than to how it measures it. It is the last thing standing between
the pricing in §10.1 and the swap.

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

**The case analysis, which is the condition on landing it**, since freeing memory a continuation
still reaches is a wrong program rather than a slow one. `k` is the continuation `K.capture(n)`
produces at a `perform`; `R` is a region whose scan records the clause, which by construction means
the `handle` is written inside `R`.

1. **`k` reaches no binder.** ADR 0005 §1.3 evaluates a tail-resumptive clause body under
   `P.env[x̄ ↦ v̄]` — no `κ`. The tail-resumptive rule is the only one that builds a `Resume` frame,
   so the single reference to `k` is that frame, and nothing in the clause body can name it.
2. **The frame runs before the close.** `Resume(k)` is pushed onto `K′`, the stack below the
   handler's prompt, and `R`'s close sits below that prompt because the `handle` is inside `R`.
   Frames run innermost first, so `k` is spliced before `R` closes.
3. **A clause body that escapes is a different cause and is still counted.** If the clause body
   performs an operation answered outside `R`, that perform is `Cause::Escapes`: a clause body is
   walked under the enclosing context, and `walk_region` clears `handled` at every region boundary.
   This is why `Scan::tail` is its own slot rather than a filter at `settle` — `direct` keeps only
   the first cause in source order and the clause is recorded before its own body is walked, so a
   `TailClause` in `direct` would *hide* this case.
4. **Every other route out is already its own cause** — a second clause binding `resume`, a
   `perform` the region does not answer, `task`, `simulate`, an unknown callee, a callback builtin.

Demonstrated rather than argued, because the entry it replaces was argued:
`region_meaning_adversarial::a_tail_resumptive_clause_writing_its_own_region_still_threads` runs the
shape on both engines with the clause writing the cell of the region it is now `unique` over.

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

### §8.1 Why releasing a parameter releases nothing still read

S3 seeds `lower_block`'s `cumulative` with the enclosing barrier's parameters, so a parameter can
appear in a `Dead` set. ADR 0025 made writing this down the condition for landing it, and the code
comment is one sentence, so it is here.

The filters are unchanged and a parameter has to clear all of them: absent from `after[i]`, what is
still read once statement `i` has finished, and present in `before`, what is read entering it.
`Live` is a backward pass, so those are exact for direct reads. Five routes are not direct reads,
and each keeps the name in `after[i]`:

- **captured by a closure, a handler clause or a `simulate` body** — `Live::close` replays a
  barrier's still-live names into the enclosing set as reads *at the construct that captured them*,
  never last ones, so every statement left of the lambda sees the name live;
- **stored in a cell** — `cell_set(c, xs)` is an ordinary read at that statement, and the value is
  then the arena's rather than this binding's to release;
- **read in a later `match` arm** — `lower_arm` walks the arm inside the enclosing walk, so the read
  is recorded before the walk reaches any statement to its left;
- **read in the tail** — the tail is lowered first, which puts its reads in the live set before any
  statement is visited;
- **shadowed by an inner binder of the same name** — `shadow`/`union` keep the two apart, and the
  `bound[i]` arm of the filter names the shadowing binder rather than the parameter.

Only parameters are seeded. `ownable` holds every name bound anywhere in the barrier, and one from a
sibling block is not in scope at this block at all.

The failure mode is `Slot::Released` reaching `INTERNAL_ERROR` on a legal program — loud, but a new
way to reach a diagnostic whose point is being unreachable. All five routes are run in
`reference_counting_audit::a_parameter_a_later_construct_still_reaches_is_not_released`.

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
  residue rather than a rule, which is the tier a reporting flag fits. It prints
  `unknown` rather than rounding to `reuses` or `COPIES`: `ply_eval::costs`'s
  header lists four shapes no analysis of one body can decide, and rounding them
  is the one thing that would make the checker worse than not having it.
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

**G2 — the corpus, after §4. It has already fired once, on S3.** Over each module's own test suite, by
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

### §10.1 G3 answered, and G2 is what decides it

Both candidate representations were priced before §5 was built, against
`Arc<Vec<Value>>`. Times are the minimum of three; allocations are counted with a
global allocator over one 8,000-element list built by repeated unique append.

| shared/unique time ratio | n = 4,000 | 8,000 | 16,000 | 32,000 |
| --- | ---: | ---: | ---: | ---: |
| `Arc<Vec>` — today | 105 | 215 | 644 | 1,406 |
| `imbl::Vector` | 12.4 | 11.3 | 11.9 | 11.6 |
| `rpds::Vector` | 6.7 | 7.0 | 6.8 | 7.0 |

**G3's property holds for both and fails for what ships.** Today's ratio grows
without bound; both candidates are flat — `imbl` 1.10 across the range, `rpds`
1.05, against a bar of 1.5. The index term is negligible where ADR 0027 §7 says
to measure it: per index, 0.47 ns today against 6.96 ns (`imbl`) and 2.95 ns
(`rpds`), against roughly 1.7 µs of dispatch around it.

**G3 as registered selects `rpds`, and `rpds` is disqualified — by G2.**

| allocations, one 8,000-element list | allocations | bytes | per append |
| --- | ---: | ---: | ---: |
| `Arc<Vec>` | 13 | 131 KB | 0.002 |
| `imbl::Vector` | 135 | 75 KB | 0.017 |
| `rpds::Vector` | **9,293** | 328 KB | **1.162** |

`rpds` allocates **once per append**, which is the spine-node cost ADR 0025's
fallback section predicted and set against itself. Allocations per request is
what G2 bounds and what this record judges a milestone on, so `rpds` cannot land
whatever its time ratio is. `imbl` costs ten times `Vec`'s allocation count in
the relative reading and 122 allocations in the absolute one, while using **43%
fewer bytes**, because `Vec`'s doubling over-allocates where a chunked spine does
not.

**The gate is not being moved to fit this.** G3 asked for a *ratio* and got a
true answer to that question; what it never bounded is the absolute cost of the
good case, and that is not a new bar — it is G2, already registered, already the
thing that caught S3. Read together they admit `imbl` and refuse `rpds`, and the
lesson for the next gate written here is that a ratio criterion needs a level
criterion beside it or it will select whatever makes the common case uniformly
bad.

**Still unmeasured, and it is what §5 turns on:** whether `imbl`'s 122 extra
allocations per large list show up on the request path at all. `/health` builds
small lists, and G2 is measured there. That number needs the change to exist, so
§5 stays gated on it rather than on this table.

---

## §11 Sequence

| | item | state |
| --- | --- | --- |
| **S0** | arm G1 | done — red on five shapes, non-vacuity shown by mutation |
| **S1** | §8, the tail-resumptive refinement | done — sound, and worth zero regions |
| **S2** | wire `ply check --costs` (ADR 0025 §2a, never built) | done |
| **S3** | ADR 0025's P2 — a parameter may appear in a `Dead` set | **measured, gated, not landed** — it fails G2, see below |
| **S4′** | §4's probe — P1 at the `App` and `Record` carry sites | done — four of five pairs to 0.000; §4 has the narrowed claim |
| **S4″** | both of ADR 0025's fallback primitives for P1, priced | done — both roughly double request-path allocations; §4 has the table |
| **S6′** | §6's flat record representation, which the fifth pair needs | **next**, and ahead of S4 rather than behind it |
| **S4a** | §4's flat closure conversion — a lambda captures its free variables, not the scope | done; +0.6 points of reuse, +2.6 allocations, both marginal |
| **S4b** | slot resolution at lowering, verified against the names | done — no runtime change; the assignment the rewrite switches to is wrong-checked first |
| **S4c** | clause and `return` bodies copy in their free variables rather than extending the prompt scope | done, behind the probe — removes §4.1's addressing obstacle |
| **S4** | §4, slot frames — the machine reads by index | gated on **G1**, then **G2** |
| **S5a** | the append counter measures volume, not a boolean | done — necessary, and not sufficient: see §5 |
| **S5b** | the shape assertion moves from "did it copy" to "what did it move" | done — **bytes**, not allocation count; see §5. One of the sites, not all of them |
| **S5c** | the other 44 references to the copy counters | **not started**, and it is the larger half of S5 |
| **S5** | §5, the chunked vector — `imbl::Vector`; `rpds` refused on allocations | gated on **G2**, which §10.1 shows binds harder than G3 |
| **S6** | §6, reuse | G2 does not regress |
| **S7** | §7, `fip` | — |

**S3 fails G2, and what it buys is larger than the record first said.** Landed by default it costs
`/health` about 35 allocations a request, +4.6%. What that buys, on `examples/` and the `std`
modules they import, is in-place appends going from **66.7% to 91.1%** — 10,461 list copies that do
not happen. The cost and the benefit are measured on *different corpora*, and that is the whole
tension: `/health` pays the release without pushing the lists that would repay it, while the corpus
that does push them gains a quarter of its appends. The cause is the one ADR 0025 §What would make this wrong item 1 predicted for P1 and
which applies to P2 for the same reason: `Env::release` rebuilds every link above the binding it
releases, and a *parameter* is the deepest binding in its barrier's chain, so releasing one rebuilds
the whole scope. Whether that trade is worth taking is not something G2 alone answers, and it is recorded here with
both numbers rather than only the one that fails.

So S3 sits behind `PLY_ADR0034_PROBE=1` with S4's probe, and the flag now arms both. The case
analysis (§8.1), the adversarial cases and the G1 pair all stand; what does not stand is landing it
by default. `Env::keep_only(live)` built up from empty is ADR 0025's suggested alternative primitive
and is the thing to try before P2 is proposed again — or `Env::release` writing through a link it
uniquely owns rather than rebuilding it, which it does not do today.

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
5. **If a second evaluator returns.** The tree-walker is deleted, so there is one engine and
   nothing here can diverge between two. That also removes the differential check that would have
   caught a lowering change altering an answer, which makes `differential_corpus`'s replacement
   coverage the thing to watch when §4 lands.

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
