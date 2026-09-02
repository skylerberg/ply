# ADR 0021 — Why a bootstrapped compiler, and what would unblock one

**Accepted as a statement of intent. It decides no implementation.**

ADR 0020 answers *can Ply host its own front end today?* — no, and it measures
why. **It does not say why anyone wanted one. That rationale existed only in a
conversation, which meant the next reader would find a rejection with no goal
behind it.** This is the goal.

## The claim

**Ply's verification loop is O(the change). Every other toolchain this project
competes with is O(the project.)**

That is not a performance difference. **It is a difference in exponent, and it
is the whole thesis.** A workspace test run costs in-target time proportional to
the size of the repository, excluding all compilation. A warm run over the same
examples selects out all but a handful of tests and finishes in milliseconds.

**The second is not "faster." It is *proportional to what changed***, because a
test runs iff its hash is absent from the cache, and a rename changes no hash.
The first is proportional to the size of the repository, **and no amount of CI,
sharding, caching or hardware alters that. Those are constant factors on an
exponent.**

So the argument for a bootstrapped compiler is not that Ply would be faster than
Rust — it is measured as slower by more than an order of magnitude, **and that
is a floor.** It is that compiler work done *in Ply* would verify in time
proportional to the edit, **and compiler work done in Rust never will.**

**That is a claim about this repository before it is a claim about anyone
else's.** Ply's own loop is a Rust loop: the unit of rebuild is the crate, a
release build gates every measurement in the tree, and every improvement Ply
makes to definition-level incrementality reaches every project except the one
building Ply. The bootstrap is what puts the language's own development inside
the language's own thesis, and that compounding is the payoff this record has no
other route to.

## Why today's measurement is the wrong instrument

The obvious way to price this is to measure what fraction of an agent's wall
clock is spent waiting on tooling versus on inference, and to act if tooling is
large. **That instrument answers a question about a regime that is ending.**

If tooling is a tenth of the loop today and inference gets a hundred times
faster, tooling becomes almost all of it. **The measurement would have been
accurate and useless — the same error as reporting an in-fragment kernel ratio
for an end-to-end decision. Right number, wrong denominator.**

The correct framing is that **Ply *only matters* in the world where fast
inference arrives.** In every other world its thesis is a curiosity. So it
should be developed for that world rather than hedged against it.

**A second payoff path makes the bet less contingent than that sounds.**
O(change) versus O(project) also pays off from the project simply growing, with
inference speed held fixed. **At today's test count the constant factors still
mask it. They will not at ten times that.**

## What ADR 0020 established, and what it did not

Established, by writing a Ply lexer in Ply and measuring it: **expressiveness is
not the blocker** — Ply lexed the whole corpus and lexes itself; **throughput
is**, by more than an order of magnitude against the Rust front end, **and that
is a floor.**

*ADR 0020 also argued that the bootstrap method does not survive past the lexer
— that a parser's recursion is the grammar, so it must outrun the call ceiling
and become an explicit-stack automaton, which cannot be differentially compared
function-for-function, losing the verification method rather than just the
porting strategy. **That premise is refuted** (ADR 0022): the reference parser
drives every sequence with a loop, reserves recursion for grammar nesting, and
bounds that nesting itself far below the ceiling — and `iterate` gives Ply the
same shape at depth 1. **A falsifier fired on this record's own terms. The
throughput objection is untouched and remains the live one.***

Not established, and **it is the number that would most change the estimate**:
the lexer-to-front-end multiplier was **assumed**. The only thing that would
settle it is writing the parser, which ADR 0020 recommends against — **so the
figure that could refute its central estimate is one it declines to take.** *(A
parser has since been written and the multiplier measured for two phases of six;
the conclusion strengthened.)*

## The critical path

**None of these are self-hosting work.** All of them are defects that hurt every
Ply program, and each is a precondition.

1. **The positional cost rule must become visible.** A growing container must be
   built in the last sub-expression of its enclosing node or the program is
   quadratic; **the rule is non-local, so a correct callee goes quadratic when
   its caller places the call in a non-final position**; and it is already being
   paid in shipped code, in the JSON serializer.

   *This item was "a lint for the field-order rule", on the reasoning that there
   is no local property an author can check so a lint is the only fix rather
   than the convenient one. **The first clause is right and the conclusion does
   not follow.** A lint over this property is a partial oracle, and measurement
   showed what that means: it fired on a shape that copies nothing and stayed
   silent on one that is fully quadratic — **a false negative on the exact shape
   it existed for.** "No local property an author can check" is an argument for
   putting the property in the *type*, **which is what this record already did
   for effects** (ADRs 0024, 0025).*
2. **The nested-call ceiling**, which needed either raising or a shape that does
   not reach it. *Delivered as `iterate`, and a raisable ceiling **refused**,
   because the result cache would answer `Pass` for a program a lowered bound
   would make raise (ADR 0022).*
3. **The fragment, entered at token granularity.** Dispatch dominates builtin
   bodies by roughly twenty to one, **so compilation removes the right half.**
   What is unmeasured is the cost at one entry per token rather than one per
   file. **This is the item that makes a compiled backend a critical-path
   question rather than a throughput preference** (ADR 0026).
4. **The map, record and list machinery.** A fifth of executed work, **outside
   the fragment however many functions compile.** Widening moves which functions
   compile; **it does not make a map insert cheaper.**

## What would make this wrong

- **If the inference speedup does not arrive.** Then tooling stays a minority of
  the loop, O(project) remains affordable, and this is over-engineering. The
  second payoff path is the hedge, **and it is slower.**
- **If the parser turns out to be expressible as recursive descent after all** —
  by raising the ceiling, by trampolining, or by a form nobody has tried.
  **This fired, by the third route.** The reference parser already reserves
  recursion for grammar nesting and drives sequences with loops, and `iterate`
  gives Ply the same shape at depth 1. **Recorded because a falsifier that fires
  and is not written down is a falsifier nobody wrote.**
- **If a Rust-side tool could make the conventional loop O(change).** Nothing
  has tried, **and it would remove the motive entirely.** The reason to doubt it
  is that Ply had to be designed around content-addressed definitions from the
  start to get the property; **retrofitting it onto a language whose compilation
  unit is a crate is a different and much larger problem.**

## What this is not

**It is not a decision to self-host.** ADR 0020 decides against it on today's
interpreter and that decision stands. **This records why the goal exists, so
that the next person to read a rejection knows what was being rejected and what
would change the answer.**

**And it is not a measurement.** The claim at the top is stated as an exponent
and has never been taken as one. Both pieces are in the tree and nothing
composes them: `ply-corpus sweep` varies the size of a generated project and
benchmarks whole-project phases over it, and `ply-corpus w5` times a rebuild
after a one-leaf edit at a single size on the deploy path. **No row applies an
edit across two sizes**, which is the only shape that separates an exponent from
a constant. ADR 0037 registers that row, with its criteria fixed before the
reading, and orders it ahead of the path this record motivates.
