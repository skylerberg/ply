# ADR 0010 — Derivation now, dispatch deferred

**Partially accepted.** Derivation shipped. The resolution layer is deferred
with two candidates and **the evidence that would choose between them was never
taken** — see the end of §6.

## Context

Ply has no typeclasses, traits, interfaces or `derive`. There is no way to write
one function that works for any type with a known encoding.

Tests and specs are written against concrete types, so for a long time this cost
nothing. A web API cannot avoid it: JSON encoding and decoding, SQL row mapping,
equality and ordering all want the same shape — given a type's structure,
produce a function — and without a mechanism each is hand-written per type and
drifts from the type the moment someone adds a field.

**Parametric is not ad-hoc.** Ply already has full parametric polymorphism; what
it lacks is dispatch directed by a type. Much of what gets called "framework"
needs only the first: routing, middleware, connection pools, config, tracing and
shutdown are all concrete or parametric. **The ad-hoc requirement concentrates
at the serialization boundary** — row in, domain type, JSON out. Narrow as a
surface, crossed by every endpoint.

And it does not hurt at concrete call sites, where an explicit argument costs
nothing. It hurts where a type is still abstract **inside a function's own
body** — rare in application code, common in framework code.

## The design space, and what the survey taught

This is not two options. Every design is a point on four axes: the dictionary
form (positional argument, record, module, method table, synthesized), the
passing (explicit or implicit with search), the search scope (global, imports,
owner-restricted, none), and where errors surface (the signature or the
instantiation). Haskell is *(synthesized, implicit, global, signature)*; Zig
comptime is *(none, n/a, none, instantiation)*.

- **Rust: derivation and dispatch are complements, not alternatives.** serde is
  `derive` *and* a trait bound. Derivation generates the mechanical instances;
  the trait provides abstraction. **So deferring dispatch is a scheduling
  decision rather than a claim that derivation suffices.**
- **Rust: the orphan rule makes coherence a local property.** You may implement
  a trait for a type only if you own one of them — coherence without a global
  table, checkable from what a module can see. The closest existing fit to Ply's
  constraint.
- **Scala: unconstrained implicit search is a debugging disaster.** Scala 3
  renamed implicits and constrained resolution precisely because the mechanism
  was too powerful and "implicit not found" was among the worst error
  experiences in mainstream languages. If resolution is added here, the
  dependency must stay visible in the signature.
- **Zig and pre-concepts C++: reflection without constraints fails late**, deep
  inside instantiation. C++ added concepts to fix exactly this, so bare
  reflection does *not* win on error locality.
- **C++ concepts: constraint checking and dispatch are separable.** A concept is
  a predicate on a type, checked at the call boundary, doing no resolution. Good
  errors without any dispatch risk.
- **OCaml: explicit-first is a legitimate stable end state.** It is also the
  natural experiment for "no coherence, explicit naming", and serialization is
  where that is most painful.
- **Idris: a constraint *is* an implicit argument, and resolution is a search.**
  There is no categorical difference between a typeclass and a dictionary, only
  whether you write it and how the compiler finds it.

## Decisions

**1. Build derivation now — it is the substrate under every candidate.** It
generates ordinary Ply definitions by walking the type's structure at compile
time, using the canonical normalized form hashing already computes. Typeclass
*instances* for records and ADTs would be derived rather than hand-written under
any design, so this cannot be stranded by a later choice. It is structural and
total: a type containing a function, a cell or a continuation cannot be derived,
and that is a compile error naming the field rather than a partial encoder.
Generated definitions are hashed from their generated form, so changing a type
changes its derived function's hash and re-selects its tests.

**2. A dictionary is a record.** Ply already has structural records, so this is
free and strictly better than loose positional arguments: named parameters
instead of several, extensible without breaking call sites, and **a record of
functions is exactly what a typeclass dictionary elaborates to** — so it is the
most forward-compatible form available.

**3. Framework signatures take explicit dictionaries**, deliberately in the
elaborated form of a typeclass constraint. `fn query<a: FromRow>(sql)` and
`fn query<a>(sql, rows: RowCodec<a>)` are not competing designs at the signature
level — the second is what the first becomes. **Adding resolution later is a
sugar layer that fills in an argument, not a rewrite.**

**4. Constraints are checked at the signature, not at instantiation.** Borrowed
from concepts and independent of any dispatch mechanism. This buys error
locality, which is the thing an agent actually pays for, since a non-local error
is a search inside the edit loop — and it repairs the one axis on which bare
reflection is genuinely worse than typeclasses.

**5. No ambient instances, whatever lands.** If a resolution layer is added,
what it resolves against must be determined by what a module **imports** — no
global instance table, no orphan instances, no ambient scope.

This project's two worst defects were both a definition's meaning depending on
something outside what it could see: a whole-program effect rank, where adding
an unrelated look-alike effect silently changed hashes in untouched modules, and
an exhaustiveness claim asserted over regions never examined. **Instance
resolution is the same shape of hazard, and lexical scoping defuses it** — an
instance dependency becomes an ordinary import edge, which the incremental front
end already tracks.

An earlier draft claimed typeclasses were structurally incompatible with content
addressing. That was too strong. **The incompatibility is with *global* instance
scope specifically**; with mandatory instance imports and elaboration before
hashing, typeclasses and content addressing coexist.

**6. The resolution layer is deferred, with two candidates.**

*(a) Elaboration at concrete call sites.* Where the type is concrete, insert the
canonical derived dictionary. Resolution is local, runs before hashing, and
ambiguity is a compile error rather than a silent pick. Handles application
code; leaves abstract-in-body threading explicit.

