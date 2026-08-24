# R5 timing: what entering compiled code bought, and what it cost

The experiment in `PRE-REGISTERED.md`, run. Read that file first: the arms, the
statistic, the filter, the load threshold and the decision rule were all fixed
before any number here existed, and one of them turned out to be **wrong for the
per-function table** — which is reported below rather than quietly repaired.

Raw report: `mcts-r5.json`. Filter and statistic: `analyze.py`, which is the
only thing that turns that file into the numbers below.

**This is a timing document and it is not the whole R5 result.** Three of the
four reviews of R5 refuted a claim it made, none of them about a ratio; §3 below
is corrected in place because of one of them. `ROADMAP.md` §R5 and
`docs/adr/0018-compute-kernel-performance.md` §0.5 carry all three, and
`CONTRIBUTING.md` §"Things known to be broken" items 9–13 carry them as open.
The load-bearing one for a reader of this file: with a backend attached the
machine **answers where it would have raised** on a body pending more than 100
frames per call, because the seam passes the call budget and cannot express
`DEFAULT_MAX_FRAMES`.

---

## 1. The hot-path tax: zero allocations, 237.87 checks

`./target/release/w6-alloc --repo . --requests 200`, two binaries from the one
frozen tree, arms alternated `H N H N H N`, three runs each:

| window | `HOOK` (ships) | `NOHOOK` (call site deleted) | delta |
| --- | --- | --- | --- |
| 20 requests | 2140.05 | 2140.05 | **0.0** |
| 200 requests | **773.4** | **773.4** | **0.0** |
| 2000 requests | 649.599 | 649.599 | **0.0** |

All six 200-request runs byte-identical, `bytes_per_request` included
(108199.93 both arms). **The hook costs 0.0 allocations per `/health` request**,
and by the pre-registered rule (`HOOK − NOHOOK > 0.0` ⇒ regressed) it is **not a
regression**. R4's 773.4 is also unmoved, so this doubles as R4's staleness
guard.

`NOHOOK`'s `Machine::enter_code` was checked to be **byte-for-byte identical to
`89787d0`'s** before it was built, so the two arms differ in the hook's call site
and in nothing else. In the linked binary the difference is **80 bytes** of
machine code.

**Zero allocations is not zero cost, and the count that says so.** Instrumented
with a counter at the top of `enter_code` (a third build, discarded after; it
read 773.4 itself, so the counter is free), one `/health` request reaches the
hook **237.87 times** at the 200-request window and 222.687 times at 2000. Every
one of those is a **miss**: `compiled_answer` exits on its first line, because
`Compiled`/`set_compiled` appear nowhere in `ply-cli` and no shipping command can
install a backend. So the shipping cost of R5 on the shipping workload is 237.87
predictable branch tests per request, buying nothing, and **no allocation, no
byte, and no reachable behaviour**.

The wall-clock cost of those 237.87 tests was **not** measured — see "what was
not exercised".

---

## 2. The kernel: 6.199x, and it entered 2,162 times

**Load gate.** 1-minute load average immediately before the timed section:
**2.63** against a threshold of 4.5 — **pass**. The harness now samples the load
itself at every window; the first ladder window read 2.52 and all 84 ladder
windows fell between **2.40 and 2.91**, so **no window was dropped by either
pre-registered filter, on any rung**.

**Controls**, both required to land in 0.95–1.05:

| control | result |
| --- | --- |
| `harness_floor` (backend attached to neither arm) | **0.9995x** [0.9718, 1.0084] |
| `control: nothing enterable` (backend attached, everything declines) | **0.9758x** |

**The ladder**, `mcts --dir benches/kernel --iterations 100 --inner 3 --repeats 21`:

| rung | ratio | 10th–90th | interpreter | hybrid | entries/call |
| --- | --- | --- | --- | --- | --- |
| control: nothing enterable | 0.976x | — | 57,306 µs | 58,575 µs | **0** |
| the exploration term | 2.860x | [2.835, 2.871] | 56,994 µs | 19,956 µs | 1,275 |
| + the playout | 6.176x | [6.139, 6.197] | 57,130 µs | 9,242 µs | 2,161 |
| **everything the fragment accepts** | **6.199x** | **[6.143, 6.226]** | 57,329 µs | 9,218 µs | **2,162** |

**Reported: 6.199x, 10th–90th percentile [6.143, 6.226], over 21 of 21 surviving
windows, with 2,162 native entries during the timed run.** Pre-registered
verdict: **`entry-paid-off`** (median ≥ 1.10x, 10th percentile ≥ 1.00x, entries
> 0). Pre-R5 the same rung was **0.998x with 0 entries**.

### The other face of the hook, in wall clock

The `control: nothing enterable` rung is the same kernel with a backend attached
that compiles nothing the search calls, so all **45,586** offered calls decline:
**0.976x, i.e. 2.4% slower, 1,268 µs over 45,586 declines — 27.8 ns per complete
decline.** That is the cost of a *full* decline round trip: every gate, plus the
backend's own lookup, plus the return. It is **an upper bound on, and not a
measurement of, the shipping tax**, which stops at `self.compiled.as_ref()?` and
never reaches any of that.

