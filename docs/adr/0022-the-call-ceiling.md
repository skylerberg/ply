# ADR 0022 — The call ceiling, and the loop that does not spend it

**Accepted.** It adds one builtin, `iterate`, and it refuses one thing that has
been asked for twice — a flag to raise the call ceiling.

It also **corrects a settled question three documents re-derived without citing
it**: ADR 0005 had already decided the tail-call question, and the
self-hosting record, the bootstrap record and the lexer spike each re-opened it
as though it were an oversight.

## The two claims, before any measurement

Written before the numbers existed, with the statistic, the run count and the
decision rule for each fixed in advance.

**Claim 1 — Ply has no early-terminating loop, and that is a cost, not a missing
convenience.** The fold visits every element, so a search, a scan or a parse
written over one runs to a conservative bound and no-ops after its real work is
finished.

**Claim 2 — the call ceiling has leaked out of the interpreter and into a public
API.** The HTTP limits record declared a maximum chunk count whose own comment
said the field was *also* a recursion bound, because the streaming function is a
tail call and the evaluator caps nested calls. **The largest usable value of a
field in an HTTP server's configuration was a fact about a constant in the
evaluator.**

Both are now false.

### The load gate, and what it cost

The threshold was written down when the reading was already above it, **and the
machine never came down** — sampled across the session, the one-minute average
ranged from three to seven times the gate. So the series are reported with that
fact attached, and the consequences are not the same for all of them.

- **Peak-memory figures stand as measured.** Load does not move resident memory.
- **Deterministic figures stand** — does it answer, and with what. A contended
  machine produces the same answer as an idle one.
- **An upper-bound wall clock stands, in the conservative direction.** A run
  that clears an upper bound at high load clears it at zero.
- **A *ratio* of two contended measurements does not stand, and its figure is
  withdrawn.** There is no direction in which contention is conservative for it.
  Labelled unmeasured, with the raw windows kept so it can be re-taken.
  **No threshold was re-cut after seeing it.**

## `iterate`, and why its budget is an argument

```ply
iterate(seed, budget, step)    where step : (s) -> Iter<s, r> / e
type Iter<s, r> = Continue(s) | Stop(r)
```

**It rides the protocol the fold already rides.** The builtin answers a step
carrying its own frame, exactly as the fold does; the machine pushes and pops
that frame each round and the tree-walker keeps the round on its host stack in a
loop. **Neither nests.** One definition, two engines, which is the property the
differential oracle exists to protect. The existing bounds are untouched.

**The budget is the second argument, and that is not a style choice.** Region
inference reads a higher-order builtin's function out of the *last* argument,
with two tests asserting every higher-order builtin has it there. A callback in
the middle would be read as data and the budget read as the callback, **and
region-kind inference would record an indirect call silently.**

**Why the bound is an argument and not a constant.** A loop that cannot end is
worse than no loop, and ADR 0005 records what that costs: under tail-call
elision a runaway ran past a 45-second wall clock with no diagnostic where the
tree-walker answered in milliseconds. **There is no per-test timeout anywhere in
the runner** — re-verified for this record — **so a hang does not fail a test,
it hangs the suite.**

The registered instrument passes and passes conservatively: a runaway at the
default budget answers in hundredths of a second against a one-second threshold,
at many times the gate's load, and contention can only inflate a wall clock. **A
supplementary run at a hundred times the ceiling does not, and that is recorded
rather than dropped** — what it measures is not whether a runaway terminates but
how long a budget the *program chose* takes to spend, **which is the honest
shape of this bound.**

**The load-independent half of the claim is a test, not a stopwatch**: both
engines produce the diagnostic, and it does **not** contain the phrase "recursion
limit". Deliberately: nothing nested, **and things classify on that string.** A
budget below one is refused before the first round, because zero steps is a
bound nobody writes on purpose.

**And the argument is the whole link to the refusal below: a number in the
source is a number in the hash.** A program that raises its own bound
invalidates its own cached results. **A flag could not do that.**

### What it costs

**A new prelude type name**, so a project's own type of that name is now a
duplicate definition. The name was free in the tree; two better-sounding
alternatives were **not** available, because a shipped standard library module
and a test fixture already declare them.

Constructor names are a separate namespace and are **not** globally reserved —
and **the price of that is real and the silence about it misleads.** A module
declaring its own constructor of one of the two names keeps that constructor,
and pays for it by **losing `iterate` entirely**: the local name shadows the
prelude's, so the step's result will not unify. **And there is no way to name the
prelude's constructor past the shadow** — the qualified form parses as an effect
operation, and annotating the return type does not help, because the constructor
is resolved before the annotation is consulted. The only remedy is to rename the
module's own constructor. Nothing in the tree pays this today, and it is written
down because these are ordinary words in a way the prelude's other constructor
names are not, **and "constructor names remain shadowable" reads as *no cost*
when the cost is that the builtin this record exists for becomes unwritable in
that module.**

