# ADR 0030 — Compiled code on the front end

Status: accepted — **a measurement and a bound, not a change of direction**.
Amends `crates/ply-eval/src/backend.rs`'s module header and ADR 0026's audit
note. Supersedes nothing.

**This document was written in four passes and the last one changes what the
first measured.** §§1–5 price the seam as it was gated when the work started —
an argument test over `Value` discriminants. §6 widens that to declared parameter
types and then to the declared **return** type, at which point the whole front
end is one entry per file and the ceiling stops being resolvable by this ADR's
instrument. §10 prices what a *code generator* could reach instead of a
tree-walker. **ADR 0031 measures the closed fragment and ADR 0032 measures the
two halves together; read those for the current numbers.**

**The one sentence, as this ADR first measured it.** With the seam widened to
`Bytes`, the shipping backend enters the Ply front end several times per token —
not the zero the record projected — and that buys a small measured speedup
against a ceiling barely above it, which moves the front end from tens of times
the cost of the Rust front end to slightly fewer tens. **Compiled code, gated as
it was then, does not close this gap.** What would is one line of the census: the
argument test refused **100%** of what was refused, and a *type-level* argument
test reaches an order of magnitude more.

## Why this ADR exists

Every speedup this project had recorded for compiled code was taken on a compute
kernel. ADR 0018 §0 says so; ADR 0026 §1.2's entries were over `examples/` and
`tests/fixtures/`, **which is the workspace's own test corpus and not a program
anyone is trying to make fast.** Neither says what a compiled Ply *front end*
would cost, and the bootstrapping goal (ADR 0021) turns on nothing else.

ADR 0026 §3 projected that it would cost nothing, because nothing could enter:
*`fn read_line(buf: Bytes, ..) -> Line` cannot cross*. **That is true of
`read_line` and the generalisation drawn from it — that a real front end's
arguments are outside the fragment — is false, and was false before the `Bytes`
widening.** A lexer's hot arguments are not `Bytes`; they are *offsets and
bytes*, and a byte in Ply is an `Int`. At the pre-widening `Int | Bool` rung the
front end already admitted tens of thousands of calls. **Neither number is zero
and one of them predates this work.**

## The workload, and it is the one the gap was measured on

`spikes/ply-parser/` — six modules, a lexer and a recursive descent parser —
parsing **`examples/*.ply` as byte literals**, at the byte count
`spikes/ply-parser/GAPS.md` §13R took the gap on. Driven through the only
shipping command that can attach a backend:

```
ply test <dir> --no-cache -j 1 --filter probe.parse [--backend reference]
```

A generated `probe.ply` asserts over each file's bytes; `--filter` hides the
parser's own unit tests, **so the workload is the corpus and not a test suite.**
Both arms typecheck all seven modules, so that term cancels, and `--no-cache` is
on both sides as ADR 0026 §4.6 requires.

Pre-registered before any number in this document existed. **Two exploratory runs
taken before the pre-registration are disclosed in it and are not used.**

**`ply test examples/` is a different program and must not be substituted for
this one.** A later change measured a code generator over `examples/` and read
the result as a re-take of this ADR's figure; it is not — the two runs differ by
a factor of six in length, so a fixed compilation cost is most of one window and
a sixth of the other. **The direction transfers and the magnitude does not.**

## 1. Entries — what actually crossed

Deterministic; identical on every run that printed it. **The fragment enters the
lexer and not the parser**: `is_digit`, `skip_trivia`, `is_ident_start`, `scan`,
`is_keyword` and `at` account for nearly all of it, one parser definition clears
a thousand entries, and the next-largest contributor from the four parser modules
is nothing.

### Which gate refused the rest, and it is one gate

**Every refusal is `Gate::ArgumentShape`. Not one is an effect.** The two gates
this project had treated as the structural obstacles — `Gate::PublishedRow` and
`Gate::InternalEffects`, the ones that are "correct and not negotiable" because a
native body has no machine to perform into — **refuse zero calls on a real front
end, because a front end is pure.** `Gate::Budget`, `Gate::SimulateRegion` and
the frame ceiling refuse zero as well.

**The whole of what kept the Ply front end out of compiled code is a `matches!`
over three `Value` discriminants, and what it was refusing is `Record`
arguments: the parser's state record, passed to every parser function on every
call.**

### The single most valuable call in the corpus is refused on its *return* type

On a lexer-only probe the backend declines exactly one call per file, and they
are all the same definition. `lex(Bytes) -> Scan` clears every gate — its
argument crosses, it is pure, it is lowered code, it has a name — **and is
declined because `Scan` is a record, so the registry has no body for it.**

