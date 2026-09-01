# ADR 0017 — Regions, and what replaces the forkable world

Status: accepted — **implemented**. Supersedes ADR 0005 §2 and amends ADR 0008
§6.

`ply_eval::world` is gone; `TaskRegions` holds a per-task region stack,
`ExprKind::WithRegion` is a `Code` node in both the AST and the lowering,
`region_kind::infer` decides a region's kind, and `Isolation::Region` has replaced
`Isolation::World` in the test scheduler. §4 is implemented by a **compiler
pass** rather than at runtime: `ply_eval::rc` holds the liveness analysis and the
lowering runs it, so a last use moves and a dead binding releases without a
runtime check. Cycles are uncollected exactly as §4 accepts.

**The forward-looking tense in §6 and §"What must be measured" was written before
the work and is left as written, because each obligation is immediately followed
by the measurement that answered it.**

## Context

Two decisions taken together force this document.

**Zero-cost is the goal.** ADR 0016 measured codegen at a large ratio on the
fragment it can compile and almost nothing end to end, because that fragment is a
few percent of a request; a request meanwhile allocates far more times than it
writes bytes. **Raising codegen's ceiling means fixing representation first.**

**Regions are the memory model.** Ply already has most of one and did not notice:
`with_cell[r]` is a lexically-scoped region whose atoms are discharged at the
boundary, effect rows already state what code touches, and footprints already
prove disjointness.

The consequence is the subject of this ADR. **Perceus-style in-place update fires
only when a value is uniquely owned; a design that forks worlds keeps reference
counts high by construction and the optimization never triggers.** So the
persistent forkable world and the zero-cost path are mutually exclusive, and
choosing zero-cost chooses this.

> **That paragraph is the premise this ADR was accepted on, it was never
> measured, and R3's attribution contradicts it. It is kept as written, because
> deleting it would hide what the milestone was decided from.**
>
> It claims that the forkable world is what holds allocations up. R1 and R2
> removed the world, **and allocations per `/health` went up.** R3 then hoisted
> the two compile-time analyses that were running at runtime, and the figure came
> back down to where it was before the lexical close — **still above where it
> started.** Not measurement noise: an allocation count is exact and does not
> move with a machine, which is the whole reason `w6_report_allocations.rs`
> exists.
>
> The two-window fit says where the allocations are, and neither analysis is on
> the request path in either sense any more. **The largest per-request site is
> the machine's own frame dispatch**, which a region model does not touch.
>
> **The premise was a chain of sound-sounding reasoning — unique ownership,
> reference counts, forking — with no measurement under any link of it, and the
> word this ADR used for its conclusion was *forced*.** `CONTRIBUTING.md`
> §"Measure an ADR's motivating claim before accepting the ADR" is written from
> this document. What the region model *is* worth is measured elsewhere in this
> file: the arena against the persistent map it replaced, and the escape
> discipline — **which is a safety property and was never an allocation claim.**

### Why this is available now and was not in M6

ADR 0005 merged the control stack and the forkable world into one milestone for a
specific reason: capture a continuation inside a `with_cell` region, resume it
outside, and the cell escapes. Three answers were considered — brand the region so
it cannot escape, make the world a persistent value, or copy at capture. **The
persistent world won on two objections to branding, and this ADR dissolves only
the first:**

1. **Branding looked heavy in the type system** — rank-2 polymorphism introduced
   into an otherwise Hindley–Milner system to serve one construct. Building
   regions for memory means building that branding anyway, so this objection is
   genuinely gone: **the mechanism is one this project is now committed to for an
   unrelated reason.**
2. **Branding "forbids the programs multi-shot exists for"** — and this objection
   is **not** dissolved. **It is paid.** §2's "that retires a landed shape" is
   that payment, itemized: three shapes that used to compile no longer do. A
   reader is entitled to see it priced, **because it is a change of program
   meaning under an ADR whose governing property is that meaning does not
   change.**

## The property this ADR must not break

**Program meaning does not change.** Everything below alters representation and
cost, not semantics. A program's observable behaviour under the region model must
be identical to its behaviour under the forkable world, which is what makes
`--engine both` still meaningful as a differential oracle and what makes
migration safe. **Where a construct cannot preserve meaning it is refused at
compile time, never silently reinterpreted.**

## Decisions

### 1. A region is a lexical allocation scope with a brand

