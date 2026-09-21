# AGENTS.md

Ply is a general-purpose programming language. Its compiler is written in Ply
(`crates/ply-compiler/ply`), compiled to C and committed as a bootstrap bundle. The Rust crates are
the runtime and the CLI.

## Prose

- Prose is `README.md`, `docs/`, and this file. A document earns its place by being read: it
  says what a reader needs and nothing more, and it is deleted when it stops being true. No
  status files, reports, ledgers or dated readings. Git history and PR descriptions hold the
  history.
- Comments: default to none. Write one only for a non-obvious why, an invariant the types don't
  enforce, or a trap, and keep it to one line. No history ("used to", "since #123"), no
  references to PRs or documents, no figures.
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
- CI is `.github/workflows/ci.yml`; `.github/ci-shards.sh` holds its solo and gate tables and cuts
  the test partitions from the durations CI measured.
- Never commit `crates/ply-compiler/bootstrap`, `crates/ply-cli/bootstrap` or
  `crates/ply-codegen-tests/fixtures/goldens` in a pull request: CI regenerates all three on
  `main` after each merge (the `refresh` job), and a pull request runs its sources through the
  checked-in bundle as a stage. A golden that moved is listed in the partition job's summary;
  read it, since nothing fails on it.
- The bundle carries `crates/ply-compiler/ply` and the shipped modules it imports, pulled as a
  project's are (today `std.hash` alone), so only those cannot use a language rule the same pull
  request introduces. The rest of `crates/ply-std/ply` can.
- `docs/GUIDE.md` is the user manual. A change to syntax, types, builtins, the standard library,
  CLI commands, flags or exit codes, or diagnostic codes updates it in the same PR.

This file stays short. Don't add a rule in response to one incident; fix the cause instead.
