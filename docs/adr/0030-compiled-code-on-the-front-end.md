# ADR 0030 — Compiled code on the front end: 190,618 entries, 1.089× measured, 1.121× ceiling

Status: accepted — **a measurement and a bound, not a change of direction**.
Amends `crates/ply-eval/src/backend.rs`'s module header and ADR 0026's audit
note, both annotated in place. Supersedes nothing. Adds no dependency, moves no
toolchain pin, promotes nothing: `grep -c cranelift Cargo.lock` still answers
**0**.

> **The last clause is a fact about this ADR's change and stopped being a fact
> about the tree on 2026-08-31.** `grep -c cranelift Cargo.lock` now answers
> **44**: `crates/ply-codegen` puts a cranelift JIT behind `ply test --backend
> cranelift` (ADR 0026 §4.7, §4.9). Nothing measured below changes — every
> figure here is `ply_eval::backend::Reference` on the front end and was
> re-taken independently as **1.106×** over `examples/` by the change that
> added the code generator, against the **1.0887×** published here.
>
> One thing this ADR predicted is worth marking as **confirmed and then some**.
> §5 reasons that a real code generator can add at most about 3% over
> `Reference` on the front end. Measured: on `examples/` the code generator is
> **0.363×** — 2.76× *slower* than no backend at all — because it enters 1.1%
> of offers there and pays per-run compilation for the privilege. The ceiling
> was right and the sign was worse than the ceiling suggested. ADR 0026 §4.9.

