# ADR 0005 — The control stack

**Accepted.** The machine landed and is the only engine's model. The persistent
forkable world this ADR also introduced is **superseded by ADR 0017**; §"The
world" below records only what the replacement had to preserve. The resumption
semantics stand unchanged and are what ADR 0017 was rewritten to preserve.

## Context

Multi-shot continuations and copy-on-write world state were one milestone
because one design question decides both.

The question is what `with_cell` guarantees. A cell's atoms are discharged at
the region boundary and a `Cell` in the region's result type is a compile error,
so a cell cannot outlive the region that made it — the ST-monad trick, and why a
handler backed by a test-local cell contributes nothing to the test's footprint,
which is in turn the whole selection-and-scheduling story.

Multi-shot breaks the argument as stated. Capture a continuation inside the
region, resume it after the region has returned, and code holding the cell runs
outside the region whose type-level check was supposed to make that impossible.
The check inspects the region's result *type*; a continuation carries the cell
in its captured environment, where no type mentions it.

Three answers were available.

**(a) Brand the region and forbid escape.** Rank-2 polymorphism over a region
variable, ST-style. Principled, and hostile to the point: the handler patterns
multi-shot exists to enable — backtracking, generators, a scheduler that parks
and resumes — are exactly the ones that move a continuation across a boundary.
It also puts rank-2 types into an otherwise Hindley–Milner system for one
construct, and it moves the failure from "impossible" to "a type error a user
has to understand" for a construct as ordinary as a state handler.

**(b) Make the state a value the machine threads.** State stops being a location
a value points into and becomes an entry in a map. A cell is a key. Escape stops
being a memory-safety question, because there is no memory to be unsafe about.

**(c) Deep-copy cell state at capture.** (b) with worse performance, no
structural sharing and no fork story — (b) with everything good about (b)
removed.

This ADR takes **(b)**.

## The rule everything else follows from

> **State is a value the machine threads. Control is a value the machine
> splices. A continuation captures control only.**

Everything below is a consequence. State is never snapshotted at a capture and
never restored at a resumption; there is exactly one current state at every
point of an execution, and it moves forward.

## The machine

The tree-walking evaluator becomes a CEK-style abstract machine with an explicit
control stack. A configuration is `⟨state, stack, world⟩`, where the state is
evaluating an expression, returning a value, performing an operation, or halted.

**The stack is a list of segments**, each a persistent list of frames sitting on
top of the `handle` that delimits it. Capturing a delimited continuation is
taking the segments from the innermost down to and including the one whose
prompt matched — **one entry per enclosing handler crossed**, one in every
ordinary program, and never one per pending frame. The frames inside a captured
segment are not copied, compared or walked.

```
capture(K, n) = (K[0..n], K[n..])
resume(K, k)  = k ++ K
```

Resuming pushes the captured segments back onto *whatever stack is current*,
which may be a different stack from the one they were cut out of. Because a
captured segment carries its own prompt, **the handler is reinstalled by the act
of resuming: handlers are deep.** And a clause body runs on the stack *below*
its own handler, so a clause that performs the operation it handles reaches the
next handler out instead of catching itself forever.

Applying a continuation is the only rule that changes the stack's *shape* on a
return, and it is one line. A continuation takes exactly one argument — the
value the `perform` it was captured at should have produced.

Performing searches the stack for a handler, splits, and dispatches. The world
is threaded through capture and through resumption **unchanged**; that is the
whole of the resumption rule, stated as a machine transition.

**`map`, `filter` and `fold` are frames rather than host recursion**, because a
continuation captured inside the function passed to `map` would otherwise be
captured across a native frame that cannot be re-entered — the second resumption
would have nowhere to return to.

**The AST is lowered once per machine into a shape with `Rc` on every node.** A
frame cannot hold a borrowed expression without a lifetime on `Value`, which
would spread to every crate that holds an evaluated value; it cannot hold an
owned one, because a frame is pushed per node and cloning a subtree per push is
quadratic.

## Surface syntax for the continuation

A clause opts in to a reified continuation with a binder in its head:

```ply
amb.flip[coin]() resume k -> k(true) + k(false)
```

`resume` is **contextual** — a keyword only between a clause's `)` and its `->`
— so a program that already binds `resume` as an ordinary name is unaffected.

