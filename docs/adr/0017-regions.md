# ADR 0017 — Regions, and what replaces the forkable world

Status: accepted — **implemented**. Supersedes ADR 0005 §2 and amends ADR
0008 §6.

> **Corrected by the W6 documentation audit.** This line read "proposed" while
> the document's own §"What must be measured" reported readings "taken with the
> arena wired", said "`World` is gone", and quoted measurements from
> `region_reclamation_census.rs` — none of which is possible from a proposal.
> Checked against the tree rather than against the prose: `ply_eval::world` does
> not exist; `TaskRegions` is `crates/ply-eval/src/task_regions.rs`;
> `ExprKind::WithRegion` is in both `ply_syntax::ast` and `ply_eval::code` as a
> `Code` node; `region_kind::infer` is `crates/ply-eval/src/region_kind.rs`;
> `Isolation::Region` has replaced `Isolation::World` in
> `crates/ply-test/src/schedule.rs`, whose module doc cites this ADR §6 by
> number; and `crates/ply-eval/tests/` holds twelve files whose names begin
> `region_` or `reference_`, counted rather than estimated.
>
> §4 is implemented too, and by a compiler pass rather than at runtime as its
> one-paragraph statement might suggest: `crates/ply-eval/src/rc.rs` holds the
> liveness analysis (`Live`, `Own`, `Dead`) and `code.rs` runs it at lowering,
> so a last use moves and a dead binding releases without a runtime check.
> Cycles are uncollected exactly as §4 accepts, with `rc::cell_cycle` and
> `rc::take_cycles` supplying the diagnostics §4 asks for.
>
> The forward-looking tense throughout §6 and §"What must be measured" — "that
> cost must be measured before this lands", "`--explain` must keep reporting" —
> was written before the work and is left as written, because each is
> immediately followed by the measurement that answered it.

## Context

Two decisions taken together force this document.

**Zero-cost is the goal.** ADR 0016 measured codegen at **11.67× on the fragment
it can compile** and 1.02–1.05× end to end, because that fragment is 2–5% of a
request. The ceiling is low not because compilation fails but because the
*representation* is expensive: every value is heap-allocated, every handler
dispatch walks a stack. One `/health` response cost **1,035 allocations and
0.124 MB** at the point this ADR was opened. Raising codegen's ceiling means
fixing representation first.

> **Two figures in the paragraph above were wrong and are corrected in place, by
> a documentation audit that checked them against their sources rather than
> against the prose that carried them.**
>
> - **8.44× was unsourced.** It appears nowhere else in this repository. ADR
>   0016 §9 reports the spike's `k` as **11.67×** — the minimum over its five
>   inputs, interpreter-best against spike-worst — and `benches/w6-spike.json`'s
>   five inputs give **11.67× to 12.97×** on that pairing (12.97, 12.83, 11.67,
>   12.97, 12.31). The widest reading of the file, best against best, tops out at
>   13.23×; a first pass of this bullet wrote the band as "11.67× to 13.23×",
>   which silently mixes the two pairings. No pairing of that file produces
>   8.44×. The 1.02–1.05× end to end and the 2–5% fragment share do
>   check out against ADR 0016 §10.3.
> - **9,343 allocations and 1.03 MB was a retired first take.** It is ADR 0016's
>   pre-constant-memo number; the correct *pre-region baseline* — which is what
>   the Context paragraph wants, since it is sizing the problem this ADR
>   inherits — is **1,035 allocations and 0.124 MB**, which is what
>   `benches/w6-ladder.json` publishes and what `w6_report_allocations.rs`
>   guards. (A first pass of this correction called 1,035 "the shipped figure".
>   It is not, and this document says so 375 lines below: the shipped figure
>   after the arena and the lexical close is **1,122 / 131,677**, re-taken by a
>   later audit as `{"allocations_per_request":1122.335,
>   "bytes_per_request":131677.4}`. §"What must be measured" ¶1 shows the whole
>   1,035 → 1,082 → 1,122 progression and why the +87 is one-time. Leaving
>   "shipped" attached to 1,035 reproduced, one level down, the exact defect the
>   bullet was written to fix.) This ADR already said so 290 lines below,
>   in "What must be measured" ¶1 — but it said it *there*, while the Context
>   went on asserting the retired pair as current fact. Anyone reading only the
>   Context would have sized this milestone's win against a baseline nine times
>   too large. The correction is now at both ends.

