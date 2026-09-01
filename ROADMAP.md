# Ply — Roadmap

> **How to read an exit criterion here (docs pass, 2026-08-17).** "Demonstrable"
> below means *a test demonstrates it on a machine that can run that test*.
> **Five** conditional skips exist, and each returns a **passing** result when
> its dependency is absent:
>
> | gate | where | when it skips | says so? |
> | --- | --- | --- | --- |
> | `cluster::available()` | `crates/ply-host-tests/tests/support/cluster.rs:38` | no `initdb`/`postgres` on the machine | yes, on stderr of a passing test |
> | `PLY_PG_URL` | `crates/ply-host/src/db/scope/tests/live.rs:101` | the variable is unset — nothing sets it on a stock local checkout, and **CI sets it** | yes, on stderr of a passing test |
> | `#![cfg(unix)]` | `crates/ply-cli/tests/suite/w5_shutdown.rs:18` | non-Unix host | **no — the file is not compiled and nothing is printed** |
> | its own `[workspace]` | `crates/ply-codegen-spike/Cargo.toml` | always, under `cargo test --workspace` | no |
> | `PLY_TEST_DB` | `crates/ply-host/src/db/pool/tests.rs:25` | the variable is unset | **no — nothing is printed at all, on either stream** |
>
> **This table describes a stock local checkout, not CI.** All five gates are
> supplied in CI and the first four are *asserted* open — the run fails if any of
> them skips. §W4's exit criterion carries the gate-by-gate reading.

Each milestone has an exit criterion that is demonstrable, not aspirational.
M0–M4 are the vertical slice: the smallest system that proves the thesis end to
end. M5+ are the milestones the slice's architecture is shaped to accept without
a rewrite.

**Status: M0–M8, W1–W6, and R1–R4 are complete.** The web track closed at W6 with
a measured decision rather than a build. The region track that followed it landed
in four parts — R1 built the machinery, R2 connected it, R3 took the
compile-time work back off the request path, R4 changed how a value is built —
and it removed the forkable world M6 shipped; see the region track below, and
read M6's entry with that correction in hand. **R3 ended on a decision rule
fixed before it started, and the rule went against the design**: allocations per
`/health` were still above the pre-region baseline, so whether the regions trade
was worth making is open rather than settled. §R3 is where that is recorded and
it is not a formality. **R4 was requested on a premise that turned out to be
false and reported that first**; its own rule then answered `Short` on the lever
that worked, because the share the ADR placed under it was wrong and the
threshold was left alone rather than edited. §R4.