*(b) Lexically-scoped typeclasses.* Full dispatch, instances explicitly
imported, elaborated to dictionaries before hashing. Handles both cases, at the
cost of instance resolution interacting with Hindley–Milner and row-polymorphic
effect inference.

**The deciding evidence — how often a type is abstract at the point of dispatch
in a real Ply web stack — was never taken.** The whole web track shipped and no
such number was produced. That is a **decision not taken**, not a decision made,
and it is recorded so a future reader does not mistake the silence for a
negative result. What would settle it is unchanged and is now cheap, because a
real stack exists to count: the sites in `examples/` and the standard library
where a type is abstract inside a function's own body at a point that wants an
encoding. **The honest interim position is that a working service shipped
without a resolution layer, which is weak evidence for (a) or for neither, and
no evidence at all about (b).**

## Investigated and rejected: handlers as the dictionary-passing mechanism

Ply has an implicit-passing mechanism no surveyed language has — the handler
stack — and its atoms are already resource-parameterized, so type-indexed
dispatch through `json.encode[Order]` clauses looks natural.

**The type-level problem is real but solvable.** Resources are ground symbols
and set-based row unification depends on that, so unifying a row with a type
variable in a resource position has no principal solution. Scoped labels — rows
as ordered multisets, as Koka uses for type-parameterized effects — restore
principality, at the cost of rewriting the most delicate code in the checker.

**The semantic problem is not solvable, and it decides this.** Handlers are
dynamically scoped, so which codec a call uses depends on whichever handler is
installed in an enclosing frame, possibly many levels up, invisible at the call
site. The row states that an encoding is *required*; it says nothing about
*which one will be supplied*. That is the coherence problem relocated into
dynamic scope, which is strictly worse than either alternative: with explicit
dictionaries divergence is visible at the call site; with typeclasses coherence
prevents divergence; with handlers **divergence is decided at a distance and
invisible locally**.

Two smaller problems, for completeness: handlers are not first-class values, so
a top-level handler needs one clause per serialized type while `derive` emits
functions; and a genuinely polymorphic requirement cannot be discharged by any
single clause, because a clause body must be a concrete function — **so the
abstract-in-body case, which motivated dispatch, is exactly where this does not
help.**

### The rule this yields, which outlives the decision

**Effects are right when the enclosing context should decide.** Which database,
which clock, which scheduler, which logger — dynamic scoping is the feature, and
substitutability at the handler boundary is the point.

**Effects are wrong when the type should decide, always and everywhere.** How an
`Order` serializes is a property of `Order`. No context should change the answer,
so making it contextual creates a bug surface rather than a capability.

**This is the test for every future "should this be an effect?" question.**

### A representation finding worth keeping

Rows and footprints need not share a representation. Rows are the inference-time
structure; footprints are the solved result, and after inference every atom is
ground. Erasing to a set at that point leaves conflict detection and canonical
ordering untouched, **so the row solver can change — to scoped labels or
anything else — without disturbing scheduling or hashing.**

## The argument that carries the most weight

Not verbosity. An agent writing a dictionary name at a call site costs nothing,
and verbosity was always the weakest argument for dispatch.

**Coherence is the real one.** Typeclasses guarantee one canonical instance per
(class, type). With explicit dictionaries nothing prevents two modules passing
two different codecs for one type: both typecheck, the same type serializes two
ways, and nobody notices until a client breaks. Code generated at volume across
many modules makes accidental inconsistency likelier, not less likely.

**This happened, twice, during derivation's own milestone, and the framing above
under-states it in three ways.** It does not need two modules: a type alias in
*one* module made two spellings of one type into two wire formats, because the
encoding was chosen from the key as *written* and an alias is transparent to the
checker. It does not need six months or anyone writing a second codec: a bug in
how a generated body named its leaf codecs meant a module that happened to
define its own leaf supplied it, with no ambiguity error and with nobody writing
a codec twice — **the divergence was introduced by the *deriver*.** And it is
not only about call sites: both defects are invisible at the call site, which is
the property decision 3 leans on when it argues explicit dictionaries are an
acceptable interim. **That argument is weaker than written: it holds for two
dictionaries a human named, and not for two the same `derive` line produced.**

The fixes were not a resolution layer, and are worth stating because they are
what the interim actually rests on: an **orphan rule**, a **reserved builtin no
module can shadow**, resolving a key through the module's own aliases before the
wire form is chosen, and synthesizing an import binder so **a generated body
never writes a bare name**. Coherence here is bought by making a derived
definition unable to *name* a divergent leaf, not by resolution — which is a
fourth point in the design space that this record did not have when it was
written.

Until a resolution layer lands, the partial answers are convention plus a lint —
one derived codec per type — and a law, which makes divergence *statable* though
it cannot prevent it. **Detection rather than prevention, and worth naming as
weaker.**

## Consequences

Derivation stops codecs drifting from their types, and the web track proceeds
without the dispatch question being settled. Constraint checking gives
signature-local errors without resolution risk. Generic library code stays more
verbose, and abstract-in-body dispatch is threaded by hand.

Worth measuring rather than assuming: **derived codecs inflate definition
counts**, and derivation is exactly the feature that makes those counts jump
while the cache is already the largest thing in a warm run.

## Not in this ADR

User-defined derivers. Conditional instances. Higher-kinded types. Any form of
ambient or global resolution, which decision 5 rules out permanently rather than
deferring. Type-parameterized effects, which the rejected option would have
required and which nothing else currently needs.
