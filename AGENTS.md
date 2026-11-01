# AGENTS.md

Ply is a general-purpose programming language. [`GOALS.md`](GOALS.md) explains what Ply is for, and
it is the source of truth for every design decision. A human wrote it, so never edit it.

Critical Path project: Ply (pass `--project "Ply"` to `cpath`).

## How `ply` is built

- `ply` is a native program built from three Ply packages and one Rust library:
  - `packages/compiler`: the compiler, written in Ply.
  - `packages/std`: the standard library.
  - `packages/cli`: the command-line program.
  - `crates/ply-runtime`: the runtime, a static library that cargo builds.
- `packages/bootstrap` builds a new `ply` from a checkout. The running `ply` compiles that package
  and the checkout's compiler, and runs them inside itself on its own runtime, the way `ply run`
  runs a program. They compile the CLI to C for the checkout's runtime, and that C is linked with
  the runtime library. So C from one version of Ply is never linked with the Rust of another.
- `scripts/bootstrap-with.sh` runs that bootstrap with a `ply` you name. `scripts/bootstrap.sh` does
  a full rebuild: cargo builds the runtime, and then the `ply` in `target/` runs the bootstrap. A
  fresh checkout has no `ply` yet, so for the first bootstrap the script downloads the latest
  release (`scripts/stage0.sh`). Set `PLY_C_PHASES=1` to print how long each step took.
- You need to build a new `ply` only when your edit changes what `ply` itself does. Inside a
  checkout, `ply` reads the `std.*` and `compiler.*` modules from the checkout's sources. So to
  check and test an edit to the compiler or the standard library, run `scripts/iterate.sh <package>
  [filters...]` with the newest `ply` you have.

## Writing

- Write plainly. Use short, complete sentences in common words, one idea per sentence. Say who does
  what to what: don't drop a sentence's subject or object, and don't make the reader work out what
  "one", "there" or "it" refers to. Use the standard term for a standard idea: "returns", not
  "answers"; "borrows", not a new verb. Define a term the first time you use it.
- Be brief. Prose costs tokens, time and maintenance, so cut every sentence the reader doesn't need.
  Brief means fewer sentences, not compressed ones.
- Make each comment, message and paragraph readable on its own. Give the main point first, then the
  exceptions.
- The only prose documents are `README.md`, `docs/GUIDE.md` and this file. A document should say
  what its reader needs and nothing more, and should be deleted when it stops being true. Don't
  write status files, reports, logs or dated notes; git history and PR descriptions already record
  history.
- Say what to do directly. Don't hedge, and don't estimate how long something will take.
- When text is wrong, fix the text itself. Never write "previously", "corrected" or "a draft of
  this said".
- Put measurements in the description of the PR they support, not in the repository.
- Keep PR descriptions and commit messages to a few lines about what changed and why.
- Cards on the Critical Path board follow these rules too. A card's title is a command that names
  one deliverable. Its description explains the problem, the planned approach, and when the card is
  done, in words that someone new to the code can follow. Don't put card codes, dates or links to
  scratch files in a card.

## Comments

- Don't write comments. Clear names and small functions should explain the code. CI fails on every
  comment except a doc comment on a public definition of the standard library, and the few comments
  that a test reads as its input.
- A standard library doc comment (`///` above a public declaration, `//!` at the top of a module) is
  written for the people who call the code. Start with a one-sentence summary. Then say what the
  signature can't: what the result means, when the function raises, units, and edge cases. Don't
  repeat the types. Don't include examples: the tests that use a definition serve as its examples,
  and `ply callers` lists those tests. Don't describe history, refer to PRs or quote measurements.

## Naming

- Name a thing by what it is or does, in full words: `return_type`, not `ret_ty`; `expression`,
  not `expr`. Abbreviations are fine only where they are the everyday word outside this project
  (`url`, `json`, `http`).
- Don't invent terms. Use the established name for an established idea. Where Ply has a genuinely
  new idea, give it a plain name and define it in the guide.
- Don't put version or age markers (`v2`, `new_`, `old_`) or task, PR or card numbers in names.
- When the plain word is a keyword, name the value's role instead (`original`, `annotation`).

## Work

- Fix the whole class of a bug, not just the one instance. Add a test that would have caught the
  bug, a type that can't represent the mistake, or a check that fails the build. Don't write a
  paragraph warning the next reader instead.
- Do the work; don't narrate it. Never open a PR whose only content is a finding, a measurement or
  a plan.
- Read the call sites before you plan a change. Base the plan on the code, not on descriptions of
  it.
- Make one PR per goal. Fix the bugs you find along the way in the same PR, with their tests. Split
  the work into several PRs only when one change must be on `main` before another can build on it.
- Find waste by reading the code for work it computes, copies or repeats needlessly. Measure
  something only when the number decides a choice you are making now. One slow CI run is not worth
  investigating: hosted runners sometimes start jobs late.
- Code that exists only for tests or proofs, such as code that counts steps, must add no cost when
  the program runs in production.
- Keep one implementation of each thing. Don't add environment switches, fallbacks or a second code
  path to make a test pass, and don't add a stopgap that a planned change will delete.
- Write each rule in one place. When two places implement the same rule, make both call one
  function in the module that owns the rule, and delete the copy.
