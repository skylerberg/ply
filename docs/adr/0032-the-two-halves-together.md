# ADR 0032 — The two halves together, and the one sentence that predicts which

- **Status.** Accepted.
- **Supersedes nothing. Amends** ADR 0030 §10 and ADR 0031 §5.2, which priced a
  callback-free code generator's ceiling with `ply_eval::backend::Reference`
  narrowed by `PLY_BACKEND_ONLY`, **because no real code generator could be put
  in that position.** One can now, and it reproduces their entry line to the
  call while reading a *time* neither could take.
- **Closes** the disagreement the code generator's review opened: **the code
  generator's fragment on the front end is narrower than the tree-walker's**, and
  its entry count is exactly the figure ADR 0030 names as the pre-`Bytes`
  `Int | Bool` rung.

## Why this ADR exists

Two changes landed a day apart and **neither was ever run with the other.** One
put a cranelift code generator behind `ply test --backend cranelift`; the other
closed the seam's fragment, taking admitted body calls from a twelfth to most of
them and collapsing the entry count to one per file. ADR 0031 §2 then measured
the closed fragment end to end and found the run **1.46× slower**, because the
only backend that could inhabit the widened seam was a tree-walker. Its §5.2
named the way out and could not take it: *"A cranelift backend as that file stands would decline the
entry this whole line of work opened."*

This document runs the combination. It is the experiment both prior ADRs
describe and neither could perform.

**Pre-registered** outside the repository, with two amendments each written
before the series it registers, and **every registered prediction recorded
before its arm was timed.** The unblinded entry lines that motivated each
amendment are quoted inside it, **so what was known when each prediction was
made is on the record.**

## The workloads, checked rather than cited

**W1** — ADR 0030's and ADR 0031's corpus: `spikes/ply-parser`'s modules,
re-verified byte-identical to the tree this sitting, plus a generated `probe.ply`
whose `test` blocks each parse one `examples/*.ply`. **The byte count was
confirmed this sitting rather than carried over.**

```
ply test /tmp/arc-typegate/W1 --no-cache -j 1 --filter probe.parse [--backend ..]
```

**W3** — `ply test benches/kernel --no-cache -j 1`: the three-heap-Nim MCTS.
ADR 0018's corpus, **and the one on which compiled code has ever paid.**

## 1. The seam's widening reached cranelift. Its registry did not.

One of the reviewer's numbers reproduces to the call and the other two have
moved, and which is which is the finding:

```
reference  ·      26 of      26 offers entered ·       0 declined · 413 in the fragment
cranelift  ·   89912 of 2028112 offers entered · 1938200 declined ·   6 in the fragment
```

**The entry count reproduces exactly and the denominator does not** — it is now
the closed fragment's admitted set, an order of magnitude larger. And `reference`
no longer enters at the leaves but once per file, which is ADR 0031 §1's
collapse. **So the type gate *did* reach cranelift, on the *offer* side, the day
it landed: the machine now offers it several times more calls.**

**What did not reach it is the *registry*.** Cranelift holds a handful of
enterable definitions against `Reference`'s hundreds, so it declines almost every
offer and **enters at the bottom instead of once at the root.**

The two registries are two functions, and they had parted company in the one
place nobody looked:

| backend | registration filter |
| --- | --- |
| `Reference` | `backend.rs`'s `carried_signature` — the type gate's `CarriedTypes` |
| `Cranelift` | `ply-codegen/backend.rs`'s `scalar_signature` — `Int \| Bool`, hand-rolled |

**`scalar_signature` predates the type gate and was never revisited by it.**

### It is not a safety gate, and this is checkable

Its own doc says so — *"Necessary and not sufficient, and the machine's boundary
is the authority on both sides anyway"* — and the runtime backs the claim:
`rt.rs rt_unbox_int` on a non-`Int` calls `ctx.fail`, `rt_unbox_bool` the
same, and `backend.rs run` maps a set `failed` to `None`, counting it as
`Declines::failed`. **A wrongly registered body declines; it cannot answer
wrongly.** What `scalar_signature` buys is therefore *time* — *"declining before
the fact is cheaper than declining 120,000 times"* — and a claim about time is
settled by a clock, not by a review.

### How narrow it actually is

`crates/ply-codegen/tests/suite/parser_census.rs`, added by this change, runs the
fixpoint over `spikes/ply-parser` and reports how many functions survive it and
how many of those are enterable. **All but a twentieth of the already-compiled
bodies are dropped at registration.** So `PLY_CODEGEN_REGISTER=all` —
measurement scaffolding on `PLY_BACKEND_ONLY`'s model, off by default, read once
per process — registers them all.

**That reproduces ADR 0031 §5.2's `Bfo` entry line to the call, by a real code
generator**, where §5.2 could only reach it by narrowing the tree-walker.
**The two instruments agree on the *set* and disagree by two definitions.**

