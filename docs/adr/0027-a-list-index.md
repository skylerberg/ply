# ADR 0027 — A list index

**Accepted.** It adds **one** builtin, `list_at`, and it refuses four things: a
raising index, a head, a last, and — **on a gate fixed before the measurement
and missed by it** — the defaulting variant this record was drafted to add
beside it.

Corrects several records that reasoned from the *absence* of a list index, and
**corrects itself**: the argument that a raising index would block the property
tier is right about the outcome and was wrong about the mechanism, and the
proved-tier cost is the one that is inert.

## The gap

A parser *is* a peek at successive offsets. The list surface had no index, no
head, no tail — **so the Ply parser's token buffer could not be a list at all.**
It folded the lexer's token list into a map, the only random-access container in
the language, **and every peek became a tree descent with a comparison at each
node plus an option to unwrap, where the reference does one bounds-checked
load.**

The lexer spike had ranked the same absence tenth of fifteen, *"starts to
bite"*; the parser spike ranked it **first**. **The difference is not opinion: a
lexer walks forward and a parser looks around.**

Four workarounds were counted in the shipped tree, each a hand-written index
under a different name, plus two more over bytes for the same reason.

## The surface, and why it is not the raising one

`list_at(xs, i) -> Option<a>`. **Total**: absent for a negative index and for
one at or past the end. The language already contains two conventions for an
accessor, and choosing between them is the real decision. Four arguments, all
from the tree rather than from taste.

**1. No caller in the tree wants raising.** Every hand-written indexer already
shipped is total; two of them are the language's one raising index wrapped in a
total function at its heaviest use sites, and the spike's goes further and
*clamps*. **A raising index would have shipped a builtin whose first act at
every use site is to be wrapped, and that wrapper is exactly the "three calls"
the standard library complains about.**

**2. A raising index blocks the *property* tier, and the mechanism is
execution.** A property run evaluates the term on randomized inputs, one is out
of range, the term *raises*, and the obligation becomes **unattempted** — a
**gap**, never green, never cached — and the definition is reported as not
covered by any claim that holds. Guarding the raising accessor puts it back at
`property`, **so the tier cost of the raising convention is paid at every
unguarded peek, and the price of avoiding it is exactly the wrapper argument 1
counts.**

*This record was drafted saying that, then "corrected" it to say the cost is to
`proved` via the total-builtins list, then withdrew that correction — because it
had reasoned from one mechanism to a conclusion about a tier reached by another
and **talked itself out of the only argument in this section that is live
today**. The correction is now gated rather than argued: a test runs the total
index and a raising control as one module and asserts `property` for one and a
raised gap for the other, and it was seen to fail when the total arm was given
the raising diagnostic. That corruption also demonstrated argument 4 without
being asked to.*

**The total-builtins half is, on lists, currently a cost of nothing — found by
trying to arm it.** Removing the entry and re-running changed no tier in any law
that could be written over a list, because **no list-valued term is in the
decidable fragment today.** The membership is correct and inert, kept because it
is true and is what a later fragment with a list theory would need, **and
recorded as an unarmed change rather than as a gate. A finding about the tree
rather than about this feature.**

**3. The language is already moving the other way.** Every accessor added after
the raising one answers optionally. **It is the precedent that exists, not the
precedent that is being followed.**

**4. A small tell.** The one shared out-of-range diagnostic hardcodes the word
*bytes*. **The code is not built to grow a second raising index.**

**Against the optional form alone**: the option is part of what the spike is
complaining about, and it does not go away — the wrapper is an allocation per
peek. The index alone removes the descent and the comparison chain and keeps the
allocation and the match.

## Head and last are refused, and that is a finding

The spike files three separate needs; **an index alone removes all three.** Head
is the index at zero. Last is the index at length minus one — **and the spike's
problem was never that last was unspellable, it was that with no index the only
spelling was a fold, i.e. a quadratic hidden inside a deduplication rule.**
Length is constant time, so the second spelling is not a traversal. **One
primitive, three workarounds gone; two names not added.**

## Negative indices are absent, not counted from the end

A reader arriving from Python will expect otherwise, which is why the guide says
it in three places rather than one. **The reason: counting from the end reads
well until an arithmetic slip turns an intended index negative, at which point
the program gets an element rather than the absence that would have named the
mistake.** It is not clamping either — an out-of-range index is absent, not the
nearest element, **which keeps "slices and indices are never clamped" true of
lists too.**

## What the engines and the compiled fragment do with it

