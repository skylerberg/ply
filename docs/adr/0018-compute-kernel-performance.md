# ADR 0018 — Closing the gap for compute kernels

Status: proposed. **No decision here is accepted.** Every section states what is
measured, what is assumed, and the measurement that would settle it — because
ADR 0017 asserted an unmeasured premise, called it forced, and cost three
milestones (see `CONTRIBUTING.md` §"Measure an ADR's motivating claim before
accepting the ADR").

## Context

The motivating question: could a maximally-performant Monte Carlo tree search
library be written in Ply rather than Rust? MCTS is a good probe because it is
almost pure compute — a tight loop over a mutable tree, millions of iterations,
hot RNG, and parallel rollouts contending on shared nodes. It exercises every
place Ply is currently weak and almost none of where it is strong.

The answer today is no, by roughly an order of magnitude. What follows is a plan
per gap, ordered by what the existing measurements actually support.

### What is measured

| Figure | Source |
| --- | --- |
| 41.6× the Rust floor, served request | `benches/w6-ladder-r3.json` |
| 2.51× tree-walker, control-stack machine on the pure path | ADR 0016 |
| 11.67× minimum, codegen spike on its compilable fragment | **computed from** `benches/w6-spike.json` — interpreter-best ÷ spike-worst, not a field in the file |
| 1.55× end to end from a fast execution strategy | ADR 0016 §"the second lever" |
| 1,082 allocations per `/health`, 24% in `frame::dispatch` | `w6-alloc`, `w6_alloc_sites` |

> **The served rung runs a pre-built binary (2026-08-27).** `mcts` itself is
> built by cargo and is therefore current by construction, but its `--served`
> denominator starts `target/release/ply` as a subprocess
> (`crates/ply-codegen-spike/src/main.rs:558`) and checks only that the file
> **exists**. `benches/kernel/mcts.ply` and `work.ply` import no `std` module,
> so the kernel numbers are exposed to a stale interpreter and never to a stale
> stdlib; the served rows are exposed to both. `CONTRIBUTING.md` §"The binary is
> an instrument too" has the check and the full exposure list.

### What is assumed and not measured

That an MCTS kernel's hot loop falls mostly inside the spike's compilable
fragment. Everything below is ordered on that assumption and **§1 exists to test
it before anything is built.**

## 0. §1 RAN. The assumption held and the plan failed anyway.

> **Measured 2026-08-21.** `benches/adr0018-mcts.json`, one run, cranelift
> 0.134.3, load 3.54 falling to 3.05. Kernel: `benches/kernel/mcts.ply`,
> three-heap Nim, tree as `Map<Int, Node>`, UCB1 in integer fixed point. Agreement
> was taken **before anything was timed** — 1,344 generated cases across 21
> functions plus 24 whole-kernel searches, against both engines, 0 disagreements.

§1 said everything below it was ordered on one untested assumption: that an MCTS
kernel's hot loop falls mostly inside the spike's compilable fragment. It said to
rewrite this ADR if the fraction came back low.

**The fraction came back high — 81.0% of executed work, against 2–5% for an HTTP
request — and codegen still bought nothing.**

| compiled set | crossings | end to end |
| --- | ---: | ---: |
| floor, no JIT | 0 | 1.000× |
| entry only | 1 | 0.999× |
| outer loop | 102 | 0.996× |
| + playouts | 102 | 0.999× |
| **everything accepted** | 102 | **0.998×** |

0.998× is inside the method's own noise floor. The kernel ratio on the part that
is wholly inside the fragment and crosses zero times, `mcts.playouts`, is
**52.58×** — and reporting *that* would misprice the decision in exactly the way
ADR 0016 warned about.

**Why 81% inside and 52× there still buys nothing: the interpreter cannot call
compiled code.** Only compiled→interpreter exists (`rt_call_machine`). Every
function the fragment accepts whose *callers* it refuses is compiled and then
never entered. Compiling the twenty arithmetic functions between rungs 2 and 4
moved the whole program from 57,700 µs to 57,582 µs. In the hybrid, compiled code
is reached by exactly three functions: `mcts.iterate` (100 entries), `mcts.root`
(1), `mcts.best_action` (1).

It is **not** the boundary cost. 102 crossings total 9.9 µs, 0.017% of the run.

**The ceiling, by Amdahl over the two measured numbers and nothing else:**

| | |
| --- | ---: |
| a backend that could be *entered* from interpreted code | **4.86×** |
| an infinitely fast fragment | **5.26×** |

Not 11.67×, and not 52×. This ADR opens by saying Ply is off the Rust floor "by
roughly an order of magnitude" on MCTS. **The fragment as specified caps the
recovery at 5.26×**, so §2 onward cannot reach the goal §"Context" states, and
the binding constraint is architectural — one-way calling — rather than the size
of the fragment or the cost of the boundary.

### What is actually outside, ranked — this is the roadmap §1 promised

By lowered nodes removed from the fragment:

| nodes | fns | what |
| ---: | ---: | --- |
| 253 | 7 | a field access |
| 71 | 2 | a list pattern `[x, ..rest]` |
| 25 | 2 | unary `-` |
| 10 | 1 | a list literal `[]` |

> **Corrected (fragment widening, 2026-08-24): this is a FIRST-REFUSAL census,
> and read as a work list it is wrong by more than an order of magnitude.**
>
> The table is accurate and the section's title is not. It is titled *"What is
> actually outside, ranked — this is the roadmap §1 promised"*, which invites
> exactly one reading: remove the top row and seven functions, 253 nodes, come
> inside. That is not what happens, and the number to plan from is not in this
> table.
>
> Each row reports the construct named **first** when a function was refused, one
> function at a time. Two things break the arithmetic:
>
> * **A refusal names one construct, not all of them.** `mcts.empty_node` is
>   listed under unary `-` and also contains `[]`. Admitting unary `-` moves it
>   to the list-literal row rather than into the fragment.
> * **The fragment must be closed under calls** (`jit::Denotes::Uncompiled`
>   refuses the caller of anything it did not compile), so a function is only
>   admitted when every construct in it *and every function it reaches* is
>   admitted.
>
> Measured, one item at a time, `mcts --dir benches/kernel --only agreement`:
>
> | after admitting | fns inside | nodes inside | enterable | entries |
> | --- | ---: | ---: | ---: | ---: |
> | *nothing (baseline)* | 19 / 34 | 352 / 745 | 19 | 56,876 |
> | a field access | 20 / 34 | 363 / 745 | 19 | 56,876 |
> | + constructor patterns | 20 / 34 | 363 / 745 | 19 | 56,876 |
> | + unary `-`, `[]`, list patterns | **34 / 34** | **745 / 745** | **21** | 49,489 |
>
> **The 253-node row delivered 11 nodes and one function, and changed nothing
> about what executes.** The four rows are not four wins; they are one closure,
> and the whole of it arrives on the last item. A fifth construct nobody
> censused — a **constructor pattern** (`Some(n) ->`), which sits behind the
> field access in `mcts.node_at` and therefore never got to be a first refusal —
> had to be lowered as well.
>
> `benches/README.md` §"What `mcts` adds" carries the same correction, and
> `crates/ply-codegen-spike/tests/spike.rs`'s stdlib row for
> `std.http.parse_head` is the standing illustration: it used to be refused for
> "a field access" and is now refused for its call to `read_line`. Removing the
> named construct moved the name.
>
> **What did not move, and bounds all of it:** the 19.0% of executed work that
> is the `Map`, record and list machinery itself is still outside the fragment
> and is untouched by any of this. `rt_field`, `rt_record` and `rt_list` call
> `ply_eval`'s own representations; widening changes *which functions compile*,
> never what a `Map` insert costs.

And by *executed* work, **19.0% of the run is the `Map`/record/list machinery
itself**, which is outside the fragment no matter which functions compile.

Two hazards that belong to the fragment rather than to any function, so they
appear in no census: there is **no `Float` path** — accepted silently and raising
at run time — and **no nested-call bound**, where the machine answers
`recursion limit of 10000 nested calls exceeded` and compiled code SIGABRTs.

> **Corrected (R5, 2026-08-21): the nested-call half is closed; the `Float` half
> is closed at the boundary and still open inside the fragment.**
>
> The sentence above is quoted verbatim from before R5 and both halves of it were
> true when it was written. `benches/adr0018-mcts.json`'s
> `recursion_bound_compiled` holds the crash it describes:
> `the process died: signal: 6 (SIGABRT) — ... stack overflow`.
>
> *Nested calls.* Every compiled function now carries a four-instruction fuel
> prologue seeded from the budget `ply_eval::Compiled::enter` is handed — the
> machine's own `max_calls` minus its current depth. A body that would pass it
> fails, the entry declines, and the machine evaluates the definition and raises
> **its own** diagnostic. Both probes now answer
> `recursion limit of 10000 nested calls exceeded`
> (`mcts --dir benches/kernel --probe machine|compiled`), and
> `crates/ply-codegen-spike/tests/mcts_kernel.rs::a_runaway_recursion_is_the_machines_diagnostic_and_not_a_crash`
> asserts it as a subprocess, because an in-process assertion cannot observe its
> own `SIGABRT`. It is not free: that program takes **7.9 s** with a backend
> attached against **0.11 s** without, because the machine re-offers the same
> function at all ten thousand interpreted depths and each attempt burns its
> whole remaining fuel first — 19,992 entries and 10,000 fuel declines. Bounded,
> paid only by a program that is about to die, and recorded rather than fixed;
> `crate::entry::Declines::out_of_fuel` carries the reasoning.
>
> *`Float`.* The fragment still compiles `a + b` as `Int` arithmetic whatever the
> operands are, and
> `mcts_kernel.rs::the_fragment_accepts_float_arithmetic_and_then_fails_on_it_at_run_time`
> still passes. What changed is that it can no longer be *reached* from a
> program: `ply_eval::Compiled` carries `Int` and `Bool` and nothing else in
> either direction, and the spike will not register a definition whose declared
> signature is not `Int`/`Bool` throughout. Two independent refusals, and the
> same test now asserts that a `Float` call is never offered to the backend at
> all. Inside the fragment, called directly, the gap is exactly as described.
>
> > **Narrowed again (R5 audit pass, 2026-08-21): "it can no longer be reached
> > from a program" is one word too strong.** Neither refusal reads a definition's
> > *body*, so a `Float`, `Decimal` or `String` **literal inside an `Int` -> `Int`
> > body** passes both: `numerics.float_inside(n) = if 1.5 + 1.5 > 2.0 { n } else
> > { n * 2 }` is compiled, is registered as enterable, and is offered. What
> > happens then is the part worth writing down — the native body runs, meets the
> > constant at `rt_unbox_int`, fails, and the entry **declines**, so the program
> > gets the interpreter's answer and a slow call rather than a wrong one.
> > `crates/ply-codegen-spike/tests/hazards.rs::a_float_or_decimal_literal_inside_an_int_body_is_never_a_wrong_answer`
> > asserts the decline as an exact count, so a lowering that ever *answered* one
> > of these goes red.
> >
> > **Closed (fragment widening, 2026-08-24).** `jit::Fx::literal` now refuses a
> > `Float` or `Decimal` **literal**, so `float_inside`, `decimal_inside` and
> > `float_arith_inside` are refused at compile time instead of compiled,
> > offered, and declined at `rt_unbox_int`. The fragment's claimed coverage and
> > its behaviour now agree.
> >
> > *Refuse rather than build a path* was the choice, and the reason is scope,
> > not difficulty: a real `Float` path widens the agreement surface by `NaN`, by
> > `-0.0`, by float equality and by `Decimal` precision, and the workload this
> > fragment is being widened for — a self-hosted front end — is field accesses
> > and list patterns, not floating point. ADR 0018 §2 still asks for `Float`
> > unboxed; this closes the *dishonesty* rather than the §2 item.
> >
> > **What is still open, and it is the half a reader will assume is closed:**
> > only the literal-shaped case is refused. A `Float` arriving in an
> > `Int` -> `Int` body as a **builtin's return value** is refused by nothing —
> > it still meets `rt_unbox_int`, still fails, and still declines. That remains
> > a slow answer rather than a wrong one, which is the property
> > `hazards.rs::a_float_or_decimal_literal_inside_an_int_body_is_never_a_wrong_answer`
> > asserts; the test was rewritten in place, with its withdrawn assertions
> > quoted, and its direct `compiled_call("numerics.fadd", ...)` leg is unchanged.
> >
> > The census consequence in ADR 0019 §5 item 4 stands and is now demonstrated:
> > those three definitions are counted as compiled and cannot run. The audit's
> > third case for this hazard, ordering on `String`, is not reachable at all —
> > `E0201` refuses `a < b` on `String`, so the run-time support in `interp.rs` is
> > unreachable from a well-typed program (`tests/fixtures/string_ordering/`).

### What this changes

§2 through §7 below were ordered on the assumption §1 tested. The assumption held
and the conclusion did not follow from it, so the ordering is not rescued by
compiling more: **make the interpreter able to enter compiled code, or the
ceiling is 5.26× however much of the fragment you accept.** That is a different
first milestone from any listed below, and nothing below should start until it is
decided.

> **R5 did it, and §0.5 is the answer.** Entry works and is worth **6.199×** on
> this kernel — which is *above* the 4.86×/5.26× this section derives, so the
> ceiling above is withdrawn as a bound rather than confirmed. Read §0.5 before
> §1: it also records that no shipping command can install a backend, so none of
> the 6.199× is reachable by any user of Ply.

## 0.5. Entry was built. The answer is 6.199×, and this ADR's ceiling was wrong.

> **Measured 2026-08-21 (R5), pre-registered 2026-08-21 before any number
> existed.** Experiment: `benches/r5-timing/PRE-REGISTERED.md`. Result:
> `benches/r5-timing/RESULTS.md`. Raw report: `benches/r5-timing/mcts-r5.json`,
> cut only by `benches/r5-timing/analyze.py`. Same kernel as §0
> (`benches/kernel/`), same box, cranelift 0.134.3, `+1.94.0`. Agreement was
> taken before anything was timed: 2,396 generated cases over 29 functions and
> 24 whole-kernel searches, 0 disagreements, against both the no-backend machine
> and the independent tree-walker.

§0 closes: **"make the interpreter able to enter compiled code, or the ceiling is
5.26× however much of the fragment you accept."** R5 made it possible. This
section is the answer.

### What was built

`crates/ply-eval/src/compiled.rs` — a `Compiled` trait taking a name, some
scalars and a call budget and returning at most one scalar. No arena, no stack,
no handler stack, no host binding, no `&mut Machine`, no route back in, so a
backend that cannot finish has changed nothing observable and declining is free
by construction. The hook is one branch in `Machine::enter_code`. Entry
demonstrably happens: with nothing enterable the search offers 45,586 calls and
enters 0; with everything the fragment accepts it enters **2,162**.

### The cost side, on the workload that ships

**0.0 allocations per `/health` request.** `HOOK` (the tree as it ships, no
backend) against `NOHOOK` (the same tree with the `enter_code` call site
deleted), two binaries from one frozen tree, arms alternated `H N H N H N`: both
read **773.4** at the 200-request window, byte-identical including
`bytes_per_request`, and identical again at 20 and at 2,000 requests, so the
delta is flat in the window rather than absorbed by it. By the pre-registered
rule (`HOOK − NOHOOK > 0.0 allocations` ⇒ regressed) it is **not a regression**.
In the linked binary the hook is 80 bytes of machine code.

Zero allocations is not zero cost, and the load-independent count that says so:
instrumented, one `/health` request reaches the hook **237.87 times**, and every
one is a miss — `compiled_answer` exits on its first line. **The wall-clock cost
of those 237.87 branch tests was never taken**, on either binary, although both
existed. The currency was pre-registered as allocations and a wall-clock rule
was not, and this ADR should not be read as if it had been.

### The kernel ratio

Load gate 2.63 against a pre-registered 4.5 on a 10-core box — that reading is
prose only and appears nowhere in `mcts-r5.json`, whose provenance line records
`2.78 3.34 4.66` and whose first ladder window records 2.52; the gate passes on
all three. All 84 ladder windows sampled between 2.40 and 2.91, so **no window
was dropped by either pre-registered filter on any rung** — which also means
neither filter was ever tested, and on macOS the 1-minute average updates every
~5 s against a ~190 ms window, so the load filter can only drop a whole rung or
none. Controls, both required in 0.95–1.05:
harness floor **0.9995×**, nothing-enterable rung **0.9758×**.

| rung | ratio | 10th–90th | entries/call |
| --- | ---: | --- | ---: |
| control: nothing enterable | 0.976× | — | **0** |
| the exploration term | 2.860× | [2.835, 2.871] | 1,275 |
| + the playout | 6.176× | [6.139, 6.197] | 2,161 |
| **everything the fragment accepts** | **6.199×** | **[6.143, 6.226]** | **2,162** |

**6.199×, 10th–90th percentile [6.143, 6.226], over 21 of 21 surviving paired
windows, 2,162 native entries.** Pre-registered verdict **`entry-paid-off`**.

> **Not re-taken on cranelift 0.132.3, and the reason is a blocked instrument
> rather than a busy machine (2026-08-31).** The spike moved from cranelift
> 0.134.3 to 0.132.3 so that it builds on this repository's pinned 1.93.1. The
> obvious question — does the older cranelift generate materially worse code —
> was pre-registered before the port with this section's own command, statistic,
> window filter, load gate and controls taken over verbatim, and then **could
> not be run**: `mcts` verifies agreement before it times anything and `bail!`s
> on the first disagreement, and the agreement corpus is currently red at **42
> disagreements**, so `--iterations 100 --inner 3 --repeats 21` exits 1 without
> reaching a clock. `CONTRIBUTING.md` §"Things known to be broken" item 18 has
> the defect.
>
> **The red is not the port's.** It reproduces on cranelift 0.134.3 built with
> `+1.94.0` from unmodified source, and the ported build's agreement output is
> byte-identical to the unported one — same `md5
> c0893d75e378b64339b8ec0746e95220`, same 42 disagreements, same per-function
> entry counts. So the port is a null result on everything this instrument can
> still see, and **6.199× stands as a 0.134.3 figure that no one has yet
> checked on 0.132.3.** Nothing here should be read as evidence that the two
> versions generate equally fast code; it is evidence that they generate the
> same *answers*, which is a different claim.
>
> The machine also failed this section's own load gate at the moment of the
> attempt — 1-minute average 4.65 against the pre-registered 4.5 — so even a
> green corpus would have produced `refused-machine-busy` rather than a ratio.
> Both facts are recorded because either alone would have been enough.
>
> > **A cranelift 0.132.3 number on this kernel now exists, and it is not this
> > one (2026-08-31).** `crates/ply-codegen` compiles `benches/kernel` from a
> > shipping command, and `ply test benches/kernel --backend cranelift` is
> > **4.871×** against no backend — min of 21 windows, arms interleaved, null
> > control at 1.008×/1.004×, load 5.8 against a 4.0 gate so an observation and
> > not a figure. `--backend reference` on the same series is 3.217×.
> >
> > **It does not replace 6.199× and must not be quoted as a re-take of it.**
> > Four differences, and each of them widens the denominator: this measures a
> > whole `ply test` invocation — front end, hashing, cache check, test harness
> > — where 6.199× measures the kernel body alone; it pays 13.2 ms of JIT
> > compilation inside the window; it runs the corpus's eight tests rather than
> > `--iterations 100 --inner 3`; and it enters 2,974 times rather than 2,162.
> > The two numbers are not in tension and neither is evidence about the other.
> >
> > What it does settle is a smaller thing this ADR could not: **the fragment
> > over this kernel is intact under 0.132.3 and under a fresh implementation of
> > the seam.** `crates/ply-codegen/tests/kernel.rs` compiles all 44 of the
> > kernel's definitions as one closed unit with **zero refusals**, registers 25
> > of them as enterable, and the search answers through compiled code. The
> > blocked instrument above is still blocked and item 18 is still open.
> >
> > > **Re-taken by an adversarial review the same day, and it replicates:
> > > 4.927x** against no backend, `--backend reference` 3.247x, null control
> > > 1.005x/1.003x, N = 15 -- with the five arms **rotated one slot per window**
> > > rather than run in a fixed order inside each rotation, which is the one
> > > design change the review made and the one that would have exposed a
> > > position confound had there been one. Load 8.63 -> 8.15, so an observation
> > > on the same terms. Every caveat above survives unchanged: this is still
> > > not a re-take of 6.199x, still a whole-`ply test` denominator, and item 18
> > > still blocks the instrument that produces 6.199x. This is now the only
> > > compiled-Ply speedup in this repository that two people have measured
> > > independently.

<!-- Corrected in place (adversarial review, 2026-08-31): this paragraph is the
tail of the "**6.199x, 10th-90th percentile ...**" paragraph above the inserted
blocks, written before them. The 2026-08-31 port inserted its blockquote in the
middle of that paragraph without a blank line after it, and Markdown's lazy
continuation then rendered these five lines INSIDE the quoted block -- so
6.199x's monotonicity and its two independent replications read as part of a
block about a measurement that was never taken. Nothing is reworded; the blank
line restores them to body text. The identical mistake was made in
docs/adr/0026-a-reachable-backend.md 4.7 by the same change and is corrected
there too. -->

The ladder is monotone in entries and the aggregate is filter-independent. Two
reviewers replicated the top rung independently on busier machines — 6.215×
at load 5.3–6.8 and 6.240× at load 12–16, both formally void under the
pre-registration's own load rule and both reported here as direction and
magnitude rather than as a second result.

Pre-R5 the same rung was **0.998× with 0 entries**. §0's diagnosis — that the
binding constraint was architectural, one-way calling, and not the size of the
fragment or the cost of the boundary — is **confirmed**.

### 6.199× is above this ADR's own ceiling, and that is a defect in the ceiling

§0 puts the Amdahl ceiling at **4.86×** for a backend that can be entered and
**5.26×** at an infinitely fast fragment. The R5 run recomputes them from its own
attribution as 4.806× / 5.212×. **The measurement is above both**, taken with
19 of 34 kernel functions accepted — nowhere near "however much of the fragment
you accept". A result that beats its own predicted ceiling is a sign that the
model was wrong, not that the result is extra good, and the model was wrong in a
way this ADR can name:

- interpreted, the search offers **45,586** calls to the hook per
  `mcts.plan_753(100)`;
- with everything compiled it offers **2,266** (2,162 entered, 104 declined);
- so **43,320 interpreted calls per search stop existing**, because they now
  happen inside a native body.

The ceiling's denominator was built by pricing each function's **body** in
isolation — `per_call` subtracts the machine's own entry cost — which charges the
call-site machinery to the 19.2% "unattributed" bucket rather than to any
function. The "81.0% of executed work inside the fragment" figure is a
body-only accounting, and entering compiled code deletes the call-site machinery
too: the argument vector, the frame push, the `Env` binding. At the measured
machine entry cost alone (0.0989 µs) those vanished calls are 4,284 µs of a
57,329 µs search, **7.5%**, and arrival is the *cheapest* part of an interpreted
call. **§0's ceiling is an artifact of a body-only attribution, not a bound, and
this ADR withdraws it as a bound.**

**The same fact turned up as a correctness defect, from the other side.** A
compiled body pushes **one** `Frame::Call` for a whole call; the interpreter
pends a frame per pending operand as well. That is the unattributed machinery,
counted. It is also why `probe.hog` below diverges, and the two are one fact.

### What entry also bought: a divergence, with the real backend and no mutation

`fn hog(n: Int) -> Int = if n == 0 { 0 } else { hog(n - 1) + 1 + 1 + ... }`, 150
`+ 1` terms, is accepted whole by the fragment. `hog(9000)` gives
`Err("recursion limit of 1000000 pending frames exceeded")` on the machine alone
and **`Ok(1350000)`** with the backend attached — one entry, zero declines,
`ply_eval::compare_answers` calling it a divergence. The seam passes only
`budget = max_calls - stack.calls()`; the machine's second bound,
`DEFAULT_MAX_FRAMES`, cannot be expressed at the boundary and no backend can
honour it. `CONTRIBUTING.md` §"Things known to be broken" item 9 carries it, and
item 10 carries the older defect it uncovered: the same program is an
`--engine both` divergence **with no backend at all**.

> **Superseded (2026-08-24): both are fixed, and the sentence above describing a
> "second bound" no longer describes the machine.** There is one bound,
> `DEFAULT_MAX_CALLS`, and `budget` expresses all of it. The frame count was a
> resource guard on the machine's own heap that had been phrased as a program
> answer, and it was sensitive to how a body's operands were spelled rather than
> to what the body did: at `hog(9000)`, `hog(n - 1) + 150` answered where
> `hog(n - 1) + 1 + 1 + ...` raised. `Machine::with_max_frames` keeps it as an
> opt-in ceiling, and a machine holding one now enters no compiled body at all,
> so nothing this seam admits can turn on a limit it was not handed.

Two more the reviews found, both open and both in that section: every entry into
the spike's backend costs O(the previous entry's peak arena) — item 12 — which is
the real mechanism behind a per-function regression `RESULTS.md` §3 blamed on its
own filter; and a definition that discharges its own effects publishes an empty
row, so the seam's purity gate clears it and offers it (item 11).