**One accepted call per file would have handed the entire lexer to the backend
and made every other entry disappear** — which is PR #30's result restated on
real code: **the win is the entry count *falling*, because the interpreter stops
driving the loop.** The return type, not the argument type, is what stands
between this workload and that.

**Taken. The prediction was right about the shape and conservative about the
size.** `Machine::compiled_answer` now decides an answer the way `compiled::admit`
decides an argument — the definition's declared **return** type is carried and
the answer is of the kind it denotes, or the answer is childless and
`compiled::crossable` carries it unchanged. On the whole front end the entry
moved one level further *out* than predicted, because `parse` is also
`Bytes -> <a record>` and swallows the lexer as part of its own subtree: **entries
in the hundreds of thousands became twenty-six — thirteen `items.parse`, one per
file, plus the thirteen nullary sources that hand it the bytes.**

**A reader who sees an entry count fall by four orders of magnitude and reads it
as a regression is reading the number the interpreter produces when it is
*driving*.** The coverage figure that rises beside it — the share of body calls
the registry can *answer*, taken with no backend attached so nothing hides inside
an entered subtree — goes from a sixth to **every call the seam admits.**

## 2. End to end

### 2.1 The first series had a position confound, and it is reported anyway

Registered as `A, B, A'` round robin. **A was always first in a block and A' —
*the same command* — always third, and A' came in below A in every block.** The
null control was most of the effect size. The series is kept and is not averaged
into what follows.

### 2.2 The counterbalanced re-take

Four arms, eight blocks, block *k* rotating the order left by *k* mod 4, **so
every arm sits in every position exactly twice.** Min user CPU. Load under this
project's 4.0 gate on both sides. Binary `current` before and after.

**The control that shows the measurement is sensitive: the two identical arms
land on the same minimum to the resolution of `/usr/bin/time -p`, and the arm
that differs by the entered set does not.**

Three independent counterbalanced series taken over forty minutes agree on the
ratio. **On the lexer alone — 99% of the entries — the saving is the same as the
whole workload's, which is the dose-response that says the effect is the entered
set and not the arm label:** the four parser modules contribute a rounding error
of the entries and no measurable time.

## 3. Against the gap

Same sitting, interleaved run by run, with `ply check` as the Rust front end over
the identical files — six phases against the Ply arm's two.

**The backend closes under a tenth of the absolute gap.** No figure from
`GAPS.md` is used as an arm here: §13R's denominator is user+sys at high load and
this is user at low load, **and the two are not comparable.** What is comparable
is that all three readings agree on the order: **a Ply front end costs one to
three dozen times a Rust one.**

## 4. The ceiling, which is the number that matters

An entry replaces the machine's evaluation of a whole subtree with the backend's.
So `userB = userA − t + r + overhead`, where `t` is what the machine spent there
and `r` is what the backend spent. **`t` is what an *infinitely fast* backend
would delete, and it is not derivable from the entry count: one equation, two
unknowns. So `r` is measured.**

### The first instrument was refused by a tripwire, and it was right to be

It timed the backend's `enter` with `Instant::now()` inside `ply-eval`. **That
crate may not read the host's clock at all**, and
`simulated_handlers::the_evaluator_reads_no_host_clock_and_no_host_entropy` bans
the type by name from every non-test line of it — *"a simulated run must be a
function of its definitions and its seed"*. It went **red** on the edit.

**The lesson is not about clocks.** Nothing about the measurement was
*observable* — the accumulator was two statics and no Ply program could read them
— **and the tripwire does not care, because it reads the source and not the
behaviour.** A measurement is exactly the kind of change that would have walked
past a behavioural test, and this one did not, because somebody had already
decided that "but it is only for measuring" is not admissible in this crate.

### The instrument that replaced it has no clock in `ply-eval` at all

Two binaries from the same tree, differing by **one line** — every entered body
is evaluated twice and the second answer returned — so **`r` = B2 − B**, measured
by `/usr/bin/time -p` on the whole process. **The precondition was checked before
anything was timed:** B2 reports the identical entry line, passes every test, and
passes `--engine both`, so the arms differ only in cost.

The withdrawn clock instrument agreed with it well inside the band the
pre-registration fixed before either number was compared. **Two instruments that
share no mechanism agree, and one of them is not allowed in the tree.**

**Three things make the bound conservative rather than optimistic, and all three
were stated before the numbers.** `r` is under-stated because B2's second pass
runs on warm caches, so `t` and the ceiling are **lower** bounds. `t` is
under-stated again because the backed arm pays `compiled::admit` on every body
call and the unbacked arm pays it on none, **so the saving attributed to entry is
charged the seam's own overhead.** And the machine still pushes and pops a
`Frame::Call` for an entered call, so `t` is body-evaluation time and the ceiling
is a ceiling on that, not on the call protocol.

