# Ply

Ply is a general-purpose, statically typed programming language with effects in
every signature. Definitions are content-addressed, and the compiler is written
in Ply and compiles to C.

Perfect incrementality is the goal the rest is shaped around: a change should
cost work in proportion to what it reached, and nothing else. Content addressing
is the mechanism, so the compiler and the test runner can tell what a change
reached rather than guess from a file's bytes or its timestamp. Where a change
still redoes more than it touched, that is a defect to fix, not a cost to live
with.

[`docs/book/`](docs/book/introduction.md) is the guide: it teaches the language in
the order you need it, with programs that grow and run.
[`docs/GUIDE.md`](docs/GUIDE.md) is the language reference: every rule, and a
diagnostic code for each — syntax, types, effects, tests, the standard library,
the `ply` command and the codes.
[`docs/DIRECTION.md`](docs/DIRECTION.md) is what the language is for and what that asks
of it next.

## Layout

| path | holds |
| --- | --- |
| `crates/ply-eval` | values, the evaluator, the scheduler and the simulator; spans, diagnostics and their codes; the program record the compiler answers with |
| `crates/ply-codegen` | the C backend: emits C, builds it and loads it |
| `crates/ply-compiler` | the compiler written in Ply (`ply/`), the builtins it declares (`prelude.ply`) and its builder, committed as a runnable (`bootstrap/`) |
| `crates/ply-store` | what `ply` keeps under `.ply-cache`, as the `store` package in `ply/`; not a cargo crate |
| `crates/ply-test` | what a test run decides, as the `suite` package in `ply/`; not a cargo crate |
| `crates/ply-prove` | specification obligations and their discharge, as the `prove` package in `ply/`; not a cargo crate |
| `crates/ply-sim` | the interleaving search, as the `sim` package in `ply/`; not a cargo crate |
| `crates/ply-host` | the Rust handlers effects resolve to (db, fs, tcp, tls, ...) |
| `crates/ply-machine` | the nested-entry capability: a program loading and entering another program; and the runtime `ply test` and `ply prove` drive |
| `crates/ply-std` | the standard library, as Ply source in `ply/` |
| `crates/ply-cli` | the `ply` program, as Ply source in `ply/` and the runnable it is built into (`bootstrap/`); not a cargo crate |
| `crates/ply-registry` | the package registry `ply publish` and `ply resolve` talk to, as Ply source in `ply/`; not a cargo crate |
| `crates/ply-launcher` | the `ply` binary: enters the program the artifact holds |
| `crates/ply-corpus` | the benchmark corpus as a Ply program (`ply/`), the checks that run it end to end (`checks/`) and the programs it measures (`fixtures/`); not a cargo crate |
| `crates/<crate>-tests` | that crate's tests |
| `editors/` | the tree-sitter grammar editors read |
| `examples/`, `tests/lang/`, `tests/fixtures/` | Ply programs the suite runs |
| `benches/` | benchmark scripts and their recorded output |
| `probes/` | standalone C probes, each run by a CI job |
| `.github/` | CI: `workflows/ci.yml`, the job tables in `ci-shards.sh`, and `ci-timings.sh` to read a run |

## Building and testing

CI is the verifier; every pull request runs these checks and the whole suite:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked --release -p ply-launcher --bins
cargo pack target/release/ply
cargo nextest run --workspace
```

`ply` is a Rust runtime with the shipped modules, the builder and the `ply` program
appended to it: `cargo pack BINARY` appends the checkout's, then runs BINARY to answer the
shipped modules and appends those answers too, so an edit to Ply sources needs only that
step, and a binary that carries no pack refuses to start.

CI builds one `cargo nextest archive --locked --workspace` and runs it in shards
cut from the durations its last run measured.
The corpus's tests are the Ply package `crates/ply-corpus/checks`, run by `.github/ci-corpus.sh`:
each `lanes` job runs a partition of them, apart from the `nextest` jobs, and the `corpus` jobs `desks-<k>`
run the tests of `serving` and `database`, cut by duration, against a postgres. The proof runs, `proofs` for the
standard library and `laws-<id>` for a package, each take a `corpus` job of their own.
`.github/ci-corpus.sh run <id>` runs one with the grants CI passes.
The postgres tests in `ply-host-tests` skip unless `PLY_PG_URL` and `PLY_TEST_DB`
name a server.

## The compiler bootstrap

The compiler is Ply source under `crates/ply-compiler/ply`. Its builder, the
compiler's own `build.main`, is committed as `crates/ply-compiler/bootstrap/build.run`
beside `build.digest`: a runnable, the front end's answer and the unit's C, which the
launcher enters without running a compiler. A binary whose `ply`
(`crates/ply-cli/bootstrap/ply.run`) is behind its sources has the committed
builder build it once and keeps it under the stages, so every such build reads
back what the builds before it kept. Where the program needs a rule the
committed builder lacks, that builder builds the sources' own builder, and it
builds `ply`. So the compiler and the shipped modules it imports cannot use a
language rule the same change introduces; the rest of the standard library can. Likewise the
runtime reads the front end's answer the committed builder carries, so it can come
to require a field of one only once main's builder writes it. Never commit the
runnables: CI rebuilds them on main after each merge.
