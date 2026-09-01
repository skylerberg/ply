# ADR 0030 — Compiled code on the front end: 190,618 entries, 1.089× measured, 1.121× ceiling

> **Amended 2026-08-31 — §9.2's widening was taken and the title's last two
> figures moved.** The argument test is now over declared parameter types, the
> seam admits **84.0%** of the front end's body calls against 12.2%, the
> measured speedup is **1.170×** and the ceiling **1.294×**. The title is left
> as it was written, because it names what *this* ADR measured; the amendment
> under §6 carries the new figures, the before/after arms and what is still
> refused. Nothing in §§1–8 is withdrawn — the before column of every table in
> the amendment reproduces §1's entry line to the call from a binary built out
> of this tree.

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
> **Third take, 2026-08-31 — ADR 0032 §2.** On the same corpus and command,
> after the seam's fragment was closed, a 10-block counterbalanced series reads
> `--backend cranelift` at **0.938×** on min user CPU (3.04 s against 2.85 s),
> with a 0.702% null control. Same sign, same order, a slightly larger loss —
> and ADR 0032 §1 finds the reason the two entry lines below differ so wildly:
> `scalar_signature` in `ply-codegen/backend.rs` registers on an `Int | Bool`
> signature test that lever 1 never reached, so it drops 467 of the 489 bodies
> the fixpoint compiles. The 89,912 below is that filter, not the seam's.
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

> **Taken, 2026-08-31, and the prediction was right about the shape and
> conservative about the size.** `Machine::compiled_answer` now decides an
> answer the way `compiled::admit` decides an argument — the definition's
> declared **return** type is carried and the answer is of the kind it denotes,
> or the answer is childless and `compiled::crossable` carries it unchanged.
> `compiled::CarriedTypes::answer_crosses` is the test and
> `backend::carried_signature` is now one line of the same table.
>
> On the full W1 workload — the same 13 files, the same command — the entry line
> goes
>
> ```
> before  backend reference · 306931 of 1580763 offers entered · 1273832 declined · 146 in the fragment
> after   backend reference ·      26 of      26 offers entered ·       0 declined · 413 in the fragment
> ```
>
> and `Counts::entered_names` says what the 26 are: **`items.parse` 13, one per
> file**, plus the 13 nullary `probe.src_N() -> Bytes` that hand it the source.
> The prediction above was `lexer.lex`, one per file, on a lexer-only probe; on
> the whole front end the entry moved one level further out, because `parse` is
> also `Bytes -> <a record>` and swallows the lexer as part of its own subtree.
> **306,931 entries became 26, and that is the win.** A reader who sees an entry
> count fall by four orders of magnitude and reads it as a regression is reading
> the number the interpreter produces when it is *driving*; PR #30 is the
> precedent (crossings 721 -> 1) and this ADR's §1 is where that was written
> down before it was observed here.
>
> The coverage number that rises beside it is the share of body calls the
> registry can **answer**, taken with no backend attached so nothing is hidden
> inside an entered subtree: **411,216 -> 2,028,230 of 2,414,170**, i.e.
> **17.033% -> 84.014%**, which is every call the seam admits. The
> "offered and declined on the return" figure this ADR made its headline —
> 1,273,832 of 1,580,763 with a backend, 1,617,014 of 2,028,230 without — is
> **zero** on this workload.

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

> **Withdrawn as a bound on the seam, 2026-08-31 —
> [ADR 0031](0031-the-closed-fragment.md).** The paragraph above is true of the
> fragment it was measured on and is **false of the one that exists now**, so it
> is corrected here rather than deleted: *"An infinitely fast backend … takes
> 2.69 s to **2.40 s**, which is **24.0×** the Rust front end and **11.2%** of
> the absolute gap"*. That fragment admitted 8.6% of body calls. The closed one
> admits every call it is offered, and the same arithmetic on the same command
> reads `F` = **0.05 s** against `A` = 2.84 s: a ceiling of **56.8×**, with the
> whole of the measured absolute gap inside the fragment. The reference backend
> now delivers **none** of that ceiling — it is 1.46× slower than the machine —
> and what a *code generator* can reach is §10's 2.07×, re-measured at 2.10×.

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