```ply
with_region[r] { ... }
```

Values allocated in `r` live in a bump arena freed at the region's close. The
brand `r` appears in the types of values allocated there, so a value cannot
outlive its region — **the ST-monad discipline, applied to allocation rather than
only to cells.**

`with_cell[r]` becomes a special case: a cell is a value allocated in `r`. The
surface syntax is unchanged, so existing programs do not move.

Regions nest. An inner region may reference an outer region's values; the
converse is a compile error naming the escaping value and the region it belongs
to.

### 2. Escape is a type error, not a runtime check

A value's brand is part of its type. Returning it, storing it in an outer
structure, capturing it in a closure that outlives the region, or sending it to
another task are all the same error, **reported at the point the value would
escape rather than where it is later used.**

This is what replaces the forkable world's guarantee. Under ADR 0005 a cell could
not meaningfully escape **because a cell was a *key* rather than a pointer** — so
an escaped cell read a live entry rather than dangling into freed memory, and the
escape question stopped being about safety. Here it cannot escape because the type
says so.

*(This paragraph read "because each resumption got its own world" until §3's
rewrite. That is the snapshot reading §3 retracts, and ADR 0005 §3.1 names the
phrase itself as the wrong one. It survived three sections away from the rewrite,
**which is exactly how a retracted premise stays in a document: it is not
retracted anywhere it is restated.**)*

#### The rule applies to a bare `with_cell[r]`, and that retires a landed shape

§1 says a `with_cell[r]` written outside any region of its name **is** a region,
so the rule above is its rule too: its cell may not leave through a closure, a
record of closures, an operation argument, a store into an enclosing binding, or a
spawn. **The check is one function — `brand_in` over the *resolved* type,
including a function type's effect row, because a closure that captured the cell
need not mention it in its parameters or its result.**

That refuses three shapes that used to compile:

```ply
with_cell[k](0) { c -> || cell_get(c) }                              // a closure
with_cell[k](0) { c -> {get: || cell_get(c), set: |v| cell_set(c, v)} }
with_cell[k](0) { c -> { sink.put(c); 0 } }                          // an operation
```

**Refusing a program that ran is a change of meaning, so it is recorded here
rather than left to an implementation: that is the trade §2 is, and it is taken.**
The forkable world made these safe; the arena does not, and a value that outlives
a freed region is the defect this milestone exists to make impossible. Nothing in
`examples/`, the standard library or the corpus was written in these shapes — the
only users were audit fixtures that smuggled a cell on purpose.

One exclusion, deliberate: **`task.spawn` is not refused for a bare
`with_cell`.** "A cell reaching a task is how tasks share memory" is
`CONTRACTS.md` §`simulate`'s rule, `simulate { with_cell[s](..) { .. spawn .. } }`
is a landed and tested shape, and it is safe under §3 as amended — a `task`
operation anywhere in a region makes the region `shared`, and a `shared` region's
slots outlive its close for exactly this reason. **`with_region` keeps the
stricter rule, because it is new syntax with no program depending on the loose
one.**

#### What the brand still does not catch

A **continuation** parked in an enclosing region's cell, which is ADR 0005
required test 6 and a *success* rather than an error:

```ply
type Saved = Nothing | Just((Bool) -> Int)
with_cell[slot](Nothing) { s ->
  with_cell[log](7) { c -> handle { .. } with { amb.flip() resume k -> cell_set(s, Just(k)) } } }
```

`k`'s row carries `cell.read[log]`, **and the row is erased where `Just`'s field
type is declared, so no type after the constructor mentions `log`.** Refusing it
would need the brand to survive a nominal declaration, which is the rank-2
machinery ADR 0005 rejected. **It is recorded here as the one route that is
open**, and it is what `crates/ply-eval/tests/suite/region_isolation_audit.rs` attacks
the region stack with — *"a cell smuggled out of its region through every carrier
the language has, and a continuation resumed after the region that made it
returned"*.

The consequence for footprints: with every other route closed, **a written row is
the only way a `cell` atom reaches a published footprint.** ADR 0008 §6's
region-label scheduling is unchanged and still exercised, through a written row
rather than through a smuggled closure.

### 3. Resumption semantics — the sharp part

A continuation captured across a region is the case that decided M6, and it is
where a wrong answer would be discovered late and expensively.

