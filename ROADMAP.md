# Ply — Roadmap

Each milestone has an exit criterion that is demonstrable, not aspirational.
M0–M4 are the vertical slice: the smallest system that proves the thesis end to
end. M5+ are the milestones the slice's architecture is shaped to accept without
a rewrite.

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

Deferred, but see W6: serving inverts the argument below, and the web track is
what would pull this forward.

Cranelift or LLVM backend. Deferred deliberately — the interpreter is not the
bottleneck the language exists to solve, and a fast interpreter with a perfect
cache beats a fast compiler with a cold one.

---

# Web track

M0–M8 built a language that can prove things about programs that never leave
memory. The web track is what it takes to serve an HTTP API, and the ordering is
driven by one fact: **Ply currently has no I/O at all**. Not limited I/O — none.
Every handler ever written for it is in-memory or simulated.

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

`docs/adr/0009-effect-set-aliases.md`

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

## W5 — Operations

- Observability as an effect, so tracing is handled rather than ambient
- Config and secrets, with secrets typed so they cannot reach a log handler
- Graceful shutdown and connection draining
- Deployment: content addressing means a deploy could ship only the definitions
  whose hashes changed. Nothing about this exists yet and it may not be worth it

**Exit:** a service that can be deployed, observed, and shut down without losing
in-flight requests.

## W6 — Performance, and whether M9 comes forward

M9 was deferred because execution was a few percent of a warm test run. Serving
inverts that argument, and the control-stack machine costs four heap allocations
per frame push.

Most web APIs are I/O-bound, so an interpreter may well be fine — but that is a
hypothesis, and W1 produces the number that tests it.

**Exit:** request throughput and tail latency under real load, and a decision on
M9 made from measurement rather than from the assumption that carried M0–M8.

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
**silently** rather than loudly. Every dangerous defect found across seven
audited milestones was a green result over unexplored space, never a crash. The
host boundary is where that failure mode is easiest to reintroduce and hardest to
detect, and it deserves harder adversarial review than anything built so far.
