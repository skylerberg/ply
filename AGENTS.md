# AGENTS.md

Ply is a general-purpose programming language. [`GOALS.md`](GOALS.md) says what it is for and is
the source of truth for every design decision. A human wrote it: never edit it.

Critical Path project: Ply (pass `--project "Ply"` to `cpath`).

## How `ply` is built

- `ply` is a native program made of three Ply packages and one Rust library:
  - `packages/compiler`: the compiler, written in Ply.
  - `packages/std`: the standard library.
  - `packages/cli`: the command-line program.
  - `crates/ply-runtime`: the runtime, a static library that cargo builds.
- `ply bootstrap` builds a new `ply` from a checkout. The running `ply` compiles the checkout's
  compiler into a program called the *builder*. The builder compiles the CLI to C, and that C is
  linked with the runtime library.
- `scripts/bootstrap.sh` is the full rebuild: cargo builds the runtime, then the `ply` in `target/`
  runs `ply bootstrap`. A fresh checkout has no `ply` yet, so the script downloads the latest
  release (`scripts/stage0.sh`) for the first bootstrap. `PLY_C_PHASES=1` prints how long each step
  took.
- You need a new binary only when an edit changes what `ply` itself does. Inside a checkout, `ply`
  reads `std.*` and `compiler.*` from the checkout's sources, so `scripts/iterate.sh <package>
  [filters...]` checks and tests an edit to the compiler or the standard library with the newest
  `ply` you have.

## Writing

- Write plainly. Use short sentences, one idea each, in common words. Use the standard term for a
  standard idea: "returns", not "answers"; "borrows", not a new verb. Define a term the first time
  you use it.
- Be brief. Prose costs tokens, time and maintenance: cut every sentence the reader does not need.
- Make each comment, message and paragraph readable on its own. Name the thing you mean, and write
  "it" only when what it refers to is unmistakable. Give the main point first, then the exceptions.
- Prose is `README.md`, `docs/GUIDE.md` and this file. A document says what its reader needs and
  nothing more, and is deleted when it stops being true. No status files, reports, logs or dated
  notes: git history and PR descriptions keep history.
- State a direction as the direction. No hedging and no time estimates.
- Correct in place. Never write "previously", "corrected" or "a draft of this said".
- Measurements belong in the description of the PR they decide, not in the tree.
- PR descriptions and commit messages: a few lines on what changed and why.
- Cards on the board follow these rules too. A card's title is one imperative deliverable. Its
  description gives the problem, the direction and when it is done, in words a reader new to the
  code can follow. No card codes, dates or links to scratch files.

## Comments

- Default to none. Clear names and small functions should explain the code; a comment that
  restates it is noise. Write one only for a reason the code cannot show, an invariant the types
  do not enforce, or a trap, and keep it to one line. Delete stale or redundant comments you pass.
- No history ("used to", "since #123"), no references to PRs or documents, no figures.
- Doc comments (`///` above a declaration, `//!` at the head of a module) are for callers. Open
  with a one-sentence summary, then say what the signature cannot: what the result means, when it
  raises, units, edge cases. Never restate the type. No examples: the tests that use a definition
  are its examples, and `ply callers` lists them.

## Naming

- Name a thing by what it is or does, in full words: `return_type`, not `ret_ty`; `expression`,
  not `expr`. Abbreviations are fine only where they are the everyday word outside this project
  (`url`, `json`, `http`).
- Don't invent terms. Use the established name for an established idea. Where Ply has a genuinely
  new idea, give it a plain name and define it in the guide.
- No version or age markers (`v2`, `new_`, `old_`) and no task, PR or card numbers in names.
- When the plain word is a keyword, name the role instead (`original`, `annotation`).

## Work

- Fix a bug's class, not just the instance: a test that would have caught it, a type that cannot
  represent the mistake, or a check that fails the build. Never a paragraph warning the next
  reader.
- Do the work; don't narrate it. Never open a PR whose only content is a finding, a measurement
  or a plan.
- Read the call sites before planning a change. Don't plan from prose.
- One goal, one PR. Bugs found on the way go in it, with their tests. Split only when one change
  must be on `main` before another can build on it.
- Find waste by reading what the code computes, copies and repeats. Measure only a number that
  decides a choice in front of you. One slow CI run is not a work item: hosted runners sometimes
  start jobs late.
