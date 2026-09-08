# ADR 0045 — Deleting the machine

**Proposed.** ADR 0042 placed this fifth, after the effects and the runtime,
and ADR 0044 built the runtime's four stages: stacks, `simulate`, `resume`
off the tail and more than once, and the host route with its production
region. With the chain entered whole the tier now takes every definition the
shipped corpora have except the one that makes a credential, and the audit
pairs every seed of every simulated test with the machine's run of it. This
record is what deleting the machine consists of, in the order it can be done
without a day on which nothing checks the language.

> **What this decides.** That the compiled tier becomes the *only* engine:
> every consumer of `Machine` takes an `Engine` facade over one compiled unit
> per program, built by the emitter written in Ply, and the CEK machine, its
> continuations, its slot stack, the lowering to its code, the seam and the
> differential oracle are deleted with the consumers switched. That the
> emitter written in Ply is bootstrapped from a checked-in snapshot of the C
> it emits for itself, so the reference emitter in Rust goes with the
> machine rather than after it. That the Rust suites over the machine are
> sorted, file by file: a statement about the language becomes a Ply test in
> a corpus `ply test` runs, and an audit of the machine's own mechanics is
> deleted with the mechanism. That a credential is a value the runtime
> bridges like every other value it does not lay out natively, so the last
> refusal goes, with the secrets suite as its test. That the corpus's
> instruments measure the tier from then on, and their thresholds are taken
> again before the deletion rather than carried across it.
>
> **What it does not decide.** The sixth step: the Rust front end stays,
> since the facade still loads a program through it and the language tests
> still need the checker's diagnostics. The per-definition object and link
> ADR 0037 describes, which is the loop's cost and not the machine's
> existence. Whether the scheduler and the search are later written in Ply.

## The inventory

The command, and what it names:

```sh
grep -rl '\bMachine\b' crates --include='*.rs' | sed -E 's#crates/([^/]+)/.*#\1#' | sort | uniq -c
```

| consumer | what it asks of the machine | what the facade gives it |
| --- | --- | --- |
| `crates/ply-test`, the harness | an engine per worker and per seed: enter a test, seed it, read its record and host use, compare two engines under the audit | one engine kind; the audit's second engine and the pairing go |
| `crates/ply-cli` `engine.rs`, `run.rs`, `artifact.rs` | `main` under a host binding and reactor; a spec's guard and body evaluated in a scope of bound names | `call` under the binding; an expression compiled as a definition over its scope's names, once per expression |
| `crates/ply-cli` `config.rs` | a nullary definition evaluated for a configuration schema | `call` |
| `crates/ply-corpus` | the request path timed and its allocations attributed, regions counted, the W3 and W6 ladders | the same shape over the tier, with numbers of its own |
| `crates/ply-prove` | a concurrency proof's bodies run, a property's expression evaluated | `call` and the compiled expression |
| `crates/ply-codegen` | the `reference` backend, a nested machine over a fragment; the seam's `Compiled` trait; the audit's `Mutant` | nothing: all three are the machine's |
| the suites under `crates/*-tests` | the machine as the oracle of a language claim, or as the subject of an audit | a Ply test, or nothing |

What `crates/ply-eval` keeps is the runtime the machine and the tier already
share, as ADR 0044's table says: the value model and the regions, the
builtins, the scheduler and the search, the host boundary, the escape checks
and the trace. What it loses is `machine.rs`, `cont.rs`, `window.rs`,
`frame.rs`, `code.rs`, `slots.rs`, `compiled.rs`, `backend.rs`,
`differential.rs`, `costs.rs`, `memo.rs`, `pool.rs`, the machine's tests and
the frame bound of `limit.rs`. The count of lines is git's to report.

## The facade

`Engine` is built from what `Machine::new` is built from — the program, its
resolution and its check — and holds one compiled unit for the program, built
by the C backend with the emitter written in Ply as its only producer and
cached where a backed run caches it now. Its surface is the surface the table
above uses and no more: `call`, `eval_test_in`, the host binding, reactor,
declared footprint and re-execution flag, the seed and the record, the host
use and the linear count, the teardown warnings, the test count and names.
Everything the harness did through `Machine` it does through `Engine`, and
the harness's `Engines::Audited` pair becomes one engine, because there is
no second thing to compare against.

**An expression in a scope is a definition.** The spec engine and the prover
evaluate a written expression against bound names. The facade compiles such
an expression as a synthetic definition whose parameters are the scope's
names, added to the program before the unit is built, once per distinct
expression; the scope's values are the call's arguments. The front end is
still Rust at this step, so the synthesis is an AST item, and the checker
types it as it types a written one.

**A credential is a bridged value.** The reference emitter refused
`secret_of_string` so that no credential would sit in the compiled arena;
the arena is an entry's and is reset when the entry ends, a secret is
carried behind a `Bridge` object like a float or a closure the model does not
lay out, and the constant pool already refuses to retain one. The refusal
goes; `crates/ply-eval-tests/tests/suite/secrets.rs`, which pins what a
credential renders as and what comparing two does, becomes the Ply test that
says the tier keeps the same promise.

## Bootstrapping the emitter