> **Amended.** The first draft said "each resumption observes the region as it was
> at capture" and asserted that this "is exactly ADR 0005's semantics".
> **That assertion was false, and the two readings are distinguishable in one
> integer on the very example this section works through.** ADR 0005 §3 threads
> one state and pins the two-resumption example's trace cell at **2** as a
> *required test*; snapshot-at-capture answers **1**. Since this ADR's governing
> property is that program meaning does not change, ADR 0005 wins and this
> section is rewritten to say so. The discriminating programs are landed in
> `region_meaning_audit.rs` and `resumption_semantics_audit.rs`.

**State is threaded, exactly as ADR 0005 §3 says.** There is one current state at
every point of an execution and it moves forward; capture does not save it and
resumption does not restore it. Resumption *n* observes resumption *n−1*'s
writes. A handler that wants per-branch state builds it in four lines with the
cell it already has, **and that direction is one-way: threaded semantics can
express snapshot semantics and not the reverse.**

**Why the snapshot reading cannot be taken, stated once so it is not
re-proposed:** `state.put[s](v) resume k -> { cell_set(c, v); k(()) }` is the
canonical state handler and the shape of essentially every stateful handler in
the standard library. **Restoring the region at `k(())` discards the clause's own
write before the computation that asked for it runs, so `put(5); get()` answers
`0`.** That is not a backtracking corner; **it retypes one-shot resumption, which
is the overwhelming majority of handlers.**

#### What the two region kinds actually decide

Not what a resumption *observes* — that is fixed above — but **when the region's
memory may be reclaimed**:

- **`unique`** — the compiler proves no continuation is captured across this
  region. Nothing can reach its slots after its lexical close, so allocation is a
  bump pointer and the close is a truncation. No copy, no reference counting.
  **This is the case that is free.**
- **`shared`** — a continuation may be captured across this region and resumed
  after its lexical close. The region's slots therefore may not be handed back to
  the bump pointer at that close; they are reference counted per §4 and reclaimed
  when the last continuation that can reach them dies. **Cost is paid only in
  regions where a capture actually happens.**

Inference picks `unique` unless a capture is reachable. An annotation forces
either, **and forcing `unique` where a capture is reachable is a compile error
naming the capture site — because that annotation is a claim that the region's
memory can be freed at its close, and it is a use-after-free when it is wrong.**

#### Worked examples, which implementations must match

**Zero resumptions** — an abort handler discards the continuation. The clause
observes the writes made before the `perform` and none made after it. The region
closes at its lexical end and its arena is freed; a `shared` region drops the last
reference to its slots there and frees them too.

**One resumption** — the ordinary case. The resumption continues on the live arena
and observes every write the clause made before it resumed. `unique` and `shared`
are indistinguishable here except in when the memory goes back.

**Two resumptions** — the case that decides the design:

```ply
with_region[r] {
  with_cell[r](0) { c ->
    handle body() with { amb.flip() resume k -> k(true) + k(false) }
  }
}
```

**The cell is allocated before the capture, so one cell serves both resumptions
and `k(false)` observes what `k(true)` wrote.** With ADR 0005 §3.2's body the
`handle` answers `30` and the cell ends at `2`. That is ADR 0005 required test
10, it is landed, and **it is what this milestone must not move.**

The region is `shared`, because the continuation may be resumed after the
region's close; **what that buys is that the cell is still there to be read**,
which is ADR 0005 required test 6.

**An implementation in which the second resumption fails to observe the first's
writes is wrong, not a permitted optimization.** It would silently break every
cell-backed state handler, and it would make the lost-update race in
`region_meaning_audit.rs::two_tasks_sharing_one_cell_can_still_lose_an_update`
unrepresentable — **a green run on a program with a race in it, which is the
false-green shape this project has found five times.**

`Arena::snapshot` / `Arena::restore` remain in the allocator as a save-and-restore
primitive. **They are not on the capture path, and wiring them there would
implement the retracted reading.** Where a capture does need one it must be
`Arena::snapshot_open`, which covers every region open at the capture rather than
only the innermost: **a resumption may write any of them, and a snapshot of the
region the capture is lexically inside leaves the enclosing regions' writes in
place.** The cost of that is the whole live arena, not one scope.

### 4. What escapes a region is reference-counted