> **Both are closed now (2026-08-24), and the sentence above is left as it was
> written.** Item 12 was fixed by `Ctx::end` clearing the arena at the end of the
> entry that filled it. Item 11 was fixed by publishing a second fact rather
> than a second row: `ply_core::DefInfo::internally_effectful`, transitive over
> the call graph, refused at the seam by `Gate::InternalEffects`. "The seam's
> purity gate" is now two gates, and a definition that discharges its own
> effects is refused by the second one while its empty published row — which is
> correct, since nothing escapes — clears the first.

### Nothing here ships, and that is the load-bearing sentence

**No shipping command can install a backend.** `Compiled` and `set_compiled`
appear nowhere in `ply-cli`; outside `crates/ply-eval`'s own tests and the
deletable spike, `set_compiled` has no caller in `crates/*`. `ply test --engine
both` cannot attach one and therefore catches **none** of the eight deliberately
wrong backends the spike's mutation harness runs. The rule that a run with a
backend attached is a third execution strategy whose results the result cache
must not keep is stated in `Machine::set_compiled` and is **not enforced,
because it is unreachable** — `cache_bypassed` has no `Machine` in scope. So
**none of the 6.199× is available to any user of Ply.** It is a measurement at a
seam only the spike's harness and `ply-eval`'s differential corpus can reach.

