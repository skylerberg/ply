# ADR 0032 — The two halves together: 10.0× on a kernel, 0.94× on a front end, and the one sentence that predicts which

- **Status.** Accepted.
- **Date.** 2026-08-31.
- **Supersedes nothing. Amends** ADR 0030 §10 and ADR 0031 §5.2, which priced a
  callback-free code generator's ceiling at 2.074× and 2.104× — both taken with
  `ply_eval::backend::Reference` narrowed by `PLY_BACKEND_ONLY`, because no real
  code generator could be put in that position. One can now, and it reproduces
  their entry line to the call while reading a *time* neither could take.
- **Closes** the disagreement PR #64's review opened: *"on that corpus
  `reference` enters 190,617 of 190,703 offers over 69 definitions; `cranelift`
  enters 89,912 of 294,538 over 6. 89,912 is exactly the number ADR 0030 names
  as the pre-`Bytes`-widening `Int | Bool` rung. The code generator's fragment
  on the front end is narrower than the tree-walker's."*

## Why this ADR exists

Two changes landed a day apart and neither was ever run with the other. PR #64
put a cranelift code generator behind `ply test --backend cranelift`; PR #65
closed the seam's fragment, taking admitted body calls from 12.205% to 84.014%
and entries from 306,931 to 26. ADR 0031 §2 then measured the closed fragment
end to end and found the run **1.46× slower**, because the only backend that
could inhabit the widened seam was a tree-walker. Its §5.2 named the way out and
could not take it: *"A cranelift backend as that file stands would decline the
entry this whole line of work opened."*

This document runs the combination. It is the experiment both prior ADRs
describe and neither could perform.

**Pre-registered** at `/tmp/arc-together/PREREGISTRATION.md`, with two
amendments each written before the series it registers, and every registered
prediction — four of them — recorded before its arm was timed. The unblinded
entry lines that motivated each amendment are quoted inside it, so what was
known when each prediction was made is on the record.

## The workloads, checked rather than cited

**W1** — `/tmp/arc-typegate/W1`, ADR 0030's and ADR 0031's corpus:
`spikes/ply-parser`'s six modules re-verified byte-identical to the tree this
sitting (six `identical`), plus `probe.ply` (md5
`eabc0e6ba4012edbe2e2a9263b3e15a4`), whose 13 `test` blocks each parse one
`examples/*.ply`. **13 files, 333,851 bytes**, confirmed by `cat examples/*.ply
| wc -c` this sitting.

```
ply test /tmp/arc-typegate/W1 --no-cache -j 1 --filter probe.parse [--backend ..]
```

**W3** — `ply test benches/kernel --no-cache -j 1`: the three-heap-Nim MCTS and
`work.ply`, 8 tests. ADR 0018's corpus, and the one on which compiled code has
ever paid.

## 1. The seam's widening reached cranelift. Its registry did not.

One of the reviewer's numbers reproduces to the call and the other two have
moved, and which is which is the finding:

```
reference  ·      26 of      26 offers entered ·       0 declined · 413 in the fragment
cranelift  ·   89912 of 2028112 offers entered · 1938200 declined ·   6 in the fragment
```

**89,912 reproduces exactly. The denominator does not: 2,028,112 against the
review's 294,538 — the closed fragment's admitted set.** And `reference` no
longer reads 190,617 of 190,703 over 69 definitions but 26 of 26 over 413, which
is ADR 0031 §1's collapse. So lever 1's type gate *did* reach cranelift, on the
*offer* side, the day it landed: the machine now offers it 6.9× more calls.

**What did not reach it is the *registry*.** Cranelift holds 6 enterable
definitions against `Reference`'s 413, so it declines 1,938,200 of those offers
and enters 89,912 times at the bottom instead of 26 times at the root.

The two registries are two functions, and they had parted company in the one
place nobody looked:

| backend | registration filter | on W1 |
| --- | --- | ---: |
| `Reference` | `backend.rs:630` `carried_signature(types, name)` — lever 1's `CarriedTypes` | 413 |
| `Cranelift` | `ply-codegen/backend.rs:589` `scalar_signature` — `Int \| Bool`, hand-rolled | 6 |

`scalar_signature` predates lever 1 and was never revisited by it.

### It is not a safety gate, and this is checkable

