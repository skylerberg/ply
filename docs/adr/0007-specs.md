# ADR 0007 — Specs

**Accepted, implemented.** Builds on ADR 0002 and ADR 0003, whose
content-addressed front end is what makes an obligation discharge once ever, and
on ADR 0006, whose `exhaustive: true` is the only place a proof here comes from
execution.

## Context

The thesis has two halves. The first — that the verification loop collapses
toward zero — is demonstrable. The second has never been built:

> what remains for a human to review is a **specification** rather than an
> implementation.

Nothing before this makes that true. A reviewer reads implementations; the test
suite tells them the implementations agree with the tests, and the tests are
themselves implementations over concrete values, which a reviewer must also read
to know what they assert.

So this is the closing argument, and it has exactly one way to fail: **by
lying.** Every prior mechanism can produce a wrong answer — a mis-selected test,
a missed interleaving, a bad culprit — and every one fails loudly and locally.
This one can produce a wrong answer *wearing a certificate*. A reviewer who is
told an obligation is proved stops reading, which is the entire point of telling
them, and which is why a wrong `proved` is worth more damage than every other
defect in this system put together.

Hence:

> **A tier label is a truth claim. When in doubt, report the weaker tier.**

Everything below is downstream of that sentence, including several places where
the design gives up reach it could have had.

## The rule everything else follows from

> **A spec is a claim *about* a definition, not part of it. An obligation is
> discharged at the strongest tier the system can *demonstrate*, and the tier is
> derived from the evidence rather than asserted alongside it.**

Both halves are structural rather than procedural. The first decides hashing;
the second decides the type of a discharge.

## Surface

`requires` and `ensures` clauses on a definition; `law "name" forall (bs) where
guard { body }` at item position. All four keywords are **contextual**, so a
program with a function or a local named `requires` is unaffected.

Each `ensures` is its **own obligation**, discharged and reported at its own
tier. A definition whose first postcondition is proved and whose second is
sampled should be told so, and one obligation per definition would force the
pair to share the weaker label.

`result` is bound in an `ensures` and nowhere else — a precondition that could
name the result would be a claim about a value that does not exist yet — and a
parameter named `result` beside an `ensures` is an error naming both, because
silently shadowing either direction changes what an existing program means.

A law binder's type is **mandatory**. Inferring it from the body would make a
law's meaning depend on how the body happened to be written and would make the
generator's job unstated; the binder type is therefore non-optional in the AST,
so the invariant is unrepresentable rather than checked.

**Rejected at the surface:** `assert` in the body instead of an expression. It
reads like a test and would let a clause use the assertion machinery's
structured diff. An assertion is an *action* and a spec must be a *proposition*
— the prover needs a Boolean term to negate, and an obligation whose statement
is "this program did not raise" cannot be proved statically about anything. The
diff is recovered at the property tier by rendering the counterexample bindings,
which is strictly more useful: it names the input, not the intermediate.

Also rejected: an `ensures` naming the function rather than binding `result`. It
invites a clause that calls the function with *different* arguments, which is a
claim about a different call, and it makes every postcondition re-evaluate the
definition once per mention.

## A spec expression is pure

> **A spec expression's row must be empty.**

Enforced as one row-purity test per clause. The row's *tail* matters too: a spec
inside an effect-polymorphic function whose clause calls a row-polymorphic
argument is not pure, because it is not pure for every instantiation.

**Why this is not a restriction to be relaxed later.** A spec that can perform
effects can change what it observes. An `ensures` that writes to the resource it
is judging is not a weak specification, it is a meaningless one — the post-state
it reports is the post-state it caused. And a property run evaluates a clause
hundreds of times: an effectful clause would perform hundreds of times against a
state nothing set up, and its footprint would enter the definition's row and
therefore the cross-test conflict graph, **so attaching a spec would change
which *tests* may run concurrently.** Purity is what keeps a claim from being a
participant.

Three consequences: **proving needs no conflict graph**, because every
obligation is pure so no two contend — that falls out of the rule rather than
being arranged; **a spec contributes nothing to any footprint**, so the
determinism check, isolation and the conflict graph are all unchanged; and the
emptiness is carried as a value so an audit can assert it rather than trusting a
comment.

**The one exception** is that a law *body*'s row may be exactly the seed atom,
which is what a body containing a `simulate` region has. Narrow on purpose: it
is the body only, since a `where` guard decides which values the law is a claim
about and a domain that depends on a seed is a different domain per run; it is
that one atom and never an arbitrary one; and a pre/post condition may not carry
it, because a condition is a claim about one call, not about a search.