### So are §2 through §7 correctly ordered?

**No, and not for the reason §0 gave.** §0 said the ordering was not rescued by
compiling more because the ceiling was 5.26×. That ceiling is withdrawn above.
What replaces it is worse for the list, not better:

1. **§0's own first milestone is discharged as an experiment and not as a
   feature.** "Nothing below should start until it is decided" — it is decided:
   entry works, it is worth 6.199× on this kernel, and it is unreachable from
   every shipping command. Making it reachable is M9, and ADR 0016 §3.5 requires
   the spike be deleted rather than promoted, so the next decision this ADR owes
   is not a lever at all.
2. **§2 is spent** and was already corrected in place by R4: there is no
   primitive boxing to remove.
3. **§3 is unpriced by this measurement and must not be read as re-ranked.**
   `benches/kernel` declares **no effect at all**, so no entered definition ever
   exercised a handler, the purity gate or the effect path. R5 says nothing
   whatever about evidence passing.
4. **§4 is the largest lever this ADR still identifies for this kernel**, by
   §0's own unchanged number: 19.0% of executed work is the `Map`/record/list
   machinery, which is outside the fragment no matter which functions compile,
   and is therefore what caps a kernel that *can* enter compiled code. That is
   inference from a pre-R5 attribution, not a new measurement, and it should be
   re-taken on a hybrid run before anything is built on it.
