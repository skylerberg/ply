# ADR 0031 — The closed fragment: 26 entries, 98.2% of the run inside them, a ceiling of 56.8× and an achievable 2.10×

Status: accepted — **a measurement, and a re-aiming**. It changes no gate, no
rule and no line of executable code. What it moves in the tree: this file; three
pointers into ADR 0030 (§3, §4 and §6.3, each quoting what it corrects); a
`///` block in `crates/ply-eval/src/{census,backend}.rs` recording the
re-measurement beside the numbers it confirms; an audit note in `ROADMAP.md`;
and one stale count in `CONTRIBUTING.md`. It adds no dependency and moves no
toolchain pin: `grep -c cranelift Cargo.lock` answers **0** either side of every
series below, as it did for ADR 0026 and ADR 0030. Nothing in
`crates/ply-codegen-spike` or `crates/ply-cli` was touched, another workstream
being live in `jit.rs`.

**The one sentence.** With the seam closed at both ends, `ply test --backend
reference` over the Ply front end parsing `examples/` enters **26 times** rather
than ADR 0030's 190,618 — `items.parse` once per file — and **98.2% of the run
is inside those 26 entries**, which takes the ceiling for an infinitely fast
backend from **1.121× to 56.8×** and moves the whole of the measured Ply/Rust
gap inside the fragment; but the ceiling a *code generator* can reach on this
tree is **2.10×**, because `jit.rs` cannot compile the one call that now
matters, and the difference between 56.8× and 2.10× is the entire remaining
question.

## Why this ADR exists

ADR 0030 measured a compiled backend on a real Ply front end for the first time
and ended with a bound and a lever: **1.121×**, and *"the argument test refuses
100.00% of what it refuses"*. Three workstreams took the lever — a type-level
argument gate, a type-level answer test, and an assessment of the lambda wall —
and each amended ADR 0030 in place. What the record did **not** hold when this
ADR was written is the thing all three were for: the front end measured end to
end after the fragment closed, on one workload, in one sitting, against the
interpreter and against the Rust front end, with a ceiling that is resolvable.

Two specific holes this ADR fills, both named in ADR 0030 itself:

- §6.3 could report only that `1/(1−f)` is *"not a resolvable quantity"* at
  `f` = 97.53%: within 2.5 pp of 1, a one-point move takes the ceiling from 25×
  to 100×. **§3 below resolves it**, with an instrument that measures the
  residue directly instead of inferring it.
- §6.3 also says *"§3's gap is deliberately NOT restated"*, because no sitting
  with a Rust arm interleaved was available. **§4 below restates it**, and finds
  the reason the earlier attempt read the warm figure.

## The workload, and it is checked rather than cited

**W1** — `/tmp/arc-typegate/W1`: `spikes/ply-parser`'s six modules, verified
byte-identical to the tree with `cmp` before the first number (all six
`identical`), plus the generated `probe.ply` (md5
`eabc0e6ba4012edbe2e2a9263b3e15a4`) whose 13 `test` blocks each parse one
`examples/*.ply` as a literal. **13 files, 333,851 bytes** — ADR 0030's corpus
and GAPS.md §13R's. Driven through the only shipping command that can attach a
backend:

```
ply test /tmp/arc-typegate/W1 --no-cache -j 1 --filter probe.parse [--backend reference]
```

**W2** — `ply test examples --no-cache -j 1`, the workspace's own corpus, for
contrast only.

Pre-registered at `/tmp/arc-closed/PREREGISTRATION.md` before any number in this
document existed, including the bar for *"closing the fragment was not worth
it"*, one amendment written before the three supplementary series it registers,
and an OUTCOME section written after the last number naming the **two
predictions this work got wrong**. No exploratory run was taken before the
pre-registration.

**Every coverage share is taken with no backend attached.** An entered call
hides its whole subtree, so with a backend attached the numerator and the
denominator shrink together — after the answer widening the machine's own
denominator on W1 is **26**, and a share taken against it is a statement about
nothing. This is `census.rs`'s rule and ADR 0030 §6's amendment made it a
requirement.

## 1. Entries and coverage

### The entry line, through the shipping command

```
ADR 0030 §1  backend reference · 190618 of 190703 offers entered ·  85 declined ·  66 in the fragment
today        backend reference ·     26 of     26 offers entered ·   0 declined · 413 in the fragment
```

