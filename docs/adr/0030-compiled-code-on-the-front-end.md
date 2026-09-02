# ADR 0030 — Compiled code on the front end

**Accepted — a measurement and a re-aiming, not a change of direction.** This
merges three passes at one experiment (0030–0032), taken days apart, each of
which withdrew a number from the one before.

**The arc in one paragraph.** The seam was gated by an argument test over
runtime value kinds, which refused **100%** of what was refused on a real Ply
front end. Widening it to declared parameter types and then to the declared
**return** type collapsed the entry count from hundreds of thousands to **one
per file**, took the reachable share of the run from a fraction to nearly all of
it, and moved **the whole of the measured Ply-versus-Rust gap inside the
fragment.** But every backend that can inhabit the widened seam makes the run
*slower*: the tree-walking one because it is a tree-walker, the code generator
because it cannot compile the root and so enters at the leaves instead.
**Nothing shipped on the strength of any of it.**

## Why this exists

Every speedup recorded for compiled code was taken on a compute kernel, or over
**the workspace's own test corpus, which is not a program anyone is trying to
make fast.** Neither says what a compiled Ply *front end* would cost, **and the
bootstrapping goal turns on nothing else.**

ADR 0026 projected it would cost nothing, because nothing could enter: the
ladder's own function takes bytes and cannot cross. **That is true of that
function and the generalisation drawn from it — that a real front end's
arguments are outside the fragment — is false, and was false before the bytes
widening.** A lexer's hot arguments are not byte buffers; **they are *offsets
and bytes*, and a byte in Ply is an integer.**

**The workload is a corpus, not a test suite**: the parser spike's modules
parsing the example files as byte literals, at the byte count the gap was taken
on, driven through the only shipping command that can attach a backend, with the
spike's own unit tests filtered out. **The workspace corpus is a different
program and must not be substituted for it** — a later change measured a code
generator over it and read the result as a re-take of this figure; **the two
runs differ by a factor of six in length, so a fixed compilation cost is most of
one window and a sixth of the other. The direction transfers and the magnitude
does not.**

## What kept the front end out, and it was one line

**Every refusal was one gate: argument shape. Not one was an effect.** The two
gates this project had treated as the structural obstacles — the published row
and internal effects, the ones that are *correct and not negotiable* because a
native body has no machine to perform into — **refuse zero calls on a real front
end, because a front end is pure.** The budget, the region gate and the frame
ceiling refuse zero as well.

**So the whole of what kept the Ply front end out of compiled code was a match
over three runtime value kinds, and what it was refusing is *record* arguments:
the parser's state record, passed to every parser function on every call.**

**And the single most valuable call was refused on its *return* type.** The
whole lex clears every gate and **is declined because its result is a record.
One accepted call per file would have handed the entire lexer to the backend and
made every other entry disappear** — which is an earlier finding restated on real
code: **the win is the entry count *falling*, because the interpreter stops
driving the loop.**

Both widenings were taken. The entry moved one level further *out* than
predicted, because the parse entry point is also bytes-to-a-record and swallows
the lexer as part of its own subtree. **A reader who sees an entry count fall by
four orders of magnitude and reads it as a regression is reading the number the
interpreter produces when it is *driving*.**

**Every coverage share must be taken with no backend attached**, because an
entered call hides its whole subtree and shrinks *both* the numerator and the
denominator. **Two readings of the same gate are not two gates.**

**The workspace corpus gains almost nothing from either widening**: what is left
there is strings, floats and decimals in the signature, deliberately outside the
fragment in both directions — and its largest refusals by name are **a SQL lexer
and parser in the standard library, the same shape of program as the front end,
refused because its tokens hold a string where the front end's hold bytes.
The value of both widenings is concentrated on byte- and integer-shaped code,
which is what a front end is.**

## What entering a whole subtree changes, which is a different claim

Before this, an entered body was a leaf-ish thing over scalars. **Now one entry
can be a program, so every rule in the seam's header has to hold over a
*subtree* rather than over a call.** Asked one at a time, **and each answer seen
red before it was believed.**