**The cheaper design — a step answering optionally, adding no name at all — was
rejected** because the absent case can only answer the seed the step was
*handed*, so a loop wanting to stop with a computed value must store it and go
round once more. **One wasted iteration per loop is the exact defect this record
is about.**

Both the runtime and front-end versions bump, and the change cannot be split to
avoid either: a cached pass written before `iterate` existed is a claim about a
program in which the name meant nothing, and a cached interface is one for a
program this front end now reads differently. **The prover's version is
deliberately *not* bumped, and that is a decision rather than an omission**: the
new prelude type does reach the prover's case-split table, but no *existing*
obligation can change, because splitting on it requires source that mentions it
and any program that previously declared its own type of that name no longer
checks at all — so its cached obligations are unreachable rather than wrong.

## The parser premise is wrong

The self-hosting record's load-bearing sentences were that a recursive-descent
parser *recurses once per element unless it is folded*, and that *the fold
escape hatch does not generalise, because a recursive-descent parser's recursion
**is** the grammar.*

**The reference implementation this project ships refutes both.** It reserves
recursion for grammar *nesting* and drives every sequence with a loop — sixteen
`while`s and five `loop`s, with **one iterative helper covering most sequences
in the language** from more than a dozen call sites: argument lists, list and
record literals, parameters, generic arguments, lambda parameters and pattern
arguments. **Precedence climbs iteratively too**, recursing only for the right
operand, so its depth is bounded by the number of binding powers rather than by
operand count.

**And the strongest fact, which neither record had: the reference parser bounds
grammar nesting itself**, at a constant far below the call ceiling and far above
the measured corpus maximum. **Grammar nesting in this design cannot reach the
ceiling — the parser refuses first.**

### What was wrong with the counts this was handed

The brief's `while` and `loop` counts hold. Its `for` count does **not**: nearly
all the matches are inside string literals and doc comments. Two line numbers
were mis-attributed, and re-reading this record's own first draft found two
more. Then adversarial review found a fifth — a citation one line out, pointing
at a function's signature rather than at the call inside it. **That a section
arguing for a mechanical check was itself one line out is the point it was
making. Three sources of line numbers for one file produced four wrong ones
between them, which is the argument for the mechanical check rather than for
reading harder.**

## The fold is depth 1 on both engines — measured, not assumed

A fold over half a million elements — fifty times the ceiling — **completes on
both engines**, in tens of megabytes of resident memory.

*An independent re-take did **not** reproduce the memory figure to the
pre-registered five percent, in either engine, and the pre-change binary
measures *higher* than the post-change one, so the change costs nothing here.
The figures are left as measured with the second observer's window beside them,
because there is no basis for saying which run was wrong. **What is withdrawn is
only the precision**: read it as a range and do not quote either figure to three
significant digits. Nothing turns on it — the claim is that a large fold
completes at depth 1 in tens of megabytes, and both readings say that.*

**Nested folds** bisect to exactly two calls per nesting level — the call and
the lambda the fold applies — against the ceiling, **and both engines change
their answer at the same integer**, which is the agreement the differential
oracle exists to check. **So the escape hatch the self-hosting record says "does
not generalise" already carries fifty times the ceiling flat, and nests to
thousands of levels against a corpus whose deepest observed nesting is
seventeen.**

## What raising the ceiling would cost

Peak memory per pending nested call, as **a slope over three depths rather than
a division at one window**, because a ranking taken at one window is not a cost.
Four cells — two engines by two body shapes — and **all four are linear** by the
rule written down before the data, with worst residual well inside the band.

**This record was handed figures for the same four cells and re-took them rather
than carrying them.** Three of four agree; one does not. **The measured ones are
what this carries, per the pre-registered rule that a handed figure is not a
target.**

**The shape is what matters, not the constants**: raising the ceiling by an
order of magnitude costs about a gigabyte of resident memory for one runaway to
reach the new diagnostic, on the worst cell. **And it is the wrong axis anyway.**
A fold carries half a million elements at depth 1 in tens of megabytes, and
`iterate` adds a loop that stops early at the same depth. **The ceiling does not
need raising for the workload that motivated raising it.**

## A bare flag to raise the ceiling is refused