Correctness, taken before any time: every test green on both corpora, under
`--engine both`, and identical under both registries.

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

| | entries | shape |
| --- | ---: | --- |
| W1, `scalar` | tens of thousands | leaf islands |
| W1, `all` | **half a million** | *more* leaf islands |
| W3, `scalar` | thousands | leaf islands |
| W3, `all` | **dozens** | **every offer entered, none declined — the root** |

**On W3 the root is compilable, so widening collapses thousands of shallow
entries into dozens at the top** — the same shape PR #30 found and ADR 0031 §1
measured, in a third instance. **On W1 the roots are not compilable, so widening
registered more *singleton* islands reached from interpreted parents, and each
additional island is one more boundary crossing rather than one fewer.**

**A boundary crossing is not free and this is the number that governs.**
`backend.rs run` pays, per entry: a registry lookup, a `RefCell` borrow,
`ctx.begin`, a `Value` clone and an arena push *per argument*, the call, two
post-conditions, a `Value` clone on the way out, and `ctx.end`. For a body the
size of `spine.t_lparen` — a nullary returning a constant — that exceeds what
the machine's own dispatch costs. **Entering it half a million times is how W1's
`D` arm became 1.66× slower than no backend at all.**

## 5. What this says about the gap, and it is not what was hoped

ADR 0031 §3 measured the ceiling of an infinitely fast backend on the closed
fragment at **56.8×** and called the prize *"real and unclaimed"*. It is still
unclaimed and this ADR narrows why.

- The ceiling is **not** reachable by widening a registry. `D` registers most of
  the compilable bodies and moves *away* from it.
- ADR 0030 §10's and ADR 0031 §5.2's **2.10× ceiling for a callback-free code
  generator is confirmed as the right target and shown to be optimistic as a
  prediction of *this* generator**: `D` sits at exactly their entry line and
  reads 0.604×. **Their arithmetic is not wrong — it prices an *infinitely fast*
  backend at that entry count, and the finding here is that at that entry height
  no real backend can be fast enough, because the boundary is paid every time
  whatever is on the other side of it.** `Bfo`'s ceiling **should be read as a
  bound that the entry count makes unreachable, not as a target.**
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
3. **`crates/ply-codegen/tests/suite/parser_census.rs` ships**, with a floor on the
   enterable set set below what it measures. (It reads a different count from the
   shipping command, because the corpora differ by `probe.ply`, which changes
   what is reachable; **the two are not in conflict and the test asserts on the
   one it actually runs.**) `fragment.rs`'s census runs over the standard library
   and **cannot see the registry narrowing at all**; this one is over the
   bootstrap target, **and it is what made a one-line filter visible after two
   ADRs had measured around it.**
4. **The next lever is the code generator's constructs, and it is now ranked
   rather than guessed.** The new census ranks the refusals over
   `spikes/ply-parser`, and the ranking is what matters rather than the counts.
   **The largest row is not a construct at all** — it is *a call to a function
   outside the unit*, **cascade rather than cause**: the fixpoint drops a
   function, and on the next round every caller is refused for calling it. So the
   rows below it are the *roots* and that row is their blast radius, **which is
   the leverage argument for fixing them, and it is why the compiled set sits
   scattered at the leaves instead of connected up to the parser's root.**

   Ranked, the roots are: `++`; a record pattern nested inside a constructor
   pattern; the three callback builtins `fold`, `map` and `iterate`; a named
   function used as a value rather than called; a lambda; a `Decimal` literal,
   which the fragment has no path for; a call whose callee is an expression; a
   constructor pattern nested inside a list pattern; and a call through a local
   binding.

   **The top two roots — `++` and nested record patterns — are not the callback
   problem** ADR 0030 §10.2 priced as expensive (breaking
   `Machine::compiled_answer`'s `&self`, moving the `Frame::Call` push above
   `enter`, deleting the `compiled_witness` tripwire). **They are plain missing
   lowerings in `jit.rs` — the cheap half of the distance between the front
   end's loss and the kernel's win — and nothing in `ply-eval` has to move for
   them.**

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

Pre-registration, both amendments and the raw series are outside the repository.
Both series carry `uptime` beside every block, and every arm is counterbalanced
with a null control in the series rather than argued.

**One harness defect is on the record because it silently produced empty fields
rather than an error:** the first series spelled the arm
`/usr/bin/time -p CMD >/dev/null 2>/dev/null`, **which silences `time`'s own
stderr along with the command's.** It was found before any number was taken from
it, fixed by silencing the command inside a subshell, **and the fix was proved to
report a number before the series was restarted. The arm now fails loudly on an
empty measurement rather than writing a blank field.**