## What a law quantifies over

**Values, including function values.** Ply already types a function, so
quantifying over one costs the type system nothing and buys the laws worth
having — map fusion, fold/append, "sorting by any key is a permutation".

Two restrictions follow from purity: a function-typed binder's row must be
empty, since applying it inside the body would make the body impure; and a
binder's type must be inhabitable by the generator, checked at *check* time
rather than left as a gap at prove time, because a law nobody can ever check is
a claim nobody will ever read.

**Type variables are handled differently by the two tiers, which is the honest
reading rather than a compromise.** The prover treats a variable as an
**uninterpreted sort**, so a proof over one is a proof for every instantiation
and the certificate records which variables stayed uninterpreted. The property
tier cannot generate a value of an unknown type, so it monomorphises to `Int`
and records that it did. `property` on a polymorphic law is a claim about `Int`
and says so.

**Laws do not quantify over handlers**, because a handler is *syntax, not a
value*: there is no handler type, no handler value, and therefore nothing for a
binder to range over. Recorded with what it would take — a handler type indexed
by the atoms it discharges and the residual row of its clauses; a handler value
and a `handle body with h` form; a normalization story with the usual de Bruijn
treatment; and a generator. **The reason to defer it is the generator, not the
mechanism**: generating "an arbitrary lawful handler" means deciding what
*lawful* means, which is itself a spec, and realistically such a law would be
sampled rather than proved. **A handler-parametric law that could only ever be
sampled is worth less than a value-parametric law that can be proved.**

## Content addressing

`requires`, `ensures` and every `law` are **erased by the normalizer**, exactly
as names, spans, `pub` and module membership are. Therefore:

> **Adding, editing or deleting a spec changes no definition hash anywhere, so
> it re-runs no test and rebuilds nothing.**

That is a headline invariant of the same shape as "renaming a function selects
zero tests", and it is what makes the milestone usable: the spec is the artifact
a reviewer edits, and an artifact whose every edit invalidates the whole suite
is an artifact nobody edits.

**The obligation is keyed by a hash whose first field is the owner's.** That
covers the clause's own structure, the hashes of every definition it names, and
through the owner the entire transitive closure of the definition being
specified. Two properties follow:

1. An obligation discharges once and stays discharged until something in its
   closure changes.
2. **Editing the implementation invalidates the obligation.** This is the
   permissive-direction failure and the one that must not ship: a key omitting
   the owner would leave a discharged `ensures` discharged after its definition
   was rewritten — a cached proof of something no longer true.

**That asymmetry is exactly the asymmetry review has.** The *obligation*
invalidates when the implementation moves; the *implementation* does not
invalidate when the claim moves.

**Gate 2 must not skip a spec clause.** Because a spec is erased from the hash,
a spec edit does not move it, so gate 2 would skip a definition whose clause is
new and the clause would never be typed. Gate 1 is what makes the fix cheap and
correct: a spec edit is a file edit, so the file is parsed and its clauses are
in hand; a file gate 1 skipped has clauses byte-identical to the ones whose
hashes are in the fingerprint. The fingerprint carries the spec hashes so a
skipped file can still contribute its obligations.

**Obligation results live in their own lazily-read file** — folding a per-test
payload into the main result cache was measured once and put the store's open at
three times its budget — and are keyed under their own version constant,
independent of the runtime's, because **a prover that learns a new rule must be
able to *upgrade* a tier without invalidating a single test result, and a
runtime change must invalidate test results without invalidating a proof that
never ran a program.**

What the cache holds is *evidence*, and the evidence type has no variant for a
refutation, a vacuity or a gap — so the "never cached" rows are enforced by the
type the cache is written in rather than by the discipline of whoever writes it.

**The asymmetry is the whole operational value of `proved`.** A sampled
discharge is a claim about the plan that sampled it, so widening the plan
re-runs it — reading it under a wider plan would let ten cases satisfy a run
that asked for a thousand. A proof is not a search: it is a claim about all
inputs satisfying the guard, so it is valid under every plan and costs nothing
forever. **A proved obligation is the only thing in this system that a wider
search does not have to re-examine.** The shrink budget is deliberately not in
the plan digest: it can only change the minimality of a counterexample, and
failures are never cached.

