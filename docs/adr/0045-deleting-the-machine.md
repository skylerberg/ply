# ADR 0045 — Deleting the machine

**Accepted, and the second and third stages are built.** ADR 0042 placed this fifth, after the effects and the runtime,
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

**Built, second stage:** the bundle in `crates/ply-compiler/bootstrap/` and the
fixpoint test, `crates/ply-codegen-tests/tests/bootstrap.rs`. The CLI builds
the emitter from the bundle and no longer needs the reference to do it. On
the way the emitter's own sources joined the lowering differential and the
resolved-C ratchet, and the audit of the emitter's own tests under the tier
found the miscompilation the bootstrap first hit: two bugs in the port's
update recognition, both in the lowering, which are gone. The second stage
was taken before the first, since it depended on nothing the facade adds and
was the riskier of the two.

**Built, third stage:** a credential is a bridged value. The refusal is gone
from both emitters; the tier's memo, the one place a word outlives its
entry, refuses to keep a value that holds a handle, a secret among them, and
the examples' census is whole. With it, a body the port's lowering does not
reach still reports what it handles, read off its source, since a refused
handler whose performers compiled would have sent a `perform` past a frame
the machine held.

## What the fixpoint measured, and the work it names

The compiled tier releases nothing inside an entry: the reference emitter's
own header calls its ownership deliberately conservative, a value passed to
a helper is duplicated first and never released, and the entry's arena is
recycled whole when the entry ends. A test is an entry and a request is an
entry, so nothing shipped had noticed. The emitter emitting a program is one
entry too, and the measurement is what the bootstrap forced:

```sh
PLY_C_PHASES=1 PLY_C_CACHE=$(mktemp -d) \
  /usr/bin/time -l ./target/release/ply test crates/ply-std/ply --backend c --no-cache 2>&1 \
  | grep -E 'maximum resident|^entry: [0-9]{6,}|live at end'
```

On 2026-09-08 the producer's one entry over the standard library allocated
twenty-five million objects, recycled four million, reserved five gigabytes
of chunks and ended with twenty million still counted, three gigabytes of
them byte strings, where its answer is one string of four megabytes; over
the emitter's own sources the run peaked at seventeen gigabytes, which is
what a CI runner cannot hold, and each of six small programs run with the
tier as the only engine leaked one object per iteration: a temporary passed
to a builtin, an old record, a matched constructor, a parameter the callee
never released. Making the emitter's two quadratic concatenations linear
moved none of it. This is not the emitter's bug and not the port's: the
machine moves a binding out of its slot at its last use and truncates the
window at an activation's end, and the tier's emitted C does neither.

So the fifth stage has a prerequisite this record did not list: **the tier
releases within an entry**, transcribing the machine's rule into the
emitter written in Ply, whose lowering already carries the last-use marks
the machine acts on and whose lowering differential says they are the
machine's. A served program's entry would leak the same way for as long as
it served, which is the other reason this comes before the deletion and not
after it. ADR 0046 built it, and the bootstrap fixpoint, the port's ratchet
over its own sources and the emitter's own tests tier-only run in CI again.

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
reference build: `crates/ply-compiler/bootstrap/*.c`, produced by the command
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
PLY_C_CACHE=$(mktemp -d) PLY_C_KEEP=1 \
  ./target/release/ply test crates/ply-std/ply --backend c --no-cache >/dev/null 2>&1