**M9 is the one milestone deferred on a measurement.** W6 deferred it against
criteria fixed in code before any number existed, and the deferral carries the
numbers that would reopen it, so a future contributor re-measures rather than
re-argues — see the M9 and W6 entries. It is *not* the only thing unbuilt: ADR 0017 "Not in this ADR" names four separate milestones, three of which come
*before* M9 and one of which is M9. **[What is next](#what-is-next)** is at the
foot of this file and is the entry a reader continuing this project should start
from.

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
threaded state the thing ADR 0017 must not move.

Copy-on-write world state: build a fixture once, fork per test. **R2 deleted
it.** `World` is gone from `ply-eval`, a `Value::Cell` is a `Slot` in a
`TaskRegions` arena, and `ply_test::region::GroupRegion` no longer forks — a
fixture is built once *per worker* and mutated in place. Read the region track
below before relying on anything in this paragraph.

Two numbers this entry used to imply, corrected rather than dropped, because the
trade R2 took is only legible with both:

- "fork per test in microseconds" understated it. `World::fork` was one pointer
  clone at any fixture size, which ADR 0017 records as **1 ns** and as making
  fixture reuse 8,939x cheaper than rebuilding a 10,000-cell fixture. *Neither
  figure is re-checkable — `World` is deleted, so both are inherited from ADR
  0017 rather than verified here.*
- What replaced it is not free. Opening a 10,000-cell region-scoped fixture and
  writing one cell costs **about 100 µs per test**: ADR 0017 published
  95.7 µs, and this audit re-ran
  `crates/ply-eval-tests/tests/allocation/fixture_open_cost.rs` and measured **105.8 µs**. Read it
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

**A code generator ships; the milestone's criteria are still not met, and those
are two different statements.**

`ply test --backend cranelift` installs a real cranelift JIT from the shipping
workspace — no feature flag, no second toolchain. It compiles the ADR 0011
fragment and the machine enters it.

**What decides whether that is worth anything is fragment coverage, not backend
speed.** On a compute kernel, which is almost entirely inside the fragment, it is
several times faster. On a program built out of the standard library it enters a
fraction of a percent of the calls it is offered, and the fixed cost of analysis
and code generation exceeds what entering them saves — so the run is *slower*.
Both directions are measured; ADRs 0026 and 0030 carry the series, the
pre-registration, the null controls and the load caveats.

**What that discharges is ADR 0026's precondition — "a backend must be
policeable before it is fast" — and not M9's criteria.** The eight deliberately
wrong backends now run against a real code generator from a shipping command.
**C3 is untouched**: nothing cheaper has been priced on the workload being
decided.

**And the request path is still the workload being decided.** The interpreter is
roughly a third of a request, so even an infinitely fast backend sits under the
bar fixed before any of these numbers existed. That is Amdahl, not Cranelift.
[ADR 0011](docs/adr/0011-the-web-track.md) is the argument.

> **The trap this section exists to prevent.** It is tempting to read a
> disappointing multiplier as a number to improve. The multiplier is a
> *consequence* of where the time is, and it moves when the fragment covers more
> of a real program — or when the front end, which dominates the warm loop, gets
> cheaper. Argue about coverage and about which workload is being decided.
> Re-deriving the ratio on the same workload settles nothing.

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

`docs/adr/0010-generic-derivation.md`, `docs/adr/0011-the-web-track.md`

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

`docs/adr/0011-the-web-track.md`. Not in W4: query building, an ORM,
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
  `crates/ply-cli/tests/suite/config_cli.rs:265`). *This line used to read "verified
  at start-up, exactly as W4 verifies a database schema". The comparison was
  wrong in the direction that flatters: `--config-schema` really is verified,
  and `--db-schema` is not — it evaluates the program's `Schema` function and
  reports its shape, and never asks the server. `E0435` is raised nowhere. See
  the audit notes in `docs/adr/0011-the-web-track.md` §7 and §8.*
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

`docs/adr/0011-the-web-track.md`. Not in W5: metrics backends, log shipping,
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

`docs/adr/0011-the-web-track.md` §8–§12. Not in W6, and held for the codegen
backend: whatever the verdict, none was built. **Not held for "optimizing
anything":** the constant memo landed in `ply-eval` between W6's two takes, which
is one of ADR 0011's own cheaper levers built rather than priced, and the
ladder was re-taken on the tree that has it. Two obligations are still open: the
spike crate has not been deleted, and the memo is refused inside any open
**task/simulation** region — so a service whose accept loop calls `task.spawn`
memoizes nothing, measured at **1.00x against 1.77x** on the same route.

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
  its whole state back through a persistent map. The substitution ADR 0011
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

It also arrived through a feature that looks like nothing. An `effect set` is
"an abbreviation for a row", and letting one cross a module boundary would have
let an edit in the declaring module leave a stale published footprint behind a
skipped incremental recheck — an under-reporting footprint, which is exactly this
failure. W3 refused it, and W6 rewrote ADR 0009 to say so, because the ADR still
showed the form the implementation rejects.

---

# Region track — R1 through R5

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

`docs/adr/0017-regions.md`, which supersedes ADR 0005 and amends ADR 0008

**It landed in three parts, and the split is the instructive part.** Describing
regions as one milestone would hide the defect the shape produced, so all three
are below.

**R4 and R5 continue the numbering and are not region milestones.** R4 is value
construction and R5 is compiled-code entry; both are here because each was what
the previous milestone's measurement pointed at, and moving them elsewhere would
break the chain that explains why they were scoped the way they were. The track
is about the representation and the execution strategy under it, and it stopped
being about regions after R3.

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
  binding releases. Cycles are not collected; ADR 0017 accepts that and
  supplies diagnostics where a cycle is constructible