**Regions are the memory model.** Ply already has most of one and did not notice:
`with_cell[r]` is a lexically-scoped region whose atoms are discharged at the
boundary, effect rows already state what code touches, and footprints already
prove disjointness.

The consequence is the subject of this ADR. Perceus-style in-place update fires
only when a value is **uniquely owned**; a design that forks worlds keeps
reference counts high by construction and the optimization never triggers. So
the persistent forkable world and the zero-cost path are mutually exclusive, and
choosing zero-cost chooses this.

### Why this is available now and was not in M6

ADR 0005 merged the control stack and the forkable world into one milestone for
a specific reason: capture a continuation inside a `with_cell` region, resume it
outside, and the cell escapes. Three answers were considered — brand the region
so it cannot escape, make the world a persistent value, or copy at capture. The
persistent world won on **two** objections to branding, and this ADR dissolves
only the first:

1. **Branding looked heavy in the type system** — rank-2 polymorphism introduced
   into an otherwise Hindley–Milner system to serve one construct. Building
   regions for memory means building that branding anyway, so this objection is
   genuinely gone: the mechanism is one this project is now committed to for an
   unrelated reason.
2. **Branding "forbids the programs multi-shot exists for"** (ADR 0005,
   Alternatives (a)) — and this objection is **not** dissolved. It is *paid*.
   §2's "that retires a landed shape" is that payment, itemized: three shapes
   that used to compile no longer do.

> **Corrected by the W6 documentation audit.** This section previously said "the
> objection that decided M6 no longer applies", singular, which is true of (1)
> and false of (2) — and (2) is the one a reader is entitled to see priced,
> because it is a change of program meaning under an ADR whose governing
> property is that meaning does not change. §2 already prices it honestly; this
> section is what pointed the reader past it.

## The property this ADR must not break

**Program meaning does not change.** Everything below alters representation and
cost, not semantics. A program's observable behaviour under the region model must
be identical to its behaviour under the forkable world, which is what makes
`--engine both` still meaningful as a differential oracle and what makes
migration safe. Where a construct cannot preserve meaning it is refused at
compile time, never silently reinterpreted.

## Decisions

### 1. A region is a lexical allocation scope with a brand

```ply
with_region[r] { ... }
```

Values allocated in `r` live in a bump arena freed at the region's close. The
brand `r` appears in the types of values allocated there, so a value cannot
outlive its region — the ST-monad discipline, applied to allocation rather than
only to cells.

`with_cell[r]` becomes a special case: a cell is a value allocated in `r`. The
surface syntax is unchanged, so existing programs do not move.

Regions nest. An inner region may reference an outer region's values; the
converse is a compile error naming the escaping value and the region it belongs
to.

### 2. Escape is a type error, not a runtime check

A value's brand is part of its type. Returning it, storing it in an outer
structure, capturing it in a closure that outlives the region, or sending it to
another task are all the same error, reported at the point the value would
escape rather than where it is later used.

This is what replaces the forkable world's guarantee. Under ADR 0005 a cell
could not meaningfully escape because a cell was a *key* rather than a pointer —
so an escaped cell read a live entry rather than dangling into freed memory, and
the escape question stopped being about safety. Here it cannot escape because
the type says so.