**The two engines cannot disagree, by construction.** One definition per builtin
that both engines run, reached from both, with no private table; a
non-higher-order builtin answers immediately and never suspends, **which is the
whole of what the two engines implement differently.** So the differential over
these builtins is **a run beside a construction argument and not a gate**;
**reporting it as a passing gate would be this project's signature defect
wearing the other hat.** It was run.

**The seam never sees a builtin as a callee**, so a definition calling this one
internally can be admitted — but the seam's argument test then decided
admissibility from **the definition's declared parameter type**, and a list of
records is carried when the record is. **Measured on the ported front end, the
two definitions this feature is named for are now among the most-admitted in the
whole corpus**, and the admitted share on that workload rises by most of an
order of magnitude. **What is unchanged is the reason one level down**: the
builtin is still refused as a *callee* and the code generator still admits it
only through a catch-all. **The two features meet because the *caller* became
admissible, not because anything about the index did.**

Where a definition is admitted and contains an index, it runs through the
generic builtin path with no span — and because the index is **total, no
diagnostic can originate from it inside a compiled body at all**, which is the
span weakness the raising index has and which a raising list index would have
widened. **A fourth argument for the total form, and it only shows up if you
read the runtime helper.**

**A gate this change shipped vacuous, and the review that found it.** The cost
report needs an arm marking an indexed element as blocked, because the fallback
would otherwise claim an in-place append onto an element the list still holds —
a wrong claim, **which is the only kind of thing that report can get wrong.** The
test written to arm it **did not**: its program wrote the append in a position
the *positional* rule flags on its own, so the verdict was the same with the arm
and without it, **and deleting the arm left the test green.** The program now
puts the append in a helper's tail, where the position rule has nothing to say,
and the test asserts the reason **names the index** — **a right answer reached by
the wrong rule being precisely what the first version could not tell apart.**

**A hole this change closed.** The builtin enumeration had no completeness
check, and four tests iterate it — **so a variant *missing* from it is never
named and therefore never checked by any of them.** Deleting the new variant was
run against the reachability test on the assumption it would go red; it stayed
**green.** The whole name list is now pinned.

**A hole this change did not open and did not close, and this record overstated
it.** The arity table is a second hand-maintained table beside the prelude
scheme. **The hole is one-directional**: an arity *narrower* than the truth is
caught loudly at run time, because the value is read on every call. **What
nothing gates is an arity that is too *wide*, because no well-typed call can
reach the extra slot to meet it** — which is exactly the drift two existing
builtins have already fallen into, both too wide, with their second legs
unreachable from any well-typed program. **So the hole is real, it is the shape
the tree has already fallen into twice, and it is half the size this record
claimed.** That is also why a general "arity agrees with the scheme" test cannot
simply be written.

## No hash moves. Three versions bump anyway, and the reason is sharper

*Verified, not argued.* A builtin call normalizes as a free reference by name —
the prelude is not in the program index — and hashing the whole corpus with a
pre-change binary and a post-change one is byte-identical across every
definition.

**And that is precisely why the versions must bump. The hazard is not a hash
that moved; it is a hash that did not.** A definition calling the new name
hashes to the same bytes before and after and **means two different things: an
unknown name before, a value after.** A cached interface, fingerprint or pass
under that hash is a claim about the old meaning.

The prover's version is bumped, **and here this record diverges from ADR 0022
rather than following it.** That one declined the prover bump with an argument;
this takes it with a different one — the name joins the total-builtins list,
which the store says explicitly to bump for. No *existing* obligation can change
its answer, for ADR 0022's reason, so this re-attempts obligations and re-runs
no test. **A bump for the rule's meaning, not for any obligation's answer, and
recorded as a decision so that nobody later discovers it as a side effect.**

**The name is not reserved.** ADR 0001 reserves exactly three names, each for a
reason this does not have. So a module's own function of the same name shadows
the builtin — **which keeps the two modules that already define one compiling
untouched, and is also why the name is not the obvious short one: those two
modules each already define it, and a bare short name would have been *legally
shadowed* into invisibility in exactly the two files that most wanted it.**

## What the measurement said, and the builtin it refused

Pre-registered before any number existed and outside the repository, with the
statistic, the run count, five numbered predictions and the decision rule. Every
run printed, none discarded. **Two rules were fixed before the number**: the
index ships regardless — **the workarounds are code, not time, and no
measurement can un-need them** — and the defaulting variant ships **if and only
if** it is at least half again faster per peek.

### The rig, and the two defects it had to be repaired for

Seven probes per size, each a generated project holding the same modules so
module typechecking is identical and cancels: a control, the lex, the map build,
a driver with no peek, and one arm per peek implementation.