**Results are cached by runtime version and hash, and shipping code writes only
passes.** Verified: of the five references to the pass outcome in the runner,
exactly two are writes and the other three only read or compare what the store
answered — and the runner says so about itself, on the arm handling a stored
failure: *never trust a stored failure; nothing here writes one, so it can only
have come from an older or foreign writer.*

**That asymmetry decides it.**

- **Raising the bound is monotone.** A program that passed under the old bound
  passes under a larger one; more budget cannot turn a pass into a failure. A
  cached pass stays true.
- **Lowering it is not.** A program cached as passing may raise under a smaller
  bound, **and the cache would answer pass without running it** — a green result
  over unexplored space.

**A flag that is safe in one direction and silently wrong in the other is not a
flag.** Making it safe means keying results on it, and the precedent for what
that costs is already in the tree: the prover hashes its plan into a second key
space with a domain separator and a rule for which outcomes may be written bare.
**That is the shape such a flag would need.**

`iterate`'s budget gets the property for free, because it is an argument: it is
in the definition's text, so it is in its hash, **so a program that changes its
own bound has already invalidated its own cached result.**

## Why this is not the tail-call decision re-opened

The load-bearing judgement, argued rather than assumed. ADR 0005 refused
tail-call **elision**: making a call cost nothing. **`iterate` elides no call.
It is the fuel budget without the elision** — the second half of ADR 0005's own
sentence, available now because it needs no engine deleted.

| | elision, as removed | `iterate` |
| --- | --- | --- |
| what a call costs | zero on one engine, one on the other | one on both, and the loop is not a call |
| what bounds a runaway | nothing | the budget, an argument |
| how the engines count | **differently — that was the defect** | identically: one frame, pushed and popped |
| how it is checked | it was not | asserted, on both engines |

**The difference is asserted rather than claimed.** A half-million-step loop runs
under a call cap of eight on both engines — fifty times the default ceiling
above, three orders of magnitude below — and passes. **Its arming leg is what
makes that non-vacuous**: the *same loop* written as the tail recursion
`iterate` replaces, at the *same* cap, raises on both engines. A frame-count
assertion covers what the call count does not imply. **Both were seen to fail**
under a driver that charges a nested call and a pending frame per round, and the
files were then restored byte-identically and re-run green.

**Tail-call elision stays out. ADR 0005's decision is not altered.**

## What this changes in the standard library

The HTTP streaming functions become `iterate` drivers, and the chunk bound
becomes a policy number.

**The exit criterion has two halves and both are required.** The same program on
the pre-change tree raises the recursion diagnostic; after, it passes on both
engines and under the differential oracle. **Without the first row the second is
a claim about a bound nothing reached, and the user-visible motivation would be
false.**

**The terminating-chunk guarantee survives, and it is why the budget is spent
inside the step.** A response cut short must still write its terminating chunk,
because a framed and unterminated chunked response on a reusable connection is
response smuggling — and an exhausted budget is a *diagnostic*, which would
abort the run with the message unterminated. So the loop carries its remaining
count in its seed, stops itself having written the terminator, and hands
`iterate` **one more round than the step can possibly take: a backstop that
cannot fire.**

**Two other functions in the same file have the same shape and the same comment,
and are deliberately not converted.** Their bounds sit far enough below the
ceiling that neither is shaped by it the way the chunk count was, **which is why
they were ranked below it and not why they are fine. Said in these words so that
silence does not imply the file is finished.**

## What would make this wrong

- **If the depth turns out not to be 1 on some engine or backend.** Asserted on
  both engines and both assertions seen to fail. **A third execution strategy
  would have to be checked against them rather than assumed into them.**
- **If a Ply parser is written and the ceiling bites anyway.** This refutes the
  *premise* about recursive descent. It does **not** port a parser, and nothing
  here says one is feasible — the throughput finding is untouched and remains
  the reason not to.
- **If the differential oracle finds a divergence on an effectful step.** A
  continuation captured inside a builtin's callback cannot be re-entered by the
  tree-walker, and `iterate`'s step is user code with an open row, **so that
  surface is newly reachable.** Covered by a test, and the corpus differential
  shard was run green — **but that shard's generator does not emit `iterate`, so
  what it establishes is that nothing already in the corpus regressed, not that
  `iterate` was swept.** Widening the generator is the cheap next step and was
  not taken: **a named gap rather than a silence.** The other newly-reachable
  surface — a scheduling point *inside* a loop's step — is covered by an
  exhaustive search over two tasks, seen to fail under a corrupted countdown.
- **If the two version bumps prove to have been unnecessary.** They discard
  every cached result and every cached type in every checkout. The intended
  cost, unsplittable, **and the largest thing this change spends.**
