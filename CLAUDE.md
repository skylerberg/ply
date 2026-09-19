# CLAUDE.md

Ply is a general-purpose programming language. Its compiler is written in Ply
(`crates/ply-compiler/ply`), compiled to C and committed as a bootstrap bundle. The Rust crates are
the runtime and the CLI.

## Prose

- The only prose documents are `README.md`, `docs/GUIDE.md` and this file. Don't add others:
  no design records, status files, reports, ledgers or notes. Git history and PR descriptions
  hold the history.
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
- CI is `.github/workflows/ci.yml`; its partitions, solo jobs and gates are tables in
  `.github/ci-shards.sh`.
- Editing `crates/ply-compiler/ply` or `crates/ply-std/ply`, comments included, turns the
  `bootstrap` CI job red. That job regenerates the bundle and uploads it as the `bootstrap-bundle` artifact:
  `gh run download <run-id> -n bootstrap-bundle -D crates/ply-compiler/bootstrap`, then commit.
- `docs/GUIDE.md` is the user manual. A change to syntax, types, builtins, the standard library,
  CLI commands, flags or exit codes, or diagnostic codes updates it in the same PR.

This file stays short. Don't add a rule in response to one incident; fix the cause instead.