- **Effects.** Already transitive, held at four hops and through a lambda. What
  changed is the *consequence* of its being wrong, from one call to a program.
- **The deterministic scheduler.** The region gate reads the machine's *state*
  and says nothing about a definition that *opens* a region two hops down. **The
  published row does**, because the seed atom escapes and propagates to every
  caller that does not discharge it. Under a deleted row gate the offer list
  grows by exactly the definitions that should not be there.
- **The budget** now bounds a whole recursion rather than a call. The test runs
  the real backend against a machine at a small bound and asserts **the same
  diagnostic**; weakening the fuel setting turns it red with an answer where a
  diagnostic belongs.

**And the one thing it gives up is real.** While the answer test read runtime
kinds, a cell **could not come back at all** — the invariant was structural. It
now reads the declared return type and the answer's *top-level* kind, **and a
declared type is a fact about what the *program* can build, not about what a
backend actually put in the record. A backend answering a record with a forged
cell inside it is believed.**

**The argument direction does not have this hole and the asymmetry is the whole
of it: an *argument* is a value the machine's own evaluation built under a
checker that accepted the program; an *answer* is built by the backend.** So a
ninth wrong backend was added rather than a sentence. Over the workspace corpus
it changes hundreds of answers and about a fifth of tests report it — **and most
do not.** Through the shipping command it fires twice and **one** of the two is
caught; the other asserts a list's length, **and a list with a forged cell in
its head is still the right length, which is the measure of what a corpus has to
*look at*.** Written into the backend author's own doc as a limit, not argued
away.

**A registry gap closed that something was standing in.** One mutation needs a
definition that is *offered* and has no body, and the corpus had exactly one —
whose container return is now inside the fragment. **Two tests failed rather
than passing quietly**, which is the entire value of that file's seen-to-fire
step. The replacement was chosen because its return type is a leaf-set exclusion
rather than a container one, **so the next container widening will not close the
gap again.**

## The instrument findings, which are what survives

### A clock in the evaluator was refused by a tripwire, and it was right to be

The first attempt at measuring what a backend spends inside an entry timed it
with a clock inside the evaluator crate. **That crate may not read the host's
clock at all**, and a test bans the type by name from every non-test line of it
— *a simulated run must be a function of its definitions and its seed.* It went
**red** on the edit.

**The lesson is not about clocks.** Nothing about the measurement was
*observable* — the accumulator was two statics no Ply program could read — **and
the tripwire does not care, because it reads the source and not the behaviour. A
measurement is exactly the kind of change that would have walked past a
behavioural test, and this one did not, because somebody had already decided
that "but it is only for measuring" is not admissible in this crate.**

**What replaced it has no clock in the crate at all**: two binaries from one
tree differing by **one line** — every entered body evaluated twice and the
second answer returned — timed from outside the process. **The precondition was
checked before anything was timed**: identical entry line, all tests pass,
differential green, **so the arms differ only in cost.** The withdrawn clock
instrument agreed with it well inside the pre-registered band. **Two instruments
that share no mechanism agree, and one of them is not allowed in the tree.**

Three things make the bound conservative, **and all three were stated before the
numbers**: the doubled pass runs warm; the backed arm pays the seam's admission
check on every body call and the unbacked arm pays it on none, **so the saving
attributed to entry is charged the seam's own overhead**; and the machine still
pushes a call frame for an entered call.

### The floor arm, which is what made the ceiling resolvable

At a reachable share within a couple of points of one, **the reciprocal is not a
resolvable quantity**: a one-point move takes the ceiling from twenty-five to a
hundred. What resolves it shares no mechanism with the doubling: **the same
command with a filter that selects no test.** It still does the whole front end
and runs no test body, **so it is the run's fixed cost, measured rather than
inferred.** The two agree on the linear quantity to a third of a percent, and a
third instrument that was already shipping — the runner's own per-test
milliseconds — lands on the same number to a twelfth of a percent **and is
additive per file, so the floor is not a coincidence of the total.**

