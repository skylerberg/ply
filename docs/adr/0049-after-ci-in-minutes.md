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
> every profile, with no-reuse an explicit diagnostic. That the compiled
> compiler's time is held to a profile taken on a runner, and no emitter
> change lands without one. That the corpus, judged by the compiled tier, is the specification
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

Each suite item below is bounded by a compiler item. The longest single tests
left are the differentials over the compiler's own sources, and their time is
the compiled compiler's own running time, so how the emitter represents and
reaches values is the suite's tail. The
bundle's load record exists because a unit cannot say what it exports, and
every consumer that wants to skip the front end will need the same trick until
the unit can. The deferred table is mostly timing tests over allocation and
resumption, which will stay flaky until what they measure is cheap. Tuning the
jobs again would shuffle the same seconds; ADR 0042's programme says to change
the thing the seconds come from.

## The language and compiler

### 1. A compiled unit describes its own exports — done

The bootstrap bundle used to carry three side files beside the unit's C: the
record the cache kept of its tables, the constructor table it was emitted
against, and a load record of arities, pure nullary constants and the module
count, so that the bundle could be entered without front-ending the
compiler's sources. Only the bundle had them; an artifact carried its own
copies in its own format, the whole-unit cache a third, and any other
compiled unit had to be re-parsed to be linked against.

**Built.** The unit's C ends with `ply_exports`, a string carrying all of it:
the constructor table, every function with its arity, the constants, the
module count, the refusals and the five tables. `ply_codegen::c::Exports`
encodes it at emission, the one place a source is read, and reads it back
through `dlsym` once the object loads. Every door goes through that read,
the fresh build included, so a table that will not read back fails every
build rather than only a warm one. The bundle is the C and two digests, an
artifact embeds the C alone, the whole-unit cache keeps an object key, and
the fixpoint compares the C, table included. A unit can now be linked
against with none of its sources present, which is the first piece of
separate compilation.

### 2. The heap reuses in every profile — done, as a bounded quarantine

`crates/ply-codegen/src/heap.rs` reused dead blocks in a release build and
not in a debug one, so that a read of a dead object in a debug build found
the marker. The consequence: a debug `ply`, or a debug test binary, entering
the compiler over a large input allocated without bound and was killed by
the runner. A run's worth of CI jobs died that way before it was understood,
and the workaround was `heap::reuse_by_default(true)` at the top of every
test that enters the bundle, which is a thing a contributor had to know.

**Built.** Every build reuses. A debug build holds a dead block in a
quarantine first, oldest out, bounded by `heap::QUARANTINE` bytes, so a
stale read still finds the marker for a while and an entry's memory is what
it holds plus the bound, never what it ever held. The record first said
reuse everywhere with no-reuse as an explicit mode; the quarantine keeps the
net the suites run under without a switch, so no test has to know it is
there. The switch and every call to it are gone; `Heap::set_quarantine(0)`
is what a test of the recycling itself asks for.

### 3. The emitter's inner loops, held to the hasher — measured, and the premise was wrong

The record first said the hash differential was the slowest phase, its time
inside the compiled hasher's byte loops, and that the emitter should turn
those into local C loops. `.github/workflows/profile.yml`, the instrument
this item built, says otherwise. A flat `perf` profile of the hash
differential under tcc, taken 2026-09-15 on a hosted runner, has no byte
loop near the top: the heaviest symbols are the runtime's `dismantle` and
`raw_alloc`, then the hasher's own `at` and `set_at` over lists, then
`rt_field`, `call_value`, `list::get` and the count traffic, none above six
per cent, and the rest is a long tail of the compiled bodies. The same
shape holds for the emitter over its own sources and for the checker. The
compiled compiler is not slow in one place; it is slow in the way its
values are represented and reached.

Two levers were then measured against the same test, at 19.9 s under tcc.
Under `cc -O0` it ran in 18.1 s and under `cc -O1` in 9.6 s, so what a
real optimiser does to the bodies is worth a factor of two, but `-O1`
compiles the compiler's unit in seventy seconds where tcc takes one, which
is not a trade the development loop can make. Rewriting the prelude's
`static inline` helpers as macros, on the guess that tcc's calls to them
were the tail, ran in 19.4 s: rejected, the calls are not where the time is.

**What is left, and named for the next record.** The hasher's `set_at`,
written as a `map` over `range` with a closure, is quadratic in the list
and is used forty-four times across the compiler's sources; a `list_set`
builtin over the trie is the one algorithmic lever the profile names, and
it needs a runtime helper, which today re-keys the bundle out of service.
Appending a helper should not: the unit's `exports` can carry the helper
table it was emitted against, and a unit serves while the runtime's table
starts with it. Below that lies the value model, ADR 0035's, where the
counts and the field reads through the runtime come from. The
allocation-attribution suites in `ply-corpus` remain the regression guard.

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