5. **§5's sequencing note is void**: it reads "after §2", and §2 does not exist.
6. **§6 and §7 are untouched.** Nothing in R5 bears on shared mutable state or
   on the host boundary.

The honest summary: the ordering below was derived from a model that undercounts
what entry removes, and every remaining item is still priced by that model. No
lever in §2–§7 has an end-to-end price on a kernel that can enter compiled code.
That is what an amendment to this ADR owes, and this section does not pay it.

### What a reader still does not know

- **The wall-clock cost of the hook on the request path.** Only allocations were
  taken. 0.0 allocations must not be read as 0 cost.
- **What a backend costs in allocations.** No allocation figure was taken with a
  backend attached anywhere, and `compiled.rs` warns that a `w6-alloc` figure
  from a run without one may not be quoted for a run with one.
- **Whether 6.199× holds anywhere else.** One kernel, one program, one box, one
  pre-registered run; the pre-registration forbade re-running.
- **What JIT compile time costs.** It is in no ratio; every window times warm
  code.
- **Anything about the seam under the rest of the language.** Effects,
  handlers, continuations captured across an entry, `simulate`, secrets,
  `Float`/`Str`/`Decimal`/`Bytes`, higher-order closures, the store, the test
  cache, `ply prove` and the host path were **not exercised with a backend**.