Three statements in increasing order of how much model they carry, **and the
first is the one to quote**: nearly all of the backed run is inside the entry;
the fixed cost of the command is under two percent of the run; and only then the
multiplier. **The multiplier is real and its third digit is not** — the two
instruments agree on the linear quantity to a third of a percent and **their
ceilings are 17% apart, which is the hypersensitivity seen from the other
side.**

**What that does *not* say, and the trap is the one ADR 0024 names.** The
reachable share is not the share a real code generator deletes. The profile
putting dispatch and refcount traffic at most of executed time bounds how much
of *that* work is interpretive overhead, **and the two compose rather than
either being the answer: a native backend still has to do the parse, and only
the machinery around it goes away.** *A window share is not a request cost.*

### Counters and controls, armed against themselves

**A counter that cannot be wrong is a counter that is not reading anything.**
The entered-names counter was **seen to fail**: it records a name only when an
answer came back, and deleting that condition puts the *declined* definition —
the one this record's whole finding is about — into the *entered* list. **The
corruption named: the histogram would report the offered set under the entered
set's name.**

**A position confound was reported rather than dropped.** The first series was
registered as a round robin, one arm always first and **the same command under a
different label** always third — **and the third came in below the first in
every block. The null control was most of the effect size.** The series is kept
and is not averaged in. Every series since rotates so **every arm sits in every
position**, with two identical arms as the control; and **the dose-response says
the effect is the entered set and not the arm label**, because the lexer-only
probe holds almost all of the entries and shows almost all of the saving.

**And the reference front end's cold arm was never cold**, which is why an
earlier attempt read the warm figure: its script deletes a cache directory in
the working directory, **but the front-end cache is written beside the target.
Two records have now spent an arm on it.**

## A sentence withdrawn: the tree-walking backend is not slower

The seam's own header said, and ADR 0026 repeated: *it is slower than the
machine and says so; it exists to be policed, not to be fast.* **On this
workload it is faster, and by a lot**, per body call over the subtrees it takes.

**This is not the tree-walker being a better engine.** Run as an engine over the
same program it loses badly, **and the control was taken because the result was
surprising.** So: **the same evaluator is several times slower over the whole
program and several times faster over the fragment inside it.** The mechanism —
consistent with the numbers, not separated by them — is that the machine's step,
dispatch and refcount protocol is a **fixed cost per call**, so on scalar and
byte bodies with no closures and no records it is nearly all of the cost,
**while the tree-walker's own weakness, deep values and cloned environments, is
exactly what the fragment excludes. Which is the same reason a real code
generator would win here, arriving through a backend that is not one.**

Corrected in place rather than deleted. **It remains true of the case ADR 0026
measured** — a body the backend *declines* is re-run to exhaustion once per
offer.

## The widenings that were refused

Strings buy nothing on a front end. A **deep walk** over record and constructor
arguments is sound, buys under two points, **and does not finish.** A
**shallow** record test is large and **unsound**. **The type-level gate is the
only rung that is both sound and large.**

The reasoning that had deferred the *answer* half survives one clause and not
the other: *a deep walk on the returned value is exactly as unaffordable as the
argument walk was* **stands**; *a type-level answer test moves a machine-side
check into a backend obligation* **also stands, and is the price that was paid**
— not a reason the test was avoidable, **it is the cost written on the ticket.**

## The lambda wall

**After the widenings, every refusal is a lambda**: anonymous bodies, and a
small tail of calls whose argument is a closure. **At the seam this is a
*naming* gate, not a callback gate** — admission needs a program-wide name
because every gate below it is a lookup keyed by one, and a lambda publishes
none. **The gate's own doc records what happens without it: replacing the name
with a fabricated empty symbol left every unit test green, because the
published-row gate refuses an unknown name one line later.**

