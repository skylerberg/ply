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
> every profile, at once. That the compiled
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
the unit can. The deferred table was mostly timing tests over allocation and
resumption, which flaked whatever the jobs did, because a duration is not a
claim about the code. Tuning the jobs again would shuffle the same seconds;
ADR 0042's programme says to change the thing the seconds come from.

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

### 2. The heap reuses in every profile — done

`crates/ply-codegen/src/heap.rs` reused dead blocks in a release build and
not in a debug one, so that a read of a dead object in a debug build found
the marker. The consequence: a debug `ply`, or a debug test binary, entering
the compiler over a large input allocated without bound and was killed by
the runner. A run's worth of CI jobs died that way before it was understood,
and the workaround was `heap::reuse_by_default(true)` at the top of every
test that enters the bundle, which is a thing a contributor had to know.

**Built.** Every build reuses a dead block at once, as a release build
always did. The switch and every call to it are gone, and no test has to
know anything. This item first landed as a quarantine, a debug build
holding dead blocks back for a while so a stale read still found the
marker, and ADR 0050 §1b's instruments took it out again: threaded through
the dead blocks and bounded by tens of megabytes it put the heap's
`dismantle` at a quarter of the compiled compiler's time, and in both that
form and a ring of a thousand slots kept outside the blocks, the memory of
the emitter over its own sources grew far past what immediate reuse holds,
for a reason the readings did not find. What immediate reuse gives up is a
stale read finding a dead header after the block is taken; a stale read is
caught by the counts the audits assert and by the header a block keeps
until then.

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

### 4. The corpus is the specification — inventoried, and sequenced after the reference retires

The differentials compare the ported compiler against the Rust reference,
module by module. They are scaffolding: ADR 0042 orders the reference's
retirement, and a differential with no reference is nothing. What already
stands without a reference is the bootstrap fixpoint, the compiler's own
`test` blocks in `crates/ply-compiler/ply`, the language corpus under
`tests/lang` on both tiers, the propositions `ply prove --backend c` judges,
and the wrong-backend, hazard and producer suites.

**What the inventory found, 2026-09-15.** The record first said each
differential should be replaced by corpus programs with expected outcomes
and then deleted. Two things make that wrong today. First, the Rust front
end is still the one every `ply` command parses and checks with; while both
front ends ship, the differential is the only statement that they are one
language, which is the programme's fourth goal, so it retires with the
reference and not before. Second, a corpus program can say what an
expression evaluates to and what a test raises; it cannot say what a tree,
a span, a scheme or a hash is, and those are what the differentials assert:
token spans over every real file, tree and diagnostic equality over some
eight hundred inputs with error recovery, load order and cycle rotation
over the real module graph, every scheme and footprint over the standard
library and the restore-from-published-interfaces round trip, generated
source byte for byte, every published hash and the reference graph, and
slot numbers with ownership marks over the lowered bodies. The compiler's
own modules already show the form those take without a reference: a `dump`
of the phase asserted against a literal. The mined `.corpus` bundles are
the inputs; what they lack is the expected dump beside each one.

**What lands now, and what waits.** The four arming scripts for the parser
modules copied the sources from a directory they had moved out of and could
not run; they read `crates/ply-compiler/ply` again. The package's harness,
which enters the compiled compiler in-process, is a runner and not a second
implementation, and it stays. When ADR 0042's retirement of the Rust front
end is reached, each bundle takes the reference's dump beside every input
before the reference goes, the harness compares the port against the file
rather than against Rust, the whole-file comparisons over `examples/` and
the standard library become the same with their dumps under `tests/`, and
the arming scripts are re-pointed at those. A dump the reference never
produced is stated by hand, as the compiler's own tests are. Metamorphic
checks belong in the same frame and need no reference at all: a printer
round trip over the corpus, or an optimisation pass held to the corpus's
outcomes, is the first to add once a Ply printer exists.

### 5. Figures live where tests read them — done

`README.md`'s request-path allocation figure was read by a test with a tight
band. It drifted during this record's first two items and was retyped by
hand. The test is right to exist; the hand step is not.

**Built.** `w6-alloc --out benches/w6-alloc.json` writes the figure as a
file and rewrites the README's sentence from it in the same command, through
`ply_corpus::w6_run::Allocation`, the one rendering both share. Two tests
replace the one: the file is held to a freshly counted window within one per
cent, and the README is held to carry exactly the sentence the file renders.
A number in that sentence is never typed again; `CONTRIBUTING.md` §"Before
you open a change" names the command.

## The suite

### 6. `examples/same-tests.sh` becomes tests — done