- **Whether the fragment's own lowering is correct.** `jit.rs`'s instruction
  selection was read and never mutated; every wrong backend R5 built corrupts
  *answers*, not lowering.
- **How strong the agreement result really is.** 12 of the 19 compiled
  functions are offered to the backend **zero** times during the 24 whole-kernel
  searches, and a uniform off-by-one in `mcts.ucb` — 1,268 wrong scores —
  changes no move in any of them. Half the entered functions are caught by their
  own generated cases and by nothing else.
- **Whether the oracle is right.** If the tree-walker and the machine were wrong
  in the same way, every comparison here agrees.

### One correction to §1's discharge block, while this is open

`benches/adr0018-mcts.json` still holds the **pre-R5** numbers, including an
`end_to_end` of 0.998× measured with zero entries; R5 wrote to
`benches/r5-timing/mcts-r5.json` and did not overwrite the artifact this ADR
quotes. The two disagree on the fragment's shape, and the newer file is right:
removing the trampoline made a compiled set closed under calls, so `mcts.search`,
`mcts.plan` and `mcts.plan_753` are now refused by name and the census moved from
**22 of 34 functions and 386 of 745 lowered nodes to 19 and 352**. §1's block
below still says 22 and 386, as does `docs/adr/0019-value-representation.md` in
two places — its measured-figures table and its §5. `benches/README.md` already
carries the correction in place. Corrected here because the number is §1's
premise; the executed-work share is unmoved (81.0% against 80.8%).

