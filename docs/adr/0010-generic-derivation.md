# ADR 0010 — Derivation now, dispatch deferred

Status: partially accepted. Decisions 1–5 are settled. Decision 6 names the two
remaining candidates and the evidence that would choose between them. One further
candidate was investigated and rejected; the reasoning is recorded so it does not
get re-proposed.

## Context

Ply has no typeclasses, traits, interfaces, or `derive`. There is no way to write
one function that works for any type with a known encoding.

For M0–M8 this cost nothing — tests and specs are written against concrete types.
A web API cannot avoid it. JSON encoding and decoding, SQL row mapping, equality
and ordering all want the same shape: given a type's structure, produce a
function. Without a mechanism, each is hand-written per type and drifts from the
type the moment someone adds a field.

### Parametric is not ad-hoc

Ply already has full **parametric** polymorphism. What it lacks is **ad-hoc** —
dispatch directed by a type. Much of what gets called "framework" needs only the
first:

| Framework piece | Needs |
| --- | --- |
| Routing, middleware chains | parametric — `(Request) -> Response` is concrete |
| Connection pool `Pool<Conn>` | parametric |
| Config, tracing, shutdown | neither |
| `query<T: FromRow>` | **ad-hoc** |
| `ok<T: ToJson>` | **ad-hoc** |
| Generic cache, job payloads | **ad-hoc** |

The ad-hoc requirement concentrates at the **serialization boundary** — row in,
domain type, JSON out. Narrow as a surface, crossed by every endpoint.

### Where the absence actually hurts

Not at concrete call sites; there an explicit argument costs nothing:

```ply
let users = query("select * from users", [], user_rows)
```

It hurts where a type is still abstract **inside a function's own body**:

```ply
fn cache_and_respond<a>(key: String, compute: () -> a) -> Response {
  let v = compute();
  ok(v)          // `a` is abstract here; there is nothing to resolve against
}
```

Rare in application code, common in framework code.

## The design space

This is not two options. Every design is a point on four axes:

| Axis | Range |
| --- | --- |
| Dictionary form | positional argument · record · module · method table · synthesized |
| Passing | explicit · implicit with search |
| Search scope | global · imports · owner-restricted · none |
| Errors surface at | the signature · the instantiation |

Haskell is *(synthesized, implicit, global, signature)*. Zig comptime is
*(none, n/a, none, instantiation)*. An earlier draft of this ADR chose
*(positional, explicit, none, instantiation)* — and that last coordinate was a
mistake, addressed by decision 4.

### What the survey taught

**Rust: derivation and dispatch are complements, not alternatives.** serde is
`#[derive(Serialize)]` *and* `fn to_string<T: Serialize>`. Derivation generates
the mechanical instances; the trait provides abstraction. The end state is
likely both, which makes deferring dispatch a scheduling decision rather than a
claim that derivation suffices.

**Rust: the orphan rule makes coherence a local property.** You may implement a
trait for a type only if you own one of them — coherence without a global table,
checkable from what a module can see. Closest existing fit to Ply's constraint.

**Scala: unconstrained implicit search is a debugging disaster.** Scala 3
renamed implicits to `given`/`using` and constrained resolution precisely because
the mechanism was too powerful and "implicit not found" was among the worst error
experiences in mainstream languages. If resolution is added here, the dependency
must stay visible in the signature.

**Zig and pre-concepts C++: reflection without constraints fails late.** Errors
surface deep inside instantiation. C++ added concepts to fix exactly this. Bare
reflection therefore does *not* win on error locality, which an earlier draft
claimed.

**C++ concepts: constraint checking and dispatch are separable.** A concept is a
predicate on a type, checked at the call boundary, doing no resolution. Good
errors are available without any dispatch risk.

**OCaml: explicit-first is a legitimate stable end state** — modular implicits
treat explicit modules as ground truth and inference as sugar. But OCaml is also
the natural experiment for "no coherence, explicit naming," and serialization is
where it is most painful.

**Idris: a constraint *is* an implicit argument, and resolution is a search.**
There is no categorical difference between a typeclass and a dictionary, only
whether you write it and how the compiler finds it.

## Decisions

### 1. Build derivation now — it is the substrate under every candidate

```ply
derive json for Order
derive eq   for Order
derive row  for Order
```