> **Corrected with §3, by the same audit.** This paragraph originally read
> "because each resumption got its own world". That is the snapshot reading §3
> retracted, and ADR 0005 §3.1 names the phrase itself — "the reading the phrase
> *each resumption gets its own world* invites" — as the wrong one, listing it in
> its Alternatives as rejected. ADR 0005's actual reason is quoted above and is
> §2's "a cell is a key, not a pointer". The sentence survived §3's rewrite
> because it is three sections away from it, which is exactly how a retracted
> premise stays in a document: it is not retracted anywhere it is *restated*.

#### The rule applies to a bare `with_cell[r]`, and that retires a landed shape

§1 says a `with_cell[r]` written outside any region of its name **is** a region,
so the rule above is its rule too: its cell may not leave through a closure, a
record of closures, an operation argument, a store into an enclosing binding, or
a spawn. The check is one function — `brand_in` over the **resolved** type,
including a function type's effect row, because a closure that captured the cell
need not mention it in its parameters or its result.

That refuses three shapes that used to compile:

```ply
with_cell[k](0) { c -> || cell_get(c) }                              // a closure
with_cell[k](0) { c -> {get: || cell_get(c), set: |v| cell_set(c, v)} }
with_cell[k](0) { c -> { sink.put(c); 0 } }                          // an operation
```

Refusing a program that ran is a change of meaning, so it is recorded here
rather than left to an implementation: **that is the trade §2 is, and it is
taken.** The forkable world made these safe; the arena does not, and a value
that outlives a freed region is the defect this milestone exists to make
impossible. Nothing in `examples/`, the standard library or the corpus was
written in these shapes — the only users were audit fixtures that smuggled a
cell on purpose, and they now reach a `cell` atom through a written row
instead.

One exclusion, deliberate: **`task.spawn` is not refused for a bare
`with_cell`.** "A cell reaching a task is how tasks share memory" is
CONTRACTS §`simulate`'s rule, `simulate { with_cell[s](..) { .. spawn .. } }` is
a landed and tested shape, and it is safe under §3 as amended — a `task`
operation anywhere in a region makes the region `shared`, and a `shared`
region's slots outlive its close for exactly this reason. `with_region` keeps
the stricter rule, because it is new syntax with no program depending on the
loose one.

#### What the brand still does not catch

A **continuation** parked in an enclosing region's cell, which is ADR 0005
required test 6 and a *success* rather than an error:

```ply
type Saved = Nothing | Just((Bool) -> Int)
with_cell[slot](Nothing) { s ->
  with_cell[log](7) { c -> handle { .. } with { amb.flip() resume k -> cell_set(s, Just(k)) } } }
```

`k`'s row carries `cell.read[log]`, and the row is erased where `Just`'s field
type is declared, so no type after the constructor mentions `log`. Refusing it
would need the brand to survive a nominal declaration, which is the rank-2
machinery ADR 0005 §"Alternatives" rejected. It is recorded here as the one
route that is open, and it is what
`crates/ply-eval/tests/region_isolation_audit.rs` now attacks the region stack
with — layer 2 of that file, "a cell smuggled out of its region through every
carrier the language has, and a continuation resumed after the region that made
it returned".

> **Path corrected by the W6 documentation audit.** This read
> `ply-eval/tests/world_isolation_audit.rs`, which does not exist and cannot:
> the file was renamed with the thing it audits when `World` became
> `TaskRegions`. It was the only dangling repository path anywhere in
> `docs/adr/`, and it pointed at the file a reader would open to check the *one*
> escape route this ADR admits is still open — so the cost of the stale name is
> exactly that the check looks unavailable.

The consequence for footprints: with every other route closed, **a written row
is the only way a `cell` atom reaches a published footprint.** ADR 0008 §6's
region-label scheduling is unchanged and still exercised, through
`fn touches(n: Int) -> Int / {cell.read[table]} = n` rather than through a
smuggled closure.

### 3. Resumption semantics — the sharp part

A continuation captured across a region is the case that decided M6, and it is
where a wrong answer would be discovered late and expensively.

