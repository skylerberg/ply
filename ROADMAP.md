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

Pre/post conditions and algebraic laws attached to definitions. The system
discharges each obligation at the strongest tier it can: static proof where
possible, property test where not, concrete example at the edges. The spec
becomes the reviewable artifact.

## M9 — Native codegen

Cranelift or LLVM backend. Deferred deliberately — the interpreter is not the
bottleneck the language exists to solve, and a fast interpreter with a perfect
cache beats a fast compiler with a cold one.