## 1. Re-price the spike against a compute kernel — do this first

> **Discharged (R4, 2026-08-21), and it changed the ordering of everything
> below.** The measurement this section asked for is
> `benches/adr0018-mcts.json`, written by the command in `benches/README.md`
> §"What `mcts` adds"; the kernel is `benches/kernel/`, in Ply, and it passes
> `ply test benches/kernel/ --engine both`. The assumption held on **shape** —
> 22 of 34 kernel functions, 386 of 745 lowered nodes, and **81.0% of the
> kernel's executed work** are inside the fragment, against the 2–5% ADR 0016
> measured for an HTTP request. The conclusion it was supposed to license did
> **not** hold: end to end the hybrid is **0.998× [0.979–1.007]** against a
> harness floor of 1.000× [0.994–1.009], because **the interpreter cannot call
> compiled code** — a function the fragment accepts whose callers it refuses is
> compiled and never entered. The Amdahl ceiling over the two measured numbers
> is **4.86×**, not the 11.67× this ADR carries for the spike and not the
> 52.58× the fragment shows where it does run. `docs/adr/0019-value-
> representation.md` §5 is the write-up and lists the six things an amendment
> to this document owes; it does not make them, and neither does this block.


The spike measured **11.67× minimum** on arithmetic, comparisons, `if`, `let`,
`block`, and `match` on literal patterns. ADR 0016 concluded 1.02–1.05× end to
end because that fragment is 2–5% of an HTTP request.

An MCTS inner loop — select by UCB, increment a visit count, accumulate a score,
compare against a bound — is close to *entirely* that fragment. If it is, the
end-to-end number is not 1.05× and the whole ordering below changes.

**The work:** write an MCTS kernel in Ply, point `crates/ply-codegen-spike` at
it, and report the same conservative interpreter-best ÷ spike-worst ratio the W6
spike reports, plus the fraction of the kernel the spike refuses by name.

**Cost:** small. The spike exists, is standalone, and `rm -r` removes it.

**Why first:** it is the cheapest measurement with the highest information, and
it tests the one assumption everything else rests on. If the compilable fraction
is low, §2–§4 are the wrong plan and this ADR should be rewritten rather than
executed.

## 2. Unboxed primitives