> **Amended.** The first draft of this section said "each resumption observes the
> region as it was at capture" and asserted that this "is exactly ADR 0005's
> semantics". **That assertion was false**, and the two readings are
> distinguishable in one integer on the very example this section works through.
> ADR 0005 §3 threads one state and pins the two-resumption example at `30` with
> the trace cell at `2` as a *required test*; snapshot-at-capture answers `1`
> for that cell. Since this ADR's governing property is that program meaning does
> not change, ADR 0005 wins and this section is rewritten to say so. The
> discriminating programs are landed in
> `crates/ply-eval/tests/region_meaning_audit.rs` and
> `crates/ply-eval/tests/resumption_semantics_audit.rs`.

**State is threaded, exactly as ADR 0005 §3 says.** There is one current state at
every point of an execution and it moves forward; capture does not save it and
resumption does not restore it. Resumption *n* observes resumption *n−1*'s
writes. A handler that wants per-branch state builds it in four lines with the
cell it already has (ADR 0005 §3.3), and that direction is one-way: threaded
semantics can express snapshot semantics and not the reverse.

Why the snapshot reading cannot be taken, stated once so it is not re-proposed:
`state.put[s](v) resume k -> { cell_set(c, v); k(()) }` is the canonical state
handler and the shape of essentially every stateful handler in the standard
library. Restoring the region at `k(())` discards the clause's own write before
the computation that asked for it runs, so `put(5); get()` answers `0`. That is
not a backtracking corner; it retypes one-shot resumption, which is the
overwhelming majority of handlers.

#### What the two region kinds actually decide

Not what a resumption *observes* — that is fixed above — but **when the region's
memory may be reclaimed**:

- **`unique`** — the compiler proves no continuation is captured across this
  region. Nothing can reach its slots after its lexical close, so allocation is
  a bump pointer and the close is a truncation. No copy, no reference counting.
  This is the case that is free.
- **`shared`** — a continuation may be captured across this region, and it may
  be resumed after the region's lexical close. The region's slots therefore may
  not be handed back to the bump pointer at that close; they are reference
  counted per §4 and reclaimed when the last continuation that can reach them
  dies. Cost is paid only in regions where a capture actually happens.

Inference picks `unique` unless a capture is reachable. An annotation forces
either, and forcing `unique` where a capture is reachable is a compile error
naming the capture site — because that annotation is a claim that the region's
memory can be freed at its close, and it is a use-after-free when it is wrong.

#### Worked examples, which implementations must match

**Zero resumptions** — an abort handler discards the continuation:

```ply
with_region[r] {
  handle body() with { fail.stop() -> 0 }        // never resumes
}
```
The clause observes the writes made before the `perform` and none made after it.
The region closes at its lexical end and its arena is freed; a `shared` region
drops the last reference to its slots there and frees them too.

**One resumption** — the ordinary case:

```ply
with_region[r] {
  handle body() with { ask.get() resume k -> k(7) }
}
```
The resumption continues on the live arena and observes every write the clause
made before it resumed. `unique` and `shared` are indistinguishable here except
in when the memory goes back.

**Two resumptions** — the case that decides the design:

```ply
with_region[r] {
  with_cell[r](0) { c ->
    handle body() with { amb.flip() resume k -> k(true) + k(false) }
  }
}
```
The cell is allocated before the capture, so **one** cell serves both
resumptions and `k(false)` observes what `k(true)` wrote. Written out with
ADR 0005 §3.2's body — increment, then `if b { 10 } else { 20 }` — the `handle`
answers `30` and the cell ends at `2`. That is ADR 0005 required test 10, it is
landed, and it is what this milestone must not move.

The region is `shared`, because the continuation may be resumed after the
region's close; what that buys is that the cell is still there to be read, which
is ADR 0005 required test 6.

