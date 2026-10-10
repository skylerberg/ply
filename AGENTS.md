# AGENTS.md

Ply is a general-purpose programming language. Its compiler is written in Ply
(`crates/ply-compiler/ply`), and its builder (`build.main`) is committed as a runnable the launcher
enters to build `ply` itself. The Rust crates are the runtime and the launcher; the Ply sources and
runnables a `ply` binary carries are appended to it by `cargo pack BINARY`, never compiled into
Rust, so after editing Ply sources `cargo pack` is the rebuild.

## Prose

- Prose is `README.md`, `docs/`, and this file. A document earns its place by being read: it
  says what a reader needs and nothing more, and it is deleted when it stops being true. No
  status files, reports, ledgers or dated readings. Git history and PR descriptions hold the
  history.
- Comments: default to none. Write one only for a non-obvious why, an invariant the types don't
  enforce, or a trap, and keep it to one line. No history ("used to", "since #123"), no
  references to PRs or documents, no figures.
- Doc comments (`///` above a declaration, `//!` at the head of a module) are for callers, and
  the rule above does not bound them: say what the signature cannot (what an answer means, when it
  raises, units, edge cases), never restate the type or row, and open with a one-sentence summary.
  No examples: the tests that name a definition are its examples, and `ply doc` lists them.
- Correct in place. Never write "previously", "corrected" or "a draft of this said".
- Measurements belong in the PR description of the change they decide, not in the tree.
- Tests check the program, never a document.
- PR descriptions and commit messages: a few lines on what changed and why.

## Work

- Fix a bug's class, not just the instance: a test that would have caught it, a type that cannot
  represent the mistake, or a check that fails the build. Never a paragraph warning the next
  reader.
- Do the work; don't narrate it. Never open a PR whose only content is a finding, a measurement
  or a plan.
- Read the call sites before planning a change. Don't plan from prose.
- Measure only what decides a choice in front of you. One slow CI run is not a work item:
  hosted runners sometimes start jobs late.
- One implementation: don't add environment switches, fallbacks or second paths to get a test
  green.
- No backwards compatibility: delete retired flags, APIs and modes outright.

## Mechanics

- Tests live in sibling `<crate>-tests` crates, so a `pub` change can break crates you didn't
  touch. Run `cargo fmt --all` before pushing; CI runs clippy with `-D warnings`.
- The CLI's tests are the Ply package `crates/ply-cli-tests/ply`; `.github/ci-corpus.sh run
  cli-<module>` runs one module with the grants CI gives it.
- CI is `.github/workflows/ci.yml`; `.github/ci-shards.sh` holds its gate tables and cuts
  the nextest shards and the corpus partitions from the durations CI measured.
- Never commit `crates/ply-compiler/bootstrap` or `crates/ply-cli/bootstrap` in a pull request:
  CI regenerates both on `main` after each merge (the `refresh` job), and a pull request's `ply`
  program is built by the checked-in builder, or by the builder that one builds of the pull
  request's compiler where the program needs a rule the checked-in one lacks.
- The builder carries `crates/ply-compiler/ply` and the shipped modules it imports, pulled as a
  project's are (`grep '^import std' crates/ply-compiler/ply/*.ply` and what those import), so
  only those cannot use a language rule the same pull request introduces. The rest of
  `crates/ply-std/ply` can, and so can `crates/ply-compiler/prelude.ply`, which declares the
  builtins: the compiler embeds its text and parses it itself. The runtime reads the front end's
  answer the committed builder wrote, so a field it comes to require of one lands after main's
  builder writes it.
- A file below `crates/ply-std/ply` that is not a `.ply` is data a shipped module embeds by its
  path from the module (`embed("oid/names.txt")` in `oid.ply`). It ships in every binary and a
  load reads it whenever it pulls the module, so a test's fixtures stay in
  `crates/ply-corpus/stdlib`.
- `crates/ply-cli` is the CLI as a Ply program plus the runnable `ply bootstrap` makes of it
  (`bootstrap/ply.run`); it is not a cargo crate. The `refresh` job rebuilds both runnables on
  main by driving the released binary, so the checkout can rebuild itself without cargo.
- A `ply.lock` records a path dependency in this checkout (prove, sim, store, suite) by name and
  version only, so editing one moves no lock. Adding or dropping a dependency or bumping a version
  does: run `ply resolve` on the lock's directory, as CI's lock gate does for every `ply.lock`.
- `docs/GUIDE.md` is the language reference and `docs/book/` is the guide. A change to syntax, types,
  CLI commands, flags or exit codes, or diagnostic codes updates the reference in the same PR, and
  the guide where it teaches the same thing. The builtins and the standard library are
  documented by their doc comments, which a change to them updates.

This file stays short. Don't add a rule in response to one incident; fix the cause instead.