**Withdrawn as a bound on the seam by ADR 0031.** The paragraph that read *"an
infinitely fast backend takes the run to N seconds, which is a fraction of the
absolute gap"* is true of the fragment it was measured on and **false of the one
that exists now.** That fragment admitted a twelfth of body calls; the closed one
admits every call it is offered, and the same arithmetic on the same command puts
**the whole of the measured absolute gap inside the fragment.** The reference
backend now delivers **none** of that ceiling — it is slower than the machine —
and what a *code generator* can reach is §10's figure.

### What the entered set actually contains, which is why `t` is that large

The census without a backend counts more body calls and far more builtin calls
than the census with one. **The difference is what ran inside the entered
subtrees and never reached the machine.** The entered set is not a pile of leaf
predicates — it is that many *subtrees*, holding a fifth of every call the run
makes.

## 5. The finding that withdraws a sentence: `Reference` is not slower

`crates/ply-eval/src/backend.rs`'s header said, and ADR 0026 repeated: *"It is
slower than the machine and says so; it exists to be policed, not to be fast."*

**On this workload it is faster, and by a lot** — several times fewer nanoseconds
per body call over the subtrees it takes. That is why arm B beats arm A at all.

**This is not the tree-walker being a better engine.** Run as an engine over the
same program it loses badly, and the control was taken because the result was
surprising: `--engine treewalk` is substantially slower over the whole lexer.
**The same evaluator is several times slower over the whole program and several
times faster over the fragment inside it.**

**Mechanism** — consistent with the numbers, not separated by them — is ADR 0020
§6.3's profile: the machine's step, dispatch and refcount protocol is most of
executed time and is a **fixed cost per call**, so on scalar and `Bytes` bodies
with no closures and no records it is nearly all of the cost, **while the
tree-walker's own weakness — deep values, cloned environments — is exactly what
the fragment excludes.** Which is the same reason a real code generator would win
here, arriving through a backend that is not one.

The sentence is corrected in place at both sites rather than deleted. **It
remains true of the case ADR 0026 measured** — a body the backend *declines* is
re-run to exhaustion once per offer, three orders of magnitude on a deep ladder —
and that is now stated as the case it is true of.

## 6. What is still refused, and the one lever that is large

The census prices every candidate rung as a share of body calls. `Str` buys
nothing on a front end. A **deep walk** over `Record` and `Ctor` arguments is
sound and buys under two points, and `crates/ply-eval/src/census.rs`'s header
prices it at hundreds of times slower on this exact workload — **it does not
finish**. A **shallow** `Record`/`Ctor` test is large and **unsound**.

**The type-level gate is the only rung that is both sound and large: an order of
magnitude wider entry set, at O(1) per call after a per-definition precompute.**

**Taken, and the counterfactual was accurate to about a point.** The type-level
gate ships as `compiled::CarriedTypes`, reached through
`compiled::Gate::ArgumentType`. Every coverage figure below is taken **with no
backend attached**, because an entered call hides its whole subtree from the
machine and shrinks *both* the numerator and the denominator — the two censuses
differ by exactly the body-call count inside the entered subtrees, arrived at
independently. **Two readings of the same gate are not two gates.**

**Every gate but two now refuses nothing on this workload, and the two are the
lambda wall.** §1's *"every refusal is `Gate::ArgumentShape`"* becomes **every
refusal is a lambda**: anonymous bodies, plus a small tail of calls whose
argument *is* a closure. **No argument test can move it** — `jit.rs`'s
`admissible_builtin` refuses all six higher-order builtins on its first branch,
and an anonymous body has no program-wide name for a registry to be keyed by.

**Then the return half was taken too, and the entry count collapsed.** With both
ends carried, the registry, the argument gate and the answer test are three
readers of one per-definition table. The reasoning that had deferred it survives
one half and not the other: *"a deep walk on the returned value is exactly as
unaffordable as the argument walk was"* **stands** and is why no walk was taken;
*"a type-level answer test moves a machine-side check into a backend
obligation"* **also stands, and is the price that was paid** — it is not a reason
the test was avoidable, it is the cost written on the ticket (§6.2).

**`examples/` gains almost nothing from either widening**, and for the same
reason: what is left there is `String`, `Float` and `Decimal` in the signature —
ADR 0019 §5 item 4's three, deliberately outside the fragment in both directions.
**The value of both widenings is concentrated on `Bytes`/`Int`-shaped code, which
is what a front end is.**

### 6.1 What entering a whole subtree changes, which is a different claim

Before this, an entered body was a leaf-ish thing over scalars. **Now one entry
can be a program, so every rule in `compiled.rs`'s header has to hold over a
*subtree* rather than over a call.** Asked one at a time, and **each answer seen
red before it was believed**:

- **Effects.** Already transitive: `DefInfo::internally_effectful` is a fixpoint
  over the call graph, held at four hops, through a mutually recursive pair and
  through a lambda. What changed is the *consequence* of its being wrong, from
  one call to a program.
- **The deterministic scheduler.** `Gate::SimulateRegion` reads the machine's
  state and says nothing about a definition that *opens* a `simulate` region two
  hops down. **The published row does**, because `sim.read` escapes and
  propagates to every caller that does not discharge it. Under a deleted row gate
  the offer list grows by exactly the definitions that should not be there.
- **The budget.** `budget` is handed over once and now bounds a whole recursion
  rather than a call. The test runs the real backend against a machine at a small
  `max_calls` and asserts the two arms produce **the same diagnostic**; weakening
  the fuel setting turns it red with an answer where a diagnostic belongs.
- **Cells and regions.** Unchanged in kind and larger in degree: an entered
  definition that opens its own `with_cell` skips an allocation, which is
  unobservable outside a `simulate` region — the same argument `Machine::constant`
  rests on — and `Gate::SimulateRegion` is what keeps it outside one.
- **Continuations.** Unchanged: capturing one needs a `perform`, and the effects
  gate refuses anything that can reach one.

### 6.2 The one thing this gives up, and it is real

While the answer test read `compiled::crossable`, a `Value::Cell` **could not
come back at all** — the invariant was structural, decided from a discriminant
over kinds that hold no `Value`. It now reads the declared return type and the
answer's *top-level* kind, **and a declared type is a fact about what the
*program* can build, not about what a backend actually put in the record.** A
backend answering a record with a forged cell inside it is believed.

**The argument direction does not have this hole and the asymmetry is the whole
of it:** an *argument* is a value the machine's own evaluation built under a
checker that accepted the program; an *answer* is built by the backend.

**So a ninth wrong backend was added rather than a sentence:**
`--backend wrong:handle`, which replaces the first field of a `Record`, the first
argument of a `Ctor` or the head of a `List` with a forged cell and leaves the
kind alone. Over `examples/` and `tests/fixtures/` it changes hundreds of answers
and about a fifth of tests report it — **and most do not.** Through the shipping
command on the backend corpus it fires twice and **one** of the two is caught;
the other is `assert_eq(len(pair(7)), 2)`, and **a list with a forged cell in its
head is still two long, which is the measure of what a corpus has to *look at*.**

That is the same class as a wrong `Int`: caught by `--engine both` and the
differential corpus, and by nothing at the seam. **It is written into
`compiled.rs`'s header and into `Compiled::enter`'s own doc — the paragraph a
backend author reads — as a limit, not argued away.**

**A registry gap closed that something was standing in.** `Mutation::Unoffered`
needs a definition that is *offered* and has no body, and the backend corpus had
exactly one — whose container return is now inside the fragment. **Two tests
failed rather than passing quietly**, which is the entire value of that file's
"seen to fire" step. The replacement was chosen because its return type is a
leaf-set exclusion rather than a container one, **so the next container widening
will not close the gap again.**

### 6.3 Time — the run gets SLOWER, and that is the ceiling's whole point

Four binaries from this tree (before/after × honest/doubled), seven arms, six
rotated blocks, min user CPU, `/usr/bin/time -p`, `uptime` on both sides.
Preconditions checked before any timing: each doubled binary reports the
identical entry count to its honest twin, all four pass, and the "before" binary
reproduces the earlier entry line to the call. **Taken inside the 4.0 load gate**
— the first series on this seam in two days that did not have to be reported as
an observation.

**`ply test <W1> --backend reference` is substantially SLOWER after the return
widening**, and against the unbacked machine it goes from faster to slower.
**That is not a defect and it is not a surprise: it is what a total collapse does
when the only backend in the tree is a tree-walker.** §5 measured this backend as
several times *faster* than the machine per body call **inside its fragment** and
several times *slower* run as a whole engine over this same program. **While the
fragment was leaves over scalars the first number governed; now that one entry is
the program, the second does.**

The null control is a fraction of a percent against a double-digit effect, and
the widening costs **exactly nothing** on the arm that enters nothing — which is
what a change that only moves what a backend is *allowed* to answer should cost.

**What the ceiling says, and why the multiplier is not the number to quote.** `f`
— the share of the unbacked run an infinitely fast backend would delete — is now
within a couple of points of 1, **where `1/(1−f)` is not a resolvable quantity**:
a one-point move takes the ceiling from twenty-five to a hundred. Resolved by a
second instrument that shares no mechanism with this one — **ADR 0031 §3's floor
arm, the same command with a `--filter` that selects no test, which measures the
residue instead of inferring it**, and whose `t` agrees with this one's to a
fraction of a percent. **The directly measured statement is better and needs no
model: `B − r` is a small residue, so nearly all of the backed run is inside
`enter`.**