**And none of it was connected to anything.** No engine consulted the kinds, no
`with_cell` allocated through the arena, and no region ever closed. The gap was
found by a benchmark rather than by a report, which is stated in the tests that
now exist to prevent its recurrence —
`crates/ply-eval-tests/tests/suite/cell_arena_wiring.rs:5` ("R1 built the allocator and
connected nothing, and a benchmark rather than a report is what found that out")
and `crates/ply-eval-tests/tests/suite/region_wiring_audit.rs:6`. R1's own tests were green
throughout, because every one of them attacked the allocator or the analysis
directly and none asked whether an engine had ever called either.

**R1 also shipped a false claim, and it is the seventh of the defects this
project's reviews have found.** ADR 0017's first draft said each resumption
observes the region as it was at capture and asserted that this "is exactly ADR 0005's semantics". It was not, and the two readings are distinguishable **in one
integer** on the section's own worked example: ADR 0005 threads one state and
pins the two-resumption example at `30` with its trace cell at `2` as a required
test, and snapshot-at-capture answers `1` for that cell. Since ADR 0017's
governing property is that program meaning does not change, ADR 0005 won and §3
was rewritten to say so. The retraction is in the ADR rather than hidden by a
deletion, and the discriminating programs are landed in
`crates/ply-eval-tests/tests/suite/region_meaning_audit.rs` and
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
  still reach the slots — which is why `crates/ply-eval-tests/tests/suite/use_after_free_audit.rs`
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
`ply-eval-tests/tests/suite/region_kind_sharing.rs`, `ply-eval-tests/tests/allocation/lowering_sharing.rs` and
`ply-corpus-tests/tests/suite/region_kind_hoisted.rs`).

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
`crates/ply-corpus-tests/tests/suite/region_kind_hoisted.rs`'s header — which says in the
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
| the same, against the file that holds the baseline | *"the report says 1035 allocations and 0.12 MB per /health request; this tree makes 1082 and 0.128 MB"* | `cargo test -p ply-corpus-tests --release --test allocation -- w6_report_allocations --nocapture` |
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
repeats), ceiling **1.53×**, projection **1.46×**, six of ADR 0011's seven
levers unpriced.

The absolutes are all larger than ADR 0011's, and the control says why:
the **Rust floor**, which has no Ply in it at all, moved from 15.68µs to
**17.13µs**. This box measures about 9% slow against the one W6 used, so the
absolutes are not comparable and the ratios are:

| reading | ADR 0011 | re-taken 2026-08-18 |
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
| the two-resumption trace cell reads 2 | `region_meaning_audit` (11 tests) and `resumption_semantics_audit` (11) all pass. The cell is pinned literally: `assert_eq(cell_get(c), 2)` at `crates/ply-eval-tests/tests/suite/region_meaning_audit.rs:167`, inside `two_resumptions_thread_one_state_rather_than_branching_it`, with the handle answering 21 |

The remaining invariants are asserted by the suite rather than re-run by hand,
and each has a name to grep for rather than a claim to take:
`ply-hash-tests/tests/suite/modules.rs::moving_a_definition_between_modules_changes_no_hash`,
`ply-cli/tests/suite/cli.rs::moving_a_definition_between_modules_re_runs_nothing`,
`ply-test-tests/tests/suite/hybrid.rs::a_regression_that_introduces_runaway_recursion_is_bisected_to_its_culprit`,
`ply-eval-tests/tests/suite/secrets.rs`, and — for `E0412` on an unsimulated nondeterministic
effect in a `det` test —
`ply-cli/tests/suite/cli.rs:570 a_nondet_test_in_a_det_test_is_a_compile_error`, which
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
| `region_kind` had no local-binder scope | `Analysis::definition` resolved a bare name against `Resolved::scopes[module]` — the *module* scope — so a parameter, a `let` or a pattern binder shadowing a definition's name was read as that definition, and the region inferred **`unique` over a callee that could be any closure in the program**. That is the one direction ADR 0017 says inference may never be wrong in | `Analysis::locals`, a lexical scope stack pushed at every binder the language has. It over-approximates on purpose: a `Var` pattern naming a nullary constructor is read as a local, which costs precision and lands on `shared` |

Neither moved what a request allocates:
`./target/release/w6-alloc --repo . --requests 200` reads
`{"allocations_per_request":1081.87,"bytes_per_request":127954.65,...}` after
both, the same pair to the hundredth as before them. The region census did not
move either — still `113 regions, 0 unique, 113 shared` from
`cargo test -p ply-eval-tests --test region_kind_inference --
the_split_over_the_repositorys_own_examples --nocapture`.

