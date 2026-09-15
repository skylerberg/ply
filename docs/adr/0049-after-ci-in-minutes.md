# ADR 0049 — After CI in minutes: what the build-once suite exposed, and the order it is fixed in

**Accepted as an ordering.** The suite is built once and run as short jobs
(`.github/workflows/ci.yml`, tables in `.github/ci-shards.sh`), the
differentials enter the compiled compiler in-process, every library's unit
tests live in a `<crate>-tests` package at opt-level 0, and the bootstrap
bundle loads without parsing its own sources. That work met its target, a
quiet run under four minutes, and along the way it found five things wrong
with the language and compiler and five things still wrong with the suite.
This record lists them, says why each is a defect rather than a tuning, and
fixes the order: **the language and compiler first**, because each of those
is also what the remaining suite time is made of.

> **What this decides.** That a compiled Ply unit describes its own exports,
> so the bundle's load record goes away. That the heap reuses dead blocks in
> every profile, with no-reuse an explicit diagnostic. That the C emitter's
> code for byte handling and tight loops is a performance target held to the
> hasher. That the corpus, judged by the compiled tier, is the specification
> the differentials are retired into, in the order ADR 0042 fixed. That
> figures a test reads live in a file the test and the prose both read. And
> that `examples/same-tests.sh`'s steps, the deferred table, the build job's
> link time and clippy's place on the critical path are dealt with after
> those, in that order.
>
> **What it does not decide.** The module interface's shape beyond what
> loading needs; that is settled by the first separate-compilation change that
> needs more. Whether runners larger than GitHub's hosted ones are worth
> paying for; that question opens only once everything below is done and the
> floor is provisioning.

## Why the language first

Each suite item below is bounded by a compiler item. The longest single test
phase left is the hash differential, and its time is the ported hasher's own
running time, so the C emitter's inner loops are the suite's tail. The
bundle's load record exists because a unit cannot say what it exports, and
every consumer that wants to skip the front end will need the same trick until
the unit can. The deferred table is mostly timing tests over allocation and
resumption, which will stay flaky until what they measure is cheap. Tuning the
jobs again would shuffle the same seconds; ADR 0042's programme says to change
the thing the seconds come from.

## The language and compiler

### 1. A compiled unit describes its own exports

`crates/ply-compiler/bootstrap/unit.load` carries the arities, the pure
nullary constants and the module count the loader needs, so the bundle is
entered without front-ending the compiler's sources. It is a side file
produced by the emitter's driver, not by the unit, and only the bootstrap
bundle has one. Any other compiled unit still has to be re-parsed to be
linked against.

**Fix.** The emitted C unit carries an export table the loader reads directly:
the same facts, emitted by the emitter as part of the unit, with the record's
digest inside it. `bundle.rs` reads it from the unit; `unit.load` is deleted,
and the fixpoint test asserts the table rather than the record. This is the
first piece of separate compilation, and it is what lets a program's
dependencies be compiled units rather than sources.

### 2. The heap reuses in every profile

`crates/ply-codegen/src/heap.rs` reuses dead blocks in a release build and
not in a debug one, so that a read of a dead object in a debug build finds
the marker. The consequence: a debug `ply`, or a debug test binary, entering
the compiler over a large input allocates without bound and is killed by the
runner. A run's worth of CI jobs died that way before it was understood, and
the workaround is `heap::reuse_by_default(true)` at the top of every test that
enters the bundle, which is a thing a contributor has to know.

**Fix.** Reuse is the default in every profile. No-reuse becomes an explicit
mode, `PLY_HEAP_NO_REUSE` or a `Heap` constructor the use-after-free tests
call, and the `reuse_by_default` switch and every call to it are deleted.
The debug build's marker check is kept where it is armed: the tests that
exercise release itself.

### 3. The emitter's inner loops, held to the hasher

`ply_compiler_diff`'s `hash` module is the slowest phase of the differentials
and the time is inside the compiled hasher, not in the harness or the
reference. The corpus timing tests point the same way. Byte handling and
tight loops are where the C tier's code is weak: every byte read goes through
a call, every loop iteration allocates its state.