> **Corrected in place (adversarial review, 2026-08-31): the block above
> measured the wrong corpus and called it this ADR's workload. The correction
> is more favourable to the code generator, not less, which is why it is worth
> stating precisely.**
>
> **Withdrawn**, and it is two sentences:
>
> > every figure here is `ply_eval::backend::Reference` on the front end and was
> > re-taken independently as **1.106×** over `examples/` by the change that
> > added the code generator, against the **1.0887×** published here.
>
> > §5 reasons that a real code generator can add at most about 3% over
> > `Reference` on the front end. Measured: on `examples/` the code generator is
> > **0.363×** — 2.76× *slower* than no backend at all — because it enters 1.1%
> > of offers there and pays per-run compilation for the privilege.
>
> **Why they are wrong.** `ply test examples/` is not this ADR's workload and
> this document says so twice. §"The workload, and it is the one the gap was
> measured on" names `spikes/ply-parser/` — six modules, 4,979 lines of Ply,
> parsing `examples/*.ply` as **13 byte literals**, 333,851 bytes — driven by
> `ply test <dir> --no-cache -j 1 --filter probe.parse`. And §"Why this ADR
> exists" separately calls `examples/` *"the workspace's own test corpus and not
> a program anyone is trying to make fast"*. The two are different programs: the
> A arm of §2.2 is **2.70 s** of user CPU, `ply test examples/`'s is **0.44 s**.
> So `1.106×` was never a re-take of `1.0887×`, and `0.363×` was never a
> measurement of what a code generator does to the front end. The agreement
> between 1.106 and 1.0887 is a coincidence of ratio between two different
> workloads.
>
> **The front end, re-taken on this ADR's own corpus and command.** Rebuilt from
> the recipe above — 13 files, 333,851 bytes, byte count identical to this
> document's — five arms, **rotated one slot per window** so every arm occupies
> every position, N = 9, min of 9, ratio against no backend:
>
> | arm | min wall | min user | ratio (wall) |
> | --- | ---: | ---: | ---: |
> | no backend | 2,845.2 ms | 2,815.9 ms | 1.0000× |
> | `--backend reference` | 2,616.8 ms | 2,592.6 ms | **1.0873×** |
> | `--backend cranelift` | 2,937.9 ms | 2,912.9 ms | **0.9685×** |
> | null control A | 2,846.8 ms | 2,813.0 ms | 0.9994× |
> | null control B | 2,840.5 ms | 2,811.5 ms | 1.0017× |
>
> **`Reference` replicates this ADR exactly: 1.0873× against the 1.0887×
> published here**, on an independent instrument, by a different person, four
> months of tree-churn later. The two null arms are within 0.2%, so the series
> is sensitive to the 8.7% it reports.
>
> **And the code generator on the real front end is 0.9685× — a 3.2% net loss,
> not a 176% one.** The withdrawn figure was dominated by fixed cost on a short
> window: `examples/` is a 468 ms run carrying 382 ms of fixpoint plus per-worker
> code generation, so the compile is most of the window. This ADR's workload runs
> for 2.85 s and the same fixed cost is a sixth of it. **The direction the
> withdrawn block reported is right and its magnitude was an artefact of corpus
> length.**
>
> **Nothing here contradicts §4's ceiling, and the reason is the entry count.**
> Over this corpus `--backend reference` enters **190,617 of 190,703 offers with
> 69 definitions in the fragment** — §1's 190,618, reproduced. `--backend
> cranelift` enters **89,912 of 294,538, with 6 definitions in the fragment**.
> **89,912 is exactly the number §"Why this ADR exists" names as the
> pre-`Bytes`-widening `Int | Bool` rung.** The code generator's fragment on the
> front end is *narrower than `Reference`'s*: it reaches less than half the
> entries and pays a compile for them. That is a fact about `crates/ply-codegen`
> and not about the ceiling, which remains **1.121×** for a backend that enters
> what `Reference` enters, infinitely fast. Nothing measured has been above it.
>
> Load 5.79 → 5.73 across the series, **above this project's 4.0 gate**, so these
> are observations and not figures — on the same terms, and with the same
> defect, as the block they correct. Pre-registration and raw series:
> `/tmp/cranelift-review/PRE-REGISTERED.md` (Amendment 1, written before any
> timing for this corpus existed) and `/tmp/cranelift-review/R4-frontend.txt`.

**The one sentence.** With the seam widened to `Bytes`, the shipping backend
enters the Ply front end **190,618 times over `examples/`** — 4.23 entries per
token, not the zero the record projected — and that buys a **measured 1.089×**
end to end against a **ceiling of 1.121×**, which moves the front end from
**26.9×** the Rust front end to **24.8×** and could never move it below
**24.0×**. Compiled code, gated as it is today, does not close this gap. What
would is one line of the census: the argument test refuses **100.00%** of what it
refuses, and a *type-level* argument test reaches **82.855%** of body calls
against today's 8.631%.

## Why this ADR exists

Every speedup this project has recorded for compiled code was taken on a compute
kernel. ADR 0018 §0 says so; ADR 0026 §1.2's 18,773 entries were over
`examples/` and `tests/fixtures/`, which is the workspace's own test corpus and
not a program anyone is trying to make fast. Neither says what a compiled Ply
*front end* would cost, and the bootstrapping goal (ADR 0021) turns on nothing
else.

ADR 0026 §3 projected that it would cost nothing, because nothing could enter:

> `fn read_line(buf: Bytes, ..) -> Line` **cannot cross**

That is true of `read_line` and the generalisation drawn from it — that a real
front end's arguments are outside the fragment — **is false, and was false before
the `Bytes` widening**. A lexer's hot arguments are not `Bytes`; they are
*offsets and bytes*, and a byte in Ply is an `Int`. The census below prices both
readings: at the pre-widening `Int | Bool` rung the front end still admits
**89,912** calls (4.069% of body calls), and the `Bytes` widening takes that to
**190,618** (8.627%). Neither number is zero and one of them predates this work.

## The workload, and it is the one the gap was measured on

`spikes/ply-parser/` — 6 modules, 4,979 lines of Ply, a lexer and a recursive
descent parser — parsing **`examples/*.ply`: 13 files, 333,851 bytes**. That byte
count is identical to `spikes/ply-parser/GAPS.md` §13R's, so §13R's **45,028
tokens** is carried over rather than re-derived, and is labelled as carried over.

Driven through the only shipping command that can attach a backend:

```
ply test <dir> --no-cache -j 1 --filter probe.parse [--backend reference]
```

The project holds the six parser modules plus a generated `probe.ply` whose 13
`test` blocks each `assert(len(parse(src_i()).node.items) > 0)` over one file's
bytes as a literal. `--filter` hides the parser's own 110 unit tests, so the
workload is the corpus and not a test suite. Both arms typecheck all seven
modules, so that term cancels; `--no-cache` is on both sides, and ADR 0026 §4.6
requires it on the backed side.

Pre-registered at `/tmp/arc-compiled-real/PRE-REGISTERED-REAL-WORKLOAD.md` before
any number in this document existed, including the amendment in §5 which is
dated after the first series and before the second. Two exploratory runs taken
before the pre-registration are disclosed in it and are not used.

## 1. Entries — what actually crossed

Deterministic; identical on every one of the 30+ runs that printed it.

```
backend reference · 190618 of 190703 offers entered · 85 declined · 66 in the fragment
```

**190,618 entries. 4.23 per token.** 36 distinct definitions, and the shape of
the list is the finding:

| entered | calls | | entered | calls |
| --- | ---: | --- | --- | ---: |
| `lexer.is_digit` | 45,060 | | `lexer.scan_until` | 2,394 |
| `lexer.skip_trivia` | 45,029 | | `spine.starts_upper` | **1,823** |
| `lexer.is_ident_start` | 44,782 | | `lexer.strip_us` | 981 |
| `lexer.scan` | 17,423 | | `lexer.exp_start` | 937 |
| `lexer.is_keyword` | 16,338 | | `lexer.strip_zeros` | 937 |
| `lexer.at` | 13,941 | | `lexer.int_of_digits` | 878 |

**The fragment enters the lexer and not the parser.** One parser definition
clears 1,000 entries. The next-largest contributor from the four parser modules
is nothing.

### Which gate refused the rest, and it is one gate

`PLY_SEAM_CENSUS=1`, same command, backend attached:

```
body calls (enter_code)   2209609
builtin calls              698723
ctor calls                 238187
admitted (all gates)       190703  (8.6306% of body calls)
  of which carried-sig     190618  (8.6268%)   <- what `Reference` answers
refused                   2018906
  ArgumentShape           2018906  (100.00% of refusals)
non-crossable arguments, by kind:  Record 3048368 · Ctor 200792 · Closure 5764 · List 5
```

**Every refusal is `Gate::ArgumentShape`. Not one is an effect.** The two gates
this project has treated as the structural obstacles — `Gate::PublishedRow` and
`Gate::InternalEffects`, the ones that are "correct and not negotiable" because a
native body has no machine to perform into — refuse **zero** calls on a real
front end, because a front end is pure. `Gate::Budget`, `Gate::SimulateRegion`
and the frame ceiling refuse zero as well. Without a backend attached the same
census reports 1,745 `Anonymous` refusals (0.09%) and nothing else new.

The whole of what keeps 91.4% of the Ply front end out of compiled code is a
`matches!` over three `Value` discriminants, and what it is refusing is
**3,048,368 `Record` arguments**: the parser's state record, passed to every
parser function on every call.

### The single most valuable call in the corpus is refused on its *return* type

On a lexer-only probe over the same 13 files, the backend is offered 188,805
calls, enters 188,792, and declines **13** — one per file. The 13 are all the
same definition:

```
lexer.lex   admitted 13   entered 0
```

`lex(Bytes) -> Scan` clears every gate: its argument crosses, it is pure, it is
lowered code, it has a name. It is declined because `Scan` is a record, so the
registry has no body for it. **One accepted call per file would have handed the
entire lexer to the backend and made the other 188,779 entries disappear** —
which is PR #30's result restated on real code: the win is the entry count
*falling*, because the interpreter stops driving the loop. The return type, not
the argument type, is what stands between this workload and that.

## 2. End to end

### 2.1 The first series had a position confound, and it is reported anyway

Registered as `A, B, A'` round robin, 5 blocks, min user CPU. A was always run 1
of a block and A' — *the same command* — always run 3, and A' came in below A in
all five blocks. Min user A=3.03, B=2.69, A'=2.91: a null control of **3.96%**
against an effect of 11.22%. R1 passes at 3.96 < 5.61 and only just. The series
is kept (`/tmp/arc-compiled-real/S3-S5.log`) and is not averaged into what
follows.

### 2.2 The counterbalanced re-take

Four arms, 8 blocks of 4, block *k* rotating the order left by *k* mod 4, so
every arm sits in every position exactly twice. Min user CPU of 8. Load 3.09 →
2.84, under this project's 4.0 gate on both sides. Binary `current` before and
after.

| arm | | min user | min wall | mean user |
| --- | --- | ---: | ---: | ---: |
| A | no backend | **2.70** | 2.73 | 2.714 |
| A' | **null control** — byte-identical command to A | **2.70** | 2.73 | 2.730 |
| B | `--backend reference` | **2.48** | 2.51 | 2.506 |
| Bt | B with the in-process timer on — see §4, **the instrument is withdrawn** | 2.49 | 2.51 | 2.503 |

**The control that shows the measurement is sensitive: d(A,A') = 0.000% against
d(A,B) = 8.148%.** The same command twice, positions balanced, lands on the same
minimum to the resolution of `/usr/bin/time -p`; the arm that differs by 190,618
entries does not.

### **A/B = 1.0887×, a saving of 0.220 s of 2.70.**

Three independent counterbalanced series, taken over 40 minutes, agree:
**1.0887×** here, **1.0847×** in §4's, **1.0847×** in §3's — A at 2.69–2.70 and B
at 2.48 every time.

On the lexer alone — 188,792 of the 190,618 entries, 99.04% — min user of 6
counterbalanced blocks A=1.13, B=0.91: **1.242×**, a saving of 0.22 s. The saving
is the same as the whole workload's while the entries are 99% of it, which is the
dose-response that says the effect is the entered set and not the arm label: the
four parser modules contribute 1,826 entries and no measurable time.

## 3. Against the gap

Same sitting, interleaved run by run, five blocks of four (`A`, `B`, Rust cold,
Rust warm), load 3.14 → 3.42. `ply check examples/` is the Rust front end over
the identical 13 files, six phases against the Ply arm's two.

| | min user | min user+sys |
| --- | ---: | ---: |
| Ply lex+parse, interpreter | 2.69 | 2.70 |
| Ply lex+parse, `--backend reference` | 2.48 | 2.49 |
| Rust front end, cold | **0.10** | 0.11 |
| Rust front end, warm | 0.01 | 0.01 |

### **26.9× becomes 24.8×. The backend closes 8.11% of the absolute gap.**

No figure from GAPS.md §13 (30×) or §13R (17.2×) is used as an arm here; §13R's
denominator is user+sys at load ~10 and this is user at load ~3, and the two are
not comparable. What is comparable is that all three readings agree on the order:
a Ply front end costs one to three dozen times a Rust one.

## 4. The ceiling, which is the number that matters

An entry replaces the machine's evaluation of a whole subtree with `Reference`'s.
So `userB = userA − t + r + overhead`, where `t` is what the machine spent there
and `r` is what `Reference` spent. `t` is what an *infinitely fast* backend would
delete, and it is not derivable from the entry count: one equation, two unknowns.
So `r` is measured.

### The first instrument was refused by a tripwire, and it was right to be

It timed `Reference::enter` with `Instant::now()` inside `ply-eval`. That crate
may not read the host's clock at all, and
`crates/ply-eval/tests/simulated_handlers.rs`'s
`the_evaluator_reads_no_host_clock_and_no_host_entropy` bans the type by name
from every non-test line of it — *"a simulated run must be a function of its
definitions and its seed"*. It went **red** on the edit. The clock is gone from
`ply-eval`; its reading (`r` = 0.0859 s corrected, ceiling 1.128×) is kept below
only as corroboration, taken with a tool this crate may not carry.

Worth stating plainly, because the lesson is not about clocks. Nothing about the
measurement was *observable* — the accumulator was two statics and no Ply program
could read them — and the tripwire does not care, because it reads the **source**
and not the behaviour. A measurement is exactly the kind of change that would
have walked past a behavioural test, and this one did not because somebody had
already decided that the argument "but it is only for measuring" is not
admissible in this crate.

### The instrument that replaced it has no clock in `ply-eval` at all

Two binaries from the same tree, differing by **one line** in
`Reference::answer` — `let _ = self.run(name, args, fuel);` before the real call,
so every entered body is evaluated twice and the second answer is returned. Then
**`r` = B2 − B**, measured by `/usr/bin/time -p` on the whole process.

The precondition was checked before anything was timed: B2 reports the identical
`190618 of 190703 offers entered · 85 declined · 66 in the fragment`, passes
13/13, and passes `--engine both` — so the arms differ only in cost. Six
counterbalanced blocks, each arm in each position twice, min user of 6:

| arm | | min user | mean user |
| --- | --- | ---: | ---: |
| A | no backend | 2.69 | 2.697 |
| B | `--backend reference` | 2.48 | 2.483 |
| B2 | B, every entered body evaluated twice | 2.56 | 2.575 |

```
r  = B2 − B                                     0.0800 s
t  = (A − B) + r                                0.2900 s
f  = t / A                                      10.78 %
```

### **CEILING = 1 / (1 − f) = 1.121×.** The lexer alone: A=1.13, B=0.91, B2=1.00, f = 27.4%, ceiling **1.378×**.

The withdrawn clock instrument gave `r` = 0.0859 s and a ceiling of 1.128× on the
same workload — **6.9% apart**, inside the ±25% agreement band the amendment
fixed before either number was compared. Two instruments that share no mechanism
agree, and one of them is not allowed in the tree.

An infinitely fast backend — zero cost per entry, perfect codegen, cranelift or
better — takes 2.69 s to **2.40 s**, which is **24.0×** the Rust front end and
**11.2%** of the absolute gap. The reference backend, which is a *tree-walker*,
already delivers **72%** of that ceiling.

Three things make the bound conservative rather than optimistic, and all three
were stated before the numbers. `r` is under-stated because B2's second pass runs
on warm caches, so `t` and the ceiling are **lower** bounds. `t` is under-stated
again because arm B pays `compiled::admit` on all 2,209,609 body calls and arm A
pays it on none (`Machine::compiled_answer` returns at
`self.compiled.as_ref()?`, before `admit`), so the saving attributed to entry is
charged the seam's own overhead. And the machine still pushes and pops a
`Frame::Call` for an entered call (`Machine::enter_code`), so `t` is
body-evaluation time and the ceiling is a ceiling on that, not on the call
protocol.

### What the entered set actually contains, which is why `t` is that large

The census without a backend reports **2,315,307** body calls and **1,070,245**
builtin calls; with the backend attached the machine sees **2,209,609** and
**698,723**. The difference is what ran inside the entered subtrees and never
reached the machine: **105,698 body calls and 371,522 builtin calls**. The
entered set is not 190,618 leaf predicates — it is 190,618 subtrees holding
296,316 body calls and 371,522 builtin calls, **18.4% of every call the run
makes**.

## 5. The finding that withdraws a sentence: `Reference` is not slower

`crates/ply-eval/src/backend.rs`'s header said, and the note at the head of ADR
0026 repeated:

> **It is slower than the machine and says so; it exists to be policed, not to be
> fast.**

On this workload it is **faster**, and by a lot. Over the subtrees it takes it
does 296,316 body calls in 0.0800 s — **270 ns a body call** — against the
machine's 0.2900 s, **979 ns**. That is **3.63×**, and it is why arm B beats arm
A at all.

This is not the tree-walker being a better engine. Run as an engine over the same
program it loses badly, and the control was taken because the result was
surprising:

| | machine | `--engine treewalk` | |
| --- | ---: | ---: | --- |
| lex + parse | 2.69 s | 4.06 s | 1.51× slower |
| lex only | 1.13 s | 3.51 s | 3.11× slower |

The same evaluator is 3.11× slower over the whole lexer and 3.56× faster over the
fragment inside it. **Mechanism** — consistent with the numbers, not separated by
them — is ADR 0020 §6.3's profile: the machine's step, dispatch and refcount
protocol is 70.3% of executed time and is a fixed cost per call, so on scalar and
`Bytes` bodies with no closures and no records it is nearly all of the cost,
while the tree-walker's own weakness (deep values, cloned environments) is
exactly what the fragment excludes. Which is the same reason a real code
generator would win here, arriving through a backend that is not one.

The sentence is corrected in place at both sites rather than deleted. It remains
true of the case ADR 0026 measured — a body the backend **declines** is re-run to
exhaustion once per offer, 26.45 s against 0.04 s over a 20,000-deep ladder — and
that is now stated as the case it is true of.

## 6. What is still refused, and the one lever that is large

From the same census, as a share of the 2,209,609 body calls:

| rung | admitted | args + return |
| --- | ---: | ---: |
| `Int \| Bool` (before 2026-08-30) | 4.072% | 4.069% |
| **`+ Bytes` (today)** | **8.631%** | **8.627%** |
| `+ Str` | 8.631% | 8.627% |
| `+ Record, Ctor` — shallow, **unsound** | 82.854% | 46.428% |
| `+ Record, Ctor` — deep walk, sound | 10.429% | 10.427% |
| no world-handle — shallow, **unsound** | 82.855% | 82.855% |
| no world-handle — deep walk, sound | 15.187% | 15.187% |
| **declared parameter types — sound, O(1) per call** | **82.855%** | **82.855%** |

`Str` buys nothing on a front end. The deep walk buys 1.8 pp and
`crates/ply-eval/src/census.rs`'s header prices it at 475× on this exact
workload — it does not finish. **The type-level gate is the only rung that is
both sound and large: 82.855% against 8.631%, a 9.6× wider entry set, at O(1) per
call after a per-definition precompute.**

What it would be worth in *time* is not measured and this ADR does not claim it.
The one bridge available is the calibration this document establishes: on this
workload **8.63% of body calls accounted for 10.78% of run time**, a ratio of
1.25. That is a single point and it cannot be extrapolated to 82.9% — 1.25 ×
82.9% exceeds 100%, so the relation must saturate somewhere between, and where is
unknown. What can be said without arithmetic is that the ceiling for the fragment
as gated today is **1.121×**, and it is not a fragment-widening away from
anything that closes a 26.9× gap; the entry that would matter is `lexer.lex`, one
call per file, and it is refused on its return type.

## 7. Correctness — the entries were seen to matter before they were reported

An entry count is worth nothing if the run stays green when the backend lies.
Each of these is the same command with `--backend wrong:<mutation>`:

| mutation | answers changed | verdict |
| --- | ---: | --- |
| `inverted` | 667,728 | **13 failed, 0 passed** |
| `off-by-one` | 21,722 | **13 failed, 0 passed** |
| `stale` | 19 | **13 failed, 0 passed** |
| `wrong-type` | 13 | **13 failed, 0 passed** |
| `unoffered` | 13 | **13 failed, 0 passed** |
| `answers=1@lexer.is_digit` | 13 | **13 failed, 0 passed** |
| `exceeds-budget=2` | **0** | 0 failed, 13 passed |

Six of seven caught. The seventh **did not fire** — 190,703 offers of the target
and zero answers changed, because nothing in this corpus outruns the machine's
bound — and is reported as not-fired, not as not-caught, which is the distinction
ADR 0026 §4.5 and `backend.rs`'s `Mutation::ExceedsBudget` both insist on.

`--engine both --backend reference`: **audited 13 of 13, 0 failed** — machine,
tree-walker and backend agree on the whole corpus. `cargo test -p ply-cli --test
backend`: 14 passed, unchanged.

### And the instrument was armed against itself

One counter is new — `Counts::entered_names`, written from `Reference::enter`
under `PLY_SEAM_CENSUS` — and a counter that cannot be wrong is a counter that is
not reading anything.

- **Seen to fail.** It records a name only on `answer.is_some()`. Deleting
  `&& answer.is_some()`, rebuilding, and re-running the lexer-only census turns
  `32 distinct, 188792 entries` into `33 distinct, 188805 entries` and puts
  **`lexer.lex` in the entered list at 13** — the definition that is offered once
  per file and *declined* every time, which is §1's whole finding. The corruption
  named: the histogram would report the **offered** set under the entered set's
  name. Restored, and the comment at the site records the numbers rather than
  naming a test that does not exist.
- **It closes against two counters it does not share code with.** The CLI's
  `190618 entered / 85 declined` and the census's `admitted 190703`; the
  histogram's 36 names sum to 190,618.
- **The withdrawn timer had its own negative control**, recorded because it is
  what says the reading corroborating §4 was not noise: with no backend attached
  it reported `entries 0 · declines 0 · nanos 0`, and `r` varied run to run
  (95.9 – 97.7 ms over eight runs), so it was reading a clock and not a constant.
- **The B2 instrument's control is its precondition** (§4): identical entry
  counts, 13/13 green, `--engine both` green. A doubling that changed an answer
  or an entry count would not be a cost-only arm.

## 8. What would make this wrong

- **`t` is attributed to the entered set and something else caused the saving.**
  The dose-response is the answer: the lexer-only probe holds 99.04% of the
  entries and shows 91% of the saving, and its `r` (0.0941 s) is 98% of the whole
  workload's. If a future change makes those diverge, `t` is measuring something
  else.
- **`r` is measured by doubling and the second pass is not the same work.** It
  runs on warm caches, so B2 − B under-states `r`; the direction was fixed in the
  amendment before the number and it makes the ceiling a lower bound. On the
  lexer-only workload the same instrument reads `r` = 0.090 s against the whole
  workload's 0.080 s with 1.0% fewer entries — the wrong way round by one tick of
  `/usr/bin/time -p`'s 0.01 s resolution, which is the resolution this whole
  quantity is measured at and is why §4 quotes the withdrawn clock's independent
  0.0859 s beside it.
- **The probe is `ply test` and not `ply run`.** 13 of the 190,618 entries are
  `probe.src_i()` handing over a source literal, which a `ply run` shape would
  not have. Removing them changes no figure to three digits.
- **`-j 1`.** Each worker builds its own backend; at `-j 10` the counts are per
  run and the times are not comparable to these.
- **The ceiling assumes an entry saves the whole subtree.** It does under
  `Reference`, which runs the body to completion. A backend that could only
  compile part of a body and called back into the machine would not reach
  1.121×; nothing in this tree can do that, and `compiled.rs`'s header says why
  the seam hands over no route back.
- **`examples/` is 13 files of Ply that this project wrote.** A front end run on
  a corpus with different token statistics would enter a different number of
  times. The corpus was chosen because it is the one GAPS.md §13 and §13R took
  the gap on, not because it is representative of anything else.
- **The brief this work was set from says the corpus is 780,456 bytes.**
  `examples/*.ply` is 333,851 bytes today and matches §13R exactly. The
  discrepancy is recorded and not resolved; if the intended corpus was a
  different one, every absolute second here is on the wrong denominator and the
  ratios are not.

## 9. Decision

1. **No backend ships on the strength of this.** 1.121× is above the 1.10× bar
   the pre-registration fixed for "does not help", and it is nowhere near a
   reason to take 31 packages and 44 cranelift lines into `Cargo.lock`. ADR 0026
   §4.5's precondition and ADR 0016 §3.5 are untouched.
2. **The next widening is the type-level argument gate**, and this ADR is the
   evidence for choosing it over the deep walk and over `Str`. Whoever takes it
   owes what `census.rs`'s header already says they owe — a re-measurement of
   `the_shape_gate_is_reached_before_the_row_is_looked_up`, since a type-level
   test needs the name first — and owes one more thing this ADR adds: **the
   return type, measured at the same time**, because `lexer.lex` is the call that
   matters and it is refused on its return.
3. **`Reference` is no longer described as slower than the machine.** Corrected
   in place at both sites, with the case it is still true of named.
4. **`Counts::entered_names` stays; the clock does not.** Which definitions a
   backend entered was not answerable in this tree before — `admitted_names`
   answers a different question and `lexer.lex` is the difference — and it is
   eight lines of environment-gated counting with no clock in them. **How long it
   spent there is measured from outside the process and must go on being**, by
   the two-binary method in §4 or a better one; `ply-eval` may not carry a clock
   and the next person to want this number will reach for `Instant` first, as I
   did.

## Provenance

Binary `target/release/ply`, `.github/binary-is-current.sh` reporting
`current (158 inputs checked)` before and after every series; no series spans a
rebuild. All figures 2026-08-30, one machine, `uptime` recorded on both sides of
each series and printed in the logs. Raw:

- `/tmp/arc-compiled-real/PRE-REGISTERED-REAL-WORKLOAD.md` — statistics, run
  counts, decision rules and the S3b amendment, each written before its number
- `/tmp/arc-compiled-real/S3-S5.log` — the first, confounded series
- `/tmp/arc-compiled-real/S3b.log` — the counterbalanced series, 32 runs
- `/tmp/arc-compiled-real/S4b.log`, `S4c.log` — the clock-free `r`, 18 runs each,
  lex+parse and lexer-only
- `/tmp/arc-compiled-real/S5b.log` — the gap, interleaved with the Rust arm
- `/tmp/arc-compiled-real/S1b.census`, `S1b-nobackend.census`, `lexonly.census`
