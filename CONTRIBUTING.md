# Contributing to Ply

Read [`docs/ONBOARDING.md`](docs/ONBOARDING.md) first. It gets you from a fresh
clone to a running server and a verified change, with measured timings for every
step. This file is what to do *after* that: how to make a change here without
breaking something, and — the part this project cares about most — how to write
a claim down.

- [The one rule](#the-one-rule) — and [the shape it keeps taking](#the-shape-it-keeps-taking-declared-registered-raised-nowhere)
- [The loop](#the-loop)
- [Before you open a change](#before-you-open-a-change)
- [Writing a claim down](#writing-a-claim-down) — including [a moving tree invalidates a correctness number](#a-moving-tree-invalidates-a-correctness-number-and-only-an-instrument-says-so)
  and [the binary is an instrument too](#the-binary-is-an-instrument-too-and-the-rule-for-checking-it-was-blind)
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

### The shape it keeps taking: declared, registered, raised nowhere

> **This section's own count did not check out.** The withdrawn opening,
> verbatim: *"Four of the seven above and two more found since are the same
> defect wearing different clothes"* — six, over a table with five rows. Only
> **two** of the seven were ever in it, W1's footprint check and M8's
> mitigation; `E0435` is not one of the seven at all, and items 14 and 15 were
> found afterwards. The honest reading of the old table was two of the seven and
> three found since. Corrected 2026-08-27, when the check below was built and
> turned up a sixth.

**Two of the seven above and four more found since** are the same defect wearing
different clothes — a mechanism that is *named* everywhere a reader would look
for it and *constructed* nowhere. Counted, so the count is checkable rather than
rhetorical:

| the name | where it is declared | what constructs it |
| --- | --- | --- |
| `E0435 DB_SCHEMA_MISMATCH` | `ply-span`'s `codes`, its registry test, and `host.rs`'s reserved list | nothing — §"Do not state a guarantee you have not armed" |
| W1's footprint check | advertised in the milestone's own report | nothing — it was never armed |
| M8's mitigation | disclosed beside the unsoundness it was for | nothing that could fire |
| `AssertionKind::RecursionLimit` | `ply-test/src/slice.rs`'s `AssertionKind`, mapped by its `as_str` | nothing — item 14 |
| `CausalSlice` / `Event::Perform` | `ply-test/src/slice.rs`, rendered by `report.rs`, read by `commands/test.rs:793`, specified by ADR 0004 | nothing outside `tests/bisect_audit.rs` — item 15 |
| `Severity::Note` | `ply-span`'s `Severity`, rendered by `render.rs:73`, `ply-eval/src/differential.rs:803` and `ply-cli/src/commands/common.rs:55` | nothing — three renderers, no producer; found 2026-08-27 |

> **This section used to tell you to find one by hand, and the recipe does not
> work.** The withdrawn advice, verbatim: *"The check that finds one takes a
> minute: `grep -rn '<TypeName>::<Variant>'` or `grep -rn '<ConstructorFn>'
> --include=*.rs`, and then read the hits for one that is not a test and not the
> declaration."* Run it against the first row of its own table. `E0435` has
> exactly one mention outside tests — `crates/ply-eval/src/host.rs`'s
> `RESERVED_CODES`, a list of codes a *handler* may not answer with — so a sweep
> for references outside tests reports it as referenced and reports it green,
> and the registry row a reader would take as the second hit is itself inside
> `#[cfg(test)] mod tests`. **A mention is not a construction.** Eighty-three
> registered codes are also too many to re-read by hand once per change.

**The check is a test now.** `crates/ply-span/tests/armed.rs` fails when a
registered diagnostic code, or a variant of a covered enum, is constructed
nowhere in production source. Its header carries the rule in full and the list
of what it does not reach; the short version:

- A code is **armed** iff a production source passes `codes::NAME` to
  `Diagnostic::error` or `Diagnostic::warning`, literally or through a wrapper
  listed in that file's `CODE_INDIRECTION` —
  `every_registered_code_is_constructed_in_production`.
- A variant is **armed** iff it appears in production outside a pattern
  position. A `match` arm, an `if let`, a `while let`, a `matches!` and a `use`
  path are consumers, and prove only that something *reads* it —
  `every_variant_of_a_covered_enum_is_constructed_in_production`.
- Production excludes `crates/*/tests/`, `benches/`, every `#[cfg(test)]` item,
  and every module reached only through a `#[cfg(test)] mod` declaration. That
  last one is load-bearing: `crates/ply-core/src/numerics.rs` carries no marker
  of its own and is test-only solely because `lib.rs` says
  `#[cfg(test)] mod numerics;`.

  > **This clause said "block", and the gate did not deliver on the word.** The
  > withdrawn wording, verbatim: *"every `#[cfg(test)]` block"*. The scanner
  > walked from `#[cfg(test)]` to the first `{` **or `;`** and blanked only the
  > `{` case, so an item whose header contains a `;` before its body was left
  > in the production set — and one was, live in the tree:
  > `crates/ply-eval/src/argv.rs`'s
  > `#[cfg(test)] fn kept() -> [usize; CLASSES]`, where the `;` belongs to the
  > array type. A `Diagnostic::error(codes::X, ..)` in that body armed `X` and
  > the gate stayed green — the gate's own defect, wearing the gate's clothes.
  > Nothing in that body armed anything, so no answer was wrong; it was luck,
  > not the rule. Fixed 2026-08-27 in review: the walk steps over balanced
  > `(..)` and `[..]`, and a `;`-terminated item that is not a `mod`
  > declaration is blanked like any other. `mod x;` is still exempt, because
  > the resolver reads it.
  > `a_cfg_test_item_is_not_production_whatever_its_header_looks_like` fails
  > against the old walk.
- Something genuinely reserved goes in `UNARMED_CODES` or `UNARMED_VARIANTS`
  **with a reason and a citation**. That is the whole point: it stops "reserved
  on purpose" and "we forgot" looking identical.
  `no_allowlist_entry_has_outlived_its_reason` fails on an entry naming
  something now constructed, or no longer declared, so the excuse cannot outlive
  the fact.
- **It does not prove the construction site can execute.** Rows 2 and 3 above —
  W1's footprint check and M8's mitigation — are unreachable-code defects, and
  this check would not have caught either; nor would it catch a
  `Diagnostic::error(codes::X, ..)` behind an `if false`. Reachability is **not
  enforced** anywhere and is still yours to check by hand.

Run with both allowlists empty, before either was filled, it reported twelve:
`E0435`, `E0438`, six `AssertionKind` variants, three `Event` variants and
`Severity::Note`. All twelve are allowlisted with reasons rather than fixed,
because disposition belongs to whoever owns each: items 14 and 15 hold the nine
`ply-test` variants open, `docs/adr/0014-w4-contract.md`'s audit note and
§"Do not state a guarantee you have not armed" hold `E0435` and `E0438`, and
`Severity::Note` is new here and undecided. It also turned up the mirror defect, *declared and not
registered*: `REFERENCE_CYCLE` (`W0610`) was a `pub const` in `codes` with no
registry row, 83 constants against 82 rows, and
`the_code_registry_table_is_total_over_the_codes_module` now holds that closed.

Do the by-hand version anyway for everything the check does not reach, and do it
for the thing you are about to *rely on* as well — item 15 was found by asking
whether item 11's defect could reach a user, and the answer was that the route
does not exist.

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
cargo test --workspace                          # 9.5-29 min — 3,696 pass, 0 fail, 5 ignored
```

> **Re-taken 2026-08-31, as one command after the last edit of the compiled
> seam's answer-test change:** `cargo fmt --all --check` silent, `cargo clippy
> --workspace --all-targets` **0 warnings**, `cargo test --workspace` **3,868
> passed / 0 failed / 5 ignored over 167 test targets**. This is a measurement
> and not arithmetic — the source digests were taken before the run and checked
> equal after it, so nothing moved under it. The `3,696` above is left as the
> line this file has carried, because the number it is compared against below
> was taken with a different command (`--no-fail-fast -- --test-threads=2`) and
> replacing one with the other would be the conflation the block underneath
> warns about.

> **The count moved on 2026-08-28 and this line is deliberately not rewritten
> to a number nobody took.** ADR 0026's work adds **24** tests — 8 to
> `ply-eval`'s `differential_corpus` (the eight wrong backends at corpus scale),
> 14 in a new `ply-cli/tests/backend.rs`, 1 to `ply-span`'s `armed.rs` and 1 to
> `ply-corpus`'s `w6` — so a full run should read **3,720**. That is arithmetic,
> not a measurement: `--workspace` was not re-run for this change, on this file's
> own rule that a figure belongs where it was taken. The per-package counts that
> *were* taken are `cargo test -p ply-eval` at **1,014 passed / 0 failed / 1
> ignored over 43 targets**, and `cargo test -p ply-cli --test backend` at 14/0.
>
> It also adds wall clock in one place worth naming: `cargo test -p ply-eval
> --test differential_corpus` goes from seconds to **73.4s**, because five of the
> eight corruptions sweep the whole 1,116-test corpus. An earlier run of the same
> suite read 82.7s; both are **observations rather than figures**, taken at a
> 1-minute load average of 8-9 against this file's own 4.0 gate. That is the
> price of the seam having measured sensitivity inside `cargo test --workspace`
> rather than in a crate on another toolchain.

> **Re-taken after item 11's fix (2026-08-24).** The line above read `9.5-29
> min — 3,690 pass, 0 fail, 5 ignored`. `cargo test --workspace -j 2
> --no-fail-fast -- --test-threads=2` now reports **3,696 passed / 0 failed / 5
> ignored** across **155 targets** (142 binaries + 13 doc-test suites), exit 0,
> **1,172.3s real / 1,168.4s user** by `/usr/bin/time -p`, with `fmt --all
> --check` silent and `clippy --workspace --all-targets` at zero. The wall clock
> is an upper bound as usual — another agent was building on this machine.
>
> **The +6 is all of item 11's, and it was attributed per package rather than
> subtracted.** An earlier draft of this block hedged — *"The +6 is not
> attributable to this change alone and is not claimed to be … Reading a
> per-target diff against a tree that no longer exists is how you would
> attribute the rest, and this pass did not do it."* — and then named exactly
> six tests, which is the whole of the +6. The three package counts were taken
> on both sides while the change was being built, so no diff against a vanished
> tree is needed:
>
> | suite | before | after |
> | --- | --- | --- |
> | `cargo test -p ply-eval --lib` | 527 | 531 |
> | `cargo test -p ply-eval --test differential_corpus` | 5 | 6 |
> | `cargo test -p ply-core --lib` | 204 | 205 |
>
> The 527 is not this pass's word for it either: `compiled.rs`'s test-module
> header recorded *"a re-run reads 527 green"* when the frame-ceiling change
> took the 3,690. Four of the six are the effects gate's own — including
> `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop`,
> the only one that catches a propagation stopping after one hop — one is the
> corpus test, and one is `ply-core`'s for the duplicate-`fn` hazard
> `mark_internal_effects` would otherwise panic on.
>
> **Taken twice, by two methods, and they agree.** The figure above is one
> `--workspace` run. It was also taken as `.github/ci-shards.sh`'s four shards,
> each into its own log, from a `rsync`ed copy under `/tmp` that no other
> session could reach — `ci-shards.sh verify` reports *13 workspace members,
> each in exactly one shard*, so the four sum to the workspace:
>
> | shard | passed | failed | ignored | binaries | doc-tests | real |
> | --- | --- | --- | --- | --- | --- | --- |
> | `core` | 1,528 | 0 | 0 | 41 | 9 | 38.2s |
> | `cli-eval` | 1,688 | 0 | 1 | 77 | 2 | 240.7s |
> | `corpus` | 199 | 0 | 3 | 16 | 1 | 235.2s |
> | `postgres` | 281 | 0 | 1 | 8 | 1 | 44.2s |
> | **sum** | **3,696** | **0** | **5** | **142** | **13** | **558.2s** |
>
> The shard set was run **twice**, on two frozen copies taken either side of the
> last documentation edit, and the five counts are identical both times
> (1,528 / 1,688 / 199 / 281 and the 142 + 13 split); only the wall clocks moved
> — 577.3s the first time, 558.2s the second, which is what a machine with three
> other agents on it does to a wall clock and does not do to a count. Those are
> sequential shards from a warm `target/` and are not comparable with the
> single-run figure above. `PLY_PG_URL` was not set for any of the three runs,
> so the `postgres` shard's ten live tests passed without running — the gate
> every unqualified reading in this file was taken under.
>
> **Both runs were instrumented and an earlier one was not, which is the part
> worth copying.** The earlier attempt reported 3,689 / **6 failed**, the six
> being exactly the tests that go red when `Gate::InternalEffects` is deleted,
> with the gate present in the file throughout — traced to
> `crates/ply-core/src/infer.rs` being rewritten *during* the run, so cargo
> built `ply-core` from a file that was mid-edit. Nothing in the log said so.
> Who was writing is not established and the honest reading is in §"A moving
> tree invalidates a correctness number": a background `cargo test --workspace`
> and a foreground mutation in the same session will do this to you without any
> second party. The runs above digest every non-`target/`, non-`.ply-cache/`
> file before and after; the copy's digest was identical across the run
> (`da4c6109…` for the final one), and its `.rs`, `.ply` and `.toml` matched the
> worktree's when the run started. Take the digest — a wrong red costs an
> afternoon, and a wrong **green** is what this file exists to prevent.

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

`.github/workflows/ci.yml` runs on every pull request, and on pushes to `main`.
It runs the three commands above, and it opens all five of the gates the next
section describes — which is most of why it exists, because a gate that returns
a *passing* result when its dependency is absent is the thing a human is least
likely to notice. It **asserts** four of them open by a notice the job greps
for: `PLY_PG_URL`, `cluster::available()`, `#![cfg(unix)]` and the spike each
fail a job if the gate is shut. `PLY_TEST_DB` has no notice — those 26 tests
print nothing at all when they skip — so it is asserted a different way, and the
measurement behind it is in the gate table below: a *wrong* value fails 20 of
the 26 loudly, because the harness expects the database to be reachable, and a
*missing* value is caught by the job's `test -n` pre-flight. What no one can
show after the fact is a log line saying the 26 ran.

> **Corrected 2026-08-30: it used to run on every push too, and that ran the
> whole workflow twice.** The withdrawn text, verbatim: *"runs on every push and
> every pull request, on every branch and not only the default one."* That was
> accurate and it was also a bug. A push to a branch with a PR open fired both
> triggers, and the `concurrency` group cannot collapse them because it keys on
> `github.ref` — `refs/heads/<branch>` for one and `refs/pull/<n>/merge` for the
> other. Over the 20 runs before the fix, every branch commit has a matched
> pair, each a full run of the identical jobs over the identical tree. `push` is
> now filtered to the default branch. What that gives up is CI on a branch with
> no PR open yet.

| job | what it runs |
| --- | --- |
| `shard table is total` | `.github/ci-shards.sh verify`, before anything compiles |
| `cargo fmt --all --check` | the same command, and it must be silent |
| `cargo clippy --workspace --all-targets` | with `-D warnings`, so the first warning is a failed run rather than a line in a log |
| `test corpus` / `eval` / `cli` | the suite, three shards by package; the split is `.github/ci-shards.sh`. The shard that holds `ply-cli` also re-runs `w5_shutdown` by name, and the shard that holds `ply-span` re-runs the seven `TREE_CHECKS` by name — see the row below. Both steps key on *which shard holds the package*, not on a shard name, which is why renaming `cli-eval` to `eval`/`cli` on 2026-08-30 moved neither |
| the `TREE_CHECKS` step, inside `test` | the seven checks in `crates/ply-span/tests/armed.rs`, each by `--exact` name, asserting `test result: ok. 1 passed`. They already ran a moment earlier in the shard; this is what turns "the check still exists" into an exit code, because `cargo test --exact` over a name nothing defines reports `0 passed; 11 filtered out` and exits **0** |
| `test ply-host (postgres)` | `ply-host`, with `PLY_PG_URL` and `PLY_TEST_DB` pointed at a `postgres:18.6` service container **and** `initdb`/`postgres`/`psql` on `PATH` — then it fails if any test printed a skip notice |
| `wall-clock measurements` | the thirteen timing-sensitive tests, one at a time, single-threaded, alone on a runner |
| `crates/ply-codegen-spike` | `cargo test --locked --release` in the spike's own workspace. This cell read *"on a pinned 1.94.0"*; the pin moved to **1.93.1** with the crate's move to cranelift 0.132.3 on 2026-08-31 and this row was missed in that change |
| `examples/same-tests.sh` | W4's exit criterion: a release `ply` **the script now builds and freshness-checks itself** (the row used to read "a release `ply`", which was the job's build and not the script's), a cluster the script starts itself, and the twin compared against postgres byte for byte. **It also runs the code generator over `examples/` on the engine pair** — `ply test examples --engine both --backend cranelift`, which is the only check in this repository that reads compiled answers against an independent evaluator at corpus scale, plus the assertion that it entered anything at all |
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

The three shards are cut from a measurement, not a guess, and the measurement
is now taken **in CI** rather than locally — the hosted runner is the machine
the balance is for. From run 33338854134 (2026-08-30), differencing consecutive
`Running <binary>` lines in each job log so the figure is test execution without
the ~39s dependency build: `ply-corpus` **426s**, `ply-eval` **404s**, `ply-cli`
**200s**, `ply-host` ~60s, and the other nine packages **16s** summed.
`ply-corpus` is the floor — no arrangement of `-p` flags finishes sooner than
its slowest package — so three parallel shards at 426s / 404s / 216s sits on it,
and a fourth would buy nothing and pay the dependency build again.

> **Corrected 2026-08-30: the previous cut was measured on the wrong machine and
> had gone stale, and the shard it produced was the slowest thing in CI.** The
> withdrawn text, verbatim: *"`cargo test -p <package>` on the machine in
> `docs/ONBOARDING.md` §Provenance, warm target, `-j 2 -- --test-threads=2` to
> approximate a two-core hosted runner, 2026-08-24: `ply-corpus` **289s**,
> `ply-cli` **149s**, `ply-eval` **137s**, `ply-store` 32s, `ply-hash` 15s,
> `ply-test` 13s, `ply-core` 8s, `ply-span` 5s, `ply-prove` 4s, `ply-syntax` 3s,
> `ply-std` 2s, `ply-derive` 1s — 658s summed, of which `ply-corpus` is 44%.
> That one package is the floor: no arrangement of `-p` flags finishes sooner
> than it does, so a fourth shard would buy nothing and pay the dependency build
> again."* The floor argument survives; the arrangement had stopped sitting on
> the floor. `ply-eval` roughly tripled — 137s to 404s — so the `cli-eval` shard
> became 604s against a 426s floor while the `core` shard finished its 1,604
> tests in **16s**: one runner idle for ten minutes while another held the whole
> run up. `-j 2 -- --test-threads=2` on a 10-core laptop is also a poorer
> instrument for a hosted runner than the runner's own logs, which is why the
> re-take is from CI.

The numbers are the runner's and the ordering is what the table depends on;
re-take if a package grows a slow suite, and move it. **The figures above
predate `[profile.dev] opt-level = 2`**, which cut the binary dominating the
`eval` shard by most of an order of magnitude and should be expected to make
these jobs compile-bound rather than test-bound.

Re-taken after that change, and worth only what its provenance is worth: the
three shards run **locally** — a 10-core laptop, not the two-core runner they
are balanced for — at 1-minute load between 6 and 24, against the threshold of
4 that §"Gate on an idle machine before measuring, not after" sets. `corpus`
120s / 197 tests, `eval` 69s / 1,033, `cli` 41s / 2,319, all three exit 0. That
is a shape, not a set of figures, and the shape is that `ply-corpus` stops being
level with `ply-eval` and becomes the clear long pole at roughly 1.7x it — which
is the ordering the table already assumes, so nothing moves on it. **A re-take
on a two-core runner at load < 4 has not been done.**

**And the part that is not checked. This is a ledger, not a blanket.** An
earlier draft of this paragraph said *"every command in `ci.yml` was run on that
machine before it was committed, and their exit codes are the evidence for
everything above."* It was withdrawn the same day it was written, because
`ci.yml` was edited after that run and the sentence outlived the thing it rested
on. What follows is what has an exit code against the file as it stands, taken
on the machine in `docs/ONBOARDING.md` §Provenance, 2026-08-24.

> **The same thing has now happened to part of this ledger, and it is left
> standing with the boundary named rather than re-run.** The 2026-08-30 change
> that split `cli-eval` into `eval` and `cli` and set `[profile.dev] opt-level =
> 2` moves three of the rows below: the `corpus`, `core` and `cli-eval` shard
> rows name a partition that no longer exists, and **every** wall clock in the
> table was taken at `opt-level = 0`. The exit codes and the *counts* are
> unaffected by the profile — 196 / 1521 / 1669 tests and their filtered totals
> are what the packages contain, not how fast they run — and the packages
> themselves only moved between shards, so the sum is the same suite. What has
> no exit code against the current file is the three shard rows as *shard* rows
> and every second figure in the table. Re-running the ledger against the
> current `ci.yml` has **not** been done.

**Static checks, all exit 0.** `actionlint` 1.7.12 over `ci.yml` with its
`shellcheck` integration active; `python3 -c "yaml.safe_load(...)"`, which
parses and reports nine jobs; `shellcheck` over `.github/ci-shards.sh`; and
`ci-shards.sh verify`, which reports *13 workspace members, each in exactly one
shard; 1 crate(s) deliberately outside; 13 deferred tests and 7 tree checks,
each present in the tree*. `verify`'s failure paths were exercised too, eight of them, each exiting
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
| `cargo test --locked --release` in `crates/ply-codegen-spike` | **0**, **49 tests** — `hazards.rs` 18, `mutations.rs` 13, `mcts_kernel.rs` 9, `spike.rs` 9, plus 3 ignored in `entry_cost.rs` — re-taken 2026-08-31 on the default 1.93.1 after the move to cranelift 0.132.3. This row read *"`cargo +1.94.0 test --locked --release` ... **45 tests** — `hazards.rs` 16, `mutations.rs` 11, `mcts_kernel.rs` 9, `spike.rs` 9 — 438.9s including the cold release build"*; the toolchain prefix is gone and the counts had moved under it. **It does not reach the agreement corpus — see item 18.** |
| `cargo build --locked --release -p ply-cli` | **0**. Since 2026-08-31 this builds cranelift too — `ply-cli` depends on `ply-codegen`, unconditionally and with no feature flag. Marginal cost measured before the decision was taken: **16.26 s wall / 63.01 s user**, min of 3 windows, release, cleaning exactly the 32 packages the change added; see §"Things known to be broken" item 1 |
| `ply test examples --engine both --backend cranelift` | **0**, 186 passed, **696 of 62,660 offers entered**, fragment 27, 6 units compiled. Watched to fail two ways: `--backend cranelift:wrong:off-by-one` exits 1 with 629 changed answers and 11 tests reporting them, and forcing the fragment empty exits 1 on *"the code generator entered nothing, so the run above is green over a seam no call reached"* |
| `cargo test -p ply-codegen` | **0**, **11 tests** — `fragment.rs` 9 over the standard library, `kernel.rs` 2 over `benches/kernel` |
| `./examples/same-tests.sh` | **0**, **29 requests byte-for-byte identical** between the twin and postgres, ~~5.63s~~ **UNMEASURED since 2026-08-27**, against a cluster the script started itself. The count is unchanged and re-taken; the wall clock is withdrawn rather than updated, because the script builds `ply-cli` in release itself now and 5.63s named a run that did not. It was not re-taken: the 1-minute load average on the machine that made the change read 30.5, the gate this project measures behind is 4.0, and §"Say how it was checked, or say it was not" prefers a hole with provenance to a number without |
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
> | anything in `crates/ply-codegen-spike` | `cd crates/ply-codegen-spike && cargo fmt --check && cargo clippy --all-targets && cargo test --release` — **on the default 1.93.1; no `+1.94.0` and no second toolchain (2026-08-31).** This cell used to read *"`cargo +1.94.0 fmt --check && cargo +1.94.0 clippy --all-targets && cargo +1.94.0 test --release` — the toolchain is not optional, `cranelift 0.134.3` and `wasmtime 47.0.3` require rustc 1.94.0 and the default here is 1.93.1"*. The crate moved to `cranelift 0.132.3`, which declares `rust-version = "1.93.0"`, and pulls `wasmtime-internal-* 45.0.3` rather than 47.0.3; both build on the pinned 1.93.1. And **never** pipe it: `cargo build 2>&1 \| tail` reports `tail`'s exit status, so a failed build reads as a successful one. |

The original four:

| if your change touches | you must also run |
| --- | --- |
| postgres, the pool, transaction scope | `PLY_PG_URL=postgres://localhost/postgres cargo test -p ply-host` (36–38s, 281 pass, 0 fail) — otherwise ten tests pass in 0.00s without running, and cargo captures the skip notice so nothing tells you: `skipped:` occurs zero times in a whole `cargo test --workspace` log |
| the pool | `PLY_TEST_DB='postgresql://ply@127.0.0.1:5432/ply_test?sslmode=disable' cargo test -p ply-host --lib db::pool`. **This is the worst of the five: 26 tests hide behind it and print *nothing* when it is unset** — not a skip line, not on stderr, nothing. Re-measured 2026-08-24 on the machine in `docs/ONBOARDING.md` §Provenance against a local postgres 18.3: unset gives `26 passed` in `0.00s`; set and reachable gives `26 passed` in `0.94s`; set and *unreachable* gives **`20 FAILED`** in `0.02s` with `E0431`, because `db/pool/tests.rs:41` does `.expect("the test database is reachable")`. So a wrong value is loud and only a missing one is silent — CI sets the variable, pre-flights it with `test -n` and a `psql SELECT 1`, and that combination does cover it. An earlier version of this row said the figure was `1.55s` while `ROADMAP.md` said `1.35s` for the same measurement; neither had been re-taken |
| shutdown, drain, signals | anything; but know that `crates/ply-cli/tests/w5_shutdown.rs` is `#![cfg(unix)]` and compiles to nothing off Unix. CI runs on `ubuntu-24.04`, so it is compiled there, and a step fails if that binary reports zero tests |
| the served request path or its cost | `./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`, and see §"Things known to be broken". **Name the two files, never `benches/*.json`.** `benches/` holds three since R3, `w6` merges what it is given field by field on a last-wins basis, and the glob expands alphabetically — so `ply-corpus w6 benches/*.json` renders the **pre-region** ladder, dated `2026-08-16`, with `1035 times and 0.124 MB` in its boxing lever, exactly as if R3 had not happened. Checked by running it. `benches/README.md` §"There are two ladders" says which file is which |
| the Ply parser spike's differential can still go **red** | `./spikes/ply-parser/run.sh --arm` — 22 mutations, 299s on a 10-core machine. CI runs `run.sh` and not `--arm`, so what CI checks is that the comparison is green and **not** that it could ever have failed. The mutation table is also the thing that goes stale: a corruption whose anchor text has moved is scored `NOT APPLIED`, which is a real finding and one only this command reports. `.github/workflows/ci.yml`'s `parser-spike` comment carries the measurement and the trade |
| `crates/ply-codegen`, `crates/ply-eval/src/{backend,compiled,code}.rs` | `cargo test -p ply-codegen -p ply-cli --test backend` **and** `./target/release/ply test examples --engine both --backend cranelift`, in that order. The unit tests use corpora of five and forty-four definitions; the second command is 186 tests against the shipped standard library and is the only place a wrong `Int` out of compiled code is read against an independent evaluator at scale. Freshness-check the binary first — `.github/binary-is-current.sh target/release/ply` — which does cover this crate: touching `crates/ply-codegen/src/jit.rs` takes it from `current (163 inputs checked)` to `NEWER … STALE`, run rather than assumed. **A stale binary here is not a hypothetical:** the fragment-size numbers in this file were once taken against a `ply` built from a deliberately corrupted `scalar_signature` and read as `fragment 0` on `benches/kernel`; they are `fragment 25, 2,974 of 3,097 offers entered` on a current one |
| `examples/desk.ply` or any host handler | `./examples/same-tests.sh`. ~~build `--release` first, it does not build for you.~~ **It builds for you since 2026-08-27**, and refuses to run against a binary older than a source in its own dep-info, so the hand-build this row used to demand is now `--no-build` for the case where you meant a particular binary. That build is `--locked`, so a `Cargo.lock` that has fallen behind the manifests stops the script with cargo's own `cannot update the lock file ... because --locked was passed` instead of being rewritten under a run: `cargo build` once, then re-run. CI runs it in a job of its own, so this one is caught before a merge rather than only when you remember |

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
- Any of them may carry a **wrong-instrument** block: `> **Corrected: this
  number is right, and it is not the number that decides this.**` It keeps the
  figure and withdraws the *inference*, naming the question the figure actually
  answers.

All three quote the withdrawn text verbatim. Do the same.

**Why the third one had to be named.** Every correction block written in this
record before 2026-08-28 that moves a measurement moves a *magnitude* — 3.3% →
28.6%, 1,082 → 773.4, 8.44× → 11.67×. The ones that move no measurement withdraw
a claim (`DESIGN.md`'s "v0 restriction: tail-resumptive only") or an explanation
(`spikes/ply-lexer-rc/fieldorder.ply`'s last-mention rule). Not one says *the
wrong thing was measured*. So the convention was relentless at catching a wrong
number and had no mechanism at all for catching a wrong instrument, which is how
a whole tier of findings survived audits that are otherwise unforgiving. If your
correction leaves every figure standing and moves what one of them was taken to
prove, it is this kind, and it is the kind worth writing down. The template is
ADR 0024 §4 (branch `adr/ownership`, PR #43): it keeps ADR 0020 §6.3's profile
exactly as measured and withdraws what was concluded from it, because a share
measured underneath an interpreter's dispatch settles a question about this
evaluator and not about the language.

**The cost of that convention, which you will pay before you understand it: a
grep for a stale claim hits forever, and hits hardest in the document that
corrected it.** Because the withdrawn wording is kept verbatim, searching for the
old sentence finds the correction rather than a live error. Two agents have now
reported a "stale claim" that was a blockquote — one flagged `CONTRIBUTING.md`
for still saying `.github/` does not exist, three lines below a heading reading
"There is CI".

So: **before believing a grep hit, check whether it sits inside a `>` block, a
`~~strike-through~~`, or a quoted-and-withdrawn span.** The same trap is live for
`DEFAULT_MAX_FRAMES` across half a dozen files, for ADR 0018's `181x`, and for
every figure this file has ever corrected. It is a real cost of a convention
worth keeping, and it is cheaper to know than to rediscover.

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

### The two seam instruments, and the vacuity trap both of them set

`crates/ply-eval` carries two environment-gated measurement knobs. Neither is a
`--backend` spec and neither should become one: a spec is a user-facing promise,
these are instruments.

- **`PLY_SEAM_CENSUS=1`** — `crates/ply-eval/src/census.rs`. Counts what the
  compiled seam is offered and which gate refused it, printed on stderr at exit.
  Every **coverage** share must be taken with **no backend attached**: entering a
  call hides its whole subtree, so it shrinks numerator and denominator together.
  Since the 2026-08-31 answer widening that is a requirement rather than a
  convention — with a backend attached the Ply front end's denominator collapses
  from 2,414,170 body calls to 26.
- **`PLY_BACKEND_ONLY=<comma-separated program-wide names>`** —
  `crates/ply-eval/src/backend.rs`. Narrows `Reference`'s registry to those
  names. It exists to price a *different* backend's limit with the one that
  ships: `Reference` is a tree-walker and can run anything the seam admits, so
  the only way to ask "what would a code generator that cannot compile a callback
  reach" is to take the callback users away from it. ADR 0030 §10 is that
  measurement. Narrowing can only add declines, so it cannot change an answer —
  `--engine both` audits 13 of 13 with 0 failed under it.

**The trap is the same for both and it is this project's signature defect.** A
census that was never enabled prints nothing and a narrowing that never applied
enters everything, and in both cases the run still says `0 failed` — so the
number you write down is the *unrestricted* one wearing the restricted one's
label. Check the instrument fired, in the run's own output, and check in the
reading: `ply test` prints `N in the fragment` and `X of Y offers entered` on
every backend run, and the difference between `26 of 26 · 413 in the fragment`
and `495152 of 1049245 · 220 in the fragment` is what says the variable reached
the process. Both readings belong in the log, not just the one you wanted.

### A moving tree invalidates a correctness number, and only an instrument says so

The section above is about a busy machine invalidating a *timing* number. This
is the same idea on the other axis, and it was learned the same way — by getting
a number, believing it for a minute, and then finding out where it came from.

**What happened (2026-08-24, item 11's fix).** Three ways to get a number that
describes nothing, all three met in one afternoon. Two were measured here and
the third is reported; they are marked apart because that is the point of the
section.

**Measured.** A full-suite log summed to **3,744 passed / 0 failed / 6 ignored
across 144 binaries + 26 doc-test suites**, which is a plausible-looking result
and is not a result at all: 26 is 13 twice. A `cargo test` that had been killed
left a shell holding the log file open, a second run redirected to the same
path, and the file ended up holding both runs. `grep -c '^real'` on it answers
**2**, and `grep '^     Running' | sort | uniq -d` lists the targets counted
twice.

**Measured.** The `rsync`ed copy the run was taken in changed its digest during
the run, with nothing but the suite touching it — because the suite writes into
the *source* tree: `examples/.ply-cache/frontend.dat` and
`tests/fixtures/.ply-cache/`. An instrument that cries contamination at its own
subject is worse than none, which is why the prune list below is part of it.

**Reported, not measured here.** A run that came back **3,689 passed / 6
failed**, the six being exactly the tests that go red when
`ply_eval::compiled::Gate::InternalEffects` is deleted, on a tree where the gate
was present — traced to `crates/ply-core/src/infer.rs` being written *mid-suite*
at 16:52:35, with a debug `eprintln!` in it. That log is not in this session's
hands and the figure is not re-takeable, so it is recorded as an account rather
than as a measurement. The mechanism is worth carrying even so, and the
uncomfortable half of it is this: a mid-suite write does not need a mystery
agent. Editing a file to run one targeted mutation while a background
`cargo test --workspace` is still in flight does it to you, and that is the
likeliest reading of this one.

**The rule.** A suite run is a claim about *a state of the tree*, so name the
state:

- **Run verification in a frozen copy, not in the tree you are editing.** It
  costs one `rsync` and it makes the claim attributable:

  ```
  rsync -a --exclude 'target/' --exclude '.git' --exclude '.ply-cache' \
    ~/.worktrees/ply/<branch>/ /tmp/<branch>-frozen/
  cd /tmp/<branch>-frozen && CARGO_INCREMENTAL=0 cargo test --workspace ...
  ```

- **Digest the copy before and after anyway.** Prevention is what gets you a
  number; detection is what tells you a number is void. You want both, because
  a copy proves nothing if you cannot show it did not move:

  ```
  find . -path ./target -prune -o -type f \
      \( -name '*.rs' -o -name '*.ply' -o -name '*.toml' \) -print0 \
    | sort -z | xargs -0 shasum -a 256 | shasum -a 256
  ```

  **Prune `target/` and `.ply-cache/`**, or the instrument reports your own run
  as contamination. Cargo writes `.rs`, `.json` and `.toml` under `target/`; and
  the suite writes **into the source tree** — `examples/.ply-cache/frontend.dat`
  and `tests/fixtures/.ply-cache/` appear where no `.gitignore`d build directory
  is. Both were found by a digest that flagged a copy nothing but the suite had
  touched. Check the instrument the way you would check any other: take the
  digest twice with the tree held still and require the two to match *before*
  trusting a mismatch.

- **Write the log somewhere only this run can write, and check it holds one
  run.** A killed `cargo test` can leave a shell that still has the log open, so
  a second run redirecting to the same path produces a file with two runs
  interleaved in it — and the summing pipeline reads it as one. That is how
  **3,744 passed / 0 failed / 6 ignored across 144 binaries + 26 doc-test
  suites** happened here: 26 is 13 twice. `grep -c '^real'` on the log is the
  one-line check — more than one means the file is a mixture and every count
  taken from it is void.

- **The same trap, one scale down**, is already recorded in
  `crates/ply-eval/src/compiled.rs`'s test-module header: cargo fingerprints on
  second-granular mtimes, so a mutation and a `cargo test` inside the same
  second can be served the *previous* mutation's artifact. That guard covers one
  mutation cycle; this one covers a whole suite. Both are the same sentence — a
  test result describes the bytes that were compiled, not the bytes on disk when
  you read the summary.

None of the three is detectable after the fact without the instrument, which is
the whole reason to arm it before you need it. And the cheapest rule of all:
while a suite is running, do not touch the tree it is reading — start it in a
copy and leave the copy alone.

### The binary is an instrument too, and the rule for checking it was blind

The section above is about the *tree* moving under a suite. This is the same
sentence for a **pre-built binary**: `./target/release/ply` is a frozen copy of
a tree, the tree it was frozen from is not necessarily the one you are looking
at, and the rule this project used for checking that could not see the
difference in the place it is most often pointed.

**The withdrawn rule, verbatim:**

> ```
> find crates -name '*.rs' -newer target/release/ply
> ```

It is quoted rather than corrected in place because **until this section it was
written in no file in this repository.** Checked, before this section existed:

```
find . -name '*.md' -not -path './target/*' -exec grep -Hn -- '-newer' {} +
grep -rn -- '-newer' --include='*.sh' --include='*.yml' .
```

Both returned nothing. The first now returns eight hits — five here, and one
each in `benches/README.md`, `docs/adr/0020` and `spikes/ply-lexer/GAPS.md`,
all of them this section's own citations of the withdrawn form. The bare word
`newer` occurs six times in the Markdown and every one is the ordinary English
word. So this was an **oral** rule, which is worse than a written one: nothing
carried it, so nothing could correct it, and it was reproduced from memory into
two rounds of work. It is written down here so that from now on it can be.

**Why it is blind.** `crates/ply-std/src/lib.rs` `include_str!`s all eight
stdlib modules into the binary, so editing `crates/ply-std/ply/http.ply` changes
what `import std.http` means in every program and moves **no `.rs` file at
all**. Reproduced here on 2026-08-27 — one line of `http.ply` changed from
`pub fn max_reads() -> Int = 2048` to `4096`, and no rebuild:

```
$ find crates -name '*.rs' -newer target/release/ply | wc -l
0                                   # the rule reports the instrument clean

$ ply std --digest
b3:a99604f49bd5                     # unchanged — the binary still holds 2048

$ ply test probe.ply --no-cache     # assert_eq(http::max_reads(), 4096)
  assertion failed: expected 4096, found 2048
  suspects: std.http.max_reads
```

The file on disk says 4096, the program says 2048, and the check says clean.

**Reported, not measured here.** A round-1 workstream published
`examples/: 1,428 entries, 0 moved` as an exit criterion and the true figure is
**84**, traced to this hole. That log is not in this session's hands and the
figure was not re-taken here, so it is recorded as an account rather than as a
measurement — the mechanism above is what was measured, and it is sufficient on
its own. ADR 0020 §0 opens with a whole ADR nearly lost to the same class, which
makes this the second instance, and is why the replacement below is a script you
run rather than a sentence you remember.

**The corrected instrument is rustc's own dep-info, not `find`.** Every link
writes `<binary>.d` beside the binary listing exactly the files that were read
to produce it. Measured on this tree, 2026-08-27:

| dep-info | paths | `.rs` | `.ply` | crates |
| --- | ---: | ---: | ---: | ---: |
| `target/release/ply.d` | 152 | 144 | **8** | 12 |
| `target/release/ply-corpus.d` | 175 | 165 | **10** | 13 |
| `target/release/w6-alloc.d` | 175 | 165 | **10** | 13 |

The eight are `crates/ply-std/ply/*.ply`; the two extra in the corpus binaries
are `crates/ply-corpus/ply/w4.ply` and `w5.ply`. **Nothing had to tell rustc
they were there** — that is the whole argument for dep-info over any `find`
expression: it is per binary, it needs no list here that could go stale, and it
covers the next `include_str!` somebody adds. `ply.d` omits `ply-corpus` and
`ply-codegen-spike` because the `ply` binary does not depend on them, which is
correct rather than a gap; each binary carries its own.

The one-liner, if you want no script — and note it is **mtime only**, so it is
the first instrument below and not the second. It closes the `.rs`/`.ply` hole
in the withdrawn rule and stays blind to a `.ply` whose bytes changed while its
mtime did not (measured: edit `json.ply` and `touch -t 202001010000` it, and
this prints nothing while the running program disagrees with the file):

```
awk '{ i=index($0,":"); if (i) print substr($0,i+1) }' target/release/ply.d \
  | tr ' ' '\n' | grep . | xargs -I{} find {} -newer target/release/ply
```

**Do not run this in your head — run `.github/binary-is-current.sh`.** A rule in
a document is what failed twice.

```
.github/binary-is-current.sh                             # target/release/ply
.github/binary-is-current.sh target/release/ply-corpus target/release/w6-alloc
.github/binary-is-current.sh --self-test                 # watch it go red
```

Exit 0 current, 1 STALE, 2 unanswerable. It runs three instruments and the
second is the one that matters:

1. **dep-info** — a listed input missing, or not older than the binary.
2. **The bytes, not the clock** — `ply std --show std.<m>` prints the module
   source compiled into *this* binary, and it is diffed against the file. This
   is a content check, not a timestamp check, so it survives `touch`, a checkout
   that rewrites mtimes, an `rsync`, clock skew, and the second-granular window
   `crates/ply-eval/src/compiled.rs`'s test-module header records. **Read that
   as `ply` only.** `std --show` exists on no other binary, so `ply-corpus` and
   `w6-alloc` — which embed `crates/ply-corpus/ply/{w4,w5}.ply` — get arms 1 and
   3 and not this one. Measured 2026-08-27: appending a line to `w4.ply` and
   backdating its mtime leaves `binary-is-current.sh target/release/w6-alloc`
   reporting `current`, exit 0, while the binary holds other bytes. The script
   now prints `NOTE  no content check for …` on those two rather than passing
   silently, which labels the gap; closing it needs a `--show` equivalent on the
   corpus binary, not more `find`. ADR 0019's `w6-alloc` row is the measurement
   this bounds.
3. **Cargo's inputs, which rustc never sees** — `Cargo.toml`, `Cargo.lock`,
   `rust-toolchain*` and `.cargo/config.toml` appear in no `.d` file and are
   checked explicitly. A newer `.rs`/`.ply` in a depended-on crate that the
   dep-info does not list is reported as `SUSPECT`: usually a new file no `mod`
   declares yet.

Timestamps compare to whole seconds and **equality counts as stale**, because a
false STALE costs one rebuild and a false "current" costs a measurement.

`--self-test` is the part worth copying. Both instruments are driven red and
green on every run — a corrupted *copy* of the stdlib, a dep-info naming a file
newer than the binary, one naming a file that no longer exists, and one naming a
file that really is older — and neither arm touches the worktree. A freshness
check nobody has watched fail is this repository's signature defect one level
up.

Arms 6 and 7 were added on 2026-08-27 because arms 1-5 test the instruments and
not the tool. They exercise the assembled verdict end to end. The corruption
that made the case: rewriting `check_depinfo … || rc=1` to `… || true` in
`verdict_for` left `--self-test` **green**, while the tool printed
`NEWER crates/ply-std/ply/http.ply` and then `current` on the next line and
exited 0 — a false green in the freshness check, which is the exact shape this
section exists to prevent. Arm 6 now goes red on it. `check_cargo_inputs` and
`check_unlisted` still have no self-test arm.

**What else is embedded, established by measurement rather than by reading.**
`grep -rn 'include_str!\|include_bytes!\|include!(' --include='*.rs' .`:

| what | where | into |
| --- | --- | --- |
| 8 stdlib modules | `crates/ply-std/src/lib.rs:46-81` | every binary linking `ply-std`: `ply`, `ply-corpus`, `w6-alloc` |
| `w4.ply`, `w5.ply` | `crates/ply-corpus/src/{w4.rs:52,w5.rs:53}` | `ply-corpus`, `w6-alloc` |
| five `ply-eval` sources into their own test modules | `region.rs:317`, `region_kind.rs:1085`, `explore.rs:1817`, `sim.rs:1727`, `sched.rs:2014` | `ply-eval`'s lib test binary |
| `machine.rs` | `crates/ply-eval/tests/determinism_audit.rs:944` | that test binary |
| `examples/ledger.ply` | `crates/ply-hash/tests/audit.rs:702,711` and `crates/ply-test/src/bisect/delta_tests.rs:357` | those test binaries — and note this one is **outside `crates/`**, so even a `find crates` widened to `*.ply` would miss it |

No `include_bytes!` and no `include!` anywhere. **No cargo build script either,
and this is a trap worth naming**: `find . -name build.rs` returns three files —
`crates/ply-cli/src/commands/build.rs`, `crates/ply-corpus/src/build.rs`,
`crates/ply-eval/src/build.rs` — and all three are ordinary modules. No
`Cargo.toml` in the workspace carries a `build =` key, so the count of real
build scripts is **zero** and that `find` is three false positives.

**Everything under `cargo` is safe; only a pre-built binary is exposed.**
Measured, not assumed: `touch crates/ply-std/ply/http.ply` and nothing else,
then `cargo build -p ply-std`, prints `Compiling ply-std` — cargo reads the same
dep-info and rebuilds on a `.ply` edit. So a figure that came from
`cargo test …` or `cargo run …` cannot be stale in this way. A figure that came
from `./target/release/<bin>`, or from a script that does not build first, can.

Which scripts build before they run, checked by reading them:

| builds first | does not |
| --- | --- |
| `examples/serve.sh:155`, `benches/run.sh:20`, `spikes/ply-lexer/run.sh:12` | `examples/same-tests.sh` (§"Things known to be broken" item 2), `spikes/ply-lexer-nesting/bench.sh`, `spikes/ply-lexer-rc/bench.sh`, `spikes/ply-lexer-throughput/bench.sh`, and `crates/ply-codegen-spike/src/main.rs:558`, whose `--served` denominator shells out to `target/release/ply` and checks only that the file **exists** |

**Which published measurements are exposed to this**, listed so a reader knows
which numbers carry the risk. **This is not a claim that any of them is
wrong** — none has been re-taken here, and ADR 0020 §1's were re-taken three
times by two parties on a clean binary and survived. It is a claim about what
their provenance does not rule out.

| measurement | why it is exposed |
| --- | --- |
| ADR 0020 §4.1's shipped-`json.ply` series (0.03/0.07/0.22/0.79 s at k = 1,000–8,000) | the highest-exposure figure in the record: it runs `crates/ply-std/ply/json.ply` itself, through a pre-built binary, and `json.ply` is one of the eight embedded modules |
| ADR 0020 §1's `ply test spikes/ply-lexer/lexer.ply` and the 33-file agreement corpus | `./target/release/ply`, pre-built. §0 is the account of this exact hazard being caught on this exact run |
| ADR 0020 §3.1, §5.2 and §6.1, and `spikes/ply-lexer/GAPS.md` §1 and §13 | taken with `./target/release/ply` and `harness/target/release/plydump`, neither of them built by the command that used them. §6.1's `ply check examples/` row (0.21 s user cold, 0.03 s warm) is exposed twice over: it is a pre-built binary *and* the programs it checks import all eight `std` modules. `plydump` depends only on `ply-syntax` and `ply-span`, so it is exposed to a stale lexer and not to a stale stdlib |
| `spikes/ply-lexer-nesting/bench.sh`, `-rc/bench.sh`, `-throughput/bench.sh` outputs | `PLY=${1:-../../target/release/ply}` with no build |
| `benches/README.md` §"What `regions` adds"'s `179 + 1 + 2 + 2 + 2 = 186` group sizes | `./target/release/ply test examples/ --explain --no-cache`, pre-built, over `examples/` — which imports all eight `std` modules |
| ADR 0019's `1,082 → 773.4` allocations per `/health` | `./target/release/w6-alloc --repo . --requests 200`, pre-built |
| ADR 0018 §0.5 and §"The kernel ratio", and `benches/adr0018-mcts.json` | `crates/ply-codegen-spike`'s `mcts` is itself built by cargo, but its `--served` rung starts `target/release/ply` as a subprocess |

**Pre-registered, and quoted here so the choice of instrument is not a choice
made after seeing the answer.** Written to
`/tmp/ply-r2-instrument/PREREGISTRATION.md` before any of the numbers above
existed — outside the repository, because two round-1 branches each committed a
root-level `PREREGISTRATION.md` and collided:

> **M2.** Statistic B: whether each of the eight `crates/ply-std/ply/*.ply`
> paths appears in `target/release/ply.d`. Decision rule, fixed before the
> number exists: if B is *all eight present*, dep-info is declared the correct
> instrument and `CONTRIBUTING` is corrected to name it; if B is *any absent*,
> dep-info is declared insufficient and the corrected rule must union it with an
> explicit `find(1)` over `*.ply`. No run is discarded.
>
> **M3.** The workstream succeeds only if the arm that edits a stdlib `.ply`
> without rebuilding shows the old rule **clean** and the new check **stale**,
> and the arm after the rebuild shows the new check **current**. If the old rule
> already reports stale, the premise is refuted and the refutation is reported
> instead of a fix.

B came back *all eight present*, so the first branch was taken. Every figure in
this section is deterministic — file counts, path counts, exit codes, verdict
strings — so N = 1 and the command beside each is what reproduces it; no wall
clock is claimed anywhere in it.

**Not exposed, and worth stating so the list is a partition rather than a
warning.** Everything in ADR 0019 rendered by `cargo test -p ply-corpus
--release …` — the allocation attribution, the arity table, `size_of::<Value>()`
— because cargo rebuilds. `benches/w6-ladder*.json` and
`benches/w6-spike*.json`, taken through `cargo run --release -p ply-corpus` and
`benches/run.sh`, which builds. And `benches/kernel/mcts.ply` and `work.ply`
import no `std` module at all, so ADR 0018's kernel numbers are exposed only to
a stale interpreter, never to a stale stdlib.

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

For a diagnostic code and for a variant of a covered enum, that test is now
mechanical: `crates/ply-span/tests/armed.rs` fails if nothing in production
constructs it. It does not decide reachability — see §"The shape it keeps
taking" for what it reaches and what it does not — so for anything shaped like
W1's footprint check, the paragraph above still stands on its own.

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
literal number. `every_registered_code_has_its_published_number` asserts both
that no code moved and that no two constants share a number.

> **This section said adding to only one place fails the test.** The withdrawn
> sentence, verbatim: *"Add to both places, or the test fails — which is the
> intent."* It was false in the direction that mattered. That test iterates the
> *table*, so a constant added to `codes` and to no row was checked by nothing —
> and one had been sitting that way: `REFERENCE_CYCLE` (`W0610`), out of numeric
> order between `W0608` and `W0609` in the module, 83 constants against 82 rows.
> Corrected 2026-08-27 by adding the missing row, which moves no number, and by
> `the_code_registry_table_is_total_over_the_codes_module` in
> `crates/ply-span/tests/armed.rs`, which makes the sentence true going forward.

Three things a new code has to satisfy, each of them checked by a named test:

1. a `pub const` in `codes` **and** a row in the registry table —
   `the_code_registry_table_is_total_over_the_codes_module` for the row,
   `every_registered_code_has_its_published_number` for the number;
2. a `Diagnostic::error(codes::NAME, ..)` or `Diagnostic::warning(codes::NAME, ..)`
   somewhere in production source —
   `every_registered_code_is_constructed_in_production` — or a row in that
   file's `UNARMED_CODES` giving the reason it is reserved and unraised, which
   is where `E0435` and `E0438` are;
3. if it reaches the constructor through a wrapper instead of literally, the
   wrapper goes in `CODE_INDIRECTION` with a reason —
   `every_diagnostic_constructor_call_names_its_code_literally`. There is
   exactly one today, `Lexer::error` in `crates/ply-syntax/src/lexer.rs`, and it
   is why `E0002 UNTERMINATED_STRING` is armed: with that list empty the check
   reports `E0002` dead, and `E0002` is not dead.

Ranges in use: `E0001`–`E0002` and `E01xx`–`E05xx` for errors (73 codes; the two
single-digit ones are the generic pair and are easy to miss when you assume the
range starts at `E01xx`), and `W0601`–`W0610` for warnings. There is also a
reserved list in `crates/ply-eval/src/host.rs` (`DB_SCHEMA_MISMATCH` is at
`:1106`) naming codes a *handler* may not answer with. A code appearing in
`codes`, in the registry, and in that list is **still raised nowhere** — that is
exactly `E0435`'s situation, and that list is the reason a file-granularity grep
calls `E0435` live. The reserved list is itself a real, armed restriction
(`is_reserved_code`); it just is not a raise. Nothing has to be remembered here
any more: `every_registered_code_is_constructed_in_production` fails on a code
in that position unless `UNARMED_CODES` says why.

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

`docs/adr/` is **thirty-one** files, `00NN-slug.md`, with **no index** — the
numbers are the ordering. Pick the next free one — and **nothing you have to
remember decides this any more**: `ply-span:armed:no_two_adrs_share_a_number`
fails when two files share a number, so the check catches you rather than a
reader finding an ambiguous `ADR NNNN` citation months later.

> **The count has now been stale a fourth time (2026-08-31), which is the whole
> argument for the test below it.** It read *"`docs/adr/` is **twenty-nine**
> files"* while the directory held **thirty**; ADR 0031 makes it thirty-one. The
> *advice* has been right since the test arrived and the *count* beside it has
> gone wrong every time an ADR landed — so read the number as prose and
> `ls docs/adr/*.md | wc -l` as the answer.

> **Corrected again (2026-08-30), and this time by a test rather than by better
> advice.** The line above read *"Number yours `0028` and up, and read the open
> pull requests before you pick: counting the directory is necessary and is not
> sufficient."* That was itself the correction to *"count the directory"*, which
> had failed when two branches wrote `0022`. It then failed the same way: two
> branches wrote `0027`, because **both were created before either pull request
> existed**, so there was nothing to read. Advice that assumes a contributor can
> see the other contributor's work is not advice this repository can follow —
> its changes are written in parallel worktrees that cannot see one another.

> **Corrected by the list index, 2026-08-30 — the third time this line has gone
> stale, which is the point the block below is making.** It read:
>
> > `docs/adr/` is **twenty-two** files, `00NN-slug.md`, with **no index** — the
> > numbers are the ordering. Number yours `0027` and up, and read the open pull
> > requests before you pick: counting the directory is necessary and is not
> > sufficient.
>
> The *count* had drifted by four while the *advice* stayed right, which is the
> more dangerous of the two failure modes: a reader who trusts "twenty-two" and
> does not run `ls docs/adr/*.md | wc -l` picks `0023` and collides with four
> landed ADRs. `docs/adr/0027-a-list-index.md` is this change's, so the next
> author wants `0028` — and should still run the count rather than trust this
> sentence, for the reason it has now been wrong three times.

> **Corrected by W4 round 2, which is the case the old wording could not
> survive.** It read:
>
> > Number yours `0023` and up. (This read "seventeen" and "`0018` and up", then
> > "nineteen" and "`0020` and up"; it has now been wrong twice for the same
> > reason, which is that nothing counts the directory for you —
> > `ls docs/adr/*.md | wc -l`. Run it.)
>
> The count was right and the advice still failed, because two round-1
> branches each wrote a `docs/adr/0022-*.md` without seeing the other —
> `0022-the-call-ceiling.md`, open as PR #34, and `0022-record-update.md`,
> which this branch renumbered to `0023-record-update.md`. So this tree holds
> twenty-two files with a **deliberate gap at `0022`** until #34 lands, and the
> next author wants `0024`. Run `ls docs/adr/*.md | wc -l`, then look at what
> the open branches have already claimed: the directory cannot show you a
> number a sibling branch is holding.

ADR 0005 is superseded in part by
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
| `crates/ply-eval/src/code.rs` | `crates/ply-codegen-spike`, which **nothing in the workspace compiles**. It has now bit-rotted this way twice — `Stmt::Expr` becoming a struct variant, then `NodeKind::Lit` widening to `Lit(Lit, Value)` under R4. It builds today, on the default toolchain since the move to cranelift 0.132.3 (this used to read `cargo +1.94.0 test --release`): `cd crates/ply-codegen-spike && cargo test --release`. Run that after touching this file, or the only instrument for pricing codegen stops answering. CI's `spike` job runs exactly that command, so a break is caught at the pull request rather than at the next re-take |
| how a `Value` is built or shared | `crates/ply-corpus/tests/r4_value_construction.rs`, the attribution ADR 0019's thresholds are fractions of. Two traps: it is **about three times slower in debug than release** (70.9s against 25.6s) because it captures a backtrace per allocation, and its rule table is matched against a **three-frame window whose contents differ by profile** — a rule verified only in release can leave the same allocation unattributed in debug and fail the residue ceiling there. Check both. ADR 0019 §6 is the worked example |
| the request path | `benches/w6-ladder.json` and the two integrity tests, and the M9 verdict that reads it. Also `README.md`'s one guarded sentence — re-take it with `./target/release/w6-alloc --repo . --requests 200`, which reads **773.4** on this tree |
| `Value::cmp`, `values_equal`, or how a `Map` key is stored | the four guarantees the note on `ply_eval::Map` lists. `cmp` is deliberately **coarser** than rendering at `Decimal` (`1.50m` and `1.5m` are one key and two strings), so a key is reduced to one representative per class by `ply_eval::value::canonical_key` before it is stored — `ply_eval::value::insert_key` is the single site, and adding a second one re-opens a defect that made `map_keys` a function of insertion history for four milestones. Any new coarseness in `cmp` needs a matching arm there. `map_order.rs`, `value_semantics_audit.rs` §5 and `derivation_determinism_audit::a_decimal_keyed_map_encodes_one_body_whichever_spelling_was_written_last` are what fail; `docs/adr/0019-value-representation.md` §7 is the write-up |
| `collect_refs_inner` in `crates/ply-core/src/infer.rs` | the compiled seam's effect gate, silently. It is one walk answering two questions — the names a body mentions, and whether the body is written with `perform` or `handle` — and `Checker::mark_internal_effects` propagates the second to a fixpoint over the first. Widen the name set and definitions stop being enterable; narrow it and a definition that performs becomes enterable, which is `CONTRIBUTING.md` item 11 again. The `match` is exhaustive with no wildcard on purpose, so a **new** `ExprKind` fails to compile here rather than defaulting to "pure" — do not add a `_ =>` arm |
| the `Builtin` enum in `crates/ply-eval/src/builtins.rs` | **four checks at once, by omission.** `every_builtin_is_reachable_by_the_name_it_reports`, `exactly_the_callback_builtins_are_higher_order`, `tests::every_builtin_checks_its_argument_count` and `region_kind::tests::the_callback_builtins_are_the_six_this_module_knows` all *iterate* `Builtin::all()`, so a variant left out of `all()` is never named and therefore never checked by any of them — the suite stays green over a builtin nothing has looked at. Deleting `Builtin::ListAt` from `all()` was run against the reachability test on exactly that assumption and it stayed green. `builtin_all_is_complete_and_lists_each_name_once` was written for it; it pins the whole name list, so adding a builtin means adding its name there, and that is the point rather than the cost |
| `Builtin::arity()` | **an arity that is too *wide*, and nothing else.** ~~Nothing that a well-typed program meets~~ — corrected 2026-08-30: `builtins::call` reads `b.arity()` on every call (`builtins.rs:558`; `region_kind.rs:1086` and `value.rs:169` read it too), so an arity *narrower* than the truth reddens every test that calls the builtin, at run time. `every_builtin_checks_its_argument_count` asserts the *declared* arity is enforced, not that it is right — giving `list_at` an arity of `(2, 3)` leaves it green, because `(2, 3)` still refuses one argument and still refuses four and no well-typed call can reach the third slot. `assert` and `range` are both `(1, 2)` over schemes of 1 and 2 arguments, i.e. the table has already drifted twice in exactly that direction. Pin the argument count where it bites, which is a `ply-core` test that a call with the wrong number of arguments does not check |
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

   > **True of this crate, and routinely re-quoted as a fact about cranelift,
   > which it is not. Measured 2026-08-30.** The 1.94.0 floor is a property of
   > the version pinned three lines up in `Cargo.toml`. From the crates.io
   > index, `rust-version` per release: `cranelift-jit` `0.132.0`–`0.132.3`
   > declare **1.93.0**; 1.94.0 first appears at `0.133.0`. `-codegen`,
   > `-frontend`, `-module` and `-native` share the boundary exactly.
   >
   > Run, not read: with `cranelift-jit = "=0.132.3"` on this machine's default
   > `stable` (**rustc 1.93.1** — the version `.github/workflows/ci.yml` pins in
   > six jobs), a probe emits and **calls** native code for the body of
   > `lexer.is_digit`, `is_digit(47,48,53,57,58,97) = [0,1,1,1,0,0]`, whole
   > cranelift stack built clean in 23.86s on aarch64-apple-darwin. Seen to fail
   > first: flipping one expected answer reports the JIT's real output against
   > it.
   >
   > **This does not make the spike buildable on 1.93.1 and no invocation here
   > changes.** Downgrading the pin to `=0.132.3` leaves **11 compile errors, all
   > in `src/jit.rs`** — `ir::MemFlagsData`, eight `iadd_imm_s`/`icmp_imm_s`
   > sites (the `_s` suffix arrived in 0.133), and two `stack_load`/`stack_addr`
   > signature changes. Naming and arity drift rather than missing capability,
   > but not ported, so nothing here says a twelfth is not behind the eleventh.
   > `+1.94.0` remains the invocation for this crate as it is pinned.
   >
   > What the correction is *for* is the inference the record kept drawing from
   > this sentence — that a compiled backend forces the workspace toolchain.
   > [ADR 0026](docs/adr/0026-a-reachable-backend.md) §5's *"One is forced by
   > M9"* is withdrawn there on the same measurement. The cost that does **not**
   > go away: an optional, default-off cranelift dependency still puts 31
   > packages into the shipping `Cargo.lock` and takes `grep -c cranelift
   > Cargo.lock` from 0 to 44, feature **off**, crate excluded from
   > `workspace.members`.
   >
   > > **Both of those numbers were taken on 2026-08-31 and both are exactly
   > > right, which is worth saying because a prediction that survives contact
   > > is rarer here than one that does not.** `crates/ply-codegen` is now a
   > > workspace member and `crates/ply-cli` depends on it: the lock went from
   > > **250 packages to 282** — 31 new dependencies plus `ply-codegen` itself,
   > > 0 removed, diffed against the unmodified lock in the main checkout — and
   > > `grep -c cranelift Cargo.lock` is **44**.
   > >
   > > What did *not* happen is the "optional, default-off" half. There is no
   > > feature flag. The choice was pre-registered with a threshold before the
   > > cost was known — under 60 s of marginal build time means no flag, 60 s or
   > > over means a default-**on** flag with a documented off path — and the
   > > marginal cost came back at **16.26 s wall / 63.01 s user** (min of 3,
   > > release, after `cargo clean -p` of exactly the 32 added packages; null
   > > control, cleaning `ply-codegen` alone, 0.70 s) and **18.55 s / 95.54 s**
   > > on the dev profile `cargo test` uses. Load was 6.4–9.9 throughout, above
   > > this file's own gate, so those are **observations**; the decision is
   > > robust to that because load inflates wall clock and the threshold is 3.7×
   > > above the reading. The reason the flag was never on the table in the
   > > *off* position is this file's §"The one rule": the spike rotted twice
   > > sitting off the default path, and a feature CI ran once a milestone would
   > > be the same mistake with a shorter name.

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

   > **Both figures in this entry are stale and are corrected here rather than
   > rewritten above, 2026-08-28, because the block is a dated re-take and this
   > is the next one.** Whole run captured, from `crates/ply-codegen-spike/`:
   >
   > ```
   > $ cargo +1.94.0 test --release --no-fail-fast
   > tests/entry_cost.rs    0 passed;  3 ignored     # a measurement, not a gate
   > tests/hazards.rs      18 passed;  0 failed
   > tests/mcts_kernel.rs   9 passed;  0 failed
   > tests/mutations.rs    13 passed;  0 failed
   > tests/spike.rs         9 passed;  0 failed
   > ```
   >
   > **49 passed, 0 failed, 3 ignored**, over eight test binaries plus the
   > doc-tests — so "45 tests across 8 targets" is wrong on the count, on
   > `hazards` (16 → 18), on `mutations` (11 → 13), and it lists no
   > `tests/entry_cost.rs` at all. `crates/ply-eval/src/compiled.rs`'s policing
   > table already said 13 for `mutations`, so two documents disagreed about one
   > crate and that one was right — which is this entry's own closing paragraph
   > happening again, one re-take later.
   >
   > **And the crate is clippy-error-clean.** `cargo +1.94.0 clippy
   > --all-targets` exits **0** with **zero errors and ten warnings** — five
   > `arc_with_non_send_sync`, one unused import (`SLACK`,
   > `tests/entry_cost.rs:54`), one `collapsible_match`, one
   > `useless_conversion`, one `useless_vec`, one `type_complexity`. `grep -c
   > not_unsafe_ptr_arg_deref` over the whole log is **0**, and the only
   > suppression in `rt.rs` is `#![allow(clippy::missing_safety_doc)]`. The "13
   > errors" figure is quoted a second time in `.github/workflows/ci.yml`'s
   > `spike` job as its reason for building and testing but not linting; that
   > reason no longer holds and is corrected there too.
   >
   > What the crate is *for* is now decided rather than pending:
   > [ADR 0026](docs/adr/0026-a-reachable-backend.md) §4.7 honours ADR 0016
   > §3.5's refusal to promote it and **amends** §3.5's deletion requirement,
   > because R5 already falsified that requirement's stated reason — the seam in
   > `ply-eval` survives `rm -r` — replacing "delete it when W6 closes" with a
   > condition: the spike goes when its eight wrong backends have been
   > reproduced over the `Compiled` doubles in `crates/ply-eval/tests/` and run
   > under `cargo test --workspace`.
   >
   > **The reproduction landed 2026-08-28 and the condition is NOT satisfied.
   > The crate stays, and the reason is a measurement rather than a
   > preference.** Seven of the eight configurations are reproduced over
   > `ply_eval::backend::Reference` — five of them under `cargo test
   > --workspace` in `crates/ply-eval/tests/differential_corpus.rs` at corpus
   > scale, and `exceeds-budget=4` through `ply test` on a corpus built to
   > outrun the machine's bound, with `answers=` standing on its offer count as
   > it always has. The eighth does not survive the move, for a structural
   > reason:
   >
   > | | the spike | the workspace |
   > | --- | --- | --- |
   > | the backend | cranelift, native frames, a fixed stack | a second tree-walker, `stacker`-grown frames on the heap |
   > | `exceeds-budget` unbounded | `fatal runtime error: stack overflow`, **signal 6 in seconds**, reported from outside by `run_guarded` / `Ended::as_disagreement` | **no crash and no report** — the runaway grows the heap instead. Measured: no output and no exit in 45 seconds, killed by `timeout` |
   >
   > ADR 0026 §4.7's condition names *measured sensitivity*, and §7's fifth
   > bullet predicted exactly this — *"The mutation harness may not survive the
   > move … the condition names measured sensitivity and not a test count."* So
   > the spike is still the only thing in this repository that has demonstrated
   > that a backend which ignores its budget entirely would be noticed, and it
   > stays until something else can. What would discharge it is a reporter
   > outside the run — `run_guarded`'s shape, moved into the workspace, with a
   > backend whose runaway actually dies.
   >
   > Everything else the condition asked for is done and is stronger than what
   > it replaces on the axes that moved: the corpus is **1,116 real tests**
   > rather than 2,396 generated cases, and `unoffered` is reported by 901 of
   > them. The per-corruption numbers are in
   > `crates/ply-eval/src/compiled.rs`'s policing table, printed by the tests
   > that take them.

   Consequence, revised: ADR 0016's `11.67x` **is** re-takeable now, and R4 took
   it — `benches/w6-spike-r4.json`, at `11.68×` by the same expression, so the
   `read_line` half did not move. `ROADMAP.md` §"What is next" item 3 is
   unblocked and ADR 0018 §1 is discharged; what item 3 asked for is in
   `benches/adr0018-mcts.json` and `docs/adr/0019-value-representation.md` §5.
   ADR 0016 records the toolchain wall at lines 764–767 and 1105–1106 and is
   otherwise unamended.

   > **The toolchain wall is gone (2026-08-31), and this whole entry can be read
   > in the past tense on that point.** Every `+1.94.0` above was true of
   > `cranelift 0.134.3`. The crate now depends on **cranelift 0.132.3**, whose
   > manifest declares `rust-version = "1.93.0"` — below this workspace's pinned
   > 1.93.1 — so `cargo fmt --check`, `cargo clippy --all-targets` and
   > `cargo test --locked --release` all run here on the default toolchain, and
   > `.github/workflows/ci.yml`'s `spike` job installs 1.93.1 with every other
   > job. 0.133+ declares 1.94.0 and is not the route. Re-taken on 1.93.1 the
   > same day: **49 passed, 0 failed, 3 ignored**, identical per-binary to the
   > 0.134.3 run, and `cargo clippy --release --all-targets` exits 0 with zero
   > errors. The eleven `src/jit.rs` API sites the move needed are listed in the
   > `spike` job's comment.
   >
   > **This entry should not be read as saying the crate is now sound.** It says
   > the crate compiles and its tests pass, and that was always a narrower claim
   > than it looks: no test here runs the agreement corpus, which is red at 42
   > disagreements on both cranelift versions. **Item 18.**
2. ~~**`examples/same-tests.sh` does not build the binary it runs.** It uses
   `target/release/ply` (line 44) with no `cargo build` anywhere.~~ **Fixed
   2026-08-27.** The script runs
   `cargo build --locked --release --manifest-path "$root/Cargo.toml" -p ply-cli`
   itself — `examples/serve.sh:160`'s exact form — and then, on that path and on
   the new `--no-build` path alike, refuses to run against a binary that is
   absent or older than a source listed in `target/release/ply.d`, cargo's own
   dep-info for that binary, and then calls `.github/binary-is-current.sh`,
   which additionally sees a stale `.ply` — `desk.ply` imports all eight `std`
   modules and those are `include_str!`ed into `ply`, so editing one changes
   what this comparison means while moving no `.rs` file. The build is the convenience; the refusal is the
   load-bearing half.

   **`--locked` was not on that line until a second pass the same day**, and this
   entry quoted it without. It read:

   >   The script runs
   >   `cargo build --release --manifest-path "$root/Cargo.toml" -p ply-cli`
   >   itself

   An unlocked build re-resolves `Cargo.lock` when the manifests have moved past
   it, silently, and in CI this build runs *after* `cargo build --locked --release
   -p ply-cli` — so it could rewrite the very file that step had just vouched for
   and leave the locked check worth nothing. Reproduced on this tree with one
   `[[package]]` entry deleted from `Cargo.lock`:

   ```
   $ cargo build --release --manifest-path .../Cargo.toml -p ply-cli
       Finished `release` profile [optimized] target(s) in 0.37s      # exit 0
   # and Cargo.lock is byte-identical to the pre-deletion copy again:
   # the build put the entry back without being asked

   $ cargo build --locked --release --manifest-path .../Cargo.toml -p ply-cli
   error: cannot update the lock file .../Cargo.lock because --locked was passed
   to prevent this                                                   # exit 101
   # and Cargo.lock is byte-identical to the deleted-entry copy: untouched
   ```

   `examples/serve.sh:160` needed the same flag and now carries it, because
   `same-tests.sh` starts `serve.sh` **twice**: an unlocked build there is an
   unlocked build on the CI path. Checked the same way, with a stub `cargo` on
   `PATH` recording its argv and then running the real one: `serve.sh` handed it
   `build --release --manifest-path .../Cargo.toml -p ply-cli`, and the stale lock
   came back rewritten. What `--locked` costs is that a tree whose lock has
   genuinely fallen behind must run `cargo build` once before measuring — the
   bargain the `clippy` and `test` jobs already make.

   Seen to fail before it was believed. Binary moved aside:
   `./examples/same-tests.sh --no-build` exits **2** with `no release binary at
   …/target/release/ply`. `crates/ply-cli/src/cli.rs` touched:
   exits **2** with `…/target/release/ply is older than a source it was built
   from:` and the path. Same stale tree without `--no-build`: rebuilds and exits
   **0**, so the guard clears by re-running rather than standing in the way.
   That run took 38.7s of wall clock, which is an **observation** and not a
   figure: it was taken at a 1-minute load average of about 30 against the 4.0
   gate in §"Gate on an idle machine before measuring, not after", the same
   reason the row in §"What CI runs, and what each step is worth" withdraws
   5.63s rather than replacing it.

   `find crates -name '*.rs' -newer target/release/ply` — house rule 6's literal
   form — is deliberately **not** the check. It walks `ply-corpus`,
   `ply-codegen-spike` and every crate's `tests/`, none of which is in this
   binary's graph. A guard that fires on an edit which cannot change the binary,
   and that rebuilding cannot clear, gets commented out. What the dep-info form
   does **not** catch is an edit and a build inside the same second;
   §"A moving tree invalidates a correctness number" records that trap for
   cargo's own fingerprints, this inherits a weaker form of it, and the script's
   comment says so rather than claiming the instrument problem is closed.

   **How big that graph is is no longer written down.** This paragraph carried a
   figure that the script's comment carried too. It read:

   >   the dep-info is 152 files across twelve crates and holds none
   >   of those three

   That was true when it was written and it is true today; it was also a number
   that moves the day a crate is added, asserted here and in the guard's own
   comment in `examples/same-tests.sh`, and checked by nothing. The script now **counts** it, from the same file it was
   already reading, and prints what it counted before step 1. On 2026-08-27, on
   the machine in `docs/ONBOARDING.md` §Provenance, that line read:

   ```
   instrument: 152 sources across 12 crates in target/release/ply.d, none newer than the binary
   ```

   which is a transcript of a run and not a figure to keep true.

   Derive the same two numbers by hand:

   ```
   sed -n '1s/^[^:]*://p' target/release/ply.d | tr ' ' '\n' | grep -c .
   sed -n '1s/^[^:]*://p' target/release/ply.d | tr ' ' '\n' |
     grep -o '/crates/[^/]*/' | sort -u | wc -l
   ```

   and the "holds none of those three" half, which is `0`:

   ```
   sed -n '1s/^[^:]*://p' target/release/ply.d | tr ' ' '\n' |
     grep -c 'ply-corpus\|ply-codegen-spike\|/tests/'
   ```

   Counting it closed a hole and not only a staleness risk. The loop had **no
   floor**. Cargo writes `target: src src ...` on line 1 of a dep-info; a first
   line that names no sources parses to an empty list, and a loop over an empty
   list finds no stale file and pronounces fresh whatever binary you hand it.
   Seen: with that line emptied, the round-1 script printed nothing about the
   instrument at all and went straight into step 1; the script as it stands exits
   **2** with `... named no sources, so nothing was compared against ...`. The
   check is `sources >= 1` — a floor, like step 1's `passed >= 1` — never an
   equality against 152, which would turn the script red the day a module is
   added.

   One thing this entry never said and should not be read as saying: CI was not
   the exposure **for the binary**. `.github/workflows/ci.yml` has run
   `cargo build --locked --release -p ply-cli` immediately before the script
   since the job existed. That exposure was the **local** run, which §"If your
   change touches" told you to work around by hand; that row is corrected too,
   and so is the CI comment, whose stated rationale — "the script's own
   requirement" — is no longer why the step is there.

   CI *was* exposed on the other axis, and that is the `--locked` finding above:
   until both scripts took the flag, the job's one locked build was followed by
   three unlocked ones — `same-tests.sh`'s own, and `serve.sh`'s twice — any of
   which could have re-resolved the lock the locked step had just checked.
   Nothing is known to have happened. This is a hole that was open, not a run
   that went wrong, and it is recorded as the former.

   **No timing figure was taken for any of this.** Every statistic above is an
   exit status, a `cmp` of two copies of a file, or a count derived from a file:
   deterministic, N=1 per case, pre-registered before it had a value, in a file
   written outside this repository. `uptime` before the first probe read

   ```
   22:13  up 64 days,  5:39, 9 users, load averages: 8.47 7.30 10.23
   ```

   and after the last, `10.14`, against this project's gate of 4.0 — so the two
   withdrawn wall clocks, `5.63s` in §"What CI runs, and what each step is worth"
   and `4.6s` in `docs/ONBOARDING.md` §4, stay **withdrawn**. Neither was
   re-taken, neither was replaced by a guess, and the end-to-end run behind them
   was re-run only for its **29 requests** and its exit **0**, which are counts
   and not clocks.
3. ~~**`examples/same-tests.sh` step 1 can be vacuous.** It passes

3. **`examples/same-tests.sh` step 1 can be vacuous.** It passes
   `--no-incremental`, which disables only the front-end cache; on a warm
   `examples/.ply-cache` it prints `0 failed, 0 passed, 68 cached` and the script
   exits 0. `--no-cache` is the flag that forces the run.~~ **Fixed 2026-08-27,
   and the diagnosis was right on both halves.** Re-measured on this tree, one
   warm `examples/.ply-cache`, the two flags back to back:

   ```
   $ ply test examples/desk.ply --no-incremental   # the withdrawn step 1
   0 failed, 0 passed, 68 cached (0.00s)           # exit 0
   $ ply test examples/desk.ply --no-cache         # what step 1 passes now
   0 failed, 68 passed, 0 cached (0.10s)           # exit 0
   ```

   Step 1 passes `--no-cache`. The flag alone is not the fix, though: a step
   that trusts an exit status is one flag away from being vacuous again, so step
   1 now **reads its own counts** and refuses on them — `cached == 0` and
   `passed >= 1`, never `passed == 68`, so adding a test cannot turn the script
   red. Seen to fail: with the flag reverted to `--no-incremental` on a warm
   cache the script exits **1** with `step 1 served 68 test(s) from the result
   cache`; with one `test` block in `examples/desk.ply` falsified (`4.00m` →
   `4.01m` at `:2017`) it exits **1** at step 1 on `1 failed, 67 passed, 0
   cached`, and exits 0 again once the file is restored from a byte copy and
   `cmp`-verified.

   The counts line is printed by `print_summary` at
   `crates/ply-cli/src/commands/test.rs:1016`, not by
   `ply_test::RunReport::summary` (`crates/ply-test/src/report.rs:220`) — the
   two build the same shape and nothing pins either. So the guard **aborts**
   when the line does not parse rather than skipping: a check that quietly
   stopped matching would be this same defect one layer up.
4. ~~**`README.md`'s `ply-corpus gen` invocation is missing the required
   `--out`** and fails verbatim.~~ **Fixed** at `README.md:97`, along with the
   missing positional corpus argument on the `ply-corpus bench` beside it.
   `benches/README.md` always had the correct form.
5. ~~**`examples/serve.sh` claims `--db-schema` refuses with `E0435`.**~~
   **Fixed, twice.** It claimed that and nothing raises `E0435`; the comment at
   `examples/serve.sh:37-54` now records the claim, the grep that refutes it, and
   the error you actually get. That 2026-08-17 pass then **missed the widest
   instance of the same claim**: `--db-schema`'s own help text at
   `crates/ply-cli/src/db.rs:538-541` told everyone who ran `ply run --help` that
   the schema is "diffed against the live database at bind time so a mismatch is
   `E0435` before anything runs". The `serve.sh` comment had even *named* that
   line, and filed it as "prose describing the check as future work" — it was
   neither prose nor future work but shipped interface text, and reading a doc
   comment as a comment rather than as UI is how it survived a pass looking
   straight at it. Corrected 2026-08-30, to agree with `schema_line`
   (`db.rs:1035`), the refusal note (`db.rs:877`) and the `Declared` state's own
   comment (`db.rs:734-738`). Kept in this list because the *code* gap — no
   schema check at bind time — is still open; only the false claims were closed.
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
7. ~~**No `LICENSE` or `LICENSE-APACHE` file exists** although `README.md:499`
   and the workspace root `Cargo.toml:22` declare `MIT OR Apache-2.0`.~~
   **Fixed 2026-08-27.** `LICENSE-MIT` and `LICENSE-APACHE` are at the
   repository root, and `README.md`'s `## License` section names both. The two
   manifest claims agree and always did — `Cargo.toml:22` is
   `license = "MIT OR Apache-2.0"` and `README.md` says the same expression — so
   both files were owed and both were written, not one chosen.

   Provenance, because an invented licence text is worse than none: the Apache
   text is byte-identical to three independent copies in
   `~/.cargo/registry/src/index.crates.io-*/` (`async-channel-2.5.0`,
   `either-1.15.0`, `scoped-tls-1.0.1`), **zero** differing lines, APPENDIX
   retained with `Copyright [yyyy] [name of copyright owner]` as the published
   template. Repeat that check against a crate picked at random and read the
   result carefully: `syn-2.0.87`, `serde_json-1.0.151` and `rand-0.9.2` ship
   the same 176 lines with the 25-line APPENDIX dropped, so they differ by
   exactly that block and nothing else. `once_cell-1.21.4` is a fourth copy that
   keeps it and matches this file byte for byte. The MIT text differs from
   three independent copies
   (`scoped-tls-1.0.1`, `either-1.15.0`, `bit-set-0.8.0`) on **exactly one**
   line: those three carry a copyright line and this file carries none, so the
   text begins at `Permission is hereby granted`.

   > **Withdrawn (2026-08-28): "the copyright line, which reads `Copyright (c)
   > 2026 Skyler Berg`", and the paragraph justifying it — "That copyright line
   > is the one thing in this entry nothing in the tree can check … The holder
   > is inferred from `Cargo.toml:23 repository = …` and the year from the
   > earliest date in the prose (`2026-02-11`). If the holder is an entity, or
   > the work predates 2026, that line is wrong and a human has to say so."**
   > A human said so: the line is removed rather than confirmed. The reasoning
   > was sound and its conclusion was that the value was inferred, which is not
   > a thing this repository asserts.

   **What that costs, stated rather than glossed.** MIT's grant conventionally
   names the party granting it, and this text now names nobody, so who licensed
   the work is not stated in the file. `Cargo.toml:22`'s `MIT OR Apache-2.0`
   and the repository URL are what remain. Restoring a holder is a one-line
   edit whenever there is one to name; nothing in the tree checks it either way
   so — it is the only claim in this repository whose error has consequences
   outside it.

   ~~`README.md:499`~~ was already a stale reference when this item was
   written: `README.md` is 663 lines — 658 when the item was written, before
   this same change added five below `## License` — and line 499 is
   mid-paragraph about type aliases and wire formats. The licence is at
   **`README.md:656`**. The same stale reference is in `docs/ONBOARDING.md` §9
   item 11 and is corrected there.

   The rest of the entry stands and is **not** fixed: the thirteen member crates
   inherit the expression with `license.workspace = true` rather than each
   carrying the SPDX string, and `crates/ply-codegen-spike/Cargo.toml` still
   declares no license at all. That last one is left alone on purpose — it is
   `publish = false` in its own workspace, adding a key there is a code change
   nobody asked for, and this pass fixed the absent files rather than the
   manifest style.
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
11. ~~**A definition that discharges its own effects publishes an empty row, so
    the compiled seam's purity gate clears it and the machine offers it.**~~
    **Fixed (2026-08-24).** The entry as written is kept below; what was wrong
    about it is the last sentence of the block after it, not the diagnosis.
    Latent
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

    **The fix, and the part of the entry it refutes.** The entry says *"No
    published row can close it"*, and that is true and is why the fix is not a
    row. `ply_core::DefInfo::internally_effectful` is a second published fact —
    "running this can execute a `perform` the row does not show" — and
    `ply_eval::compiled::admit` refuses on it as `Gate::InternalEffects`, a
    variant of its own rather than a second way to reach `Gate::PublishedRow`,
    because a definition this refuses has an *empty* row and folding the two
    would make the row gate's test satisfiable by this one. That is item 13's
    failure mode and it is what this fix had to avoid reproducing.

    **What was checked before it was built, because the obvious form of the fix
    is wrong.** A per-body syntactic bit — "is this body written with `perform`
    or `handle`" — closes the `handled` case in the entry above and leaves the
    hole open one call away. Measured, not reasoned: with
    `fn wrapper(x) = handled(x)`, inference publishes an empty `footprint`
    **and** an empty `performed` for `wrapper`, `wrapper` is written with
    neither keyword, and running it records `state.read` in `ply_eval::Trace`.
    Every fact `wrapper` carries about its own text is a fact a pure definition
    carries. So the bit is transitive over the call graph:
    `Checker::mark_internal_effects` (`crates/ply-core/src/infer.rs`) seeds it
    from `Refs::effects` — set in `collect_refs_inner`'s `Perform` and `Handle`
    arms, in an exhaustive `match` with no wildcard, so a new `ExprKind` fails
    to compile rather than silently answering "pure" — and propagates it to a
    fixpoint over reverse edges.
    `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop`
    holds that at four hops, through a mutually recursive pair whose entry point
    is one member, and through a call reached only from a lambda in a `let`. A
    one-pass propagation passes every other test in the block and fails that one.

    **Polarity, which is the other thing that had to be right.** Every `DefInfo`
    is constructed with the flag **set** and only `mark_internal_effects` lowers
    it, for a definition it positively cleared — so "nothing walked this" and
    "do not enter this" are one answer. `driver.rs`'s `restore_skipped` seeds
    `true` for the same reason: a module gate 1 skipped has no AST, and by gate
    1's own import rule nothing a run can call is restored that way.

    **The corpus half of item 13's third bullet closes with it.**
    `tests/fixtures/self_handled_effect.ply` is the first corpus in the tree
    that declares an effect and discharges it, so `differential_corpus.rs`
    reaches both effect gates over real source instead of over doubles.

    **What is not closed, stated because the entry claims it.** The entry says
    an entered definition *"would tell a user a branch was not taken when it
    was"*. That does not follow and is withdrawn: entering a body can only lose
    atoms **discharged inside** it, an escaping atom is refused by the row gate
    one line earlier, and a discharged atom is in no declared row anywhere — so
    no *declared* atom can go missing this way. The real cost is an
    `observed_footprint` that under-reports a run. And that cost cannot be paid
    today for an unrelated reason: nothing populates a `CausalSlice` at all —
    see item 15. One thing found while checking it **is** a live wrong claim and
    is corrected in place: `slice.rs`'s comment on `CausalSlice::observed` said
    the observed atoms are *"a subset of the declared footprint"*, and they are
    not — both engines record every `perform`, including one a `handle` inside
    the call discharges, and discharging is what keeps an atom out of a row.

    **"Latent rather than live" was right about the outcome and wrong about the
    reason, and the reason matters.** The entry says *"Nothing stops it today
    except that the only backend in the tree refuses `handle` at compile time"*.
    On this tree what stops it is the **argument shape**. `examples/desk.ply`
    has **11** definitions that are this defect — `desk.under` is
    `handle { .. } with { signal.stopping() -> false }` under an empty published
    row, and ten more reach it — so the corpus a reader is meant to learn from
    is full of them, not just a spike fixture. They are never offered because
    they take and return records, lists and closures, and `Gate::ArgumentShape`
    precedes both effect gates. Measured: with `differential_corpus.rs`'s
    tree-walking backend over every corpus in the tree except the new fixture,
    the counters read **18,772 entered / 101,567 declined over 1,011 tests**
    with the new gate and **18,772 / 101,567** without it. The gate is free on
    everything that exists here; what it costs is a definition that both
    discharges its own effects and takes scalars, which is what the fixture is.

    > **Narrowed 2026-08-31, when the argument test became a type test.** The
    > sentence *"They are never offered because they take and return records,
    > lists and closures, and `Gate::ArgumentShape` precedes both effect
    > gates"* is now true for one of its three reasons. A record and a list
    > cross this seam: `compiled::Gate::ArgumentShape` carries them and
    > `compiled::Gate::ArgumentType` decides them from the declared type. What
    > still refuses `desk.under` is the **closure** — its second parameter is
    > declared `body: () -> a / {Serving | e}`, so the argument is a
    > `Value::Closure` and the kind gate refuses it with no lookup, ahead of the
    > effect gates exactly as the sentence says.
    >
    > The effect gates are consequently doing more work than they were, which is
    > the point of writing this down rather than leaving the old sentence to be
    > re-quoted. Measured on the same corpus either side of the widening,
    > `PLY_SEAM_CENSUS=1 ply test examples --no-cache -j 1`:
    > `Gate::InternalEffects` goes **54 -> 91** refusals and
    > `Gate::PublishedRow` **385 -> 1,144**. Both gates were reached by more
    > calls, both refused every one of them, and no definition that discharges
    > its own effects became enterable — `tests/fixtures/self_handled_effect.ply`
    > is still refused on the corpus path and
    > `a_definition_that_discharges_its_own_effects_is_in_the_corpus_and_is_never_entered`
    > is still what says so.

    **One thing the fix nearly got wrong, recorded because it was found by
    reading and not by the suite.** `mark_internal_effects` indexes definitions
    by program-wide name, and the first draft let a module's *second* `fn f`
    overwrite the first one's index with one no vector had. `E0105` is reported
    after this pass runs, so that is reachable source, and every
    duplicate-definition fixture in the tree is **pure** — which survives it,
    because `&&` short-circuits the out-of-bounds read. Written with `handle`,
    the same program panicked the checker: `index out of bounds: the len is 1
    but the index is 1`. A user would have got a crash where `E0105` belongs.
    `a_definition_declared_twice_is_a_diagnostic_even_when_one_of_them_handles_an_effect`
    in `ply-core`'s tests is the standing form.

    **It over-approximates and the size is measured.** An edge is any reference
    naming a definition of this program, minus the definition's own parameters —
    those shadow a global of the same name for the whole body. Locals bound
    further in are not resolved away, so a lambda parameter or a `let` binder
    that shadows a definition's name still draws that edge; the error is always
    "refuse something enterable" and never the reverse. On `examples/` the
    parameter subtraction is the difference between 29 refusals and 11, and the
    eighteen were one shape: `desk.item_named(shelf, ..)` folds over its own
    parameter and `desk` also declares `fn shelf`.
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
13. **Three holes in what polices the compiled seam; two of the three are now
    closed.** Recorded together because each is a green result over space
    nothing exercises.

    > **Re-headed (2026-08-24).** This read: *"one of the three is now half
    > closed … The unarmed name gate under the third bullet is fixed and the
    > bullet says so; the other two holes, and the corpus half of the third,
    > are open."* The budget-ignoring backend closed, and item 11's fix closed
    > the corpus half of the third bullet — `tests/fixtures/self_handled_effect.ply`
    > is a corpus that reaches the seam's effect gates. **The first bullet is
    > the one still open**, and it is open on purpose.

    - ~~**`ply test --engine both` cannot install a backend at all.**~~ **Fixed
      2026-08-28**, and the block below is left as it was written because it is
      the record of what was true for four days. What it read is unchanged; what
      it *claims* is withdrawn by the note under it.

      **`ply test --engine both` cannot install a backend at all.** Still true
      and deliberately not fixed here — wiring a backend into the CLI is gated on
      item 9 and on the result-cache rule. What is new is that the claim is now
      written down as an inventory somebody can check rather than as a sentence:
      `crates/ply-eval/src/compiled.rs` §"What polices this seam, and what does
      not" counts it. `Compiled` and `set_compiled` occur **zero** times in
      `crates/ply-cli`, source and tests both; all **42** `set_compiled` call
      sites in the workspace are tests or the spike's own harness (27 in
      `ply-eval/src/compiled.rs`'s own tests, 5 in
      `ply-codegen-spike/tests/hazards.rs`, 3 in
      `ply-eval/tests/differential_corpus.rs`, 3 in
      `ply-eval/tests/equivalence_audit.rs`, 2 in
      `ply-codegen-spike/tests/mutations.rs`, 2 in its `src/measure.rs`). So the
      shipping CLI catches **zero** of the eight deliberately wrong backends, and
      the rule that a backend run must not populate the result cache is
      **unenforced because it is unreachable**.

      > **The count and its own list were both wrong, corrected 2026-08-28.** It
      > read *"all five `set_compiled` call sites in the workspace are tests or
      > the spike's own harness (2 in `ply-codegen-spike/src/measure.rs`, 5 in
      > its `hazards.rs`, 3 in its `mutations.rs`, 27 in
      > `ply-eval/src/compiled.rs`'s own tests, 2 in
      > `ply-eval/tests/differential_corpus.rs`)"* — a parenthetical summing to
      > 39 introduced by the word "five", with `ply-eval/tests/equivalence_audit.rs`
      > missing and two of the five per-file figures wrong.
      > `grep -rn '\.set_compiled(' --include=*.rs` counts 42. **The claim the
      > count decorates is unaffected and was re-checked one file at a time: all
      > 42 are tests or the spike's harness, and the CLI still installs
      > nothing.** [ADR 0026](docs/adr/0026-a-reachable-backend.md) §1.1 carries
      > the re-take; §4.1 decides that a backend *is* reachable and §4.5 makes
      > catching these eight from a shipping command the condition on any backend
      > shipping at all, which is what closes this bullet.

      > **Closed 2026-08-28, by building it.** `ply test --backend <spec>`
      > installs one: `reference` is a backend that answers correctly —
      > `ply_eval::backend::Reference`, a second tree-walker over the
      > carried-signature fragment, **not** a code generator and no cranelift in
      > `Cargo.lock` — and `wrong:<mutation>` is one of the eight, so that a
      > green run with a backend attached can be read as evidence. Under
      > `--engine both` the backend is a **third** engine, compared against the
      > plain machine rather than against the tree-walker, so a divergence
      > reported is the backend's and nothing else's.
      >
      > **The shipping CLI catches seven of the eight configurations. It is
      > seven and not eight, and the eighth is named rather than rounded up.**
      > `crates/ply-cli/tests/backend.rs` is the standing form, 14 tests, and its
      > header carries the table. What escapes is ignoring the budget *entirely*
      > over a **non-terminating** recursion: that is not a wrong answer, the run
      > never comes back, and every candidate reporter is inside the process it
      > took down — measured at no output and no exit in 45 seconds against
      > 0.03s for the run that reports. The spike's `run_guarded` is the reporter
      > that can see it, from outside, and it is still the only one.
      >
      > > **The last two sentences are withdrawn, 2026-08-31, and what withdrew
      > > them is a second backend rather than a better harness.** They read:
      > > *"every candidate reporter is inside the process it took down … The
      > > spike's `run_guarded` is the reporter that can see it, from outside,
      > > and it is still the only one."* The first clause was never quite
      > > right and the second stopped being true: **every** test in
      > > `crates/ply-cli/tests/backend.rs` already runs `ply` as a child, so
      > > the reporter has always been outside the process. What was missing was
      > > a backend whose runaway *dies* rather than hanging.
      > >
      > > `--backend cranelift:wrong:exceeds-budget` over
      > > `fn spin(n: Int) -> Int = 1 + spin(n + 1)` aborts with
      > > `fatal runtime error: stack overflow`, **exit 134, in 0.02 s** — two
      > > runs, release binary — against `reference`'s no-output-and-no-exit in
      > > 45 s over the same corpus. Native frames sit on a fixed stack;
      > > `stacker`-grown frames do not.
      > > `the_unbounded_runaway_dies_under_a_code_generator_and_hangs_under_a_tree_walker`
      > > asserts both arms, so the contrast is checked rather than recalled,
      > > and it asserts the child died **by signal** rather than merely failing
      > > — watched to fail with the fragment forced empty, where `!success()`
      > > alone still held while nine other tests went red.
      > >
      > > So **eight of the eight configurations are now accounted for under
      > > `cranelift`**, against seven under `reference`, inside
      > > `cargo test --workspace`. That is ADR 0026 §4.7's deletion condition
      > > met; the spike is **not** deleted in the same change and §4.7 says
      > > why.
      >
      > Self-tested the way `mutations.rs` self-tests: replacing every
      > `Mutation` with `Mutation::None` in `backend::parse` fails **7 of the
      > 14** and leaves exactly the controls, the gate test and the two
      > cache-rule tests green. Run 2026-08-28.
      >
      > > **"scalar-signature" corrected to "carried-signature", 2026-08-30.**
      > > `compiled::crossable` now carries `Value::Bytes` as well as the two
      > > scalars — a lexer's arguments are `Bytes` and ADR 0026 §3 records the
      > > seam refusing `read_line` on `admit`'s first line — so
      > > `backend::scalar_signature` was renamed `carried_signature` and reads
      > > `Int | Bool | Bytes`. All fourteen tests in
      > > `crates/ply-cli/tests/backend.rs` still pass unchanged, seven of eight
      > > still caught, and the eighth escapes for the same reason. What moved
      > > is what the fragment reaches: `ply test examples --engine both
      > > --backend reference` goes from 51 definitions and **768** entries to
      > > 153 definitions and **62,388**, measured either side on 2026-08-30 by
      > > narrowing the seam back and rebuilding.
      > >
      > > > **Re-taken after the type gate, 2026-08-31, by the same method — a
      > > > binary built from this tree with the gate narrowed back.** `ply test
      > > > examples --no-cache -j 1 --engine both --backend reference` goes
      > > > from **153** definitions in the fragment to **180**, and from
      > > > 55,693 entered of 58,425 offers to **56,379 entered of 60,223**.
      > > > The fragment grew by 27 definitions and the entries by 686, which
      > > > is small beside the front end's 190,617 -> 306,931 and is the
      > > > finding rather than a disappointment: `examples/` is a corpus of
      > > > `String` and `Decimal`, and this widening deliberately did not move
      > > > the leaf set. `crates/ply-eval/src/census.rs`'s header attributes
      > > > it — 108,925 of the 121,642 `Gate::ArgumentType` refusals are
      > > > `String`. All fourteen tests in `crates/ply-cli/tests/backend.rs`
      > > > still pass unchanged.
      > > >
      > > > > **Re-taken again after the ANSWER test, 2026-08-31, and the last
      > > > > sentence above is withdrawn for it.** *"All fourteen tests in
      > > > > `crates/ply-cli/tests/backend.rs` still pass unchanged"* — two of
      > > > > them **failed**, correctly, and that file is now fifteen tests
      > > > > over a changed corpus. `Machine::compiled_answer` decides an
      > > > > answer from the declared **return** type, so `pair(Int) ->
      > > > > List<Int>` moved *inside* the fragment and
      > > > > `Mutation::Unoffered` — which needs a definition that is offered
      > > > > and has no body — lost the only one that corpus had.
      > > > > `label(Int) -> String` replaces it, and the fifteenth test is
      > > > > `wrong:handle`, a ninth wrong backend for the hazard this
      > > > > widening created: a container answer is checked for its kind and
      > > > > not for its contents. On `examples/` the fragment goes **180 ->
      > > > > 220** definitions and 56,379 entered of 60,223 offers to
      > > > > **56,703 of 59,435**; on the ported front end it goes **306,931
      > > > > entries -> 26**, one `items.parse` per file, which is the whole
      > > > > point and is PR #30's shape.
      >
      > **And the result-cache rule is armed, in both of ADR 0026 §4.6's
      > stages, each seen to fail before it was believed.** `cache_bypassed`
      > reads `--backend`, so a backend run on the *default* engine reads
      > nothing from the store — delete that clause and a backend run over a
      > warm cache reports `selected 0 of 5 (5 cached)` and `0 of 0 offers
      > entered`, a green run over a backend that never ran. `ply_test::run_with`
      > records `Record::Backend` for any test with a non-zero native entry
      > count, so nothing is written whatever the flags said — delete that arm
      > and `ply test` reports three `E0505 … entered compiled code, and its
      > pass was written to the result cache` and exits 1. The source half is
      > `crates/ply-span/tests/armed.rs`'s
      > `a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache`,
      > which fires on a **new route** rather than on a wrong answer: adding a
      > `set_compiled` call to `crates/ply-cli/src/commands/run.rs` turns it red.
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
    - ~~**The published-row gate is untestable by every corpus in the tree.**~~
      **Closed (2026-08-24), by item 11's fix.** It read: *"`benches/kernel`
      declares no effect at all and `ply-eval`'s differential corpus declines
      effectful names, so if that gate regresses both corpora report success.
      Unit tests in `ply_eval::compiled` are the only thing that notices.
      **Still open**: no corpus in the tree exercises this gate, and adding one
      means a corpus that declares an effect."* Adding one is what happened:
      `tests/fixtures/self_handled_effect.ply` declares `effect tally`, performs
      both of its operations and discharges both under its own `handle`, so its
      `handled` and `wrapper` are refused by the effects gate on the corpus
      path, and its `measured` publishes a row that is not empty, which is what
      the row gate reads. The row is what the test asserts, not the gate: a
      corpus run counts declines and does not record which gate produced one. Deleting the effects gate and running
      `cargo test -p ply-eval --test differential_corpus` reads **4 passed, 2
      failed** — `a_backend_that_answers_correctly_agrees_over_every_corpus_on_disk`
      and `a_definition_that_discharges_its_own_effects_is_in_the_corpus_and_is_never_entered`
      — with `observed footprint — left {self_handled_effect.tally.read[log],
      self_handled_effect.tally.write[log]}, right {}`. `benches/kernel` still
      declares no effect; that half of the bullet is unchanged, which is why the
      fixture is in `tests/fixtures/`.

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

    **Held open by a test since 2026-08-27, and still not fixed.** The
    disposition call has not been made; what changed is that the gap can no
    longer quietly close by being forgotten. All six unbuilt variants — `Bool`,
    `Panic`, `Runtime`, `UnhandledEffect`, `RecursionLimit`, `Deadlock` — are
    rows in `UNARMED_VARIANTS` in `crates/ply-span/tests/armed.rs`, and
    `no_allowlist_entry_has_outlived_its_reason` fails the moment one of them is
    constructed or removed, so this item cannot go stale in silence. The line
    numbers above have drifted with the file: `AssertionKind` is declared at
    `slice.rs:275` today and `Eq` is built at `:345`. Neither `slice.rs` nor
    anything else in `ply-test` was changed to add the check.
15. **Nothing populates a `CausalSlice`, so `--trace` reports nothing.** Found
    2026-08-24 while checking whether item 11's seam defect could reach a user
    through `ply-test`. It cannot, and neither can anything else: the causal
    slice is the fifth "declared, registered, raised nowhere" this file records;
    §"The shape it keeps taking" counts them.
    `ply_test::SliceBuilder` is constructed in exactly one place in the
    workspace — `crates/ply-test/tests/bisect_audit.rs`, four times, all tests —
    and `grep -rn 'Event::Perform' --include=*.rs` matches only `slice.rs`'s own
    `match` arm and its own unit test. `Attribution::slice` starts `None`
    (`ply-test/src/lib.rs:305`) and its only writer is
    `attribution.resolve(bisection, evidence.slice)` (`diagnose.rs:109`), whose
    `evidence.slice` is `failure.attribution.slice.clone()` (`lib.rs:1331`) —
    itself. Measured rather than read:

    ```
    $ ./target/debug/ply test tests/fixtures/assertion_failed.ply --trace always --json \
        | python3 -c 'import sys,json; d=json.load(sys.stdin); \
            print([ (f["key"], f.get("causal_slice")) for f in d["failures"] ])'
    [('assertion_failed.the running total of the journal', None)]
    ```

    The pipe is there because the field is `null` in a report of a few hundred
    lines and "I did not see it" is not the same reading as "it is null".

    Consequences worth knowing. `ply-cli/src/commands/test.rs:793`'s
    `if let Some(slice) = &failure.attribution.slice` branch — the `ran: a → b →
    c` line and the "the replay did not reproduce this failure" warning — is
    dead. `Tracing`, `--trace auto|always|never`, `SliceBuilder::DEFAULT_CAP`
    and the truncation story are all live code with no producer. And ADR 0004's
    `causal_slice.observed_footprint`, which §"What a failure report carries"
    presents as the answer to "which branch was taken, and which handler
    fired", is `null` in every report the shipped binary has ever written.
    Not fixed here: wiring the tracer is a `ply-test` design call — construct
    the slice, or delete the type and its `--trace` surface — and this change
    was in `ply-eval` and `ply-core`. **One thing that was fixed, because it is
    a claim rather than a gap:** `slice.rs`'s comment on `CausalSlice::observed`
    said the observed atoms are *"a subset of the declared footprint"*, and that
    is false the moment the tracer is armed — `ply_eval`'s `Trace` records every
    `perform` including one a `handle` discharges, so a definition with an empty
    published row contributes atoms to `observed` that are in no declared row at
    all. Measured on the fixture item 11's fix added: `handled` publishes `{}`
    and running it records `{tally.read[log], tally.write[log]}`. Corrected in
    place, with the withdrawn sentence beside it.

    **Held open by a test since 2026-08-27, and still not fixed.**
    `Event::Enter`, `Event::Return` and `Event::Perform` are rows in
    `UNARMED_VARIANTS` in `crates/ply-span/tests/armed.rs`, reached by
    `every_variant_of_a_covered_enum_is_constructed_in_production`. The type
    with no producer is caught only through its variants: there is no general
    "public constructor called only from tests" rule, because for a library
    crate that has a large legitimate-API false-positive surface, so
    `SliceBuilder` and `CausalSlice` themselves are **not enforced**. Nothing in
    `ply-test` was changed.

16. **`spikes/ply-lexer/run.sh` reaches no test at all: its harness does not
    compile.** Found 2026-08-30 while repairing the same defect one directory
    over.

    ```
    $ ./spikes/ply-lexer/run.sh
    error[E0004]: non-exhaustive patterns:
      `&ply_syntax::lexer::TokenKind::Question` not covered
      --> src/lib.rs:66:11
    ```

    `TokenKind::Question` arrived with ADR 0028 and that harness has a `match`
    with no `_` arm — which is the design working, not failing: the build stops
    rather than the comparison going quietly green. What failed is that
    **nothing ran the build**. That spike is in no CI job, and the figures ADR
    0020 §6.1, ADR 0021 and ADR 0022 quote from it — the lexer's throughput, the
    whole basis of §6.2's multiplier — cannot be re-taken until it is fixed.
    Its `lexer.ply` also still has no arm for byte 63, so `?` lexes as an
    error there.

    **Not fixed here**, and the reason is scope rather than difficulty: it is a
    second spike with its own corpus and its own measurement obligations, and
    fixing it under another change's cover would produce exactly the undated,
    unmeasured figures this file's §"The one rule" is about. It is registered
    where CI is configured — `.github/ci-shards.sh`'s `SPIKES_OUTSIDE_CI` — so
    `plan` prints the reason on every run, and moving it to `SPIKE_JOBS` is the
    one-line change that turns the repair into a required check.

17. ~~**`spikes/ply-parser` is in no CI job and its differential is red.**~~
    **Fixed 2026-08-30, and recorded because the prediction was in the spike's
    own README before it happened.** Four language features landed after that
    spike was taken; its harness stopped compiling on two new `ExprKind`
    variants, `tests/fields.rs` began failing on a field the AST had gained, and
    28 of its 763 corpus inputs — **70.2% of the corpus by bytes** — disagreed
    with `ply_syntax`. Nothing said so, because `spikes/` was in no shard, the
    harness declares its own `[workspace]`, and `.github/ci-shards.sh` named
    neither.

    What was done: the comparison moved to `ply_syntax::parse_unexpanded`, a
    new `#[doc(hidden)]` entry point that answers the tree the **grammar** built
    before `effect_set`, `record_update` and `try_op` rewrite it, and the Ply
    parser learned to *parse* `?`, `{..b, f: e}`, named arguments and default
    parameters without expanding any of them.
    `spikes/ply-parser/GAPS.md` §11R.D is the decision, §11R.X what it cost.
    `run.sh --arm` exits 0: 766 inputs, 0 disagreements, 0 tolerances, 22
    mutations armed.

    **And there is a job.** `.github/workflows/ci.yml`'s `parser-spike`, gated
    on `fmt`/`clippy` and named in the `ci` aggregate's `needs:`, watched to
    fail three ways and to stay green on a control first. `ci-shards.sh verify`
    now covers `spikes/` as well as `crates/`, so the *next* spike that nobody
    wires up fails the `plan` job instead of rotting quietly — which is the part
    of this entry that generalises.

    > **Corrected on review, 2026-08-30.** The job does go red three ways and
    > stays green on the control — but *"watched to fail three ways"* was
    > written about the wrong step in two of the three. Re-run: all three
    > corruptions are caught by `test-items.sh`, `run.sh` **exits 1** rather
    > than 101, and the differential is never reached. The job's own comment in
    > `ci.yml` carries the withdrawn text and the measured replacement. Three
    > further defects were found on review and fixed: the job installed the
    > toolchain with **no `components:`** while `run.sh` runs `cargo fmt` and
    > `cargo clippy` (`dtolnay/rust-toolchain` installs `--profile minimal`, so
    > neither binary would have existed and the job could not have passed);
    > `ci-shards.sh verify` never checked that the job it names actually runs
    > *that* spike's `run.sh`; and `run.sh` **exited 0** over a differential
    > that ran no tests at all, watched both ways with `#[ignore]` on the seven
    > tests in `agreement.rs`.

    **What it does not cover, stated because a green job invites the opposite
    reading:** the job runs `run.sh` and **not** `run.sh --arm`. The 22
    mutations cost 299s locally and each re-runs the whole comparison; they stay
    a by-hand obligation and belong in §"The suite proves less than it looks
    like it proves"'s table, which now lists them.

18. **`crates/ply-codegen-spike`'s agreement corpus is red, and
    `cargo test --release` does not say so.** Found 2026-08-31 while porting the
    crate to cranelift 0.132.3, by running the command
    `benches/README.md` §"What `mcts` adds" documents:

    ```
    $ cd crates/ply-codegen-spike
    $ ./target/release/mcts --dir ../../benches/kernel --only agreement
    ...
       DISAGREEMENT  mcts.heap case 87: the boundary carried an argument kind it
       refuses (85 entries and 0 failed bodies became 85 and 1)
       ... and 30 more
    Error: 42 disagreement(s): a faster wrong answer prices nothing
    ```

    **42 disagreements, exit 1.** All 42 are the refused-kind check in
    `src/bin/mcts.rs::verify`, none are `compare_answers` divergences, and the
    run is deterministic — three runs byte-identical, `md5
    c0893d75e378b64339b8ec0746e95220`.

    **This is not the port.** It reproduces exactly on cranelift 0.134.3 built
    with `+1.94.0` from source with no edit in it, and the ported build's output
    is byte-identical to it — same md5, same 42, same per-function entry counts.
    The port is the reason it was *found*, not the reason it is red.

    **What made it invisible.** Three things at once, and each is worth knowing
    on its own. The crate's CI job runs `cargo test --locked --release`, which
    is green — **49 passed, 0 failed, 3 ignored** — because no test in the crate
    runs the agreement corpus; it is a `main`, not a `#[test]`. The published
    figures come from a binary rather than from a run: the `mcts` in the working
    checkout's `target/release/` was built **2026-08-24 15:11**, 33 source files
    under `crates/ply-{codegen-spike,eval,core}/src` are newer than it, and it
    still answers `0 disagreements` and `56,876 entries` — the pre-widening
    number — because it is a binary for source that no longer exists. And
    `.github/binary-is-current.sh`, which exists for exactly this, cannot judge
    it: it reports `UNKNOWN ... no target/release/mcts.d`, because cargo writes
    dep-info into `target/release/deps/` under a hashed name and the `.d` it
    does write for this binary lists only `src/bin/mcts.rs` — not the library
    the binary is mostly made of. `find <sources> -newer <binary>` is what
    answered the question here, which is the shape that script's own header
    calls blind. **Both instruments were wrong in the same direction: toward
    green.**

    **What it blocks.** `mcts` runs agreement before it times anything and
    `bail!`s on the first disagreement — deliberately, "Correctness first" — so
    `--only entries` and the full ladder
    (`--iterations 100 --inner 3 --repeats 21`) both refuse. **ADR 0018 §0.5's
    6.199× cannot be re-taken on any cranelift version until this is fixed**,
    and it was not re-taken on 0.132.3 for that reason.

    **What is NOT in doubt.** The corpus's sensitivity is intact and was
    re-checked rather than assumed: `--mutate off-by-one` takes it to **1,692**
    disagreements over 26 subjects and `--mutate inverted` to **215** over 25,
    against 42 unmutated. So the 42 is a fingerprint with teeth, and the
    byte-identical agreement output either side of the cranelift move is
    evidence about the port rather than a constant.

    Not fixed here. Fixing it means deciding whether `verify`'s refused-kind
    check is right and the backend wrong, or the reverse: the check compares
    `harness.bodies.declines().failed`, which is a **global** counter, against a
    per-function entry count, and every one of the 42 has the entry count
    unchanged and only the global `failed` moving. Its own comment says the
    totals cannot serve — *"a call that raises on a refused kind may
    legitimately have entered other functions first"* — which is the argument
    for `entries_for(harness, &name)` beside it, and the `failed` half did not
    get the same treatment.

Items 9, 10 and 11 are closed; see the block at the end of item 10 and the one
at the end of item 11 for the fixes, the measurements behind them and the tests
that arm them. Items 12, 13, 14 and 15 are open — 12 as fixed-but-listed, 13 in
its first bullet only.

~~Items 2, 3, 4, 6 and 7 are one-line fixes this documentation pass did not make,
because the rule is that code is what shipped and a documentation pass corrects
documents.~~ **Items 2, 3 and 7 were made on 2026-08-27** by a pass that was
allowed to touch code; 4 and 6 stand as written. None of the three turned out to
be a one-line fix. Item 7 is two files whose text had to be checked against three
independent copies each, plus a copyright line no measurement can settle. Items 2
and 3 are a build, a freshness check and a counts check, because the one-line
versions — add `cargo build`, change one flag — would each have left the script
reporting a green result over an exit status nobody had made fail. Item 5's
comment was a document and was corrected; item 1 is a real
code defect and is reported, not fixed. Items 9 through 13 are R5's, found by
the reviews of it: **three of the four review lenses pointed at R5 refuted the
claim they were given**, and the documents they refuted are corrected in place
rather than rewritten. ~~None of 9–13 is fixed. They are open.~~ **That is no
longer true, and the current state is:**

- **9 and 10 are fixed (2026-08-24).** The frame bound was an engine's private
  resource guard rather than semantics; `DEFAULT_MAX_FRAMES` is deleted and
  `Machine::with_max_frames` is an opt-in ceiling no shipping command sets.
- ~~**11 is open.** A definition that discharges its own effects still publishes
  an empty row, so the seam's purity gate still clears it.~~ **11 is fixed
  (2026-08-24).** It still publishes an empty row — correctly, since nothing
  escapes — and the seam no longer reads the row alone.
  `DefInfo::internally_effectful` is a second published fact, transitive over
  the call graph, and `Gate::InternalEffects` refuses on it. The corpus half of
  13's third bullet closed with it.
- **12 is fixed (2026-08-24).** `Ctx::begin` no longer walks the previous entry's
  arena; `Ctx::end` clears it at the end of the entry that filled it, and the
  shrink is amortized over `SHRINK_EVERY` entries.
- **13's third bullet is now fully closed** — the unarmed-gate half by the six
  per-gate tests, the corpus half by `tests/fixtures/self_handled_effect.ply`.
- **15 is open and is not mine to fix.** Nothing constructs a `SliceBuilder`, so
  every `causal_slice` the binary has ever emitted is `null`.
- **13 is two-thirds closed.** The unarmed name gate and the budget-ignoring
  backend that used to take the process down uncaught are both fixed. The first
  bullet — no shipping command can install a backend — is deliberately left
  open, because closing it is gated on the result-cache rule; what changed is
  that it is now an inventory somebody can check.
- **16 is open and is deliberately not fixed here.** `spikes/ply-lexer`'s
  harness does not compile, so its `run.sh` reaches no test. It is registered in
  `.github/ci-shards.sh`'s `SPIKES_OUTSIDE_CI`, which prints the reason on every
  `plan` run.
- **17 is fixed (2026-08-30)**, and the *class* of it is closed rather than the
  instance: `ci-shards.sh verify` now fails on any directory under `spikes/`
  that no CI job runs and no entry excuses, so 16 is visible because of 17's
  fix.

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