> **Corrected in place (R4, 2026-08-21). Both sentences of "The gap" below are
> wrong, and they are left standing verbatim underneath this block because
> `CONTRIBUTING.md` §"Correct, do not delete" wants the withdrawn claim beside
> the measurement.** This ADR opened §2 by reasoning from them, and a milestone
> was requested on their strength.
>
> **"Every `Int` is a heap-allocated `Value`" is false.** `Int`, `Bool`,
> `Float`, `Unit`, `Decimal`, `Cell` and `Task` are inline variants of the
> `Value` enum (`crates/ply-eval/src/value.rs:50-104`) and building one touches
> no allocator. `size_of::<Value>()` is 32 bytes. Printed by name, by a test
> that runs:
>
> ```
> cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture
> ```
>
> > ```
> > -- what one Value costs to build --
> >   Value::Int       0 allocations
> >   Value::Bool      0 allocations
> >   Value::Float     0 allocations
> >   Value::Unit      0 allocations
> >   Value::Decimal   0 allocations
> > ```
>
> **There is no primitive boxing in this evaluator to remove**, so the plan
> below — "a tagged representation where `Int`, `Bool` and `Float` live inline
> in the value word rather than behind a pointer" — describes what the tree
> already does.
>
> **"`interp::literal` allocates 111 times per request" attributes to the wrong
> thing.** The count was real; the conclusion drawn from it did not follow.
> `interp::literal` cannot allocate at all unless the literal is a `Str` or a
> `Bytes` (`crates/ply-eval/src/interp.rs:1000-1010`), and the 111 was read off
> a **20-request window**: it fits to 65.0 per request plus 925 once per
> `Machine`, and 65.0 + 925/20 = 111.25. One-time work divided by twenty looks
> exactly like a per-request cost, which is the failure `CONTRIBUTING.md`
> §"Measure an ADR's motivating claim before accepting the ADR" closes with.
>
> **What the request actually spends its allocations on** was measured instead,
> attributed to the value being built rather than to the frame that built it and
> fitted over two windows: the largest line was the **call-argument vector**, at
> 372.4 per request, 40.9%. R4 landed a free list for it and built a
> compile-time constant's `Value` once, and `/health` went from **1,082 to 773**
> allocations per request (`./target/release/w6-alloc --repo . --requests 200`).
> `docs/adr/0019-value-representation.md` is that ADR; its §4 **rejects**
> narrowing `Value`, with the number that would have justified it, which is
> zero allocations.
>
> **§2's success criterion is also spent.** It reads "a `w6_alloc_sites` re-run
> showing `interp::literal` gone from the top sites". `interp::literal` is now
> 0.0 allocations per request on both routes — and the profile's top line is
> unmoved, because it never was `interp::literal`.
>
> **This ADR is not otherwise amended, and §5 below is untouched.** ADR 0019 §5
> lists what an amendment owes, including the item that outranks everything in
> this section: a backend the interpreter cannot *enter* buys nothing whatever
> the representation is, measured at 0.998× end to end.

**The gap.** Every `Int` is a heap-allocated `Value`. `interp::literal` allocates
111 times per request on a workload doing almost no arithmetic. MCTS is visit
counts and score accumulation in the innermost loop.

**The plan.** A tagged representation where `Int`, `Bool` and `Float` live inline
in the value word rather than behind a pointer, with heap allocation only for
compound values. This is the change ADR 0017 was *supposed* to enable and did
not, because the allocations were never in the world.

**What it interacts with.** `ply-hash` normalization, the store codec and its
schema fingerprint, `--engine both` (both engines must agree on the new
representation), and `Value::render`. The schema fingerprint will need bumping;
W2's `Map` landing is the precedent for how.

**How to know it worked.** Allocations per `/health` against 1,082, and a
`w6_alloc_sites` re-run showing `interp::literal` gone from the top sites.

**Unmeasured:** whether tagging costs more in branch overhead than it saves in
allocation. Measure on the kernel from §1, not on the HTTP path.

## 3. Handler specialization (evidence passing)

**The gap.** `random.gen()` walks the handler stack. MCTS calls it in the
innermost loop, millions of times. Effects are the language's central
abstraction and they are dynamically dispatched.

**The plan.** Where the handler for an operation is statically known at a perform
site, pass the evidence directly and inline the clause, so the effect disappears.
This is the effect-system analogue of monomorphization, and it is what would make
Ply's defining abstraction free rather than merely cheap.

**Precedent.** Koka does this. ADR 0010 already named evidence passing as the
mechanism that would let a resolution layer elaborate away before hashing.

**How to know it worked.** Time a `random.gen()`-dominated loop with the handler
statically known versus installed dynamically. The ratio is the win.

**Unmeasured:** what fraction of performs in a real kernel have a statically
known handler. If most handlers are installed at an entry point and the kernel
performs against them, the fraction should be high — but that is a guess.

## 4. Unboxed mutable arrays

**The gap.** MCTS wants a flat node array with index-based children, not a
pointer-chasing tree of ADTs. Ply has `List`, `Map` and records — all persistent
or boxed. There is no primitive array with in-place update. Regions give scoped
lifetime, not a contiguous unboxed buffer.

**The plan.** A primitive `Array<a>` with in-place update, region-allocated, with
its brand preventing escape exactly as a cell's does. In-place update requires
unique ownership, **which R1/R2's region work does not establish** — it is a
missing prerequisite of this item rather than machinery the item inherits, and
unless it is built the array copies exactly where a mutable array is supposed to
be worth having.