Bare `-> e` stays tail-resumptive and keeps its current typing. That is not
backwards compatibility for its own sake: a tail-resumptive clause is the
overwhelming majority, its body's type is the operation's return type rather
than the whole `handle`'s, and making every clause general would retype every
handler in every existing program to no benefit. `op(x) → e` *is*
`op(x) resume k → k(e)`, which is why every existing handler keeps its meaning.

## Resumption semantics

> **A resumption observes state as of the handler's call to `resume`, not as of
> the capture.**

Snapshot-at-capture is the reading that the phrase "each resumption gets its own
world" invites, and it makes a state handler unwritable. The canonical one
writes the cell and *then* resumes:

```ply
state.put[s](v) resume k -> { cell_set(c, v); k(()) }
```

If the resumption restored state as of the capture, the write would be discarded
before the computation that asked for it ever ran, and `put(5); get()` would
answer `0`. The only state handler you could then write would pass the state
through the resumption value — the state-passing encoding that region-scoped
cells exist to avoid. This is not a backtracking corner: it retypes one-shot
resumption, which is the overwhelming majority of handlers.

The three cases:

| | what the clause sees | what the resumption sees | what survives |
| --- | --- | --- | --- |
| zero resumptions | state at the `perform` | — | the clause's writes |
| one resumption | state at the `perform` | the clause's writes | everything, in order |
| two resumptions | state at the `perform` | resumption *n* sees *n−1*'s writes | the last branch's writes |

The two-resumption case is the one that pins the design and is a required test.
A handler resuming twice over a body that increments a trace cell leaves that
cell at **2**; under snapshot-at-capture it would be **1**. That integer is the
observable.

Nothing runs at a discarded continuation — Ply has no finalizers, so dropping
one is dropping a pointer.

**A handler that wants per-branch state builds it explicitly**, with the cell it
already has: save before each resumption, restore after. Four lines, no new
construct, and — importantly — *visible in the handler*, where a reader can see
that the handler is the thing deciding. This is the classic `State ∘ Nondet`
versus `Nondet ∘ State` ordering. Ply fixes one order and lets the handler build
the other. **The reverse choice cannot be undone by a handler at all, which is
the asymmetry that settles it.**

## Footprint typing under multi-shot

**A row is a set.** Resuming twice performs the same atoms twice and the set is
the same set, so the `handle` rule is unchanged and `resume` performs nothing of
its own — its atoms are the residual atoms of the handled computation, which the
body's row already carries.

A general clause's continuation is typed `(ret) → R` at the *handle*'s row, and
two things about that must be exactly right. **One row variable per `handle`,
not per clause** — every clause's continuation is the same residual computation.
And **solving it drops a self-occurrence in the tail**: the handle's row is
built from the clause rows, which may carry that variable as their tail, so the
constraint is self-referential. Set union is idempotent, so the least fixed
point is reached in one step. This is the **only** row variable for which a
self-occurrence is permitted; general unification's occurs check must stay
exactly as it is.

**The conflict graph is invariant under multi-shot.** Adding or removing a
resumption changes no row, so no footprint, so no edge, so no colouring. That is
not a coincidence to be grateful for; it is the reason rows are sets.

**A footprint has never been a count, and this is where that starts to show.** A
test whose footprint is `{db.write[orders]}` and whose handler resumes twice
writes twice. The scheduler was already correct — one write and a thousand
conflict with the same things — but a *reader* must not read an observed
footprint as evidence about frequency. What does move is wall clock: a test can
be exponential in resumptions while its footprint stays a singleton, and groups
are coloured by conflict, never by cost.

## The world, and what replaced it

This ADR made state a persistent map: `Value::Cell` became a key rather than a
pointer, `fork` was `clone`, and a fixture was a `(world, value)` pair. **ADR
0017 removed all of it** — Perceus-style in-place update fires only on a
uniquely owned value, and a design that forks worlds keeps reference counts high
by construction, so the two are mutually exclusive.

What is worth keeping from that section is the mapping and one reversal.
`fork` at an entry point became a region reset between entry points. "A cell is
a key, not a pointer" is still true, and the key is now an arena slot rather
than a map key. And **"the world is monotone; an entry is never removed" is
false now**: a region's slots are reclaimed at its lexical close unless a
continuation can still reach them.

What did *not* move is the resumption semantics above. Read them as current.

## What the explicit stack lets us delete

