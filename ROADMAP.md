# Ply — Roadmap

> **How to read an exit criterion here (docs pass, 2026-08-17).** "Demonstrable"
> below means *a test demonstrates it on a machine that can run that test*. Four
> conditional skips exist, and each returns a **passing** result when its
> dependency is absent:
>
> | gate | where | when it skips | says so? |
> | --- | --- | --- | --- |
> | `cluster::available()` | `crates/ply-host/tests/support/cluster.rs:38` | no `initdb`/`postgres` on the machine | yes, on stderr of a passing test |
> | `PLY_PG_URL` | `crates/ply-host/src/db/scope/tests/live.rs:101` | the variable is unset — and **nothing in the repo sets it** | yes, on stderr of a passing test |
> | `#![cfg(unix)]` | `crates/ply-cli/tests/w5_shutdown.rs:18` | non-Unix host | **no — the file is not compiled and nothing is printed** |
> | its own `[workspace]` | `crates/ply-codegen-spike/Cargo.toml` | always, under `cargo test --workspace` | no |
>
> `cargo test --workspace` green therefore proves the W4 database claims only on
> a machine with postgres, the W5 shutdown claims only on Unix, and none of ADR
> 0016's spike claims anywhere. Four required properties are enforced by nothing
> at all: ADR 0014 required tests **14** and **16**, ADR 0015 required test
> **18**'s harness, and the second clause of ADR 0013 required test **26a**.
> Each is annotated in place. Milestone entries below carry an audit note where
> their criterion depends on one of these.

Each milestone has an exit criterion that is demonstrable, not aspirational.
M0–M4 are the vertical slice: the smallest system that proves the thesis end to
end. M5+ are the milestones the slice's architecture is shaped to accept without
a rewrite.

**Status: M0–M8, W1–W6, and R1–R3 are complete.** The web track closed at W6 with
a measured decision rather than a build. The region track that followed it landed
in three parts — R1 built the machinery, R2 connected it, R3 took the
compile-time work back off the request path — and it removed the forkable world
M6 shipped; see the region track below, and read M6's entry with that correction
in hand. **R3 ended on a decision rule fixed before it started, and the rule went
against the design**: allocations per `/health` are still above the pre-region
baseline, so whether the regions trade was worth making is open rather than
settled. §R3 is where that is recorded and it is not a formality.

