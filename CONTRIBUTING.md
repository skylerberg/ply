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
cargo test --workspace                          # 9.5-29 min — 3,690 pass, 0 fail, 5 ignored
```

**All three are currently clean**, re-verified after R4 (2026-08-21): `fmt
--all --check` silent and exit 0, `clippy --workspace --all-targets` **zero**
warnings and zero errors, `cargo test --workspace --no-fail-fast` **3,644 passed
/ 0 failed / 5 ignored** across **155 targets** (142 binaries + 13 doc-test
suites). If you introduce the first warning, that is a regression, not a
baseline.

> **Re-taken after the frame-ceiling fix (2026-08-24).** The line above read
> `9.5-18 min — 3,661 pass, 0 fail, 5 ignored`. `cargo test --workspace -j 2
> --no-fail-fast -- --test-threads=2` now reports **3,690 passed / 0 failed / 5
> ignored** across **155 targets** (142 binaries + 13 doc-test suites), exit 0,
> **1,735.2s real / 1,548.7s user** by `/usr/bin/time -p`. That wall clock was
> taken on a machine at load 25–43 with three other agents building and testing
> on it, so it is an upper bound and not comparable with the older figures; the
> counts are deterministic and are. The +29 is three tests from §"Things known
> to be broken" items 9, 10 and 13's close and twenty-six from the per-gate
> tests item 13's close added. `fmt --all --check` silent, `clippy --workspace
> --all-targets` zero warnings and zero errors, both re-run on the same tree.
> Two caveats worth carrying. The heaviest new test peaks near 4.2 GiB — see
> §"The suite proves less than it looks like it proves". And the **binary count
> is 142 here where the reading above it is 143, and this pass could not account
> for the difference**: every `tests/*.rs` in every workspace member appears in
> the run except the five under `crates/ply-codegen-spike/`, which are excluded
> by design because that crate declares its own `[workspace]` (item 1), and
> `value_semantics_audit` — the binary the 143 reading added — is present and
> ran. Recorded rather than reconciled, because "the count moved and I guessed
> why" is the failure this file is about.

> **Re-verified again (second regression audit, 2026-08-21).** `fmt --all
> --check` silent and exit 0; `clippy --workspace --all-targets` zero lines
> matching `warning` or `error`; `cargo test --workspace --no-fail-fast`
> **3,661 passed / 0 failed / 5 ignored** across **156 targets** (**143**
> binaries + 13 doc-test suites), **1,087.3s real** on a machine at load 4–12.
> The four new tests are one binary the audit before it added
> (`ply-eval/tests/value_semantics_audit.rs`) and three the fixes for what it
> found added: two in `ply-eval/tests/map_order.rs` and
> `value_semantics_audit.rs`, and
> `ply-cli/tests/derivation_determinism_audit.rs::a_decimal_keyed_map_encodes_one_body_whichever_spelling_was_written_last`.

> **Re-taken after R4 (2026-08-21).** The line read `~6.5 min — 3,597 pass, 0
> fail, 4 ignored` and the paragraph `3,597 / 0 / 4 across 151 targets in
> 399.6s, and again at 406.9s`. R4 added four test binaries; §"Things known to
> be broken" and `docs/ONBOARDING.md` §2 name them. The wall clock re-take is
> **569.3s (9m 29s) real, 726.6s user**, `/usr/bin/time -p` on one run from an
> already-built `target/` — and that run was **not on an idle machine**, so it
> is an upper-ish bound rather than the clean figure the older ones are. The
> `test result:` lines sum to 478.5s of in-target time, excluding compilation.
> The reason the estimate went up is one target:
> `ply-corpus/tests/r4_value_construction.rs` takes **70.9s in debug and 25.6s
> in release**, because it captures a backtrace per allocation. It is not
> `ignored`, and the figures it is documented to print are the release ones.

> **The counts moved twice and the reason is in the tree both times, not in the
> suite.** They read 3,566 / 147 / 324.5s, then 3,584 / 150 / 352.4s. R3 added
> three test binaries and nine tests inside existing ones; the audit after it
> added `ply-eval/tests/hoist_staleness_audit.rs`, and the fixes for what it
> found added three more tests — including the first doc-test `ply-eval` has
> ever had, a `compile_fail` example that is the only way to assert a variance.
> `README.md`'s Status paragraph and `docs/ONBOARDING.md` §2 carry the same
> re-take and the list of files.

### There is CI, and it is younger than most of this file

> **This section read "There is no CI".** The withdrawn text, verbatim:
> *"`.github/` does not exist. Nothing runs on a push. The three commands above
> are the entire verification apparatus and they only run when a human runs
> them. Assume the person before you did not."* That was true until 2026-08-24,
> when `.github/workflows/ci.yml` was added.

`.github/workflows/ci.yml` runs on every push and every pull request, on every
branch and not only the default one. It runs the three commands above, and it
opens all five of the gates the next section describes — which is most of why it
exists, because a gate that returns a *passing* result when its dependency is
absent is the thing a human is least likely to notice. It **asserts** four of
them open by a notice the job greps for: `PLY_PG_URL`, `cluster::available()`,
`#![cfg(unix)]` and the spike each fail a job if the gate is shut. `PLY_TEST_DB`
has no notice — those 26 tests print nothing at all when they skip — so it is
asserted a different way, and the measurement behind it is in the gate table
below: a *wrong* value fails 20 of the 26 loudly, because the harness expects
the database to be reachable, and a *missing* value is caught by the job's
`test -n` pre-flight. What no one can show after the fact is a log line saying
the 26 ran.

| job | what it runs |
| --- | --- |
| `shard table is total` | `.github/ci-shards.sh verify`, before anything compiles |
| `cargo fmt --all --check` | the same command, and it must be silent |
| `cargo clippy --workspace --all-targets` | with `-D warnings`, so the first warning is a failed run rather than a line in a log |
| `test corpus` / `cli-eval` / `core` | the suite, three shards by package; the split is `.github/ci-shards.sh` |
| `test ply-host (postgres)` | `ply-host`, with `PLY_PG_URL` and `PLY_TEST_DB` pointed at a `postgres:18.6` service container **and** `initdb`/`postgres`/`psql` on `PATH` — then it fails if any test printed a skip notice |
| `wall-clock measurements` | the thirteen timing-sensitive tests, one at a time, single-threaded, alone on a runner |
| `crates/ply-codegen-spike` | `cargo test --locked --release` on a pinned 1.94.0, in the spike's own workspace |
| `examples/same-tests.sh` | W4's exit criterion: a release `ply`, a cluster the script starts itself, and the twin compared against postgres byte for byte |
| `CI` | one required check that fails unless every job above reported `success` |

Three things follow, and the second is the one this project would regret not
having straight.

**The suite is sharded, and the shard table is the thing to get right.** A
package in no shard is a package nothing tests, and the run would be green and
say nothing about it — this repository's most expensive defect class, in CI
form. So the partition is written once, in `.github/ci-shards.sh`, and the first
job reads the workspace members out of `Cargo.toml` and fails if a member is in
no shard, in two shards, or named in the table and absent from the tree. It also
fails on a crate directory under `crates/` that is in neither `members` nor the
script's `KNOWN_OUTSIDE` list, and on a `Cargo.toml` whose `members` list it
cannot parse — because a table that matched nothing would otherwise pass every
check it makes. Add a crate, and CI tells you where to put it.

**The gates are asserted open, not assumed open.** A service container that came
up is not evidence that a test connected to it. So the postgres job runs the ten
live tests with `--nocapture`, greps its own output for the notice a skipped
test prints, and fails if it finds one; it does the same for a cluster-gated
binary; and it fails unless the runs report exactly `10 passed` and `2 passed`.
The `--nocapture` matters and so does the shape of the grep: cargo captures a
passing test's output, so without it the notice appears **zero** times, and
libtest prints it on the same line as the test name — `test <name> ... skipped:
PLY_PG_URL is unset, …` — so a `^skipped:` anchored pattern never fires. The
first version of that grep in this workflow was anchored, and it was a guard
that could not fire; it was caught by running the job's own command with the
variable unset and watching it pass.

**CI is not a substitute for the outer loop, because it is not the same run.**
On your machine every gate below still skips exactly as it always did: nothing
local sets `PLY_PG_URL` or `PLY_TEST_DB`, and `cargo test --workspace` still
does not reach the spike. CI closing them means the gap is caught before a
merge, not that it is closed where you are standing. Run the outer loop anyway.

Two things about the shape, and one about how much to trust any of it.

The three shards are cut from a measurement, not a guess. `cargo test -p
<package>` on the machine in `docs/ONBOARDING.md` §Provenance, warm target,
`-j 2 -- --test-threads=2` to approximate a two-core hosted runner, 2026-08-24:
`ply-corpus` **289s**, `ply-cli` **149s**, `ply-eval` **137s**, `ply-store`
32s, `ply-hash` 15s, `ply-test` 13s, `ply-core` 8s, `ply-span` 5s, `ply-prove`
4s, `ply-syntax` 3s, `ply-std` 2s, `ply-derive` 1s — 658s summed, of which
`ply-corpus` is 44%. That one package is the floor: no arrangement of `-p` flags
finishes sooner than it does, so a fourth shard would buy nothing and pay the
dependency build again. The numbers are that machine's and the ordering is what
the table depends on; re-take with `cargo test -p <package>` if a package grows
a slow suite, and move it.

**And the part that is not checked. This is a ledger, not a blanket.** An
earlier draft of this paragraph said *"every command in `ci.yml` was run on that
machine before it was committed, and their exit codes are the evidence for
everything above."* It was withdrawn the same day it was written, because
`ci.yml` was edited after that run and the sentence outlived the thing it rested
on. What follows is what has an exit code against the file as it stands, taken
on the machine in `docs/ONBOARDING.md` §Provenance, 2026-08-24.

**Static checks, all exit 0.** `actionlint` 1.7.12 over `ci.yml` with its
`shellcheck` integration active; `python3 -c "yaml.safe_load(...)"`, which
parses and reports nine jobs; `shellcheck` over `.github/ci-shards.sh`; and
`ci-shards.sh verify`, which reports *13 workspace members, each in exactly one
shard; 1 crate deliberately outside; 13 deferred tests, each present in the
tree*. `verify`'s failure paths were exercised too, eight of them, each exiting
1 with a named reason: a member in no shard, a member in two, a shard naming a
non-member, a deferred test renamed, a deferred test in a missing target, a
`KNOWN_OUTSIDE` entry for a crate not in the tree, an unexcused crate directory
(`crates/zz-verify-probe/`, created and removed), and a `Cargo.toml` whose
`members` list does not parse — which would otherwise have made every check
above it pass vacuously. `ci-shards.sh packages nosuch` exits **1**.

**Commands the jobs run, with exit codes.**

| command | result |
| --- | --- |
| `cargo fmt --all --check` | **0**, silent |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | **0**, 34.36s, zero lines beginning `warning` or `error` |
| `cargo +1.94.0 test --locked --release` in `crates/ply-codegen-spike` | **0**, **45 tests** — `hazards.rs` 16, `mutations.rs` 11, `mcts_kernel.rs` 9, `spike.rs` 9 — 438.9s including the cold release build |
| `cargo build --locked --release -p ply-cli` | **0** |
| `./examples/same-tests.sh` | **0**, **29 requests byte-for-byte identical** between the twin and postgres, 5.63s, against a cluster the script started itself |
| the postgres job's live-test step | **0**, `10 passed` in **0.29s**, zero skip notices |
| the postgres job's cluster step | **0**, `2 passed` in **2.87s**, zero skip notices |
| the `test` job's `w5_shutdown` step | **0**, **8 passed**, guard matches |
| the `test` job's `corpus` shard, end to end | **0**, 733s, **196 tests ran, 0 failed** across 16 test binaries, **3 filtered** — exactly its three |
| the `test` job's `core` shard, end to end | **0**, 105s, **1521 tests ran, 0 failed** across 41 test binaries, **6 filtered** — exactly its six |
| the `test` job's `cli-eval` shard, end to end | **0**, 366s, **1669 tests ran, 0 failed** across 77 test binaries, **4 filtered** — exactly its four. Its first run, before the thirteenth entry existed, was `exit=101`; see below |
| the `test-timing` job, all **thirteen** deferred tests | **0** and `1 passed` for every one, including both tests that failed inside a shard — run alone and single-threaded they pass at load 20–28, which is the entire argument for the job |
| `cargo test -p ply-host --lib db::pool`, all three `PLY_TEST_DB` states | unset **26 passed** `0.00s`; set and reachable **26 passed** `0.94s`; set and unreachable **20 FAILED** `0.02s` `E0431` |
| the `test-postgres` job's `cargo test`, both variables set at a live server | **0**, **281 passed, 0 failed** across **9 targets** — 8 binaries and doc-tests — in 45s, and no skip notice anywhere in the log |

**The guards, exercised in both directions.** With `PLY_PG_URL` unset the live
step's command exits **0** and reports `10 passed` in `0.00s` — the green this
whole job exists to refuse — and the grep fires. Piping stdout alone puts **0**
notices in the log against **10** with `2>&1`, and running without `--nocapture`
puts **0** there either way. The shards' `--skip` list was shown to remove
exactly what it names: `ply-store --lib` goes from `102 passed` to `100 passed;
… 2 filtered out`. `cargo test -p ply-derive -- --exact ""` gives `0 passed; 24
filtered out` at exit **0**, while bare `--exact` with no filter gives `24
passed` — so the run-nothing hazard is the empty word and not the flag. And the
`ran -gt 0` backstop was run against a zero log (**exit 1**) and a real one
(`ran=153`, **exit 0**). The thirteenth entry was demonstrated the same way:
`ply-cli --test w3_http_audit` goes from `18 passed` to
`17 passed; … 1 filtered out` once the table names it.

**One measurement that was wrong in two documents at once.** `PLY_TEST_DB`'s
live-path timing was recorded as `1.55s` here and `1.35s` in `ROADMAP.md` for
what was described as the same run. Neither could be reproduced, because the
re-take had to be done against a server that was actually up — the first attempt
gave `20 FAILED` and the reason turned out to be that the scratch cluster had
been shut down hours earlier, which is itself the finding: *a set-but-unreachable
`PLY_TEST_DB` fails loudly*. All three states are now in the table above, and
`ROADMAP.md` and `docs/ONBOARDING.md` carry the same three with the old figures
quoted as withdrawn.

**Run, and it failed.** The corpus shard's command against the *seven*-entry
deferred table: **`rc=101`**, `measure::tests::every_resumption_costs_about_what_the_first_one_did`.
That failure is how the table went from seven to twelve; the cli-eval shard
later failed the same way and took it to thirteen. See §"The suite proves less
than it looks like it proves". Re-run against the current table, that package's lib
target reports **`150 passed; 3 filtered out`** where it previously reported
`152 passed; 1 failed` — 153 either way, and the three filtered are exactly the
three `ply-corpus` names the table adds.

**A figure that was doubted and turned out to be right.** The `281 pass` in the
gate table below had been flagged as the kind of number that gets typed rather
than taken. It was taken: the run above gives exactly **281 passed** across
exactly **9 targets**, counting doc-tests as the ninth. Suspicion of a
suspicious-looking figure is a reason to re-take it, not a finding on its own.

**What it cost to get the `corpus` row.** It was started three times. Two runs
were abandoned after the machine went from load 19 to load **52** with three
other lanes running `cargo test --workspace` on it — the second reached 8 of its
17 targets in about an hour, against 672s for the same shard earlier the same
day. Its slow target, `r4_value_construction`, is the one §"Where a change is
likely to bite" already names. Neither abandoned run was written down as a
result, in either direction: this file's own §"Gate on an idle machine before
measuring, not after" makes a wall-clock number from a load-52 machine worth
less than no number, and that cuts both ways. The third attempt, at a load the
machine could sustain, is the 733s row above. Inside it, the `--lib` target
reports `150 passed; 3 filtered out` where the seven-entry table gave
`152 passed; 1 failed` — 153 either way, and the flake gone.

**`cli-eval` failed first, and that is how the list became thirteen.** `exit=101`,
765s, 1652 tests ran, one failure:
`routing_a_path_of_escapes_costs_its_length_and_not_its_square`,
`crates/ply-cli/tests/w3_http_audit.rs:714` — *"four times the escapes cost
1655.9ms against 143.6ms for k, which is 11.5x"*, against `four <= one * 9.0`.
Run alone it passes three times out of three at load 20, so it is contention and
not a regression. Its own doc comment says the 9x threshold was chosen "so that
a slow or contended machine cannot make it red"; a contended machine made it red
at 11.5x, which is the measurement that matters and the reason the row exists.
With that row in the table the same shard is the `exit 0` above.

**All thirteen are accounted for across the shards**, which is the check that the
table and the runs agree: `corpus` filtered 3, `core` 6, `cli-eval` 4. 3 + 6 + 4
= 13, and no shard filtered a test belonging to another.

**So what is left.** Every command in `ci.yml` has now been run on this machine
with a real exit code, including all four shard invocations and all thirteen
timing tests. What has not been exercised is the runner itself.

**Never run anywhere.** **Nothing in `ci.yml` has ever run on GitHub Actions.**
The runner image, the service container, the `apt-get` fallback, the cache
action and the `ubuntu-24.04` `PATH` layout are exercised by a push and by
nothing else, and the first push is where they get tested. If the first run is
red, that is the expected place for it to be red.

### The suite proves less than it looks like it proves

Five gates skip silently or near-silently. `ROADMAP.md`'s preamble table
enumerates **four** of them — the fifth, `PLY_TEST_DB`, is in no document in
this repository older than 2026-08-24 — and `docs/ONBOARDING.md` §2 explains the
two that matter and carries the measurement for the fifth. The short version:

> **A fifth, found 2026-08-24 by the lane widening the compiled fragment.** The
> sentence above said four and there are five. `crates/ply-codegen-spike`
> declares its own `[workspace]` — deliberately, so that deferring M9 can delete
> it with `rm -r` (ADR 0016 §3.5) — and the consequence is that **every
> `--workspace` gate in this file skips it in complete silence**. It is not in
> `Cargo.toml`'s `members`, so `cargo test --workspace`, `cargo clippy
> --workspace --all-targets` and `cargo fmt --all --check` have never once
> looked at it.
>
> Both of the other two gates were red when this was checked, on merged `main`:
>
> * `cargo clippy --all-targets` inside the crate: **20 errors** — not warnings,
>   errors — all `not_unsafe_ptr_arg_deref`, on every `rt_*` helper. Thirteen
>   predate the widening. So "clippy is clean" has been true of the workspace and
>   never checked here.
> * `cargo fmt --check` inside the crate: **two files unformatted**,
>   `src/bin/mcts.rs` and `src/wrong.rs`, both from the item-12/13 work merged
>   earlier the same day.
>
> Both are fixed. The lesson is the one `PLY_PG_URL` already teaches and this is
> the second instance of it: a gate's green can be manufactured by not running,
> and nothing in the output distinguishes "passed" from "never looked".
>
> | if your change touches | you must also run |
> | --- | --- |
> | anything in `crates/ply-codegen-spike` | `cd crates/ply-codegen-spike && cargo +1.94.0 fmt --check && cargo +1.94.0 clippy --all-targets && cargo +1.94.0 test --release` — the toolchain is not optional, `cranelift 0.134.3` and `wasmtime 47.0.3` require rustc 1.94.0 and the default here is 1.93.1. And **never** pipe it: `cargo build 2>&1 \| tail` reports `tail`'s exit status, so a failed build reads as a successful one. |

The original four:

| if your change touches | you must also run |
| --- | --- |
| postgres, the pool, transaction scope | `PLY_PG_URL=postgres://localhost/postgres cargo test -p ply-host` (36–38s, 281 pass, 0 fail) — otherwise ten tests pass in 0.00s without running, and cargo captures the skip notice so nothing tells you: `skipped:` occurs zero times in a whole `cargo test --workspace` log |
| the pool | `PLY_TEST_DB='postgresql://ply@127.0.0.1:5432/ply_test?sslmode=disable' cargo test -p ply-host --lib db::pool`. **This is the worst of the five: 26 tests hide behind it and print *nothing* when it is unset** — not a skip line, not on stderr, nothing. Re-measured 2026-08-24 on the machine in `docs/ONBOARDING.md` §Provenance against a local postgres 18.3: unset gives `26 passed` in `0.00s`; set and reachable gives `26 passed` in `0.94s`; set and *unreachable* gives **`20 FAILED`** in `0.02s` with `E0431`, because `db/pool/tests.rs:41` does `.expect("the test database is reachable")`. So a wrong value is loud and only a missing one is silent — CI sets the variable, pre-flights it with `test -n` and a `psql SELECT 1`, and that combination does cover it. An earlier version of this row said the figure was `1.55s` while `ROADMAP.md` said `1.35s` for the same measurement; neither had been re-taken |
| shutdown, drain, signals | anything; but know that `crates/ply-cli/tests/w5_shutdown.rs` is `#![cfg(unix)]` and compiles to nothing off Unix. CI runs on `ubuntu-24.04`, so it is compiled there, and a step fails if that binary reports zero tests |
| the served request path or its cost | `./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`, and see §"Things known to be broken". **Name the two files, never `benches/*.json`.** `benches/` holds three since R3, `w6` merges what it is given field by field on a last-wins basis, and the glob expands alphabetically — so `ply-corpus w6 benches/*.json` renders the **pre-region** ladder, dated `2026-08-16`, with `1035 times and 0.124 MB` in its boxing lever, exactly as if R3 had not happened. Checked by running it. `benches/README.md` §"There are two ladders" says which file is which |
| `examples/desk.ply` or any host handler | `./examples/same-tests.sh` — build `--release` first, it does not build for you. CI runs it in a job of its own, so this one is now caught before a merge rather than only when you remember |

Also: `ply-eval/tests/region_arena_cost.rs::snapshot_cost_as_a_function_of_region_size`
asserts on a wall-clock ratio and runs by default. A busy machine can fail it.
Re-run on a quiet one before you believe it.

And one test is memory-heavy rather than slow:
`ply-eval/tests/equivalence_audit.rs::the_two_engines_and_a_backend_agree_however_many_frames_a_body_pends`
peaks around **4.2 GiB** in a debug build. It runs the tree-walker over a
program pending 1,011,700 frames, and the tree-walker spends kilobytes of native
stack per pending level. That is inherent, not sloppiness — the machine's frame
stack is the tree-walker's native stack reified one for one, so no cheaper
program crosses the million-frame ceiling this test exists to prove is gone. See
§"Things known to be broken" item 10 for the figures.

> **It is not the only one. There are thirteen, and two of them failed here.**
> Thirteen tests assert a *performance* figure — a ratio, or a nanosecond,
> microsecond or millisecond budget — rather than a result. Note how the count
> was arrived at, because it matters more than the number: **two surveys, each
> of which declared itself complete, and each of which was refuted by the next
> shard run within the hour.**
>
> Seven are in `crates/*/tests`: the named ratio above;
> `ply-eval/tests/fixture_open_cost.rs::a_seeded_fixture_opens_per_test_in_microseconds`
> (`< 2ms` per test); `ply-eval/tests/simulation.rs::a_long_sleep_is_a_jump`
> (`< 1s` for a test that must not wait); and all four tests in
> `ply-test/tests/region_fixture_cost.rs`, which compare measured nanosecond
> figures against each other and against absolute budgets.
>
> **Five are unit tests in `src/`, and the first survey missed all five** — it
> read `crates/*/tests` and stopped there. They are
> `ply-corpus`'s `measure::tests::every_resumption_costs_about_what_the_first_one_did`,
> `::capture_and_resume_are_flat_in_the_frames_they_move` and
> `::opening_a_fixture_beats_rebuilding_it_once_the_fixture_is_real`, and
> `ply-store`'s `tests::opening_a_ten_thousand_definition_cache_is_under_the_budget`
> and `tests::a_baseline_for_every_test_does_not_slow_the_open` — the last two
> being `elapsed < 250ms` in debug and `< 5ms` in release.
>
> **How they were found is the point.** Not by a better grep: by running the
> corpus shard's own `cargo test` command on this machine while three other
> `cargo test --workspace` runs were on it, and watching
> `every_resumption_costs_about_what_the_first_one_did` fail —
> *"the fourth resumption cost 5680.8965 us against 2196.552 us for a whole
> one-resumption call"*, `crates/ply-corpus/src/measure.rs:861`, against an
> assertion of `four.marginal_micros < one.micros * 2.0`. That is exactly the
> flake a hosted runner produces, and the survey that was supposed to have
> caught it had already been written down as complete. It was not.
>
> **The thirteenth was missed by the second survey too, and differently.**
> `crates/ply-cli/tests/w3_http_audit.rs::routing_a_path_of_escapes_costs_its_length_and_not_its_square`
> reads no Rust clock at all — it parses milliseconds out of `ply test`'s own
> output through a local `duration_of` helper — so no timing vocabulary appears
> in it and no grep for one will ever find it. The cli-eval shard found it, at
> `exit=101`.
>
> All thirteen are listed once, in `.github/ci-shards.sh`'s `DEFERRED` table, and
> CI runs them in a job of their own — one at a time, single-threaded, alone on
> a runner — while the parallel shards `--skip` them by name.
> `ci-shards.sh verify` fails if a name in that table no longer exists in the
> tree, so the two halves cannot drift apart silently.
>
> **Three more read a clock and were deliberately left where they are**, because
> what they assert is a *deadline* the code under test is supposed to honour
> rather than a speed: `w5_drain_audit.rs`'s `elapsed < 8s` and its
> `200ms + 5s + 1s` teardown bound — whose own comment says the last second is
> "slack for a loaded machine" — and `db_transaction_audit.rs`'s
> `waited >= 250ms && waited < 5s`. Moving those to a serial job would not make
> them more true; they are second-scale bounds on configured timeouts, and they
> live in `ply-host`, which already has a runner to itself.
>
> **Stop surveying; run the shards.** That is the actual lesson of the two
> misses. The second pass keyed on a timing vocabulary — `Instant`, `elapsed`,
> `Duration`, `_nanos`, `_micros` — and the thirteenth test contains none of
> those words; a third pass would have its own blind spot and would also feel
> complete. `crates/ply-codegen-spike` has still never been surveyed at all.
> **Treat thirteen as the current count, not the answer**: when a shard goes red
> on a ratio or a budget, the fix is usually another row in `DEFERRED`, and that
> is a normal maintenance event rather than a sign something was done wrong.
> And a runner of one's own is not a quiet machine: a hosted runner is two
> shared cores, so this reduces the noise rather than removing it.

## Before you open a change

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`,
   `cargo test --workspace` — all three, all clean.
2. If you changed behaviour, there is a test asserting the new behaviour, named
   as an English sentence. See §Style.
3. If you changed a *guarantee*, you found and updated the document that states
   it. `grep -rn` the guarantee's words across `README.md`, `DESIGN.md`,
   `ROADMAP.md`, `CONTRACTS.md` and `docs/adr/`. Roughly 24,500 lines of prose
   and **one sentence** of it is read by a test — `README.md`'s request-path
   allocation count, by
   `w6_report_allocations::the_readme_still_describes_this_request_path`, added
   after that sentence went stale twice. For the other 24,500 lines you are the
   only check. (Re-take the figure rather than quoting it;
   `docs/ONBOARDING.md` §7 gives the command.)
4. If you changed something the shipped measurement files describe, re-take
   them. The command is in `docs/adr/0016-w6-performance.md` §"Provenance"; the
   three tests that will fail otherwise are
   `w6_report_integrity::the_shipped_ladder_still_describes_the_tree_it_ships_in`,
   `w6_report_allocations::the_shipped_allocation_evidence_still_describes_this_request_path`
   and — if you moved what a request allocates at all —
   `w6_report_allocations::the_readme_still_describes_this_request_path`, whose
   band is 1% rather than a factor of two.
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

### Gate on an idle machine before measuring, not after

A wall-clock measurement on this machine is worthless above about load 4, and
the way to find that out is not to take forty windows and discard thirty by rule.
R4 §1 spent three sittings doing exactly that. Windows above load 30 spread
0.497–1.607 on a ratio whose true value was about 1.000.

Two cheap habits, the second borrowed from a peer session measuring an unrelated
crate on the same machine and offered explicitly:

- **Spin until the machine is quiet, then measure.** A loop on
  `top proc < 60% CPU && load < 4` costs nothing and replaces a sitting's worth
  of rounds you were going to throw away.
- **Check the instrument, not just the result.** Require the final re-run to
  land within N% of that session's *first* measurement. A run whose own repeat
  disagrees with it has told you the instrument drifted, and no amount of
  averaging fixes that. R4 lacked this check and would have caught its own drift
  sooner with it.

**Pre-register the filter.** Write the load threshold, the statistic and the
decision rule down before any data exists. R4 ended with a cut that cleared its
2% criterion and had to discard it, because the threshold was chosen after
seeing the numbers — the honest row was the underpowered pre-registered one whose
confidence interval straddled the bar. "Re-run until it passes" is not a
protocol.

**And know which of your numbers this even applies to.** Allocation counts,
hashes, interleaving counts and seeded replays are deterministic: they reproduce
to the digit on a burning machine, and three agents in three worktrees confirmed
R4's to the digit at load 40. Only wall clock is at risk. If the deterministic
half carries your claim, an unresolved timing number is a caveat in a harmless
place — say `UNMEASURED` and check in the raw windows so a reader can re-cut
them. That is a better artifact than a number of unknown provenance sitting
where the hole was.

### Say how it was checked, or say it was not

Every number gets a provenance: the machine, the profile, the command, and
whether it is one run or a best-of-N. Where something was **not** measured, write
"not measured" — `README.md` §"Where this is not competitive" does this
throughout and it is the most trustworthy section in the repository.

Never quote a figure from another document. Re-take it or cite the file that
holds it (`benches/w6-ladder.json`, `benches/w6-spike.json`) and the command
that renders it.

### Disclosing a gap does not close it

**Writing "this might have missed X" discharges the honesty obligation and not
the work obligation, and the two feel identical from the inside.** The disclosure
is the part that reads as diligence — it is candid, it is in the reader's
interest, and it is the thing the section above asks for. It is also the point at
which the gap stops being uncomfortable, which is exactly when it stops getting
closed. If X is one grep away, the caveat is not the finish line. Go and grep.

This is a different failure from the seven at the top of this file. Those were
claims that were false. This one is a claim that was *true* — the caveat
described the hole accurately — and was still the wrong thing to have stopped
at. That is why it needs its own rule: nothing in "correct, do not delete" or
"say how it was checked" catches it, because both were obeyed.

The worked example is this file, on 2026-08-24. The correction it produced is in
§"The suite proves less than it looks like it proves".

The `test-timing` job exists because some tests assert a wall clock and a shared
runner fails them at random. The list of those tests was built by grepping
`crates/*/tests` for `Instant::now`. That is a real method with a real hole in
it, and the hole was written down, here, in these words: *"It is not a survey. It
came from one pass over `Instant::now` in `crates/*/tests`, so a wall-clock
assertion inside a `#[test]` in `src/` would not have been found by it."* Seven
tests went into the table. The caveat went into the documentation. Both were
true. Nothing else happened.

The next thing that ran was the corpus shard's own command, on a machine with
three other `cargo test --workspace` runs on it, and it failed:

```
crates/ply-corpus/src/measure.rs:861
measure::tests::every_resumption_costs_about_what_the_first_one_did
the fourth resumption cost 5680.8965 us against 2196.552 us for a whole
one-resumption call
```

A wall-clock assertion inside a `#[test]` in `src/`. The exact category the
caveat had named, in the safety device built to catch that category, found by
running the thing rather than by reading it. A second pass over `crates/*/src`
took minutes and turned seven into twelve.

**Then it happened again, to the corrected version.** The second pass came with
its own caveat — that it keyed on a timing vocabulary and would miss a test that
measured without those words. The next shard run failed on
`routing_a_path_of_escapes_costs_its_length_and_not_its_square`, which measures
by parsing `ply test`'s own output and contains none of those words. Twelve
became thirteen. The rule below is not "survey harder"; it is that a list of
this kind is maintained by the thing that exercises it, and a survey is at best
a way to seed it.

So the rule, and it is narrower and more demanding than "say how it was
checked":

- **A caveat that names a specific, cheap check is a to-do, not a disclaimer.**
  "I did not look in `src/`" is a task. "I could not get a quiet machine" is a
  disclaimer. Only the second one is finished when you write it down.
- **State limits as a floor, not as an apology.** "Twelve is a lower bound: the
  pass keyed on `Instant`, `elapsed`, `Duration`, `_nanos`, `_micros`, so a test
  that measures without those words is still missable, and
  `crates/ply-codegen-spike` was not surveyed at all" is a caveat the next
  person can act on. "This is not a survey" is one they cannot. The difference
  is whether the reader is told *where to look next*.
- **Prefer the check that can fail to the sentence that cannot.**
  `ci-shards.sh verify` resolves every name in the table against the tree and
  exits non-zero if one is missing. A list that cannot prove its own entries
  exist is a list that rots silently, and a paragraph explaining that it might
  rot is not a substitute for the exit code. The same pass found the reason
  this matters: `cmd_deferred` split `package:target:test` on the *last* colon,
  so a unit test named `measure::tests::foo` yielded `foo`, and the shard's
  `--skip foo` under `--exact` matched **nothing**. A skip list that silently
  skips nothing, protecting a test that then flakes, inside the mechanism whose
  only job is to stop flakes reaching a shard — a defect in a safety device is
  invisible precisely because the device reports success either way.

**A second worked example, one level in, and it is not mine.** The corrected
survey came with a corrected caveat — a floor, naming the vocabulary it keyed on
and what could still hide from it. The next shard run was refuted by something
sitting in exactly that blind spot:
`crates/ply-cli/tests/w3_http_audit.rs::routing_a_path_of_escapes_costs_its_length_and_not_its_square`,
which asserts a wall-clock ratio while touching no Rust clock at all — it parses
milliseconds out of `ply test`'s own output. **No vocabulary-based survey can
find that class**, which is why the `DEFERRED` table is now maintained by running
the shards rather than by grepping the tree, and why a new row in it is normal
maintenance rather than evidence that someone was sloppy.

Now read that test's own doc comment:

> *"The threshold is deliberately loose — quadratic is 16x and this refuses at
> 9x — so that a slow or contended machine cannot make it red while a
> re-introduced `push` accumulator cannot make it green."*

A contended machine made it red, at **11.5x**. The tolerance was reasoned and
never taken. This is the repository's signature defect appearing *inside a test
written to guard against a different one*, in a sentence that reads as though
the measurement had been done — and it is the same failure as the `src/` caveat
above, one level further in: a claim about robustness, argued rather than
measured, sitting in the artifact whose entire job was robustness. When you
write "this cannot fail under X", X is a thing to go and do, not a thing to
reason about.

### Measure an ADR's motivating claim before accepting the ADR

If an ADR is motivated by a performance argument, that argument needs a
measurement **before** the ADR is accepted — not after the milestone ships.

ADR 0017 is the worked example, and it is the most expensive instance in the
repository. It opened by asserting that the persistent forkable world and the
zero-cost path were "mutually exclusive", reasoning that Perceus-style in-place
update fires only on uniquely-owned values and that forking keeps reference
counts high. It concluded that removing the world was therefore *forced*.

Every sentence of that was reasoned. None of it was measured. R1 and R2 removed
the world across two milestones, and allocations per `/health` went **up**, from
1,035 to 1,122. A later attribution run (`cargo test -p ply-corpus --release
--test w6_alloc_sites -- --nocapture`) showed why: the allocations were never in
the world. They were in `frame::dispatch` (24%), in `code::lower_*` running on
the request path (~24%), and — after R2 — in `region_kind::infer` running at
runtime (~12%). One profiling run before the ADR would have caught it.

**R3 finished the example, and the ending is the part worth learning from.** It
hoisted both compile-time analyses off the request path and they are now at
**0.0 allocations per request** apiece, measurable by the command above. The
figure came back to **1,082** — and 1,082 is still above 1,035, so a milestone
run to a decision rule fixed in advance handed back the answer *the design does
not look justified on this route*. `ROADMAP.md` §R3 records it.

Two lessons, and the second is the expensive one:

- The rule existed before the number, which is the only reason the answer could
  be reported instead of argued with. `ply_corpus::w6::Criteria::default()` is
  the same idea in code.
- **One of the two things R3 was scoped to remove may never have been on the
  request path at all.** The `~24%` for `code::lower_*` above was read off a
  20-request window, and one-time work divided by twenty looks exactly like that;
  the same family reads 33.8% of a 20-request window today while costing nothing
  per request. If you take an attribution, take it at two windows and fit a
  slope — `w6_alloc_sites.rs` does, and its header says why. A ranking is not a
  cost.

Note what *was* measured. R1 measured the isolation cost, the one number that
could argue against the design, and it came back zero before six agents built on
it. That was the right instinct pointed at the wrong claim: the isolation cost
was a tiebreaker, and the premise was load-bearing for the entire milestone.

So: **the claim that motivates the work is the one to measure first**, not the
claim most likely to object to it. If you cannot measure the premise, say in the
ADR that it is unmeasured and what would test it, and do not write "forced".

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
- ~~ADR 0016's spike figures cannot be re-taken because the spike does not
  compile.~~ Stale twice over: R4 repaired the crate, and CI's `spike` job now
  builds and tests it on every push. See §"Things known to be broken" item 1,
  which has carried the correction since 2026-08-21.

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

`docs/adr/` is **nineteen** files, `00NN-slug.md`, with **no index** — the
numbers are the ordering. Number yours `0020` and up. (This read "seventeen" and
"`0018` and up"; 0018 and 0019 have since been written, and nothing counts the
directory for you — `ls docs/adr/*.md | wc -l`.) ADR 0005 is superseded in part by
ADR 0017, and it is the model for how to record that: 0005's *title* does not say
so, but its header does — lines 3–9 carry `Status: accepted — … §2's persistent
forkable world is **superseded by ADR 0017**` and `Superseded in part by:
docs/adr/0017-regions.md (§2 only)`, and §2 itself opens with a
`> **Superseded by ADR 0017.**` block at line 289. Do that in both files.

An ADR here is expected to state the criteria *before* the measurement, in code
where possible. `ply_corpus::w6::Criteria::default()` is the model: eight
thresholds that a measurement file cannot supply, so a number cannot set the bar
it is about to clear. That is why the M9 deferral is re-derivable
(`ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`) rather than
re-arguable. Name the files; see the warning in the gate table above.

## Where a change is likely to bite

| you changed | what breaks quietly |
| --- | --- |
| `crates/ply-hash/src/normalize.rs` | **every cached result everywhere.** The bytes are the identity. A change here is a cache-format change; see `CACHE_VERSION_CHANGED` (`W0603`). |
| `crates/ply-core/src/ty.rs` `conflicts_with` | test scheduling, silently — tests still pass, they just stop running concurrently, or start racing |
| `crates/ply-test/src/schedule.rs` `group_by_conflict` (`:216`) | same; `parallelism()` at `:172` is what reports it |
| `crates/ply-eval/src/code.rs` | `crates/ply-codegen-spike`, which **nothing in the workspace compiles**. It has now bit-rotted this way twice — `Stmt::Expr` becoming a struct variant, then `NodeKind::Lit` widening to `Lit(Lit, Value)` under R4. It builds today: `cd crates/ply-codegen-spike && cargo +1.94.0 test --release`. Run that after touching this file, or the only instrument for pricing codegen stops answering. CI's `spike` job runs exactly that command, so a break is caught at the pull request rather than at the next re-take |
| how a `Value` is built or shared | `crates/ply-corpus/tests/r4_value_construction.rs`, the attribution ADR 0019's thresholds are fractions of. Two traps: it is **about three times slower in debug than release** (70.9s against 25.6s) because it captures a backtrace per allocation, and its rule table is matched against a **three-frame window whose contents differ by profile** — a rule verified only in release can leave the same allocation unattributed in debug and fail the residue ceiling there. Check both. ADR 0019 §6 is the worked example |
| the request path | `benches/w6-ladder.json` and the two integrity tests, and the M9 verdict that reads it. Also `README.md`'s one guarded sentence — re-take it with `./target/release/w6-alloc --repo . --requests 200`, which reads **773.4** on this tree |
| `Value::cmp`, `values_equal`, or how a `Map` key is stored | the four guarantees the note on `ply_eval::Map` lists. `cmp` is deliberately **coarser** than rendering at `Decimal` (`1.50m` and `1.5m` are one key and two strings), so a key is reduced to one representative per class by `ply_eval::value::canonical_key` before it is stored — `ply_eval::value::insert_key` is the single site, and adding a second one re-opens a defect that made `map_keys` a function of insertion history for four milestones. Any new coarseness in `cmp` needs a matching arm there. `map_order.rs`, `value_semantics_audit.rs` §5 and `derivation_determinism_audit::a_decimal_keyed_map_encodes_one_body_whichever_spelling_was_written_last` are what fail; `docs/adr/0019-value-representation.md` §7 is the write-up |
| any public signature | `CONTRACTS.md`, which no test reads |
| `examples/desk.ply` | `examples/serve.sh`, whose `rewrite()` (`serve.sh:103-112`) matches exact source lines with `grep -qF` and aborts loudly if one is missing — that abort is deliberate and is the good case. How many lines it rewrites depends on the mode: `--memory` rewrites two (`:122` and `:125`), `--tls` rewrites one (`:127`), and a plain `--db` run rewrites none |

## Things known to be broken

Recorded here so nobody spends an afternoon rediscovering them.

1. ~~**`crates/ply-codegen-spike` does not compile.**~~ **Half fixed, and the
   half that is left is the one that bit it.** The second wall is gone: R4
   repaired the `E0164`s (`ply_eval::code::Stmt::Expr` is a struct variant) and
   then widened `NodeKind::Lit` under it and repaired that too, so the crate
   builds and its tests run. Re-taken 2026-08-24, from
   `crates/ply-codegen-spike/`:

   ```
   $ cargo +1.94.0 test --release
       Finished `release` profile [optimized] target(s) in 1m 22s
   test result: ok. 16 passed; 0 failed; ...     # tests/hazards.rs
   test result: ok.  9 passed; 0 failed; ...     # tests/mcts_kernel.rs
   test result: ok. 11 passed; 0 failed; ...     # tests/mutations.rs
   test result: ok.  9 passed; 0 failed; ...     # tests/spike.rs
   ```

   **45 tests across 8 targets, 0 failed.** This block read `8 passed #
   tests/spike.rs` and `7 passed # tests/mcts_kernel.rs` and listed no others,
   which was the 2026-08-21 reading; R5 then rewrote most of the crate and added
   `tests/hazards.rs` and `tests/mutations.rs` without re-taking it here. Nothing
   was wrong with the crate — the figures had simply stopped describing it, which
   is the failure mode §"The one rule" is about and the reason the `spike` CI job
   now runs this command on every push.

   The **first** wall stands and is not a defect to fix: cranelift 0.134.3
   requires rustc ≥ 1.94.0 and nothing in this repository pins a toolchain, so
   `cargo test` on the default `stable` still refuses. `+1.94.0` is the
   invocation, and every command in `benches/README.md` §"What `mcts` adds"
   carries it.

   Why it rotted is unchanged and the consequence is not: the crate declares its
   own `[workspace]`, so `cargo build --workspace`, `cargo test --workspace` and
   `cargo clippy --workspace --all-targets` still do not touch it — but since
   2026-08-24 something does. `.github/workflows/ci.yml`'s `spike` job runs
   `cargo test --release` there on 1.94.0, on every push, so the next change to
   `ply_eval::code` that breaks it fails a required job instead of being found
   two milestones later. Locally it is still on you to run it. It is
   also not clippy-clean and never has been — `cargo +1.94.0 clippy
   --all-targets` there reports 13 `not_unsafe_ptr_arg_deref` errors, all in
   `src/rt.rs`, which is the JIT's calling convention, plus 6 warnings; the
   project's stated gate is `--workspace` and does not reach them.

   Consequence, revised: ADR 0016's `11.67x` **is** re-takeable now, and R4 took
   it — `benches/w6-spike-r4.json`, at `11.68×` by the same expression, so the
   `read_line` half did not move. `ROADMAP.md` §"What is next" item 3 is
   unblocked and ADR 0018 §1 is discharged; what item 3 asked for is in
   `benches/adr0018-mcts.json` and `docs/adr/0019-value-representation.md` §5.
   ADR 0016 records the toolchain wall at lines 764–767 and 1105–1106 and is
   otherwise unamended.
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
6. ~~**`PLY_PG_URL` is set by nothing in the repository**, so ten postgres
   tests pass without running by default.~~ **Half fixed.**
   `.github/workflows/ci.yml` sets it, at a service container, and fails the run
   if any of the ten prints its skip notice — so the gap is caught before a
   merge. Nothing sets it *locally*, so on your machine the ten still pass in
   0.00s without running. And a **fifth** gate of the same shape was found while
   wiring that job: `crates/ply-host/src/db/pool/tests.rs:25` hides 26 pool
   tests behind `PLY_TEST_DB`, prints nothing whatsoever when it is unset, and
   is named in no document in this repository. `docs/ONBOARDING.md` §2 has the
   measurement — 26 passed in 0.00s without it, 26 passed in 0.94s with it, and
   **20 of 26 failed** when it is set at a server that is not there.
7. **No `LICENSE` or `LICENSE-APACHE` file exists** although `README.md:499` and
   the workspace root `Cargo.toml:22` declare `MIT OR Apache-2.0`. The thirteen
   member crates inherit it with `license.workspace = true` rather than each
   carrying the SPDX expression, and `crates/ply-codegen-spike/Cargo.toml`
   declares no license at all.
8. **`w6-alloc`'s `bytes_per_request` grows with the window and nobody knows
   why.** `./target/release/w6-alloc --repo . --requests N` reports 127,954
   bytes at N=200, 177,236 at 400 and 277,417 at 800 — total bytes growing
   faster than the request count — while `allocations_per_request` falls with N
   exactly as a per-request slope plus a per-`Machine` intercept must. Something
   on that path is superlinear in the number of connections in one script.
   Consequence: the **allocation count** is the sound half of that output and the
   **byte count** may only be compared at the window a baseline was taken at,
   which for every published figure is 200. Found by R3 while re-taking the
   figure; not diagnosed, and `docs/adr/0017-regions.md` §"What must be measured"
   ¶1 says so in place.

9. ~~**The compiled-entry seam carries one of the machine's two resource
   bounds, so a backend answers where the machine raises.**~~ **Fixed
   2026-08-24, together with item 10 — they were one defect.** The entry below
   is left as it was written, and the fix is at the end of item 10. Found by an
   R5 review with
   the real cranelift backend, no mutation, one entry and zero declines.
   `Machine::compiled_answer` computes `budget = max_calls - stack.calls()` and
   that is the only bound `Compiled::enter(name, args, budget)` receives. The
   machine has a second: `DEFAULT_MAX_FRAMES = 1_000_000`, enforced in `push`. A
   compiled body pushes **one** `Frame::Call` for the whole call, where the
   interpreter also pends a frame per pending operand, and nothing at the
   boundary can express that — no backend can honour a bound it is not handed.

   ```
   pub fn hog(n: Int) -> Int =
     if n == 0 { 0 } else { hog(n - 1) + 1 + 1 + ... }     // 150 "+ 1"
   ```

   `probe.hog` is accepted whole by the fragment. `hog(9000)`:

   ```
   machine alone:   Err("recursion limit of 1000000 pending frames exceeded")
   machine + spike: Ok(1350000)          1 entry, 0 declines
   ```

   `ply_eval::compare_answers` calls that a `Divergence` on the verdict axis.
   Reproduced inside `ply-eval` alone with a hand-built honest backend built as
   `for_program(..).with_max_calls(budget)`, so it provably cannot outrun its
   budget, and with `Machine::with_max_frames(64)` and **no recursion at all** —
   a body of 300 chained `+ 1`s, which pends 300 frames and calls nothing. The doc on
   `DEFAULT_MAX_FRAMES` and the module doc on `limit.rs` both used to assert the
   opposite and both now carry the correction; `compiled.rs`'s "Recursion"
   bullet is narrowed. **Not fixed**, and the fix is not obvious: a frame budget
   is meaningless to a native body, so the options are charging an entry an
   estimated frame cost, dropping `DEFAULT_MAX_FRAMES` so both engines share one
   bound, or refusing to enter when `max_frames - frames()` is small relative to
   `budget` — and all three change shipping semantics.
10. ~~**The two engines disagree on the recursion bound for any body pending
    100 or more frames per call, with no backend involved.**~~ **Fixed
    2026-08-24; the close is at the end of this entry.** This is not R5's defect;
    R5's review found it while probing item 9, and it is older than the seam.
    `DEFAULT_MAX_FRAMES / DEFAULT_MAX_CALLS` = 1,000,000 / 10,000 = 100, and the
    tree-walker has no frame bound at all, so it passes where the machine raises.
    Item 9's program plus `test "..." { assert_eq(hog(9000), 1350000) }`, with
    the shipping `target/release/ply` and no backend, re-taken 2026-08-22:

    ```
    $ ply test hog.ply --engine both
      PANIC a recursion whose body pends 150 frames a level
        no culprit: the interpreter failed rather than the program; this is a
        defect in Ply
        `treewalk` and `machine` disagree
        = treewalk: passed
        = machine: [E0502] recursion limit of 1000000 pending frames exceeded
    ```

    Measured crossover at depth 9,990 with `--engine machine`: k = 90 passes,
    k = 100 raises. Consequence for the suite:
    `crates/ply-eval/tests/equivalence_audit.rs::the_two_engines_agree_on_the_recursion_bound`
    holds only below that ratio — both of its programs pend two frames a level —
    and its name and doc claimed the general statement. The doc is narrowed in
    place; the test is **not** changed to assert the divergence, so **nothing in
    the suite arms the true bound**.

    > **Closed, 9 and 10 together (2026-08-24).** The machine has no default
    > frame ceiling any more, so `DEFAULT_MAX_CALLS` is the only bound on what a
    > program may do — the one the tree-walker counts as its own nesting, the
    > machine counts as `Frame::Call`s, and `Compiled::enter` is handed as
    > `budget`. `pub const DEFAULT_MAX_FRAMES` is deleted;
    > `Machine::with_max_frames` remains as an opt-in resource ceiling that is
    > **not** semantics, does not say "recursion limit", and withdraws the
    > compiled seam's offer entirely while it is set, because a native body pends
    > no frames and could not honour it.
    >
    > **Why removal and not one of the three fixes item 9 lists.** The ceiling
    > was a function of **spelling**, not of behaviour. Two definitions of the
    > same function `hog(n) = 150n`, the same 9,001 nested calls, shipping
    > release binary, 2026-08-24:
    >
    > ```
    > $ ply test spell_a.ply --engine machine --no-cache   # hog(n - 1) + 150
    >    ok    one addition of 150
    > $ ply test spell_b.ply --engine machine --no-cache   # hog(n - 1) + 1 + 1 + ...
    >    FAIL  one hundred fifty additions of 1
    >      recursion limit of 1000000 pending frames exceeded
    > ```
    >
    > Copying the ceiling into the tree-walker would have made *both* engines
    > refuse the right-hand program over how its additions were written, and
    > would still have left a backend answering where both raised — item 9 is
    > structurally unfixable while the ceiling is semantics, because a native
    > body has no frames and any charge is an estimate, and an estimate that
    > differs from the interpreter's exact count *is* the divergence. The only
    > conservative charge, `budget × the body's static pend`, declines every
    > recursive entry the seam exists for.
    >
    > **What the ceiling was protecting, measured.** Nothing the product does not
    > already spend. Peak RSS, `/usr/bin/time -l`, debug, one process per figure,
    > same program and same 1,350,000 pending levels on both sides: the
    > **machine** holds them in **194 MiB**, about 151 bytes a frame; the
    > **tree-walker** holds them in **5,365 MiB**, about 4.2 KiB a level, and
    > reports `passed`. The engine carrying the guard was the one spending about
    > a 28th as much of the resource it guarded.
    >
    > Pending frames are also not unbounded without calls, which is what would
    > have made removal reckless. Measured, not reasoned: `fold` over a list
    > needs exactly the frames the list's builder needs and `map` exactly one
    > more, whatever the length — peak frames 205/405/805 for the builder alone
    > and for `fold`, 206/406/806 for `map`, at lengths 200/400/800 — so a
    > builtin loop is O(1) in frames; and a continuation splice carries its calls
    > with its frames, staying at `5 × calls − 1` across three sizes (54/11,
    > 104/21, 204/41). What bounds the heap is `DEFAULT_MAX_CALLS` times how much
    > a body pends, and the tree-walker has always been held to exactly that and
    > no more.
    >
    > **Armed by**
    > `crates/ply-eval/tests/equivalence_audit.rs::the_two_engines_and_a_backend_agree_however_many_frames_a_body_pends`
    > — tree-walker against the plain machine, then the plain machine against a
    > machine with a budget-honest backend, on a body pending 150 frames a call
    > at depth 6,700 (1,011,700 pending frames), plus R5's crossover at depth
    > 9,990 for k = 90, 100 and 150 — and by
    > `a_machine_asked_for_a_frame_ceiling_offers_nothing_to_a_backend`.
    >
    > **The first was confirmed to fail with the change reverted, and the
    > provenance is not the usual one.** The revert was taken by the
    > orchestrating session, not by the lane that wrote the fix, in an `rsync`
    > copy of this worktree excluding `target/` and `.git`, with
    > `CARGO_INCREMENTAL=0`, setting `max_frames: None` back to
    > `Some(1_000_000)`. It went red on the verdict axis, which is the right one:
    >
    > ```
    > treewalk vs machine: 1 compared, 1 footprints, 0 machine-only, 1 divergences
    >   a recursion whose body pends 150 frames a level: verdict —
    >   left passed, right [E0502] this engine's ceiling of 1000000 pending frames was reached
    > ```
    >
    > It was taken that way because the lane could not hold its own tree still
    > long enough to take it: `crates/ply-eval/src/machine.rs` returned four
    > different sha256 values across that session with no write by the lane
    > between them, once changing inside a single `sha → touch → cargo build →
    > sha` command, and one `grep` reported `max_frames: Some(1_000_000)` and no
    > seam gate ninety seconds before a `sed` on the same path reported
    > `max_frames: None` and the gate present in full. A revert-verification run
    > on a tree that will not hold still is worth nothing, so it was refused
    > there and taken somewhere it could be trusted.
    >
    > **The second test's revert behaviour is confirmed too, and by where its
    > failure landed rather than by its own build.** Reverting both halves at
    > once — the `None` default and the seam gate — turned
    > `a_machine_asked_for_a_frame_ceiling_offers_nothing_to_a_backend` red at
    > its *final* assertion, `entries()` = 1 against 0. That is the gate's
    > assertion, and the failure reaching it is what attributes it: the test
    > opens with a control leg requiring a machine nobody capped to reach the
    > seam at all, and with the gate present and only the default reverted it is
    > the control that goes red instead. So each half of the change is armed, at
    > a different assertion of the same test. A gate-only revert was not run as
    > its own build.
    >
    > **The left-hand side has to be the plain machine**: comparing the
    > tree-walker against a machine that has a backend attached can pass with the
    > ceiling restored, because a backend answers at the first level shallow
    > enough to fit — that masking is item 9 itself, and it must not be what
    > stands in for the comparison.
    >
    > The first test also proves it is not vacuous **without** a source edit: it
    > hands the same program to the same machine under `with_max_frames(1_000_000)`
    > and requires it to raise. A witness too small to have reached the old
    > default would sail through every leg above it, and that assertion is what
    > says this one is not.
    >
    > **Cost, because it is inherent and someone will want to cut it.** That test
    > peaks at **4,858 MiB** and runs 15.5s in a debug build — `/usr/bin/time -l`
    > on the test binary, `--test-threads=1 --exact`, 2026-08-24. The machine's
    > frame stack is the tree-walker's native stack reified, one frame per level,
    > so any program pending a million machine frames nests a million native
    > levels at kilobytes each — 1,529 MiB at 304,000 levels, 3,054 MiB at
    > 608,000 and 5,036 MiB at 1,003,200, which is 5,274, 5,267 and 5,264 bytes a
    > level, and 5,365 MiB at 1,350,000, which is 4,167 and so does not sit on
    > that line. There is no cheaper witness that crosses a ceiling of a million,
    > so the tree-walker is run **once** and every other leg is a machine.
11. **A definition that discharges its own effects publishes an empty row, so
    the compiled seam's purity gate clears it and the machine offers it.** Latent
    rather than live, and reported because the design claims otherwise.
    `crates/ply-codegen-spike/tests/fixtures/hazards/effects.ply`'s `handled`
    performs two operations and handles them under its own `handle`, inside its
    own `with_cell`; it is declared `-> Int` with no row, and both `footprint`
    and `performed` come back empty. A probe over the existing `Harness`/`Mutant`
    plumbing had it offered and answered with the value the interpreter produces,
    and the corpus reported
    `effects.handled: observed footprint — left {effects.tally.read[log],
    effects.tally.write[log]}, right {}` — character for character the evidence
    R5's mutation table reports for *deleting* the purity gate from shipping
    code. The gate is in place; it does not apply. Nothing stops it today except
    that the only backend in the tree refuses `handle` at compile time, which is
    a backend remembering an invariant the seam claims to enforce structurally.
    No published row can close it: `handled` carries no fact distinguishing it
    from a genuinely pure definition. It also reaches further than the seam —
    `ply-test`'s `report.rs` emits an `observed_footprint` and `slice.rs` reads a
    declared-but-unobserved atom as "a branch was not taken", so an entered
    definition would tell a user a branch was not taken when it was.
    `compiled.rs`'s "Effects" bullet is narrowed in place.
12. ~~**Every entry into the spike's backend costs O(the *previous* entry's peak
    arena).**~~ **Fixed (2026-08-24), and re-measured the way it was found.**
    `Ctx::begin` no longer touches the arena: `Ctx::end` clears it at the end of
    the entry that filled it, so an entry pays for its own work and its successor
    pays for nothing. Re-taken with
    `mcts --dir benches/kernel --carryover mcts.playouts --repeats 7`, a mode
    added for this so the curve is re-takeable by a command rather than by a
    reviewer, two runs per arm on one tree at load 14–17:

    | | 4 slots | 384 | 3824 | 19,584 | spread |
    | --- | --- | --- | --- | --- | --- |
    | before | 0.375 µs | 1.709 | 13.667 | 67.833 | **180.888x** |
    | before, again | 0.458 | 1.917 | 14.875 | 68.125 | **181.667x** |
    | after | 0.416 | 0.458 | 0.500 | 0.500 | **1.202x** |
    | after, again | 0.417 | 0.458 | 0.625 | 0.583 | **1.499x** |
    | after, per-entry shrink | 0.458 | 0.541 | 0.583 | 0.625 | **1.365x** |
    | after, per-entry, again | 0.458 | 0.542 | 0.583 | 0.542 | **1.365x** |

    The before arm reproduces the published table nearly digit for digit
    (0.375 / 1.666→1.709 / 13.584→13.667 / 68.083→67.833), which is what says the
    two arms are measuring the same thing. At the unit,
    `crates/ply-codegen-spike/tests/entry_cost.rs` reads `end` at 4.168 ns/slot
    and `begin` at **0 ns at every rung** — below the platform's ~40 ns clock
    resolution, which is the honest way to say "constant".

    The last two rows are the shipped code and the first two are not; see the
    correction below.

    Three things the re-measurement found that the original did not:

    - **The interpreter arm is not flat either.** The original recorded "no
      carry-over on the interpreter arm (0.79 / 0.83 / 0.83 µs)" — measured after
      the three *smallest* predecessors, where this re-take also reads 0.79–0.88.
      Run out to the 19,584-slot predecessor and the interpreter column reads
      1.2–2.8 µs, so a call after an expensive interpreted call is slower on both
      arms. It is not monotone and not the seam; the likeliest reading is cache.
      It matters because it was the control: a pre-registered rule of mine said
      to withdraw the whole measurement if the control moved, and **that rule
      failed**. It is reported failed rather than rewritten. What carries the
      claim instead is the paired before/after on the same rows in the same
      binary, where a common-mode effect appears in both arms.
    - **The 181x is not the thing to fix.** See the withdrawn clause below: the
      181x is the *clear*, at 4.17 ns a slot, and the fix is not "stop
      shrinking".
    - **The shrink is not free either, and the first fix said it was.** Priced
      properly — same timed entry, varying only what ran before it — handing back
      a buffer costs nothing for a steady state, +0.3 µs after a predecessor
      twice the size, +14 µs at four times and +32 µs at eight. Tens of
      microseconds is the same order as clearing 19,584 slots. It is a trade, not
      a saving: **one** `free` at a downward transition, against item 12's 4.17
      ns for every one of the predecessor's slots on **every** entry.

    > **Corrected in place (2026-08-24), same day, and the correction is a code
    > change.** The paragraph above read "and the shrink is amortized over
    > `SHRINK_EVERY` = 64 entries instead of running per entry", and the two
    > `after` rows in the table were measured against that. The window is gone.
    > It was justified by a measurement of the shrink at 19,584 slots — 81,667 ns
    > against 81,708 ns, **1.00x, the shrink is free** — and that measurement
    > shrank a buffer already close to its target and does not generalise to one
    > four times the size. Worse, a schedule cannot answer a question about
    > demand: **one entry that used 27,002 slots left the arena at capacity
    > 32,768 for the entries that followed it**, for up to two 64-entry windows,
    > and for ever if the provider then went idle.
    >
    > `Ctx::end` now decides per entry, against what that entry used, with a
    > factor-of-two slack so a steady state neither shrinks nor regrows. The
    > hazard is armed by
    > `hazards.rs::one_large_entry_gives_the_arena_back_to_the_entry_after_it`,
    > which was written **red** against the windowed version and quotes the
    > numbers above in its failure message. The carryover curve was re-taken
    > against the shipped policy — the last two rows of the table, 1.365x twice —
    > because the first two rows measure code that no longer exists.
    >
    > One test in that file was also mis-describing itself and is corrected
    > rather than changed: `the_entry_arena_does_not_grow_with_executed_work`
    > called `pure.ladder(5_000_000, 1)` "one entry deep enough to box tens of
    > thousands of intermediates". Measured with the counters, it is **zero
    > entries and 10,000 `out_of_fuel` declines**, and the last entry to close
    > used four slots — the provider refuses every offer before the body can
    > allocate, so the second half of that test asserted memory was returned
    > without ever establishing it had been taken. Its assertion is left exactly
    > as written; the property it was named for is held by the new test, which
    > checks the entry count and the slots used before it checks anything about
    > memory.

    The original entry, kept because it is the record of how it was found:
    `crates/ply-codegen-spike/src/rt.rs`, `Ctx::begin`, ran
    `slots.clear()` and then `shrink_to(RETAINED_SLOTS)`; the clear drops that
    many `Value`s and the shrink reallocates. Measured best-of-7, twice, at two
    loads: the identical hybrid call `mcts.playouts(0,0,0)` runs in 0.375 µs
    after a 4-slot predecessor and 68.083 µs after a 19,584-slot one — **181x**,
    monotone, about 3.5 ns a retained slot, with no carry-over on the
    interpreter arm. `begin`'s own comment describes the retained buffer as a
    *memory* leak and does not connect it to the time. This is the mechanism
    behind R5's `mcts.playouts` 0.068x row, which
    `benches/r5-timing/RESULTS.md` §3 attributed to its own arm interleaving;
    that section is corrected in place and its "no function of the 26 is below
    1.00x" is withdrawn. Spike-only, so `rm -r` removes it — but the number it
    invalidates was published.

    > **Reproduced, and one clause withdrawn (2026-08-24).** A number that
    > withdrew a published claim should be reproducible by somebody other than
    > the reviewer who took it, so `Ctx::begin` was re-measured directly —
    > `crates/ply-codegen-spike/tests/entry_cost.rs`, release, best-of-7, run on
    > request with `--ignored --nocapture`. The mechanism holds: cost is monotone
    > in the previous entry's slots and the slope is **3.06 ns/slot** against the
    > reported "about 3.5 ns", across three runs. The 181x is not restated here
    > because it is an end-to-end ratio and this measures `begin` alone; the
    > slope is the claim, and the ratio was the slope times the ladder's span.
    >
    > **Withdrawn**: *"the clear drops that many `Value`s, **and the shrink
    > reallocates**"*. The shrink is not measurably part of it. `RETAINED_SLOTS`
    > is 4096, and `capacity() > RETAINED_SLOTS` is the only thing deciding
    > whether the shrink runs: an arena grown to 4096 has capacity 4096 and skips
    > it, one more push doubles capacity to 8192 and the reallocation fires.
    > Across that boundary the cost is **12583 ns against 12583 ns — 1.00x**, and
    > the ladder's slope is identical above and below it (3.07 ns/slot at 3824
    > slots, where no shrink runs, and 3.06 at 19,584, where one does). The whole
    > of the effect is `slots.clear()` walking N `Value`s to drop them. This
    > matters for the fix rather than for the diagnosis: removing the `shrink_to`
    > would return the memory the comment is worried about and change the timing
    > not at all.
    >
    > **And it gets worse in exactly the direction the fragment is being widened
    > in.** The ladder fills the arena with `Value::Int`, the cheapest thing
    > `clear` can walk — no refcount, no destructor. ADR 0018 §0's census and the
    > lexer profile both say the next thing to compile is record construction and
    > field access, which puts `Arc<BTreeMap<Symbol, Value>>` in those slots
    > instead, and then `clear` drops a refcount per slot and frees a map
    > whenever it held the last handle. Over the same 19,584 slots:
    > **Int 4.17 ns/slot, `Str` 17.4, two-field `Record` 177.0 — a
    > record-shaped arena costs 42x an Int-shaped one to clear.** So widening
    > multiplies both terms at once: more slots per entry, and a drop per slot
    > that is two orders of magnitude dearer. Item 12 is cheap to fix now and
    > expensive to leave until after the widening it most affects.
13. **Three holes in what polices the compiled seam; one of the three is now
    half closed.** Recorded together because each is a green result over space
    nothing exercises. The unarmed name gate under the third bullet is fixed and
    the bullet says so; the other two holes, and the corpus half of the third,
    are open.

    - **`ply test --engine both` cannot install a backend at all.** Still true
      and deliberately not fixed here — wiring a backend into the CLI is gated on
      item 9 and on the result-cache rule. What is new is that the claim is now
      written down as an inventory somebody can check rather than as a sentence:
      `crates/ply-eval/src/compiled.rs` §"What polices this seam, and what does
      not" counts it. `Compiled` and `set_compiled` occur **zero** times in
      `crates/ply-cli`, source and tests both; all five `set_compiled` call sites
      in the workspace are tests or the spike's own harness (2 in
      `ply-codegen-spike/src/measure.rs`, 5 in its `hazards.rs`, 3 in its
      `mutations.rs`, 27 in `ply-eval/src/compiled.rs`'s own tests, 2 in
      `ply-eval/tests/differential_corpus.rs`). So the shipping CLI catches
      **zero** of the eight deliberately wrong backends, and the rule that a
      backend run must not populate the result cache is **unenforced because it
      is unreachable**.
    - ~~**A backend that ignores its budget is a stack overflow, not a
      disagreement.**~~ **Closed (2026-08-24).** It read: *"`--mutate
      exceeds-budget` dies with `fatal runtime error: stack overflow`, exit 134,
      before a single case is compared."* It still overflows the stack — that is
      what the corruption *is*, a native recursion with no bound — but nothing
      inside a dead process can report it, so the report comes from outside it:
      every `--mutate` run is now started as a child, and a child that dies by a
      signal is scored as the disagreement it is. Re-taken with the shipped
      binary, the run ends `DISAGREEMENT  the backend took the process down
      (signal 6)` and a non-zero exit.
      `mutations.rs::a_backend_that_ignores_its_budget_kills_the_process_and_is_reported_from_outside_it`
      is the standing form; it runs the crash as a child of the test binary and
      asserts the child died by a signal, that it died of a stack overflow, and
      that the harness phrases it as a disagreement.

      The related weakness the catalogue recorded — the bounded form
      (`exceeds-budget=4`) is caught on the diagnostic-label axis only, and a
      harness scoring `(Err, Err)` as agreement would have passed it — is now
      armed by
      `mutations.rs::two_raises_that_differ_are_not_agreement_although_both_are_raises`,
      which asserts error-*ness* agrees on two real diagnostics, that
      `compare_answers` reports them anyway, and that one diagnostic compared
      against itself is still agreement. The corpus-scale row for
      `exceeds-budget=4` was **not** re-taken: one `--mutate` corpus run costs
      4m46s at this load and the bounded form was killed after twenty minutes, so
      R5's figure stands as R5 took it.
    - **The published-row gate is untestable by every corpus in the tree.**
      `benches/kernel` declares no effect at all and `ply-eval`'s differential
      corpus declines effectful names, so if that gate regresses both corpora
      report success. Unit tests in `ply_eval::compiled` are the only thing that
      notices. **Still open**: no corpus in the tree exercises this gate, and
      adding one means a corpus that declares an effect.

      The unarmed-gate half of this bullet is **closed**. It read: replacing
      `let name = closure.name.as_ref()?` with a fabricated empty `Symbol` left
      `cargo test -p ply-eval --lib` at 519/519 green, because
      `memo::pure_by_published_row` refuses the unknown name downstream — so
      `an_anonymous_closure_is_never_offered`, whose doc claimed "the name gate
      is what refuses it", was satisfied by a different gate. The six gates now
      live in `ply_eval::compiled::admit`, which answers with the `Gate` that
      refused rather than with a bare `None`, and each has a test asserting
      *that* gate. Every one of the six deletions was run against the full
      526-test lib suite and the reds are tabulated in that module's test
      header; the fabrication above is now caught, by exactly one test and
      nothing else.

14. **`AssertionKind::RecursionLimit` classifies nothing.** Found 2026-08-24
    while splitting the frame ceiling's diagnostic out of
    `ply_eval::limit::err_recursion_limit`, whose doc asserted the opposite:
    *"The message keeps the phrase 'recursion limit' so that ADR 0004's
    `AssertionKind::RecursionLimit` still classifies it."* The variant is
    declared at `crates/ply-test/src/slice.rs:268` and mapped to
    `"recursion_limit"` at `:284`, and it is **constructed nowhere** —
    `grep -rn 'AssertionKind::' --include=*.rs` finds exactly one variant ever
    built, `Eq`, at `slice.rs:326`. This is the `E0435` pattern: declared,
    registered, raised nowhere. Consequence is small and worth knowing — a
    consumer reading `Assertion::kind` to tell a runaway recursion from a failed
    `assert_eq` cannot, and the four tests that do tell them apart
    (`ply-cli/tests/failure_classification_audit.rs`, `ply-test/tests/hybrid.rs`,
    `ply-test/src/tests.rs`, `ply-eval/src/tests.rs`) all match the rendered
    string instead. `limit.rs`'s doc is corrected in place; the code gap is
    **not** fixed, because deciding whether the fix is to construct the variant
    or to delete it is a `ply-test` design call and this change was in
    `ply-eval`.

Items 9 and 10 are closed; see the block at the end of item 10 for the fix, the
measurements behind it and the tests that arm it. Items 11, 12, 13 and 14 are
open.

Items 2, 3, 4, 6 and 7 are one-line fixes this documentation pass did not make,
because the rule is that code is what shipped and a documentation pass corrects
documents. Item 5's comment was a document and was corrected; item 1 is a real
code defect and is reported, not fixed. Items 9 through 13 are R5's, found by
the reviews of it: **three of the four review lenses pointed at R5 refuted the
claim they were given**, and the documents they refuted are corrected in place
rather than rewritten. ~~None of 9–13 is fixed. They are open.~~ **That is no
longer true, and the current state is:**

- **9 and 10 are fixed (2026-08-24).** The frame bound was an engine's private
  resource guard rather than semantics; `DEFAULT_MAX_FRAMES` is deleted and
  `Machine::with_max_frames` is an opt-in ceiling no shipping command sets.
- **11 is open.** A definition that discharges its own effects still publishes an
  empty row, so the seam's purity gate still clears it.
- **12 is fixed (2026-08-24).** `Ctx::begin` no longer walks the previous entry's
  arena; `Ctx::end` clears it at the end of the entry that filled it, and the
  shrink is amortized over `SHRINK_EVERY` entries.
- **13 is two-thirds closed.** The unarmed name gate and the budget-ignoring
  backend that used to take the process down uncaught are both fixed. The first
  bullet — no shipping command can install a backend — is deliberately left
  open, because closing it is gated on the result-cache rule; what changed is
  that it is now an inventory somebody can check.

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