### The measurement passes ADR 0018's ceiling, and here is why

ADR 0018 §0 puts the Amdahl ceiling at **4.86x**, 5.26x at an infinitely fast
fragment; this run recomputes its own as 4.806x / 5.212x. **6.199x is above
both.** That is not a ceiling being broken by magic — it is the attribution
under the ceiling being incomplete, and the same report contains the number that
shows it:

- interpreted, the search offers **45,586** calls to the hook per
  `mcts.plan_753(100)`;
- with everything compiled it offers **2,266** (2,162 entered + 104 declined);
- so **43,320 interpreted calls per search stop existing**, because they now
  happen inside a native body.

The ceiling's denominator was built by pricing each function's *body* in
isolation (`per_call` subtracts the machine's own entry cost), which charges the
call-site machinery — argument vector, frame push, `Env` binding — to the 19.2%
"unattributed" rather than to any function. Entering compiled code deletes that
machinery too. At the measured machine entry cost alone (0.0989 µs) those 43,320
vanished calls are **4,284 µs of a 57,329 µs search, 7.5%**, and arrival is the
cheapest part of an interpreted call. **ADR 0018's ceiling should be read as an
artifact of a body-only attribution, not as a bound.**

---

## 3. The worst per-function ratio — and a pre-registered filter that was wrong

Pre-registration: report the **worst**, not the mean; a per-function median
below 1.00 is a finding even under a winning aggregate; nothing is dropped for
being inconvenient.

> **Corrected in place (R5 review, 2026-08-22). The section below found the
> right number, disbelieved it, and blamed the wrong thing. Everything under
> this block is left standing verbatim because `CONTRIBUTING.md` §"Correct, do
> not delete" wants the withdrawn claim beside the measurement.**
>
> **What is withdrawn**, quoted:
>
> - *"**I did not identify the mechanism** — only that it is the arm
>   interleaving and not the backend, since one-arm-at-a-time and interleaved
>   disagree by 30x on the same sets in the same process."*
> - *"**6.199x is if anything an underestimate**"*
> - *"**The corrected worst per-function ratio is `mcts.root` at 2.151x**"*
> - *"**no function of the 26 is below 1.00x**"*
> - *"**Nothing in this kernel is too small to be worth entering.**"*
>
> **The mechanism is the backend.** `crates/ply-codegen-spike/src/rt.rs`,
> `Ctx::begin` — the function every entry starts with — runs `slots.clear()`
> and then `if self.slots.capacity() > RETAINED_SLOTS { self.slots.shrink_to(
> RETAINED_SLOTS) }`. Both are O(the *previous* entry's peak arena): the clear
> drops that many `Value`s, and the shrink reallocates. So the cost of an entry
> is a function of the call before it, and the reviewer measured exactly that,
> best-of-7, twice, at two loads — the identical hybrid call
> `mcts.playouts(0,0,0)` timed while only its predecessor varied:
>
> ```
> previous entry's slots:     4     64    184    384   3824   4084  19584
> hybrid (0,0,0), µs:     0.375  0.625  1.041  1.666 13.584 14.458 68.083
> ```
>
> The same call is 0.375 µs or 68 µs — **181x**, monotone in the predecessor, at
> about 3.5 ns per retained slot. The interpreter shows no carry-over on the
> same three predecessors (0.79 / 0.83 / 0.83 µs). That is why `--why` (one set
> repeated, every call preceded by one its own size) and the paired loop (eight
> sets whose costs span 15,617x, cycled) disagree: `--why` hides the effect
> rather than correcting for it. `begin`'s own comment already describes the
> retained buffer — as a *memory* leak — and does not connect it to the time.
>
> **So the pre-registered reading was right and the correction was wrong.** A
> small compiled call following a large one is a real per-call regression, the
> `mcts.playouts` 0.068x row is a measurement of it, and `mcts.root at 2.151x`
> is not a corrected worst — it is the worst row that happens not to follow an
> expensive one. The conservatism argument for the headline goes with it: the
> top rung's hybrid arm does follow its interpreter arm, but the mechanism is
> not the arm order, so nothing here licenses "underestimate". §2's 6.199x is
> unaffected — it was replicated twice by reviewers, at load 5.3–6.8 (6.215x)
> and at load 12–16 (6.240x), and the ladder is filter-independent — but it is
> a **paired** number and no longer carries a sign.
>
> **A second row this table cannot show at all.** The selection loop that picks
> a function's argument sets (`crates/ply-codegen-spike/src/bin/mcts.rs`, §4b)
> skips any set where `harness.interpret_outcome(..).is_err()`. That discards
> every set that *raises* — which is precisely the fuel-decline path, the one
> case where the hybrid is catastrophically slower. Re-measured here on
> 2026-08-22 with the shipped binary:
>
> ```
> $ mcts --dir benches/kernel --probe machine     0.17s user
> $ mcts --dir benches/kernel --probe compiled   11.82s user
>       the fragment was entered 19992 times and declined 10000 of them
>       for running out of fuel
> ```
>
> **~69x slower with a backend attached**, on a program that is about to raise
> either way. ADR 0018 §0 records it (as 7.9 s against 0.11 s) and
> `entry::Declines::out_of_fuel` carries the reasoning; this section does not
> mention it, and the pre-registration's promise that "no per-function result is
> dropped for being inconvenient" does not survive a filter that drops the
> erroring sets by construction.
>
> **What is still true below:** the eight `--why` rows are real and reproduce,
> `mcts.turn` really is 2.2x faster entered on the sets it was timed with, and
> the aggregate ladder is unaffected. What must not be carried forward is any
> claim of the form "no function regressed".

**By the pre-registered rule the worst is `mcts.playouts` at 0.068x** — a 14.7x
regression. **It is an artifact of my own filter, and here is the evidence.**

The stall filter discards a window whose interpreter arm exceeds 1.5× the
minimum interpreter arm across windows. That is correct for a rung, where every
window does identical work. It is wrong for a per-function row, where the 21
windows cycle through 8 different argument sets whose costs span **15,617x** for
`mcts.playouts` (0.54 µs to 8,449 µs). The filter therefore keeps **only the
cheapest set's windows** — and those are exactly the windows in which a
sub-microsecond hybrid call is preceded, inside the same window, by a
1,700 µs interpreted one. Measured, on the same sets, one at a time
(`mcts --why mcts.playouts`):

```
   (-6, 254, 49)    interpreted 8229.3 µs   hybrid 128.4 µs   64.104x
   (4096,4103,191)  interpreted 1405.0 µs   hybrid  28.8 µs   48.870x
   (28, 62, 1)      interpreted   73.9 µs   hybrid   1.8 µs   42.214x
   (253, 3, 14)     interpreted 1694.9 µs   hybrid  25.8 µs   65.717x
   (0, 0, 0)        interpreted    0.7 µs   hybrid   0.3 µs    2.428x
   (0, 3, 3)        interpreted   22.8 µs   hybrid   0.8 µs   30.333x
   (3, 0, 3)        interpreted  150.5 µs   hybrid   2.8 µs   53.923x
   (3, 3, 0)        interpreted    0.6 µs   hybrid   0.3 µs    1.871x
```

Every set faster, **0 declines of any kind**. Run the *same sets* back through
the paired loop and `(0,0,0)` reads 0.58 µs against 9.75 µs — reproducibly, on a
fresh harness, across all three passes. The inflation tracks **the cost of the
preceding window's interpreter arm**: sets that follow an expensive window are
inflated (sets 1, 2, 4, 7), sets that follow a cheap one are not (sets 0, 3, 5,
6). **I did not identify the mechanism** — only that it is the arm interleaving
and not the backend, since one-arm-at-a-time and interleaved disagree by 30x on
the same sets in the same process.

Two consequences, both of which cut *toward* conservatism in §2: the top rung's
hybrid arm follows its interpreter arm in every window, so whatever this is, it
is charged to the hybrid, and **6.199x is if anything an underestimate**.

**The corrected worst per-function ratio is `mcts.root` at 2.151x**
[2.095, 2.178]. Taking the unfiltered median of all 21 windows — valid, because
each window pairs the *same* argument set on both arms — **no function of the 26
is below 1.00x**, and 21 of the 26 rows have equal-cost sets, so for them the
filter is a no-op and filtered and unfiltered agree to the digit.

| | function | ratio |
| --- | --- | --- |
| worst | `mcts.root` | 2.151x |
| | `mcts.turn` (0.505 µs a call, the cheapest body measured) | 2.207x |
| best | `mcts.rollout` | 35.931x |

`mcts.turn` is the row worth keeping: a 0.5 µs body, five AST nodes, is still
2.2x faster entered than evaluated. **Nothing in this kernel is too small to be
worth entering.**

---

## Provenance

- Tree: `impl/r5-interpreter-entry` at `4fd2d54`, plus this directory and the
  harness additions in `crates/ply-codegen-spike/src/bin/mcts.rs` (per-window
  load sampling, the raw per-window record in the JSON, the per-function table,
  and `--why`). No shipping crate was modified; `crates/ply-eval` and
  `crates/ply-corpus` were edited only to build the `NOHOOK` and counting arms
  of §1 and were restored and checksum-verified byte-for-byte afterwards.
- The spike builds only under **rustc 1.94.0** (`cargo +1.94.0`); cranelift
  0.134.3 requires it. `mcts-r5.json`'s `provenance.rustc` says 1.93.1 because
  it shells out to the default toolchain rather than reporting its own — that
  field is wrong in every report this binary has ever written.
- `mcts-r5.json`'s `provenance.command` names `--out /tmp/r5_kernel.json`, which
  is where the reported run wrote; the file was copied here unmodified.
- **`benches/adr0018-mcts.json` is still the pre-R5 file.** Every number in it
  was taken when the interpreter could not enter compiled code, and its
  `end_to_end` of 0.998x was measured with zero entries. It is not updated here
  because replacing an artifact an ADR quotes is not a measurement decision.