> ### Taken, 2026-08-31 — the counterfactual row was right and the ADR's
> headline moves with it
>
> The type-level gate ships as `compiled::CarriedTypes`, reached through a new
> `compiled::Gate::ArgumentType`. Re-measured on **this** workload with the same
> command and the same corpus, and reported on a common denominator — the
> census with **no backend attached**, so an entered subtree cannot shrink the
> denominator under one arm and not the other:
>
> | | admitted | of body calls |
> | --- | ---: | ---: |
> | before — `Int\|Bool\|Bytes` on the value | 294,656 | **12.205%** |
> | **after — declared parameter types** | 2,028,230 | **84.014%** |
>
> (The before row is not a reconstruction: a binary was built from this tree
> with the type gate disabled and `carried_signature` narrowed back, and it
> reproduces §1's line to the call — `190617 of 190703 offers entered · 86
> declined · 69 in the fragment`, against §1's `190618 … 85 … 66`, the one-call
> difference being the corpus moving between sittings.
>
> **12.205% and §1's 8.631% are the same gate observed two ways, not two
> gates.** §1's census ran **with** a backend attached, and an entered call
> hides its whole subtree from the machine — so *both* the numerator and the
> denominator shrink. With the backend attached on this tree the same gate reads
> **190,703 of 2,308,472 = 8.261%**; with it detached, 294,656 of 2,414,170.
> The denominators differ by exactly 105,698, which is §4's count of the body
> calls inside the entered subtrees, arrived at here independently. Every
> **coverage** row in this amendment is taken without a backend for that reason,
> and the two arms are then comparable to each other; the entry counts and the
> timing tables below are of course taken with one.)
>
> **The counterfactual was accurate to 1.2 pp** — 82.855% predicted, 84.014%
> delivered, the difference being the corpus and the tree moving between the two
> sittings rather than anything about the rule.
>
> **Every gate but two now refuses nothing on this workload, and the two are the
> lambda wall.** The histogram, same run:
>
> | gate | refusals | share |
> | --- | ---: | ---: |
> | `Anonymous` | 380,176 | **98.51%** |
> | `ArgumentShape` (all of them `Closure` arguments) | 5,764 | 1.49% |
> | `ArgumentType`, `PublishedRow`, `InternalEffects`, `SimulateRegion`, `Budget`, the frame ceiling | **0** | 0 |
>
> §1's *"Every refusal is `Gate::ArgumentShape`. Not one is an effect"* becomes
> **every refusal is a lambda**: 380,176 anonymous bodies and 5,764 calls whose
> argument *is* a closure. That is this ADR's third sub-lever measured at full
> size, and no argument test can move it — `jit.rs`'s `admissible_builtin`
> refuses all six higher-order builtins on its first branch, and an anonymous
> body has no program-wide name for a registry to be keyed by.
>
> **Entries, end to end, through the shipping command** (`ply test <dir>
> --no-cache -j 1 --filter probe.parse --backend reference`):
>
> ```
> before  backend reference · 190617 of 190703 offers entered ·      86 declined ·  69 in the fragment
> after   backend reference · 306931 of 1580763 offers entered · 1273832 declined · 146 in the fragment
> ```
>
> Entries rose 1.61×, offers 8.29×, and the gap between them is §1's other
> finding restated at scale: **1,273,832 of 1,580,763 offers — 80.6% — are now
> declined on the return type** (the same ratio read off the no-backend census,
> where nothing is hidden inside an entered subtree, is 411,216 of 2,028,230
> admitted, so 79.7%). `carried_signature`'s return half is untouched
> at `Int | Bool | Bytes`, because widening the *answer* test needs either a
> deep walk on the returned value — which the parser's state record makes
> exactly as unaffordable as the argument walk was — or a type-level answer
> test, which moves a machine-side check into a backend obligation. §9.2's
> second obligation is therefore **measured and not discharged**, and the
> measurement is the 1,273,832.
>
> > #### Discharged the same day — the return type, and the 1,273,832 goes to 0
> >
> > The two sentences above that say what was *not* done are withdrawn:
> > *"`carried_signature`'s return half is untouched at `Int | Bool | Bytes`"*
> > and *"§9.2's second obligation is therefore **measured and not
> > discharged**"*. It is discharged. `Machine::compiled_answer` reads
> > `compiled::CarriedTypes::answer_crosses` — the declared **return** type is
> > carried and the answer is of the kind it denotes, or the answer is childless
> > and `compiled::crossable` carries it exactly as before — and
> > `backend::carried_signature` is `CarriedTypes::signature_carried` and nothing
> > else, so the registry, the argument gate and the answer test are three
> > readers of one per-definition table.
> >
> > The paragraph's own reasoning survives one half and not the other. *"A deep
> > walk on the returned value is exactly as unaffordable as the argument walk
> > was"* **stands** and is why no walk was taken. *"A type-level answer test
> > moves a machine-side check into a backend obligation"* **also stands, and is
> > the price that was paid** — it is not a reason the test was avoidable, it is
> > the cost written on the ticket. See §6.1 below.
> >
> > | | entries | offers | declined | fragment |
> > | --- | ---: | ---: | ---: | ---: |
> > | before (argument gate only) | 306,931 | 1,580,763 | 1,273,832 | 146 |
> > | **after (both ends)** | **26** | **26** | **0** | **413** |
> >
> > | share of body calls, **no backend**, denominator 2,414,170 | admitted | answerable |
> > | --- | ---: | ---: |
> > | before | 2,028,230 (84.014%) | 411,216 (**17.033%**) |
> > | after | 2,028,230 (84.014%) | 2,028,230 (**84.014%**) |
> >
> > The admitted column does not move, because this change touches no gate. What
> > moves is whether a backend has a body for what it is offered, and on this
> > workload the two sets have met.
> >
> > **The entry count fell by four orders of magnitude and that is the result,
> > not a regression.** §1 predicted the shape and PR #30 is the precedent
> > (crossings 721 -> 1, because one entry swallowed a search). `items.parse` is
> > entered **once per file** — 13 of the 26 entries; the other 13 are the
> > nullary `probe.src_N() -> Bytes` — and its whole subtree, every one of the
> > ~2.4 million body calls the census counts without a backend, runs inside that
> > entry where the machine sees nothing at all.
> >
> > `examples/` gains almost nothing again, and for the same reason it gained
> > almost nothing from the argument gate: **60,054 -> 61,414** answerable of
> > 246,782 body calls, entries 56,379 -> 56,703. What is left there is
> > `String`, `Float` and `Decimal` in the signature — ADR 0019 §5 item 4's
> > three, deliberately outside the fragment in both directions. The value of
> > both widenings is concentrated on `Bytes`/`Int`-shaped code, which is what a
> > front end is.
> >
> > ### 6.1 What entering a whole subtree changes, which is a different claim
> >
> > Before this, an entered body was a leaf-ish thing over scalars. Now one entry
> > can be a program, so every rule in `compiled.rs`'s header has to hold over a
> > **subtree** rather than over a call. Asked one at a time, and each answer
> > seen red before it was believed:
> >
> > - **Effects.** Already transitive: `DefInfo::internally_effectful` is a
> >   fixpoint over the call graph, held at four hops, through a mutually
> >   recursive pair and through a lambda. What changed is the *consequence* of
> >   its being wrong, from one call to a program.
> >   `an_entered_subtree_is_refused_for_an_effect_two_hops_down_that_it_would_hide`
> >   is the same claim asked the way this widening makes it matter.
> > - **The deterministic scheduler.** `Gate::SimulateRegion` reads the machine's
> >   state and says nothing about a definition that *opens* a `simulate` region
> >   two hops down. The row does: `sim.read` escapes, so it propagates to every
> >   caller that does not discharge it. New test
> >   `a_definition_that_calls_one_that_opens_a_simulate_region_is_never_offered`;
> >   under a deleted row gate the offer list reads
> >   `["outer", "searched", "double"]` against `["double"]`.
> > - **The budget.** `budget` is handed over once and now bounds a whole
> >   recursion rather than a call. `an_entered_subtree_is_bounded_by_the_budget_it_was_handed_and_not_by_its_entry`
> >   runs the real `backend::Reference` against a machine at `max_calls = 50`
> >   and asserts the two arms produce the **same diagnostic**; weakening
> >   `Reference::run`'s `set_max_calls(fuel)` to `fuel.max(10_000)` turns it red
> >   with `Ok(400)` against `recursion limit of 50 nested calls exceeded`.
> > - **Cells and regions.** Unchanged in kind and larger in degree: an entered
> >   definition that opens its own `with_cell` skips an allocation, which is
> >   unobservable outside a `simulate` region — the same argument
> >   `Machine::constant` rests on — and `Gate::SimulateRegion` is what keeps it
> >   outside one.
> > - **Continuations.** Unchanged: capturing one needs a `perform`, and the
> >   effects gate refuses anything that can reach one.
> >
> > ### 6.2 The one thing this gives up, and it is real
> >
> > While the answer test read `compiled::crossable`, a `Value::Cell` could not
> > come back **at all** — the invariant was structural, decided from a
> > discriminant over kinds that hold no `Value`. It now reads the declared
> > return type and the answer's *top-level* kind, and a declared type is a fact
> > about what the **program** can build, not about what a backend actually put
> > in the record. A backend answering `Record { toks: [Cell(..)] }` for a
> > definition declared `-> Lexed` is believed.
> >
> > The argument direction does not have this hole and the asymmetry is the whole
> > of it: an *argument* is a value the machine's own evaluation built under a
> > checker that accepted the program; an *answer* is built by the backend.
> >
> > So a **ninth wrong backend** was added rather than a sentence:
> > `backend::Mutation::Handle`, reachable as `ply test --backend wrong:handle`,
> > which replaces the first field of a `Record`, the first argument of a `Ctor`
> > or the head of a `List` with `Value::Cell(Slot::new(0, 0))` and leaves the
> > kind alone. Measured over `examples/` and `tests/fixtures/`: it changes
> > **388** answers and **237 of 1,127** tests report it, the first as
> > `E0502 \`bytes_concat_all\` expects Bytes, but got Cell`. **890 tests do
> > not.** Through the shipping command on `crates/ply-cli/tests/backend.rs`'s
> > corpus it fires twice and **one** of the two is caught — the other is
> > `assert_eq(len(pair(7)), 2)`, and a list with a forged cell in its head is
> > still two long, which is the measure of what a corpus has to *look at*.
> >
> > That is the same class as a wrong `Int`: caught by `--engine both` and the
> > differential corpus, and by nothing at the seam. It is written into
> > `compiled.rs`'s header and into `Compiled::enter`'s own doc — the paragraph a
> > backend author reads — as a limit, not argued away.
> >
> > **A registry gap closed that something was standing in.** `Mutation::Unoffered`
> > needs a definition that is *offered* and has no body.
> > `crates/ply-cli/tests/backend.rs`'s corpus had exactly one, `pair(Int) ->
> > List<Int>`, and a container return is now inside the fragment. Two tests
> > **failed** rather than passing quietly —
> > `an_answer_for_a_definition_the_backend_has_no_body_for_is_caught_by_ply_test`
> > with `"fired":0`, and `the_honest_backend_agrees_over_the_corpus_and_enters_it`
> > with *"the honest backend declined nothing"* — which is the entire value of
> > that file's step 2. `label(Int) -> String` replaces it, chosen because
> > `String` is a leaf-set exclusion rather than a container one, so the next
> > container widening will not close the gap again.
> >
> > ### 6.3 Time — the run gets SLOWER, and that is the ceiling's whole point
> >
> > Same instrument as §4 and as the amendment above: four binaries from this
> > tree (before/after x honest/doubled), 7 arms, 6 blocks of 7, block *k*
> > rotated left by *k* mod 7, min user CPU of 6, `/usr/bin/time -p`, wall clock
> > beside every run, `uptime` on both sides. Preconditions checked before any
> > timing: each doubled binary reports the identical entry count to its honest
> > twin, all four pass 13/13, and `ply-before` reproduces the amendment's entry
> > line to the call (`306931 of 1580763 · 1273832 · 146`).
> >
> > **Taken inside the 4.0 load gate**, 2026-08-31, load 2.98 -> 2.05, which is
> > the first series on this seam in two days that did not have to be reported
> > as an observation. The six runs of each arm agree to 0.03 s.
> >
> > | | A no backend | B `--backend reference` | B2 every entered body twice | A/B | `r` | `f` | 1/(1-f) |
> > | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
> > | before (argument gate only) | 2.83 | 2.43 | 2.68 | **1.165x** | 0.250 | 22.97% | 1.298x |
> > | **after (both ends)** | 2.83 | **4.15** | 8.23 | **0.682x** | 4.080 | **97.53%** | (40x, and see below) |
> >
> > Null control `d(A1, A1p)` = **0.353%** against `d(A1, B1)` = 46.6%.
> > `A_after / A_before` = **1.0000x** — the widening costs exactly nothing on
> > the arm that enters nothing, which is what a change that only moves what a
> > backend is *allowed* to answer should cost. The before row reproduces the
> > amendment above (1.165x against 1.170x, ceiling 1.298x against 1.294x) on a
> > different day at a different load, which is the reason to trust the after
> > row.
> >
> > **`ply test <W1> --backend reference` is 1.71x SLOWER after this change**
> > (2.43 s -> 4.15 s), and against the unbacked machine it goes from 1.165x
> > faster to 1.47x slower. That is not a defect and it is not a surprise: it is
> > what a total collapse does when the only backend in the tree is a
> > **tree-walker**. §5 of this ADR measured `Reference` at 3.63x faster than the
> > machine *per body call inside its fragment* and, run as a whole engine over
> > this same program, **1.51x slower**. While the fragment was 306,931 leaves
> > over scalars the first number governed; now that one entry is the program,
> > the second does. Observed 1.47x against that 1.51x.
> >
> > **What the ceiling says, and why the multiplier is not the number to quote.**
> > `f` — the share of the unbacked run an infinitely fast backend would delete —
> > goes **22.97% -> 97.53%**. `1/(1-f)` is 40x at that value, and it is not a
> > resolvable quantity: at `f` within 2.5 pp of 1 a one-point move takes the
> > ceiling from 25x to 100x. **Resolved 2026-08-31 by a second instrument
> > that shares no mechanism with this one — ADR 0031 §3's floor arm, the same
> > command with a `--filter` that selects no test, which measures the residue
> > instead of inferring it: `F` = 0.05 s, `A/F` = 56.8x, and `t` from the two
> > instruments agrees to 0.36%.** The directly measured statement is better and needs
> > no model: **`B - r` = 4.15 - 4.08 = 0.07 s**, so **98.3% of the backed run is
> > inside `enter`** and 0.07 s of it is not. The prize named in the brief this
> > work was set from — *"the ceiling moves from 1.121x to somewhere near
> > 5-7x"* — is cleared, and 5-7x is a **lower** bound rather than an estimate.
> >
> > **One caveat on `f`, because the model bends here.** `r = B2 - B` prices a
> > second pass through the entered bodies **at the backend's speed**, and
> > `t = (A - B) + r` then mixes that with the machine's timeline. §4's model
> > assumes a backend at least as fast as the machine; this one is 1.47x slower,
> > so `t` over-estimates the interpreter-equivalent work and `f` is an estimate
> > whose direction is certain and whose third digit is not. The
> > out-of-gate series taken an hour earlier (load 12.5 -> 6.9, reported as an
> > observation) puts the same quantity at **107.4%**, i.e. above 1, which is the
> > same statement with the bend visible. Both say: essentially all of it.
> >
> > **And `f` = 97.5% is larger than the 84.014% call share, which is not a
> > contradiction.** The call share is what the seam **admits**; the time share
> > is what an entered subtree **contains**. Once `items.parse` is entered there
> > is no seam inside it, so the 380,176 anonymous lambdas the census counts as
> > `Gate::Anonymous` refusals run inside the entry too. The census taken *with*
> > a backend attached says it in one line: the machine makes **26 body calls**
> > in the whole run, against 1,964,958 before this change, of which 378,431 were
> > lambda refusals. **The lambda wall is stepped over rather than solved** — see
> > §6.4.
> >
> > ### 6.4 What this does to the lambda wall, and to what polices the seam
> >
> > **The wall moves from the seam to the code generator.** This ADR's amendment
> > measured the residue at 98.51% `Gate::Anonymous` and concluded that *"no
> > argument test can move it"*, which is true and is not the same as "nothing
> > can". Entering at a **leaf** requires every lambda to be entered separately
> > and an anonymous body has no name for a registry to be keyed by; entering at
> > the **root** requires nothing of the sort, because the lambdas are inside the
> > body a backend compiled. With the backend attached, `Gate::Anonymous` refuses
> > **0** calls on W1 — not because it was widened, but because the machine never
> > reaches one.
> >
> > What that costs is honest and specific: `jit.rs`'s `admissible_builtin`
> > refuses all six higher-order builtins on its first branch, so a *cranelift*
> > backend cannot compile `items.parse` at all and would decline the entry this
> > change makes available. The obstacle is now a fact about a code generator's
> > coverage rather than about the seam's rules, and the seam is no longer what
> > has to change. That is a different piece of work from the one this ADR named
> > and it is a smaller claim than "the wall is gone".
>
> > > **Priced, 2026-08-31 — §10.** Nothing above is withdrawn and the last
> > > sentence is completed rather than corrected. *"A smaller claim than 'the
> > > wall is gone'"* is right; what it does not say is how much smaller. §10
> > > measures it: a backend whose registry is narrowed to the definitions a
> > > callback-free code generator could compile covers **61.06%** of body calls
> > > against this one's 100%, and its ceiling is **2.074×** against an `f` of
> > > 99.65% that `1/(1−f)` can no longer resolve. And *"cannot compile
> > > `items.parse` at all"* is now a count: 27 definitions in its closure call a
> > > callback builtin, at 28 sites, every one of them passing a lambda.
> >
> > **Three of the eight wrong backends stop firing on W1, and this is the
> > finding that most needs writing down.** `ply test <W1> --backend wrong:<m>`,
> > before and after, one run each:
> >
> > | mutation | before: fired / verdict | after: fired / verdict |
> > | --- | --- | --- |
> > | `off-by-one` | 21,722 / **13 failed** | **0** / 13 passed |
> > | `inverted` | 1,669,476 / **13 failed** | **0** / 13 passed |
> > | `unoffered` | 13 / **13 failed** | **0** / 13 passed |
> > | `stale` | 19 / **13 failed** | 25 / **13 failed** |
> > | `wrong-type` | 13 / **13 failed** | 13 / **13 failed** |
> > | `handle` (new) | — | 13 / **13 failed** |
> > | `exceeds-budget=2` | 0, not fired | 0, not fired |
> >
> > The mechanism is plain: after the change the only entered calls on W1 answer
> > a `Bytes` (13 x `probe.src_N`) or a `Record` (13 x `items.parse`).
> > `off-by-one` needs an `Int` answer, `inverted` needs a `Bool`, and
> > `unoffered` needs a registry miss — W1 has none of the three left. **A
> > workload that enters one call per file cannot police a corruption of a scalar
> > answer, because it never asks for one.** Nothing about the seam got weaker:
> > all three still fire and are still caught over `examples/` and
> > `tests/fixtures/` in `crates/ply-eval/tests/differential_corpus.rs` (15
> > tests) and through `ply test` in `crates/ply-cli/tests/backend.rs` (15). But
> > anyone who reads W1's green as evidence about those three is reading a
> > vacuous pass, and that is exactly the defect class `CONTRIBUTING.md` §"The
> > one rule" names.
> >
> > One more thing the same table shows, and it is the answer test doing visible
> > work on real source: under `wrong:stale`, `entered` is **14 of 26 offers**.
> > The other 12 are stale answers of the *wrong kind* for the definition asked —
> > a `Bytes` where a `Record` belongs — and `Denotes::matches` refuses them, so
> > the corruption becomes a decline rather than a wrong answer. That is the kind
> > half of `answer_crosses` biting outside a unit test.
> >
> > **`--engine both --backend reference` after the change**: W1 `audited 13 of
> > 13 · 0 failed`; `examples/` `audited 166 of 186 · 20 ran on one engine only ·
> > 0 failed`.
>
> **Time, and the ceiling.** Four binaries from this tree — before/after ×
> honest/doubled — 7 arms, 6 blocks of 7, block *k* rotated left by *k* mod 7,
> min user CPU of 6, `/usr/bin/time -p`, `uptime` on both sides, the doubled
> arms' precondition checked first (identical entry counts, 13/13, `--engine
> both` audits 13 of 13 with 0 failed):
>
> | | A no backend | B `--backend reference` | B2 every entered body twice | A/B | `r` | `f` | **ceiling** |
> | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
> | before | 2.81 | 2.61 | 2.68 | 1.077× | 0.070 | 9.61% | **1.106×** |
> | **after** | 2.82 | **2.41** | 2.64 | **1.170×** | 0.230 | 22.70% | **1.294×** |
>
> Null control `d(A, A')` = **0.000%** against `d(A, B)` = 14.2%. A second
> series taken an hour earlier at a load inside the 4.0 gate (3.35 → 3.41) puts
> the four shared arms at 2.82 / 2.61 / 2.81 / 2.41 — identical to
> `/usr/bin/time -p`'s 0.01 s resolution — and a third, four-arm confirmation
> reads the after ceiling at 1.276×. The seven-arm series was taken at a load
> of 6.83 → 4.42, **above this project's 4.0 gate**, because another workstream
> occupied the machine; it is reported with the load recorded rather than
> re-taken silently, and its agreement with the in-gate series on every shared
> arm is the reason it is quoted at all.
>
> **The gate costs nothing on the arm that enters nothing**: A after / A before
> = 0.9965×, which is the null control's own magnitude. The `FxHashMap` lookup
> the ordering reversal buys is below this instrument's resolution on 2.4M
> calls.
>
> **§3's gap is deliberately NOT restated** (and was restated on 2026-08-31 in
> ADR 0031 §4, in one sitting with the Rust arm interleaved: **31.6x**
> interpreter, **46.1x** with the shipping backend, the whole absolute gap
> inside the fragment at the ceiling. It also found why the re-take below read
> the warm figure — `ply check` writes its cache **beside the target**, so
> `gap.sh`'s `rm -rf $root/.ply-cache` never made `ply check $root/examples`
> cold.) §3's 26.9× came from a Rust arm
> interleaved run by run with the Ply arms in one sitting, and no such sitting
> was available: the machine carried another workstream at a load of 6 to 9
> throughout, and an attempt to re-take the Rust arm read 0.01 s — the *warm*
> figure, `ply check` having already cached — which is not §3's arm. Dividing
> this change's Ply number by that ADR's Rust number is two sittings over each
> other and is not done here. Every figure above is a ratio *within* one
> counterbalanced series.
>
> **What polices this widening**, because a coverage number is worthless if the
> entries stop being checked. All eight wrong backends over
> `crates/ply-eval/tests/differential_corpus.rs`: 14 passed. Through the
> shipping command on this workload, `ply test <W1> --backend wrong:<m>`: six of
> seven **13 failed, 0 passed**, and the seventh (`exceeds-budget=2`) fires 0
> times and is reported as not-fired exactly as §7 reports it. `--engine both
> --backend reference`: **audited 13 of 13, 0 failed** on W1 and 166 of 186 on
> `examples/`. `cargo test -p ply-cli --test backend`: 14 passed, unchanged.
> Seven corruptions of the new gate itself were each seen red and are tabled in
> `crates/ply-eval/src/compiled.rs`'s test-module header; one of them — making
> the type walk recurse into a declaration instead of reading the fixpoint —
> does not fail an assertion but overflows the stack, which is what "the
> precompute must terminate" looks like when it is false.
>
> §9.1's decision is **unchanged**: 1.294× is not a reason to take 31 packages
> into `Cargo.lock`. What moved is where the remaining gap is — it is no longer
> the argument test, and this ADR's §6 table no longer has a sound-and-large
> rung left in it. The two that remain are the return type and the lambdas.
>
> Raw: `/tmp/arc-typegate/PRE-REGISTERED-TYPE-GATE.md` (with two amendments,
> each written before its number), `S4.log`, `S5.log`, `S5c.log`,
> `BEFORE.W*.census`, `AFTER.W*.census`, `MUTATIONS.W1.log`, `red.*.log`.

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

   > **Done, 2026-08-31, and both obligations were met — one by discharging it
   > and one by measuring it.** The ordering was re-measured and the test
   > re-taken in place: the type gate needs the name, so it sits *below* the row
   > and effects gates rather than above the shape gate, and what survives of
   > the old cost claim is that a `Str`, `Float`, `Decimal`, `Secret`,
   > `Closure`, `Cell`, `Task` or `Continuation` argument is still refused with
   > no `Symbol` hash at all. The return type was measured at the same time and
   > is **not** widened: `carried_signature`'s return half stays `Int | Bool |
   > Bytes`, 1,273,832 admitted calls are offered and declined on it, and the
   > reason it was not taken is written where the cost is — in
   > `crates/ply-eval/src/backend.rs`'s header and in `compiled::crossable`'s.
   > See the amendment under §6.

   > **And the return type was taken the same day; the sentence above is
   > withdrawn (2026-08-31).** *"The return type was measured at the same time
   > and is **not** widened: `carried_signature`'s return half stays `Int | Bool
   > | Bytes`"* — it does not. `Machine::compiled_answer` reads
   > `compiled::CarriedTypes::answer_crosses` and `carried_signature` is
   > `CarriedTypes::signature_carried`. The 1,273,832 declines are **0**, and
   > the entry count goes **306,931 -> 26** because `items.parse` is entered
   > once per file. §6's amendment carries the tables, §6.1 re-asks every gate
   > of a subtree rather than of a call, and §6.2 names the one thing it gives
   > up — a container answer is checked for its kind and not for its contents,
   > which is a **backend obligation** where a discriminant test was a
   > structural guarantee. A ninth wrong backend, `wrong:handle`, exists so that
   > limit has something standing on it.
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

## 10. The lambda wall, priced — 2.07× against a ceiling that is no longer resolvable

§6.4 concluded that the wall *"moves from the seam to the code generator"*. That
is confirmed here and it is not the end of the sentence: **the move is what makes
the wall expensive**, because after it every second of the win lives on the far
side. Measured 2026-08-31, same tree, same W1, same protocol, pre-registration in
`/tmp/arc-lambda/PREREGISTRATION.md` written before the first number.

### 10.1 The seam's refusal and the code generator's are not the same refusal

**At the seam, `Gate::Anonymous` is a *naming* gate, not a callback gate.**
`compiled::admit` needs a program-wide name because every gate below it is a
lookup keyed by one — `memo::pure_by_published_row`, `internally_effectful`,
`CarriedTypes::args_cross` — and `Compiled::enter(&Symbol, ..)` is keyed by one
too. A lambda publishes none of those facts and offers no key. The gate's own doc
records what happens without it: replacing it with a fabricated empty `Symbol`
left every unit test in the crate green, because `Gate::PublishedRow` refuses an
unknown name one line later. Admitting a lambda would need a stable per-lambda
identity **and** per-lambda published facts, and neither exists.

**In the code generator the refusal is three refusals**, and `fold` needs all
three lifted, not one:

| `crates/ply-codegen-spike/src/jit.rs` | what it refuses |
| --- | --- |
| `admissible_builtin`, first branch | ``b.higher_order()`` — "a builtin that calls user code" |
| `NodeKind::Lambda { .. }` | "a lambda" — there is no closure representation at all |
| `Denotes::Local(_)` | "a call through a local binding" — the indirect call `comma_list`'s `item(c, s.p)` is |

And the refusal **propagates**: `Denotes::Uncompiled` refuses the *enclosing*
function rather than emitting a trampoline, so the compiled set is closed under
calls (`entry::admissible` is that fixpoint). One lambda anywhere under a root
refuses the root.

### 10.2 What the seam would lose if a backend were given a callback instead

The alternative to compiling the callback is handing the backend a route back
into the machine. `Machine::compiled_answer`'s own doc prices it and the price is
paid in invariants, not in lines:

- `compiled_answer` takes **`&self`**. *"Nothing is committed until there is a
  value, so a decline restores nothing because nothing was disturbed."* A
  callback needs `&mut Machine`.
- The `Frame::Call` at the call site is pushed **after** `enter` returns
  (`Machine::enter_code`), sound *"only because `enter` is handed no route back
  into this machine"*. Give a backend a callback and that push moves above
  `enter` — and then a decline must pop it, so the two paths stop being one line
  apart.
- **The bailout stops being free**, which is the concrete cost. Today `None`
  after a whole body costs nothing. With a callback, `stack.calls()`, the arena's
  allocation counter and the memo have already moved when the backend declines,
  so re-evaluating from the top repeats them — and re-evaluating from the top is
  how this seam keeps *"the code, message, spans, labels and notes … the
  interpreter's by construction"*.
- `Machine::compiled_witness` — debug-only, asserting `(frames, calls, host_ops,
  sims.len, allocations, regions_opened)` unchanged across `enter` — is stated to
  exist so that *"adding a callback, or handing a backend an arena, goes red here
  instead of breaking `note_step_site` in silence"*. A callback moves `calls` and
  `allocations` at minimum, so the change would begin by deleting the tripwire
  built to catch it.
- `budget` is computed once, before `enter`, as `max_calls − stack.calls()`. A
  callback consumes nested calls the machine cannot see, so the
  `recursion limit of 10000 nested calls exceeded` both engines share needs live
  accounting rather than one `usize`.
- `Gate::NotLoweredCode` keeps `Interp`'s closures out of compiled code because
  *"routing its closures into compiled code would audit the backend against
  itself"*. A callback is a route back into whichever engine is running, and
  `--engine both` is the only thing that catches a wrong `Int`.

**One invariant does *not* break, and it is the one that looks most fragile.**
The effects gate survives a callback. `DefInfo::internally_effectful` is a
transitive fact over the named call graph *and its scan walks lambda bodies*, so
a lambda that performs marks the definition it is written inside, and that
definition is the one `admit` gated. What a callback breaks is the machine-state
invariant, not the purity one.

**So the cheap direction is the other one: compile the callback.** It changes
nothing in `ply-eval` at all — a `fold` inside an entered body is invisible to a
seam that gates entries.

### 10.3 Is it worth it — the seam says no, the clock says it is the only thing left

**At the seam the lambdas now cost nothing, and that is measured rather than
inferred.** W1, `PLY_SEAM_CENSUS=1`, both arms, same binary:

| | body calls | admitted | refused | `Anonymous` | `ArgumentShape` |
| --- | ---: | ---: | ---: | ---: | ---: |
| no backend | 2,414,170 | 2,028,230 (84.014%) | 385,940 | **380,176** (98.51%) | 5,764 |
| `--backend reference` | **26** | 26 | **0** | **0** | 0 |

Every other gate refuses 0 in both arms. The no-backend arm is the positive
control for the backed arm's zero: the same binary, the same counter, 380,176.
The lambdas did not get admitted; the machine stopped reaching them, because
`items.parse` is entered once per file and they are inside it.

On `examples/` the picture is the opposite and is worth carrying, because it is
what stops "the residue is exactly the lambdas" from being a property of Ply:
246,782 body calls, `Anonymous` **8,963 — 4.92% of refusals**, against
`ArgumentType` 121,642 (66.82%, 89.5% of it `String`). **The lambda wall is a
front-end phenomenon.** A corpus of `String` and `Decimal` never reaches it.

**Then the clock, and this is the finding.** `Reference` is a tree-walker, so it
runs lambdas by interpreting them; a code generator cannot. To price that, the
registry was narrowed to the set a *callback-free* code generator could compile
and the same A/B/B2 series re-run, through `PLY_BACKEND_ONLY` — a registry
narrowing that can only add declines, so it cannot change an answer.

`N` is the fixpoint `entry::admissible` computes, with only the callback
constraint active. It is taken **outside** the spike — `ply hash --deps --json`
for the call graph and a source scan for the call sites — because the spike is
its own workspace, another workstream is live in `jit.rs`, and nothing in
`crates/*` may depend on it. 426 functions in W1, **36** of them call one of the
six, and `N` = the **223** with no such definition anywhere in their transitive
closure. The scan is validated against `spine.ply`'s own comment — *"one of
twenty-one in the five modules"* — which is exactly the 21 `iterate` sites it
finds.
`items.parse` is not in `N`: its closure holds 363 definitions of which **27**
call a callback builtin, at **28** sites, and **all 28 pass a lambda written at
the call site**.

6 arms × 6 blocks, block *k* rotated left by *k* mod 6, min user CPU of 6,
`/usr/bin/time -p`, load 4.28 → 3.52, **null control `d(A, A′)` = 0.000%**:

| arm | what it is | min user | mean user |
| --- | --- | ---: | ---: |
| A | no backend | 2.82 | 2.835 |
| A′ | no backend, a second label — the null control | 2.82 | 2.830 |
| B | `--backend reference` | 4.12 | 4.150 |
| B2 | B, every entered body evaluated twice | 8.23 | 8.252 |
| Bfo | B, registry narrowed to `N` | 4.94 | 4.952 |
| Bfo2 | Bfo, every entered body evaluated twice | 8.52 | 8.553 |

Preconditions checked before anything was timed, as §4 requires: each doubled
arm reports the identical entry line to its honest arm — `26 of 26 · 413 in the
fragment` and `495152 of 1049245 · 554093 declined · 220 in the fragment` — and
all four pass 13 of 13.

| | covered | `r` = B2−B | `t` = (A−B)+r | `f` = t/A | ceiling |
| --- | ---: | ---: | ---: | ---: | ---: |
| **B** — callback-capable | **100%** of body calls | 4.110 | 2.810 | **99.65%** | 282×, not resolvable |
| **Bfo** — callback-free | **61.06%** | 3.580 | 1.460 | **51.77%** | **2.074×** |

### **The lambda wall costs the difference between an infinitely fast backend deleting 99.65% of the run and deleting 51.77% of it. As a ceiling: 2.07×, against a number this instrument can no longer resolve.**

The model-free form, because `1/(1−f)` is not a resolvable quantity that close to
1: `B − r` = **0.010 s of 4.12** — 99.8% of the callback-capable backed run is
inside `enter` — against `Bfo − r` = **1.360 s of 4.94**, 27.5% of the
callback-free one still outside it.

Three supporting facts, each measured rather than argued:

- **Coverage.** With `N` the backend enters 495,152 of 1,049,245 offers and the
  machine still makes 1,435,185 body calls; 2,414,170 − 1,435,185 + 495,152 =
  1,474,137 = **61.06%** of body calls run inside the backend, against 100% for
  the unrestricted one. `--engine both --backend reference` under the narrowing:
  **audited 13 of 13 · 0 failed**, so the narrowing changed no answer.
- **A callback-free fragment cannot hide a single lambda, ever.** Under the
  narrowing the refusal histogram is *identical to the no-backend arm, gate for
  gate* — 385,940 refused, `Anonymous` 380,176, `ArgumentShape` 5,764. That is
  not a coincidence to be re-measured: a definition with no callback user
  anywhere in its closure has no lambda call beneath it, by construction. So for
  a code generator that cannot compile a callback, the 380,176 lambda refusals
  are **irreducible** — no widening of the fragment can swallow one.
- **And the callback-free fragment gives W1 back the power to police the seam
  that §6.4 recorded it losing.** §6.4's finding was that the collapse switched
  three of the eight corruptions off on this workload — `off-by-one` 21,722
  fired → **0**, `inverted` 1,669,476 → **0**, `unoffered` 13 → **0** — because
  the only entered calls left answer a `Bytes` or a `Record`, and those three
  need an `Int` answer, a `Bool` answer and a registry miss. Under the narrowing
  the fragment is scalar leaves again, and every one of them fires and is
  caught. `ply test <W1> --backend wrong:<m>` with `PLY_BACKEND_ONLY` set:

  | mutation | offers entered | verdict |
  | --- | ---: | --- |
  | `off-by-one` | 17,599 of 26,402 | **13 failed, 0 passed** |
  | `inverted` | 6,009,842 of 10,684,172 | **13 failed, 0 passed** |
  | `unoffered` | 26 of 26 | **13 failed, 0 passed** |
  | `stale` | 20 of 66 | **13 failed, 0 passed** |
  | `wrong-type` | 13 of 52 | **13 failed, 0 passed** |
  | `handle` | 187,318 of 570,745 | **13 failed, 0 passed** |

  (The offer counts are not the honest run's 495,152: a wrong answer changes
  which branches the parser takes, so `off-by-one` fails fast and `inverted`
  loops. What matters is that each **fired** before it was caught, which is the
  middle step `mutations.rs` insists on and the one usually missing.) This is a
  fact about the *instrument* rather than about the wall — a narrowing that
  could not be caught lying would be worth nothing — and it is the one place
  where the callback-free fragment is strictly better than the collapsed one.

### 10.4 The narrow version — buildable, sound, and measured to be worth zero

The obvious escape from §10.1's three refusals is to take only the easy half of
the first: compile a `fold` whose callback is a **named** already-admissible
definition rather than an anonymous lambda. It is buildable and it is sound. On
this front end it moves nothing, in either of the two places it could be spent,
and both were measured before either was built. Neither was built.

**At the seam.** The only `ArgumentShape` refusals on W1 are 5,764 calls to
`spine.comma_list` — the one definition in `closure(items.parse)` with a
function-typed parameter (there are three in the program). Admitting a
`Value::Closure` argument when it is a named, carried-signature, internally-pure
definition is a sound widening, and every one of `comma_list`'s 14 call sites
inside a definition does pass a named one — `pattern`, `import_name`,
`fn_param`, `binder`, `ty` (three sites), `op_param`, `call_arg`, `expr`,
`record_field`, `param`, `clause_param`, `ty_field` — as do the six further
sites in `spine.ply`'s own `test` blocks, all `an_ident`. It is worth **0** with
the shipping backend, because those 5,764 calls are inside `items.parse`; and
**0** with a callback-free one, because `comma_list`'s *own body* drives its loop
with `iterate` and a lambda, so no such code generator has a body for it. **The one place on this front end where
a callback is a named definition is a function whose own body needs the anonymous
case.**

**In the code generator.** Of 43 higher-order call sites inside definitions
(`iterate` 21, `fold` 14, `map` 8), **38 pass a lambda written at the call site
and 5 pass a name** — and of those 5, one is `spine.dump_list`'s `f`, a
*parameter*, which is an indirect call `jit.rs` refuses separately. The remaining
4 are in `lexer.dump`, `spine.dump_diag` and `spine.dump_diags`: the debug-dump
path, and **none of the three is in `closure(items.parse)`**.

Recomputing the fixpoint with exactly that widening applied — those three removed
from the callback-user set — moves `N` from **223 to 223**. It gains **zero**
definitions: `lexer.dump` is still blocked by `lexer.hex`, `lexer.int_of_digits`
and `lexer.lex`, and `spine.dump_diag` and `spine.dump_diags` by
`spine.primary_span_of`, each of which passes a lambda. A widening that changes
no member of the fragment cannot change a number, so no series was run for it.

### 10.5 What would make this wrong

- **`N` models one constraint and `jit.rs` has several.** It is the callback
  fixpoint only; `jit.rs` also refuses `Float` and `Decimal` literals, `++`,
  match guards, a call whose callee is an expression, and a lambda in any
  position, and `entry::enterable` narrows the registry again to `Int | Bool`
  signatures. So **2.074× is an upper bound** on a callback-free cranelift
  backend's reach on this workload, and the real figure is below it. The
  direction is stated before the number, as §4's caveats were.
- **`r = B2 − B` under-states**, because B2's second pass runs on warm caches, so
  both `f`s and both ceilings are lower bounds — §4's caveat, unchanged and
  applying to both arms equally.
- **Both backed arms pay `compiled::admit` on every body call and A pays it on
  none**, so `t` is charged the seam's own overhead in both. Under the narrowing
  that is 1,435,185 calls plus 554,093 registry misses.
- **`Reference` is a tree-walker and both backed arms are slower than A** —
  1.46× and 1.75× slower. That does not touch the ceiling: `t` is what the
  *machine* would have spent in the entered subtrees and is independent of how
  fast the backend is, which is the whole reason §4 measures `r` rather than
  assuming it.
- **The source scan is a scan.** It strips `//` comments and string literals and
  then looks for a call to one of the six by name; it does not resolve shadowing,
  so a local binding named `fold` would be counted as the builtin. That is
  conservative in the direction that *shrinks* `N`, and the front end shadows
  none of the six. It was validated against three independent facts: the 21
  `iterate` sites `spine.ply`'s own comment claims, the runtime builtin histogram
  (`iterate` 27,799 · `fold` 4,814 · `map` 13, and `filter` / `map_fold` /
  `bytes_position` 0 at run time and 0 sites), and the seam's own
  `refused_names`, whose only `Closure`-argument entry is `spine.comma_list`.
- **The instrument was checked for vacuity.** A narrowing that silently did
  nothing would have reported the unrestricted run under the restricted label;
  the same command with `PLY_BACKEND_ONLY` unset reads `26 of 26 offers entered ·
  413 in the fragment` against the narrowed `495152 of 1049245 · 220 in the
  fragment`, and both readings are in the log.

### 10.6 What this means for the decision

§9.1 is **unchanged and the reason for it has changed**. The sentence that is no
longer the one to quote is *"1.294× is not a reason to take 31 packages into
`Cargo.lock`"* — §6.3 already withdrew the number. What replaces it is sharper: a
cranelift backend as `jit.rs` stands could not enter `items.parse` at all, and
the fragment it *could* enter has a measured ceiling of **2.07×**. Anything above
that is on the far side of the three refusals in §10.1 — and
`admissible_builtin`, the one usually named, is the smallest of the three: what
compiling a `fold` actually needs first is a closure representation and an
indirect call, neither of which `jit.rs` has.

**What shipped in code for this, and it is not a widening.** One measurement
knob, `PLY_BACKEND_ONLY`, in `crates/ply-eval/src/backend.rs`: `Fragment::build`
intersects its registry with a named set. It can only add declines, so it cannot
change an answer, and `--engine both` audits 13 of 13 with 0 failed under it. Two
tests hold it and both were seen red first — deleting the intersection reads
three names against two, and dropping the empty-name filter makes `"m.one,"` a
different experiment from `"m.one"`. Nothing in `crates/ply-codegen-spike` or
`crates/ply-cli` was touched.

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

The 2026-08-31 amendment under §6 (the type-level argument gate) is provenanced
in `/tmp/arc-typegate/`. The 2026-08-31 amendments under §1, §6 (the answer
test), §6.1-§6.4 and §9.2 are provenanced in `/tmp/arc-return/`:

- `PRE-REGISTERED-RETURN-TYPE.md` — every statistic, run count and decision rule,
  written before the first number existed and including the one prediction this
  work got **wrong**: C3 registered a slowdown as the expected outcome *and* the
  ceiling as where the speed claim lives. The slowdown happened (1.71x) and the
  ceiling is where the claim lives, so the rule held; what it did not anticipate
  is §6.4, that a collapse switches three of the eight corruptions off on this
  workload.
- `BEFORE.W1.*.census`, `AFTER.W1.*.census`, `BEFORE.W2.*`, `AFTER.W2.*` — the
  four coverage censuses, each taken twice (with and without a backend) because
  an entered subtree shrinks numerator and denominator together
- `S4.log` — the seven-arm series at a load of 12.5 -> 6.9, **an observation**
- `S4b.log` — the same series inside the 4.0 gate, load 2.98 -> 2.05, which is
  what §6.3 reports
- `S2e.log`, `S2e-before.log` — the mutation table of §6.4
- `S3.log` — `--engine both --backend reference` on both workloads
- `red.*.log` — the five corruptions of the answer test, each seen red, tabled in
  `crates/ply-eval/src/compiled.rs`'s test-module header
- `DIGESTS.txt`, `DIGESTS.final.txt` — the source digests before and after the
  mutation series, checked equal

§10 is provenanced in `/tmp/arc-lambda/`:

- `PREREGISTRATION.md` — every statistic, its instrument, the counterbalancing,
  the null control and the two decision rules, written before the first number.
  It got **two predictions wrong and they are the same mistake**: E1 was
  registered at *"< 30%"* and is **61.06%**, and D at *"`N` is a small minority
  of the functions the run calls"* — it is **223 of 426**. Both under-estimated
  how much of a front end is *not* a loop, and both were harmless to the
  conclusion for the reason §10.3 makes explicit: coverage in calls and coverage
  in time are different quantities, and the ceiling is 2.07× at 61.06% coverage
- `A.W1.nobackend.census`, `B.W1.backend.census`, `C.W2.*.census` — the four
  censuses; every coverage share is taken from the no-backend arm
- `E.W1.fo.census`, `E.W1.fo.stdout` — the callback-free registry, and the
  vacuity check that the narrowing applied
- `D.hash.json`, `D.result.json`, `D.sites.json`, `N.list`, `N2.list` — the call
  graph from `ply hash --deps --json`, the fixpoint, the 43 call sites with each
  callback classified, and the two name lists the series was run with
- `series.sh`, `series.tsv`, `ANALYSIS.txt` — 6 arms × 6 blocks with `uptime` on
  both sides of every block, the null control, and the arithmetic
- `S2.mutations-fo.log` — the six wrong backends under the narrowed registry
- `S3.engine-both.log` — `--engine both` on both registries, and the vacuity
  check with `PLY_BACKEND_ONLY` unset
- `red.no-narrowing.log`, `red.empty-name.log` — the two corruptions of the
  instrument itself, each seen red before either test was believed
- `W1.md5`, `DIGESTS.txt`, `DIGESTS.final.txt`
