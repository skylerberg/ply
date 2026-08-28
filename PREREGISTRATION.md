# Pre-registration — "Dead surfaces: investigate and report"

Worktree `~/.worktrees/ply/w5/dead-surfaces-report`, at commit `d88aae5`.
Written **2026-08-28T03:24Z**, before any number in the report exists. Required
by `CONTRIBUTING.md` §"Gate on an idle machine before measuring, not after"
(*"Write the load threshold, the statistic and the decision rule down before any
data exists"*).

## 0. What this workstream is, and what it may not do

It produces exactly one new file, `docs/dead-surfaces-report.md`. It changes no
code, deletes nothing, wires nothing, and corrects no other document — including
the stale claims section 5 records, which are handed on rather than fixed here.
If the report recommends a correction, the recommendation is the deliverable and
the correction is somebody else's change.

## 1. The exit criterion, stated so it can fail

`docs/dead-surfaces-report.md` exists and, for **each of the three surfaces**
(the causal slice, `AssertionKind::RecursionLimit`, the compiled seam), carries
all five of:

1. a **command or grep printed with its output**, taken in this worktree, that
   establishes what is actually there — not a sentence quoting the catalogue;
2. the citation the decision record makes (**ADR 0004 §7** for the slice,
   **ADR 0016 §3.5** for the spike, **ADR 0004 §7**'s `assertion` row for the
   assertion kind) and the sentence a user reading it today would wrongly
   believe;
3. option **ARM** — the files it touches, and for the slice specifically the note
   that `slice.rs`'s already-corrected "subset of the declared footprint"
   comment becomes false again on the other axis once the tracer runs;
4. option **DELETE** — what goes, which documents need correcting in place, and
   the capability genuinely lost;
5. a **recommendation** with a one-line reason and a rough cost in each
   direction.

Plus a fourth section specifying a gate check that **catches E0435**, stated
precisely enough for another workstream to implement without asking a question,
and demonstrated by running it here.

**This fails if** any surface is missing any of the five; if any claim in it is
sourced to the catalogue rather than to a command run here; if the proposed gate
check does not catch E0435 when run; or if any file other than
`docs/dead-surfaces-report.md` and this one is modified. Verified by re-reading
the finished report against this list and by `git status` **run by the parent
agent, not by me** — this worktree runs no git command.

## 2. Statistics, run counts, decision rules

Every statistic below is **deterministic**: a grep count, or the nullity of a
JSON field on a fixed fixture. `CONTRIBUTING.md` §"And know which of your numbers
this even applies to" says the load gate governs wall clock and not this class
(*"Allocation counts, hashes, interleaving counts and seeded replays are
deterministic: they reproduce to the digit on a burning machine"*). Load is
recorded anyway, before and after, per house rule.

**No timing or performance number is part of this workstream.** No ratio, no
throughput, no benchmark. If one ever becomes necessary, it must be added to this
file as a dated amendment *before* it is taken, with its own run count and gate.

### S1 — Does the shipped binary ever emit a `causal_slice`?

- **Statistic:** the values of `failures[].causal_slice`, `failures[].assertion`,
  `failures[].suspects[].ran` and `failures[].suspects[].depth` in
  `ply test tests/fixtures/assertion_failed.ply --json`, under each of
  `--trace never`, `--trace auto` and `--trace always`.

  > **Amended 2026-08-28T03:26Z, before any run.** `--no-cache` is added to
  > every invocation. `CONTRIBUTING.md` §"Things known to be broken" item 3
  > records a step of `examples/same-tests.sh` that reported `0 failed, 0
  > passed, 68 cached` and exited 0 — a green result over a run that never
  > happened, which is this project's signature defect and would be a
  > particularly stupid way to conclude that a field is null. `--no-cache`
  > forces the run; `--no-incremental` would not. Recorded as an amendment
  > rather than an edit because the protocol is what is being kept honest.
- **Instrument:** `target/debug/ply`, built in this worktree. Before the series,
  `find crates -name '*.rs' -newer target/debug/ply` **must print nothing**
  (house rule 6). If it prints anything the series is void and is re-taken after
  a rebuild.
- **Runs:** N = 5 per arm (each invocation is well under 2s), 15 total. Arms
  interleaved in the order never, auto, always, repeated 5 times, so a drifting
  tree shows up as disagreement within an arm.
- **Decision rule, committed now:**
  - If **all 15** runs give `causal_slice == null` and `assertion == null` and
    every `ran`/`depth` null → the report states the four fields are null in
    every report this binary writes, and states it as measured here.
  - If **any** run gives a non-null `causal_slice` → item 15 as briefed is
    **wrong**, the report leads with that, and no ARM/DELETE recommendation is
    made for the slice until the producer is found.
  - If runs **disagree within an arm** → the tree moved under the instrument;
    the series is void, reported void, and re-taken. It is not averaged and no
    run is discarded (house rule 4).
  - The three arms are compared to each other on purpose. If `--trace always`
    and `--trace never` produce byte-identical failure objects, the report says
    the flag changes nothing, which is a claim about the flag the catalogue does
    not make.
- **Recorded:** the exact command, the raw JSON extract, and `uptime` before and
  after the series, in the report.

### S2 — How many registered diagnostic codes are constructed nowhere?

- **Statistic:** over every `pub const <NAME>: &str = "E….."` / `"W….."` in
  `crates/ply-span/src/lib.rs`'s `codes` module, the count of names with **zero
  production construction sites**, where a construction site is an occurrence of
  the name as the first argument to `Diagnostic::error(` or `Diagnostic::warning(`
  in a `crates/*/src` file, excluding `#[cfg(test)]` modules, `crates/*/tests/`,
  doc comments and string literals.
- **Runs:** N = 3 identical invocations of the same script.
- **Decision rule, committed now:** the three counts must be identical or the
  number is void and reported void. **The number is reported whatever it is.**
  If it comes out **zero**, that refutes this workstream's own hypothesis that
  the population is larger than E0435, and the report says so in those words and
  narrows section 4 to E0435 alone. If E0435 is **not** among the names the check
  returns, the proposed gate is wrong and section 4 is rewritten, not the number.
- The script is printed in the report so a reader can re-run it. It is written
  to this worktree under `/private/tmp/.../scratchpad`, **not** into the
  repository, because this workstream may create only one file.

  > **Amended 2026-08-28T03:31Z, after the first S2 number and before the
  > second, and the amendment is the point.** The rule above was insufficient:
  > the first instrument returned **25**, identically on all three runs, and 25
  > is wrong. Three identical runs of a broken instrument are three identical
  > wrong numbers, which is exactly `CONTRIBUTING.md` §"Check the instrument,
  > not just the result". It was caught by spot-checking five of the 25 names
  > against the tree by hand: `E0203 OCCURS_CHECK` is constructed at
  > `crates/ply-core/src/infer.rs:5410` in plain sight. Cause: the script
  > stripped string literals with a regex that mis-parses `r#"…"#` raw strings,
  > which cascaded and deleted **76% of `infer.rs`** (240,286 bytes → 58,210)
  > including the constructor. The number is reported **void**, not discarded —
  > it and its cause appear in the report.
  >
  > **Added to the decision rule, and it is now the first thing run:** a
  > **positive control**. Before any count is believed, the check must be run
  > against a hand-verified list of codes known to be constructed in production
  > — `E0203 OCCURS_CHECK` (`ply-core/src/infer.rs:5410`), `E0109 MODULE_CYCLE`
  > (`ply-syntax/src/resolve.rs:805`), `E0102 UNKNOWN_TYPE`
  > (`ply-core/src/infer.rs:1545`), `E0437 DB_POOL_EXHAUSTED`
  > (`ply-host/src/db/pool.rs:1490`), `E0443 ARTIFACT_INVALID`
  > (`ply-cli/src/artifact.rs:1179`) — and **none of the five may appear in its
  > output**. If any does, the instrument is defective, the number is void, and
  > the report says so with the cause. A **negative control** runs beside it:
  > `E0435 DB_SCHEMA_MISMATCH` **must** appear, or the check does not do the one
  > job the brief set it. Both controls are reported with the number, always,
  > including in the run that passes.

### S3 — Disclosed: numbers already taken before this file existed

Instruction 1 of the brief ordered the brief's own claims verified before
anything was planned, so the greps below were run **before** this file was
written. They are disclosed rather than presented as pre-registered, and each is
re-taken 3 times with the same identical-or-void rule as S2:

- `set_compiled` call sites, by file and by count.
- Occurrences of `Compiled` / `set_compiled` in `crates/ply-cli`.
- Construction sites of `SliceBuilder`, `Event::Perform`, `AssertionKind::*`,
  `Assertion::*`.
- Line numbers of the declarations the brief and `CONTRIBUTING.md` cite.

## 3. What would make the report wrong, written down before it is written

- **A producer exists that the greps missed** — behind a `cfg`, a macro, a
  re-export under another name, or a generic. Mitigation: every "constructed
  nowhere" claim in the report is backed by a grep for the *type* as well as the
  constructor, and by the S1 runtime observation, which cannot be fooled by a
  spelling.
- **The catalogue is right and this pass is reading a stale worktree.** The
  worktree is at `d88aae5` and nothing in it is edited; if `main` has moved, the
  report describes `d88aae5` and says so.
- **A recommendation costed without reading the consumers.** Mitigation: for each
  surface the report lists every consumer site, and the ARM cost includes the
  suite assertions that would go red.

## 4. Execution log — which series have been run, not what they said

Kept so a reader can tell a series that happened from one that was planned. The
values belong in `docs/dead-surfaces-report.md`, not here.

| series | run | outcome |
| --- | --- | --- |
| instrument freshness | 2026-08-28T03:24Z | `find crates -name '*.rs' -newer target/debug/ply` printed nothing; `target/debug/ply` built in this worktree at 20:24 local |
| S1, 15 runs, 3 arms | 2026-08-28T03:25Z | complete; load 8.38 before, 8.38 after; deterministic across all 15 |
| S2 v1 | 2026-08-28T03:29Z | **VOID** — instrument defective, positive control would have caught it; cause recorded in the §S2 amendment |
| S2 v2, 3 runs | 2026-08-28T03:33Z | complete; positive and negative controls both PASS; identical across 3 |
| S3 re-takes | pending | to be re-taken 3× in the report stage |

Load at the close of the planning stage: `20:28 up 64 days, load averages: 4.54
5.75 4.87`.

---

## 5. Amendment 2026-08-28T03:33Z — S4, added **before** the number is taken

Reason for the amendment: section 2's `S1` covers one fixture,
`tests/fixtures/assertion_failed.ply`, which fails an `assert_eq`. Item 14 is a
claim about telling a **runaway recursion** apart from a failed `assert_eq`, and
`S1` says nothing about the recursion side. Rather than reason from the code
alone, the recursion side gets its own runtime observation, on the same terms.

### S4 — Is `failures[].assertion` also null when the failure is a runaway recursion?

- **Program** (verbatim from `crates/ply-cli/tests/failure_classification_audit.rs:219-220`,
  the audit's own `RUNAWAY` constant), written to a scratchpad directory, never
  into the repository:

      fn spin(n: Int) -> Int = spin(n + 1)
      test "spins" { assert_eq(spin(0), 0) }

- **Statistic:** the values of `failures[0].assertion`, `failures[0].diagnostic.code`
  and `failures[0].causal_slice` from `ply test --json --no-cache` run in that
  directory.
- **Instrument:** the same `target/debug/ply`; freshness re-checked immediately
  before the series with `find crates -name '*.rs' -newer target/debug/ply`,
  which must print nothing.
- **Runs:** N = 5 (each invocation is well under 2s). Load recorded before and
  after. The statistic is deterministic — the nullity of a JSON field — so
  `CONTRIBUTING.md` §"And know which of your numbers this even applies to"
  exempts it from the load gate; the reading is recorded regardless, and it is
  high.
- **Decision rule, committed now:**
  - All 5 runs `assertion == null` → the report states that the artifact
    distinguishes a runaway recursion from a failed `assert_eq` **only** by the
    rendered `diagnostic.message`, measured rather than inferred, and that
    constructing `AssertionKind::RecursionLimit` alone would change nothing
    because nothing constructs an `Assertion`.
  - **Any** run `assertion != null` → a producer exists that the greps missed;
    item 14 as briefed is wrong, the report **leads with that refutation** and
    makes no ARM/DELETE recommendation for that surface.
  - Runs disagree → void, reported void, re-taken. No run discarded.
- **Not** measured here: any duration. No timing number is part of this
  workstream (section 2), and the load reading below is why that rule is worth
  having rather than a formality.

Load at the moment of writing this amendment, before any S4 run:
`20:33 up 64 days, load averages: 30.45 15.75 9.05`.

## 6. Execution log, report stage

| series | run | outcome |
| --- | --- | --- |
| instrument freshness | 2026-08-28T03:31Z, and again before S4 | `find crates -name '*.rs' -newer target/debug/ply` printed nothing both times |
| S3 re-take, 3 runs | 2026-08-28T03:42Z | identical 3/3; load 15.89 before, 16.54 after |
| S4, 5 runs | 2026-08-28T03:34Z | complete; `assertion` null 5/5, `diagnostic.code` `E0502` 5/5, `causal_slice` null 5/5; load 49.99 before and after |
| §4 gate check, 3 runs | 2026-08-28T03:44Z | identical 3/3 (`md5` of output equal); positive and negative controls PASS; 2 unarmed, both allow-listed; load 14.20 before and after |
| §4 gate, seen-to-fail | 2026-08-28T03:45Z | four demonstrations on a scratchpad **copy** of `crates/`, never on the worktree: new unarmed const → exit 1; an armed code stripped of its one construction → exit 1; the S2 v1 broken strip restored → 24 unarmed and all five positive controls FAIL → exit 2; the narrow constructor set → E0002 false positive → exit 1 |

Load at the close of the report stage: `20:52 up 64 days, load averages: 11.42
17.86 15.95`. No timing number was taken at any point, per section 2.

Files created or modified by this workstream: `docs/dead-surfaces-report.md` and
this file. Verified here by `find . -newermt '2026-08-27 20:16' -not -path
'./target/*' -not -path './.git*' -type f`, which prints those two and nothing
else; the authoritative check is `git status`, **run by the parent agent** — this
worktree ran no git command.
