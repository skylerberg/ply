# Ply

Ply is a general-purpose, statically typed programming language. A function's type lists the
effects it can have, such as reading a file or writing to the network. Code is stored by the hash
of its content, so the compiler and the test runner know exactly what an edit changed and redo only
that work. The compiler is written in Ply and compiles to C.

- [`GOALS.md`](GOALS.md): what Ply is for.
- [`docs/GUIDE.md`](docs/GUIDE.md): the manual for the language and the `ply` command.
- [`AGENTS.md`](AGENTS.md): how to work in this repository.

## Building

```sh
scripts/bootstrap.sh --release
```

This builds `target/release/ply`: cargo builds the runtime library, then `packages/bootstrap`
builds the rest. A fresh checkout downloads the latest release (`scripts/stage0.sh`) to run the
first bootstrap. After an edit to Ply sources alone, cargo has nothing to rebuild.

These environment variables help when working on `ply` itself:

| variable | effect |
| --- | --- |
| `PLY_C_STAGE=DIR` | the directory where the compiler stores the intermediate programs it builds in stages when it compiles itself (default `ply/stage`, beside the directory where `ply` caches the generated C) |
| `PLY_C_PHASES=1` | print what each build phase did and how long it took |
| `PLY_HEAP_POISON=1` | fill freed memory with a pattern, and fail on any read of it |
| `PLY_HEAP_DELAY=N` | reuse freed memory only after `N` more frees |

## Layout

| path | contents |
| --- | --- |
| `ply.work` | where each package is, and which packages make up `ply`; `ply work` prints it |
| `packages/std` | the standard library; `prelude.ply` declares the builtins |
| `packages/compiler` | the compiler, with one directory per phase (`syntax/`, `expand/`, `check/`, `front/`, `store/`, `emit/`); `build.main`, the entry point of the *builder* (the program that compiles the CLI to C); and `cc.ply` and `native.ply`, which run the C compiler and linker |
| `packages/cli` | the `ply` command |
| `packages/bootstrap` | the code that builds a new `ply` from a checkout, which runs inside the running `ply` |
| `packages/testing` | test results, and the diagnosis of a failing test, which reports what differs between a passing run and the failing one |
| `packages/proof` | the prover behind `ply prove` |
| `packages/sim` | the search behind `simulate`, which runs the same program under many different orders of its concurrent tasks |
| `packages/store` | what `ply` stores in `.ply-cache` between runs |
| `packages/registry` | the package registry that `ply publish` and `ply resolve` talk to |
| `tests/cli` | the tests of the `ply` command, each a process it starts |
| `tests/in_process` | tests that call the compiler and the CLI in the test's own process, starting no `ply` |
| `tests/support` | what those two suites share: writing a project and reading what `ply` reports |
| `tests/std` | standard library tests that read fixture files, which are stored in the same directory |
| `examples/`, `tests/lang/`, `tests/programs/` | Ply programs that CI checks and tests; `tests/programs/` holds the programs the CLI's tests run |
| `tests/fixtures/` | Ply programs that must fail to check, each with the error it must produce |
| `crates/ply-core` | what the runtime crates share: the value encoding, source spans, the runtime's diagnostics, effects |
| `crates/ply-compiled` | loads a compiled program and runs it |
| `crates/ply-host` | the Rust code that performs effects on the machine: files, sockets, TLS, databases |
| `crates/ply-privileged` | operations only trusted programs get: compiling and running other programs, and reading the standard library inside `ply` |
| `crates/ply-launcher` | the start-up code of a native Ply program: reads the data appended to the executable and calls the program's entry point |
| `crates/ply-pack` | the format of the data appended to a native program: the standard library's sources and their precomputed check results |
| `crates/ply-runtime` | the static library every native Ply program, `ply` included, is linked with |
| `crates/<crate>-tests` | that crate's tests |
| `editors/` | the tree-sitter grammar editors use |
| `probes/` | standalone C programs, each run by a CI job |
| `.github/` | CI: `workflows/ci.yml`, the job tables in `ci-shards.sh`, and `ci-timings.sh` to read a run's timings |

## Testing

CI runs these Rust tests, these Ply tests and these other checks:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
scripts/bootstrap.sh --release
cargo nextest run --workspace
```

CI builds one test archive with `cargo nextest archive --locked --workspace`. It splits the
archive's Rust tests across several parallel jobs. The Ply tests run as `ply test` or `ply prove` of
each package, and `.github/ci-shards.sh` lists these runs. Each run has an id. `.github/ci-ply.sh
run <id>` runs the run with that id on your machine, with the same permissions CI gives it. The
PostgreSQL tests in `ply-host-tests` are skipped unless both `PLY_PG_URL` and `PLY_TEST_DB` are set.
Each variable must contain the address of a PostgreSQL server.