The recursion depth guard was a workaround for a native stack limit — its own
comment said so. Once the stack is a heap value, none of that is true: the
native-stack growth helper goes, the `#[inline(never)]` markers that kept the
recursive evaluator's frame small go, the `RefCell` reentrancy failure stops
being reachable, and a stack that is a value cannot leak from one entry point to
the next.

The *semantic* limit stays: a runaway recursion must be a diagnostic and not an
out-of-memory kill.

**The bound is on nested calls, not on pending frames**, and getting that wrong
shipped two evaluators disagreeing. A frame budget does not cover tail position
— the machine reused the pending call frame of a tail call, so a tail call cost
zero frames and no frame budget could ever fire for one; `fn spin(n) = spin(n+1)`
ran past a 45-second wall clock with no diagnostic where the tree-walker
answered in milliseconds. And a frame count and a call count are different
things at different scales, so a program between the two budgets is a diagnostic
on one engine and an answer on the other.

**Tail-call elision is gone with it.** Charging a tail call is what bounds a
tail-recursive runaway; eliding it is what made the two engines disagree. What
takes its place is `iterate`, an early-terminating loop that is depth 1 on both
engines and takes its bound as an argument (ADR 0022).

**A separate frame bound is gone too, and the reason is worth keeping.** The
argument for one was that a call costs at least one frame, so the call bound is
reached first and a frame bound catches only a pathological program. That is
wrong: a *call* costs one frame, a *body* costs as many as it pends, so a body
pending enough frames per call reaches a frame bound first. But the ceiling had
to go rather than be copied into the other engine, **because it was a function
of *spelling*, not of behaviour** — measured on two definitions of the same
function making the same nested calls, the one written with a folded constant
answered and the one written as a long chain of additions raised. What is left
is an opt-in ceiling that is not part of what a program means, and a machine
carrying one enters no compiled body, because a native body pends no frames and
cannot honour a limit counted in them.

## Consequences

- **No headline invariant moves.** The machine changes how a program runs, not
  what a program *is*.
- **The `resume` binder enters normalization.** A clause with a binder is a
  different definition from one without, and renaming the binder must change no
  hash because it is a local and becomes a de Bruijn level. Getting it wrong the
  other way — omitting the binder from the hash — makes two programs with
  different semantics share a cache entry, which is the worst defect available
  in this system.
- **The persistent-collection dependency** was chosen over the obvious
  alternative because it is parameterized over the shared-pointer kind, so
  non-atomic refcounts match the decision that an interpreter is confined to one
  thread, and because its map iterates in key order, which the
  byte-identical-artifact rule needs and a HAMT does not give.

## Alternatives rejected

**Rank-2 region branding.** Above. Kept in weakened form: the result-type region
check stays because it is a good error message for the ordinary mistake, and its
demotion from load-bearing to convenience is deliberate.

**Deep-copy at capture.** Per-resumption state — which the resumption rule
argues is the wrong semantics anyway — at O(state) per capture.

**Snapshot at capture.** Makes a cell-backed state handler unwritable, and is
one-way: a handler can build snapshot semantics out of threaded semantics in
four lines, and cannot build threaded out of snapshot at all.

**Shallow handlers.** Cheaper to implement, makes every stateful handler
recursive by hand, and would break every existing program, since deep is what
the existing tail-resumptive semantics already is.

**`resume` implicitly bound in every clause.** No grammar change, and it retypes
every existing handler and forces a capture at every `perform` whether or not
anyone wanted one.

**Frames holding borrowed expressions.** A lifetime on `Value` spreads to every
crate that holds an evaluated value.

**Trampoline the tree-walker instead of rewriting it.** Buys the stack depth and
none of the milestone: the frames have to exist for capture to be O(1), and once
they exist the recursive evaluator has nothing left to do.

## Not done here

- **A source-level `fixture` construct.** The mechanism landed and no syntax
  did. A fixture is a definition, so it needs a hash story, a determinism story
  and a place in the namespace, and inventing those under a machine rewrite is
  two designs in one change. **This is the largest gap.**
- **A state snapshot/restore builtin.** A capability with no type-level account:
  restoring un-does writes that the row still reports.
- **One-shot annotations on continuations.** Worth doing when there is a
  measurement saying the capture costs something; there is not.
- **Control operators beyond `resume`.** `handle` is the only delimiter.