**Entries fell by a factor of 7,331 and that is the win, not a regression.** It
is PR #30's shape — widening a fragment to a whole MCTS kernel took crossings
721 → 1, because the interpreter stopped driving the loop and handed the search
over once — and ADR 0030 §1 predicted it in those words for `lexer.lex` before
any of it was built. What entered is the whole parse: `Counts::entered_names`
reads **14 distinct definitions, 26 entries — `items.parse` 13, one per file,
and the 13 nullary `probe.src_N() -> Bytes` that hand it the source.**

The count that says what happened is not the entry count but the machine's own
work. With a backend attached the machine makes, for the entire run:

| | ADR 0030 §1 | today |
| --- | ---: | ---: |
| body calls the machine evaluates | 2,209,609 | **26** |
| builtin calls | 698,723 | **26** (13 `assert`, 13 `len`) |
| ctor calls | 238,187 | **0** |
| calls refused at the seam | 2,018,906 | **0** |

### Coverage, on a common denominator

Census with **no backend attached**, denominator 2,414,170 body calls,
identical under both arms:

| rung | admitted | answerable (`carried-sig`) |
| --- | ---: | ---: |
| ADR 0030's shipped gate — `Int\|Bool\|Bytes` on the value | 294,656 (12.205%) | 294,570 (12.202%) |
| + type-level **argument** gate *(the answerable column carried over from ADR 0030 §6; the tree cannot be in that state and this one at once)* | 2,028,230 (84.014%) | 411,216 (17.033%) |
| **+ type-level answer test — today** | **2,028,230 (84.014%)** | **2,028,230 (84.014%)** |

The first and third rows are this run's own census — the first is the ladder's
`+Bytes` rung, which the census computes as a counterfactual on every run.

and the same census with the backend attached says the two sets have met:
**26 admitted of 26 body calls, 26 answerable, 0 refused, 0 offered-and-declined.**

**8.631% and 84.014% are not comparable and the difference between them is not
the change.** ADR 0030 §1's 8.631% was taken *with* a backend attached; on this
tree the pre-change gate reads 8.261% that way and 12.205% without one, and the
two denominators differ by exactly the 105,698 body calls that ran inside the
entered subtrees. The comparison that means something is 12.205% → 84.014%, a
**6.9× wider seam**, and then a second change — the answer test — that took the
*answerable* share from 17.033% of body calls to all 84.014% of the admitted set.

### The gate histogram, and every gate that is not a lambda now refuses nothing

W1, no backend, 385,940 refusals:

| gate | refusals | share |
| --- | ---: | ---: |
| `Anonymous` | 380,176 | **98.51%** |
| `ArgumentShape` — every one a `Closure` argument, all of them `spine.comma_list` | 5,764 | 1.49% |
| `ArgumentType` | 0 | — |
| `PublishedRow`, `InternalEffects`, `SimulateRegion`, `Budget`, the frame ceiling | 0 | — |

ADR 0030 §1's *"every refusal is `Gate::ArgumentShape`"* — 2,018,906 of
2,018,906, over 3,048,368 `Record` arguments — is now **every refusal is a
lambda**: 380,176 anonymous bodies and 5,764 calls whose argument is a closure.
And with the backend attached that histogram is **empty**: the machine never
reaches a lambda, because the lambdas are inside the entry.

### W2 does not collapse, and the contrast is the finding

`ply test examples`, same two arms:

| | no backend | `--backend reference` |
| --- | ---: | ---: |
| body calls | 246,782 | 239,917 |
| admitted | 64,744 (26.235%) | 59,435 (24.773%) |
| answerable | 61,414 (24.886%) | 56,703 (23.634%) |
| entries | — | **56,703 of 59,435 offers · 2,732 declined · 220 in the fragment** |

`examples/` still enters 56,703 times and still declines 2,732 on the return,
because what blocks it is not structure. Its 182,038 refusals:

| gate | refusals | share | what it names |
| --- | ---: | ---: | --- |
| `ArgumentType` | 121,642 | 66.82% | `String` 108,925 · `Var` 10,867 · `Decimal` 1,504 · `Fn` 346 |
| `ArgumentShape` | 49,591 | 27.24% | argument kinds: `Str` 46,398 · `Decimal` 2,617 · `Closure` 2,089 |
| `Anonymous` | 8,963 | 4.92% | |
| `PublishedRow` 1,144 · `SimulateRegion` 607 · `InternalEffects` 91 | 1,842 | **1.01%** | the three structural gates, together |

**94.1% of what `examples/` refuses is one of the two argument gates, and at
least 86.8% of every refusal names `String`, `Str` or `Decimal`** — 110,429 of the
121,642 `ArgumentType` refusals name `String` or `Decimal` (90.8%), and of the
49,591 `ArgumentShape` refusals at most 2,089 can be attributed to a `Closure`
argument. The largest refusals by name are `std.db.lex` (10,817), `std.db.tok_word` (9,632)
and `std.db.tok_sym` (8,358) — a **SQL lexer and parser in the standard library**, the same
shape of program as the front end, refused because its tokens hold a `String`
where the front end's hold `Bytes`. The 1.01% is the whole of what the gates
this project has always called the structural ones refuse anywhere in this
document.

## 2. End to end — the run gets slower, and the slowdown is the result

Ten arms, **10 blocks of 10, block *k* rotating the order left by *k* mod 10**,
so every arm sits in every position exactly once. Min user CPU of 10,
`/usr/bin/time -p`, wall clock beside every run, `uptime` on both sides of every
block. **Taken inside this project's 4.0 load gate**: 2.84 → 1.63.

| arm | | min user | mean user | min wall |
| --- | --- | ---: | ---: | ---: |
| `A` | no backend | **2.84** | 2.850 | 2.88 |
| `A'` | **null control**, byte-identical command to `A` | **2.83** | 2.840 | 2.86 |
| `B` | `--backend reference` | **4.15** | 4.168 | 4.25 |
| `B2` | `B`, every entered body evaluated twice | 8.24 | 8.280 | 8.43 |
| `F` | **floor** — the same command with 0 tests selected | **0.05** | 0.051 | 0.07 |
| `Fc` | floor, second instrument — `ply check W1 --no-incremental` | 0.05 | 0.050 | 0.06 |
| `R` | `ply check examples` after `rm -rf $root/.ply-cache` | 0.01 | 0.010 | 0.02 |
| `Rn` | `ply check examples --no-incremental` | **0.09** | 0.090 | 0.10 |
| `Bfo` | `B` with the registry narrowed to what a callback-free code generator could compile | 4.94 | 4.970 | 5.01 |
| `Bfo2` | `Bfo`, doubled | 8.53 | 8.571 | 8.69 |

**The control that shows the measurement is sensitive: `d(A, A')` = 0.352%
against `d(A, B)` = 46.127%.**

### **A/B = 0.682×. The backed run is 1.46× SLOWER than the interpreter.**

That is not a defect and it is not new — ADR 0030 §6.3 measured 1.47× on this
change the day it landed, and this series is an independent confirmation at a
different load with a different arm layout. It is what a total collapse does
when the only backend in the tree is a **tree-walker**: ADR 0030 §5 measured
`Reference` at **3.63× faster than the machine per body call inside its
fragment** and **1.51× slower as a whole engine over this program**. While the
fragment was 190,618 leaves over scalars the first number governed. Now that one
entry is the program, the second does.

**The speed claim moved entirely into the ceiling, and §3 is where it lives.**

## 3. The ceiling, and this time it is resolvable

ADR 0030 §4's model is unchanged: an entry replaces the machine's evaluation of
a subtree with the backend's, so `userB = userA − t + r + overhead`, where `t`
is what an *infinitely fast* backend would delete and `r` is what `Reference`
spent. One equation, two unknowns, so `r` is measured. **No clock goes into
`ply-eval`** — `the_evaluator_reads_no_host_clock_and_no_host_entropy` reads the
source and refuses one, and ADR 0030 §4 records it going red on exactly that
edit.

### Instrument 1 — doubling, which is ADR 0030 §4's

Two binaries from one tree differing by **one line** in `Reference::answer`:
`let _ = self.run(name, args, fuel);` before the real call. Precondition checked
before anything was timed: the doubled binary reports the **identical** entry
line (`26 of 26 · 0 declined · 413 in the fragment`) and passes 13/13.

