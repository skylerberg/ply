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

## Reproducing a corpus

A corpus is a pure function of its spec and seed, both recorded in the
`corpus.json` the generator writes next to the sources. Re-running `gen` with
the same flags reproduces it byte for byte, which is why the generated trees are
gitignored.
