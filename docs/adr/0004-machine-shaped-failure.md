# ADR 0004 — Machine-shaped failure

**Accepted, implemented.** Depends on ADR 0003: the store must hold definition
**bodies**, not only hashes and interfaces.

## Context

The premise of the language is that the primary consumer of a test failure is an
agent, not a person. An agent reading a suspect list still has to do the
diagnosis itself — read four definitions and re-derive what the system already
knows. Two things make that unnecessary, and both are consequences of decisions
already made.

**The system knows what changed.** The store records which hashes it has seen
and a test's closure is on hand, so their intersection is the suspect set — but
that over-approximates three separate ways. A reference contributes the
referent's hash, so editing one definition moves the hash of every transitive
dependent and every one shows up as a suspect with no edit behind it. The
closure is static, so it names definitions that never ran. And nothing
distinguishes a definition that is *implicated* from one that is *responsible*.

**The system can run the question.** Because compilation is content-addressed
and cached, a *hybrid* program — some definitions at their old hashes, the rest
at their new ones — is a legitimate program, its test hash is a legitimate cache
key, and most of a search over such programs is answered without evaluating
anything. This is why bisection comes after the incremental front end rather
than before it: on a system that recompiles a module per touch, running a test
twenty times in twenty configurations is absurd, and here it is cheaper than
reading the diff.

So the job is not to add fields. It is to move work from the agent to the
system: *which change broke this, out of the ones I made*, and *what actually
ran on the way to the assertion*.

## A failure answers four questions, in this order of value

1. **Which change caused this?** — bisection over the definition graph.
2. **What actually ran?** — the causal slice.
3. **What else could have?** — the suspect set, ranked and annotated.
4. **What was asserted?** — the structured expected/actual.

The terminal output follows the same order, because the culprit is the answer
and the diff is the evidence. When there is no culprit the block degrades rather
than apologizing, and the slice carries the weight: a first-ever red test and a
regression are genuinely different situations, and a regression leads with a
name while a new test leads with a path.

## The baseline

Bisection needs a configuration the test passed at. A result cache keyed by hash
is not enough — a test's hash covers its whole closure, so a regression has a
*different* hash and there is nothing to look up.

So the store keeps a **pass record** keyed by the test's `<module>.<label>`,
holding the name-to-hash map of its closure at the moment it passed. One record
per test, overwritten on each pass, never written for a failing or nondet test.

Keying by *name* is the one place in this system where a name is load-bearing
for a cache, and it is deliberate: the whole point is to survive an edit that
moves the hash, so the key has to be the thing that does not move. Renaming a
test's label loses its baseline, and the cost is one missing bisection, never a
wrong one.

## An edit is not the same as a hash that moved

Every name where the baseline and current closures disagree is a change, but
only some are candidates. A definition whose body is byte-identical and whose
hash moved only because a dependency's did is **derived**, and is never a
candidate — there is no change to attribute to a definition whose text nobody
touched, and flipping one is a no-op.

**That split is the single biggest reduction and is only available in a
content-addressed system.** Editing one leaf moves the hash of every transitive
dependent; on a realistic graph that turns one edit into a dozen suspects,
eleven of which nobody wrote.

**How it is decided.** Re-normalize the definition's *current* body against the
*baseline* hash table; if the result equals its baseline hash, its own structure
is unchanged. One normalization per changed definition, and exact.

Two cheaper tests were considered and both are unsound, which is worth recording
because both look right: *"the set of names it mentions is unchanged"* —
`f(x)` becoming `f(f(x))` mentions the same names and is a different definition;
and *"its interface is unchanged"* — an interface says nothing about a body. A
false "derived" drops a real candidate and yields a confidently wrong culprit,
which is worse than no bisection at all.

## Hybrids, and the ones that do not typecheck

A hybrid takes the post-edit body for every definition in a chosen set and the
baseline body for everything else, and runs **the test as it is written now**.
The test is pinned to its current body in every hybrid, because the failure
being explained is *this* test's failure; the old test asserting something else
is not evidence about it.

The empty hybrid is therefore the baseline definitions under the current test,
and its outcome is itself an answer. It fails and the test was edited → the edit
to the test is the culprit. It fails and the test was not edited → nothing in
the definition graph explains this failure, so look at a nondet effect,
something outside the program, or a defect in Ply — a genuinely useful answer
that a plain suspect list cannot produce. It passes → bisect.

**The typecheck problem.** A changed signature means old and new callers
disagree, so many hybrids are not well-typed programs. This is common, not
exotic: it is what every rename-a-parameter edit looks like. "Give up" is not an
answer, and neither is "treat it as a failure" — a program that does not compile
is not evidence that the test broke.

*Part one: fuse what cannot be separated, before searching.* Compare each
candidate's published interface on the two sides. **A candidate whose interface
is unchanged stands alone; a candidate whose interface changed is fused with
every candidate that mentions it.** That rule is exactly the typecheck
condition: a caller only notices its callee being swapped when the callee's
*interface* moved, and a caller that had to be edited because of that is itself
a candidate. A caller that did *not* have to be edited is derived, its body is
identical on both sides, and it compiles against either. Dependency edges are
unioned over **both** eras — the current graph alone misses a baseline body
referencing a since-deleted definition, and the baseline graph alone misses a
caller written against a since-added one. Unioning over-approximates, which
merges two clusters that could have been searched apart: a slower search, never
a wrong flip.

*Part two: a hybrid that still does not check is not evidence.* The search is
three-valued, and `Unresolved` covers "does not typecheck", "a body is missing"
and "it failed, but not with the failure being explained". Delta debugging
refines its partition past an unresolved configuration rather than concluding
from it.