- Work that only serves tests or proofs, such as counting steps, must cost a production run
  nothing.
- One implementation: no environment switches, fallbacks or second paths to get a test green, and
  no stopgap that a planned change will delete.
- A rule lives in one place. When two places restate one rule, route both through one function in
  the module that owns it and delete the copy.
- No backwards compatibility: delete retired flags, APIs and modes outright.
- New code is Ply. Port Rust to Ply when you work on it.

## Tests

- Tests check the program, never a document.
- Write only the tests that matter, and keep each small and fast.
- Assert relations rather than counts a layout change would move: equal at two sizes, equal to a
  reference, `live == 0`.
- A test that touches a real resource uses a handler instead. A few `test/nondet` tests check the
  real handlers.
- A program a CLI test runs is a file under `tests/programs/<test module>/` that the test reads with
  `embed`, so `ply check`, `ply fmt` and `ply rename` cover it. Use a raw string only for text that
  must fail to check, that the test builds, or that is not Ply.
- Rust tests test only the Rust: none compiles Ply or reads the standard library. Ply tests cover
  what Ply programs do, including the runtime's internals.

## Mechanics

- Before pushing, run the cheap checks CI fails on: `ply fmt --check` on the packages you changed;
  `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` when Rust
  changed; `scripts/ci-lint.sh` when CI files changed; and `npx tree-sitter generate` and `npx
  tree-sitter test` in `editors/tree-sitter-ply` when syntax changed. Push, then run anything slower
  alongside CI.
- Rust tests live in sibling `<crate>-tests` crates, so a `pub` change can break crates you didn't
  touch.
- The CLI's tests are the Ply package `tests/cli`. `.github/ci-ply.sh run cli-<module>` runs one
  module with the permissions CI gives it.
- CI is `.github/workflows/ci.yml`. `.github/ci-shards.sh` lists CI's Ply test runs (each is `ply
  test` of one package) and which job runs each.
- A pull request bootstraps only when it changes the runtime's Rust, `ply.work`, the compiler, the
  CLI, a package their manifests list, or a file one of those embeds from outside itself, such as
  `std.prelude`, whose text the checker reads builtins from (`compiler.prelude_source`). Otherwise
  its suites run with the newest `ply` main bootstrapped of those, which reads the rest of the
  checkout's standard library from its sources. Main bootstraps whenever any of the library changed,
  so a library change reaches the `ply` binary only once it merges.
- A standard library module's behavior is covered by its own tests or `tests/std`, which run on the
  checkout's library. The CLI's suite tests the CLI: the programs it checks and runs see the library
  the binary ships, which is main's on a pull request that bootstraps nothing.
- CI bootstraps twice. First the latest release builds the pull request's builder; then the `ply`
  that produced builds itself again, reusing the first builder unless the code that makes it
  changed. Because the release compiles the builder:
  - The compiler, `std.prelude` and the standard library modules the compiler imports (`grep -rh
    '^import std' packages/compiler`, and what those import) cannot use a language feature added in
    the same pull request. The rest of `packages/std` can.
  - The builder's command-line arguments, how the runtime describes itself to the compiler, and
    the layout of the data appended to a `ply` binary can change only in a form the release still
    understands.
- A file under `packages/std` that is not `.ply` is data a standard library module embeds by its
  path (`embed("oid/names.txt")` in `oid.ply`). It ships inside every `ply` binary, so test
  fixtures go in `tests/std`, not here.
- `packages/` contains the Ply packages and `crates/` the cargo packages. `ply.work` says which
  package plays which part of `ply`; CI and the scripts read it with `ply work`. CI publishes each
  build of `main` to the rolling `main-latest` release, which `scripts/stage0.sh` downloads.
- A `ply.lock` records a path dependency inside this checkout (proof, sim, store, testing) by name
  and version only, so editing that package changes no lock. Adding or removing a dependency, or
  changing a version, does: run `ply resolve` in the lock's directory, as CI does for every
  `ply.lock`.
- `docs/GUIDE.md` is the user manual. A change to syntax, types, CLI commands, flags or exit codes
  updates it in the same PR. Diagnostic codes are documented in `packages/cli/explain.ply`, which
  `ply explain` prints. Builtins and the standard library are documented by their doc comments.

This file stays short. Don't add a rule in response to one incident; fix the cause instead.