> **Corrected (mechanism sweep, 2026-08-28): a prerequisite was booked as
> already paid.** This read *"In-place update requires unique ownership, **which
> is what R1/R2's region work already establishes** — this is the first feature
> where that machinery pays for itself on performance rather than on safety."*
> The first clause is right; the attribution is not. Regions establish
> **non-escape**: a brand stops a value outliving its region, which is a safety
> property about lifetime. Non-escape is not non-aliasing. A region-allocated
> value can be aliased freely *inside* its region, and that is exactly the case
> that makes an update copy — `push` probes with `Arc::get_mut`
> (`crates/ply-eval/src/builtins.rs`) at the moment of the update and copies the
> whole value when anything else can still see it, and the commonest way a
> second owner gets there is `rc::carry` (`crates/ply-eval/src/rc.rs:98`), which
> hands a pending frame a live clone of the scope whenever any sub-expression of
> the enclosing node remains. That is a positional fact about the call site and
> its caller, entirely within one region, and no brand touches it.
>
> ADR 0017 §4 now says the same thing about itself: it makes in-place update
> *available*, not guaranteed, and establishes no complexity claim. ADR 0017's
> Context block had already warned about this shape once — the region work "was
> never an allocation claim" — and this sentence is that claim being re-derived
> one document over, which is how an unmeasured premise travels.
>
> **It bears on the ranking.** §0.5 item 4 calls §4 the largest lever this ADR
> still identifies for this kernel. An `Array<a>` built on "the regions already
> did it" would ship the same dynamic probe under a mutable-looking type, so this
> item's real cost includes establishing uniqueness — statically, or by a rule an
> author can check locally — and neither exists today. Until it does, §4 is a
> larger item than it is priced as, not a smaller one.

**What it interacts with.** The escape check must run on resolved types (W2 found
the analogous hole reachable through a type alias). Derivation must refuse or
handle arrays deliberately. `--engine both` must agree.

**How to know it worked.** A node-array MCTS kernel against the same kernel built
from ADTs, on allocations and wall clock.

## 5. Monomorphization

**The gap.** A generic `Node<S, A>` stays boxed and dynamically shaped where Rust
emits flat structs.

**The plan.** Specialize generic definitions at their concrete instantiations.
Content addressing makes the bookkeeping natural — a specialization is a
definition with its own hash, cached and invalidated like any other.

**The cost to watch.** Definition-count inflation, which ADR 0010 already flagged
for derivation and which the incremental front end absorbed (10,000 definitions
check from scratch in 0.45 s). Monomorphization multiplies harder than derivation
does. Measure the definition count and cold-check time, not just the speedup.

**Sequencing:** after §2, because monomorphizing over boxed values buys much less.

## 6. Parallel rollouts and shared mutable state

**The gap, and it is a design gap rather than an optimization.** Tasks are
suspended machine states with their own region stacks — heavier than a rollout
worker needs. Worse, MCTS wants many workers mutating shared tree nodes, and the
region brand refuses exactly that. There is no vocabulary in the language for an
explicitly shared, interior-mutable arena.

**The plan.** This needs its own ADR and should not be improvised. The shape worth
exploring: an explicitly declared shared region whose values are reachable from
multiple tasks, where the isolation guarantee is *deliberately* relaxed and the
relaxation is visible in the type — so a reader can see which state is shared and
the scheduler can stop pretending those tasks are independent.

**What must not happen.** A silent relaxation. W4's worst defect was a pooled
connection coupling two tests the footprint graph believed disjoint; a shared
arena is the same hazard with a different name, and it must be declared rather
than inferred.

**Consolation for the interim:** the deterministic simulation is a genuine
advantage here. Seeded rollouts replay exactly, and footprint-guided reduction
found the `bank.ply` race in 2 interleavings with the seed as the repro.
Debugging a nondeterministic parallel tree search in Rust has no equivalent.

## 7. SIMD, layout control, and an escape hatch

**The gap.** Rust MCTS implementations lean on all three. Ply has none.

**The plan: mostly don't.** An `unsafe` escape hatch punctures the effect
system's central claim that the runtime knows what code can do, and ADR 0008 §2
already establishes the honest alternative — a **host handler** with a declared
footprint, a determinism flag, a linearity obligation, and a line in `ply hosts`.
The boundary costs 0.5 µs per crossing, measured, which is nothing at kernel
granularity.

So: SIMD kernels and layout-sensitive inner loops go behind host handlers, in the
trusted computing base, enumerable in one command. That is a better answer than a
language-level escape hatch because it is *listable*.

## The middle path, available today

Write the MCTS kernel in Rust behind a host handler, and the strategy, evaluation
and experiment harness in Ply. That is precisely what W1's boundary was built
for. You get simulation, specs, exact test selection and bisection over the part
of the system where correctness is hard, and native speed over the part where it
is not.

This is not a concession — it is the design working. But it should be stated as
the current answer so nobody discovers it by disappointment.

## Sequencing

> **Audit note (R5, 2026-08-22): this list is stale in three places and §0.5 is
> why.** Item 1 is discharged (twice: R4 and R5). Item 2 was refuted by R4 —
> there is no primitive boxing to remove — which also voids item 6's "after §2".
> Item 5, "re-price codegen again", was done and produced a number **above** this
> ADR's own ceiling, which §0.5 withdraws. What the list does not contain, and
> what §0.5 says is now owed first, is a decision about whether a backend is ever
> reachable from a shipping command: today `set_compiled` has no caller in
> `crates/*` outside `ply-eval`'s tests and the deletable spike. The order below
> is left standing verbatim because it is what this ADR was written from.

1. **§1, re-price the spike on a kernel.** Cheap, and it tests the assumption
   everything else rests on.
2. **§2, unboxed primitives.** Largest single gap; nothing else matters until it
   closes.
3. **§3, handler specialization.** Makes the defining abstraction free.
4. **§4, unboxed arrays.** First feature where the region work pays on
   performance.
5. **Re-price codegen again.** ADR 0016's verdict was about the old
   representation; §2–§4 change exactly what made its ceiling low.
6. **§5 monomorphization**, then **§6 shared state** as its own ADR.

## What would make this ADR wrong

If §1 shows an MCTS kernel is mostly *outside* the spike's compilable fragment,
the ordering above is wrong and codegen is not the endpoint. Rewrite rather than
proceed.

If §2 lands and allocations do not move, the same reasoning that failed for
regions has failed again, and the next step is another attribution run rather
than the next item on this list.
