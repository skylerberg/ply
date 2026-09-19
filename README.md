# Ply

Ply is a general-purpose, statically typed programming language with effects in
every signature. Definitions are content-addressed, so the compiler and the test
runner redo only what a change affects. The compiler is written in Ply and
compiles to C.

[`docs/GUIDE.md`](docs/GUIDE.md) is the language manual: syntax, types, effects,
tests, the standard library, the `ply` command and the diagnostic codes.
[`docs/DIRECTION.md`](docs/DIRECTION.md) is what the language is for and what that asks
of it next.

## Layout

| path | holds |
| --- | --- |
| `crates/ply-span` | spans, diagnostics and the registry of diagnostic codes |
| `crates/ply-ty` | the type vocabulary the checker produces and everything else reads |
| `crates/ply-eval` | values, the evaluator, the scheduler and the simulator |
| `crates/ply-codegen` | the compiled tier: emits C, builds it and loads it |
| `crates/ply-compiler` | the compiler written in Ply (`ply/`) and its bootstrap bundle (`bootstrap/`) |
| `crates/ply-store` | the result and front-end caches under `.ply-cache` |
| `crates/ply-test` | test selection, scheduling and running |
| `crates/ply-prove` | specification obligations and their discharge |
| `crates/ply-host` | the Rust handlers effects resolve to (db, fs, tcp, tls, ...) |
| `crates/ply-std` | the standard library, as Ply source in `ply/` |
| `crates/ply-cli` | the `ply` binary |
| `crates/ply-corpus` | synthetic projects and the benchmark harnesses |
| `crates/<crate>-tests` | that crate's tests |
| `examples/`, `tests/lang/`, `tests/fixtures/` | Ply programs the suite runs |
| `benches/` | benchmark scripts and their recorded output |
| `probes/` | standalone C probes, each run by a CI job |
| `.github/` | CI: `workflows/ci.yml` and the job tables in `ci-shards.sh` |

## Building and testing

CI is the verifier; every pull request runs these checks and the whole suite:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked --release -p ply-cli --bin ply
cargo nextest run --workspace
```

CI builds one `cargo nextest archive --locked --workspace` and runs it in slices.
Tests that need a runner of their own are `SOLO` in `.github/ci-shards.sh`; run one
with `cargo nextest run --workspace -E "$(.github/ci-shards.sh solo-filter <id>)"`.
The postgres tests in `ply-host-tests` skip unless `PLY_PG_URL` and `PLY_TEST_DB`
name a server.

## The compiler bootstrap

The compiler is Ply source under `crates/ply-compiler/ply`, compiled to C and
committed as `crates/ply-compiler/bootstrap/unit.c.gz` beside `SOURCES.digest`, a
digest of those sources and `crates/ply-std/ply`. A binary whose bundle is behind
its sources has the bundle's emitter emit them once, keeps that stage under the unit
cache, and runs the sources from then on. Editing either makes CI's `bootstrap` job
fail and upload the regenerated bundle as the `bootstrap-bundle` artifact; bring it
into the tree with
`gh run download <run-id> -n bootstrap-bundle -D crates/ply-compiler/bootstrap`.