**With a backend attached the gate refuses zero — not because it was widened,
but because the machine never reaches one.** Entering at a **leaf** requires
every lambda to be entered separately; **entering at the root requires nothing
of the sort, because the lambdas are inside the body a backend compiled.** *No
argument test can move it* is true and **is not the same as "nothing can". The
wall moves from the seam to the code generator**, which refuses on three
separate lines what the entry point is made of: any builtin that calls user
code; a lambda, because **there is no closure representation at all**; and a
call through a local binding — **and an uncompiled callee refuses the
*enclosing* function rather than emitting a trampoline, so one lambda under a
root refuses the root.**

**What the seam would lose if a backend were given a callback instead** is paid
in invariants, not lines. Entry takes a shared reference: *nothing is committed
until there is a value, so a decline restores nothing because nothing was
disturbed.* The call frame is pushed **after** entry returns, sound *only
because entry is handed no route back* — with a callback that push moves above
entry, **and then a decline must pop it, so the two paths stop being one line
apart.** **The bailout stops being free**: the call count, the allocation
counter and the memo have already moved, **so re-evaluating from the top repeats
them — and re-evaluating from the top is how this seam keeps the diagnostic the
interpreter's by construction.** A debug-only witness asserting the machine's
counters are unchanged across entry **exists so that adding a callback goes red
here instead of breaking in silence — so the change would begin by deleting the
tripwire built to catch it.**

**One invariant does *not* break, and it is the one that looks most fragile.**
The effects gate survives a callback: it is transitive over the named call graph
**and its scan walks lambda bodies.** **What a callback breaks is the
machine-state invariant, not the purity one. So the cheap direction is the other
one: compile the callback**, which changes nothing in the evaluator at all.

**Priced**, by narrowing the registry to what a callback-free code generator
could compile — **a narrowing that can only add declines, so it cannot change an
answer** — computed **outside** the spike and **validated against a shipped
module's own comment about its call-site count, which is exactly what the scan
finds.** The root the whole win depends on is **not** in the set.

> **The lambda wall costs the difference between an infinitely fast backend
> deleting nearly all of the run and deleting about half of it.**

**A callback-free fragment cannot hide a single lambda, ever**: its refusal
histogram is **identical to the no-backend arm, gate for gate** — not a
coincidence, but because **a definition with no callback user anywhere in its
closure has no lambda call beneath it, by construction. So for such a code
generator the lambda refusals are *irreducible*.**

**The narrow escape — compile a fold whose callback is a *named* definition — is
buildable, sound, and measured to be worth zero, in both places it could be
spent, before either was built. Neither was built.** At the seam, the one
definition with a function-typed parameter is inside the entered root, and under
a callback-free backend **its own body drives its loop with a lambda, so no such
generator has a body for it. The one place on this front end where a callback is
a named definition is a function whose own body needs the anonymous case.** In
the code generator, recomputing the fixpoint with that widening moves the set by
**zero definitions** — **and a widening that changes no member of the fragment
cannot change a number, so no series was run for it.**

## The collapse costs the workload its power to police the seam

**Three of eight corruptions stop firing here, and that is the finding.** After
the widening the only entered calls answer bytes or a record, so the three that
need a scalar answer, a boolean, or a registry miss **have nothing left to bite.
A fourth cannot even be presented: a corruption aimed at a named lexer
definition gets zero offers, because that definition is inside the entry now.**

Nothing about the seam got weaker: all of them still fire and are still caught
over the workspace corpus. **But anyone who reads this workload's green as
evidence about them is reading a vacuous pass.** And the other half of the same
coin is the callback-free arm's: **under that narrowing the fragment is scalar
leaves again and all of them fire and are caught. A fragment that enters half a
million leaves polices the seam better than one that enters thirteen roots, and
the fragment that is fastest is the one that polices worst. That tension is now
a property of this tree and should be stated whenever a result from this
workload is quoted as evidence.**

One more thing the same table shows, **and it is the answer test doing visible
work on real source**: under a stale-answer corruption most offers are *not*
entered, because an answer of the wrong kind for the definition asked is refused
— **so the corruption becomes a decline rather than a wrong answer.**