An implementation in which the second resumption *fails* to observe the first's
writes is **wrong**, not a permitted optimization. It would silently break every
cell-backed state handler, and it would make the lost-update race in
`region_meaning_audit.rs::two_tasks_sharing_one_cell_can_still_lose_an_update`
unrepresentable — a green run on a program with a race in it, which is the
false-green shape this project has found five times.

`Arena::snapshot` / `Arena::restore` remain in the allocator as a save-and-
restore primitive. **They are not on the capture path**, and wiring them there
would implement the retracted reading. Where a capture does need one — an
explicit checkpoint, a future world-snapshot builtin — it must be
`Arena::snapshot_open`, which covers every region open at the capture rather
than only the innermost: a resumption may write any of them, and a snapshot of
the region the capture is lexically inside leaves the enclosing regions' writes
in place. The cost of that is the whole live arena, not one scope.

### 4. What escapes a region is reference-counted

A value that outlives its region is heap-allocated and reference-counted
Perceus-style: the compiler inserts the operations and elides most of them
statically, and a uniquely-owned value is mutated in place rather than copied.

Cycles are not collected. A cycle among escaped values leaks, and this ADR
accepts that rather than adding a tracing collector — say so in the diagnostics
where a cycle is constructible, and revisit if it proves to matter in practice.

### 5. Tasks each hold a region stack

M7 gave every task its own world. Each task now holds its own region stack, and
values cannot cross tasks except through explicitly shared regions or effects,
because the brand prevents it. Footprints are unaffected: they are static and do
not depend on how memory is represented.

### 6. Test isolation becomes `Isolation::Region`

`Isolation::World` is renamed and reimplemented. A test's allocations live in a
region closed when the test ends, so tests still cannot observe each other's
allocations.

What is genuinely lost is the case where two tests share a resource label but
have disjoint state — under the forkable world they parallelized, and now they
are grouped by footprint conflict. **That cost must be measured before this
lands**: today's split is `isolated 176 of 186 · 10 tests can contend`, but a
pure test has an empty footprint and conflicts with nothing regardless, so the
number that matters is how many of the 176 would newly serialize.

`--explain` must keep reporting which tests are isolated and which contend. ADR
0008 §6 established that when host-backed tests lost world isolation the
reporting had to change too, or the trivially-parallel count silently
over-claims; the same trap applies here.

Fixture reuse, which forking made cheap (8,939× cheaper than rebuilding a
10,000-cell fixture), is replaced by a region-scoped fixture built once per group
and mutated in place, or by W4's transaction-and-rollback pattern.

## What must be measured