A value that outlives its region is heap-allocated and reference-counted
Perceus-style: the compiler inserts the operations and elides most of them
statically.

**Whether a given update is in place is a *dynamic* test, and this section stated
it as a compile-time property — in the section a reader comes to in order to ask
whether Ply reuses memory. It answered yes unconditionally.**

`push` reaches `Arc::get_mut` and rewrites in place only if the pointer is
unshared **at that instant**; otherwise it allocates and copies the whole array.
The compiler's entire contribution is `Own` on a `Var` node, and `Own`'s own
documentation says it is *"an optimization hint and never a permission"* — a wrong
`Owned` costs a wasted walk and can never change an answer. **That is the property
which keeps `--engine both` meaningful under this ADR, and it is exactly why
nothing in this section is a guarantee about cost.**

**What decides sharing in practice is position.** `rc::carry` hands a pending
frame a live clone of the scope whenever any sub-expression of the enclosing node
remains, **and never asks what those remaining sub-expressions read.** So a value
built anywhere but the last sub-expression of its enclosing node is aliased when
the probe runs and is copied — once per element for a growing container, which is
quadratic — **and the trailing sub-expression that costs the copy may be a literal
constant.** The rule composes across call boundaries, **so a correctly written
callee is made quadratic by its caller** (`spikes/ply-lexer/GAPS.md` §1; ADR 0020
§5.2 is the measurement).

**This ADR therefore establishes no complexity guarantee.** It establishes that
in-place update is *available*; whether a given program gets it is a fact about
where its sub-expressions sit, decided at run time and reported nowhere. **That is
the same shape the Context's correction records one level up — the region work is
a safety property and was never an allocation claim — and a cost claim read out
of this section is that same unmeasured inference one level down.** ADR 0018 §4
is where it was in fact read that way.

Cycles are not collected. A cycle among escaped values leaks, and this ADR accepts
that rather than adding a tracing collector — **say so in the diagnostics where a
cycle is constructible**, and revisit if it proves to matter in practice.

### 5. Tasks each hold a region stack

M7 gave every task its own world. Each task now holds its own region stack, and
values cannot cross tasks except through explicitly shared regions or effects,
because the brand prevents it. **Footprints are unaffected: they are static and do
not depend on how memory is represented.**

### 6. Test isolation becomes `Isolation::Region`

A test's allocations live in a region closed when the test ends, so tests still
cannot observe each other's allocations.

**What is genuinely lost is the case where two tests share a resource label but
have disjoint state** — under the forkable world they parallelized, and now they
are grouped by footprint conflict. **That cost must be measured before this
lands**, and the number that matters is not the isolated count: a pure test has an
empty footprint and conflicts with nothing regardless, **so the question is how
many currently-isolated tests would newly serialize.**

**`--explain` must keep reporting which tests are isolated and which contend.**
ADR 0008 §6 established that when host-backed tests lost world isolation the
reporting had to change too, **or the trivially-parallel count silently
over-claims**; the same trap applies here.

Fixture reuse, which forking made almost free, is replaced by a region-scoped
fixture built once per group and mutated in place, or by W4's
transaction-and-rollback pattern.

## What must be measured