Its own doc says so — *"Necessary and not sufficient, and the machine's boundary
is the authority on both sides anyway"* — and the runtime backs the claim:
`rt.rs:386 rt_unbox_int` on a non-`Int` calls `ctx.fail`, `rt_unbox_bool` the
same, and `backend.rs run` maps a set `failed` to `None`, counting it as
`Declines::failed`. **A wrongly registered body declines; it cannot answer
wrongly.** What `scalar_signature` buys is therefore *time* — *"declining before
the fact is cheaper than declining 120,000 times"* — and a claim about time is
settled by a clock, not by a review.

### How narrow it actually is

`crates/ply-codegen/tests/suite/parser_census.rs`, added by this change, runs the
fixpoint over `spikes/ply-parser`:

```
1075 functions offered, 489 survived the fixpoint, 22 enterable
```

**467 of 489 already-compiled bodies are dropped at registration.** So
`PLY_CODEGEN_REGISTER=all` — measurement scaffolding on `PLY_BACKEND_ONLY`'s
model, off by default, read once per process — registers them all:

```
cranelift, register=all · 495152 of 1049245 offers entered · 554093 declined · 222 in the fragment
```

**That is ADR 0031 §5.2's `Bfo` line — `495152 of 1049245 offers entered ·
554093 declined · 220 in the fragment` — reproduced to the call by a real code
generator**, where §5.2 could only reach it by narrowing the tree-walker with
`PLY_BACKEND_ONLY`. The two instruments agree on the *set* and disagree by two
definitions.

Correctness, taken before any time: 13/13 on W1, 186/186 on `examples --engine
both`, `corruption: nothing`, `fired: 0`, identical under both registries.

## 2. End to end on W1 — every backend arm is slower than no backend

10 blocks of 6, block *k* rotated left by *k* mod 6, so every arm sits in every
position at least once. Min user CPU of 10, `/usr/bin/time -p`, `uptime` on both
sides of every block. Load 3.74 → 3.90 across the series.

| arm | | min user | mean user | min wall | vs `A` | vs Rust |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `A` | no backend | **2.85** | 2.958 | 2.89 | 1.000× | 31.7× |
| `A'` | **null control**, byte-identical to `A` | **2.83** | 3.019 | 2.87 | — | — |
| `C` | `--backend cranelift` (shipping) | **3.04** | 3.250 | 3.07 | **0.938×** | 33.8× |
| `B` | `--backend reference` | **4.17** | 4.663 | 4.29 | 0.683× | 46.3× |
| `D` | `--backend cranelift`, `register=all` | **4.72** | 5.111 | 4.81 | 0.604× | 52.4× |
| `Rn` | Rust front end, cold (`--no-incremental`) | **0.09** | 0.095 | 0.10 | — | 1× |

**The control that shows the measurement is sensitive: `d(A, A')` = 0.702%
against effects of +6.67%, +46.32% and +65.61%.**

This series is also an independent re-take of ADR 0031's, and it reproduces it:
**`B/A` = 1.463× against its 1.46×; `A/Rn` = 31.7× against 31.6×; `B/Rn` = 46.3×
against 46.1×; `Rn` = 0.09 to the tick.** Nothing in ADR 0031 §2 or §4 is
withdrawn.

### The three registered W1 predictions, and how they came out

1. *"`C` beats `B`"* — **held.** 3.04 against 4.17.
2. *"`C` does not beat `A` by more than 10%"* — **held, and understated.** `C`
   does not beat `A` at all; it is 6.67% slower.
3. *"`C/Rn` stays above 25×"* — **held.** 33.8×.

### §4's bar is not met, and it is not close

The pre-registration's bar was *"`C < A` by more than `d(A, A')`"*. `C` is
**above** `A` by 9.5× the control. On a front end, **attaching any backend in
this tree — tree-walker or code generator, narrow registry or wide — makes the
run slower.** The best of them, `C`, gives up 6.67%.

## 3. W3, the kernel — the same code, the opposite result

15 blocks of 5, rotated; min user CPU of 15, more blocks than W1 because the run
is ~0.2 s against a 0.01 s instrument. Load 2.43 → 2.21.

| arm | | min user | mean user | vs `K` |
| --- | --- | ---: | ---: | ---: |
| `K` | no backend | 0.20 | 0.201 | 1.00× |
| `K'` | **null control** | 0.20 | 0.200 | — |
| `Kb` | `--backend reference` | 0.04 | 0.047 | 5.00× |
| `Kc` | `--backend cranelift` (shipping) | 0.03 | 0.030 | **6.67×** |
| `Kd` | `--backend cranelift`, `register=all` | **0.02** | 0.020 | **10.00×** |