1. **Allocations per request**, against the baseline for one `/health` response.
   This is the number the ADR exists to move.

   **The 9,343 / 1.03 MB in the Context above is not the baseline this milestone
   can be credited against.** ADR 0016 §12.1's constant memo landed before it and
   already took the same request to **1,035 allocations and 0.124 MB**, which is
   what `benches/w6-ladder.json` publishes. Measured now:

   ```
   $ ./target/release/w6-alloc --repo . --requests 200
   {"allocations_per_request":1081.78,"bytes_per_request":127801.79,
    "requests":200,"response_bytes":107,"route":"/health"}
   ```

   **1,082 against 1,035: +4.5% allocations and +3.1% bytes, in the wrong
   direction — and this reading is taken with the arena wired.** `World` is
   gone, `Value::Cell` is a `Slot` in a `TaskRegions` arena, and
   `ply_test::region::GroupRegion` no longer forks. The number moved by two
   allocations, from 1,083.79 to 1,081.78.

   So the hypothesis that making the arena the cell store would move this figure
   is **falsified on this route**. `/health` allocates no cells: its body is a
   nullary pure definition served from ADR 0016 §12.1's constant memo, and the
   1,035 are `Rc<Value>` boxes on the request path — framing, routing, the JSON
   encode — which a region model does not touch. Unboxed primitives and
   monomorphization are what move them, and this ADR puts both in "Not in this
   ADR".

   What the arena *is* worth is visible where cells exist.
   `crates/ply-eval/tests/region_arena_cost.rs::a_region_against_the_persistent_map_it_replaced`
   is now a result rather than a projection: at 10,000 cells the persistent map
   costs 20,000 allocations and 1.04 MB to build and 10,000 more to write every
   cell; the region costs **0 to build, 0 to write and 0 to close**.

   **The lexical close is now on the evaluation path, and it did not move this
   number either.** Both engines ask `region_kind` for the kind of the region at
   each `with_cell`'s own span, open an arena scope for it, and close it at its
   lexical end; `ExprKind::WithRegion` is a `Code` node rather than lowered away;
   and a capture that can outlive its region takes an `Arena::pin`, so the close
   frees or defers according to whether a continuation can still reach the slots.
   Measured the same way:

   ```
   $ ./target/release/w6-alloc --repo . --requests 200
   {"allocations_per_request":1122.34,"bytes_per_request":131677.40, ...}
   ```

   **1,122 against 1,082, and all forty of them are one-time.** Taken at 200,
   400 and 800 requests the delta halves exactly each time — +40.54, +20.27,
   +10.14 — so the wiring costs **8,108 allocations once per `Machine` and
   nothing per request**. That fixed cost is `region_kind::infer`, a whole-program
   analysis run lazily at the first region a machine opens; a service opens three
   at start-up and none per request. Published against a 200-request window it
   reads as +3.9%; against 800 it reads as +0.9%; against a server's lifetime it
   is nothing. It is still a cost this ADR did not predict, and the way to remove
   it is to share one `Regions` across the machines built from one program rather
   than to make the analysis cheaper.

   So the hypothesis is falsified twice over on this route, once for the arena
   and once for the close. `/health` opens no region per request and its 1,035
   are boxes.

   The **dynamic** split is where the milestone is worth something, and it is not
   the static one. Over `examples/` and the `std` modules they import the static
   split is **113 regions, 0 `unique`, 113 `shared`**, every one of them because
   of a tail-resumptive clause
   (`region_kind_inference.rs::the_split_over_the_repositorys_own_examples`) —
   which under a design where `shared` meant "never reclaimed" would have said
   the zero-cost claim buys nothing at all. It does not mean that, because the
   two kinds are a claim about what a close will find rather than a decision
   about whether one happens: running every test in `examples/` gives **709
   region closes, 709 freed at the close and 0 deferred**, 348 slot bumps against
   a peak of 6 live, 73 pins taken and 0 slots reclaimed late
   (`region_reclamation_census.rs`). Every one of the 113 `shared` regions
   reclaims at its close on every run of the corpus, because no continuation
   captured across one ever outlives it there.

   The refinement §3 leaves open — a tail-resumptive clause's continuation is
   consumed by the `Resume` frame the machine pushes for it, so it cannot outlive
   the region's close — is therefore worth **precision in a report and not
   memory**, and it is deliberately not taken here. It is taken at the *capture*
   instead, where it is a cost rather than a kind: a tail-resumptive clause takes
   no pin, because the only thing that will ever splice its continuation is that
   `Resume` frame, one frame above the `CloseRegion` frames of every region open
   at the capture. Without that, `perform` allocated an `Rc` per operation and
   the served request paid seven of them.

   `w6_report_allocations.rs` will not catch this drift: its band is a factor of
   two either way, deliberately, because it is a staleness guard on the report
   and not a performance gate. A gate on this number needs to be its own
   artifact.
