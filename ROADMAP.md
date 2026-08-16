# Ply — Roadmap

Each milestone has an exit criterion that is demonstrable, not aspirational.
M0–M4 are the vertical slice: the smallest system that proves the thesis end to
end. M5+ are the milestones the slice's architecture is shaped to accept without
a rewrite.

**Status: M0–M8 and W1–W6 are complete.** The web track closed at W6 with a
measured decision rather than a build. **M9 is the one milestone still deferred**,
and W6 deferred it on a number that says what would reopen it — see the M9 and W6
entries below.

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

## M6 — Full handlers, forkable state

Multi-shot continuations (evaluator moves to an explicit control stack).
Copy-on-write world state: build a fixture once, fork per test in microseconds.
This is where the unit/integration distinction is meant to stop mattering.

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

**Still deferred — decided by W6 against criteria written before the numbers
existed. The reason is not the one that carried this milestone through M0–M8.**

The old reason was that the interpreter is not the bottleneck. In a *test* run
that is still true and W6 re-measured it: at 10,000 definitions and 5,000 tests,
execute is **3.3% of a warm run** (10.4ms of 310.7ms) and typecheck plus hash
plus parse are 93%. In a *served* request it is less false than it looks, and
W6 measured that too: the interpreter is **35% of a request** (209.3µs of
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

**And what W6 measured that argues even the 1.48x is optimistic**, recorded so a
future contributor re-measures rather than re-argues: compiling `read_line`
alone and trampolining its two callees back into the machine gives **1.71x**, not
11.67x — coverage is a cliff, not a slope. The spike's fragment accepts **141 of
366** functions across `std.http`, `std.router` and `std.json`, refusing field
accesses, constructor patterns, lambdas and list literals — which is what
endpoints and derived codecs are made of. And `read_line`'s own directly
measured end-to-end value is **1.02x**.

`docs/adr/0016-w6-performance.md` §8–§11. The spike lives in
`crates/ply-codegen-spike`, in its own workspace, depended on by nothing; ADR
0016 §3.5 requires that closing W6 delete it, and its numbers survive in
`benches/w6-spike.json`.

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
  read; a `--config-schema` verified at start-up, exactly as W4 verifies a
  database schema, so a missing credential is a refusal rather than a 3am 500
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
spike crate has not been deleted, and the memo is refused inside any open region
— so a service whose accept loop calls `task.spawn` memoizes nothing, measured at
**1.00x against 1.77x** on the same route.

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
  asserts informally about its test doubles.
- **An endpoint's footprint is inferred and exact.** Which tables a route
  touches comes from the type, not a comment — giving exact test isolation and a
  static answer to "what writes this table".
- **Concurrent request races become findable.** M7 finds a check-then-act race in
  two interleavings and returns a seed. Pointed at two requests hitting one row,
  that is the bug class that otherwise surfaces in production at 3am.
- **`ply review --changed` on an API** answers whether any endpoint's *specified*
  behaviour moved, without reading the diff.

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
