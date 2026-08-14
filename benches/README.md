# Benchmarks

`ply-corpus` generates a synthetic Ply project of a given size and reports where
a run's wall clock goes. The corpus is not a toy: it compiles, typechecks and
passes `ply test` before any number is taken, and generation fails loudly if it
does not.

```
benches/run.sh                       # the default size ladder, 250 → 10,000 definitions
benches/run.sh 40,25,500             # one size: modules,defs_per_module,tests
PLY_BENCH_REPEATS=3 benches/run.sh   # slower, less noise
```

Or drive the binary directly:

```
cargo run --release -p ply-corpus -- gen --out /tmp/c --modules 200 --defs-per-module 50 --tests 5000 --depth 6
cargo run --release -p ply-corpus -- bench /tmp/c --repeats 3 [--json]
cargo run --release -p ply-corpus -- sweep --out /tmp/sweep --sizes "40,25,500 80,25,1000"
cargo run --release -p ply-corpus -- measure /tmp/c --repeats 3 [--json]
```

## What is measured

Nine phases, timed separately: discover, read, parse, resolve, typecheck, hash,
cache open, select, execute. The split is the point — a total tells you nothing
about a system whose thesis is that most of the work should be skipped.

Five scenarios, each with the source tree and the cache restored afterwards so
the order they run in cannot leak into the numbers:

| scenario | state |
| --- | --- |
| `cold` | cache cleared before every repeat; every test runs |
| `warm` | nothing changed; every deterministic test is a cache hit |
| `rename` | a top-level definition renamed corpus-wide |
| `edit-leaf` | one definition's body changed, few dependents |
| `edit-hub` | a widely depended upon definition's body changed |

The `rename` and `edit-*` mutations are value-preserving by construction, so a
mutated corpus still passes and a scenario never measures failure formatting.

## What `measure` adds

`bench` reports a whole run and therefore hides which engine cost what: a
worker is built per rayon thread per concurrency group, so a setup cost
proportional to the program size is charged many times over and reads as
interpreter speed. `measure` separates the two and prices the claims ADR 0005
makes on their own:

| section | question |
| --- | --- |
| throughput | the two engines on one corpus, one worker, one thread — worker setup, a first pass, and a steady pass apart from each other |
| fork | `World::fork` against rebuilding the same fixture, at five world sizes |
| multi-shot | resuming zero, one, two and four times, plus `Stack::capture` and `Stack::resume` against pending-frame count |
| scheduling | world-isolated against shared tests, and the groups the shared ones alone need |
| `Store::open` | against the cache the corpus has already filled |

`--engine <treewalk\|machine>` restricts the throughput table to one engine and
`--only-throughput` drops the rest, which together is what to point a profiler
at. `crates/ply-corpus/tests/frame_cost.rs` counts the allocations a frame push
and pop cost, which is the machine-independent half of the same question.

## What `sim` adds

`measure` prices the machine ADR 0005 built; `sim` prices the search ADR 0006
built on top of it. It drives the real scheduler through the real machine, and
the one thing it does that `ply test --measure-reduction` cannot is choose the
root set per trial — which is what makes a median over many trials possible.

```
cargo run --release -p ply-corpus -- sim <corpus> [--trials N] [--budget N] [--json]
```

| section | question |
| --- | --- |
| exploration | interleavings pruned, the same search with the recording's clocks withheld, and an unpruned enumeration — three columns, one driver |
| race finding | interleavings to the first failure, `dpor` against sampling, median and worst over `--trials` roots, with misses counted rather than dropped |
| throughput | seeds per second, where one seed is one whole test replayed from a fresh world |

The middle column of the exploration table is the honest way to price the
happens-before filter without checking out the tree that predates it: an empty
stamp is documented to mean "no synchronization known", so a driver that clears
the stamps reproduces the older search exactly.

A corpus generated with `--concurrent-tests` at a chosen `--conflict-density` is
what the reduction is measured against; `--tasks-per-test` and `--steps-per-task`
are the two exponents the schedule count grows in.

## Reproducing a corpus

A corpus is a pure function of its spec and seed, both recorded in the
`corpus.json` the generator writes next to the sources. Re-running `gen` with
the same flags reproduces it byte for byte, which is why the generated trees are
gitignored.