The honest cost is stated in the artifact rather than hidden: **any unresolved
trial disqualifies the minimality claim**, even one off the path to the answer,
because the search walked around a question it could not ask. Confidence drops,
and a count of unresolved trials is reported. At the limit, where the search
narrows nothing and had unresolved trials, the verdict is *inconclusive* rather
than *bisected* — returning the whole change set under a "bisected" label would
have a consumer act as though the search had endorsed it.

**A failure signature** — code, primary span, message — decides whether a hybrid
reproduced *this* failure. A different assertion failing is unresolved, not a
reproduction. Requiring the message to match is deliberate: an `assert_eq` that
now reports different numbers is a different failure, and attributing this one
to it would be a false positive.

## The search

Delta debugging over clusters, not a plain binary search. "Flip half and see"
assumes a single cause; two edits that only break the test together are ordinary
— a caller and its callee, a constant and the assertion about it — and a binary
search silently returns whichever half it happened to try first. ddmin returns a
1-minimal set and degenerates to the same cost in the single-cause case, so
there is nothing to buy by choosing the cheaper algorithm.

Three things make it cheap, in descending importance: **one cluster needs no
trials at all**, which is the overwhelmingly common case; **a hybrid's test hash
is a cache key**, so the empty hybrid under an unedited test *is* the baseline
test hash the store already holds; and subsets are memoized within a search.

**Writing to the cache.** A hybrid that passes may be recorded under its own
test hash — the hash covers the entire hybrid closure, so the claim is true of
exactly that configuration and of nothing else. Bisection must **never** record
the definitions it ran as *seen*, though: recording a definition retires it as a
suspect, and a definition that was fine *in a hybrid* has not been vindicated in
the real program. This is the one silent-wrongness path here.

**The budget is in evaluations, not seconds.** A budget in wall-clock time makes
the artifact vary with machine load, and an artifact that differs between two
runs over the same code cannot be diffed against yesterday's. A cached trial
costs nothing and is not charged.

Bisection runs automatically when the test failed (not panicked — a panic is a
defect in Ply, not a change), is not nondet, has a pass record, has a non-empty
delta, and has bodies in the store. It does **not** run for a test that has
never passed: there is no "before", and inventing one would produce a confident
answer to a question nobody asked.

## The causal slice

The closure is what a test *could* reach; the slice is what it *did*. Three
things are recorded because they answer different questions: the **stack** at
the moment of failure, which is the path a person reads; **every definition
entered** with a call count, because a definition that returned before the
assertion is still implicated and is *not* where the failure happened, and a
call count in the thousands next to an assertion about a list length is itself a
finding; and **the atoms actually performed**, because a declared atom that
never fired means a branch was not taken.

**Tracing runs on a re-run, not on the first execution.** The green path is the
one that has to be fast, and a push/pop per call sits on the hottest path. A
deterministic test replays identically by construction, so re-running one
failing test costs one test. For a nondet test the replay may not reproduce; the
slice then says so and is reported as evidence about a different execution
rather than silently mixed in. The traced re-run doubles as a **reproduction
check**: a deterministic test that fails and then passes on replay is a defect
in Ply or a leak in a handler, and the artifact says so instead of bisecting a
phantom.

**Two things about the slice are not true of the shipped system and are recorded
rather than implied.** Nothing constructs a slice outside its own audit test, so
every report carries an empty one and the fields below are a specification
rather than a description. And the observed-atom field is described as showing
which branch was taken and which handler fired, which is wrong on its own terms:
both engines record *every* perform, including one a `handle` inside the call
discharges — and discharging is exactly what keeps an atom out of a published
row. **A consumer may read a *missing* declared atom as a branch not taken; it
may not read a *present* observed atom as a declared branch that was.**

## The artifact

`--json` emits one object with a schema version at the top level, because a
machine consumer needs to know what it is parsing before it parses it. Every
field is justified by the action it enables: a field an agent cannot act on is
noise and should be deleted rather than defended. The ranking of suspects is
total and deterministic — bisected culprit, then on the failing stack innermost
first, then ran-but-returned, then untraced, then did not run — because two runs
over the same failure must produce byte-identical artifacts or the artifact
cannot be diffed against yesterday's.

## Alternatives rejected

**Bisect over files with git.** The obvious implementation, and wrong here: it
needs a VCS, it cannot see uncommitted edits — which is the state an agent is
always in — and every step re-runs the whole front end over the whole project.
The definition graph is finer-grained, works on unsaved work, and is nearly free
precisely because of the two ADRs before this one.

**Binary search instead of delta debugging.** Cheaper and silently wrong when
two edits interact, with nothing to buy in the single-cause case.

**Treat "does not typecheck" as a failure.** It terminates faster and attributes
failures to definitions that merely could not be mixed.

**Skip the fusion pre-pass and let the search discover the constraints.** It
degrades gracefully, and pays several unresolved trials to learn what one
interface comparison knows for free, then reports a pair where the fused answer
is exact.

**Rank suspects and stop there.** Real value, and it ships first — but it is a
heuristic ordering of a list. The difference between "probably this one" and
"this one, and here is the program that proves it" is the difference between the
agent checking and the agent acting.

**Trace every execution rather than re-running the failure.** Simpler, and it
taxes the green path — the one the entire language exists to make fast — to
serve the red one.

**Shrinking input values is not here.** Delta-debugging a *counterexample* is
property-test territory and needs parameterized tests; nobody should build a
value shrinker speculatively when there is nothing to shrink. It arrives with
specs (ADR 0007).