**Rejected: specs as part of the definition body.** No new hash, no new cache
namespace, and the whole content-addressing section disappears. It inverts the
invalidation the design needs — every spec edit would move the definition's hash
and every transitive dependent's, so adding a postcondition to a leaf function
would re-run the entire suite. It is also false: two definitions computing the
same thing under different claims are one computation.

## The tier contract

`Example < Property < Proved`. `proved` claims a static argument covering
**every** input satisfying the guard; `property` claims randomized cases with
the count reported and shrinking on failure; `example` claims concrete cases and
**no** coverage.

### What qualifies for `proved`

A decision procedure over a fixed fragment answers *valid* for `guard ⟹ body`
with every binder replaced by a fresh symbolic constant — which is what makes
the answer universal rather than a statement about one case. The fragment, and
nothing outside it:

**Linear integer arithmetic.** Addition, subtraction, negation, and
multiplication where at least one factor is a literal. `x * y` with both
symbolic is uninterpreted and participates only in congruence closure.
**Division and modulo are not in the fragment as values, at all, including by a
literal.** Division is expressible in Presburger arithmetic and implementing it
correctly is real work that is easy to get subtly wrong; `x / 2 * 2 == x`
reported proved is exactly the defect this must not ship. An uninterpreted
division costs the fragment `x / 1 == x` and buys the guarantee that a wrong
division rule cannot exist.

**Propositional structure**, by case-splitting over the atoms.

**Case analysis over ADTs.** A `match` splits on the scrutinee's outermost
constructor, and within each arm the constructor's fields become fresh symbolic
constants. This is **exhaustive and terminating for every ADT, recursive or
not**, because every value of a sum type has exactly one outermost constructor —
the split is over the *constructor set*, not over the value space. A recursive
type is split to depth 1 and its fields stay opaque, which is precisely the
boundary where induction would be needed and is not available.

**Structural equality and congruence closure**, with constructors injective and
distinct, records equal iff every field is, and every other application — a user
function, an unfolded-nowhere call, symbolic multiplication, division —
an **uninterpreted function symbol** closed under congruence. Treating a Ply
function value as uninterpreted is sound in the direction that matters: an
equality that holds for an arbitrary `f` holds for every actual `f`.

**Bounded unfolding of non-recursive definitions**, to a small fixed depth. A
member of a recursive component is **never** unfolded. This is the rule that
decides the reach, and it is drawn where the mathematics draws it: unfolding a
recursive definition needs induction to terminate at a general statement, there
is no induction, so anything whose truth depends on a recursive definition's
behaviour over unbounded data falls to `property`. `reverse(reverse(xs)) == xs`
is `property`. **It should be, and no amount of clever unfolding makes it
otherwise.**

**Exhaustive enumeration of a finite domain**, when every binder's type is
finite and the product of the cardinalities is small. Every point held ⟹ proved,
because the domain was covered; any point failed ⟹ refuted, with that point as
the counterexample and no shrinking needed. This is why `forall (b: Bool)
{ b || !b }` is proved for two evaluations rather than sampled for two hundred.
**Ground evaluation is the degenerate case with a domain of size one, and is
stated explicitly so an implementer does not report `example` for the strongest
possible evidence.**

### Definedness, which every other rule is conditional on

> A proof is issued only once the prover has also decided that **every input
> satisfying the guard has an answer**.

The rules above reason over mathematical integers and total function symbols.
Ply evaluates over 64-bit integers with checked arithmetic, raises on a zero
divisor, and has no termination checker. Where the two disagree there is no
result for a postcondition to be true of. `x + 1 > x` is valid over ℤ and
**raises** at the maximum; `spin(x) == spin(x)` is valid for a total symbol and
never returns.

So lowering records a **requirement** per construct, discharged in the same
decision as the goal and under the same guard: arithmetic stays in range;
divisors are non-zero; a call this prover did not inline is unsatisfiable unless
the callee is a constructor, a quantified binder or one of a few total prelude
functions.

Three properties carry that. **Every integer fits the machine word**, which is a
theorem and not an assumption — so the proof may assume the bound of every term
that denotes a Ply integer, which is what puts `x + 1` under `x < 100` back
inside the fragment. It may **not** assume it of `result`, nor of anything built
from it: `result` is a value only if the definition returned one, so assuming
its width beside `result == <body>` would let a goal prove its own definedness.
**A requirement is conditional on the path it was reached under**, so the `else`
arm of an `if` owes nothing where it does not run. And **a guard's own
requirements may not assume the guard**: one that raises has no domain to speak
of.