**One caveat, because the model bends here.** `r = B2 − B` prices a second pass
through the entered bodies **at the backend's speed**, and `t = (A − B) + r` then
mixes that with the machine's timeline. §4's model assumes a backend at least as
fast as the machine; **this one is slower**, so `t` over-estimates the
interpreter-equivalent work and `f` is an estimate whose direction is certain and
whose third digit is not. An out-of-gate series puts the same quantity above 1,
**which is the same statement with the bend visible.**

**And `f` is larger than the admitted call share, which is not a
contradiction.** The call share is what the seam **admits**; the time share is
what an entered subtree **contains**. Once the root is entered there is no seam
inside it, **so the anonymous lambdas the census counts as refusals run inside
the entry too.** The census taken *with* a backend says it in one line: the
machine makes twenty-six body calls in the whole run. **The lambda wall is
stepped over rather than solved** — §6.4.

### 6.4 What this does to the lambda wall, and to what polices the seam

**The wall moves from the seam to the code generator.** The earlier amendment
measured the residue as almost entirely `Gate::Anonymous` and concluded that *"no
argument test can move it"*, which is true and **is not the same as "nothing
can"**. Entering at a **leaf** requires every lambda to be entered separately and
an anonymous body has no name for a registry to be keyed by; entering at the
**root** requires nothing of the sort, because the lambdas are inside the body a
backend compiled. With the backend attached, `Gate::Anonymous` refuses **zero**
calls — **not because it was widened, but because the machine never reaches
one.**

What that costs is honest and specific: `jit.rs`'s `admissible_builtin` refuses
all six higher-order builtins on its first branch, **so a *cranelift* backend
cannot compile the root at all and would decline the entry this change makes
available.** The obstacle is now a fact about a code generator's coverage rather
than about the seam's rules. **That is a different piece of work and a smaller
claim than "the wall is gone"** — §10 prices how much smaller.

**Three of the eight wrong backends stop firing on this workload, and this is the
finding that most needs writing down.** After the change the only entered calls
answer a `Bytes` or a `Record`; `off-by-one` needs an `Int` answer, `inverted`
needs a `Bool`, and `unoffered` needs a registry miss — **this workload has none
of the three left.** Nothing about the seam got weaker: all three still fire and
are still caught over `examples/` and `tests/fixtures/`. **But anyone who reads
this workload's green as evidence about those three is reading a vacuous pass,
and that is exactly the defect class `CONTRIBUTING.md` §"The one rule" names.**

One more thing the same table shows, **and it is the answer test doing visible
work on real source**: under `wrong:stale`, most offers are *not* entered,
because a stale answer of the wrong kind for the definition asked is refused by
`Denotes::matches` — **so the corruption becomes a decline rather than a wrong
answer.**

## 7. Correctness — the entries were seen to matter before they were reported

**An entry count is worth nothing if the run stays green when the backend lies.**
Run with `--backend wrong:<mutation>`, six of seven mutations turn the whole
corpus red. The seventh, `exceeds-budget`, **did not fire** — the target is
offered and no answer changes, because nothing in this corpus outruns the
machine's bound — **and is reported as not-fired, not as not-caught**, which is
the distinction ADR 0026 §4.5 insists on.

`--engine both --backend reference` audits the whole corpus with zero failures:
machine, tree-walker and backend agree.

### And the instrument was armed against itself

One counter is new — `Counts::entered_names` — and **a counter that cannot be
wrong is a counter that is not reading anything.**

- **Seen to fail.** It records a name only when an answer came back. Deleting
  that condition and re-running puts `lexer.lex` in the *entered* list — the
  definition that is offered once per file and **declined** every time, which is
  §1's whole finding. **The corruption named: the histogram would report the
  offered set under the entered set's name.**
- **It closes against two counters it does not share code with**, and its names
  sum to the CLI's entry count.
- **The withdrawn timer had its own negative control**: with no backend attached
  it reported zeros, and its reading varied run to run, **so it was reading a
  clock and not a constant.**
- **The B2 instrument's control is its precondition** (§4): identical entry
  counts, all green, `--engine both` green. **A doubling that changed an answer
  or an entry count would not be a cost-only arm.**

## 8. What would make this wrong

- **`t` is attributed to the entered set and something else caused the saving.**
  The dose-response is the answer: the lexer-only probe holds almost all of the
  entries and shows almost all of the saving. **If a future change makes those
  diverge, `t` is measuring something else.**
