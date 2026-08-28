# Pre-registration — W9 "repository hygiene: the licence, and a vacuous script"

Written **before any number was taken**, per `CONTRIBUTING.md` §"Gate on an idle
machine before measuring, not after" ("Pre-register the filter. Write the load
threshold, the statistic and the decision rule down before any data exists").

Worktree: `/Users/skylerberg/.worktrees/ply/w9/hygiene`, at `d88aae5`.
Machine: the one in `docs/ONBOARDING.md` §Provenance. `uptime` at the moment this
file was written, before anything was built or run:

```
20:24  up 64 days,  3:50, 9 users, load averages: 11.26 5.50 4.39
```

That is **above** the load-4 gate this file adopts below. No timing number may be
taken in this state.

## Scope

`CONTRIBUTING.md` §"Things known to be broken" items 2, 3 and 7. Nothing else is
in scope. No evaluation semantics, recursion budget, normalization, inference or
stored type is touched, so **neither `RUNTIME_VERSION` nor `FRONTEND_VERSION` is
bumped**, and this file records that as a decision rather than an omission.

## Assumptions that cannot be falsified from inside the repository

1. **The copyright holder is `Skyler Berg`.** No file in the tree names one:
   `grep -rn -i "copyright\|authors" --include=*.toml --include=*.md .` outside
   `Cargo.lock` returns nothing. The only evidence is
   `Cargo.toml:23 repository = "https://github.com/skylerberg/ply"`. This is the
   one item in this workstream whose error has consequences outside the
   repository, and it is flagged for a human rather than resolved by measurement.
2. **The copyright year is 2026.** The earliest date anywhere in the prose is
   `2026-02-11` (`grep -rho '20[0-9][0-9]-[01][0-9]-[0-3][0-9]' --include=*.md .`
   — `2020-01-01` is a fixture value, not a project date).

## Statistics, run counts, decision rules

### S1 — the licence texts (deterministic, N=1)

*Statistic:* `diff` between each file written and **three** independent copies of
the same canonical text taken from `~/.cargo/registry/src/`.

*Decision rule:* the Apache-2.0 text must be byte-identical to all three, with
**zero** differing lines. The MIT text must be byte-identical to all three except
exactly one line — the `Copyright (c) <year> <holder>` line. Any other difference
means the text was mis-transcribed and the file is not shipped.

### S2 — the build guard, item 2 (deterministic, N=1 per case)

*Statistic:* exit status of `examples/same-tests.sh`, and whether
`target/release/ply` exists after it.

*Decision rule, three cases, each of which must land where stated:*

| case | required |
| --- | --- |
| no `target/release/ply` at all | the script **builds it** and proceeds past line 79; it must **not** die "No such file or directory" |
| binary present, a source file in its own dep-info newer than it, build suppressed | exit **non-zero**, with a message naming the stale file |
| binary present and fresh | exit 0, build is a no-op |

Case 2 is the "seen it fail" obligation (house rule 5). If it exits 0, the guard
is vacuous and the item is **not** fixed.

### S3 — step 1's vacuity, item 3 (deterministic, N=1 per case)

*Statistic:* the counts on step 1's summary line, whose format is
`"{failed} failed, {passed} passed, {cached} cached ({:.2}s)"` at
`crates/ply-test/src/report.rs:220`, and the script's exit status.

*Decision rule, four cases:*

| case | required |
| --- | --- |
| warm `examples/.ply-cache`, fixed script | `0 failed, N passed, 0 cached` with **N ≥ 1**, exit 0 |
| warm cache, guard's flag reverted to `--no-incremental` | exit **non-zero**; the guard fires on `cached > 0` |
| one `test` block in `examples/desk.ply` deliberately falsified | exit **non-zero at step 1** |
| `examples/desk.ply` restored from a byte copy, verified with `cmp` | exit 0 again |

`N` is **not** pre-registered as `68`. `grep -c '^test ' examples/desk.ply` is 68
on this tree, and the guard asserts `N ≥ 1` so that adding a test does not turn
the script red. The observed `N` is recorded as an observation, not a threshold.

Rows 2 and 3 are the "seen it fail" obligation. If either exits 0 the fix is
vacuous and must be reported as such rather than shipped.

### S4 — the end-to-end run (deterministic, N=1)

*Statistic:* exit status, and the request count on the line
`"<n> requests, byte for byte identical between the twin and postgres."`

*Decision rule:* exit 0 and **29** requests, the figure `CONTRIBUTING.md:327`,
`ROADMAP.md:1236` and `docs/ONBOARDING.md:46` all carry. A different count is a
finding to open, per `docs/ONBOARDING.md` §"Your wall clocks will differ. The
counts should not."

### S5 — the script's wall clock (timing; **gated**, N=3)

Only taken if the timing figures at `CONTRIBUTING.md:327` (**5.63s**) and
`docs/ONBOARDING.md:604` (**4.6s**) must be re-taken, because adding a
`cargo build` to the script changes what those figures measure.

*Statistic:* **median of N=3** runs of `./examples/same-tests.sh` on an already
warm cargo target, reporting **wall clock and user CPU** for each run
(`/usr/bin/time -p`, or bash `time`, reporting `real` and `user`).

*Gate, pre-registered:* `uptime`'s 1-minute load average must be **< 4.0**
immediately before the series and immediately after it. Both numbers are recorded
whatever they are.

*Decision rule:* if the gate does not open, the figure is reported as
**`UNMEASURED`** and the two documents get a correction note saying only that the
figure now includes the script's own `cargo build` and predates it — no number is
invented. `CONTRIBUTING.md` §"Say how it was checked, or say it was not" is
explicit that this is the better artifact.

*No run is discarded after the fact.* All three are reported, including any that
look wrong. Load before and after is reported with them.

### S6 — the gates the house rules require regardless (deterministic)

`cargo fmt --all --check` → 0 and silent. `cargo clippy --workspace
--all-targets -- -D warnings` → 0. `bash -n examples/same-tests.sh` → 0.
`cargo test --workspace` is **not** run (house rule 10).

## Exit criterion

Falsifiable, all conjuncts:

1. `LICENSE-APACHE` and `LICENSE-MIT` exist at the worktree root and pass S1.
2. S2 lands in all three rows, **including the non-zero exit in row 2**.
3. S3 lands in all four rows, **including the two non-zero exits**.
4. S4 is exit 0 with 29 requests.
5. `CONTRIBUTING.md` items 2, 3 and 7 are struck with the withdrawn text quoted
   verbatim and the new truth stated — **not deleted**. Falsifier: `grep -F` for
   each original sentence returns 0 hits.
6. Every other document that asserts the old behaviour is corrected in place:
   `CONTRIBUTING.md:248/327/457`, `docs/ONBOARDING.md` §4 and its items 2, 3, 11.
   Falsifier: a `grep -n "does not build for you\|never builds it"` hit that is
   not inside a `~~`, a `>` block or a quoted-and-withdrawn span.
7. S6's gates are 0.

If any conjunct cannot be met, the result is reported as **blocked, with the
reason and the cost to unblock**, not shipped weaker (house rule 12).

## Protocol notes

- `examples/desk.ply` is copied to the scratchpad **before** any deliberate
  break, and restored from that copy and verified with `cmp`, never by retyping
  the edit.
- The script writes `examples/.ply-cache/` into the source tree
  (`CONTRIBUTING.md` §"A moving tree invalidates a correctness number" says so),
  so that directory is expected to appear and is not a contamination finding.