The producer's unit is built by the reference emitter today, never by
itself. With the reference gone, the emitter written in Ply is built from a
checked-in snapshot of the C it emitted for its own sources under the last
reference build: `spikes/ply-parser/bootstrap/*.c`, produced by the command
that emits a unit and kept with the digest of the sources it was emitted
from. A change to the emitter's sources rebuilds the emitter with the
snapshot's binary, then re-emits the snapshot with the new binary, and a
test asserts the second emission is a fixpoint of the first: the emitter
built from what it emits emits the same thing. That is the check every
self-hosting compiler runs, and it is the check that lets the reference go.
The snapshot is not a second emitter; it is the emitter's last output, and
the fixpoint is what says so.

The snapshot's size was measured before this was accepted, since a large
file checked in and rewritten often is a cost the tree would carry forever:

```sh
PLY_C_CACHE=$(mktemp -d) PLY_C_KEEP=1 PLY_C_EMITTER=ply-whole:spikes/ply-parser \
  ./target/release/ply test crates/ply-std/ply --backend c --no-cache >/dev/null 2>&1
ls -la "$PLY_C_CACHE"/*.c            # the larger unit is the emitter's, with the library it uses
zstd -19 -c "$PLY_C_CACHE"/<emitter>.c | wc -c
```

On 2026-09-08 the emitter's unit was twelve megabytes of C and under a
megabyte compressed with `zstd -19`. It is checked in compressed, and it is
rewritten only when the snapshot can no longer build the emitter's current
sources, which a change to the language the emitter is written in causes
and a change to what it emits does not; the fixpoint test is what says the
snapshot still serves, and a lagging snapshot that serves is not updated.

## The suites

`crates/ply-eval-tests/tests/suite` holds both kinds of file, and the sort
is by what a file's first line claims. A claim about the language — the bit
surface, the byte and list and map builtins, number types, tuples, keyword
fields, min and max, a lambda's return, the seeded handlers against their
signatures, what a resumption sees, secrets, the transaction's two hazards,
determinism of a simulated run — is written again as a Ply test in
`tests/lang/`, which `ply test tests/lang` runs and CI requires. A claim
about the machine — slot resolution, the slot rewrite, position invariance,
the argument-vector pool, the constant memo, the region arena's wiring and
reclamation census, the reference-counting pass's numbers, the ownership
checker's counters, the snapshot audit's frame counts — is deleted with the
mechanism it audits, and the runtime's own tests in `crates/ply-codegen`
are what stand where a mechanism claim is still needed. A file that mixes
the two is split before it is sorted. The commit that moves or deletes each
file is the record of its sort, not a table here.

This is where ADR 0042's loop begins: a language test written in Ply is
content-addressed like every other, and re-runs only when an edit reaches
it.

## The corpus's instruments

`ply-corpus` measures the request path and attributes its allocations to
the machine's own sites; ADR 0019's thresholds are fractions of those, and
`README.md` carries one guarded sentence taken with `w6-alloc`. The tier
allocates differently and in different places, so none of those figures
carries across. Before the deletion, every instrument is run once more with
the machine and once with the tier on the same tree, and the tier's figures
become the new baselines, with the command beside each; after it, the
instruments run over the facade. A threshold is re-registered from a
measurement, not adjusted to pass.

## Staging, and the oracle at each stage

1. **The facade with the machine inside it.** `Engine` wraps a `Machine`;
   every consumer in the table is switched to the facade. Oracle: the whole
   workspace suite and the CLI's, unchanged, since nothing runs differently.
2. **The snapshot and the fixpoint.** The bootstrap snapshot is checked in,
   the fixpoint test runs in CI, and the producer is built from the
   snapshot rather than the reference. Oracle: the fixpoint, and the audit
   over both corpora unchanged.
3. **The credential.** The bridged secret; the refusal removed on both
   emitters. Oracle: the secrets suite as a Ply test, and the census over
   the examples losing its last row.
4. **The instruments' baselines.** Each instrument run under both engines
   on one tree, the tier's figures registered. Oracle: the registered
   figures reproduce.
5. **The facade over the tier alone.** `Engine` holds a unit and no machine;
   `eval_test_in`, `call` and the compiled expression run compiled;
   `--audit-backend`, `--backend` and `PLY_C_EMITTER` are retired and §17 of
   the guide moves with them; the suites are sorted. Oracle: `tests/lang`
   green, the served examples under `--host` through the CLI suite, the
   examples and the standard library green with nothing to fall back to.
6. **The deletion.** The modules in the table, the reference emitter, the
   seam, the mutant, the spike's differentials against the reference, and
   the CI jobs that ran them. Oracle: the workspace builds, and CI's wall
   clock is measured against the run before stage one by the command in
   `benches/`.

Stages one to four each leave the machine in place and every check running.
Stage five is the first run with one engine, and it is one commit after the
last audit that compared two.

## What would make this wrong

- **If the spec engine's expressions are many and distinct.** A unit rebuild
  per expression is a C compile each; if a spec evaluates thousands of
  distinct guards, the compiled expression is priced out and the answer is
  an expression evaluator over the runtime, which is a small interpreter of
  expressions and not a machine. Count them before stage one.
- **If an instrument's claim was about the machine.** A threshold that says
  where the machine's allocations were is not carried; an instrument whose
  only subject was the machine is deleted as an audit is.
- **If the snapshot drifts from the reference's last emission.** The
  fixpoint test is what says so; a snapshot that does not reproduce itself
  is not used, and the reference is rebuilt from git to take a fresh one.
- **If a Ply test cannot say what a Rust one said.** Some audits read the
  machine's counters; a language test cannot. Those are mechanism claims by
  this record's rule, and go.