2. **The isolation cost** — how many of the 176 currently world-isolated tests
   newly serialize. The only real argument against this design.

   **Zero, on this corpus.** `ply-corpus regions examples` colours the same 186
   tests with and without the world-backed exemption and reports `5→5` groups,
   a critical path of 20.2 ms either way and a modelled wall clock ratio of
   `1.00x`. The reason is stated rather than celebrated: no test in `examples/`
   carries a `cell` atom in its footprint at all, so the exemption was
   exempting nothing. `--hypothetical cells:labels` is how the risk is priced
   for a corpus that would.

   *Audit re-take: everything load-bearing here reproduced —* `5→5` *groups, 186
   tests, 176 isolated, 0 carrying a* `cell` *atom, 0 newly serialized, ratio*
   `1.00x`. *The one figure that did not is the critical path, which came back
   at* **91.4 ms → 91.4 ms** *(modelled makespan 223.7 ms) on a box that measures
   slower than this rig across the tree. It is a wall-clock absolute derived from
   per-test durations, so it is the least portable number in the paragraph and
   the ratio beside it — which is the actual claim — is the portable one.*
3. **Region-scoped fixture cost**, measured the way fork's 1 ns was, so
   "cheap" stays a fact rather than becoming a slogan.

   `fixture_open_cost.rs`: opening a 10,000-cell fixture and writing one cell
   costs **95.7 µs per test**, against `World::fork`'s 1 ns. At 100,000 cells an
   open is 800 allocations and 4.45 MB. (Audit re-take: the two allocation
   figures are exact — the test prints `100000 → 800` allocations and `4452576`
   bytes. The 95.7 µs is a wall-clock timing and came back at **155.3 µs** on a
   slower box, so it is the figure to re-take rather than to quote.) This is the
   price §6 says is paid, and
   it is paid per test rather than per group. Every Ply program in this
   repository opens an empty fixture, where it is nothing at all — which is also
   why the number is a projection about a construct that is still not writable
   in Ply.
4. **`--engine both` agreement** across the change — and the size of the hole in
   it, stated rather than left implied.

   The oracle compares the tree-walker against the machine, and **the
   tree-walker refuses every clause that binds a continuation** (`E0504`, which
   is ADR 0005 required test 3). So `--engine both` audits nothing about
   multi-shot resumption, which is precisely the construct this ADR changes.
   `ply test examples/ --engine both --explain` counts the gap: **audited 166 of
   186 · 20 ran on one engine only**. The 20 are the tests reaching a `resume`
   binder or a `simulate` region; `simulated: 11 of 186` is the second of those
   counts and is the figure an earlier draft of this list quoted as the whole of
   the hole. On the generated corpus there is no hole: **audited 300 of 300**.
   Zero divergences on both.

   A second and larger hole: both engines hold the same state representation, so
   a change to the memory model moves them together and the comparison stays
   green whatever it did to meaning.

   The oracle for "meaning did not move" is therefore not `--engine both`. It is
   `crates/ply-eval/tests/region_meaning_audit.rs` and
   `crates/ply-eval/tests/resumption_semantics_audit.rs`: programs whose answer
   *differs* between the two candidate readings, with the expected integer
   written down. `--engine both` remains worth running and remains weak
   evidence here.

## Consequences

The forkable world is removed. Codegen's ceiling should be re-measured after this
lands, because ADR 0016's 1.05× was a verdict on the old representation and this
ADR changes exactly what made that ceiling low.

Where this could go wrong, in order of how hard it would be to see:

- **A resumption failing to observe a previous resumption's writes.** §3's
  two-resumption example is the test that catches it, and `region_meaning_audit`
  is the file of programs where the two readings differ. Silent, and it breaks
  every cell-backed state handler.
- **An escape the brand does not catch** — through a closure, a constructor
  field, a Map key, a returned continuation, or a task. W2 found the analogous
  hole reachable through a *type alias*, so the check must run on resolved types.
- **`unique` inferred where a capture is reachable**, which frees memory a
  continuation can still reach. Inference must be conservative: when in doubt,
  `shared`. In particular a `handle` that lexically *encloses* a region does not
  make the operations it answers local to that region — it answers across the
  region's boundary, which is the definition of a capture crossing it.

## Not in this ADR

Unboxed primitive representation, monomorphization, evidence passing and handler
specialization, and native codegen. Each is a separate milestone; this one
establishes the memory model they all depend on.