- Don't keep backwards compatibility. When you retire a flag, API or mode, delete it.
- Write new code in Ply. When you work on Rust code, port it to Ply.

## Tests

- Tests check what the program does, never the text of a document.
- Write only the tests that matter, and keep each test small and fast.
- Assert relationships, not exact counts that a layout change would break. For example, assert that
  two sizes give equal results, that a result equals a reference, or that `live == 0`.
- Tests use handlers in place of real resources such as files and the network. A few `test/nondet`
  tests check the real handlers.
- When a CLI test runs a program, put the program in a file under `tests/programs/<test module>/`
  and read it with `embed`, so that `ply check`, `ply fmt` and `ply rename` cover the program. Use a
  raw string only for text that must fail to check, text the test builds, or text that isn't Ply.
- Rust tests test only Rust code: none of them compiles Ply or reads the standard library. Ply tests
  cover what Ply programs do, including the runtime's internals.

## Mechanics

- Before you push, run the quick checks that CI fails on:
  - `ply fmt --check` on the packages you changed;
  - `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` when you
    changed Rust code;
  - `scripts/ci-lint.sh` when you changed CI files;
  - `npx tree-sitter generate` and `npx tree-sitter test` in `editors/tree-sitter-ply` when you
    changed the syntax.

  Then push, and run any slower checks while CI runs.
- Each crate's Rust tests are in a separate `<crate>-tests` crate, so changing something `pub` can
  break crates you didn't touch.
- The CLI's tests are the Ply package `tests/cli`. Each test starts `ply` as a process, and the suite
  does not import the CLI, so it loads in seconds. A test belongs in this suite only if it needs a
  real process: exit codes, streams, signals, a terminal, sockets, or a command that compiles and
  runs a program. `.github/ci-ply.sh run cli-<module>` runs one test module with the same
  permissions that CI gives it.
- `tests/in_process` calls the compiler and the CLI in the test's own process. Use it for tests that
  cannot go beside the code: a test that calls `main` would import the package it tests, and a
  fixture embedded from `packages/compiler` or `packages/cli` becomes a bootstrap input. This suite
  builds its compiler at `-O0`, so calling `main` costs as much as starting `ply`, and compiling a
  program costs far more. First try a test beside the function's code, or a program in `tests/lang`.
  `.github/ci-ply.sh run package-in_process --filter <module>.` runs one module.
- CI is defined in `.github/workflows/ci.yml`. `.github/ci-shards.sh` lists the Ply test runs in CI
  (each runs `ply test` on one package) and which job runs each one.
- A pull request bootstraps a new `ply` only when it changes the runtime's Rust code, `ply.work`, the
  bootstrap, the compiler, the CLI, a package listed in their manifests, or a file outside those packages that one
  of them embeds. `std.prelude` is an example of such a file: the checker reads the builtins from
  its text (`compiler.prelude_source`). Other pull requests run their tests with the newest `ply`
  built on main, and that `ply` reads the rest of the standard library from the checkout's sources.
  Main bootstraps whenever anything in the standard library changes, so a change to the library
  reaches the `ply` binary only after it merges.
- Test a standard library module in its own tests or in `tests/std`. Both run against the checkout's
  standard library. The CLI's test suite tests the CLI. The programs those tests check and run use
  the standard library built into the `ply` binary, which is main's library when the pull request
  doesn't bootstrap.
- CI bootstraps twice. First, the latest release runs the pull request's bootstrap. Then the new
  `ply` runs it again. The release compiles that code and runs it on its own runtime, so:
  - The bootstrap, the compiler, `std.prelude` and the standard library modules they import (`grep
    -rh '^import std' packages/bootstrap packages/compiler`, and the modules those import) can't use
    a language feature or call a native function added in the same pull request. The rest of
    `packages/std` can.
  - `scripts/bootstrap-with.sh` can use only what the release's `ply hash` and `ply run` accept.
  - Nothing else needs a step for the release. A runtime helper, a layout field, the way the runtime
    describes itself and the layout of the data appended to a `ply` binary can all change in one
    pull request.
- A file under `packages/std` that isn't a `.ply` file is data that a standard library module embeds
  by its path (`embed("oid/names.txt")` in `oid.ply`). Every `ply` binary includes these files, so
  put test fixtures in `tests/std` instead.
- `packages/` contains the Ply packages, and `crates/` contains the cargo packages. `ply.work` says
  which package is the compiler, the standard library, the CLI and the runtime; CI and the scripts
  read it with `ply work`. CI publishes each build of `main` to the `main-latest` release, replacing
  the previous build, and `scripts/stage0.sh` downloads it.
- A `ply.lock` records a dependency on a path inside this checkout (proof, sim, store, testing) by
  name and version only, so editing that package doesn't change any lock. Adding or removing a
  dependency, or changing a version, does change the lock: run `ply resolve` in the lock's
  directory. CI runs `ply resolve` for every `ply.lock`.
- `docs/GUIDE.md` is the user manual. A PR that changes syntax, types, CLI commands, flags or exit
  codes updates the guide too. `packages/cli/explain.ply` documents the diagnostic codes, and `ply
  explain` prints that documentation. Builtins and the standard library are documented in their doc
  comments.

Keep this file short. Don't add a rule because of one incident; fix the cause instead.