1. **Allocations per request**, against the baseline for one `/health` response.
   **This is the number the ADR exists to move, and it moved the wrong way.**

   The right baseline is not this document's opening figure: ADR 0016 §12.1's
   constant memo landed first and had already taken the same request down, which
   is what `benches/w6-ladder.json` publishes. **Against *that* the region track
   is up a few percent in allocations and in bytes, measured with the arena
   wired** — `World` gone, `Value::Cell` a `Slot` in a `TaskRegions` arena, the
   test-group region no longer forking.

   **So the hypothesis that making the arena the cell store would move this
   figure is falsified on this route.** `/health` allocates no cells: its body is
   a nullary pure definition served from the constant memo, and its allocations
   are `Rc<Value>` boxes on the request path — framing, routing, the JSON encode
   — **which a region model does not touch.** Unboxed primitives and
   monomorphization are what move them, and this ADR puts both in "Not in this
   ADR".

   **What the arena *is* worth is visible where cells exist.**
   `region_arena_cost.rs::a_region_against_the_persistent_map_it_replaced` is a
   result rather than a projection: at ten thousand cells the persistent map
   costs tens of thousands of allocations and a megabyte to build and ten
   thousand more to write every cell; **the region costs nothing to build,
   nothing to write and nothing to close.**

   **The lexical close is now on the evaluation path, and it did not move this
   number either.** Both engines ask `region_kind` for the kind of the region at
   each `with_cell`'s own span, open an arena scope for it, and close it at its
   lexical end, and a capture that can outlive its region takes an `Arena::pin`.
   The wiring added a fixed cost that **halves exactly** at each doubling of the
   window — so it is **once per `Machine` and nothing per request**, and the way
   to remove it is to share one `Regions` across the machines built from one
   program rather than to make the analysis cheaper.

   **R3 took that at its word and the fixed cost is gone.** The two-window fit now
   puts both compile-time analyses at zero per request, and the largest remaining
   per-request site is the machine's frame dispatch. `region_kind::decide`, which
   was a ranked site before R3, does not appear at all.

   Two cautions on the numbers above, **because this document has been burned by
   both.**

   - **A window share is not a request cost.** The lowering reads a third of the
     short window and a twelfth of the long one while contributing **nothing**
     per request; the two-window fit is what separates them. **A pre-R3
     attribution taken at one window and read as request-path work is exactly the
     mistake this shape produces.**
   - **`bytes_per_request` is only comparable at the published window, and that
     is an unexplained finding rather than a caveat.** It *rises* with the
     window, so total bytes grow faster than the request count while the
     allocation *count* falls with the window exactly as a slope plus an
     intercept must. **Something on the `w6-alloc` path is superlinear in the
     number of connections in one script; this milestone did not diagnose
     which**, and whoever next touches the allocation harness should find out
     why.

   **Against the pre-region baseline the number is still up.** `ROADMAP.md` §R3
   records which branch of R3's decision rule fired.

   **The dynamic split is where the milestone is worth something, and it is not
   the static one.** Over `examples/` and the `std` modules they import the
   static split is **every region `shared` and none `unique`**, every one because
   of a tail-resumptive clause — **which under a design where `shared` meant
   "never reclaimed" would have said the zero-cost claim buys nothing at all.**
   It does not mean that, because the two kinds are a claim about what a close
   will *find* rather than a decision about whether one happens: running every
   test in `examples/` frees **every** region at its close and defers none, with
   a peak of a handful of live slots. **Every `shared` region reclaims at its
   close on every run of the corpus, because no continuation captured across one
   ever outlives it there.**

   The refinement §3 leaves open — a tail-resumptive clause's continuation is
   consumed by the `Resume` frame the machine pushes for it, so it cannot outlive
   the region's close — is therefore worth **precision in a report and not
   memory**, and is deliberately not taken here. **It is taken at the *capture*
   instead, where it is a cost rather than a kind:** a tail-resumptive clause
   takes no pin, because the only thing that will ever splice its continuation is
   that `Resume` frame. Without that, `perform` allocated an `Rc` per operation.

   **`w6_report_allocations.rs` will not catch drift in this number**: its band is
   a factor of two either way, deliberately, because it is a staleness guard on
   the report and not a performance gate. **A gate on this number needs to be its
   own artifact.**
2. **The isolation cost** — how many currently world-isolated tests newly
   serialize. **The only real argument against this design.**

   **Zero, on this corpus.** `ply-corpus regions examples` colours the same tests
   with and without the world-backed exemption and reports the same group count
   and a modelled wall-clock ratio of 1.00×. **The reason is stated rather than
   celebrated: no test in `examples/` carries a `cell` atom in its footprint at
   all, so the exemption was exempting nothing.** `--hypothetical cells:labels`
   is how the risk is priced for a corpus that would.

   *Everything load-bearing here reproduced on a re-take. The one figure that did
   not is the critical path, which is a wall-clock absolute derived from per-test
   durations — **the least portable number in the paragraph, and the ratio beside
   it is the portable one.***
3. **Region-scoped fixture cost**, measured the way fork's was, **so "cheap"
   stays a fact rather than becoming a slogan.**

   `fixture_open_cost.rs`: opening a ten-thousand-cell fixture and writing one
   cell costs about a tenth of a millisecond per test, **against `World::fork`'s
   one nanosecond.** *(The allocation and byte figures are exact; the timing came
   back half again as large on a slower box, **so it is the figure to re-take
   rather than to quote.**)* **This is the price §6 says is paid, and it is paid
   per test rather than per group.** Every Ply program in this repository opens
   an empty fixture, where it is nothing at all — **which is also why the number
   is a projection about a construct that is still not writable in Ply.**