**The pre-registration said the sweep driver is identical in all arms so it
cancels in the subtraction. It does not** — the difference is driver *plus*
peek — **and worse, subtracting across arms whose *builds* differ produced an
incoherent answer: a tree descent measuring cheaper than an array load.** The
instrument was repaired, in an amendment written before the second sitting's
first run and changing no bar: each arm is measured at one sweep and at many, so
**a peek is a *within-arm* difference in which every byte of the program that is
not the sweep is identical and cancels exactly.**

### The numbers, and what the instrument can actually resolve

**The defaulting variant is refused**: it misses the pre-registered bar at the
registered size and misses it with room to spare at ten times that. **A peek is
almost entirely interpreter dispatch**, and the allocation and match the variant
removes are a fifth of it — real, and not a second name's worth. **The standard
library asked for this to be *measured* before it was added; it was, and the
answer is no.**

**What the tick resolves, added on review.** A derived per-peek figure is a
difference of four readings each quantized to the clock's resolution, **so a
comparison *between two arms* rests on eight of them.** The gate is resolved at
better than two to one; **the headline is not resolved at all at the registered
size**, and is resolved ten to one at ten times it. **So the honest statement of
the headline is that the two containers are within about a tenth of each other,
measured where the instrument can see it, and indistinguishable at the
registered size.** "The same to within two percent" is a point estimate quoted
to a precision this rig does not have, **and it comes from the *less* resolved
of the two sizes.** Nothing about either conclusion changes. **The claim that
moves is the precision, not the verdict.**

### The headline is a withdrawal, and it was registered as one in advance

**The spike's *cost* claim does not hold.** A map peek and a list peek cost the
same to within a couple of percent at the registered size and within a tenth at
ten times it. **The map the parser spike was forced into was not materially more
expensive than the index it wanted**, and neither arm's per-peek cost grows with
size, **so the logarithmic descent is invisible against dispatch — which
withdraws the *mechanism* half of the claim as well.**

**The pre-registration registered this outcome in advance**: if the speed
prediction failed, the cost claim is withdrawn with a number beside it and **the
feature ships on the code cost** — four hand-written workarounds, a container
the spike did not want, and two extra fields threaded through most of its
functions. **That is the case for the index, and it is a smaller case than the
one that motivated it. Stated that way rather than quietly rescoped.**

**And a mechanism this record got wrong.** The prediction that the hand-written
recursive indexer would exceed the call budget and answer a diagnostic was right
in outcome and wrong in reason: **it does not finish at all**, at any interesting
size, inside a generous cap. The tail pattern allocates the tail at every step,
so one peek is quadratic and the sweep is cubic. **The prediction's outcome was
right and its reason was wrong, and that is a sharper argument for the index
than the timing was: the workaround the standard library ships is not slow, it
is asymptotically unusable.**

## What this deliberately does not do

**The spike is not ported**, so the tree gains a builtin with **zero call
sites** — the shape ADR 0025's ill-posed criterion warns about, **filed as
explicit follow-up rather than smuggled in here.**

**No byte-defaulting variant.** The standard library asks for one and the
measurement has now answered the question it asked: on a list the defaulting
form saves a fifth of a peek. **A strong prior that it would not clear the bar
either, and it is a prior rather than a measurement**, because the raising byte
accessor's total wrapper is three calls rather than one, which is a bigger
saving than the one measured here. **Somebody should take it under its own
registered gate.**

**No list representation change.** The persistent-vector gate is untouched, and
is corrected in place to say that **it now has an index cost to be a cost *of*.**

## What would make this wrong

- **The measurement did not support the motivation.** Both of the things this
  bullet was drafted to warn about happened. **If a later reader finds a workload
  where the map *is* materially more expensive, these numbers are the ones to
  argue with** — one machine, one interpreter, one load band, **and they price a
  peek rather than a parser.**
- **If dispatch cost falls.** The whole finding is that a peek is dispatch and
  approximately no container access. **A compiled backend changes the
  denominator, and the refused variant becomes worth re-opening the moment
  dispatch stops dominating — under the same bar, re-measured, not re-argued.**
- **If the representation fallback lands.** It makes the index logarithmic where
  it is constant today. **That matters less than it looks** — a tree descent over
  a large map is already invisible against dispatch — **but it is not the
  constant time a reader might have been promised**, which is why the guide says
  *constant time on today's representation* rather than stating a complexity.
- **If the code cost turns out to be small.** **That is now the *whole* case for
  this builtin**, the measurement having removed the time one.