The same audit found `README.md`'s request-path allocation sentence stale for the
**second** time in this milestone, the second time inside the correction block
written for the first. That sentence is now the one line of prose in this
repository that a test reads —
`ply-corpus-tests/tests/allocation/w6_report_allocations.rs:163
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
compile time (`crates/ply-eval-tests/tests/suite/use_after_free_audit.rs`). And the **arena
beats the persistent map it replaced**, re-taken here with
`cargo test -p ply-eval-tests --release --test allocation -- region_arena_cost --nocapture`, which
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

## R4 — value construction, and a milestone whose premise was false

`docs/adr/0019-value-representation.md` is the record;
`docs/adr/0018-compute-kernel-performance.md` §1 and §2 are what it answers.

### The premise was measured first, and it did not hold

R4 was requested as **unboxed primitives**, on ADR 0018's two sentences:
*"Every `Int` is a heap-allocated `Value`. `interp::literal` allocates 111 times
per request on a workload doing almost no arithmetic."*

The first is false — `Int`, `Bool`, `Float`, `Unit`, `Decimal`, `Cell` and
`Task` are inline variants of a 32-byte enum and building one touches no
allocator — and the second is a **20-request window** fitting to 65.0 per
request plus 925 once per `Machine`, which is one-time work divided by twenty.
Both are corrected in place in ADR 0018 with the originals beside them.
`cargo test -p ply-corpus-tests --release --test r4_value_construction -- --nocapture`
prints the zeroes by name.

**So the milestone as scoped had nothing to remove**, and this is R3's lesson
arriving a second time: `CONTRIBUTING.md` §"Measure an ADR's motivating claim
before accepting the ADR" is where it is written down, and it was written down
after R3 and did not stop R4 being scoped this way.

### What was built instead, and what it moved

An attribution by **the value being built** rather than by the frame that built
it, fitted over a 20- and a 200-request window so a per-`Machine` intercept
cannot masquerade as per-request work. It ranked three changes and refused a
fourth, with a floor under each fixed in `ply_corpus::r4` before any of them
existed. Two landed:

- **§1, a free list for the call-argument vector** — the largest line on both
  routes, 372.4 per request.
- **§2, a compile-time constant's `Value` built once** — a literal's value on
  the lowered node, and one `Value` per constructor mention per thread.

`/health` went from **1,082 to 773.4** allocations per request, re-taken with
`./target/release/w6-alloc --repo . --requests 200`, three runs byte-identical.
`README.md` §"Where this is not competitive" carries the correction and the
per-lever split.

### The rule fired again, and again it was left alone

`ply_corpus::r4::judge` answers **`Verdict::Short`** on §1: the floor was 20% of
the request, derived before the lever from an attribution that assumed every
transient buffer reaches `Machine::enter_code`. It does not — `builtins::call`
takes its `Vec<Value>` by value — so the most the mechanism could ever remove is
178.0/911.5 = **19.53%**, and it removed all of it. **The floor was not edited
and neither was the attributed share.** That is the whole point of putting a
threshold in code: a documentation defect reported as `Short` is worth more than
a passing number nobody can check. ADR 0019 carries the four-way split
(178.0 recycled / 31.0 retained as `Ctor.args` / 23.0 too wide / 140.4 freed
where they cannot be handed back) and says what the next lever is.

§3, a record's fields in one allocation, is **ranked and priced and not
accepted**: it waits on a record-width histogram that does not exist. §4,
narrowing `Value` below 32 bytes, is **rejected with its number** — it would
save bytes and **zero** allocations, and cost one allocation per applied
constructor.

### ADR 0018 is discharged, and it inverted that ADR's ordering

The codegen spike compiles again (`+1.94.0`; see `CONTRIBUTING.md` §"Things
known to be broken" item 1) and was re-priced against a real compute kernel —
`benches/kernel/`, three-heap Nim under MCTS, in Ply, passing
`ply test benches/kernel/ --engine both`. `benches/adr0018-mcts.json` is the
report.

The premise held on **shape**: 81.0% of the kernel's executed work is inside the
compiled fragment, against the 2–5% ADR 0011 measured for an HTTP request. The
conclusion did not: end to end the hybrid is **0.998× [0.979–1.007]** against a
floor of 1.000× [0.994–1.009], because **the interpreter cannot call compiled
code** — a function the fragment accepts whose callers it refuses is compiled
and never entered. The Amdahl ceiling over the two measured numbers is
**4.86×**, not 11.67× and not the 52.58× the fragment shows where it runs. And
a lever ADR 0018 does not list outranks most of the ones it does: Ply ships no
`sqrt` and no `ln`, so the kernel computes its own, at 28.34µs a call against
1.35µs without — **≈2.5× on the whole kernel from two prelude builtins**.

ADR 0019 lists the six things an amendment to ADR 0018 owes. Two are
discharged; **the other four are open**, and the one that matters is that a
backend the interpreter cannot enter buys nothing whatever the representation
is.

### What R4 could not do

Take the same numbers before R4. This work was done under a rule forbidding git,
so the per-lever A/B deltas in `README.md` are as the build agents measured them
and are not re-derivable from this tree without re-editing the two function
bodies by hand. The claim that survives is the narrow one: `w6-alloc` reads
773.4 here.

### A defect R4 did not cause and R4's audit closed: a `Map` key was a function of insertion history

## R5 — the interpreter can enter compiled code, and three of four reviews refuted the write-up

`docs/adr/0018-compute-kernel-performance.md` §0.5 is the record;
`benches/r5-timing/` holds the pre-registration, the raw report and the results.
ADR 0018 said **"make the interpreter able to enter compiled code, or the
ceiling is 5.26× however much of the fragment you accept"**, and named that as a
different first milestone from anything in its own list. R5 is that milestone.

### What was built, and it is not a feature

`crates/ply-eval/src/compiled.rs`: a `Compiled` trait that takes a name, some
scalars and a call budget and gives back at most one scalar. No arena, no stack,
no handler stack, no host binding, no `&mut Machine` and no route back in, so a
backend that cannot finish a call has changed nothing the program can observe
and declining is free by construction. One branch in `Machine::enter_code` is
the whole shipping delta.

**No shipping command can install a backend, and this is the sentence to read
first.** `Compiled` and `set_compiled` appear nowhere in `ply-cli`; outside
`crates/ply-eval`'s own tests and `crates/ply-codegen-spike`, which ADR 0011 requires be deletable, `set_compiled` has **no caller in `crates/*`**. So
`ply test --engine both` cannot attach one, and the rule that a backend run is a
third execution strategy whose results the result cache must not keep — written
down on `Machine::set_compiled` — is **not enforced, because it is
unreachable**. **Ply does not ship this.** Everything below is a measurement at
a seam only the spike's harness and `ply-eval`'s differential corpus can reach.

ADR 0011 said deferring M9 "deletes one feature block and one dependency
line, and nothing else in the workspace knows it existed". After R5 that clause
is false — `compiled.rs`, the trait, `set_compiled`, three counters on `Machine`
and a branch on every interpreted call all survive the `rm -r` — and it is
corrected in place there. The first half of §3.5 was verified by **performing**
the deletion rather than arguing it: `rm -r crates/ply-codegen-spike` in a copied
tree, then `cargo build --workspace --all-targets` and `cargo test --workspace`
green, 155 test binaries, **3,680 passed, 0 failed**, with the seam's 25 unit
tests and both differential-corpus backends still among them.

### What it costs on the workload that ships

**0.0 allocations per `/health` request.** `HOOK` (the tree as it ships) against
`NOHOOK` (the same tree with the `enter_code` call site deleted), two binaries
from one frozen tree, arms alternated `H N H N H N`: both read **773.4** at 200
requests, byte-identical including `bytes_per_request`, and identical again at 20
and 2,000 requests. Pre-registered rule, decided before any number existed:
`HOOK − NOHOOK > 0.0 allocations` ⇒ regressed. **Not a regression.** In the
linked binary the hook is 80 bytes of machine code. R4's 773.4 is unmoved, so
this doubles as R4's staleness guard.

Zero allocations is not zero cost. Instrumented, one `/health` request reaches
the hook **237.87 times** and every one is a miss — `compiled_answer` exits on
its first line, because nothing can install a backend. **The wall clock of those
237.87 branch tests was never measured**, on either binary, although both
existed: the currency was pre-registered as allocations and a wall-clock rule was
not. That is the honest shape of this result and not a technicality — a
deterministic quantity known in advance to be zero is a check, not an experiment.

### What it bought on the kernel

`benches/kernel/`, the same three-heap Nim MCTS R4 measured. Load gate **2.63**
against a pre-registered 4.5; all 84 ladder windows sampled between 2.40 and
2.91, so **no window was dropped by either pre-registered filter on any rung**.
Controls, both required in 0.95–1.05: harness floor **0.9995×**,
nothing-enterable **0.9758×**.

| rung | ratio | 10th–90th | entries/call |
| --- | ---: | --- | ---: |
| control: nothing enterable | 0.976× | — | **0** |
| the exploration term | 2.860× | [2.835, 2.871] | 1,275 |
| + the playout | 6.176× | [6.139, 6.197] | 2,161 |
| **everything the fragment accepts** | **6.199×** | **[6.143, 6.226]** | **2,162** |

**6.199×, [6.143, 6.226], over 21 of 21 surviving paired windows, 2,162 native
entries**, pre-registered verdict `entry-paid-off`. Pre-R5 the same rung was
**0.998× with 0 entries**, and the ladder is monotone in entries. Two reviewers
replicated the top rung independently at load 5.3–6.8 (6.215×) and at load 12–16
(6.240×) — both formally void under the pre-registration's own load rule, and
reported as direction and magnitude rather than as a second result. A reviewer
attacked it directly, controlled for the arm order R4 alternated and R5 did not
(≤3%, sign flips with load), and could not break it.

**So ADR 0018's diagnosis was right: the binding constraint was
architectural.** One-way calling was what made 81% of executed work inside the
fragment worth 0.998×.

### 6.199× is above the ceiling that ordered ADR 0018, and the ceiling is what was wrong

ADR 0018 puts the Amdahl ceiling at **4.86×** for an enterable backend and
**5.26×** at an infinitely fast fragment. 6.199× is above both, taken with 19 of
34 functions accepted — nowhere near "however much of the fragment you accept".
**A number that beats its own predicted ceiling means the model was wrong, not
that the result is extra good**, and the same report holds the number that says
how: interpreted, the search offers **45,586** calls to the hook per
`mcts.plan_753(100)`; compiled, it offers **2,266**. **43,320 interpreted calls
per search stop existing.**

The ceiling's denominator priced each function's *body* in isolation, charging
the call-site machinery — argument vector, frame push, `Env` binding — to a 19.2%
"unattributed" bucket belonging to no function. Entry deletes that machinery too,
and arrival is the cheapest part of an interpreted call. ADR 0018 withdraws its
ceiling as a bound in §0.5.

**The same fact is also a correctness defect, read from the other side**: a
compiled body pushes **one** frame where the interpreter pends many. That is
finding 1 below, and it and the ceiling are one fact.

### A wrong backend was pointed at this for the first time

R5 as delivered argued the corpus could detect a wrong answer and never
demonstrated it. `crates/ply-codegen-spike/tests/mutations.rs` now runs eight
deliberately wrong backends and names what caught each; the table is
reproducible and a reviewer reproduced three rows digit for digit
(`--mutate off-by-one`: **1,635 disagreements**, first
`mcts.heap case 0: result value — left 0, right 1`). **Two were not caught.**

- **A backend that ignores its call budget** is a stack overflow, exit 134,
  before a single case is compared. The bounded form (4× fuel) *is* caught — on
  the diagnostic-label axis only, so a harness scoring `(Err, Err)` as agreement
  would have passed it.
- **The published-row gate is untestable by every corpus in the tree.**
  `benches/kernel` declares no effect at all; `ply-eval`'s differential corpus
  declines effectful names. If that gate regresses, both corpora report success.

And the sharpest row is not a mutation at all: a uniform off-by-one in
`mcts.ucb` — 2,156 wrong scores — is reported **only** by the 84 cases generated
directly against `mcts.ucb`. No caller and none of the 24 whole-kernel searches
notices, because UCB feeds an argmax. Measured separately: **12 of the 19
compiled functions are offered to the backend zero times during those 24
searches.** The whole-kernel leg of R5's agreement result is much weaker than it
sounds, and half the entered functions are policed by their own generated cases
and by nothing else.

`ply test --engine both` catches **none** of the eight, on any corpus, for any
mutation, because it cannot install a backend.

### Three of the four review lenses refuted the write-up

Reported here as prominently as the 6.199×, because that is what this file is
for. All three are open in `CONTRIBUTING.md` §"Things known to be broken", items
9 through 13.

**1. The seam carries one of the machine's two resource bounds.** With the real
cranelift backend, no mutation, one entry and zero declines:

```
pub fn hog(n: Int) -> Int =
  if n == 0 { 0 } else { hog(n - 1) + 1 + 1 + ... }     // 150 "+ 1"

machine alone:   Err("recursion limit of 1000000 pending frames exceeded")
machine + spike: Ok(1350000)
```

`compiled_answer` hands over `budget = max_calls - stack.calls()` and nothing
else; `DEFAULT_MAX_FRAMES = 1_000_000`, enforced in `push`, cannot be expressed
at the boundary and no backend can honour it. Reproduced inside `ply-eval` with
a hand-built honest backend, and with `with_max_frames(64)` and no recursion at
all. `DEFAULT_MAX_FRAMES`'s own doc asserted the opposite — *"every recursion
reaches [`DEFAULT_MAX_CALLS`] first, since a call costs at least one frame"* — and
that is false: a **call** costs one frame, a **body** costs as many as it pends.

**2. The per-function regression was the backend, and R5 blamed its own filter.**
`benches/r5-timing/RESULTS.md` §3 reported `mcts.playouts` at 0.068× — a 14.7×
regression — then withdrew it as an artifact of R5's own stall filter, saying
*"I did not identify the mechanism — only that it is the arm interleaving and not
the backend"*. It is the backend. `crates/ply-codegen-spike/src/rt.rs`,
`Ctx::begin`, runs `slots.clear()` then `shrink_to(RETAINED_SLOTS)`, so **every
entry costs O(the previous entry's peak arena)**. Measured best-of-7, twice, at
two loads: the identical hybrid call `mcts.playouts(0,0,0)` runs in 0.375 µs
after a 4-slot predecessor and 68.083 µs after a 19,584-slot one — **181×**,
monotone, ~3.5 ns a slot, with no carry-over on the interpreter arm. §3 is
corrected in place and these sentences are withdrawn by name: *"no function of
the 26 is below 1.00×"*, *"nothing in this kernel is too small to be worth
entering"*, and *"6.199× is if anything an underestimate"*. The aggregate is
unaffected — it is filter-independent and was replicated twice — but it no longer
carries a sign.

A second row that table cannot show at all: its argument-set selection skips any
set the interpreter raises on, which is exactly the fuel-decline path. Re-taken
2026-08-22 with the shipped binary, `mcts --dir benches/kernel --probe machine`
is **0.17 s** against `--probe compiled` at **11.82 s** — ~**69× slower with a
backend attached** on a program that is about to raise either way. ADR 0018
records it and RESULTS.md never mentions it.

**3. A definition that handles its own effects is offered to a backend.**
`effects.handled` in the spike's own hazard fixture performs two operations and
discharges them under its own `handle`; it is declared `-> Int` with no row and
type-checks, and both `footprint` and `performed` come back **empty**, so the
purity gate clears it. A probe had it offered and answered, and the corpus
reported `effects.handled: observed footprint — left {effects.tally.read[log],
effects.tally.write[log]}, right {}` — character for character the evidence R5's
own mutation table reports for *deleting* that gate from shipping code. The gate
is in place; it does not apply. Latent only because the one backend in the tree
refuses `handle` at compile time — which is a backend remembering an invariant
`compiled.rs` claims to enforce by construction. That bullet is narrowed in
place. It also reaches past the seam: `ply-test` reads a declared-but-unobserved
atom as "a branch was not taken", so an entered definition would say a branch was
not taken when it was.

**The fourth lens held**, and it held after being executed rather than argued:
the deletability of the spike, the falsification table's reproducibility, and the
0-allocation hot-path tax all survived (see §"What was built" above).

### A defect R5 did not cause and R5's review found

> **Not an R5 regression** — it predates the seam and no backend is involved —
> but the probe that found finding 1 found it, and it is a `--engine both`
> divergence, which is the failure mode this project treats as never a warning.
>
> The two engines bound recursion at different things: the tree-walker at nested
> calls, the machine at nested calls **and** at 1,000,000 pending frames. A body
> pending more than `1_000_000 / 10_000` = **100 frames per call** hits the frame
> bound first, and the tree-walker has no such bound. With the shipping binary
> and no backend:
>
> ```
> $ ply test /tmp/zzhog.ply --engine both
>   PANIC a recursion whose body pends 150 frames a level
>     no culprit: the interpreter failed rather than the program; this is a
>     defect in Ply
>     `treewalk` and `machine` disagree
>     = treewalk: passed
>     = machine: [E0502] recursion limit of 1000000 pending frames exceeded
> ```
>
> Crossover measured at depth 9,990: k = 90 passes, k = 100 raises.
> `crates/ply-eval-tests/tests/suite/equivalence_audit.rs::the_two_engines_agree_on_the_recursion_bound`
> therefore holds only below that ratio — both its programs pend two frames a
> level — and its name and doc claimed the general statement.
>
> > **Audit note (docs pass, 2026-08-24): the sentence above ended "The doc is
> > narrowed; the test is **not** changed to assert the divergence, so **nothing
> > in the suite arms the true bound**."** Something arms it now. The machine no
> > longer carries a default frame ceiling, so both engines bound the same one
> > thing, and
> > `equivalence_audit.rs::the_two_engines_and_a_backend_agree_however_many_frames_a_body_pends`
> > compares the tree-walker against the plain machine on a body pending 150
> > frames a call at depth 6,700 — 1,011,700 pending frames, past where the
> > ceiling sat. Restoring the ceiling makes it fail with the divergence quoted
> > above, checked by doing it — though not by the lane that wrote the fix: the
> > revert was taken by the orchestrating session in an `rsync` copy of the
> > worktree under `CARGO_INCREMENTAL=0`, because the lane's own tree would not
> > hold still long enough for the result to mean anything.
> > `CONTRIBUTING.md` item 10 carries the provenance and why it reads that way.
> > That test is also the memory-heaviest in the crate, at **4,858 MiB** peak
> > RSS and 15.5s in a debug build, which is inherent: the machine's frame stack
> > is the tree-walker's native stack reified one for one, so no cheaper program
> > crosses a ceiling of a million frames.

### What R5 did not do

Kept as a list because each line is a gap somebody could close, and because the
*kinds* recur:

- **Wall clock on `/health` with and without the hook.** Both binaries existed
  and the A/B was never run.
- **Any allocation figure with a backend attached.** The speedups are wall clock
  only.
- **Replicate.** One run, one kernel, one program, one box. The pre-registration
  forbade re-running, so the reported run is a sample of size one, and the two
  later replications were taken above the load gate and are formally void.
- **Test its own filters.** No window was dropped by either pre-registered
  filter on any rung. The load filter *cannot* fire as written: the 1-minute
  average updates on a timescale far longer than one window, so it is a per-rung
  gate wearing a per-window filter's clothes. Where the stall filter did fire it
  produced a false regression.
- **Source the decisive gate reading.** The load figure the write-up quotes
  appears in no data file; the provenance line and the analysis script disagree
  with it and with each other. Nothing turns on it because the gate passes on
  all three readings — but the input to a *refusal rule* was prose only.

**Two of those generalise and are worth carrying into any measured milestone
here.** A filter that never fires has not been shown to work — arrange for it to
fire once on purpose. And a number that a decision rule reads must live in the
data file the rule reads, not in the write-up.

## Compute kernels — is Ply ever the right choice for MCTS-shaped work?

`docs/adr/0018-compute-kernel-performance.md`. Proposed, nothing accepted.

The probe: a maximally-performant Monte Carlo tree search library. Almost pure
compute — a tight loop over a mutable tree, millions of iterations, hot RNG,
parallel rollouts contending on shared nodes. It exercises every place Ply is weak
and almost none of where it is strong. The answer today is Rust, by roughly an
order of magnitude.

The gaps, and what the ADR plans for each: boxed primitives, interpreted dispatch,
no unboxed mutable arrays, per-operation effect dispatch, no monomorphization, no
vocabulary for shared mutable state across tasks, and no SIMD or layout control.

**The first step is a measurement, not a build.** ADR 0011 priced the codegen
spike at 11.67x on its compilable fragment and 1.02-1.05x end to end, because that
fragment is 2-5% of an HTTP request. An MCTS inner loop may be *mostly* that
fragment. Re-pricing the existing spike against a kernel is cheap, and every other
item is ordered on an assumption only that measurement can test.

Available today, and it is the design working rather than a concession: the kernel
in Rust behind a host handler with a declared footprint, the strategy and
experiment harness in Ply. The boundary costs 0.5 us per crossing, measured.
