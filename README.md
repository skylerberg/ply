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

[`docs/GUIDE.md`](docs/GUIDE.md) is the language manual: syntax, types, effects,
tests, the standard library, the `ply` command and the diagnostic codes.
[`docs/DIRECTION.md`](docs/DIRECTION.md) is what the language is for and what that asks
of it next.

## Layout

| path | holds |
| --- | --- |
| `crates/ply-eval` | values, the evaluator, the scheduler and the simulator; spans, diagnostics and their codes; the program record the compiler answers with |
| `crates/ply-codegen` | the C backend's runtime: loads a compiled unit, or the one an executable was linked with, and runs it |
| `crates/ply-compiler` | the compiler written in Ply (`ply/`), with its builder (`build.main`) and the C compiling and linking it drives (`cc.ply`, `native.ply`) |
| `crates/ply-store` | what `ply` keeps under `.ply-cache`, as the `store` package in `ply/`; not a cargo crate |
| `crates/ply-test` | what a test run decides, as the `suite` package in `ply/`; not a cargo crate |
| `crates/ply-prove` | specification obligations and their discharge, as the `prove` package in `ply/`; not a cargo crate |
| `crates/ply-sim` | the interleaving search, as the `sim` package in `ply/`; not a cargo crate |
| `crates/ply-host` | the Rust handlers effects resolve to (db, fs, tcp, tls, ...) |
| `crates/ply-machine` | the nested-entry capability: a program loading and entering another program; and the runtime `ply test` and `ply prove` drive |
| `crates/ply-std` | the standard library, as Ply source in `ply/`, the builtins' declarations (`prelude.ply`) among it |
| `crates/ply-cli` | the `ply` program, as Ply source in `ply/`; not a cargo crate |
| `crates/ply-registry` | the package registry `ply publish` and `ply resolve` talk to, as Ply source in `ply/`; not a cargo crate |
| `crates/ply-launcher` | what enters a native program: its pack read, its roots and hosts lent, its entry called |
| `crates/ply-native` | the runtime's static library a native program, `ply` among them, is linked with |
| `crates/ply-std-tests` | the standard library's tests that read fixture files, as Ply source in `ply/` beside the files; not a cargo crate |
| `crates/<crate>-tests` | that crate's tests |
| `editors/` | the tree-sitter grammar editors read |
| `examples/`, `tests/lang/`, `tests/fixtures/` | Ply programs the suite runs |
| `probes/` | standalone C probes, each run by a CI job |
| `.github/` | CI: `workflows/ci.yml`, the job tables in `ci-shards.sh`, and `ci-timings.sh` to read a run |

## Building and testing

CI is the verifier; every pull request runs these checks and the whole suite:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
scripts/bootstrap.sh --release
cargo nextest run --workspace
```

`ply` is a native program: the CLI compiled to C and linked with the runtime's static library,
with a pack after it holding the shipped modules, what they answer and what the CLI's front end
answered of the definition it enters. `scripts/bootstrap.sh` has cargo build that library and
`ply bootstrap` build the rest, run by the `ply` already in `target/` or, in a fresh checkout, the
last release (`scripts/stage0.sh`). Cargo builds nothing after an edit to Ply sources alone.

CI builds one `cargo nextest archive --locked --workspace` and runs it in shards.
The Ply tests run as the runs `.github/ci-shards.sh` lists, each one `ply test` or `ply prove` of a
package in a `runs` job: the library's tests and its proofs, the CLI's suite in parts, and the
packages' own suites. `.github/ci-ply.sh run <id>` runs one with the grants CI passes.
The postgres tests in `ply-host-tests` skip unless `PLY_PG_URL` and `PLY_TEST_DB`
name a server.

## The compiler bootstrap

The compiler is Ply source under `crates/ply-compiler/ply`. `ply bootstrap` describes the runtime's
library cargo built of the checkout, builds the builder against it, a native program on that
runtime that enters the compiler's own `build.main`, and runs the builder over the CLI, whose C it
links with the library into the new `ply`. That `ply` answers the shipped modules, and its pack
carries them. The builder and the pack are kept in the toolchain's cache under what decides them,
so a bootstrap makes again only what moved. The builder is built by the `ply` that runs the
bootstrap, a release in CI, so the shipped modules the builder pulls, the compiler's and the
standard library's they import, cannot use a language rule the same change introduces; the rest
of the standard library can.