The cost is real and is the point: an unbounded numeric law falls to the
property tier, where the generator's boundary draws report the raise honestly,
and the reader can act on it by writing the bound.

### Inconclusive, and what it may not report

> **An inconclusive proof attempt reports `property`. Never `proved`.**

**And never `refuted` either.** If the prover finds the negation *satisfiable*
it has a model over uninterpreted symbols, and such a model need not correspond
to any actual Ply value — an uninterpreted `f` in the model can be a function no
closure computes. Reporting that as a counterexample would be a confidently
wrong red, symmetric to a confidently wrong proof. **The static side is
refutation-incomplete on purpose: it either proves or shrugs, and the property
tier does the refuting, with a value it actually ran.**

### The outcomes that are not tiers

**Vacuous** — the guard admitted nothing, so the obligation is trivially valid
and says nothing. A system that reported it proved would turn a typo in a guard
into a proof of everything. It fires when the prover proved the guard
unsatisfiable, or when a property run kept **zero** of a full budget *and a
directed search for a satisfying value also found none*. That second half is
needed because the generator draws from the whole of a type, so a guard
admitting nine integers a million away from zero is one two hundred draws will
never satisfy — that is a fact about the *search*, and reporting "the guard
admits no value" for it states something false about the program. So the guard
is evaluated at the points its **own literals** name, and a value found there
rebuts the vacuity with something that actually ran. **The same value vouches
for the domain of a static argument the prover had already completed, which is
the one place a witness upgrades a proof rather than replacing it.**

**Unattempted** — the system could not decide, and says so rather than labelling
it. Four reasons: an unhandled effect, since checking an `ensures` means
*calling* the definition; an ungeneratable parameter type, which is a gap rather
than a compile error because forbidding it would forbid attaching a spec to a
higher-order definition; an evaluation that raised, which is not a refutation
because a spec that raises is not false, and whose raising input is shrunk with
"still raises" as the predicate; and a guard that kept nothing but does admit a
value, which is the search missing the domain rather than the spec being at
fault. **An unattempted obligation does not count toward coverage** — a
definition whose only obligation is undischargeable is a definition a reviewer
still has to read.

### `example` is not a thing a user asks for

A property run draws candidates, rejects those failing the guard, and evaluates
the rest. Above a fixed minimum of *kept* cases it is `property`; below it and
above zero it is `example`. **`example` is what the system honestly reports when
the guard was tight enough that a coverage claim would be a lie** — being told
"property, 200 cases" when seven cases ran is exactly the misreport this exists
to avoid. The minimum is a constant and not a fraction of the budget, so asking
for fewer cases than a property claim needs can only ever produce `example`.

### A tier is never upgraded on a guess, structurally

**There is no `tier` field.** A tier is a function of the evidence a discharge
carries, and the only evidence that computes to `Proved` is a certificate. A
component that wants to report proved has to produce a proof; there is no other
spelling. The certificate names every inference rule it used, from a **closed**
enum containing exactly the fragment's rules — so a prover that grew a rule
nobody sanctioned is caught by a match that stops compiling — and carries a
required flag saying the guard was established, so a certificate that did not
establish its domain cannot be constructed and reported as holding.

**This is the single most important structural decision here. Every other
honesty rule is a discipline; this one is a type.**

## Concurrency laws

A law whose body carries the seed atom is discharged by **execution**, and it is
the one place a proof does not come from a static argument. It is `proved` iff
the mode is the systematic search, the frontier emptied, the budget was not
spent, nothing failed, **and the value domain was covered too** — the law has no
binders, or every binder's type is finite and enumeration ran over all of them.

**That last condition is the one an implementer will drop, and dropping it is
the worst available defect here.** An exhaustive interleaving search over
*sampled values* proves something about those values and nothing about the law:
exhaustiveness is a claim about *schedules*, and a law over an integer ranges
over 2⁶⁴ of them. **The two coverage claims are independent and a proof needs
both.**

The certificate for this path names a distinct rule, so an audit can find every
execution-derived proof in a corpus and check it against those conditions.
Caching is sound under the bare key here for a reason worth stating:
exhaustiveness means the frontier emptied, so raising the budget cannot reach an
interleaving the search did not, so the claim is plan-independent.

## Frame conditions come from footprints

The classic tarpit of program verification is the frame problem: an `ensures`
says what changed, and a caller also needs to know what *didn't*, so other
systems make the user write a `modifies` clause and then prove it. Writing it is
tedious, getting it wrong is easy, and it is the single largest source of
"specification that is longer than the code".

