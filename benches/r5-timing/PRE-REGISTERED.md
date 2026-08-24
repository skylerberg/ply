# R5 timing: what will be measured, and what will count as an answer

Written **before any number was taken**. R4 lost three sittings to a filter
chosen after the fact and had to throw away a result that would otherwise have
cleared its bar; everything below — the arms, the statistic, the filter, the
load threshold and the decision rule — is fixed here and is not revisited once
data exists. Tree: `impl/r5-interpreter-entry` at `4fd2d54`, `benches/kernel`.

Two measurements. Both are reported whatever they say.

---

## 1. The hot-path tax — taken first, taken unconditionally

**Question.** R5 put a check in `Machine::enter_code` that fires on *every*
interpreted call. Ply's shipping workload is an HTTP request, where ADR 0016
measured 2–5% of the work inside the fragment, so nearly every one of those
checks is a miss that buys nothing. What does the miss cost?

**Currency: allocations per request, not wall clock.** R4 established this is
the linear one — allocation counts diverged 1.0% between window pairs where byte
totals diverged 95.3% — and, decisively, an allocation count does not move with
machine load. So this number is recordable today regardless of what the load
average says, and it is taken before the load gate below is even consulted.

**Command.** `./target/release/w6-alloc --repo . --requests 200`, the window R4
published 773.4 at.

**Arms.** Two binaries from the one frozen tree, differing in one expression:

| arm | what it is |
| --- | --- |
| `HOOK` | the tree as it stands: the hook present, **no backend registered**, which is what every shipping `Machine` in this workspace is |
| `NOHOOK` | the same tree with the `if let Some(value) = self.compiled_answer(closure, &args) { … }` block deleted from `Machine::enter_code` |

`NOHOOK` deletes the *call site* and leaves the `compiled` field, `set_compiled`
and `compiled_answer` itself in the binary. That is the honest comparison and it
is chosen over building `89787d0`: the call site is the entire shipping delta on
this path. Verified by reading the diff before choosing — `argv.rs`'s R5 change
is two `#[cfg(test)]` helpers moving out of a `mod tests`, and `memo.rs`'s is
`is_constant` renamed to `pure_by_published_row` with its visibility widened.
Building `89787d0` instead would drag in those two non-hook edits and a whole
separate codegen of the workspace, and would compare two trees where the
question is about one expression.

**Protocol.** Three consecutive runs per arm, arms alternated `H N H N H N`.
Allocation counting is deterministic, so within an arm all three runs must be
byte-identical; if they are not, the measurement is void and re-taken. Also
taken at `--requests 20` and `--requests 2000`, to check that any delta is flat
in the window rather than growing with it.

**Decision rule.** `hotPathTaxRegressed = true` **iff** `HOOK − NOHOOK > 0.0`
allocations per request at 200 requests. No tolerance band: the quantity is
deterministic, not statistical, so any positive difference is a real cost
charged to a workload that gets nothing back.

---

## 2. The kernel ratio

**Command.** `mcts --dir benches/kernel --iterations 100 --inner 3 --repeats 21`
— the shape `benches/adr0018-mcts.json` was taken at, so the new number replaces
the old one on the same rig.

**Arms.** `Harness` holds two `Machine`s over one program: `machine` with no
backend, `hybrid` with `set_compiled(SpikeBodies)`. One frozen tree; the backend
is the only toggle.

> **Declared deviation from the brief, before data.** The brief says "build both
> arms to separate binaries". I am not doing that, and this is the reason. That
> requirement exists so the arms differ *only* in the backend. Two `Machine`s
> built by the same `Machine::new` from the same AST and the same `CheckOutput`,
> differing only in whether `set_compiled` was called, differ in strictly less
> than two binaries would — two binaries additionally differ in layout, ASLR and
> allocator warm-up. And the arms are alternated **inside each timed window**
> (arm A's `inner` calls, then arm B's, then the ratio), which is the only form
> of this comparison that survives a shared box; two processes cannot be
> interleaved that finely. The cost of the deviation is that a single-process
> confound would be invisible, so two controls are required to come out at 1.00x
> or the run is void — see "Controls" below.