- **`r` is measured by doubling and the second pass is not the same work.** It
  runs on warm caches, so `r` is under-stated; **the direction was fixed in the
  amendment before the number** and it makes the ceiling a lower bound. On the
  lexer-only workload the same instrument reads `r` slightly *higher* with fewer
  entries — the wrong way round by one tick of `/usr/bin/time -p`'s resolution,
  **which is the resolution this whole quantity is measured at.**
- **The probe is `ply test` and not `ply run`.** A handful of entries are the
  probe handing over a source literal, which a `ply run` shape would not have.
  Removing them changes no figure to three digits.
- **`-j 1`.** Each worker builds its own backend; at higher parallelism the
  counts are per run and the times are not comparable to these.
- **The ceiling assumes an entry saves the whole subtree.** It does under a
  backend that runs the body to completion. **A backend that could only compile
  part of a body and called back into the machine would not reach it**, and
  `compiled.rs`'s header says why the seam hands over no route back.
- **The corpus is thirteen files of Ply that this project wrote.** A front end
  run on a corpus with different token statistics would enter a different number
  of times. **It was chosen because it is the one `GAPS.md` took the gap on, not
  because it is representative of anything else.**
- **The brief this work was set from names a much larger corpus than the one that
  exists.** The discrepancy is recorded and not resolved; **if the intended
  corpus was a different one, every absolute second here is on the wrong
  denominator and the ratios are not.**

## 9. Decision

