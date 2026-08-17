# Contributing to Ply

Read [`docs/ONBOARDING.md`](docs/ONBOARDING.md) first. It gets you from a fresh
clone to a running server and a verified change, with measured timings for every
step. This file is what to do *after* that: how to make a change here without
breaking something, and — the part this project cares about most — how to write
a claim down.

- [The one rule](#the-one-rule)
- [The loop](#the-loop)
- [Before you open a change](#before-you-open-a-change)
- [Writing a claim down](#writing-a-claim-down)
- [Adding things](#adding-things) — a diagnostic code, a host handler, an ADR
- [Where a change is likely to bite](#where-a-change-is-likely-to-bite)
- [Things known to be broken](#things-known-to-be-broken)
- [Style](#style)

## The one rule

**A claim is true only if you checked it against the thing that would satisfy
it.** "The ADR says so" is not evidence. Read the file, run the command, grep the
symbol.

This is not a slogan. Across sixteen milestones, adversarial review of this
repository found **seven** written claims that did not hold, and it is the most
expensive defect class the project produces — because a stated guarantee is
exactly what stops the next reader looking. The seven:

- M7 reported `exhaustive: true` over regions never examined.
- M8 disclosed an unsoundness and named a mitigation structurally incapable of
  firing.
- W1 advertised a footprint check that was never armed.
- W4 refused `DISCARD ALL` and documented the consequence nowhere.
- W5 listed a secret-exfiltration route as closed while the backstop covered
  five of seven map operations.
- R1's ADR asserted that snapshot-at-capture "is exactly ADR 0005's semantics".
  It was not; the two are distinguishable in one integer.
- R2 found `CONTRACTS.md` specifying a deleted type, and a required property
  claiming constant time for something linear.

The last two were documentation about *correct code*. Correct code does not
protect you here.

## The loop

The inner loop is fast; use it. All timings measured on the machine in
`docs/ONBOARDING.md` §Provenance.

```
cargo build --workspace                        # 0.11s warm, 16.6s cold
cargo test -p <crate>                           # seconds
./target/debug/ply test examples/               # 0.31s warm (0.03s for the release binary)
```

The outer loop, run before you call anything done:

```
cargo fmt --all --check                         # must be silent
cargo clippy --workspace --all-targets          # must be 0 warnings; 13.7s cold, 0.4s warm
cargo test --workspace                          # ~5.5 min — 3,566 pass, 0 fail, 4 ignored
```

**All three are currently clean**, re-verified by the docs audit: `fmt --check`
silent and exit 0, `clippy` zero warnings, `cargo test --workspace` 3,566 passed
/ 0 failed / 4 ignored across 147 targets in 324.5s. If you introduce the first
warning, that is a regression, not a baseline.

### There is no CI

`.github/` does not exist. Nothing runs on a push. The three commands above are
the entire verification apparatus and they only run when a human runs them.
Assume the person before you did not.

### The suite proves less than it looks like it proves

Four gates skip silently or near-silently; `ROADMAP.md`'s preamble table
enumerates them and `docs/ONBOARDING.md` §2 explains the two that matter. The
short version:

| if your change touches | you must also run |
| --- | --- |
| postgres, the pool, transaction scope | `PLY_PG_URL=postgres://localhost/postgres cargo test -p ply-host` (36–38s, 281 pass, 0 fail) — otherwise ten tests pass in 0.00s without running, and cargo captures the skip notice so nothing tells you: `skipped:` occurs zero times in a whole `cargo test --workspace` log |
| shutdown, drain, signals | anything; but know that `crates/ply-cli/tests/w5_shutdown.rs` is `#![cfg(unix)]` and compiles to nothing off Unix |
| the served request path or its cost | `./target/release/ply-corpus w6 benches/*.json`, and see §"Things known to be broken" |
| `examples/desk.ply` or any host handler | `./examples/same-tests.sh` — build `--release` first, it does not build for you |

Also: `ply-eval/tests/region_arena_cost.rs::snapshot_cost_as_a_function_of_region_size`
asserts on a wall-clock ratio and runs by default. A busy machine can fail it.
Re-run on a quiet one before you believe it.

## Before you open a change

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`,
   `cargo test --workspace` — all three, all clean.
2. If you changed behaviour, there is a test asserting the new behaviour, named
   as an English sentence. See §Style.
3. If you changed a *guarantee*, you found and updated the document that states
   it. `grep -rn` the guarantee's words across `README.md`, `DESIGN.md`,
   `ROADMAP.md`, `CONTRACTS.md` and `docs/adr/`. Roughly 24,500 lines of prose
   and no test reads any of it — you are the only check. (Re-take the figure
   rather than quoting it; `docs/ONBOARDING.md` §7 gives the command.)
4. If you changed something the shipped measurement files describe, re-take
   them. The command is in `docs/adr/0016-w6-performance.md` §"Provenance"; the
   two tests that will fail otherwise are
   `w6_report_integrity::the_shipped_ladder_still_describes_the_tree_it_ships_in`
   and
   `w6_report_allocations::the_shipped_allocation_evidence_still_describes_this_request_path`.
5. If you deleted or renamed a public type, `grep` `CONTRACTS.md` for it. It is
   a construction document written ahead of the code and it goes stale silently;
   `World` occurs in it **37 times on 33 lines**, and the `ply_eval` type of that
   name has not existed since ADR 0017. (`crates/ply-corpus/src/model.rs:269`
   defines an unrelated `World` that is alive and well, so grep for
   `ply_eval::world` rather than for the bare word.)

## Writing a claim down

This is the section that exists because of the seven failures above.

### Correct, do not delete

When a claim turns out false, **correct it in place and keep the original beside
the measurement.** A claim that was silently removed teaches nobody; a claim with
a note saying what was actually measured teaches the next reader — including
that this class of error happens here.

The house conventions:

- `README.md` uses `> **Corrected: <what was wrong>.**` blocks, which state the
  old figure, the new one, how the new one was taken, and whether the conclusion
  survives.
- `ROADMAP.md` uses `> **Audit note (docs pass, <date>): <what it used to
  say>.**` blocks.

Both quote the withdrawn text verbatim. Do the same.

### Say how it was checked, or say it was not

Every number gets a provenance: the machine, the profile, the command, and
whether it is one run or a best-of-N. Where something was **not** measured, write
"not measured" — `README.md` §"Where this is not competitive" does this
throughout and it is the most trustworthy section in the repository.

Never quote a figure from another document. Re-take it or cite the file that
holds it (`benches/w6-ladder.json`, `benches/w6-spike.json`) and the command
that renders it.

### Do not state a guarantee you have not armed

Before writing "X is refused / checked / verified", grep for the code that does
the refusing and confirm it can fire. Two live examples of what this catches:

- `E0435 DB_SCHEMA_MISMATCH` is defined, registered, and listed as reserved —
  and **constructed nowhere**. `examples/serve.sh` used to tell you `--db-schema`
  refuses at bind time with it. It does not, and a later audit pass corrected the
  comment in place: `examples/serve.sh:37-54` now carries the claim, the grep
  that refutes it, and what you actually get (`E0433 DB_PREPARE_FAILED`, per
  statement, on first execution). `README.md` §"What is missing" had this right
  all along. This is the W1 defect exactly — a check advertised and never armed —
  and it is left legible rather than deleted for that reason.
- ADR 0016's spike figures cannot be re-taken because the spike does not
  compile. See §"Things known to be broken".

The test for whether you have armed something: **name the file and line that
raises it, and the test that proves the raise.** If you cannot, write "not
enforced" instead.

### Mark what is checked

A reader cannot tell an asserted invariant from an observation. Say which:

> Selecting zero deterministic tests after a rename is an invariant the suite
> asserts — `crates/ply-cli/tests/cli.rs:145
> renaming_a_definition_re_runs_nothing` — not a heuristic.

That sentence is checkable in one grep. "The rename path is safe" is not.

## Adding things

### A diagnostic code

`crates/ply-span/src/lib.rs` holds every code as a `pub const` in `codes` **and**
a registry table in its test module pairing the constant, its name and its
literal number. The test asserts both that no code moved and that no two
constants share a number. Add to both places, or the test fails — which is the
intent.

Ranges in use: `E0001`–`E0002` and `E01xx`–`E05xx` for errors (73 codes; the two
single-digit ones are the generic pair and are easy to miss when you assume the
range starts at `E01xx`), and `W0601`–`W0610` for warnings. There is also a
reserved list in `crates/ply-eval/src/host.rs` (`DB_SCHEMA_MISMATCH` is at
`:1106`) naming codes a *handler* may not answer with. A code appearing in
`codes`, in the registry, and in that list is **still raised nowhere** — that is
exactly `E0435`'s situation.

### A host handler

`ply hosts <file> --host` is the trusted-computing-base listing and it is the
review artifact. A new handler must appear there with its footprint, determinism
flag, linearity (`repeatable` / `at-most-once`), whether it blocks, and whether
it can receive a secret. `ply hosts` also prints a digest; if you add a handler,
that digest moves and anything pinning it must move with it.

Adding to the TCB is a decision, not a change. `Cargo.toml`'s dependency
comments record why each of the three non-obvious ones (`rustls`,
`tokio-postgres`, `rust_decimal`) is there. Write the equivalent.

### An ADR

`docs/adr/` is seventeen files, `00NN-slug.md`, with **no index** — the numbers
are the ordering. Number yours `0018` and up. ADR 0005 is superseded in part by
ADR 0017, and it is the model for how to record that: 0005's *title* does not say
so, but its header does — lines 3–9 carry `Status: accepted — … §2's persistent
forkable world is **superseded by ADR 0017**` and `Superseded in part by:
docs/adr/0017-regions.md (§2 only)`, and §2 itself opens with a
`> **Superseded by ADR 0017.**` block at line 289. Do that in both files.

An ADR here is expected to state the criteria *before* the measurement, in code
where possible. `ply_corpus::w6::Criteria::default()` is the model: eight
thresholds that a measurement file cannot supply, so a number cannot set the bar
it is about to clear. That is why the M9 deferral is re-derivable
(`ply-corpus w6 benches/*.json`) rather than re-arguable.

## Where a change is likely to bite

| you changed | what breaks quietly |
| --- | --- |
| `crates/ply-hash/src/normalize.rs` | **every cached result everywhere.** The bytes are the identity. A change here is a cache-format change; see `CACHE_VERSION_CHANGED` (`W0603`). |
| `crates/ply-core/src/ty.rs` `conflicts_with` | test scheduling, silently — tests still pass, they just stop running concurrently, or start racing |
| `crates/ply-test/src/schedule.rs` `group_by_conflict` (`:216`) | same; `parallelism()` at `:172` is what reports it |
| `crates/ply-eval/src/code.rs` | `crates/ply-codegen-spike`, which nothing compiles. It has already bit-rotted this way. |
| the request path | `benches/w6-ladder.json` and the two integrity tests, and the M9 verdict that reads it |
| any public signature | `CONTRACTS.md`, which no test reads |
| `examples/desk.ply` | `examples/serve.sh`, whose `rewrite()` (`serve.sh:103-112`) matches exact source lines with `grep -qF` and aborts loudly if one is missing — that abort is deliberate and is the good case. How many lines it rewrites depends on the mode: `--memory` rewrites two (`:122` and `:125`), `--tls` rewrites one (`:127`), and a plain `--db` run rewrites none |

## Things known to be broken

Recorded here so nobody spends an afternoon rediscovering them.

1. **`crates/ply-codegen-spike` does not compile.** Two independent walls:
   cranelift 0.134.3 requires rustc ≥ 1.94.0 (no `rust-toolchain.toml` pins
   anything), and on 1.94.0 it fails with two `E0164`s because
   `ply_eval::code::Stmt::Expr` became a struct variant. It is outside the
   workspace, so nothing compiles it and nothing caught the drift. Consequence:
   ADR 0016's `11.67x`, `1.71x` and §9.2 census are **not reproducible from this
   tree**, and `ROADMAP.md` §"What is next" item 3 — re-measure codegen's
   ceiling — is blocked until this is fixed or the crate is deleted per ADR 0016
   §3.5. ADR 0016 records the toolchain wall only, at lines 764–767 and
   1105–1106; the source-incompatibility wall is recorded nowhere but here and
   in `docs/ONBOARDING.md` §1.
2. **`examples/same-tests.sh` does not build the binary it runs.** It uses
   `target/release/ply` (line 44) with no `cargo build` anywhere.
3. **`examples/same-tests.sh` step 1 can be vacuous.** It passes
   `--no-incremental`, which disables only the front-end cache; on a warm
   `examples/.ply-cache` it prints `0 failed, 0 passed, 68 cached` and the script
   exits 0. `--no-cache` is the flag that forces the run.
4. ~~**`README.md`'s `ply-corpus gen` invocation is missing the required
   `--out`** and fails verbatim.~~ **Fixed** at `README.md:97`, along with the
   missing positional corpus argument on the `ply-corpus bench` beside it.
   `benches/README.md` always had the correct form.
5. ~~**`examples/serve.sh` claims `--db-schema` refuses with `E0435`.**~~
   **Fixed.** It claimed that and nothing raises `E0435`; the comment at
   `examples/serve.sh:37-54` now records the claim, the grep that refutes it, and
   the error you actually get. Kept in this list because the *code* gap — no
   schema check at bind time — is still open; only the false comment was closed.
6. **`PLY_PG_URL` is set by nothing in the repository**, so ten postgres tests
   pass without running by default.
7. **No `LICENSE` or `LICENSE-APACHE` file exists** although `README.md:499` and
   the workspace root `Cargo.toml:22` declare `MIT OR Apache-2.0`. The thirteen
   member crates inherit it with `license.workspace = true` rather than each
   carrying the SPDX expression, and `crates/ply-codegen-spike/Cargo.toml`
   declares no license at all.

Items 2, 3, 4, 6 and 7 are one-line fixes this documentation pass did not make,
because the rule is that code is what shipped and a documentation pass corrects
documents. Item 5's comment was a document and was corrected; item 1 is a real
code defect and is reported, not fixed. They are open.

## Style

**Rust.** `cargo fmt` defaults; `cargo clippy --all-targets` clean. Edition
2024, resolver 3.

**Test names are English sentences.** `renaming_a_definition_re_runs_nothing`,
`the_shipped_ladder_still_describes_the_tree_it_ships_in`,
`a_shipped_definition_the_project_never_touched_is_not_a_suspect`. This is not
decoration — grepping test names for a behaviour is the fastest index this
repository has, and it is the reason a stale test is visible.

**Assertion messages state the failure, not the expectation.** `"a rename
rebuilt something:\n{text}"` beats `assert_eq!`'s default. A failing test should
be readable by someone who has never seen the file.

**Comments explain a non-obvious *why*.** The existing code comments are dense
and load-bearing — `crates/ply-hash/src/lib.rs`'s explanation of why the
component loop re-encodes once more after the partition settles is the model.
Do not add comments that restate the code or a type signature.

**Dependencies.** Pin the latest stable version and write, in `Cargo.toml`, why
the crate is there and why not the obvious alternative. The existing comments on
`memchr`, `rust_decimal`, `rustls` and `tokio-postgres` are the standard. Adding
a dependency to `ply-host` grows the trusted computing base that
`ply hosts --host` invites a reader to audit; treat it as a decision.

**License.** MIT OR Apache-2.0. By contributing you agree your work is licensed
under both.