`Kc` at 6.67× is **consistent with** ADR 0018's **6.199×**, and is not a
re-take of it: that number carries a CI of [6.143, 6.226] over 2,162 samples,
and this one is a ratio of two figures on a 0.01 s clock, so its own interval is
about [5.7, 8.0]. What it does say is that the kernel multiplier survived the
seam's widening and the move to shipping code — which is the claim that has been
unavailable since the MCTS agreement corpus went red, because `mcts` verifies
before it times and bails. A precise re-take still needs that corpus green.

**Quote the resolution honestly: `Kd` is 2 ticks of a 0.01 s instrument.** All
15 runs read exactly 0.02 and all 15 of `Kc` read exactly 0.03, so the *ordering*
is solid and the *ratio* is coarse — 10.0× should be read as "roughly 8–13×", and
`Kd/Kc` as "roughly 1.5×". A finer instrument is the obvious follow-up and is
not needed for anything this ADR concludes.

## 4. The one sentence, and the fact that it was registered before the arm

> **Widening the registry helps when it lets the machine enter *higher*, and
> hurts when it only adds more *leaves*.**

Registered in Amendment 2 before any W3 arm was timed, together with the
prediction it entails — *"the sign of `Kd − Kc` is opposite to `D − C` on W1"* —
and the statement that if `Kd` were not faster than `Kc`, the explanation is
wrong and W1 needs another. **It held.**

The entry lines are the mechanism, and they are the whole of it:

| | definitions registered | entries | shape |
| --- | ---: | ---: | --- |
| W1, `scalar` | 6 | 89,912 | leaf islands |
| W1, `all` | 222 | **495,152** | *more* leaf islands |
| W3, `scalar` | 25 | 2,974 | leaf islands |
| W3, `all` | 44 | **63** | **63 of 63 offers, 0 declined — the root** |

On W3 the root is compilable, so widening collapses 2,974 shallow entries into
63 at the top: PR #30's 721→1 shape, and ADR 0031 §1's 7,331× collapse, in a
third instance. On W1 the roots are **not** compilable, so widening registered
more *singleton* islands reached from interpreted parents, and each additional
island is one more boundary crossing rather than one fewer.

**A boundary crossing is not free and this is the number that governs.**
`backend.rs run` pays, per entry: a registry lookup, a `RefCell` borrow,
`ctx.begin`, a `Value` clone and an arena push *per argument*, the call, two
post-conditions, a `Value` clone on the way out, and `ctx.end`. For a body the
size of `spine.t_lparen` — a nullary returning a constant — that exceeds what
the machine's own dispatch costs. Entering it 495,152 times is how W1's `D` arm
became 1.66× slower than no backend at all.

## 5. What this says about the gap, and it is not what was hoped

ADR 0031 §3 measured the ceiling of an infinitely fast backend on the closed
fragment at **56.8×** and called the prize *"real and unclaimed"*. It is still
unclaimed and this ADR narrows why.

- The ceiling is **not** reachable by widening a registry. `D` registers 222 of
  the 489 compilable bodies and moves *away* from it.
- ADR 0030 §10's and ADR 0031 §5.2's **2.10× ceiling for a callback-free code
  generator is confirmed as the right target and shown to be optimistic as a
  prediction of *this* generator**: `D` sits at exactly their entry line and
  reads 0.604× rather than 2.10×. Their arithmetic is not wrong — it prices an
  *infinitely fast* backend at those 495,152 entries, and the finding here is
  that at that entry height no real backend can be fast enough, because the
  boundary is paid 495,152 times whatever is on the other side of it.
  **`Bfo`'s 2.10× should be read as a bound that the entry count makes
  unreachable, not as a target.**
- The absolute gap on W1 is unchanged: **31.7× against the Rust front end**, and
  the best backend arm makes it 33.8×.

## 6. Decision

1. **`scalar_signature` stays the default.** It is 1.55× better than `all` on
   W1 and 1.5× worse on W3, and W1 is the bootstrap workload. The pre-registered
   bar for changing it was not met.