4. **`--engine both` agreement** across the change — **and the size of the hole in
   it, stated rather than left implied.**

   The oracle compares the tree-walker against the machine, and **the tree-walker
   refuses every clause that binds a continuation** (`E0504`, ADR 0005 required
   test 3). **So `--engine both` audits nothing about multi-shot resumption,
   which is precisely the construct this ADR changes.**
   `ply test examples/ --engine both --explain` counts the gap; the tests that
   ran on one engine only are the ones reaching a `resume` binder or a `simulate`
   region. On the generated corpus there is no hole. Zero divergences on both.

   **A second and larger hole: both engines hold the same state representation,
   so a change to the memory model moves them together and the comparison stays
   green whatever it did to meaning.**

   **The oracle for "meaning did not move" is therefore not `--engine both`.** It
   is `region_meaning_audit.rs` and `resumption_semantics_audit.rs`: programs
   whose answer *differs* between the two candidate readings, with the expected
   integer written down. `--engine both` remains worth running and remains weak
   evidence here.

## Consequences

The forkable world is removed. **Codegen's ceiling should be re-measured after
this lands, because ADR 0016's verdict was a verdict on the old representation
and this ADR changes exactly what made that ceiling low.**

**Done, after R3, and the ceiling did not move.** The whole ladder was re-taken
with the command `benches/README.md` publishes and is shipped as
**`benches/w6-ladder-r3.json`** so it can be re-rendered rather than quoted:

```
./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json
```

That prints the whole table, the verdict and the audit. The verdict is unchanged:
**keep deferring M9.**

**The absolutes are all larger than ADR 0016 §8.1's, and the reason is the rig
rather than this change:** the **Rust floor**, which has no Ply under it at all,
moved by about a tenth, so the box measures that much slow against the one W6
used. **The portable readings are the ratios, and they are flat.** R3 did not
move what a request costs and was not expected to — **it removed work that was
already amortized over a served process's lifetime. It removed it from the
*count*, which is exact, and the count is where it is visible.**

**`benches/w6-ladder.json` is deliberately not overwritten:** it is the only
record of the pre-region baseline, which is the anchor of R3's decision rule and
of this document's corrections. `benches/README.md` says so beside both files.

### Where this could go wrong

In order of how hard it would be to see:

- **A resumption failing to observe a previous resumption's writes.** §3's
  two-resumption example is the test that catches it, and `region_meaning_audit`
  is the file of programs where the two readings differ. **Silent, and it breaks
  every cell-backed state handler.**
- **An escape the brand does not catch** — through a closure, a constructor
  field, a `Map` key, a returned continuation, or a task. **W2 found the
  analogous hole reachable through a *type alias*, so the check must run on
  resolved types.**
- **`unique` inferred where a capture is reachable**, which frees memory a
  continuation can still reach. **Inference must be conservative: when in doubt,
  `shared`.** In particular a `handle` that lexically *encloses* a region does
  not make the operations it answers local to that region — it answers across the
  region's boundary, **which is the definition of a capture crossing it.**

  > **This one happened, and it was found by a reader rather than by a test.**
  > `region_kind::Analysis` carried no local scope: it resolved a bare name
  > against the *module* scope, **so a parameter, a `let` or a pattern binder
  > shadowing a top-level definition's name was read as that definition.** A
  > function taking a function-typed parameter named `helper`, in a module also
  > declaring `fn helper`, recorded an edge to the definition — which reaches no
  > capture — instead of an indirect call, **and inferred `unique` over a callee
  > that is whatever the caller passed.** It was latent rather than live only
  > because the arena's close never reads the kind, **but `region_kind::check`
  > did accept a hand-written `unique` this section requires it to refuse.**
  > Closed by `Analysis::locals`, with the answer pinned for a parameter, a
  > `let`, a `match` binder, a lambda parameter and a callback argument by
  > `hoist_staleness_audit.rs`. **The census the module comment publishes did not
  > move.**

## Not in this ADR

Unboxed primitive representation, monomorphization, evidence passing and handler
specialization, and native codegen. Each is a separate milestone; **this one
establishes the memory model they all depend on.**