## The two halves together, and the sentence that predicts which

Two changes landed a day apart and **neither was ever run with the other.** One
put a code generator behind a flag; the other closed the fragment. Running the
combination is the experiment both describe and neither could perform.

**The seam's widening reached the code generator. Its registry did not.** The
tree-walking backend's entry count reproduces exactly and its *denominator* does
not — it is now the closed fragment's admitted set, an order of magnitude
larger. **So the type gate did reach the code generator on the *offer* side the
day it landed. What did not reach it is the registry**: it holds a handful of
enterable definitions against the tree-walker's hundreds, so it declines almost
every offer and **enters at the bottom instead of once at the root.** The two
registries are two functions, and they had parted company in the one place
nobody looked: **a hand-rolled scalar test that predates the type gate and was
never revisited by it.**

**It is not a safety gate, and this is checkable.** Its own doc says so, and the
runtime backs the claim — an unbox of the wrong kind fails and the failure maps
to a decline. **A wrongly registered body declines; it cannot answer wrongly.**
What it buys is *time*, **and a claim about time is settled by a clock, not by a
review.** A census added by this change reports that **all but a twentieth of
the already-compiled bodies are dropped at registration**, and a measurement
knob registers them all — **reproducing the callback-free entry line by a real
code generator**, where the earlier arm could only reach it by narrowing a
tree-walker. **The two instruments agree on the *set* and disagree by two
definitions.**

**On the front end, every backend arm is slower than no backend.** The
pre-registered bar was that the code generator beat the unbacked arm by more
than the null control; **it is *above* it by nearly ten times the control.** The
three registered predictions all held, one of them understated: it does not beat
no-backend at all. **On the kernel, the same code, the opposite result** — the
code generator is several times faster than no backend, and the widened registry
faster still, **which says the kernel multiplier survived the seam's widening and
the move to shipping code.** *Quote the resolution honestly: the widest kernel
arm is two ticks of the clock, so the ordering is solid and the ratio is
coarse.*

> **Widening the registry helps when it lets the machine enter *higher*, and
> hurts when it only adds more *leaves*.**

**Registered before any kernel arm was timed**, with the prediction it entails —
that the sign of the widening's effect is *opposite* on the two workloads — and
the statement that if it were not, **the explanation is wrong and the front end
needs another. It held.**

**The entry lines are the mechanism and they are the whole of it.** On the front
end, narrow gives tens of thousands of leaf islands and wide gives half a
million — ***more* leaf islands.** On the kernel, narrow gives thousands of leaf
islands and wide gives dozens: **every offer entered, none declined — the
root.** On the kernel the root is compilable, so widening collapses shallow
entries into a few at the top; **on the front end the roots are not compilable,
so widening registered more *singleton* islands reached from interpreted
parents, and each additional island is one more boundary crossing rather than
one fewer.**

**A boundary crossing is not free and this is the number that governs.** Per
entry: a registry lookup, a borrow, a context begin, a value clone and an arena
push *per argument*, the call, two post-conditions, a value clone out, and a
context end. **For a nullary body returning a constant that exceeds what the
machine's own dispatch costs. Entering it half a million times is how the widest
arm became two-thirds slower than no backend at all.**

## What this says about the gap

- **The absolute gap is unchanged**, and the best backend arm makes it slightly
  worse.
- **The ceiling is not reachable by widening a registry.** The widest arm
  registers most of the compilable bodies and moves *away* from it.
- **The callback-free ceiling is the right target and is optimistic as a
  prediction of *this* generator.** Its arithmetic is not wrong — **it prices an
  *infinitely fast* backend at that entry count, and the finding is that at that
  entry height no real backend can be fast enough, because the boundary is paid
  every time whatever is on the other side of it.** Read it as **a bound the
  entry count makes unreachable, not as a target.**

## Decision