2. **`PLY_CODEGEN_REGISTER=all` ships as measurement scaffolding**, off by
   default, read once per process, on `PLY_BACKEND_ONLY`'s model — because the
   arm it enables is the one that produced §4, and a knob that took a finding is
   worth keeping to re-take it.
3. **`crates/ply-codegen/tests/suite/parser_census.rs` ships**, with a floor of 20 on
   the enterable set, set below the 22 it measures over that corpus. (W1 reads 6
   through the shipping command; the corpora differ by `probe.ply`, which changes
   what is reachable. The two numbers are not in conflict and the test asserts on
   the one it actually runs.) `fragment.rs`'s census runs over the standard library plus
   one arithmetic module and cannot see the registry narrowing at all; this one
   is over the bootstrap target, and it is what made a one-line filter visible
   after two ADRs had measured around it.
4. **The next lever is the code generator's constructs, and it is now ranked
   rather than guessed.** From the new census, over `spikes/ply-parser` —
   **586 refusals in total**:

   | refusals | construct |
   | ---: | --- |
   | **275** | *a call to a function outside the unit* — **cascade, not a cause** |
   | 68 | `++` |
   | 58 | a record pattern nested inside the constructor pattern `Ok` |
   | 41 | `fold`, a builtin that calls user code |
   | 35 | `map`, a builtin that calls user code |
   | 30 | a named function used as a value rather than called |
   | 19 | `iterate`, a builtin that calls user code |
   | 18 | a lambda |
   | 6 | a `Decimal` literal, which the fragment has no path for |
   | 5 | a call whose callee is an expression |
   | 5 | a constructor pattern nested inside a list pattern |
   | 4 | a call through a local binding |

   **The first row is 47% of all refusals and is not a construct at all.** It is
   the fixpoint propagating: `closure()` drops a function, and on the next round
   every caller is refused for calling it. So the 311 refusals below it are the
   *roots*, and the 275 above are their blast radius — which is the leverage
   argument for fixing them, and it is why the compiled set sits scattered at
   the leaves instead of connected up to `items.parse`.

   **The top two roots — `++` and nested record patterns, 126 between them — are
   not the callback problem** ADR 0030 §10.2 priced as expensive (breaking
   `Machine::compiled_answer`'s `&self`, moving the `Frame::Call` push above
   `enter`, deleting the `compiled_witness` tripwire). They are plain missing
   lowerings in `jit.rs`. **They are the cheap half of the distance between
   0.94× and the kernel's 10×**, and nothing in `ply-eval` has to move for them.

## 7. What would make this wrong

- **A finer instrument moving W3's ordering.** `Kd` and `Kc` differ by one tick.
  The ordering is 15-for-15 consistent, but a microsecond clock is the honest
  follow-up and could change 10.0× materially even if it cannot plausibly change
  the sign.
- **`d(A, A')` = 0.702% is larger than this project's usual control** (ADR 0031's
  was 0.352%), because the series ran at load 3.7–4.7 rather than 2.8–1.6 — a
  second session was building in a sibling worktree throughout. Every effect
  reported here is at least 9.5× the control, so no conclusion turns on it, but
  a quieter re-take would tighten §2's smallest number.
- **W1 and W3 are two workloads, and §4's sentence is fitted to exactly two
  points.** It was registered before the second, which is what makes it a
  prediction rather than a description, but a third workload with a partially
  compilable root is the test it has not had.
- If a future change gives the code generator `++` and nested record patterns
  and the parser's compiled set *still* does not connect upward, then §4's
  mechanism is right about entries and wrong about what is blocking them.

## Provenance

Pre-registration and both amendments: `/tmp/arc-together/PREREGISTRATION.md`.
Raw series: `/tmp/arc-together/raw.tsv` (W1, 60 timed runs),
`/tmp/arc-together/kernel.tsv` (W3, 75 timed runs). Both carry `uptime` beside
every block. Tree: `main` at 9d699c7 — PR #64 and PR #65 merged — plus this
change.

One harness defect is on the record because it silently produced empty fields
rather than an error: the first series spelled the arm
`/usr/bin/time -p CMD >/dev/null 2>/dev/null`, which silences `time`'s own
stderr along with the command's. It was found before any number was taken from
it, fixed by silencing the command inside a subshell, and the fix was proved to
report a number before the series was restarted. The arm now fails loudly on an
empty measurement rather than writing a blank field.