**M9 is the one milestone deferred on a measurement.** W6 deferred it against
criteria fixed in code before any number existed, and the deferral carries the
numbers that would reopen it, so a future contributor re-measures rather than
re-argues — see the M9 and W6 entries. It is *not* the only thing unbuilt: ADR
0017 "Not in this ADR" names four separate milestones, three of which come
*before* M9 and one of which is M9. **[What is next](#what-is-next)** is at the
foot of this file and is the entry a reader continuing this project should start
from.

> **Audit note (docs pass, 2026-08-17): this line read "M0–M8 and W1–W6 are
> complete" and "M9 is the one milestone still deferred".** Both were written
> before the region track and neither was updated by it. R1 and R2 had landed —
> `World` is gone from `ply-eval`, `Isolation::World` is `Isolation::Region`
> (`crates/ply-test/src/schedule.rs:101`), and both engines consult
> `region_kind::infer` on the evaluation path — and this file mentioned neither
> milestone anywhere in 456 lines. A roadmap that omits the milestone which
> deleted a subsystem two other milestones are described in terms of is the
> failure mode this audit exists for.

---

## M0 — Foundation

Workspace, source management, diagnostics.

- `ply-span`: `SourceId`, `Span`, `SourceMap`, `Diagnostic`, severity, labels
- Rendering via `ariadne`; every diagnostic also serializes to JSON
- Error code registry (`E0xxx`) so codes are stable across releases

**Exit:** a diagnostic with a code, span, and label renders to a terminal and to
JSON from the same value.

## M1 — Core language

- Lexer, parser, AST for: literals, `let`, lambdas, application, `if`, ADTs,
  pattern matching, records, top-level `fn` / `type` / `effect` / `test`
- Hindley–Milner inference with let-polymorphism, over types only
- Tree-walking evaluator with closures and pattern matching

**Exit:** `ply run` evaluates a non-trivial pure program; `ply check` infers and
prints principal types.

## M2 — Effects

- `effect` declarations with `read` / `write` modes, `[r]` resource parameters,
  `nondet` marker
- Effect rows as ground-atom sets with a tail variable; row unification with
  occurs check
- Inference extended so every expression carries a row; annotations checked as
  upper bounds
- `handle` / perform, tail-resumptive; the handle typing rule from DESIGN.md §2
- Region-scoped cells (`with_cell`) so handlers can carry state

R2 made "region-scoped" literal: a `with_cell[r]` **is** a region, its cell is a
slot in a bump arena, and the surface syntax is unchanged so existing programs
did not move — but the escape rule tightened and three shapes that compiled under
M2 no longer do. See the region track.

**Exit:** `ply check` prints `fn f() -> T / {db.read[users]}` for an unannotated
function, and a handler that discharges `db` visibly removes those atoms from the
caller's row.

## M3 — Content addressing

- Normalization: de Bruijn locals, hash-substituted references, erased names
- Dependency graph, Tarjan SCC, component hashing
- `ply-store`: content-addressed definition store + namespace + result cache
- `ply hash` / `ply deps` for inspection

**Exit:** renaming a top-level function changes zero definition hashes; editing
one function body changes exactly its hash and those of its transitive
dependents.

## M4 — The test system

- `test` / `test/nondet` as language constructs
- Selection by cache miss on `test_hash`
- Footprint conflict graph → concurrency groups → parallel execution via `rayon`
- Determinism check: `nondet` atoms surviving in a `det` test's row is `E0412`
- Reporting: human summary, `--json`, `--explain` for selection and scheduling

**Exit — the demo:**

```
$ ply test
47 passed (2.1s)

$ # edit one function body
$ ply test
selected 3 of 47 (44 cached) — 3 passed (0.08s)

$ # rename a top-level function
$ ply test
selected 0 of 47 (47 cached) — rename changed no definition hash
```

---

## M5 — Machine-shaped failure

Delta-debugged minimal counterexamples; suspect-set attribution by intersecting
changed hashes with the failing test's closure; automatic bisection over the
definition graph (nearly free once builds are cached).

## M6 — Full handlers, forkable state — **half of it has since been removed**

Multi-shot continuations (evaluator moves to an explicit control stack). That
half is load-bearing and untouched: it is what makes a task a suspended machine
state in M7, a rollback a discarded continuation in W4, and a resumption's
threaded state the thing ADR 0017 §3 must not move.

Copy-on-write world state: build a fixture once, fork per test. **R2 deleted
it.** `World` is gone from `ply-eval`, a `Value::Cell` is a `Slot` in a
`TaskRegions` arena, and `ply_test::region::GroupRegion` no longer forks — a
fixture is built once *per worker* and mutated in place. Read the region track
below before relying on anything in this paragraph.

Two numbers this entry used to imply, corrected rather than dropped, because the
trade R2 took is only legible with both:

- "fork per test in microseconds" understated it. `World::fork` was one pointer
  clone at any fixture size, which ADR 0017 §6 records as **1 ns** and as making
  fixture reuse 8,939x cheaper than rebuilding a 10,000-cell fixture. *Neither
  figure is re-checkable — `World` is deleted, so both are inherited from ADR
  0017 rather than verified here.*
- What replaced it is not free. Opening a 10,000-cell region-scoped fixture and
  writing one cell costs **about 100 µs per test**: ADR 0017 §6 published
  95.7 µs, and this audit re-ran
  `crates/ply-eval/tests/fixture_open_cost.rs` and measured **105.8 µs**. Read it
  as an order of magnitude — the test *prints* the figure and asserts only a
  2 ms ceiling, about twenty times the reading, deliberately, so it is a
  staleness guard and not a performance gate. It is paid per test rather than per
  group. At 100,000 cells an open is **800 allocations and 4.45 MB**, which the
  same run reproduced exactly. All of it is a projection about a construct still
  not writable in Ply: every Ply program in this repository opens an empty
  fixture, where `GroupRegion::open` and `close` are both no-ops and the cost is
  nothing at all.

"This is where the unit/integration distinction is meant to stop mattering"
survives the change, but through a different mechanism: the isolation is now the
region closed at the end of a test rather than the fork given to it, and R2
measured that swap at **no cost on this corpus** — `5→5` groups and a modelled
wall-clock ratio of `1.00x`, verified by `ply-corpus regions examples` in this
audit. The reason is stated rather than celebrated in the region track below.

## M7 — Deterministic simulation

Concurrency arrives as an effect, because the language has none: `task.spawn` /
`task.join` are operations with one declared signature, so a threaded production
handler and a seeded simulated one cannot drift. A task is a suspended machine
state, which is what M6's explicit control stack bought.

- `simulate { .. }` — `handle` with a fixed clause set over `task`, `clock` and
  `random`, structured so the handler is the scope
- Virtual clock: time advances only when nothing is runnable, so a simulated
  timeout can never fire early
- A seed determines the whole run; `ply test --seed 7:3.0.2` replays exactly, and
  the seed is the repro artifact in M5's failure JSON
- Footprint-guided partial-order reduction: two tasks whose footprints do not
  conflict commute, so those interleavings are never explored. The reduction is
  **measured** against an unpruned search, because that number is the evidence
  that resource-granular effects were worth it
- `sim.read` in the row makes the seed dependency part of the type, and the
  cache key covers the definition set **and** the search plan

M7 gave every task its own world; R2 gives every task its own **region stack**
instead, and footprints are unaffected because they are static and do not depend
on how memory is represented. Nothing else in this entry moved.

**Exit:** a `det`, cacheable test over concurrent, time-dependent code; a
reported reduction against the naive interleaving count; and `exhaustive: true`
on a test, which is a proof over every interleaving rather than a sample.

> **Audit note (docs pass, 2026-08-17): re-checked, because this is the claim
> M7 originally got wrong.** `exhaustive: true` over regions never examined was
> the first of the seven defects this project's reviews found, and the guard
> against it is now real and adversarial rather than incidental:
> `crates/ply-eval/tests/exploration_soundness.rs` runs each reduced search
> against a reference and fails on a program that reports `exhaustive` while
> never reaching a reachable state — its header states the failure mode
> directly, that over-pruning "is worse than not pruning at all, because
> `exhaustive: true` is read as a proof". The budget direction is covered by
> `a_spent_budget_is_exhausted_and_not_exhaustive`
> (`crates/ply-eval/src/explore.rs:1598`), and the caching consequence by
> `an_exhausted_search_is_not_cacheable` (`crates/ply-eval/src/sim.rs:1346`).
> The tier consequence — that an exhaustive search over a *value* domain still
> only earns `property` — is `a_concurrency_law_over_a_binder_is_property_however_exhaustive_the_search`
> (`crates/ply-cli/tests/tiers.rs:527`), with the positive case at line 542.
> All of these are hermetic, in the workspace, and behind no gate.

`docs/adr/0006-deterministic-simulation.md`. Not in M7: real threads, a real
network effect, and finding races in Rust code.

## M8 — Specs

`requires` / `ensures` on a definition and standalone `law` declarations. A spec
expression is pure — an empty effect row — so a claim cannot disturb what it
judges. Specs are erased by normalization, so writing one changes no definition
hash and re-runs no test; the claim gets its own hash, which covers the
definition's.

- Each obligation is discharged at the strongest tier the system can
  **demonstrate**: `proved` (a static argument over every input satisfying the
  guard), `property` (randomized cases, N reported, shrinking on failure),
  `example` (concrete cases, no coverage claim)
- The `proved` fragment is small and exactly stated: linear arithmetic over
  `Int`, case analysis over ADTs, structural equality and congruence closure,
  unfolding of *non-recursive* definitions only. An inconclusive attempt reports
  `property`, never `proved`
- A tier is derived from the evidence rather than asserted beside it — the only
  evidence that computes to `proved` is a certificate naming its rules
- A concurrency law is `proved` only when M7's search was `exhaustive` **and**
  the value domain was covered too
- Frame conditions come from footprints; there is no `modifies` clause, because
  the effect system has inferred and checked that set since M2
- Value shrinking lands here — M5 shrank the definition set, M8 shrinks the input
- Coverage — the count of definitions carrying no obligation — is in the default
  output of `ply prove` and `ply review`, never behind a flag

**Exit:** `ply prove` reports every obligation with its tier, and a wrong
`proved` is caught by a differential audit that re-samples every proved
obligation; `ply review --changed` reports an implementation that moved under an
unchanged spec as a review of the obligations rather than of the diff; and
writing a spec selects zero tests.

`docs/adr/0007-specs.md`. Not in M8: an SMT integration, induction, a termination
checker, call-site precondition checking, or specifying what a definition does to
a resource.

## M9 — Native codegen

**Still deferred — decided by W6 against criteria written before the numbers
existed. The reason is not the one that carried this milestone through M0–M8.**

The old reason was that the interpreter is not the bottleneck. In a *test* run
that is still true, but by far less than this file used to claim. **The
`3.3% of a warm run` (10.4ms of 310.7ms) this paragraph carried, and the `93%`
for typecheck plus hash plus parse beside it, are both withdrawn.** README.md's
warm-loop table was re-taken on the documented corpus — which regenerates
byte-identically, and whose selection counts all reproduce exactly — and
`execute` came back at **125.1ms of 437.2ms, 28.6%**, with typecheck plus hash
plus parse at **68.6%**. Every other phase in that table reproduced its published
value closely, so it is one row that was wrong rather than a machine difference.
A second, independent re-take under background load agreed on the direction and
put `execute` at 17–19% of a much larger total; the cleaner run is the one
README.md publishes and the one to quote. Either way the conclusion survives —
a warm loop is still front-end and hash bound — but at 28.6% a faster evaluator
is worth *something* in a test run, where at 3.3% it was worth nothing, and M9's
case should not be argued from this number in the old terms again.

That figure was README's warm-loop table, taken by its own run of `ply-corpus gen`
and `ply test` on W6's machine — **not** by W6, which did not re-take it: ADR
0016 §0 carries M9's original deferral number, **4.2%** — now marked there as
contradicted by this re-measurement, and by the audit's own third take of the
`warm` scenario (`execute 125.47 ms 29.2%` of `total 429.04 ms`) — and the nine rungs of
§8.1 are a *request* ladder with no warm-run row in it. `benches/w6-ladder.json`
contains no such measurement either. In a *served* request the old reason is less false than
it looks, and W6 did measure that: the interpreter is **35% of a request** (209.3µs of
592.6µs, after the ladder's own seam is charged against it), a Ply request costs
**37.8x** the same syscalls answering the same bytes, and a Cranelift spike on
`std.http.read_line` hit **11.67x** on its weakest input — which projects
**1.48x** end to end by Amdahl, against a **1.55x** ceiling an infinitely fast
backend would have.

So three of the four criteria fail: the share is under the 50% C1 asks for, the
projection is under the 1.50x C2 asks for, and **six of the seven cheaper levers
are still unpriced**, which W6's criteria made independently sufficient on its
own.

**The seventh was priced, because it landed.** `ply-eval`'s constant memo
evaluates a nullary pure definition once per process, which is ADR 0016 §4's
"caching derived work" lever built rather than argued about: **1.77x on
`/health` and 1.15x on `/items`**, end to end on the real binary against the
same service with its constants disabled. That is the third time in this project
that a cheap algorithmic change beat a predicted codegen win — and it is why the
interpreter's share fell from the 67% W6's first take measured. Every cheaper
lever that lands makes M9's case weaker.

**What reopens it, computed rather than argued:** the share reaches 50% (it is
35%), the projection reaches 1.50x (it is 1.48x), and the six unpriced levers in
ADR 0016 §4 are priced with the best of them at or below **1.24x** end to end.
Two of them carry bounds worth looking at first: the tree-walker beats the
control-stack machine **2.73x** on the same pure request path, and one `/health`
request makes **1,035 allocations and 0.124 MB** to produce a 107-byte response.

> **Corrected (docs pass, 2026-08-17): the allocation figure has moved, and the
> direction matters.** 1,035 / 0.124 MB is what W6 published and what
> `benches/w6-ladder.json` still holds. R2 then put the arena and the lexical
> close on the evaluation path, and this audit re-ran the measurement on the
> shipped tree:
>
> ```
> $ ./target/release/w6-alloc --repo . --requests 200
> {"allocations_per_request":1122.335,"bytes_per_request":131677.4,
>  "requests":200,"response_bytes":107,"route":"/health"}
> ```
>
> **1,122 against 1,035 — the region milestone moved this number the wrong
> way.** ADR 0017 "What must be measured" §1 records why and is worth reading
> before treating it as a regression: about 40 of the 87 are `region_kind::infer`
> run once per `Machine`, which amortizes to nothing over a server's lifetime,
> and the rest is the arena wiring on a route that allocates no cells at all.
> The lever is unaffected either way — `/health`'s ~1,000 allocations are
> `Rc<Value>` boxes on the framing, routing and encode path, which is what
> unboxed representation and monomorphization attack and what a region model
> does not touch.

**And what W6 measured that argues even the 1.48x is optimistic**, recorded so a
future contributor re-measures rather than re-argues: compiling `read_line`
alone and trampolining its two callees back into the machine gives **1.71x**, not
11.67x — coverage is a cliff, not a slope. The spike's fragment accepts **141 of
366** functions across `std.http`, `std.router` and `std.json`, refusing field
accesses, constructor patterns, lambdas and list literals — which is what
endpoints and derived codecs are made of. And `read_line`'s own directly
measured end-to-end value is **1.02x**.

**And what R2 changed about all of the above.** Every number in this entry was
priced against the representation W6 measured, and R2 replaced part of it. ADR
0017's Consequences say so directly: "Codegen's ceiling should be re-measured
after this lands, because ADR 0016's 1.05× was a verdict on the old
representation and this ADR changes exactly what made that ceiling low." So the
**1.48x** projection and the **1.55x** ceiling are pre-R2 figures, and the first
of the three reopen conditions to re-take is the share `S`, because it is the one
a change of representation moves. Nothing in R2 measured it: ADR 0017 re-took
allocations per request and the isolation cost, not the ladder. **The ladder has
not been re-taken since the region track landed**, and re-taking it is the
concrete first step of any future M9 argument. The command is in ADR 0016 "The
result".

`docs/adr/0016-w6-performance.md` §8–§11. The spike lives in
`crates/ply-codegen-spike`, in its own workspace, depended on by nothing; ADR
0016 §3.5 requires that closing W6 delete it, and **some** of its numbers survive
in `benches/w6-spike.json` — the headline five inputs, `nodes` and
`compile_micros`, and nothing else. The **1.71x**, the **1.02x** and the
**141 of 366** quoted above are *not* in that file, nor in any other measurement
file; ADR 0016 §9.1 and §9.2 are their only record, so deleting the crate would
strand three of the four figures this entry rests on. **Verified still present in
this audit** — it is absent
from `Cargo.toml`'s `members` and no crate under `crates/*` names it as a
dependency, so `rm -r crates/ply-codegen-spike` remains the whole deletion, as
ADR 0016 §11 says.

> **Audit note (docs pass, 2026-08-17): ADR 0017's Context once gave the spike
> 8.44×; it now gives 11.67×, and this file agrees with it.** Checked against
> `benches/w6-spike.json` rather than against either prose: the conservative
> ratio per input (interpreter best ÷ spike worst) is 12.97, 12.83, **11.67**,
> 12.97 and 12.31, and `k` is defined as the minimum — so **11.67× is what the
> shipped measurement file supports**, and the 1.02–1.05× end-to-end band is
> §10.3's directly measured `read_line` value (1.021× at a 63-byte head, 1.050×
> at a browser-sized one), not the projection.
>
> **A first pass of this note said the discrepancy was live and deferred it to
> the ADR owner. It is not live and was not deferred.** ADR 0017's Context at
> `docs/adr/0017-regions.md:35` reads "ADR 0016 measured codegen at **11.67× on
> the fragment it can compile**", and `grep -n '8.44' docs/adr/0017-regions.md`
> now returns only lines 46 and 50 — both inside that ADR's own correction block,
> recording the retired figure. The two documents agree. Two passes of the same
> audit fixed the same discrepancy without reading each other's output, and this
> note is what was left pointing at a conflict that no longer exists. Where 8.44×
> came from is still not established; nothing in the tree produces it.

> **Audit note (docs pass, 2026-08-17): "in its own workspace" has a cost this
> line does not price.** `cargo test --workspace` does not reach
> `crates/ply-codegen-spike/tests/spike.rs`, so ADR 0016's required tests 12,
> 13 and 14 — the ones establishing that the spike compiled what it claims to
> have compiled, and that its output matches both evaluators — are not run by
> the suite anyone runs. Running them means `cargo test` inside that directory,
> and on the audit machine that fails to resolve: cranelift `0.134.3` needs
> `rustc 1.94.0` and the installed toolchain is `1.93.1`, with no
> `rust-toolchain.toml` pinning anything. The **11.67x** and the **141 of 366**
> above therefore rest on numbers no green suite re-checks. The ladder side —
> the 35% share, the 1.48x projection, the 1.55x ceiling and `w6::decide`
> itself — is fully covered inside the workspace by
> `crates/ply-corpus/src/w6.rs` and `crates/ply-corpus/tests/w6_report_integrity.rs`,
> and the deferral does not turn on the spike's magnitude in any case.

---

# Web track — complete, W1 through W6

M0–M8 built a language that can prove things about programs that never leave
memory. The web track is what it takes to serve an HTTP API, and the ordering was
driven by one fact true when it was written: **Ply had no I/O at all**. Not
limited I/O — none. Every handler ever written for it was in-memory or simulated.

So a postgres driver is not the first problem. The first problem is that the
runtime's knowledge of what a computation can do is the foundation of every
guarantee here, and reaching the host is by construction a hole in it.

## W1 — The boundary, and one endpoint

The smallest thing that answers the two questions deciding everything downstream.

- The host effect boundary: a Rust-implemented handler for a Ply-declared effect,
  with a declared footprint, a determinism flag, and at-most-one resumption
- `ply hosts` — the trusted computing base, enumerable in one command
- A production `task` handler over an async runtime. M7 built only the simulated
  scheduler; real threads were explicitly out of scope
- `Bytes`, and strings that survive contact with UTF-8
- Minimal TCP, and an HTTP handler that returns a fixed response

No database, no JSON, no routing. If the answers are bad this is small enough to
throw away.

**Exit:** a request served over a real socket; `ply hosts` lists every host
handler with its footprint; `ply test` is hermetic without `--host` and says so;
resuming a host continuation twice is a diagnostic; and **a measured per-request
interpreter cost**, which is the number W6 turns on.

> **Audit note (docs pass, 2026-08-17): re-checked, because W1 is the milestone
> that advertised a footprint check it had not armed.** It is armed now, and by
> hermetic tests behind no gate. The boundary check that a handler may not
> report an atom outside the entry point's declared footprint is
> `codes::HOST_FOOTPRINT_ESCAPE`, raised at `crates/ply-eval/src/machine.rs:2311`
> and `:2331` and asserted from both sides in
> `crates/ply-eval/tests/host_boundary.rs:566,612`,
> `crates/ply-eval/tests/host_trust_audit.rs:822,868` and
> `crates/ply-test/tests/host_trust_audit.rs:543`. W4's database-specific
> version, `DB_FOOTPRINT_UNDECLARED`, is raised at
> `crates/ply-host/src/db.rs:247,267` and asserted by unit tests that need no
> server (`crates/ply-host/src/db/tests.rs:111,142,196`,
> `crates/ply-host/src/db/handler/tests.rs:180,216`) as well as by the
> postgres-gated integration tests. The remaining clauses hold too:
> "hermetic without `--host` and says so" is
> `test_is_hermetic_without_the_flag_and_says_which_binding_it_used`
> (`crates/ply-cli/tests/cli.rs:936`), and resuming a host continuation twice
> is `codes::HOST_CONTINUATION_RESUMED`, asserted five times in
> `crates/ply-eval/tests/host_linearity_audit.rs`.

`docs/adr/0008-host-effect-boundary.md`

## W2 — Payloads

- `Map` as a first-class type. Headers, query params, JSON objects and connection
  pools all want one, and `List<(K,V)>` is not a substitute at request volume
- Compile-time derivation over normalized definitions: `derive json for Order`
- JSON encode and decode, derived rather than hand-written
- A dictionary is a **record** — `JsonCodec<a> = {encode: .., decode: ..}` — which
  Ply's structural records give for free and which is what a typeclass dictionary
  elaborates to anyway
- Framework signatures take explicit dictionaries — the elaborated form of a
  typeclass constraint, so a resolution layer can be added later as sugar rather
  than as a rewrite
- Constraints checked at the **signature**, not at instantiation: `where
  derivable(json, a)`. Bare reflection fails late, which is the expensive failure
  for an agent; this repairs it without any dispatch machinery
- `Float`, and a decimal type — `i64` cents is a decision you regret later
- A **stdlib path**: `import std.json`, `import std.net`. W1 left the `net`
  effect unimportable, and JSON would hit the same wall
- **Byte-oriented builtins**, because W1's 5.41us per byte of request head is
  five O(n) folds boxing an Int per byte with no early exit — an algorithm, not
  a constant factor

Type-directed **dispatch** is deliberately not decided here. Derivation is the
substrate under either candidate, so W2 proceeds without settling it; W3 and W4
produce the evidence that does — how often a type is abstract at the point of
dispatch in a real stack.

**Exit:** an endpoint that parses a JSON body into an ADT and returns a JSON
response, with the codec derived; and a law that decode-after-encode is identity,
discharged at whatever tier it earns.

`docs/adr/0010-generic-derivation.md`, `docs/adr/0012-w2-contract.md`

## W3 — A real server

- HTTP/1.1 properly: streaming bodies, chunked encoding, keep-alive, timeouts
- Routing, with the route table as ordinary data
- TLS
- Effect-set aliases, because a hundred endpoints with explicit rows is noise

**Exit:** a multi-route service under load, with per-endpoint footprints visible
in `ply check --types` — which is the first point where an endpoint's declared
signature says which tables it touches.

`docs/adr/0009-effect-set-aliases.md`, **amended by W6**: an alias may not name a
whole effect and may not cross a module boundary. Both refusals are `E0114` and
both exist for one reason the original ADR did not have — expansion has to be a
function of the file, or an edit elsewhere leaves a stale published footprint
behind a skipped recheck, which corrupts scheduling and isolation silently. The
ADR showed the rejected form for three milestones; it no longer does.

## W4 — Postgres

- Wire protocol, connection pool, prepared statements
- **Transactions as handlers.** A transaction is a scoped handler over `db.*`
  that commits or rolls back at the boundary, and because M6 gave real reified
  continuations, a rollback is discarding the continuation rather than unwinding
- An in-memory handler satisfying the same signature, and an M8 law asserting the
  two agree

**Exit:** a CRUD endpoint against real postgres; the same tests passing against
the in-memory handler with no source change; and the agreement law discharged as
`property` with its case count — the mock-drift claim every backend team makes
and none of them check.

> **Audit note (docs pass, 2026-08-17): the "against real postgres" half of this
> exit criterion is conditional on the machine.** Every W4 test that touches a
> server is behind `cluster::available()`
> (`crates/ply-host/tests/support/cluster.rs:38`), which returns `false` when
> `initdb` and `postgres` are not findable — and the tests then print a skip
> line and return green. Six `#[test]`s and roughly thirty-five sequenced
> phases are behind it, seventeen of them in
> `db_transaction_audit.rs::transactions_the_pool_and_parameters_under_adversarial_conditions`
> alone. A second gate, `PLY_PG_URL`, hides ten more tests in
> `crates/ply-host/src/db/scope/tests/live.rs` and is set by nothing in the
> repository, so those skip even on a machine that has postgres. The twin half
> of the criterion, and the agreement law, are hermetic and unconditional.
> `docs/adr/0014-w4-contract.md` §13.1 has the full inventory and says which
> required tests depend on which gate. **Two of W4's required tests are
> enforced by nothing at all** — 14, the `EXPLAIN (GENERIC_PLAN)` differential
> over `scan`, and 16, the `E0438` refusal of a trigger or cascade reaching
> outside its atom. The second is the guard on the very failure the "risk that
> matters" section below names, and it is not armed.

`docs/adr/0014-w4-contract.md`. Not in W4: query building, an ORM,
connection-level `LISTEN`/`NOTIFY`, replication, migrations as a tool, cursors,
a time type, and a database per test.

## W5 — Operations

A log, a configuration and a way to stop are ambient in every other language, and
ambient is what the previous eight milestones exist to remove.

- **Observability as an effect**, with the resource label a *channel*, so a
  function's row says which channels it records on exactly as it already says
  which tables it touches. Metrics are operations on the same effect. There is no
  disabled path that skips the perform — a row cannot be conditional on a flag —
  so what a span costs when nothing is collecting is stated and measured rather
  than promised away
- **Typed secrets**, the headline: `Secret<a>` is a builtin type constructor with
  no constructor pattern, no path to `String`, no `json` / `ord` / `row`
  derivation, no generator and no rendering, so every route from a credential to
  a log is a compile error. The routes that stay open — a source literal, the
  plaintext it was built from, one bit per `secret_verify`, a host handler that
  receives one, and memory — are enumerated, because a guarantee with an unstated
  hole is worse than none
- **Configuration** from `--set`, a `KEY=VALUE` file and the environment, in that
  precedence, snapshotted **once** at bind time so `config.read` is honestly a
  read; a `--config-schema` verified at start-up so a missing credential is a
  refusal rather than a 3am 500 — a `required` key nothing supplies is `E0441`
  before anything is bound (`crates/ply-cli/src/config.rs`, asserted by
  `crates/ply-cli/tests/config_cli.rs:265`). *This line used to read "verified
  at start-up, exactly as W4 verifies a database schema". The comparison was
  wrong in the direction that flatters: `--config-schema` really is verified,
  and `--db-schema` is not — it evaluates the program's `Schema` function and
  reports its shape, and never asks the server. `E0435` is raised nowhere. See
  the audit notes in `docs/adr/0014-w4-contract.md` §7 and §8.*
- **Graceful shutdown**: stop accepting, drain, then a pinned teardown order —
  roll back every open transaction, close every open span, flush the sink, close
  the pool. W5 still has no cancellation, so a request live at the deadline sees
  its connection closed with no response and the run exits `3`; that is stated,
  not smoothed over
- **Deployment**: `ply build` produces a whole-program `.plyx` — the entry
  point's closure, every body verified against its own key, reproducible
  byte-for-byte from any machine. Incremental *transfer* is refused with the
  measurement that would re-open it; incremental *review* is kept, as
  `ply build --diff`

**Exit:** a service that can be deployed, observed, and shut down without losing
in-flight requests — with `examples/desk.ply`'s accept loop draining with **no
source change**, a credential that appears in no log line, no cache entry and no
definition hash, and two builds of one tree producing identical digests.

> **Audit note (docs pass, 2026-08-17): the draining half of this criterion is
> demonstrated only on Unix, and only where postgres is installed.** The eight
> tests that drive the real binary over a real socket and send it a real signal
> live in `crates/ply-cli/tests/w5_shutdown.rs`, whose first non-comment line is
> `#![cfg(unix)]` — so on any other host the file is not compiled and, unlike
> the postgres gates, **prints nothing**. The transaction-rollback half needs
> `initdb` on the machine as well. The other two clauses of this criterion are
> unconditional: the credential-leak sweep and the byte-identical-build check
> are hermetic and run everywhere. `docs/adr/0015-w5-contract.md` §12.1 maps
> required tests 27–34 onto the three gates. Separately, required test 18's
> constant-time *harness* for `secret_verify` was never written — the property
> holds by construction in `ply_eval::value::constant_time_eq`, but it is
> established by reading the function, not by measuring it.

`docs/adr/0015-w5-contract.md`. Not in W5: metrics backends, log shipping,
orchestration, autoscaling, distributed tracing propagation, sampling,
cancellation, live config reload, incremental deploy transport, artifact signing,
zeroization, and — breaking a promise W4 made — backpressure and load shedding.

## W6 — Performance, and whether M9 comes forward — **done; the track closes here**

M9 was deferred because execution was a few percent of a warm test run. Serving
inverts that argument, and the control-stack machine costs four heap allocations
per frame push.

The hypothesis this milestone was written to test was: *most web APIs are
I/O-bound, so an interpreter may well be fine.* **The measurement says: not
mostly, and it depends what you put on the path.** With a real postgres on it the
database is 55% of a request — partly because `/items`' own JSON encode sits
inside that rung — and the interpreter is 35%. There is not much I/O to hide
behind above the database: the whole socket layer is 7.8%, TLS is 0.6% and
tracing to JSON is 1.0%.

**Exit, met:** the accumulated stack in one table, throughput and tail latency
under real load on both transports, both stores and both accept loops, and an M9
verdict computed by `ply_corpus::w6::decide` from criteria pinned in code before
any number existed. The measurement files are `benches/w6-ladder.json` and
`benches/w6-spike.json`; neither contains a verdict, a test asserts that, and
each is written by the command that takes it rather than by hand.

The headline numbers, all on one machine in one run:

| | |
| --- | --- |
| a request, end to end, TLS + postgres + tracing | **592.6µs** |
| the same syscalls in Rust answering the same bytes | **15.7µs** — a Ply request is **37.8x** the floor, **16.8x** like for like on `/health` over plaintext |
| what a reader gets, over postgres | **1,687–3,778 req/s on one core** |
| p99 at concurrency 1 | **0.29–0.65ms** |
| the interpreter's share | **35%**, after the ladder's negative residue is charged against it |
| concurrency | buys nothing: `/health` 3,914 req/s at c=1 and 3,930 at c=32, p99 287µs → 252ms |
| the constant memo, priced end to end | **1.77x** on `/health`, **1.15x** on `/items` — and **1.00x** on the accept loop that spawns, where the memo is inert |

`docs/adr/0016-w6-performance.md` §8–§12. Not in W6, and held for the codegen
backend: whatever the verdict, none was built. **Not held for "optimizing
anything":** the constant memo landed in `ply-eval` between W6's two takes, which
is one of ADR 0016 §4's own cheaper levers built rather than priced, and the
ladder was re-taken on the tree that has it. Two obligations are still open: the
spike crate has not been deleted, and the memo is refused inside any open
**task/simulation** region — so a service whose accept loop calls `task.spawn`
memoizes nothing, measured at **1.00x against 1.77x** on the same route.

> **Clarified (docs pass, 2026-08-17): "region" in that sentence is not the
> region R1 and R2 built.** This line was written before the region track and
> reads, on the current tree, as though every `with_cell` disabled the memo. It
> does not. `Machine::constant` (`crates/ply-eval/src/machine.rs:1775`) refuses
> the memo on `!self.sims.is_empty()` — `sims` is the *scheduler* region stack, a
> `simulate { .. }` block or the production region `task.spawn` opens, and it is
> untouched by ADR 0017's memory regions, which live in `self.regions`. A
> `with_cell` opens an arena scope and leaves the memo alone; `/health` still
> serves from it, which is why ADR 0017's allocation reading calls its body "a
> nullary pure definition served from ADR 0016 §12.1's constant memo".
> CONTRACTS.md ("The constant memo") states the same thing and goes further:
> the implementation is **"wider than the reason"**, because a production region
> keeps no trail and records no step yet disables the memo anyway, and CONTRACTS
> calls that "a defect rather than a rule". The obligation is therefore a code
> fix with a known shape, not an accepted design constraint.

## Where the web track landed

Three things the track cost that its plan did not price, recorded here so the
next reader inherits the corrections rather than rediscovering them:

- **W3's claim that swapping the store touches one function was false.** A
  database is not a variable; `store.all[items]()` handing back a whole table had
  already thrown away the statement, the transaction and the constraint. Every
  endpoint's body moved. What survived is the useful half and is the claim worth
  making: the **resources** did not move, so every row still names the same
  tables, and swapping the twin for postgres really is one function.
- **The in-memory twin is slower than the database it stands in for** — in
  process the twin's `/items` handler costs 544.6µs a call, 344.9µs of which is
  `std.db`'s memory engine parsing its SQL in Ply, and every twin clause writes
  its whole state back through a persistent map. The substitution ADR 0016
  planned to price the database with therefore prices the twin, and the ladder
  uses a route difference instead and says so. Test doubles being dearer than the
  real thing is a real cost of "the double and the real thing share one
  signature".
- **W4 promised backpressure and load shedding; W5 broke that promise
  explicitly** and W6 measured what it means: throughput is flat in concurrency
  and latency grows linearly, so an overloaded Ply service queues rather than
  sheds. With no cancellation either, a request live at the drain deadline loses
  its connection with no response.

## What the web track is for

Parity with other languages is not a reason to use this. These are:

- **Mock drift becomes checkable.** W4's agreement law is the thing every team
  asserts informally about its test doubles. What it costs to actually check is
  in the law's own declaration: it is a `law/host`
  (`examples/agreement.ply:519`), so it reaches a real server, it can never be
  `proved` — `property` is the stated ceiling, and `ply prove` prints the reason
  — it is never cached, and under `ply prove`'s default hermetic run it reports
  `W0604 unattempted` rather than passing. Verified by running it: `ply prove
  examples/agreement.ply` reports `4 obligations · 0 proved · 3 property · 0
  example · 1 unattempted`, and names the unattempted one — *"reaches the host
  (…); run `ply prove --host`"*. A reader who sees three green laws there without
  `--host` has checked `replay_memory`, not postgres, and the file says so at
  line 514.
- **An endpoint's footprint is inferred and exact.** Which tables a route
  touches comes from the type, not a comment — giving exact test isolation and a
  static answer to "what writes this table". Verified: `ply check --types
  examples/desk.ply` prints 68 rows, and `list_items` carries
  `/ {std.db.db.read[items]}` from a signature that declares `{db.read[items]}`.
- **Concurrent request races become findable.** M7 finds a check-then-act race in
  **two interleavings** and returns a seed. Verified by running it: `ply test
  tests/fixtures/bank_race.ply` fails in `2 interleavings`, names the two
  conflicting steps, and prints `replay: ply test --seed 0:0.1.0.2 --filter "no
  account is ever overdrawn"`. **The demonstration is over a cell-backed ledger,
  not over a database row**, and "pointed at two requests hitting one row" is a
  projection rather than a landed test: no `simulate` block in this repository
  contains a `db` operation, `examples/desk.ply` has none at all, and W4
  deliberately removed the one check-then-act the service used to have by taking
  the order id back out of the `INSERT` instead of folding for it
  (`examples/desk.ply:722`). The path is real and unwalked —
  `tests/fixtures/db_transaction_scope.ply:22` says a race between two requests
  on one row is found "against the twin, which is pure Ply", and a twin-backed
  test's row is empty and so runnable inside `simulate` — but nobody has written
  that test.
- **`ply review --changed` on an API** answers whether any endpoint's *specified*
  behaviour moved, without reading the diff. Verified: `ply review --changed
  examples/desk.ply` reports over 180 definitions with their coverage and
  baseline state in the default output.

## The risk that matters

A host handler that misreports its footprint corrupts scheduling and isolation
**silently** rather than loudly. Every dangerous defect found across the audited
milestones was a green result over unexplored space, never a crash. The host
boundary is where that failure mode is easiest to reintroduce and hardest to
detect, and it deserved harder adversarial review than anything else in the
track.

> **Audit note (docs pass, 2026-08-17): one route into exactly this failure is
> open, and W4 documented the guard as though it were closed.** ADR 0014 §2.5
> specifies `E0438 DB_UNMODELLED_SIDE_EFFECT` — a trigger, a rewrite rule, or a
> foreign key with a cascading action reaching a table outside the atom it
> fires under is refused at bind time, before anything runs. That check was
> never built. `E0438` exists as a registered code and as a `RESERVED_CODES`
> entry so no handler can impersonate it, and it is raised nowhere; nothing in
> the tree queries `pg_trigger`, `pg_rewrite` or a constraint's delete action.
> Against a database carrying such an object, a statement touches a table its
> text never names, the published row does not say so, and two host-backed
> tests over what look like disjoint tables are scheduled concurrently on the
> strength of that row. This is not a host handler misreporting — it is the
> database doing work the handler cannot see — but the consequence is the one
> described above, and it is the one place in the track where the stated
> mitigation is structurally absent rather than merely partial. Reported as a
> code gap; not fixed in this documentation pass.

It also arrived through a feature that looks like nothing. An `effect set` is
"an abbreviation for a row", and letting one cross a module boundary would have
let an edit in the declaring module leave a stale published footprint behind a
skipped incremental recheck — an under-reporting footprint, which is exactly this
failure. W3 refused it, and W6 rewrote ADR 0009 to say so, because the ADR still
showed the form the implementation rejects.

---

# Region track — complete, R1 through R3

W6 said codegen's ceiling is low because the *representation* is expensive, not
because compilation fails: every value is heap-allocated and every handler
dispatch walks a stack. The region track is the first milestone aimed at the
representation, and it is the memory model the three milestones after it depend
on.

It also forced a decision the project had been carrying since M6. Perceus-style
in-place update fires only when a value is uniquely owned, and a design that
forks worlds keeps reference counts high by construction — so the persistent
forkable world and the zero-cost path are mutually exclusive. ADR 0005 had picked
the forkable world over branding the region **because branding looked heavy in
the type system**; building regions for memory means building that branding
anyway, so the objection that decided M6 stopped applying and the world went.

> **Audit note (R3, 2026-08-18): the paragraph above states the premise as fact
> and it was never measured.** It is left standing because it is what the track
> was decided from, and because ADR 0017's Context now carries the same
> correction beside the same sentence. Three milestones of measurement did not
> support it: R1 and R2 removed the world and allocations per `/health` went
> **up**; R3 removed the compile-time work that had landed on the request path
> and they came back down to **1,082**, against a pre-region **1,035**. What
> the region model demonstrably bought is safety (the escape discipline) and the
> arena against the persistent map it replaced. What it has never been shown to
> buy is the request path. §R3 below.

`docs/adr/0017-regions.md`, which supersedes ADR 0005 §2 and amends ADR 0008 §6.

**It landed in three parts, and the split is the instructive part.** Describing
regions as one milestone would hide the defect the shape produced, so all three
are below.

## R1 — the machinery

The allocator, the analysis and the escape discipline, built and tested on their
own terms:

- `crates/ply-eval/src/arena.rs` — a bump arena of scopes, a `Slot` carrying a
  generation so a reclaimed position fails to resolve rather than answering
  stale, `Reclaim`, and `Arena::pin` / `Pin` for a capture that can outlive its
  scope
- `crates/ply-eval/src/region_kind.rs` — `infer` over a whole program, splitting
  each region into `unique` (no continuation captured across it, so the close is
  a truncation) and `shared` (a capture is reachable, so the slots are reference
  counted and reclaimed when the last continuation that can reach them dies)
- brand-based escape checking on the **resolved** type including a function
  type's effect row, because a closure that captured a cell need not mention it
  in its parameters or its result. A value that would outlive its region is
  `E0446` (`codes::REGION_ESCAPE`), reported where it would escape
- Perceus reference counting for what does escape, and it is a compiler pass
  rather than a runtime check: `crates/ply-eval/src/rc.rs` holds the liveness
  analysis and `code.rs` runs it at lowering, so a last use moves and a dead
  binding releases. Cycles are not collected; ADR 0017 §4 accepts that and
  supplies diagnostics where a cycle is constructible

**And none of it was connected to anything.** No engine consulted the kinds, no
`with_cell` allocated through the arena, and no region ever closed. The gap was
found by a benchmark rather than by a report, which is stated in the tests that
now exist to prevent its recurrence —
`crates/ply-eval/tests/cell_arena_wiring.rs:5` ("R1 built the allocator and
connected nothing, and a benchmark rather than a report is what found that out")
and `crates/ply-eval/tests/region_wiring_audit.rs:6`. R1's own tests were green
throughout, because every one of them attacked the allocator or the analysis
directly and none asked whether an engine had ever called either.

**R1 also shipped a false claim, and it is the seventh of the defects this
project's reviews have found.** ADR 0017 §3's first draft said each resumption
observes the region as it was at capture and asserted that this "is exactly ADR
0005's semantics". It was not, and the two readings are distinguishable **in one
integer** on the section's own worked example: ADR 0005 §3 threads one state and
pins the two-resumption example at `30` with its trace cell at `2` as a required
test, and snapshot-at-capture answers `1` for that cell. Since ADR 0017's
governing property is that program meaning does not change, ADR 0005 won and §3
was rewritten to say so. The retraction is in the ADR rather than hidden by a
deletion, and the discriminating programs are landed in
`crates/ply-eval/tests/region_meaning_audit.rs` and
`resumption_semantics_audit.rs`.

## R2 — the wiring, and the first real free

R2 put R1's machinery on the evaluation path and deleted what it replaced:

- **`World` is gone.** A `Value::Cell` is a `Slot` in a `TaskRegions` arena
  (`crates/ply-eval/src/task_regions.rs`); `ply_eval::world` does not exist
- **Both engines ask for the kind and open a scope.** `Machine::region_kind`
  (`machine.rs:435`) and `Interp::region_kind` (`interp.rs:625`) hold a
  `OnceCell<Rc<Regions>>` filled by `region_kind::infer`, consulted at each
  `with_cell`'s **own span**, which is what makes the analysis load-bearing
  rather than decorative
- **`with_region` is executed rather than lowered away.** `ExprKind::WithRegion`
  is a node in `ply_syntax::ast`, in `ply_eval::code`, and in both engines
  (`machine.rs:992`, `interp.rs:422`)
- **The close is a real free**, deferred by an `Arena::pin` when a capture can
  still reach the slots — which is why `crates/ply-eval/tests/use_after_free_audit.rs`
  exists: before R2 a region's memory was never handed back, so an escape the
  checks missed was harmless. R2 makes the same escape this language's first
  possible use-after-free, and every program in that file is written to produce
  one
- **Test isolation is `Isolation::Region`** (`crates/ply-test/src/schedule.rs:101`),
  and `ply_test::region::GroupRegion` no longer forks — a fixture is built once
  per worker and mutated in place

### What R2 measured, re-verified in this audit by running it

| claim | measured |
| --- | --- |
| allocations per `/health` request | **1,122.34** and 131,677 bytes for a 107-byte response, against W6's pre-region **1,035** — the region model moved this the *wrong* way, and ADR 0017 says so rather than burying it. R3 took the ~40 one-time allocations back off (§R3): the figure is **1,082** today and still above 1,035. The 1,122.34 is *not* re-takeable from this tree; the 1,082 is, with `./target/release/w6-alloc --repo . --requests 200` |
| the static split over `examples/` and the `std` it imports | **113 regions, 0 `unique`, 113 `shared`** — every one because of a tail-resumptive clause |
| the dynamic split, which is the one that matters | **709 region closes, 709 freed at the close, 0 deferred**; 348 slot bumps against a peak of 6 live; 73 pins taken, 0 slots reclaimed late |
| the arena against the persistent map it replaced, at 10,000 cells | map: **20,000 allocations and 1.04 MB** to build, 10,000 more to write every cell. Region: **0 to build, 0 to write, 0 to close** |
| the isolation cost — the only real argument against this design | **zero on this corpus**: `5→5` groups, `1.00x` modelled wall clock, `isolated 176 of 186` either way |
| `--engine both` agreement | `audited 166 of 186 · 20 ran on one engine only`, zero divergences |

Three of those need their reason attached or they mislead:

- **The allocation result is a falsified hypothesis, not a regression to chase.**
  `/health` allocates no cells — its body is a nullary pure definition served
  from the constant memo — so the ~1,000 are `Rc<Value>` boxes on the framing,
  routing and encode path, which a region model does not touch. About 40 of the
  87 added are `region_kind::infer` run **once per `Machine`** — ADR 0017 took the
  reading at 200, 400 and 800 requests and the delta halved exactly each time, so
  the wiring costs ~8,100 allocations once and nothing per request. This audit
  re-took only the 200-request point (1,122.34, matching), so the amortization is
  ADR 0017's measurement rather than one re-checked here. A service opens three
  regions at start-up and none per request.
- **The isolation cost is zero because the exemption was exempting nothing**, not
  because the design is free. No test in `examples/` carries a `cell` atom in its
  footprint at all. `ply-corpus regions --hypothetical cells:labels` is how the
  risk is priced for a corpus that would have one.
- **`--engine both` is weak evidence here and ADR 0017 says why.** The
  tree-walker refuses every clause that binds a continuation (`E0504`), which is
  exactly the construct R2 changes, and both engines hold the same state
  representation, so a change to the memory model moves them together. The oracle
  for "meaning did not move" is `region_meaning_audit.rs` and
  `resumption_semantics_audit.rs` — programs whose answer *differs* between the
  two candidate readings, with the expected integer written down.

### What R2 took away from programs

Refusing a program that used to run is a change of meaning, so it is recorded
rather than left to an implementation. Three `with_cell` shapes no longer
compile — a cell escaping through a closure, through a record of closures, or as
an operation argument — because the forkable world made them safe and an arena
does not. Nothing in `examples/`, the standard library or the corpus was written
in those shapes; the only users were audit fixtures smuggling a cell on purpose,
which now reach a `cell` atom through a written row instead.

One exclusion is deliberate: **`task.spawn` is not refused for a bare
`with_cell`**, because a cell reaching a task is how tasks share memory,
`simulate { with_cell[s](..) { .. spawn .. } }` is a landed and tested shape
(`examples/bank.ply`), and a `task` operation anywhere in a region makes that
region `shared`, whose slots outlive its close for exactly this reason.
`with_region` keeps the stricter rule, having no program depending on the loose
one.

**One route stays open and is named rather than implied:** a continuation parked
in an enclosing region's cell, where the row is erased at the constructor's field
type so no type after the constructor mentions the region. Closing it needs the
brand to survive a nominal declaration, which is the rank-2 machinery ADR 0005
rejected. The consequence is a good one for footprints — with every other route
closed, **a written row is the only way a `cell` atom reaches a published
footprint**.

## R3 — the hoists, and the rule that went against the design

R2 left allocations per `/health` **higher** than the representation it replaced,
and ADR 0017 said so rather than burying it. What it did not do was find out
where they were. An attribution run then ranked two compile-time analyses among
the largest items: `region_kind::infer`, a whole-program analysis memoized per
`Machine` when a `Machine` is built per entry point, and body lowering. Neither
is a design tradeoff; both are work the front end could have done once. R3
hoisted both — the region-kind analysis and the lowered bodies are now scoped to
the *program*, so an engine built next over the same program is handed the answer
instead of recomputing it (`Machine::share_region_kinds`,
`Machine::share_lowering`; the new tests are
`ply-eval/tests/region_kind_sharing.rs`, `ply-eval/tests/lowering_sharing.rs` and
`ply-corpus/tests/region_kind_hoisted.rs`).

R3 did **not** do unboxing, evidence passing or codegen. Those were the planned
next milestone, and the premise under them is the one that had just failed.

**One caveat on the run that scoped this milestone, because it is the same trap
the milestone then found.** That attribution reported `code::lower_*` at roughly
a quarter of a request. It was read at a 20-request window, and the same family
reads **33.8%** of a 20-request window on this tree today while costing **0.0**
allocations per request — so a large window share was never evidence that
lowering was on the request path, and it is not re-takeable now whether it ever
was. The region-kind half of that run is different: its cost was one whole-program
traversal per `Machine`, both harnesses build a `Machine` per call, and the
before-and-after pair is written down in
`crates/ply-corpus/tests/region_kind_hoisted.rs`'s header — which says in the
same breath that only the *after* half of it is re-takeable from this tree. What
R3 can show is where both families are now, and it is zero per request for both.

### The rule, which was fixed before the milestone started

> Allocations per `/health` **below 1,035** → regions were fine, the cost was
> always elsewhere, removing the forkable world was neutral-to-good, and the next
> milestone is frame representation.
>
> Allocations per `/health` **still above 1,035** → regions carry a real fixed
> overhead ADR 0017 did not account for, and whether to keep them genuinely
> reopens.

1,035 is what `benches/w6-ladder.json` publishes in its `boxing on hot paths`
alternative, and it is the pre-region reading.

### What it measures at

**1,082 allocations and 0.128 MB.** The second branch fires.

Everything below was taken on this tree on 2026-08-18, release profile, on the
machine in `docs/ONBOARDING.md` §Provenance, and each row names the command that
re-takes it. Where a figure from before this milestone appears it is in its own
column, labelled with where it comes from, and it is *not* re-takeable here —
that is the point of separating the columns rather than merging them into a
delta.

| | measured | command |
| --- | --- | --- |
| allocations per `/health`, 200 requests | **1,081.87**, 127,954.65 bytes | `./target/release/w6-alloc --repo . --requests 200` |
| the same, against the file that holds the baseline | *"the report says 1035 allocations and 0.12 MB per /health request; this tree makes 1082 and 0.128 MB"* | `cargo test -p ply-corpus --release --test w6_report_allocations -- --nocapture` |
| the same at 800 requests | **961.92**, 277,417.23 bytes | `./target/release/w6-alloc --repo . --requests 800` |
| the two-window fit | **911.5 per request + 34,465 once per `Machine`** | `cargo test -p ply-corpus --release --test w6_alloc_sites -- --nocapture` |

**So the hoists worked and the milestone still fails its own rule.** Those are
two separate findings and both belong here.

### The attribution: both hoist targets are gone, and what is on top now

From the same `w6_alloc_sites` run, which fits a per-request slope and a
per-`Machine` intercept from two windows over one call rather than reading a
share off one window:

| family | per request | per `Machine` | share at n=20 | share at n=200 |
| --- | --- | --- | --- | --- |
| `ply_eval::region_kind` | **0.0** | **0** | 0.0% | 0.0% |
| `ply_eval::code::lower` | **0.0** | 17,821 | 33.8% | 8.2% |

The largest per-request sites that remain, ranked by slope:

| allocations per request | share of the marginal | site |
| --- | --- | --- |
| **415.0** | **45.5%** | `frame::dispatch < Machine::step < Machine::call` |
| 65.0 | 7.1% | `interp::literal < Machine::step < Machine::call` |
| 53.0 | 5.8% | `Machine::step < Machine::call < w3::Loaded::over_sim` |
| 47.0 | 5.2% | `ply_eval < Machine::perform_host < Machine::step` |
| 25.0 | 2.7% | `Value::list < Machine::match_pattern < frame::dispatch` |

`region_kind::decide`, `Symbol::new < region_kind::decide` and
`region_kind::Analysis::walk_at` were all ranked sites before R3 and none of them
appears anywhere in the ranking now. The `Symbol::new` that remains is
`Symbol::new < Machine::build`, 1,011 per `Machine` and 0.0 per request.

**`code::lower` is the trap in this table and it is worth reading twice.** It
still reads 33.8% of a 20-request window while costing **nothing** per request,
because a 20-request window divides one-time work by twenty. A share taken at one
window is not evidence that work is on the request path; the fit is. That is
stated in `w6_alloc_sites.rs`'s own header, and it is the most likely way for the
next attribution to be misread.

**Frame representation is now 45.5% of what a request allocates**, which is the
lever the next milestone was already pointed at. Read the two columns for that
site the right way round before comparing it to anything: it is **27.9%** of the
20-request window and **45.5%** of the per-request slope, and an older
attribution reporting a share near the former is reporting the same site at the
same cost, not a smaller one.

### Request cost and throughput, re-taken

The whole W6 ladder was re-taken with the command in `benches/README.md`
§"Taking the ladder" and is shipped as **`benches/w6-ladder-r3.json`**, so it can
be rendered rather than quoted:

```
./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json
```

`benches/w6-ladder.json` is **not** overwritten: it holds the pre-region 1,035
this milestone's rule is stated against, and it is the file the two staleness
guards read.

The verdict is unchanged — **keep deferring M9** — and so is everything the
verdict is composed from: interpreter share **35%** (34.3%–34.7% over its
repeats), ceiling **1.53×**, projection **1.46×**, six of ADR 0016 §4's seven
levers unpriced.

The absolutes are all larger than ADR 0016 §8.1's, and the control says why:
the **Rust floor**, which has no Ply in it at all, moved from 15.68µs to
**17.13µs**. This box measures about 9% slow against the one W6 used, so the
absolutes are not comparable and the ratios are:

| reading | ADR 0016 §8.1–8.3 | re-taken 2026-08-18 |
| --- | --- | --- |
| total / floor | 37.8× | **38.5×** |
| interpreter share, residue charged back | 35.3% | **34.5%** |
| treewalk against machine, same pure request path | 2.73× | **2.82×** |
| `/health` plaintext against a floor replaying its 107 bytes | 16.8× | **15.7×** |

**R3 did not move what a request costs**, and it was not expected to: the work it
removed was already amortized over a served process's lifetime. It removed it
from the *count*, which is exact and does not move with a machine, and that is
where it is visible.

### What the front end pays, since both hoists move work forward

The concern is real — `ply test` pays the front end on a cold run — and the
answer is that the front end did not gain work. Both hoists are caches in front
of work that already happened lazily and now happens once per *program* instead
of once per *engine*; `region_kind::infer` is still not run at load
(`region_kind_hoisted.rs` asserts the analysis is unfilled until something opens
a region). Measured on this tree:

| | measured |
| --- | --- |
| `cargo test --workspace` | **3,584 passed, 0 failed, 4 ignored** across 137 test binaries + 13 doc-test suites, **359.7s** — one run, idle machine, not a best-of-N. Re-taken after the regression audit below at **3,597 / 0 / 4** across **138** + 13, **399.6s** and **406.9s** over two runs |
| `ply test examples/`, cold cache, release | 186 selected, 186 passed, **0.31s** wall |
| the same, warm | 1 selected, 185 cached, **0.04s** wall |
| 10,000-definition corpus, cold | **668.3 ms** total: parse 58.4, typecheck 162.9, hash 79.4, execute 331.6 |
| the same, warm | **362.3 ms**, 157 of 5,000 selected |
| lowering every test body in that corpus, once | **11.4 ms** (`ply-corpus measure`) |

**What I could not do is take the same numbers before R3.** Reconstructing the
pre-R3 tree needs git and this work was done under a rule forbidding it, so there
is no before-and-after wall clock here and it would be dishonest to imply one
from a document. The claim that survives is the narrow one: R3 added no phase and
no traversal to the front end, the phase split above is what a cold check costs
today, and it is re-takeable by anyone with `ply-corpus bench`.

### No regression

Re-run rather than assumed. Every row is a command whose output was read.

| invariant | measured |
| --- | --- |
| a rename selects zero deterministic tests | `selected 1 of 186 (185 cached)` after renaming `line_total` project-wide — identical to the nothing-changed run; the 1 is the `nondet` clock test |
| incremental and `--no-incremental` agree | byte-identical once wall clocks are stripped, 186 passed both ways |
| `--engine both` reports zero divergences | `audited 166 of 186 · 20 ran on one engine only`, no `E0503` |
| verdicts stable at `--jobs 1` and `--jobs 8` | 186 passed, 0 failed, both |
| `ply prove` reports honest tiers | 7 obligations · 2 proved · 5 property · 0 example, **7 held** |
| `ply hosts --host` lists the TCB | 25 host handlers · 47 operations |
| postgres transactions commit and roll back | `examples/same-tests.sh`: **29 requests byte for byte identical**, committed 201 with orders 3→4, rolled back 409 with the sequence still consumed |
| `Store::open` under 5 ms at 10,000 definitions | **1.79 ms** over 4,841 results, 9,821 definitions seen |
| simulation seed rate | **5,331–7,107 seeds/s** over 5 trials on a `--concurrent-tests` corpus; every exploration exhaustive, 54 interleavings after reduction from ≥4,096 (`ply-corpus sim`). That a seeded replay is *exact* is asserted by the suite, not by this row |
| the two-resumption trace cell reads 2 | `region_meaning_audit` (11 tests) and `resumption_semantics_audit` (11) all pass. The cell is pinned literally: `assert_eq(cell_get(c), 2)` at `crates/ply-eval/tests/region_meaning_audit.rs:167`, inside `two_resumptions_thread_one_state_rather_than_branching_it`, with the handle answering 21 |

The remaining invariants are asserted by the suite rather than re-run by hand,
and each has a name to grep for rather than a claim to take:
`ply-hash/tests/modules.rs::moving_a_definition_between_modules_changes_no_hash`,
`ply-cli/tests/cli.rs::moving_a_definition_between_modules_re_runs_nothing`,
`ply-test/tests/hybrid.rs::a_regression_that_introduces_runaway_recursion_is_bisected_to_its_culprit`,
`ply-eval/tests/secrets.rs`, and — for `E0412` on an unsimulated nondeterministic
effect in a `det` test —
`ply-cli/tests/cli.rs:570 a_nondet_test_in_a_det_test_is_a_compile_error`, which
runs `ply test --json` on a two-line project and asserts exit code 2 with
`diagnostics[0].code == "E0412"`. The suite is green above, which is what makes
those citations rather than promises.

### Two defects in the hoists, found by a regression audit and closed

R3 replaced two per-`Machine` recomputations with two per-program caches, and a
cache is a claim that an entry is still the right answer. Both caches were
audited after the milestone and both were wrong in a way the milestone's own
tests could not see, because those tests asserted that a *correct* handle is
shared. Neither cost a request an allocation and neither is visible in any figure
above; they are recorded here because the shape — a hoist that trades
recomputation for an invalidation condition — is the shape the next hoist will
have too.

| defect | what was wrong | what closed it |
| --- | --- | --- |
| `Lowering` keyed on a raw address, and the safety argument in its own doc comment was false | the argument needs `Lowering<'a>` invariant in `'a`; it was **covariant**, its only `'a`-carrying field being `&'a Program`, and `of` takes `&self` — so `&Lowering<'long>` coerced to `&Lowering<'short>` and accepted a body that does not outlive the cache. A `Box<Expr>` holding `111` keyed through that coercion and dropped, then a `Box<Expr>` holding `222` at the same address, was answered `111` on the **first** of a thousand attempts | an `invariant: PhantomData<fn(&'a Program) -> &'a Program>` field. Note `PhantomData<&'a mut Program>` does **not** do it — `&'a mut T` is invariant in `T` but covariant in `'a`, and it was tried first and compiled. The refusal is machine-checked by a `compile_fail` doc-test on the type, because a variance is a compile-time property no `#[test]` can observe |
| `region_kind` had no local-binder scope | `Analysis::definition` resolved a bare name against `Resolved::scopes[module]` — the *module* scope — so a parameter, a `let` or a pattern binder shadowing a definition's name was read as that definition, and the region inferred **`unique` over a callee that could be any closure in the program**. That is the one direction ADR 0017 §Consequences says inference may never be wrong in | `Analysis::locals`, a lexical scope stack pushed at every binder the language has. It over-approximates on purpose: a `Var` pattern naming a nullary constructor is read as a local, which costs precision and lands on `shared` |

Neither moved what a request allocates:
`./target/release/w6-alloc --repo . --requests 200` reads
`{"allocations_per_request":1081.87,"bytes_per_request":127954.65,...}` after
both, the same pair to the hundredth as before them. The region census did not
move either — still `113 regions, 0 unique, 113 shared` from
`cargo test -p ply-eval --test region_kind_inference --
the_split_over_the_repositorys_own_examples --nocapture`.

The same audit found `README.md`'s request-path allocation sentence stale for the
**second** time in this milestone, the second time inside the correction block
written for the first. That sentence is now the one line of prose in this
repository that a test reads —
`ply-corpus/tests/w6_report_allocations.rs:163
the_readme_still_describes_this_request_path`, both numbers, within 1%.
`docs/ONBOARDING.md` §7's checked/written boundary moved by exactly that line and
says so.

### What this means, said plainly

**The regions question is open.** The rule was written before the measurement
precisely so this outcome could be reported rather than argued away, and it is
the outcome. Three milestones after ADR 0017 asserted that the forkable world was
what kept allocations high, `/health` allocates more than it did before the
forkable world was removed, and the two things that were plausibly regions' fault
have been hoisted out and it still does.

What that does **not** license is ripping regions out. Two things the region
track bought are measured and are not in dispute. The **escape discipline** is a
safety property, not an allocation claim: the arena is what made a use-after-free
constructible in this language at all, and the brand is what refuses it at
compile time (`crates/ply-eval/tests/use_after_free_audit.rs`). And the **arena
beats the persistent map it replaced**, re-taken here with
`cargo test -p ply-eval --release --test region_arena_cost -- --nocapture`, which
prints for 10,000 cells:

```
  map:    build 20000 allocations, 1040000 bytes; 10000 allocations to write every cell
  region: build 0 allocations, 0 bytes; 0 allocations to write every cell; 0 to close
```

What is *not* established — and what nobody should now assert without a
measurement — is that removing the forkable world bought the request path
anything. A milestone that revisits this owes a number for what the world cost,
taken the way this one was, and ADR 0017's Context is where the untested version
of that claim is kept for comparison.

---

# What is next

**R3's decision rule fired on its second branch, and that is what sets this
queue.** Allocations per `/health` came back at **1,082** against a pre-region
**1,035** — measured, `./target/release/w6-alloc --repo . --requests 200` — so
the milestone that was planned to follow R3 is *not* automatically the right one.
§R3 is where the reading and its provenance are, and it should be read before
this list rather than after it.

M9 remains deferred on a measurement and is not the front of the queue; the
ladder was re-taken after R3 and the verdict did not move
(`./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`).

0. **Decide the regions question, because R3 reopened it and it is upstream of
   everything below.** The claim that motivated the whole track — that the
   persistent forkable world is what held allocations up — is now contradicted by
   three milestones of measurement, and the two hoists that could have explained
   the gap are gone from the request path. What is owed is a *number for what the
   world cost*, taken the way R3's was, against which "keep regions" or "the trade
   was not worth it" can be decided instead of asserted. Note what is not in
   question: the escape discipline is a safety property and the arena beats the
   persistent map it replaced by every measurement in ADR 0017 §"What must be
   measured". This item is about the request path and about nothing else. It is
   also the item most likely to be skipped, because everything below it is more
   fun and the previous entry in this position was skipped for exactly that
   reason.
1. **Unboxed primitive representation, and monomorphization.** R3's attribution
   is what now points at this, and more sharply than ADR 0017's did: with both
   compile-time passes hoisted off, the largest per-request allocation site is
   `frame::dispatch < Machine::step < Machine::call` at **415.0 allocations a
   request, 45.5%** of the marginal cost, and the rest are `Rc<Value>` boxes on
   the framing, routing and encode path rather than in any cell.
2. **Evidence passing and handler specialization.** A bound is measured and it is
   one of the three ADR 0016 §10.1 calls large enough to matter: re-taken after
   R3, the tree-walker beats the control-stack machine **2.82×** on the same pure
   request path (56.34µs against 158.92µs), which prices ADR 0005's
   four-heap-allocations-per-frame-push as a lever rather than restating it as a
   fact. The other two on that list — allocation, and framing at **101.41µs and
   15.4%** of a request in the re-taken ladder — are item 1 above and W2's
   precedent respectively, and none of the three has an end-to-end price yet.
3. **Re-measure codegen's ceiling — and do it before arguing about M9 again.**
   ~~and the ladder has not been re-taken since~~ — **it has now.**
   `benches/w6-ladder-r3.json` is the post-R3 take and the verdict machinery
   re-derives from it unchanged: interpreter share **35%** (34.3%–34.7%), ceiling
   **1.53×**, projection **1.46×**, `keep deferring M9`. Every absolute in that
   ladder is larger than W6's and the Rust floor moved with them (15.68µs →
   17.13µs), so read the ratios and not the microseconds. What is still owed here
   is the *spike* half, which cannot be re-taken at all: `crates/ply-codegen-spike`
   does not compile (`CONTRIBUTING.md` §"Things known to be broken" item 1), so
   `E = 1.46×` is a projection from a file rather than from a rebuilt spike.

Two smaller obligations are open and both are recorded above rather than in a
tracker: `crates/ply-codegen-spike` is still present and ADR 0016 §3.5 requires
its deletion, and `Machine::constant` disables the constant memo inside any open
*scheduler* region, which CONTRACTS.md calls a defect rather than a rule and
which costs a spawning service **1.78× on `/health`** — re-taken after R3, at
273.0µs sequential against 488.7µs spawning on the same service, and printed by
`ply-corpus w6` over `benches/w6-ladder-r3.json`.

The one thing a contributor should not do is re-argue M9 from the numbers in this
file. They were measured, they are re-derivable from
`benches/w6-ladder-r3.json` (or `benches/w6-ladder.json`, the pre-region
baseline) and `benches/w6-spike.json` by `ply-corpus w6`, and every one of them
is stale in the same direction: **every cheaper lever that lands makes M9's case
weaker**, and three have now landed where a code generator was predicted to be
the answer.

**The one thing this file should not be allowed to do is quietly close the
regions question.** It was reopened by a rule fixed in advance, by a measurement,
and it stays open until another measurement closes it. If a later revision of
this section presents item 0 as settled without a number beside it, that revision
is the defect.
