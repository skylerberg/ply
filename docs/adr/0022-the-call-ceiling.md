# ADR 0022 — The call ceiling, and the loop that does not spend it

**Status:** accepted. It adds one builtin, `iterate`, and it refuses one thing
that has been asked for twice — a `--max-calls` flag.
**Date:** 2026-08-27.
**Corrects in place:** `docs/adr/0020-self-hosting-the-front-end.md` §5.1, §6.3
and §7 item 4; `docs/adr/0021-why-bootstrap.md` §3 and §5;
`docs/adr/0013-w3-contract.md` §4; `spikes/ply-lexer/GAPS.md` §5;
`crates/ply-std/ply/http.ply`'s `Limits` comment; `examples/desk.ply`'s copy of
the same claim; `CONTRIBUTING.md`'s ADR count.
**Cross-references, in the direction nobody had:**
`docs/adr/0005-control-stack-and-world.md` §7.1, which had already decided the
tail-call question that ADR 0020, ADR 0021 and `spikes/ply-lexer/GAPS.md` §5
each re-derived without citing it.

---

## §0 The two claims, before any measurement

Written before the numbers below existed, in
`PREREGISTRATION.md` at the root of this worktree, which carries the statistic,
the run count and the decision rule for each of M1–M7 and the load gate they
were taken under. What follows is what those rules produced. Where a figure I
was handed disagrees with a figure I took, the one I took is what this ADR
carries and the handed one is recorded beside it as withdrawn.

**Claim 1 — Ply has no early-terminating loop, and that is a cost, not a
missing convenience.** The list surface is exactly `len`, `push`, `map`,
`filter`, `fold` and `range` (`crates/ply-eval/src/builtins.rs`). `fold` visits
every element.

> **The enumeration in Claim 1 is a snapshot and has moved twice
> (2026-08-30).** §1 of this ADR adds `iterate` to it, which is the whole point
> of the document and is said below; `docs/adr/0027-a-list-index.md` has since
> added `list_at`. The claim the sentence supports — that
> `fold` visits every element and that this was a cost — is unaffected, and is
> the reason the sentence is annotated rather than rewritten.

So a search, a scan or a parse written over one runs to a conservative bound and
no-ops after its real work is finished.

**Claim 2 — the call ceiling has leaked out of the interpreter and into a
public API.** `crates/ply-std/ply/http.ply`'s `Limits` record declared
`max_stream_chunks: 2048`, and its own comment said the field was "also a
recursion bound: `stream_chunks` is a tail call and the evaluator caps nested
calls at 10,000". The largest usable value of a field in an HTTP server's
configuration was a fact about `ply_eval::limit::DEFAULT_MAX_CALLS`.

Both are now false, and §1 is what makes them false.

### §0.1 The load gate, and what it cost this ADR

`PREREGISTRATION.md` §1 sets the threshold `CONTRIBUTING.md` §"Gate on an idle
machine" names: **1-minute load average below 4.0 at the start of a series and
below 6.0 at its end.** It was written when the reading was 6.45, already above
the gate. **The machine never came down.** Sampled every 10 s across the
session, the 1-minute average ranged 10.7–27.8 and the 5-minute average
17.5–23.5, with nine users logged in.

So the wall-clock series below are reported with that fact attached, and the
consequences are not the same for all of them:

- **M5b and M6 are peak RSS, not wall clock.** Load does not move resident
  memory, and each figure is the minimum of three processes. They stand as
  measured, and they are what §3 and §4 rest on.
- **M1, M2, M5a, M5c and M7 are deterministic** — does it answer, and with
  what — and a contended machine produces the same answer as an idle one. They
  stand, and they are what the decision rests on.
- **M3 stands, in the conservative direction.** Its threshold is *"under
  1.0 s"*, and contention can only make a wall clock larger. A run that clears
  an upper bound at load 15 clears it at load 0.
- **M4 does not stand, and its figure is withdrawn.** It is a *ratio* of two
  contended measurements and there is no direction in which contention is
  conservative for it. Labelled **UNMEASURED**, raw windows in §4.1, per the
  rule written down before the data. **No threshold was re-cut after seeing
  it.**

---

## §1 `iterate`, and why its budget is an argument

```ply
iterate(seed, budget, step)    where step : (s) -> Iter<s, r> / e
type Iter<s, r> = Continue(s) | Stop(r)         // prelude, not a module
```

Type, pinned at `crates/ply-core/tests/suite/iterate_builtin.rs`:

```
(a, Int, (a) -> Iter<a, b> / e) -> b / e
```

### §1.1 It rides the protocol `fold` already rides

`iterate` is not a new mechanism. `crates/ply-eval/src/builtins.rs`
answers a `Step::Apply` carrying a `Frame::IterateStep`
(`crates/ply-eval/src/cont.rs`), exactly as `next_fold` answers one
carrying a `Frame::FoldStep`. The machine pushes that frame and pops it again
each round (`machine.rs run_builtin_step`, dispatched at
`frame.rs`'s builtin-step arm); the tree-walker keeps the round on its host
stack in a `loop` (`builtins.rs Interp::call_builtin`). **Neither nests.**
One definition, two engines, which is the property `crate::differential` exists
to protect.

`DEFAULT_MAX_CALLS` and `MAX_VALUE_DEPTH` are untouched.

### §1.2 The budget is the second argument, and that is not a style choice

`crates/ply-eval/src/region_kind.rs walk_callback` reads a higher-order
builtin's function out of `args.last()`. Two tests assert that every
higher-order builtin has it last —
`region_kind.rs the_callback_builtins_are_the_six_this_module_knows` and
`builtins.rs exactly_the_callback_builtins_are_higher_order`. A
callback in the middle would be read as data and the `Int` read as the
callback, and region-kind inference would record `Cause::Indirect` **silently**.
So `iterate(seed, budget, step)`, matching `fold(xs, init, f)` and
`bytes_position(b, from, f)`.

### §1.3 Why the bound is an argument and not a constant

A loop that cannot end is worse than no loop. ADR 0005 §7.1 records what that
costs here: `fn spin(n) = spin(n + 1)` under tail-call elision "ran past a
45-second wall clock with no diagnostic, where the tree-walker answered in
3.8ms". **There is no per-test timeout anywhere in `ply-test` or `ply-cli`** —
`grep -rni timeout crates/ply-test/src/` returns **zero** hits, re-run for this
ADR, and `crates/ply-cli/src/` has three, all about a Ply program's own
`body_timeout_ms` — so a hang does not fail a test, it hangs the suite.

`iterate` therefore takes the bound from the program. M3, on the release binary
with the change, N = 5, **every run recorded and none discarded**. Taken twice
because the machine's load doubled between them, which is the whole reason the
gate exists. (The two takes are on two builds — one before and one after a
`#[cfg(test)]`-only edit — which `cargo build --release -p ply-cli` does not
compile, so the evaluator is the same in both. The instrument check was clean
for each.)

| program | wall, all five runs @ load 15 | again @ load 30 | diagnostic |
| --- | --- | --- | --- |
| `fn spin(n) = spin(n + 1)` — today's control | 0.04, 0.01, 0.01, 0.01, 0.01 | 0.06, 0.03, 0.03, 0.03, 0.03 | `recursion limit of 10000 nested calls exceeded` |
| `iterate(0, 10000, never)` — **M3 as registered**, "at the default budget", 10,000 being `DEFAULT_MAX_CALLS` | 0.01 ×5 | 0.03, 0.03, 0.03, 0.02, 0.04 | `` `iterate` took its budget of 10000 steps without stopping `` |
| `iterate(0, 1000000, never)` — supplementary, 100× the ceiling | 0.68, 0.71, 0.72, 0.71, 0.71 | 2.06, 1.64, 1.51, 1.32, 1.29 | `` `iterate` took its budget of 1000000 steps without stopping `` |

**The registered instrument passes and passes conservatively**: median 0.03 s
against a 1.0 s threshold, at *seven and a half times* the gate's load of 4.0,
and contention can only inflate a wall clock. **The supplementary row does not, at load 30** — it
crosses 1.0 s — and that is recorded rather than dropped. What it measures is
not whether a runaway terminates but how long a budget the *program chose* takes
to spend: about 1.4 µs a round. A program asking for a 100-million-round budget
would wait proportionally, which is the honest shape of this bound and is worth
knowing.

The control series also **fails** the pre-registered 10% instrument-drift check
in both takes (0.04→0.01, 0.06→0.03 — a cold first run each time), so its number
is UNMEASURED; its windows are above and the conclusion does not rest on it.

**The load-independent half of the claim is a test, not a stopwatch**:
`equivalence_audit.rs an_iterate_whose_step_never_stops_is_a_diagnostic_on_both_engines`
asserts that both engines produce the diagnostic, and that it does **not**
contain the string "recursion limit". An exhausted budget is
`crates/ply-eval/src/limit.rs err_iterate_budget`; a budget below one is
`limit.rs err_iterate_budget_not_a_count`, refused before the first round
because zero steps is a bound nobody writes on purpose. Neither says
**"recursion limit"**, deliberately: nothing nested, and things classify on that
string. `limit.rs`'s own correction block names four
(`ply-cli/tests/failure_classification_audit.rs`, `ply-test/tests/hybrid.rs`,
`ply-test/src/tests.rs`, `ply-eval/src/tests.rs`) — that list is quoted from
there and was not independently re-derived here; what *is* checked is that the
new diagnostics do not contain the phrase, asserted at
`equivalence_audit.rs` and at
`builtins.rs::an_iterate_that_never_stops_exhausts_its_budget_and_says_so`.

And the argument is the whole link to §5: **a number in the source is a number
in the `DefHash`.** A program that raises its own bound invalidates its own
cached results. A flag could not do that.

### §1.4 What it costs

A new prelude type name. `Iter` joins `builtin_types()`, so a project's own
`type Iter` is now `E0105`
(`crates/ply-core/tests/suite/iterate_builtin.rs::a_project_may_not_declare_its_own_iter`).
`grep -rnw Iter` over the tree found the name free outside prose. `Step` and
`Loop` were **not** available: `crates/ply-std/ply/json.ply` and
`examples/twin_divergence_audit.ply` declare `type Step`, and
`crates/ply-eval/tests/suite/reference_cycles.rs` declares `type Loop` — naming
the ADT `Step` would have broken a shipped `std` module.

Constructor names are a separate namespace and are **not** globally reserved, so
`std.signal`'s `pub type Stop` still checks; that was verified rather than
assumed
(`iterate_builtin.rs::a_module_may_still_declare_its_own_stop_and_continue`).

> **The price of that, added by adversarial review (2026-08-27), because the
> paragraph above is true and its silence misleads.** `std.signal`'s `pub type
> Stop` is a *type alias*, and a type name never collides with a constructor —
> so that case says nothing about the case that bites. A module declaring its own
> **constructor** named `Continue` or `Stop` keeps that constructor, and pays for
> it by losing `iterate` entirely: the local name shadows the prelude's, so
> `Continue(s + 1)` in that module has the module's own type and will not unify
> with `Iter<a, b>`. Measured on the release binary, not argued:
>
> ```
> type Phase = Continue(Int) | Done
> fn loop_here(n: Int) -> Int =
>   iterate(0, n + 1, |s: Int| if s >= n { Stop(s) } else { Continue(s + 1) })
> ```
> ```
> [E0201] type mismatch: branches of `if` must agree
>         expected `Iter<a, Int>`, found `shadow.Phase`
> ```
>
> **And there is no way to name the prelude's constructor past the shadow.**
> `Iter.Continue(..)` is `[E0103] unknown effect `Iter`` — the qualified form is
> parsed as an effect operation — and annotating the return type does not help,
> because the constructor is resolved before the annotation is consulted
> (`fn mk(s: Int) -> Iter<Int, Int> = Continue(s + 1)` is the same `E0201`). The
> only remedy is to rename the module's own constructor. Both spellings were run
> on `target/release/ply`.
>
> Nothing in the tree pays this today — no module in `crates/ply-std/ply/` or
> `examples/` declares a `Continue` or `Stop` *constructor*, checked with
> `grep`. It is written down because `Continue` and `Stop` are ordinary words in
> a way `Some`, `None`, `Ok` and `Err` are not, so this will be met by somebody,
> and "constructor names remain shadowable" reads as *no cost* when the cost is
> that the builtin this ADR exists for becomes unwritable in that module.

The cheaper design — `step : (s) -> Option<s>`, adding no name at all — was
rejected because `None` can only answer the seed the step was *handed*, so a
loop wanting to stop with a computed value must store it and go round once
more. One wasted iteration per loop is the exact defect this ADR is about.

`RUNTIME_VERSION` 0.11.2 → 0.12.0 and `FRONTEND_VERSION` 0.15.0 → 0.16.0. Both,
and the change cannot be split to avoid either: a cached `Pass` written before
`iterate` existed is a claim about a program in which the name meant nothing,
and a cached interface written before `Iter` existed is one for a program this
front end now reads differently.

**`PROVER_VERSION` is deliberately not bumped**, and that is a decision rather
than an omission. Adding to `prelude::ADTS` does reach the prover —
`ply-prove/src/prove/context.rs drop_incomplete` (the table is read at `:245`) reads the table to know a
prelude ADT's declaration is complete, so the case split will now split on an
`Iter`. But no *existing* obligation can change: splitting on `Iter` requires
source that mentions it, and any program that previously declared its own
`type Iter` is now `E0105` and does not check at all, so its cached obligations
are unreachable rather than wrong. Nothing else in `ply-prove` reads the new
entry.

The compiled fragment **refuses** it: `crates/ply-codegen-spike/src/jit.rs
admissible_builtin` returns `Err` for any `b.higher_order()`, which `iterate`
is. That is expected and is not a regression — `fold` is refused by the same
line. (ADR 0020 cites this function as `jit.rs`; it is at `:537`, corrected
there in place.)

---

## §2 ADR 0020 §5.1's premise is wrong

§5.1 is the section that turns the ceiling from "a tax" into "an architecture
constraint", and the load-bearing sentence is:

> A recursive-descent parser consuming *N* top-level definitions, or *N* list
> elements, or *N* arguments, recurses once per element unless it is folded.

and:

> The lexer's fold-over-a-range escape hatch does not generalise, because a
> recursive-descent parser's recursion *is* the grammar.

**The reference implementation this project ships refutes it.**
`crates/ply-syntax/src/parser.rs` reserves recursion for grammar *nesting* and
drives every sequence with a loop. Counted with `grep -nw`, re-verified for this
ADR:

| driver | where | what it consumes |
| --- | --- | --- |
| `while` ×16, `loop` ×5 | — | every sequence in the grammar |
| `:265` | `run` (`:261`) | top-level items |
| `:297` | `imports` (`:295`) | import declarations |
| `:325` | `import_decl` (`:322`) | a dotted path's segments |
| `:933` | `effect_def` (`:926`) | an effect's operation list |
| `:1533` | `block_inner` (`:1529`) | block statements |
| `:1984` | `list_pattern` (`:1980`) | list-*pattern* elements |
| `:2065` | `comma_list` (`:2059`) | **fourteen call sites** — argument lists (`:1345`), list literals (`:1456`), record literals (`:1493`), `fn` params (`:690`), generic arguments (`:1087`), lambda params (`:1586`), pattern arguments (`:1964`), and seven more |

One iterative helper, `comma_list`, covers most sequences in the language.

Precedence climbs iteratively too: `bin_expr` (`:1222`) walks the left spine
with `while let Some((op, bp)) = bin_op(self.kind())` at `:1224`. It recurses
for the *right* operand at `:1229`, so its depth is bounded by the number of
binding powers, which `bin_op` (`:2096`) fixes at **6** — not by operand count.
ADR 0020's "at perhaps 15 precedence levels that is ~255 frames" overstates it
by more than half.

And the strongest fact, which neither ADR had: **the reference parser bounds
grammar nesting itself.** `const MAX_DEPTH: u32 = 128` (`parser.rs`),
enforced by `deeper()` (`:244`) at exactly three sites — `ty_inner` (`:1035`),
`unary_expr` (`:1244`) and `pattern` (`:1846`). Against ADR 0020's own measured
corpus maximum of **17**, and a call ceiling of 10,000, grammar nesting in this
design cannot reach the ceiling. It refuses at 128 first, seventy-eight times
below the bound §5.1 says is an architecture constraint.

### §2.1 What was wrong with the count this ADR was handed

The brief that produced this workstream said `parser.rs` uses "16 `while`, 5
`loop` and 6 `for`, one per sequence". The `while` and `loop` counts hold. The
`for` count does **not**: `grep -nw for` returns ten hits and exactly **one** is
a loop — `:62`, `for (source, name, text) in inputs`, which iterates over input
*files*, not over a grammar sequence. The other nine are inside string literals
and doc comments (`:406`, `:487`, `:528`, `:555`, `:603`, `:608`, `:609`,
`:612`, `:979`). Two of the line numbers were also mis-attributed: `:933` is an
effect's operation list, not block statements (those are `:1533`), and `:1984`
is `list_pattern`, not list expressions. Recorded because
`CONTRIBUTING.md` §"The one rule" opens by counting written claims that did not
hold, and an ADR repeating line numbers nobody re-read would be one more.

Two more were caught by re-reading this ADR's own first draft rather than the
brief: `effect_def` is at `:926`, not `:928`, and the first `deeper()` site is
inside `ty_inner` (`:1035`), not `ty_body`. Both are corrected above.

> **A fifth, found by adversarial review (2026-08-27).** The two sentences above
> and §2's citation read *"`ty_inner` (`:1034`)"*. `:1034` is `fn ty_inner`'s
> signature; the `self.deeper()?;` call is at `:1035`, which is where the
> sibling citations `:1244` and `:1846` already pointed for `unary_expr` and
> `pattern`. Corrected in all three places. That a section arguing for the
> mechanical check was itself one line out is the point it was making. Three
sources of line numbers for one file produced four wrong ones between them,
which is the argument for the mechanical check rather than for reading harder.

---

## §3 `fold` is depth 1 on both engines — measured, not assumed

M5, on the release binary at the pre-change tree, instrument check clean.

**M5a.** `fold(range(0, 500000), 0, add)` — fifty times `DEFAULT_MAX_CALLS` —
**completes on both engines**. That it completes is the registered statistic and
it is deterministic. (`ply test` also printed 55.8 ms on the tree-walker and
185.1 ms on the machine; those are wall clocks taken above the load gate and are
**not** a claim — they are here only so the run can be recognised.)

**M5b, peak RSS** (`/usr/bin/time -l`, one process per figure, N = 3, minimum
taken because peak RSS is a maximum over a run and its noise is one-sided
upward):

| engine | peak RSS |
| --- | --- |
| tree-walker | 23,347,200 B = **22.3 MiB** |
| machine | 23,494,656 B = **22.4 MiB** |

The ~23 MB this ADR was handed holds.

> **Independently re-taken by adversarial review (2026-08-27) and it did NOT
> reproduce.** The figures above are left exactly as measured; this is a second
> observer's window beside them, not a correction of them, because there is no
> basis for saying which run was wrong. Same machine, same command
> (`ply --color never test --engine <e>` over `fold(range(0,500000), 0, add)`,
> `.ply-cache` removed before every run), instrument check clean, re-take
> pre-registered before any number existed:
>
> | binary | engine | N | minimum peak RSS |
> | --- | --- | --- | --- |
> | post-change (this worktree) | tree-walker | 10 | 24,821,760 B = **23.7 MiB** |
> | post-change (this worktree) | machine | 3 | 25,624,576 B = **24.4 MiB** |
> | pre-change (`d88aae5`, `ply/target/release/ply`) | tree-walker | 3 | 25,427,968 B = **24.2 MiB** |
> | pre-change (`d88aae5`, `ply/target/release/ply`) | machine | 3 | 26,181,632 B = **25.0 MiB** |
>
> The review's pre-registered rule was *reproduced iff within 5%*; the gap is
> **+6.3%** (tree-walker) and **+9.1%** (machine), so by that rule it is **not
> reproduced**. It is not the change's doing: the pre-change binary measures
> *higher* than the post-change one, so `iterate` costs nothing here. Nor is it
> too few runs — over ten tree-walker processes the whole observed range was
> 24,821,760–26,214,400 B and the figure above lies below all of it. The runs are
> otherwise the same shape: `ply test` printed 60.3 ms / 221.8 ms against the
> 55.8 ms / 185.1 ms recorded above.
>
> **Nothing in this ADR turns on it.** M5b was pre-registered *reported, not
> gated*; §3's claim is that a 500,000-element `fold` completes at depth 1 in
> tens of megabytes, and 23.7 MiB says that as well as 22.3 MiB does. What is
> withdrawn is only the precision: read the row above as *~23–25 MiB on this
> machine*, and do not quote either figure to three significant digits.

**M5c, nested folds**, found by bisection on the machine and then confirmed on
the tree-walker, over

```ply
fn nest(d: Int) -> Int = if d <= 0 { 0 } else { fold([1], 0, |acc: Int, x: Int| nest(d - 1)) }
```

| depth | machine | tree-walker |
| --- | --- | --- |
| 4,999 | answers | answers |
| 5,000 | `recursion limit of 10000 nested calls exceeded` | raises |

Exactly **2 calls per nesting level** — the `nest` call and the lambda `fold`
applies — against a 10,000 ceiling. Both engines change their answer at the same
integer, which is the agreement `--engine both` exists to check.

So the escape hatch ADR 0020 §5.1 says "does not generalise" already carries
50× the ceiling flat, and nests to 5,000 levels against a corpus whose deepest
observed nesting is 17.

---

## §4 What raising the ceiling would cost

M6. Peak RSS per pending nested call, `/usr/bin/time -l`, one process per
figure, N = 3 per depth, minimum taken. Four cells: {machine, tree-walker} ×
{a body pending one operand, a body pending twenty}, over `hog(k, depth)` —
`fn hog(n) = if n == 0 { 0 } else { hog(n - 1) + 1 + 1 … }`.

A **slope over three depths — 2,000 / 4,000 / 8,000 — and not a division at one
window**, because `CONTRIBUTING.md` §"Measure an ADR's motivating claim" records
that a ranking taken at one window is not a cost.

| cell | bytes per pending call | fitted intercept | max residual |
| --- | --- | --- | --- |
| machine, one operand | **339** | 7.62 MB | 0.16% |
| machine, twenty operands | **3,174** | 7.48 MB | 0.12% |
| tree-walker, one operand | **3,206** | 7.00 MB | 0.52% |
| tree-walker, twenty operands | **11,792** | 7.13 MB | 0.40% |

**All four are linear** by the rule written down before the data — every point's
residual from its fitted line within **1.5%** of that point's measured value.
The worst is 0.52%.

**This ADR was handed figures for the same four cells and re-took them rather
than carrying them.** Three of the four agree within a couple of percent and one
does not. **The measured ones are what this ADR carries, per the pre-registered
rule that a handed figure is not a target.**

The shape is what matters, not the constants: raising `DEFAULT_MAX_CALLS` from
10,000 to 100,000 costs, at the worst cell, 90,000 × 11,792 B ≈ **1.06 GB** of
resident memory for one runaway to reach the new diagnostic, and the tree-walker
grows its native stack in 2 MiB segments to get there
(`limit.rs::grow`). That is the price of the ceiling as a *number*.

**And it is the wrong axis.** §3 shows a `fold` carrying 500,000 elements in
22 MiB at depth 1, and §1 adds a loop that stops early at the same depth. The
ceiling does not need raising for the workload that motivated raising it.

### §4.1 What a wasted iteration costs — UNMEASURED, and why that is recorded

M4 was pre-registered to price the motivation in §0 claim 1: two 500,000-step
`fold`s, one where every step does the real work and one where 5,000 do and
495,000 return the accumulator unchanged — the shape `spikes/ply-lexer/GAPS.md`
§5 describes, where desk.ply pays 140,108 no-op iterations out of 159,684.

The runs, N = 5 each, `/usr/bin/time -p`, release binary, instrument check clean
(`find crates -name '*.rs' -o -name '*.ply' -newer target/release/ply` empty —
`.ply` added to the registered check because `ply-std` embeds those files into
the binary and the `.rs`-only form would not have caught a stale one):

| series | user CPU, all five runs | wall, all five | first→last drift |
| --- | --- | --- | --- |
| 500,000 real steps | 0.91, 0.93, 0.93, 0.93, 0.94 | 0.96, 0.97, 0.96, 0.97, 0.98 | 3.3% |
| 5,000 real + 495,000 no-ops | 0.48, 0.49, 0.48, 0.47, 0.47 | 0.51, 0.51, 0.51, 0.50, 0.49 | 2.1% |

Load 12.9 before, 12.5 after. The pre-registered instrument-drift check (final
run within 10% of the first) **passes** on both. The **load gate does not**, and
that is what decides this: by the pre-registered formula the medians give
**0.951 µs per wasted iteration, 51.1% of a real step**, and that figure is
**labelled UNMEASURED** and is not carried as a claim by this ADR. Raw windows
are above so it can be re-taken on an idle machine.

**A handed reading for the same quantity disagrees, and neither figure is
quotable from this session.** The brief named no step function and the
reproduction here allocates a record per real step, **so the disagreement is at
least partly a difference of program and not of machine.** What is not in doubt —
**because it needs no stopwatch** — is the *count*: the overwhelming majority of
the lexer loop's iterations are no-ops, **which is a fact about the program.**

**Nothing in this ADR is refused or accepted on M4.** It priced a motivation
that §3's depth-1 measurements and §7's `recursion limit of 10000 nested calls
exceeded` establish without it.

---

## §5 A bare `--max-calls` flag is refused

Asked for by ADR 0021 §4 item 2 ("the nested-call ceiling — 10,000, no flag")
and by `spikes/ply-lexer/GAPS.md` §5 ("no flag to raise it"). Refused, and this
is the reason:

**Results are cached as `(RUNTIME_VERSION, DefHash) -> Outcome`, and shipping
code writes only `Outcome::Pass`.** Verified: `grep -n 'Outcome::Pass'
crates/ply-test/src/lib.rs` finds five hits, of which exactly two are
`store.put` writes — `:1429` and `:1558` — and the other three
(`:1183`, `:1199`, `:1209`) only ever read or compare what `store.get`
answered. `ply-test` says so about itself at `:1200-1202`, on the arm that
handles a stored failure: *"Never trust a stored failure. Nothing here writes
one, so it can only have come from an older or foreign writer, and re-running
is the only safe reading of it."*

That asymmetry decides it:

- **Raising the bound is monotone.** A program that passed under 10,000 passes
  under 100,000; more budget cannot turn a pass into a failure. A cached `Pass`
  stays true.
- **Lowering it is not.** A program cached as `Pass` at 10,000 may raise
  `E0502` at 1,000, and the cache would answer `Pass` without running it — a
  green result over unexplored space, which `CONTRIBUTING.md` names as this
  project's signature defect.

A flag that is safe in one direction and silently wrong in the other is not a
flag. Making it safe means keying results on it, and the precedent for what that
costs is in the tree already: `crates/ply-prove/src/key.rs prove_key` hashes
`PLAN_DOMAIN`, the `DefHash` and the plan's digest together, so a discharge
earned under one plan is never read by a run under another —
`result_key` (`:33`) then writes a sampled tier under the *plan* key and only a
`Proved` one under the bare key, so a widened plan re-attempts rather than
inheriting. That is the shape a `--max-calls` result would need: a second key
space, a domain separator, and a rule for which outcomes may be written bare.

`iterate`'s budget gets the same property for free, because it is an argument.
It is in the definition's text, so it is in its `DefHash`, so a program that
changes its own bound has already invalidated its own cached result. **That is
why §1.3 puts the number in the source and not on the command line.**

---

## §6 ADR 0005 §7.1 had already decided this, and three documents re-derived it

`docs/adr/0005-control-stack-and-world.md` §7.1 (`:717-757`) records, and none
of ADR 0020, ADR 0021 or `spikes/ply-lexer/GAPS.md` §5 cites it:

- Tail-call elision **existed** in the machine.
- It was **removed**, for two measured reasons: the machine reused the pending
  `Call` frame of a tail call, so no frame budget could fire for one and
  `fn spin(n) = spin(n + 1)` "ran past a 45-second wall clock with no
  diagnostic, where the tree-walker answered in 3.8ms"; and the two bounds
  counted different things at different scales, so a program between them was a
  diagnostic on one engine and an answer on the other.
- Restoring it "belongs to the change that deletes the tree-walker, **together
  with the fuel budget a tail loop would then need**".

So the bootstrap track re-derived a settled question. GAPS.md §5's title — "there
is no loop, there is no tail-call elimination, and the ceiling is 10,000 nested
calls with no flag to raise it" — describes ADR 0005's decision as if it were an
oversight, and ADR 0021 §4 ranks removing it as critical-path work.

### §6.1 Why `iterate` is not that decision re-opened

This is the load-bearing judgement in the whole change and it is argued rather
than assumed. ADR 0005 §7.1 refused tail-call **elision**: making a call cost
nothing. `iterate` elides no call. It is the **fuel budget without the
elision** — the second half of §7.1's own sentence, available now because it
needs no engine deleted:

| | TCE, as removed | `iterate` |
| --- | --- | --- |
| what a call costs | zero on the machine, one on the tree-walker | one on both, and the loop is not a call |
| what bounds a runaway | nothing | the budget, an argument |
| how the engines count | differently — that was the defect | identically: one `Frame::IterateStep`, pushed and popped |
| how it is checked | it was not | `equivalence_audit.rs`, `:2234`, `:2260`, `:2295` |

The difference is asserted rather than claimed.
`equivalence_audit.rs an_iterate_of_five_hundred_thousand_steps_is_depth_one_on_both_engines`
runs a **500,000-step** loop under `with_max_calls(8)` on both engines — 50×
`DEFAULT_MAX_CALLS` above, three orders of magnitude below — and passes. Its
arming leg is what makes that non-vacuous: the **same loop** written as the tail
recursion `iterate` replaces, at the **same cap**, raises `recursion limit of 8
nested calls exceeded` on both engines. `:2234` adds the frame count, which the
call count does not imply: 500,000 rounds under `Machine::with_max_frames(8)`,
where the recursive control raises `this engine's ceiling of 8 pending frames
was reached`.

Both were **seen to fail**. A driver that charges a nested call and a pending
frame per round — the tail-recursive shape `iterate` replaces, simulated in
`frame.rs`, `cont.rs::is_call` and `Interp::call_builtin` — turned `:2194` red
with exactly `recursion limit of 8 nested calls exceeded` and `:2234` red with
`this engine's ceiling of 8 pending frames was reached`. The three files were
then restored to byte-identical digests and the pair re-run green.

**TCE stays out.** ADR 0005 §7.1's decision is not altered by this ADR.

---

## §7 What this changes in `std`

`crates/ply-std/ply/http.ply stream_chunks` and `:1613 stream_raw` are now
`iterate` drivers. `max_stream_chunks` (`:102`) is a policy number.

M7, and it is the one clause of the exit criterion with two halves that are both
required. `respond_chunked` with `max_stream_chunks: 50000` — five times
`DEFAULT_MAX_CALLS` — over a producer of **20,000** chunks:

| | result |
| --- | --- |
| pre-change tree, release binary | **`recursion limit of 10000 nested calls exceeded`**, innermost calls `stream_chunks` from `stream_chunks` from … |
| after, machine | passes, terminating chunk present |
| after, tree-walker | passes, terminating chunk present |
| after, `--engine both` | passes, no divergence |

Without the first row the second is a claim about a bound nothing reached, and
the user-visible motivation in §0 would be false. It is now
`crates/ply-cli/tests/suite/w3_http_audit.rs`.

**The terminating-chunk guarantee survives, and it is why the budget is spent
inside the step.** `w3_http_audit.rs::a_streamed_response_always_ends_with_its_terminating_chunk`
asserts that a response cut short still writes `0\r\n\r\n`, because a framed and
unterminated chunked response on a reusable connection is response smuggling. An
exhausted `iterate` budget is a *diagnostic*, which would abort the run with the
message unterminated. So `stream_chunks` carries `left` in its seed, answers
`Stop(false)` at zero having written `last_chunk()`, and hands `iterate` a
budget of `fuel + 1` — one more round than the step can possibly take, a
backstop that cannot fire.

### §7.1 What was deliberately not done

`serve` and `connection_loop` in the same file have the same tail-recursive
shape and the same comment. **They are out of scope and are not converted.** The
same rewrite applies to both; neither was done here. Their bounds —
`max_keep_alive` at 100, and a caller's `count` — sit far enough below 10,000
that neither is shaped by the ceiling the way `max_stream_chunks` was, which is
why they were ranked below it and not why they are fine. Said in these words so
that silence does not imply the file is finished.

---

## §8 What would make this ADR wrong

- **If `iterate`'s depth turns out not to be 1 on some engine or backend.** The
  depth claim is asserted at two tests — `equivalence_audit.rs` for the
  call count on both engines, `:2234` for the machine's frame count — and both
  were seen to fail under a driver that nests. A third execution strategy would
  have to be checked against them rather than assumed into them; the compiled
  fragment is not one, because it refuses `iterate` outright (§1.4).
- **If a Ply parser is written and the ceiling bites anyway.** §2 refutes ADR
  0020 §5.1's *premise* about recursive descent. It does **not** port a parser,
  and nothing here says one is feasible — ADR 0020's throughput finding is
  untouched and remains the reason not to.
- **If `--engine both` finds a divergence on an effectful step.** `cont.rs`
  records that a continuation captured inside a builtin's callback cannot be
  re-entered by the tree-walker. `iterate`'s step is user code with an open row,
  so that surface is newly reachable. It is covered by
  `equivalence_audit.rs::an_effect_performed_inside_an_iterate_step_agrees_on_both_engines`,
  and the corpus differential shard **was** run —
  `cargo test -p ply-corpus --test differential_sweep`,
  `the_two_engines_agree_over_a_sweep_of_generated_corpora`, green. **That
  shard's generator does not emit `iterate`**, so what it establishes is that
  nothing already in the corpus regressed, not that `iterate` was swept.
  Widening the generator to emit one is the cheap next step and was not taken
  here — a named gap rather than a silence. The other newly-reachable surface,
  a scheduling point *inside* a loop's step, is covered:
  `simulation.rs::two_iterate_loops_interleave_and_each_keeps_its_own_countdown`
  searches two tasks running three-round `iterate` loops over one cell
  exhaustively, and was seen to fail when the countdown is decremented by two.
- **If the two version bumps prove to have been unnecessary.** They discard
  every cached result and every cached type in every checkout. That is the
  intended cost and it cannot be split, but it is the largest thing this change
  spends.