**Fix.** Profile the ported hasher's C under `ply --backend c` on a long
input, name the three most expensive shapes, and change the emitter so each
becomes a local C loop over a raw pointer. The measure is the hash
differential's own time and `probes/` gets a micro-probe for each shape,
so the change is held to an instrument rather than a feeling. The
allocation-attribution suites in `ply-corpus` are the regression guard.

### 4. The corpus is the specification

The differentials compare the ported compiler against the Rust reference,
module by module. They are scaffolding: ADR 0042 orders the reference's
retirement, and a differential with no reference is nothing. What already
stands without a reference is the bootstrap fixpoint and the corpus
specifications judged by the compiled tier.

**Fix.** Every behaviour a differential asserts today that the corpus does
not state gets a corpus program with an expected outcome. When a module's
behaviours are all stated, that module's differential is deleted; the
`ply-compiler-diff` package ends as the fixpoint test plus the arming
scripts. Metamorphic checks belong in the same frame: an optimisation pass
is held to the corpus's outcomes, not to a second implementation of it.

### 5. Figures live where tests read them

`README.md`'s request-path allocation figure is read by a test with a tight
band. It drifted during the CI work and was retaken by hand. The
test is right to exist; the hand step is not.

**Fix.** The figure goes into a `benches/*.json` the test reads, and the
README's sentence is generated from the same file by the tool that takes the
measurement. `CONTRIBUTING.md` §"Before you open a change" then names one
command rather than one sentence.

## The suite

### 6. `examples/same-tests.sh` becomes tests

The `same-tests` job is the longest downstream job and it is serial: the
script plus the named steps around it in `ci.yml`, each a shell assertion
over the release `ply`. A step cannot be partitioned, retried alone, filtered by name
or run under nextest's timeout. Each already has the shape of a test in
`ply-cli-tests` or `ply-corpus-tests`.

**Fix.** Each step becomes a named test over the built `ply`, the script
keeps only its database management, and the job is deleted; the tests
partition with everything else. The fresh-program cost instrument stays as
the one step that is a measurement rather than an assertion, or moves into
`probes/`.

### 7. The deferred table empties

`DEFERRED` in `ci-shards.sh` holds the tests that run only in `gates`, on
`main`, because they were slow or flaked under a loaded runner. A deferred
test is a test no PR reruns. Most of them are timing claims about
allocation, resumption and fixture cost.

**Fix.** Each entry is either rewritten to assert a count rather than a
clock, the way the watch test was rewritten to be event-driven, or deleted
with the claim it armed removed from the prose. The table ends empty and the
`gates` job keeps only the tree checks.

### 8. The build job's link time

With sccache warm the build job's time is links: one suite binary per
`-tests` package plus the two bins.

**Fix.** A faster linker on the runner, `mold` or `lld`, set through
`RUSTFLAGS` in the build job so a developer's link is untouched. Measured on the build job's step
time in `--timings`, kept only if it moves.

### 9. Clippy off the critical path

Once the build job is shorter than clippy, clippy is the run's pole. It runs
the workspace with all targets plus the doc tests in one job.

**Fix.** Doc tests move to their own job, and clippy is split by the same
`-tests` boundary the build uses.

### 10. The floor

Runner provisioning is a fixed cost per job, and below it only larger or
self-hosted runners help. That is a spend decision and is taken last, with
the run's timeline in hand.

## The goal, as set

> Complete ADR 0049 (`docs/adr/0049-after-ci-in-minutes.md`) in its order:
> the five language and compiler items first, then the five suite items.
> Each item is one or more PRs; merge as you go, only after CI on the head
> SHA is green. Hold each item to the measure the record names, and correct
> the record in place if the measure says otherwise. Do not run heavy loads
> of tests locally; single-threaded checks like clippy and targeted test runs
> are fine. Keep `docs/GUIDE.md` and the ADR current with what lands.