1. **No backend ships on the strength of this.** The ceiling is above the bar the
   pre-registration fixed for "does not help", and it is nowhere near a reason to
   take cranelift into `Cargo.lock`. ADR 0026 §4.5's precondition and ADR 0016
   §3.5 are untouched. *(A backend has since shipped, on ADR 0026 §4.7's
   authorisation and for its reasons, not this ADR's.)*
2. **The next widening is the type-level argument gate**, and this ADR is the
   evidence for choosing it over the deep walk and over `Str`. Whoever takes it
   owes a re-measurement of gate ordering — a type-level test needs the name
   first, **so it sits below the row and effects gates rather than above the
   shape gate** — and owes **the return type, measured at the same time**,
   because `lexer.lex` is the call that matters and it is refused on its return.
   **Both were taken; see §6.**
3. **`Reference` is no longer described as slower than the machine.** Corrected
   in place at both sites, with the case it is still true of named (§5).
4. **`Counts::entered_names` stays; the clock does not.** Which definitions a
   backend entered was not answerable in this tree before — `admitted_names`
   answers a different question and `lexer.lex` is the difference — and it is
   environment-gated counting with no clock in it. **How long it spent there is
   measured from outside the process and must go on being**: `ply-eval` may not
   carry a clock, **and the next person to want this number will reach for
   `Instant` first, as I did.**

## 10. The lambda wall, priced

§6.4 concluded that the wall *"moves from the seam to the code generator"*. That
is confirmed here and it is not the end of the sentence: **the move is what makes
the wall expensive, because after it every second of the win lives on the far
side.**

### 10.1 The seam's refusal and the code generator's are not the same refusal

**At the seam, `Gate::Anonymous` is a *naming* gate, not a callback gate.**
`compiled::admit` needs a program-wide name because every gate below it is a
lookup keyed by one, and `Compiled::enter` is keyed by one too. A lambda
publishes none of those facts and offers no key. **The gate's own doc records
what happens without it: replacing it with a fabricated empty `Symbol` left every
unit test in the crate green, because `Gate::PublishedRow` refuses an unknown
name one line later.** Admitting a lambda would need a stable per-lambda identity
**and** per-lambda published facts, and neither exists.

**In the code generator the refusal is three refusals**, and `fold` needs all
three lifted, not one: `admissible_builtin`'s first branch refuses any builtin
that calls user code; `NodeKind::Lambda` is refused because **there is no closure
representation at all**; and `Denotes::Local` refuses a call through a local
binding. And the refusal **propagates** — `Denotes::Uncompiled` refuses the
*enclosing* function rather than emitting a trampoline, so the compiled set is
closed under calls. **One lambda anywhere under a root refuses the root.**

### 10.2 What the seam would lose if a backend were given a callback instead

The alternative to compiling the callback is handing the backend a route back
into the machine. **`Machine::compiled_answer`'s own doc prices it and the price
is paid in invariants, not in lines:**

- `compiled_answer` takes **`&self`**. *"Nothing is committed until there is a
  value, so a decline restores nothing because nothing was disturbed."* A
  callback needs `&mut Machine`.
- The `Frame::Call` at the call site is pushed **after** `enter` returns, sound
  *"only because `enter` is handed no route back into this machine"*. Give a
  backend a callback and that push moves above `enter` — **and then a decline
  must pop it, so the two paths stop being one line apart.**
- **The bailout stops being free**, which is the concrete cost. Today `None`
  after a whole body costs nothing. With a callback, the call count, the arena's
  allocation counter and the memo have already moved when the backend declines,
  **so re-evaluating from the top repeats them — and re-evaluating from the top
  is how this seam keeps the diagnostic "the interpreter's by construction".**
- `Machine::compiled_witness` — debug-only, asserting the machine's counters are
  unchanged across `enter` — **exists so that "adding a callback, or handing a
  backend an arena, goes red here instead of breaking in silence". A callback
  moves two of those counters at minimum, so the change would begin by deleting
  the tripwire built to catch it.**
- `budget` is computed once, before `enter`. A callback consumes nested calls the
  machine cannot see, **so the recursion diagnostic both engines share needs live
  accounting rather than one `usize`.**
- `Gate::NotLoweredCode` keeps `Interp`'s closures out of compiled code because
  *"routing its closures into compiled code would audit the backend against
  itself"*. **A callback is a route back into whichever engine is running, and
  `--engine both` is the only thing that catches a wrong `Int`.**

**One invariant does *not* break, and it is the one that looks most fragile.**
The effects gate survives a callback: `internally_effectful` is transitive over
the named call graph **and its scan walks lambda bodies**, so a lambda that
performs marks the definition it is written inside. **What a callback breaks is
the machine-state invariant, not the purity one.**

**So the cheap direction is the other one: compile the callback.** It changes
nothing in `ply-eval` at all — a `fold` inside an entered body is invisible to a
seam that gates entries.

### 10.3 Is it worth it — the seam says no, the clock says it is the only thing left

**At the seam the lambdas now cost nothing, and that is measured rather than
inferred.** With no backend the census counts hundreds of thousands of
`Anonymous` refusals; with the backend attached it counts **zero, out of
twenty-six body calls.** The no-backend arm is the positive control for the
backed arm's zero — same binary, same counter. **The lambdas did not get
admitted; the machine stopped reaching them.**

**On `examples/` the picture is the opposite, and it is worth carrying, because
it is what stops "the residue is exactly the lambdas" from being a property of
Ply**: there `Anonymous` is a twentieth of refusals against `ArgumentType`'s
two-thirds, most of that `String`. **The lambda wall is a front-end phenomenon. A
corpus of `String` and `Decimal` never reaches it.**

**Then the clock, and this is the finding.** `Reference` is a tree-walker, so it
runs lambdas by interpreting them; **a code generator cannot.** To price that,
the registry was narrowed to the set a *callback-free* code generator could
compile — through `PLY_BACKEND_ONLY`, **a registry narrowing that can only add
declines, so it cannot change an answer** — and the same A/B/B2 series re-run.

The callback-free set is computed **outside** the spike, from `ply hash --deps
--json` for the call graph and a source scan for the call sites, because the
spike is its own workspace and nothing in `crates/*` may depend on it. **The
scan is validated against `spine.ply`'s own comment about how many `iterate`
sites it has, which is exactly what the scan finds.** The root the whole win
depends on is **not** in the set: its closure holds dozens of definitions that
call a callback builtin, **all of them passing a lambda written at the call
site.**

### **The lambda wall costs the difference between an infinitely fast backend deleting nearly all of the run and deleting about half of it. As a ceiling: a little over 2×, against a number this instrument can no longer resolve.**

**The model-free form, because `1/(1−f)` is not resolvable that close to 1:**
under the callback-capable backend the residue outside `enter` is hundredths of a
second of the run; under the callback-free one, more than a quarter of it is
still outside.

Three supporting facts, each measured rather than argued:

- **Coverage.** The callback-free fragment runs about three-fifths of body calls
  inside the backend, against everything for the unrestricted one. `--engine
  both` under the narrowing audits clean, **so the narrowing changed no answer.**
- **A callback-free fragment cannot hide a single lambda, ever.** Under the
  narrowing the refusal histogram is **identical to the no-backend arm, gate for
  gate.** That is not a coincidence to be re-measured: **a definition with no
  callback user anywhere in its closure has no lambda call beneath it, by
  construction.** So for a code generator that cannot compile a callback, the
  lambda refusals are **irreducible** — no widening of the fragment can swallow
  one.
- **And the callback-free fragment gives this workload back the power to police
  the seam that §6.4 recorded it losing.** Under the narrowing the fragment is
  scalar leaves again, and **every one of the three corruptions that stopped
  firing fires and is caught.** (The offer counts are not the honest run's: a
  wrong answer changes which branches the parser takes, so one mutation fails
  fast and another loops. **What matters is that each fired before it was
  caught**, which is the middle step usually missing.) **A narrowing that could
  not be caught lying would be worth nothing.**

### 10.4 The narrow version — buildable, sound, and measured to be worth zero

The obvious escape from §10.1's three refusals is to take only the easy half of
the first: compile a `fold` whose callback is a **named** already-admissible
definition rather than an anonymous lambda. **It is buildable and it is sound. On
this front end it moves nothing, in either of the two places it could be spent,
and both were measured before either was built. Neither was built.**

**At the seam.** The only `ArgumentShape` refusals left are calls to
`spine.comma_list` — the one definition in the root's closure with a
function-typed parameter — and every one of its call sites does pass a named
definition. It is worth **zero** with the shipping backend, because those calls
are inside the entered root; and **zero** with a callback-free one, because
`comma_list`'s *own body* drives its loop with `iterate` and a lambda, **so no
such code generator has a body for it. The one place on this front end where a
callback is a named definition is a function whose own body needs the anonymous
case.**

**In the code generator.** Of the higher-order call sites inside definitions, the
overwhelming majority pass a lambda written at the call site; the handful that
pass a name are either an indirect call through a *parameter*, which `jit.rs`
refuses separately, or **on the debug-dump path, and none of those is in the
root's closure.** Recomputing the fixpoint with exactly that widening applied
moves the callback-free set by **zero definitions** — each candidate is still
blocked by another callee that passes a lambda. **A widening that changes no
member of the fragment cannot change a number, so no series was run for it.**

### 10.5 What would make this wrong

- **The callback-free set models one constraint and `jit.rs` has several.** It is
  the callback fixpoint only; `jit.rs` also refuses `Float` and `Decimal`
  literals, `++`, match guards, a call whose callee is an expression, and a
  lambda in any position, **and `entry::enterable` narrows the registry again to
  scalar signatures. So the ceiling here is an upper bound on a callback-free
  cranelift backend's reach, and the real figure is below it.** The direction is
  stated before the number, as §4's caveats were.
- **`r = B2 − B` under-states**, because B2's second pass runs on warm caches, so
  both ceilings are lower bounds — §4's caveat, applying to both arms equally.
- **Both backed arms pay `compiled::admit` on every body call and the unbacked
  arm pays it on none**, so `t` is charged the seam's own overhead in both.
- **`Reference` is a tree-walker and both backed arms are slower than the
  unbacked one.** **That does not touch the ceiling:** `t` is what the *machine*
  would have spent in the entered subtrees and is independent of how fast the
  backend is, **which is the whole reason §4 measures `r` rather than assuming
  it.**
- **The source scan is a scan.** It strips comments and string literals and looks
  for a call to one of the six by name; **it does not resolve shadowing**, so a
  local binding named `fold` would be counted as the builtin. That is
  conservative in the direction that *shrinks* the set, and the front end shadows
  none of the six. It was validated against three independent facts: the
  `iterate` site count `spine.ply`'s own comment claims, the runtime builtin
  histogram, and the seam's own `refused_names`.
- **The instrument was checked for vacuity.** A narrowing that silently did
  nothing would have reported the unrestricted run under the restricted label;
  the same command with the variable unset reads the unrestricted entry line,
  **and both readings are in the log.**

### 10.6 What this means for the decision

**§9 item 1 is unchanged and the reason for it has changed.** The sentence that
is no longer the one to quote is the one naming a ceiling — §6.3 already withdrew
the number. **What replaces it is sharper: a cranelift backend as `jit.rs` stands
could not enter the root at all, and the fragment it *could* enter has a measured
ceiling of a little over 2×.** Anything above that is on the far side of §10.1's
three refusals — and `admissible_builtin`, the one usually named, **is the
smallest of the three: what compiling a `fold` actually needs first is a closure
representation and an indirect call, neither of which `jit.rs` has.**

**What shipped in code for this, and it is not a widening.** One measurement
knob, `PLY_BACKEND_ONLY`, in `crates/ply-eval/src/backend.rs`: `Fragment::build`
intersects its registry with a named set. **It can only add declines, so it
cannot change an answer**, and `--engine both` audits clean under it. Two tests
hold it and **both were seen red first** — deleting the intersection reads one
name too many, and dropping the empty-name filter makes a trailing comma a
different experiment.

## Provenance

Every series is counterbalanced — each arm in each position — with a null control
of two identical commands under different labels, min user CPU over the blocks,
`/usr/bin/time -p` outside the process, and `uptime` recorded on both sides. The
binary was checked `current` by `.github/binary-is-current.sh` before and after
every series, **and no series spans a rebuild.** Pre-registrations, each
amendment written before its number, and the raw logs are outside the repository;
the load average is recorded with every series, **and a series taken above this
project's 4.0 gate is reported as an observation rather than as a figure.**