The `same-tests` job was the longest downstream job and it was serial: the
script plus the named steps around it in `ci.yml`, each a shell assertion
over the release `ply`. A step cannot be partitioned, retried alone,
filtered by name or run under nextest's timeout.

**Built.** The steps are `ply-cli-tests`' `corpus` module, eight tests over
the `ply` the suite already drives: examples on the code generator, the
whole emitter over examples, the compiler's own tests with the tier as the
only engine, the language corpus on both tiers, examples and the standard
library tier-only, the propositions judged, the served example with the
tier holding its accept loop, and the long input hashed. The compiler's own
tests run alone, as a third `solo` entry. `examples/same-tests.sh` itself
needs a server, so it runs in `test-postgres` against that job's, and the
fresh-program cost instrument moved beside the binary it measures, in
`build-ply`. The job and `.github/served-with-tier.sh` are gone.

### 7. The deferred table empties — done

`DEFERRED` in `ci-shards.sh` held the fourteen tests that ran only in
`gates`, last and alone, because each passed or failed on how much CPU a
loaded runner gave it rather than on what the code does. Most were timing
claims about allocation, resumption and fixture cost, and a survey of them
found that every claim worth keeping already had a count beside it.

**Built.** The table is gone, with the `deferred` commands, the nextest
override that serialized it and the `verify` check that held the two
together; `gates` runs the shutdown suite and the tree checks. Seven of the
fourteen keep their claim as a count and lose their clock: a snapshot's cost
is `slots_copied` at every tenfold of region size, a fixture open is what it
allocates plus that nothing a test wrote reaches the fixture, a capture of a
hundred thousand frames moves one segment, the store's two opens assert that
an entry and a baseline still answer, the group ladder keeps its arithmetic
and drops the reading it was taken from, and the empty group region asserts
its mark. The simulated sleep keeps its program and takes its bound from a
`slow-timeout` on that test in `.config/nextest.toml`, which kills the process
rather than reading a clock in the test. Six were ratios of one wall clock to
another with no count behind them — the fixture-against-rebuild and
discard-cost tables, the resumption and rebuild ratios, the router's escapes
and the map rows' subtraction — and were deleted. The prose that leaned on
them now says what it is: ADR 0017's fixture cost, ROADMAP.md's per-test
figure and its `Store::open` row, and README.md's open time are measurements
the records took, and no test asserts them.

### 8. The build job's link time — measured, and rejected

With sccache warm the build job's time was thought to be links: one suite
binary per `-tests` package plus the two bins, so a faster linker on the
runner would move it.

**Measured.** `mold`, set through `RUSTFLAGS` in the build job alone, ran
the job's `cargo test --no-run` step in 56 s on a warm second run against
36 to 49 s on the docs-only pull-request runs in the same cache state, and
cost 12 s to install. Rejected: whatever the warm step spends, it is not the
link, and `--timings` is where the next reading of it starts.

### 9. Clippy off the critical path — done

The `clippy` job ran the workspace with all targets and then `cargo test
--doc`. Every crate has zero doc tests, so that second step was a second
build of the workspace to run nothing, two minutes that kept the job above
the build job it was meant to stay below.

**Built.** `cargo test --doc` runs in the build job beside the libraries it
just built, where it costs the rustdoc pass alone and still runs a doc test
the day one appears. The clippy job is the clippy step and its cache.

### 10. The floor — the timeline, and a decision this record leaves open

Runner provisioning is a fixed cost per job, and below it only larger or
self-hosted runners help. That is a spend decision and is taken last, with
the run's timeline in hand.

**The timeline, with items 1 to 9 landed.** A quiet run on `main` is the
build job, then every downstream job starting within seconds of it, then
the slowest of those. The slowest is the emitter over its own sources, the
`solo` that enters the compiled compiler once more, with the bootstrap
fixpoint and the postgres job close behind it; the partitions finish well
inside it. So the pole is not provisioning and not the suite's shape any
more: it is the compiled compiler's own running time, which item 3's
profile names and its levers would move. A larger runner would shorten the
build job and each solo in proportion to its cores only where the work is
parallel, which the build is and a solo test is not. The decision to pay
for one is the owner's, and this record does not make it.

## The goal, as set

> Complete ADR 0049 (`docs/adr/0049-after-ci-in-minutes.md`) in its order:
> the five language and compiler items first, then the five suite items.
> Each item is one or more PRs; merge as you go, only after CI on the head
> SHA is green. Hold each item to the measure the record names, and correct
> the record in place if the measure says otherwise. Do not run heavy loads
> of tests locally; single-threaded checks like clippy and targeted test runs
> are fine. Keep `docs/GUIDE.md` and the ADR current with what lands.