**Statistic.** The median of the 21 per-window ratios of the top rung,
"everything the fragment accepts". **Interval: the 10th and 90th percentile of
the surviving windows** — order statistics, not a normal CI, because 21 paired
ratios on a shared box are not normal and nobody has shown they are.

**Window filter, fixed now.** A window is discarded if either:

1. its interpreter-arm per-call time exceeds **1.5×** the minimum
   interpreter-arm time across the 21 windows (that window stalled), or
2. the 1-minute load average sampled at that window's start is above **4.5**.

If fewer than **11 of 21** windows survive, the ladder is void and re-taken
once. Void twice ⇒ `kernelRecordable = false`.

**Load gate — refuse, do not qualify.** The 1-minute load average is read
immediately **before** the timed section. If it is above **4.5**, then
`kernelRecordable = false`, `verdict = 'refused-machine-busy'`, **no ratio is
reported at all**, and the load average goes in `refusedReason`. Not a
provisional number with a caveat attached: a provisional number survives being
quoted and its caveat does not, which is how a caveat becomes a fact. A gap with
a load average in it survives a hurried reader.

4.5 is not chosen for today's box — it is R4's pre-registered threshold
(`benches/r4-timing/README.md`, `run4.txt`), taken over verbatim on the same
10-core machine so that the number cannot have been picked to fit the load I can
already see.

**Entries gate.** `entriesDuringMeasurement` is read off
`Machine::compiled_counts()` for the reported rung's compiled set. **If it is 0
the ratio is a null result whatever it says** — that is exactly how R4's 0.998x
happened — and the verdict is `inconclusive`. Reported always, win or loss.

**Controls.** Both must land in **0.95–1.05** or the run is void:

- the `control: nothing enterable` rung — a backend attached with nothing in it,
  so every offered call declines. This is the hook's wall-clock cost inside a
  compute kernel, the other face of measurement 1.
- `harness_floor` — the same search on both `Machine`s with the backend attached
  to neither.

---

## What counts as R5 having worked

Fixed now. Reference points: pre-R5 the hybrid was **0.998x** end to end with a
per-window band of 0.979–1.007; ADR 0018's Amdahl ceiling for a backend
enterable from interpreted code is **4.86x**, 5.26x at an infinitely fast
fragment. The ceiling is context for reading the result, **not** a bar.

| verdict | condition |
| --- | --- |
| `entry-paid-off` | top-rung median ≥ **1.10x**, 10th percentile ≥ **1.00x**, and entries > 0 |
| `entry-did-not-pay-off` | entries > 0 and top-rung median < 1.10x — including a win too small to see, and including below 1.0 |
| `inconclusive` | entries == 0, or a control outside 0.95–1.05, or fewer than 11 windows surviving twice |
| `refused-machine-busy` | load gate fails |

**1.10x** is the bar because pre-R5's band already reached 1.007: anything
inside 0.979–1.007 is indistinguishable from R4's null result, and 1.10x is the
first round number clear of it. A result of, say, 1.05x is therefore
`entry-did-not-pay-off` **by rule**, and will be reported as one.

## The worst per-function ratio

The mean hides a regression; the worst does not. Every function the machine can
actually enter is paired-timed on its own — one interpreted call against one
hybrid call, same windows, over argument sets the corpus generates rather than
sets chosen by hand. Reported: the function with the **lowest median ratio**,
and its ratio. A per-function median below 1.00 is a finding and is reported as
one **even under an excellent aggregate**. No per-function result is dropped for
being inconvenient; per-function entry counts are recorded alongside so that a
"ratio" over zero entries is visible as the null it is.

## Two more rules

- **No re-running for a nicer number.** The first run that passes the load gate
  and the controls is the reported run. A second run happens only when the first
  is void under a rule above, and then both are reported.
- **The green-result-over-unexplored-space rule.** This project's signature
  defect, found seven-plus times by its own reviews, is a clean result over
  space nobody visited. Whatever comes out, the report says what was exercised
  **and what was not**.