**Ply already infers it.** A definition's footprint is a closed row of
`(effect, resource, mode)` atoms — exactly what it can touch, at resource
granularity — computed for every definition, checked as an upper bound, and used
to schedule tests. The frame condition is its complement: an `ensures` means
*this holds of the arguments and result, **and** every resource outside the
footprint's writes has the same contents after the call as before it*.

**The second half is already proved, by the type system.** It is not an
obligation, not something the prover establishes, not something a user writes.
It is a consequence of the effect system's soundness, established once for every
definition before any obligation exists. So the frame is carried, printed, and
never checked — it is evidence already in hand. **That is what the
resource-granular effect system has been paying for, and this is where the bill
comes back.**

**`old()` does not exist and does not need to.** Ply is a value language: a
function returning a new record leaves the pre-state in scope, still bound and
still exactly what it was. Where state *is* mutable it lives in an effect, and
an effect is exactly what a spec is forbidden to perform — **so no spec can name
mutable state at all, and no spec needs to say when it read it.** The same
restriction as purity, seen from the other side, and why the frame is reportable
rather than provable.

The cost, plainly: **this can specify what a definition computes and cannot
specify what it does to the world.** Saying "the balance in this table
decreased" needs a way to name a resource's contents in a pure expression — a
term with a model of the resource behind it — and that is a milestone of its
own. **This is the largest gap.**

Checking an `ensures` at the property tier means calling the definition, and a
definition with a non-empty footprint needs handlers nothing supplies —
inventing one would be inventing a behaviour and then testing against it. The
static prover still attempts such an obligation first, since a proof needs to
run nothing; it will almost always be inconclusive, because a body containing a
`perform` cannot be unfolded, but attempting costs a bounded number of steps and
occasionally proves something true of any implementation. The follow-on is
syntax that supplies handlers to an obligation, reusing `handle`'s clause syntax
verbatim so the double and the production resource still cannot drift.

## Generation and shrinking

Generation is deterministic counter-mode BLAKE3, **keyed by the obligation, not
only by the root**. Without that, adding a law would shift every later law's
cases, so an unrelated edit would change which counterexample a failing
obligation reports and a bisection over it would name the wrong definition.

Integers draw with edge bias including both extremes on every run, which is what
makes the definedness boundary a *reported* gap rather than a silent one.
Recursive ADTs stop drawing recursive constructors past a depth, so generation
terminates for every recursive type. Function types draw from a fixed family
whose every member is pure, total, extensionally deterministic and printable, so
a counterexample naming a function names something a reader can act on.

Shrinking has two requirements, and they are the whole of the honesty:

1. **A shrunk value must still falsify the obligation.** Every candidate is
   re-evaluated and only an actually-falsifying one is accepted. **The shrinker
   assumes no monotonicity of any kind.**
2. **A shrunk value must still satisfy the guard.** A candidate outside the
   domain is not a smaller counterexample, it is a counterexample to a different
   claim.

**Termination is structural, not budgetary.** Every type has a size, a candidate
is accepted only if strictly smaller, and the walk is greedy — so the process
terminates whatever the budget is, and the budget bounds wall clock rather than
correctness. The result is deterministic, which is what makes today's artifact
diffable against yesterday's. **The original is kept alongside the minimum**,
because "shrank from a list of 400 elements to `[0, 1]` in 11 steps" is the
sentence that tells a reader the space was searched.

## Coverage is in the default output, never behind a flag

A definition is covered iff it carries at least one `ensures` whose obligation
**holds**, or is **directly named** by a law that holds. Three deliberate
choices in that sentence: `requires` alone does not cover, because a
precondition restricts a domain and makes no claim about behaviour; a refuted,
vacuous or unattempted obligation covers nothing, because counting it would make
the number go up exactly when the system got less trustworthy; and *directly
named*, not transitively reachable, because taking the closure would let one law
over one hub definition claim the whole program, which is the shape every
coverage metric fails in.

> **The count of definitions carrying no obligation is exactly the surface where
> review still costs what it costs today.**

Which is why it is printed on every run, ahead of the results, and why the
uncovered set is a **list of names** and not only a number: a number is
something to feel bad about and a list is something to work through. **Hiding it
behind a flag would turn an honest tool into a marketing one** — a project with
three proved obligations and four hundred unspecified definitions would print
three green ticks and nothing else, which is a *worse* artifact than none,
because it invites a reviewer to stop.