```
r = B2 − B                          4.090 s
t = (A − B) + r                     2.780 s
f = t / A                          97.89 %
1 / (1 − f)                        47.3 ×
```

### Instrument 2 — the floor, which is new here and shares no mechanism

`F` is the same command with a filter that selects no test:
`ply test W1 --no-cache -j 1 --filter probe.NOTHING`. It still does the whole
front end — `--explain` prints `checked` for all seven modules and `--filter hid
132 tests` — and runs no test body at all. It is therefore the run's **fixed
cost**, measured rather than inferred, and `A − F` is the same `t` by a route
with no model in it:

```
t_F = A − F                         2.790 s
f   = t_F / A                      98.24 %
A / F                              56.8 ×
```

**The two instruments agree on the linear quantity to 0.36%** — 2.780 against
2.790, against a ±25% band fixed in the pre-registration before either number
existed. `Fc` (`ply check W1 --no-incremental`, a different command doing the
same front-end work) reads 0.05 s as well.

### Instrument 3 — the runner's own clock, which was already shipping

`ply test` prints each test's milliseconds. On W1 those thirteen sum to
**2,782.2 ms** against that sitting's `A − F` = 2.83 − 0.05 = **2.78 s**: a
third instrument, in the tree already, sharing no mechanism with either of the
others, landing on the same number to **0.08%**. And it is additive per file, so
the floor is not a coincidence of the total: `--filter "probe.parse 0"` reads
0.39 s user against `F` + that test's own 339.8 ms = 0.39; `--filter
"probe.parse 3"` reads 1.27 against 0.05 + 1,225.3 ms = 1.28.

### **CEILING = 56.8×**, and the honest way to quote it

| | ADR 0030 §4 | ADR 0030 §6.3 | **today** |
| --- | ---: | ---: | ---: |
| `f` | 10.78% | 97.53% | **98.24%** |
| ceiling | **1.121×** | 40× (unresolvable) | **56.8×** |

The multiplier is real and its third digit is not. The two instruments agree on
`t` to 0.36% and their ceilings are 47.3× and 56.8× — **17% apart from a 0.36%
disagreement**, which is exactly the hypersensitivity §6.3 warned about, seen
from the other side. Three statements, in increasing order of how much model
they carry, and the first is the one to quote:

1. **`B − r` = 4.15 − 4.09 = 0.06 s of 4.15. 98.6% of the backed run is inside
   `enter`, and 0.06 s of it is not.** No model.
2. **`F` = 0.05 s. The fixed cost of this command — process start, the Rust
   front end over 607,505 bytes of Ply across seven modules, test selection — is
   1.8% of the run, and everything else is now inside the fragment.** No model.
3. `A/F` = 56.8×, `1/(1−f)` = 47.3×: an infinitely fast backend takes 2.84 s to
   somewhere near 0.05–0.06 s. The brief this work was set from projected
   *"somewhere near 5–7×"* if the fragment closed. **It is cleared by roughly an
   order of magnitude**, so 5–7× should be read as a floor rather than as an
   estimate — and the number a reader should carry away is statement 1, not this
   one.

### What that does **not** say, and the trap is the same one ADR 0024 names

`f` = 98.2% is the share of the run that is now **reachable**. It is not the
share a real code generator deletes. ADR 0020 §6.3's profile — dispatch 43.8%,
refcount traffic 26.5% — bounds how much of *that* work is interpretive
overhead, and the two numbers compose rather than either one being the answer:
a native backend still has to do the parse, and only the machinery around it
goes away. ADR 0024's own warning is the shape of the error to avoid here —
*"a window share is not a request cost"*. §5.2's `Bfo` arm is what this ADR
offers instead of the composition, because it is measured rather than composed.

## 4. Against the gap

Same sitting, interleaved run by run, ten blocks, every arm in every position
once. `ply check examples` is the Rust front end over the identical 13 files —
six phases against the Ply arm's two, so the comparison is generous to Ply.

| | min user | × the Rust front end | share of the absolute gap |
| --- | ---: | ---: | ---: |
| Rust front end, cold (`--no-incremental`) | **0.09** | 1× | — |
| Rust front end, warm | 0.01 | — | — |
| Ply front end, interpreter (`A`) | **2.84** | **31.6×** | — |
| Ply front end, `--backend reference` (`B`) | 4.15 | **46.1×** | **−47.6%** — it *opens* the gap |
| Ply front end, callback-free code generator, infinitely fast | **1.35** | **15.0×** | **54.2%** |
| Ply front end, any backend, infinitely fast (`F`) | 0.05 | 0.6× | **≥100%** |

**ADR 0030 §3's 26.9× → 24.8× becomes 31.6× → 46.1× measured, and the whole of
the absolute gap is now inside the fragment.** Read carefully, because three
different things moved:

- **31.6× against §3's 26.9×** is not a regression in the front end. Arm `A` is
  the same command on a tree whose only relevant change is a gate that arm `A`
  never reaches — ADR 0030 §6.3 measured `A_after / A_before` = 1.0000×, and
  this series' own null control is 0.352%. `A` = 2.84 here against §3's 2.69
  (and §6.3's 2.83, same tree, different day), and `Rn` = 0.09 against §3's
  0.10, are two sittings a day apart on a shared machine. **Every ratio in this
  document is taken within one counterbalanced series** and none of them divides
  by a number from another one.
- **46.1× is real and is the point.** Attaching the shipping backend to the
  closed fragment makes the front end *worse* against the Rust front end than
  the interpreter is, because the fragment's only inhabitant is a tree-walker.
  §3's arithmetic run the same way: the backend closes **−47.6%** of the
  absolute gap.
- **The ceiling closes it all.** The absolute gap is `A − Rn` = 2.750 s; the
  fragment holds `A − F` = 2.790 s. The ratio is 101.5%, which is 100% plus two
  ticks of a 0.01 s instrument: `F` (0.05) sits *below* the Rust arm (0.09), so
  the residue after an infinitely fast backend is smaller than the thing it is
  being compared to. ADR 0030 §4's *"an infinitely fast backend … could never
  take it below 24.0×, 11.2% of the absolute gap"* is **withdrawn as a bound on
  this fragment** — it was true of the fragment it was measured on, which
  admitted 8.6% of body calls, and this one admits all of them.

### The cold Rust arm was never cold, and this is why §6.3 could not re-take it

ADR 0030's `gap.sh` spells the cold arm `rm -rf $root/.ply-cache`, then
`ply check $root/examples`. **That deletion has never made anything cold.** The
front-end cache is written **beside the target** — `examples/.ply-cache` — not
in the working directory, so the arm reads the *warm* figure. Arm `R` above did
exactly that and read **0.01 s** on all ten runs; deleting `examples/.ply-cache`
(registered in an amendment before the number) reads **0.10 s**, and
`--no-incremental` reads **0.09 s**. ADR 0030 §6.3's *"an attempt to re-take the
Rust arm read 0.01 s — the warm figure, `ply check` having already cached"* was
the right diagnosis with the wrong mechanism, and §3's own cold/warm pair (0.10
and 0.01) is reproduced here to the tick with the mechanism named.

## 5. What still cannot enter, and what it would take

**At the seam, on W1, with a backend attached: nothing.** Zero refusals, every
gate. That sentence is worth as much scepticism as it invites, so the shape of
it: the seam is asked 26 questions and answers yes 26 times, and the 2,414,170
body calls the census counts without a backend are 2,414,144 calls the seam is
never asked about, because they are inside `items.parse`.

So the three things that still cannot enter are one seam fact and two facts
about code generation.

### 5.1 The lambdas — a naming problem at the seam, stepped over rather than solved

Without a backend the seam refuses **380,176** anonymous bodies and 5,764
`Closure` arguments — 98.51% and 1.49% of all refusals, and no argument test can
move either. `compiled::admit` needs a program-wide name because every gate below
it is a lookup keyed by one, and a lambda publishes none. With a backend attached
it refuses **0**, because entering at the *root* asks nothing of the lambdas
inside the body. ADR 0030 §10 priced what that step-over costs, and this ADR's
`Bfo` arm re-measures it independently in §5.2: a ceiling of **2.10×**, which is
15.0× the Rust front end rather than 0.6×.

### 5.2 The code generator, which is now the whole of the limit

`crates/ply-codegen-spike/src/jit.rs` refuses, on three separate lines, exactly
what `items.parse` is made of (read-only; another workstream is live in that
file):

| line | refuses |
| ---: | --- |
| 538 | `b.higher_order()` — a builtin that calls user code |
| 1000 | `NodeKind::Lambda { .. }` → `"a lambda"` |
| 1288 | `Denotes::Local(_)` → `"a call through a local binding"` |

and `Denotes::Uncompiled` refuses the **enclosing** function rather than emitting
a trampoline, so one lambda under a root refuses the root. A cranelift backend as
that file stands would decline the entry this whole line of work opened.

**Priced, in this series, on the same ten-arm sitting.** `PLY_BACKEND_ONLY`
narrows the registry to the **223** names ADR 0030 §10's fixpoint computed for
a callback-free code generator — 220 of them land in the fragment — and the arm
reproduces §10's entry line exactly: `495152 of 1049245 offers entered · 554093
declined · 220 in the fragment`.

```
r_fo = Bfo2 − Bfo                   3.590 s
t_fo = (A − Bfo) + r_fo             1.490 s
f_fo                               52.46 %
ceiling                             2.104 ×      (ADR 0030 §10: 2.074×)
Bfo − r_fo                          1.350 s      model-free residue outside `enter`
```

**2.10× against 56.8×.** The gap between them is the price of the three refusals
above, and it is the largest single number left in this line of work.

> **Amended 2026-08-31 — ADR 0032 §3–5.** The `Bfo` arm above was taken by
> narrowing `Reference` with `PLY_BACKEND_ONLY`, because no real code generator
> could be put in that position. One can now: `PLY_CODEGEN_REGISTER=all`
> reproduces this arm's entry line — `495152 of 1049245 offers entered · 554093
> declined` — with **222** definitions against 220, on cranelift rather than on
> a narrowed tree-walker. It reads **0.604×**, not 2.104×.
>
> The arithmetic here is not withdrawn; what it prices is. **2.10× is an
> *infinitely fast* backend's number at 495,152 entries, and no real backend
> reaches it, because the boundary is paid 495,152 times whatever is on the far
> side.** Read `Bfo`'s ceiling as a bound the entry *count* makes unreachable
> rather than as a target. ADR 0032 §4 states the rule the two workloads fix:
> widening a registry helps when it lets the machine enter *higher* and hurts
> when it only adds *leaves* — on `benches/kernel` the same widening collapses
> 2,974 entries to 63 and is worth 10.0×. What it
would take is written in ADR 0030 §10.2 and is not a seam change: giving the
backend a callback *into the machine* breaks `Machine::compiled_answer`'s
`&self`, moves the `Frame::Call` push above `enter`, makes the bailout no longer
free, and starts by deleting `Machine::compiled_witness`, the tripwire built to
catch exactly that edit. **The cheap direction is to compile the callback**,
which changes nothing in `ply-eval` at all.

### 5.3 `String`, on every corpus that is not a front end

At least 86.8% of `examples/`'s 182,038 refusals name `String`, `Str` or
`Decimal` (§1). That exclusion is **deliberate and is not a seam rule either**:
`compiled::crossable`'s own doc gives the reason as ADR 0019 §5 item 4 — *"the
codegen spike has no `Float` path and lowers `+` as `Int` arithmetic whatever
the operands are"*, and `Str` and `Decimal` are compared as `Int`s by the same
lowering. It is a code generator's defect held behind a boundary, and the
boundary is doing its job. On the front end it costs **0** calls, which is ADR
0030's *"`Str` buys nothing on a front end"* confirmed on this tree and shown to
be false of everything else.

## 6. Correctness — what polices a fragment that is entered once per file

An entry count is worth nothing if the run stays green when the backend lies.
Every mutation, both corpora, one run each, through the shipping command:

| `wrong:<m>` | W1: answers changed / offers | W1 verdict | `examples/`: changed / offers | `examples/` verdict |
| --- | ---: | --- | ---: | --- |
| `off-by-one` | 0 / 26 | 13 passed — **vacuous** | 2,693 / 4,123 | **95 failed** |
| `inverted` | 0 / 26 | 13 passed — **vacuous** | 944,238 / 2,519,048 † | **74 failed** |
| `unoffered` | 0 / 26 | 13 passed — **vacuous** | 213 / 531 | **138 failed** |
| `stale` | **25** / 26 | **13 failed** | 59 / 353 | **115 failed** |
| `wrong-type` | **13** / 52 | **13 failed** | 44 / 306 | **119 failed** |
| `handle` | **13** / 26 | **13 failed** | 73 / 34,027 | **68 failed** |
| `exceeds-budget=2` | 0 / 26 | not fired | 0 / 59,435 | not fired |
| `answers=7@lexer.at` | 0 / **0 offers of the target** | not fired | — | — |

† a corrupted `Bool` changes control flow, so the corrupted run offers far more
than the honest run's 59,435 — the offer column is the corrupted run's own.

`--engine both --backend reference`: W1 **audited 13 of 13 · 0 failed**;
`examples/` **audited 166 of 186 · 20 ran on one engine only · 0 failed**.

**Every mutation that fires is caught. Three of the nine cannot fire on W1 at all,
and W1's green for those three is a vacuous pass.** This confirms ADR 0030
§6.4's finding on an independent run and adds a fourth case to it: **`answers=7@lexer.at`
gets *zero offers*.** A corruption aimed at a named lexer definition can no
longer even be presented, because `lexer.at` is inside `items.parse` now. A
workload that enters one call per file cannot police a corruption of a scalar
answer, and the corpus that can is `examples/` and `tests/fixtures/` — which is
why `crates/ply-eval/tests/suite/differential_corpus.rs` and
`crates/ply-cli/tests/suite/backend.rs` run there and not here.

The other half of the same coin, and it is `Bfo`'s: under the callback-free
registry W1 recovers all three (ADR 0030 §10.3 measured `off-by-one` at 17,599
entries, `inverted` at 6,009,842 and `unoffered` at 26, each **13 failed**). **A
fragment that enters half a million leaves polices the seam better than one that
enters thirteen roots**, and the fragment that is fastest is the one that
polices worst. That tension is now a property of this tree and should be stated
whenever a W1 result is quoted as evidence.

## 7. What would make this wrong

- **`F` is not the floor.** It would not be if `--filter` skipped work the
  timed arms do. Three checks say it does not: `--explain` prints `checked` for
  all seven modules; `Fc` — a different command — reads the same 0.05 s; and the
  runner's per-test milliseconds are additive against it to 0.4% per file and
  0.08% in total. If a future change makes `ply test` lazy about typechecking
  unselected modules, `F` collapses toward process start and this ceiling
  becomes an over-estimate.
- **`r` is measured by doubling and the second pass runs warm.** `B2 − B`
  therefore under-states `r`, which under-states `t`, which makes the doubling
  ceiling a **lower** bound. That direction was fixed in ADR 0030 §4 before any
  of these numbers existed, and here the floor instrument brackets it from the
  other side.
- **The ceiling assumes an entry saves the whole subtree.** It does under
  `Reference`, which runs the body to completion. A backend that compiled part
  of a body and called back would not reach it — see §5.2, and `compiled.rs`'s
  header for why the seam hands over no route back.
- **`f` = 98.2% is reachability, not deletability** (§3). Nothing here measures
  what native code costs, and the only number in this document that a real code
  generator can stand on is §5.2's **2.10×**.
- **W1 is 13 files of Ply this project wrote.** A front end over a corpus with
  different token statistics enters the same 26 times — one per file — and
  spends a different amount of time inside. The absolute seconds are on this
  corpus; the ratios are within one series.
- **`-j 1`.** Each worker builds its own backend; at `-j 10` the counts are per
  run and none of these times compare.
- **Three of the nine corruptions pass vacuously on W1** (§6). Anyone who reads
  W1's green as evidence about `off-by-one`, `inverted` or `unoffered` is
  reading nothing.

## 8. Decision

1. **No backend ships on the strength of this, and the reason has changed
   again.** ADR 0026 §4.5's precondition and ADR 0016 §3.5 are untouched;
   `grep -c cranelift Cargo.lock` is still 0. What is no longer the sentence to
   quote is *"1.121× is not a reason to take 31 packages into `Cargo.lock`"* —
   the ceiling is 56.8×. The sentence that replaces it is narrower and harder:
   **a cranelift backend as `jit.rs` stands could not enter `items.parse` at
   all, and the fragment it could enter is worth 2.10×.** The question a
   promotion has to answer is no longer "is the seam wide enough" — it is closed
   — but "can the code generator compile a `fold` whose step is a lambda".
2. **The next piece of work is in `crates/ply-codegen-spike`, not in
   `crates/ply-eval`.** Three refusals, in the order the fixpoint needs them:
   a closure representation, an indirect call, and `admissible_builtin`'s six
   higher-order builtins — and the usually-named one is the smallest of the
   three. Nothing at the seam has to move for any of it, and ADR 0030 §10.2 is
   the argument for why a callback *into* the machine is the expensive direction.
3. **W1 is retired as a policing workload and kept as a timing workload.** It
   cannot fire three of the nine corruptions (§6). Every claim about whether the
   seam is policed belongs to `differential_corpus.rs` and
   `crates/ply-cli/tests/suite/backend.rs` over `examples/` and `tests/fixtures/`, and
   this ADR's §6 table is the evidence for the split rather than an aside in it.
4. **The floor arm joins the two-binary method as a standing instrument.** It is
   a `--filter` that selects nothing — no code, no flag, nothing to maintain —
   and it is what made the ceiling resolvable after §6.3 could only report that
   it was not. Whoever takes the next measurement on this seam should take `F`
   beside `B2`, and should check the additivity of the runner's own per-test
   milliseconds against it. That check cost one minute and was not idle: taken
   out of gate at a load of 7.9 it read 19–21% against a 15% band, and in gate it
   reads 0.06%, which is the load showing up in a place a single arm would have
   hidden it.
5. **`ply check`'s cache location is a documented trap now** (§4). It is written
   beside the target, so `rm -rf ./.ply-cache` does not make a `ply check <dir>`
   cold. Two ADRs have now spent an arm on it.

## Provenance

Binary `target/release/ply` and two copies taken from it,
`.github/binary-is-current.sh` reporting `current (158 inputs checked)` before
and after; no series spans a rebuild. Source digests over every `.rs` in
`crates/` taken before the doubled binary was built
(`8c0f0d724871bbb614d5e2798b6ed4c1d7fd82f4`) and **equal** after it was reverted
and the tree rebuilt. All figures 2026-08-31, one machine, 10 CPUs, `uptime`
recorded on both sides of every series and printed in the logs. No git command
was run at any point in this workstream, and no clock entered `ply-eval`. Raw in
`/tmp/arc-closed/`:

- `PREREGISTRATION.md` — every statistic, its instrument, the counterbalancing,
  the null control, the admissibility rules and **the bar for "not worth it"**,
  written before the first number; one amendment written before the three
  supplementary series it registers; and an OUTCOME section written after the
  last number, naming the two predictions that came out **wrong** — arm `R`'s
  cache location, and S5's first sitting taken out of gate at a load of 7.9,
  which read 18.7% and 20.6% against a 15% band and reads 0.06% and 0.4% in gate
- `CONTEXT.txt` — load, binary state, ADR count and digest at the moment the
  pre-registration was written
- `S1.census`, `S2.census`, `S4a.census`, `S4b.census` — the four censuses, each
  taken with and without a backend
- `series.sh`, `series.tsv`, `ANALYSIS.txt` — the ten-arm, ten-block series with
  `uptime` before every block, and the arithmetic
- `s56b.tsv` — the floor's additivity and the cold Rust arm, in gate;
  `s56.tsv` — the same series out of gate, disclosed and unused
- `S7.log`, `S7b.log` — `--engine both` on both corpora and the mutation table
- `pertest.txt` — the runner's own per-test milliseconds
- `DIGESTS.txt`, `backend.rs.pre` — the tree before the doubled build, and what
  the revert was checked against

ADR 0030 is the parent of every number here: §1 for the entry line and the
census, §3 for the gap arms, §4 for the doubling instrument and the model, §5
for `Reference`'s two speeds, §6–§6.4 for the two widenings this ADR measures,
and §10 for the callback-free fixpoint that `Bfo` re-measures. Nothing in it is
withdrawn by this document except the bound in §4 — *"could never take it below
24.0×"* — which was a bound on a fragment that no longer exists, and §3's cold
Rust arm, whose spelling is corrected in §4 above.