1. **No backend ships on the strength of any of this.** *(One has since shipped,
   on ADR 0026's authorisation and for its reasons, not these.)*
2. **The next lever is the code generator's constructs, and it is now ranked
   rather than guessed.** **The largest row of the census is not a construct at
   all** — it is *a call to a function outside the unit*, **cascade rather than
   cause**: the fixpoint drops a function and on the next round every caller is
   refused for calling it. **So the rows below it are the *roots* and that row is
   their blast radius, which is the leverage argument for fixing them, and it is
   why the compiled set sits scattered at the leaves instead of connected up to
   the parser's root.** Ranked, the roots are: string concatenation; a record
   pattern nested inside a constructor pattern; the three callback builtins; a
   named function used as a value; a lambda; a decimal literal; a call whose
   callee is an expression; a constructor pattern in a list pattern; and a call
   through a local binding. **The top two are not the callback problem and are
   plain missing lowerings — the cheap half of the distance between the front
   end's loss and the kernel's win — and nothing in the evaluator has to move for
   them.** *Since taken, all of it: the two lowerings, then the callback family
   as one piece — a native closure kind, callbacks as runtime loops, values as
   closures, calls through values — after which the parser spike's fixpoint
   keeps every function that performs no effect and writes no `Decimal` or
   `Float` (`docs/BOOTSTRAP-PATH.md` step 2).*
3. **The narrow registry stays the default**, because the front end is the
   bootstrap workload and the pre-registered bar for changing it was not met. The
   widening knob ships as measurement scaffolding, **because the arm it enables
   is the one that produced the sentence above.**
4. **This workload is retired as a policing workload and kept as a timing
   workload.** Every claim about whether the seam is policed belongs to the
   workspace corpus.
5. **The floor arm joins the two-binary method as a standing instrument.** It is
   a filter that selects nothing — no code, no flag, nothing to maintain — and
   whoever takes the next measurement should take it beside the doubling arm.
   **That check cost one minute and was not idle: taken out of gate it read
   twenty percent against a fifteen percent band and in gate a twentieth of a
   percent, which is the load showing up in a place a single arm would have
   hidden it.**
6. **The entered-names counter stays; the clock does not.** **How long a backend
   spent inside an entry is measured from outside the process and must go on
   being: the evaluator may not carry a clock, and the next person to want this
   number will reach for one first, as I did.**

## What would make this wrong

- **The saving is attributed to the entered set and something else caused it.**
  The dose-response is the answer. **If a future change makes those diverge, it
  is measuring something else.**
- **The ceiling assumes an entry saves the whole subtree.** It does under a
  backend that runs the body to completion. **A backend that could only compile
  part of a body and called back would not reach it.**
- **The reachable share is not a deletable share.** Nothing here measures what
  native code costs.
- **Two workloads, and the sentence is fitted to exactly two points.** It was
  registered before the second, **which is what makes it a prediction rather
  than a description** — **but a third workload with a partially compilable root
  is the test it has not had.** And **if the code generator gains the top two
  lowerings and the compiled set *still* does not connect upward, the mechanism
  is right about entries and wrong about what is blocking them.**
- **The corpus is thirteen files of Ply that this project wrote**, chosen
  because it is the one the gap was taken on. **The brief this was set from
  names a much larger corpus than the one that exists**; recorded and not
  resolved, **and if the intended corpus was a different one, every absolute
  second is on the wrong denominator and the ratios are not.**

## Provenance

Every series is counterbalanced with a null control of two identical commands
under different labels, min user CPU over the blocks, timed from outside the
process, load recorded on both sides. The binary was checked current before and
after every series, **and no series spans a rebuild.** **A series taken above
the load gate is reported as an observation rather than as a figure.** Source
digests over every Rust file were taken before the doubled binary was built and
**equal** after it was reverted. **No clock entered the evaluator crate.**

**One harness defect is on the record because it silently produced empty fields
rather than an error**: one series silenced the timer's own output along with
the command's. It was found before any number was taken from it, **and the fix
was proved to report a number before the series was restarted. The arm now fails
loudly on an empty measurement rather than writing a blank field.**