The review command's whole argument is one table:

| implementation | spec | what a reviewer does |
| --- | --- | --- |
| changed | unchanged | read the obligations. **The cheapest review in the system.** |
| unchanged | changed | read the spec diff, and nothing else. |
| changed | changed | read both — the tier says how much the machine already checked. |
| either | **none** | read the implementation, line by line, exactly as today. |

A row is only reached when the definition carries an obligation that *holds*; an
obligation the machine could not discharge is not a claim it established, so
such a definition falls to the last row. **The advice has to agree with the
count or one of them is lying.** The baseline is what a human last *accepted*,
not what a machine last ran, keyed by name for the same reason a pass record is.

## Validating it

The property is that **a tier label is true**, and the way that property breaks
is never loudly.

**The certificate audit** checks every proof over the corpus names only fragment
rules and established its guard.

**The differential tier audit** is the one that would catch a lying prover: for
every obligation the corpus reports proved, run it at the property tier as well,
widely. A proved obligation that a sampled run refutes **or raises at** is a
defect in Ply and fails the audit loudly. This is the direct analogue of running
two engines against each other, and exists for the same reason: a claim that two
mechanisms agree is only worth what the comparison costs.

**The raise half is not optional.** A proof claims that every input satisfying
the guard has an answer *and* that the answer is true; a refutation denies the
second and a raise denies the first. Arithmetic is checked and recursion is
bounded, so an obligation the prover got wrong about totality can only ever come
back as a raise — **an audit looking for refutations alone cannot fail on the
defect it exists for.**

And the audit asserts a floor on how many proofs it examined, so **an audit that
examined nothing is red rather than green** — which is precisely the defect an
earlier milestone shipped when it reported exhaustiveness over regions never
examined. Worth copying anywhere else this project asserts a property over a set
it computed.

## What this is not

- **A general-purpose theorem prover.** The fragment is deliberately small. The
  reach matters far less than the honesty of the label, and every extension has
  to pay for itself against the risk of a wrong proof.
- **An SMT integration.** No external solver, ever. A solver is a trusted oracle
  whose *version* changes the answer, and a proved label must be reproducible
  from the definition set alone — the same argument that put counter-mode BLAKE3
  in ADR 0006 instead of a PRNG crate. **A proof that depends on which binary
  was on the path is not a proof this project can cache.**
- **A termination checker**, which is why definedness refuses rather than
  assumes.
- **Induction.** No structural induction, no well-founded recursion, no
  user-supplied lemmas. This is what puts every claim about a recursive
  definition over unbounded data at `property`, and is the single largest
  restriction on reach.
- **Quantifier alternation.** There is no `exists`. Every obligation is a
  universally quantified implication, which is what makes a counterexample a
  witness and a proof a decision.
- **Call-site precondition checking.** `requires` is a **filter on the domain of
  the `ensures` clauses beside it**, not a contract checked at every call. A
  caller that violates a precondition is not diagnosed. Checking it needs a
  path-sensitive analysis of every caller and a story for the undecidable case;
  building half of it would produce a `requires` that is enforced *sometimes*,
  which is worse than one honestly documented as never enforced. **A reader of a
  Ply spec must not read `requires` as "the compiler enforces this".**
- **Specifying effects.** The largest gap, above.
- **A bit-vector semantics.** Definedness makes the ℤ reasoning honest by
  refusing to certify an obligation whose arithmetic can leave the machine word;
  it does not make the fragment *decide* what happens there.
- **Runtime contract checking.** Evaluating a `requires` at run time would put a
  spec into a definition's *behaviour* and therefore into the meaning of its
  hash, and would tax the green path.

## Alternatives rejected

**One obligation per definition rather than per clause.** Simpler reporting, and
it throws away exactly the information the tier exists to carry.

**A `modifies` clause.** Ply is the one language that does not need it.

**Reporting the strongest tier the system *believes*.** The version that demos
better, and the one defect the project cannot ship. Made structurally
unavailable rather than merely forbidden.

**Making `example` a tier a user asks for, with syntax for concrete cases.** It
is a test, and Ply has tests. `example` is what the system reports when a
property run was too thin, which is the only time the label carries information.

**Deriving property tests from specs into the test suite.** It conflates two
claims with two cache keys, two selection rules and two exit codes, and puts a
sampled claim into a namespace whose entire promise is that a cached pass is
provably unnecessary to re-run.