ls -la "$PLY_C_CACHE"/*.c            # the larger unit is the emitter's, with the library it uses
zstd -19 -c "$PLY_C_CACHE"/<emitter>.c | wc -c
```

On 2026-09-08 the emitter's unit was twelve megabytes of C and under two
megabytes compressed with `gzip -9`, which the runtime decompresses without
a tool beside it. It is checked in compressed and rewritten when the fixpoint
says so: the emitter built from it emits the emitter's current sources, and
the emitter built from that emission emits them again, and the two must
agree; a change that alters what the emitter emits for its own sources, a
construct newly carried among them, moves the bundle, and a change that does
not, however large, leaves it. `PLY_C_BOOTSTRAP_REFRESH=1` on the fixpoint
test writes the new bundle, and only once a third emitter built from it has
emitted the same thing.

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

## The switch, which is the fifth stage's first form

Building the second and third stages showed the facade can come after the
engine: `PLY_TIER_ONLY=1` makes a machine with a backend attached refuse to
evaluate anything itself. A test root the tier answers is the pass; one it
refuses, declines or raises in is the failure, with the tier's diagnostic;
an entry point goes through the backend whole. The three corpora and the
served example run green under it, which is stage five's oracle taken
before stage five, and CI runs them so. What the switch does not cover is
what the facade is for: a bare expression in a scope, the corpus's
instruments and the prover, which construct machines of their own. The
first stage's facade is therefore taken consumer by consumer, each switched
to the tier-only engine with its own oracle, rather than as one wrapping
pass before any of them.

**Built, the runtime's digest.** A bundle's C calls the runtime's helpers
by name and shape, and a helper that moves — an argument added — leaves the
bundle calling the old shape, which no digest of the emitter's sources
sees: the emitter it builds fails on the first body that reaches the
helper, and the refresh, which builds its first emitter from the bundle,
fails with it. `RUNTIME.digest` beside the bundle is the helper table's
digest; a bundle emitted against another table does not serve, the
reference builds the producer, and the refresh starts from the reference.
The same digest is in the key of every cached body, since a body's C calls
the helpers by shape too: the differential read back bodies emitted a run
earlier, against the old shape, before it was.

**Built, for the judge and the prover.** §"The facade" said an expression in
a scope is a definition, and it is: `Source` synthesises one per law guard
and body and per `requires` and `ensures` clause, named `law#N.guard`,
`law#N.body`, `f#requires#K` and `f#ensures#K` under the module, its
parameters the binders or the owner's parameters and then `result`, its
answer `Bool`; the emitter written in Ply emits them as it emits a test
root, and the reference emitter refuses them by name. `ply prove` and
`ply review` take `--backend`, and `Cases` and the concurrency search enter
the roots with each case's values, falling back to the machine only where
the unit does not hold one. Oracle: `ply prove` over both corpora reports the
same with and without the backend, and CI compares the two.

**Built, the fourth stage's first sort.** `tests/lang/` exists and CI runs it
three ways — the machine, paired with the tier, tier-only — and nine of the
suite's files are gone: the bit surface, the byte, list and map builtins,
the number types, tuples, keyword fields, `min` and `max` and a lambda's
return, their embedded programs now the corpus's files. What a Ply test
cannot state, that a program raises with a given text, is a fixture under
`tests/fixtures/lang/` whose header names the failing tests and the text,
driven from the CLI suite on both engines; two checker claims about
footprints go the same way through `ply check --json`. The sort earned its
place at once: run tier-only, the corpus found the least `Int` literal
emitted as a constant C cannot write, `bytes_position` with no compiled
shape, a `U64` literal above the signed range refused, and four
diagnostics the tier worded differently from the machine, none of which
the machine-only suite could have shown.

## Staging, and the oracle at each stage

1. **The facade with the machine inside it.** `Engine` wraps a `Machine`;
   every consumer in the table is switched to the facade. Oracle: the whole
   workspace suite and the CLI's, unchanged, since nothing runs differently.
2. **The snapshot and the fixpoint.** Built. The bootstrap bundle is checked
   in, the fixpoint test runs in CI, and the producer is built from the
   bundle rather than the reference. Oracle, met: the fixpoint, and the audit
   over both corpora unchanged.
3. **The credential.** Built. The bridged secret, the memo refusing a handle,
   the refusal removed on both emitters. Oracle, met in part: the census over
   the examples has no rows; the secrets suite as a Ply test is the fifth
   stage's sort.
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
