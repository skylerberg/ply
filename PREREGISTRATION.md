# Pre-registration — the field-order lint (W0611) and the refcount counter surface

**Written 2026-08-28T03:26:29Z, at 1-minute load 8.76 (`uptime`: `20:26 up 64 days,
3:52, 9 users, load averages: 8.76 6.47 4.93`), on rustc 1.93.1, macOS 15.7.3.**

**No measurement of any kind has been taken at the time this file is written.**
There is no `target/` directory in this worktree (`ls target` → no such file), so
no binary exists and none of the numbers below has been seen. Everything here is
a bar set before the data, per `CONTRIBUTING.md` §"Gate on an idle machine before
measuring, not after" and §"Measure an ADR's motivating claim before accepting
the ADR".

Worktree: `~/.worktrees/ply/w2/field-order-lint`, at `d88aae5`.

---

## 0. Why the primary statistic is not a clock

ADR 0020 §9 records: *"No deterministic counter turned out to exist — `ply run
--json` reports no step, call or allocation count — so wall clock was
unavoidable and user CPU was used as the robust half."* Part B of this work
exists to remove that sentence's premise. So the primary statistic here is a
**count**, and `CONTRIBUTING.md` §"Gate on an idle machine" licenses exactly
this: *"Allocation counts, hashes, interleaving counts and seeded replays are
deterministic: they reproduce to the digit on a burning machine ... Only wall
clock is at risk."*

---

## 1. Primary statistic — deterministic, load-independent

**`in_place = updates_in_place / updates`**, both from `ply_eval::rc::Stats`
(`crates/ply-eval/src/rc.rs:117-139`), taken over one whole program run with
`ply_eval::rc::reset()` immediately before the entry point and
`ply_eval::rc::stats()` immediately after, **engine pinned to `machine`**
(`EngineArg::Machine`, the default — `crates/ply-cli/src/cli.rs:91-96`).

The engine is pinned because it is load-bearing and I checked it rather than
assumed it: `ply_eval::rc::carry` is called from eight sites, and **all eight are
in the machine** — `machine.rs:1035`, `:1092`, `:1122`, `frame.rs:107`, `:142`,
`:263`, `:301`, `handler.rs:208`. `interp.rs` calls it nowhere. Both engines do
reach `builtins::push`, so `updates` moves under either; only the machine can
show the trap.

**Run count: 1 per arm.** Not a hedge — a justification. Evaluation is
deterministic and these counters are read from a thread-local integer, so a
second run cannot differ. **Falsifier for that claim, and it is part of this
pre-registration:** every arm is run **3** times and all three must report
**byte-identical** counter objects. If any arm's three runs disagree, the counter
is not deterministic, the whole oracle is void, and I report that instead of
reporting a ratio.

## 2. The arms, fixed now

Known-quadratic (the lint MUST fire; `in_place` MUST be low):

| id | file | definition |
| --- | --- | --- |
| Q1 | `spikes/ply-lexer-rc/fieldorder.ply:48-49` | `slow_step` — `push` in field 2 of 5 |
| Q2 | `spikes/ply-lexer-nesting/nesting.ply` | `first_step` — call at argument 0 of 2, followed by a variable |
| Q3 | `spikes/ply-lexer-nesting/nesting.ply` | `first_const_step` — argument 0 of 2, followed by a **constant** |
| Q4 | `spikes/ply-lexer-nesting/nesting.ply` | `push_step` — `push` at argument 0 of 2 |
| Q5 | `crates/ply-std/ply/json.ply:588-599` | `escape_runs` — inner `push` at argument 0 of 2 of the outer `push` |

Known-linear (the lint MUST be silent; `in_place` MUST be high):

| id | file | definition |
| --- | --- | --- |
| L1 | `spikes/ply-lexer-rc/fieldorder.ply:52-53` | `fast_step` — `push` in the last field |
| L2 | `spikes/ply-lexer-rc/fieldorder.ply:70` | `bare_step` — `push` is the whole body |
| L3 | `spikes/ply-lexer-nesting/nesting.ply` | `tail_step` — the call is the whole body |
| L4 | `spikes/ply-lexer-nesting/nesting.ply` | `last_step` — the call is argument 1 of 2 |
| L5 | `spikes/ply-lexer-nesting/nesting.ply` | `node` — `push` is the last sub-expression of its record literal |

Every arm is exercised at **n = 200 and n = 400** through its existing public
entry point, one arm per program so the counters are not pooled.

Q5 is measured through `json::encode_string` on a string of k = 200 and k = 400
characters that all require escaping. **`escape_runs` is not repaired in this
worktree**; a separate lane owns that fix and this work needs it unfixed.

## 3. Decision rules — written before the data

**R1 (determinism).** Three runs of each arm produce identical `Stats`. Any
disagreement voids the oracle; report it, do not average it.

**R2 (separation).** For every linear arm L1-L5: `in_place >= 0.90` at both n.
For every quadratic arm Q1-Q5: `in_place <= 0.10` at both n.

**Any arm landing in the open interval (0.10, 0.90) is a REJECT for that arm.**
It is reported with its number, the bar is not moved, and the arm is not
reclassified. If a reject happens, the honest outcome is "the counter does not
separate this shape" and the plan's exit criterion is not met.

**R3 (agreement — this is the criterion that matters).** The set of definitions
`W0611` fires in is **exactly** the set {Q1..Q5} and contains **none** of
{L1..L5}. A miss in either direction is a defect in the lint to be fixed, never
a threshold to be renegotiated. Partial credit is not a pass.

**R4 (scale-invariance).** `in_place` for a given arm at n=400 is within ±0.02 of
its value at n=200. A ratio that drifts with n means it is measuring start-up
rather than the trap.

**R5 (the lint is armed — the anti-vacuity rule).** Per `CONTRIBUTING.md` §"Do
not state a guarantee you have not armed" and the standing house rule that this
project's signature defect is a green over unexplored space: for every new test,
I break the code deliberately and confirm the test goes red, then restore.
Specifically:
- Move `toks:` to the last field of `slow_step` → Q1's lint firing must vanish
  **and** Q1's `in_place` must cross into the linear band. Restore.
- Invert the lint's positional predicate (`last` ↔ `not last`) → the agreement
  test R3 must go red on every one of the ten arms. Restore.
- Delete the `counters` key from the `run --json` object → the counter test must
  go red. Restore.
Each is reported by name, with what the failure said.

## 4. Secondary, corroborating only — wall clock

The primary claim above rests on counters alone and is complete without any
timing. A timing series is taken **only** to demonstrate that the counter
reproduces the shape ADR 0020 §4.1 and §5.2 measured with a clock, so that the
counter can be argued to replace it.

- Statistic: **minimum of N runs**, N = 5 where a run is under 2s, N = 3
  otherwise — `ADR 0020 §9`'s pre-registration, adopted verbatim.
- **User CPU** primary, wall clock reported beside it.
- 1-minute load (`uptime`) recorded immediately **before and after** every
  series.
- **No run discarded after the fact.**
- Gate before measuring, not after: spin until 1-minute load < 4 before the
  first run of a series. If the machine will not go quiet, the series is
  labelled `UNMEASURED` and the raw windows are checked in, per
  `CONTRIBUTING.md`'s instruction that this is a better artifact than a number
  of unknown provenance.
- Decision rule, and it is corroboration only: over one doubling of n, a
  quadratic arm is >= 3.0x and a linear arm <= 2.5x. Failure here does **not**
  fail the work; it is reported as "the clock did not corroborate the counter on
  this machine at this load", because R2/R3/R4 are the criteria.

## 5. Instrument check — mandatory, before any number

Per the standing rule that a stale binary silently invalidated an ADR here once:

```
cargo build --release -p ply-cli
find crates -name '*.rs' -newer target/release/ply    # must print NOTHING
```

Run immediately before each series and the output recorded. If it prints
anything, the series is discarded **before** it is taken, not after.

## 6. What I am not measuring, said in advance

- `ply test`'s counters. Test workers are threads and `rc::COUNTERS` is
  thread-local (`crates/ply-eval/src/rc.rs:164`), so a pooled figure would be a
  function of the scheduler. Out of scope, and the reason is this sentence.
- Any claim about `map_insert`, `bytes_concat` or any container but `List`.
  `builtins.rs:460` and `:472` are the **only** two `note_update` call sites in
  the tree, so `Stats::updates` counts `push` and nothing else today.
- Whether the lint would have caught the trap in `lexer.ply`. Not an arm; not
  claimed.

---

# §7 Results — added 2026-08-27, after the fact

**Nothing above this line was edited after a number was taken.** §1–§6 are the
bars as written before `target/` existed. This section is what happened against
them, including the one arm that rejects.

## Instrument

`cargo build --release -p ply-cli`, then
`find crates -name '*.rs' -newer target/release/ply` printed **nothing** before
each series. Load recorded before and after every series; the highest seen
during a counter series was **24.75**, which does not matter here and is the
point — R1 was checked rather than assumed, and every arm's three runs were
byte-identical.

## R1 — determinism. **Pass.**

`the_counters_are_a_function_of_the_program_and_not_of_the_run` runs each
nesting arm three times and compares `(updates, in_place)` exactly.
`three_runs_of_one_program_report_identical_counters` does the same through the
CLI on the whole `counters` object. No disagreement anywhere.

## R2 — separation. **Pass on the ten spike arms. REJECT on Q5.**

| arm | lint | n=200 | n=400 | band |
| --- | --- | ---: | ---: | --- |
| Q1 `slow_step` | fires | 0/200 = **0.0000** | 0/400 = **0.0000** | ≤ 0.10 ✓ |
| Q2 `first_step` | fires | 0/200 = **0.0000** | 0/400 = **0.0000** | ≤ 0.10 ✓ |
| Q3 `first_const_step` | fires | 0/200 = **0.0000** | 0/400 = **0.0000** | ≤ 0.10 ✓ |
| Q4 `push_step` | fires | 0/200 = **0.0000** | 0/400 = **0.0000** | ≤ 0.10 ✓ |
| L1 `fast_step` | silent | 200/200 = **1.0000** | 400/400 = **1.0000** | ≥ 0.90 ✓ |
| L2 `bare_step` | silent | 200/200 = **1.0000** | 400/400 = **1.0000** | ≥ 0.90 ✓ |
| L3 `tail_step` | silent | 200/200 = **1.0000** | 400/400 = **1.0000** | ≥ 0.90 ✓ |
| L4 `last_step` | silent | 200/200 = **1.0000** | 400/400 = **1.0000** | ≥ 0.90 ✓ |

L5 `node` is a definition the lint must be silent in rather than an entry point;
it is covered by R3's set equality, not by a ratio of its own.

**Q5 `escape_runs` rejects.** Measured through `json::to_string` on a string of
k characters that all require escaping:

| k | updates | in place | **in_place** | copies |
| ---: | ---: | ---: | ---: | ---: |
| 100 | 201 | 101 | **0.5025** | 100 |
| 200 | 401 | 201 | **0.5012** | 200 |
| 400 | 801 | 401 | **0.5006** | 400 |

0.50 is inside R2's open interval (0.10, 0.90), which §3 fixed as a **REJECT**.
Per §3 the bar is not moved and the arm is not reclassified. The number is
reported and the diagnosis is:

`escape_runs` is `push(push(acc, run), escaped)` — **two** pushes. The lint
fires on the inner one only. The outer one receives the list the inner just
produced, which has one owner, so it rewrites in place. `rc::note_update`
carries no span, so `in_place` is a fraction over a whole run and cannot
attribute the two sites separately; a definition with one copying push and one
in-place push reads 0.50 by construction. **The ratio is the wrong statistic for
this shape, and that was not foreseen when §1 chose it.**

The claim that survives is sharper than the band: **copies = k exactly**, at
every size, one per escape, at the one site `W0611` names —
`the_shipped_quadratic_copies_exactly_once_per_escape`. Closing the gap properly
means giving `rc::note_update` the site's `Span` so counters can be attributed
per push. That is named here rather than done.

## R3 — agreement. **Pass.**

Over `fieldorder.ply` the firing set is exactly `{slow_step}`; over
`nesting.ply` exactly `{first_step, first_const_step, push_step}`. `node`,
`keep`, `keep_snd`, `keepl`, `empty`, `one_list`, `fast_step`, `bare_step`,
`tail_step` and `last_step` are all silent. Over `std.json`, `escape_runs`
carries exactly one firing and its span text starts `push(acc,` — the inner
push, not the outer.

## R4 — scale invariance. **Pass.**

Every spike arm moves **0.0000** across the doubling. Q5 moves 0.0006 from
k=200 to k=400 and 0.0019 from k=100 to k=400, both inside 0.02.

## R5 — the corruptions. **All armed; each seen red, each restored.**

| corruption | what failed, and what it said |
| --- | --- |
| move `toks:` last in `slow_step` | firing set went `["slow_step"]` → `[]`; and on the same edit as a standalone program `in_place` went 0.0 → **0.995**, crossing into the linear band |
| invert the positional predicate | firing set became the exact complement, `["fast_step"]`; 3 of 4 oracle tests red |
| delete `counters` from `run --json` | 6 of 9 CLI tests red: *"`ply run --json` has no `counters` key; ADR 0020 §9's premise is back"* |
| drop the reset between engines | *"`--engine both` doubled the counters"*, 400 against 200 |
| apply GAPS.md column 2's fix to `escape_runs` | firings in `escape_runs` 1 → 0 **and** copies 100 → **0** at k=100 — the lint and the counter agree on the fix as well as on the defect |
| un-register `REFERENCE_CYCLE` | *"declared in `codes` but absent from the registry: [\"REFERENCE_CYCLE\"]"* — the historical defect, reproduced |
| add an unregistered code | *"absent from the registry: [\"DUMMY_FORGOTTEN\"]"* |
| drift the two copies of one number | *"`FIELD_ORDER_COPY` is `W0611` in the module and `W0699` in the registry"* |
| drop the `fresh()` guard | `a_push_onto_a_list_this_expression_just_built_is_not_a_firing` |
| lambda body inherits instead of restarting | `a_lambda_body_is_its_own_root` |
| forget that locals shadow definitions | `a_local_named_like_a_definition_is_not_mistaken_for_it` |
| skip the interprocedural fixpoint | `the_growth_summary_is_transitive_and_survives_recursion` |
| block statement treated as final | `a_push_in_a_block_statement_fires_and_the_tail_does_not` |
| binary left operand treated as final | `the_left_operand_of_a_binary_fires_and_the_right_does_not` |

Each of the last six failed **exactly one** test and only the intended one.
`spikes/ply-lexer-rc/fieldorder.ply` and `crates/ply-std/ply/json.ply` were
verified byte-identical after their corruptions were reverted.

## §4's secondary wall-clock series — **not taken**

The 1-minute load sat between 9.5 and 24.8 throughout and never approached §4's
gate of < 4. Per §4 the series is labelled **UNMEASURED** rather than taken at
unknown provenance. It costs nothing: §4 says in advance that it is
corroboration only and that R2/R3/R4 carry the claim, and R2 in particular
separated at 0.0000 against 1.0000, which no clock could have shown more
clearly. The doubling-ratio evidence that a clock would have produced is already
on record in ADR 0020 §4.1 and GAPS.md §1.

## Not measured, as promised in §6

`ply test`'s counters (thread-local, workers are threads), any container but
`List`, and whether the lint would have caught `lexer.ply`'s trap. Additionally
**not** covered: the `rc::reset()` cycle-drain path. `take_cycles()` is drained
before every reset so an `--engine both` run cannot lose the tree-walker's
`W0610`s, but there is **no test**, because no type-correct Ply program can
build a cycle today — `crates/ply-eval/tests/reference_cycles.rs` is the pinned
argument for that. Not enforced; the reason is that sentence.