Generates ordinary Ply definitions by walking the type's structure at compile
time, using the canonical normalized form `ply-hash` already computes for
hashing. That machinery exists, is tested, and is the right input.

Typeclass *instances* for records and ADTs would be derived rather than
hand-written under any design, so this cannot be stranded by a later choice.

- Structural and total: walks records, ADT variants, lists, maps, primitives. A
  type containing a function, a cell or a continuation cannot be derived, and
  that is a compile error naming the field rather than a partial encoder.
- Generated definitions are hashed from their generated form, so changing a type
  changes its derived function's hash and re-selects its tests.
- Derivers are fixed in v1: `json`, `eq`, `ord`, `row`. User-defined derivers are
  out of scope.

### 2. A dictionary is a record

```ply
type JsonCodec<a> = { encode: (a) -> Json, decode: (Json) -> Result<a, DecodeError> }

derive json for Order        // generates order_json : JsonCodec<Order>
```

Ply already has structural records, so this is free and strictly better than
loose positional arguments: one named parameter instead of several, extensible
without breaking call sites, and a record of functions is exactly what a
typeclass dictionary elaborates to — so it is the *most* forward-compatible form
available.

### 3. Framework signatures take explicit dictionaries

```ply
fn query<a>(sql: String, rows: RowCodec<a>) -> List<a> / {db.read[..]}
```

This is deliberately the **elaborated form of a typeclass constraint**:

```ply
fn query<a: FromRow>(sql: String) -> List<a>          // constraint
fn query<a>(sql: String, rows: RowCodec<a>) -> List<a> // what it compiles to
```

Not competing designs at the signature level — the second is what the first
becomes. Adding resolution later is a sugar layer that fills in an argument, not
a rewrite. Nothing built against this is stranded.

### 4. Constraints are checked at the signature, not at instantiation

Borrowed from concepts, and independent of any dispatch mechanism:

```ply
fn respond<a>(value: a, codec: JsonCodec<a>) -> Response
  where derivable(json, a)
```

This buys error locality — the thing an agent actually pays for, since it fixes
what the error tells it and a non-local error is a search inside the edit loop.
It is a separable, cheap increment, and it repairs the one axis on which bare
reflection is genuinely worse than typeclasses.

### 5. No ambient instances, whatever lands

If a resolution layer is added, what it resolves against must be determined by
what a module **imports** — no global instance table, no orphan instances, no
ambient scope.

This project's two worst defects were both a definition's meaning depending on
something outside what it could see: the whole-program effect rank (M6), where
adding an unrelated look-alike effect silently changed hashes in untouched
modules, and multi-region `exhaustive: true` (M7), which asserted a proof over
regions never examined. Instance resolution is the same shape of hazard, and
lexical scoping defuses it — an instance dependency becomes an ordinary import
edge, which the incremental front end already tracks.

An earlier draft claimed typeclasses were structurally incompatible with content
addressing. That was too strong. The incompatibility is with *global* instance
scope specifically; with mandatory instance imports and elaboration before
hashing, typeclasses and content addressing coexist.

### 6. The resolution layer is deferred, with two candidates

**(a) Elaboration at concrete call sites.** Where the type is concrete at the
call, insert the canonical derived dictionary. `respond(order)` elaborates to
`respond(order, order_json)`. Resolution is local, runs before hashing, and
ambiguity is a compile error rather than a silent pick. Handles application code;
leaves abstract-in-body threading explicit.

**(b) Lexically-scoped typeclasses.** Full dispatch, instances explicitly
imported per decision 5, elaborated to dictionaries before hashing. Handles both
cases, at the cost of instance resolution interacting with Hindley–Milner and
row-polymorphic effect inference.

**The deciding evidence is empirical and does not exist yet**: how often is a type
abstract at the point of dispatch in a real Ply web stack? After W3 and W4 that is
a number rather than a guess. Rust's serde suggests the end state is both
derivation and dispatch, so the live question is *which* resolution mechanism and
*when* — not whether.

## Investigated and rejected: handlers as the dictionary-passing mechanism

Ply has an implicit-passing mechanism no surveyed language has — the handler
stack — and its atoms are already resource-parameterized (`db.read[users]`). So
type-indexed dispatch looks natural:

```ply
fn respond<a>(value: a) -> Response / {json.encode[a]}

handle serve(request) with {
  json.encode[Order](x) -> order_json.encode(x),
  json.encode[User](x)  -> user_json.encode(x),
}
```

**The type-level problem is real but solvable.** Resources are ground symbols
today, and set-based row unification depends on that. Unifying
`{json.encode[a] | ρ₁}` against `{json.encode[Order], json.encode[User] | ρ₂}`
has no principal solution under set semantics. Leijen's **scoped labels** — rows
as ordered multisets where unification matches the first occurrence, as used by
Koka for type-parameterized effects — restores principality. It costs a rewrite
of the row solver, the most delicate code in `ply-core`.

**The semantic problem is not solvable, and it decides this.** Handlers are
dynamically scoped, so which codec `json.encode(order)` uses depends on whichever
handler is installed in an enclosing frame, possibly many levels up, invisible at
the call site. The row states that an encoding for `Order` is *required*; it says
nothing about *which one will be supplied*. That is the coherence problem
relocated into dynamic scope, which is strictly worse than either alternative:

- explicit dictionaries — divergence is visible at the call site
- typeclasses — coherence prevents divergence
- handlers — divergence is decided at a distance and invisible locally

Two smaller problems, recorded for completeness: handlers are not first-class
values, so a top-level handler needs one clause per serialized type and `derive`
emits functions rather than handler clauses; and a genuinely polymorphic
requirement cannot be discharged by any single clause, because a clause body must
be a concrete function — so the abstract-in-body case, which motivated dispatch,
is exactly where this does not help.

### The rule this yields, which outlives the decision

**Effects are right when the enclosing context should decide.** Which database,
which clock, which scheduler, which logger — dynamic scoping is the feature, and
substitutability at the handler boundary is the point.

**Effects are wrong when the type should decide, always and everywhere.** How an
`Order` serializes is a property of `Order`. No context should change the answer,
so making it contextual creates a bug surface rather than a capability.

This is the test for every future "should this be an effect?" question.

### A representation finding worth keeping

Rows and footprints need not share a representation. Rows are the inference-time
structure; footprints are the solved result, and after inference every type
variable is substituted, so every atom is ground. Erasing to a set at that point
leaves `Footprint(BTreeSet<EffectAtom>)`, `conflicts_with`, and canonical
ordering for content addressing untouched.

So the row solver can change — to scoped labels or anything else — without
disturbing scheduling or hashing. Worth knowing if type-parameterized effects are
ever wanted for an unrelated reason.

## The argument that carries the most weight

Not verbosity. An agent writing `order_json` at a call site costs nothing, and
verbosity was always the weakest argument for dispatch.

**Coherence** is the real one. Typeclasses guarantee one canonical instance per
(class, type). With explicit dictionaries, nothing prevents:

```ply
respond(order, order_json)       // module A
respond(order, order_json_v2)    // module B, six months later
```

Both typecheck, the same type serializes two ways, and no one notices until a
client breaks. Code generated at volume across many modules makes accidental
inconsistency likelier, not less likely.

Until a resolution layer lands, the partial answers are convention plus a lint —
one derived codec per type, and a hand-written codec where a derived one exists
is a warning — and M8, which makes divergence *statable* though it cannot prevent
it:

```ply
law "the two order encoders agree"
  forall (o: Order) { order_json.encode(o) == order_json_v2.encode(o) }
```

Detection rather than prevention, and worth naming as weaker.

## Consequences

Derivation stops codecs drifting from their types, and W2 proceeds without the
dispatch question being settled. Constraint checking gives signature-local errors
without resolution risk. Generic library code stays more verbose until a
resolution layer lands, and abstract-in-body dispatch is threaded by hand in the
interim.

Worth measuring rather than assuming: derived codecs inflate definition counts —
500 types is 1,000 extra definitions, all hashed, cached and checked. The
incremental front end should absorb it (10,000 definitions check from scratch in
0.45 s), but derivation is exactly the feature that makes definition counts jump,
and the cache is already the largest thing in a warm run.

## Not in this ADR

User-defined derivers. Conditional instances. Higher-kinded types. Any form of
ambient or global resolution, which decision 5 rules out permanently rather than
deferring. Type-parameterized effects, which the rejected option would have
required and which nothing else currently needs.
