# ADR 0010 — Derivation now, dispatch deferred

Status: partially accepted. Decisions 1–3 are settled; decision 4 names two
candidates and the evidence that would choose between them.

## Context

Ply has no typeclasses, traits, interfaces, or `derive`. There is no way to write
one function that works for any type with a known encoding.

For M0–M8 this cost nothing — tests and specs are written against concrete types.
A web API cannot avoid it. JSON encoding and decoding, SQL row mapping, equality
and ordering all want the same shape: given a type's structure, produce a
function. Without a mechanism, each is hand-written per type and drifts from the
type the moment someone adds a field.

### Two different things get confused here

Ply already has full **parametric** polymorphism. What it lacks is **ad-hoc**
polymorphism — dispatch directed by a type. Much of what gets called "framework"
needs only the first:

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
let users = query("select * from users", [], user_from_row)
```

It hurts where a type is still abstract **inside a function's own body**:

```ply
fn cache_and_respond<a>(key: String, compute: () -> a) -> Response {
  let v = compute();
  ok(v)          // `a` is abstract here; there is nothing to resolve against
}
```

That case is rare in application code and common in framework code, which is why
the answer depends on how much framework this project ends up carrying.

## Decisions

### 1. Build derivation now — it is the substrate under either design

```ply
derive json for Order
derive eq   for Order
derive row  for Order
```

Generates ordinary Ply definitions by walking the type's structure at compile
time, using the canonical normalized form `ply-hash` already computes for
hashing. That machinery exists, is tested, and is exactly the right input.

This decision is independent of the dispatch question. Typeclass *instances* for
records and ADTs should be derived rather than hand-written under any design, so
derivation is not work that a later choice could strand.

- Structural and total: walks records, ADT variants, lists, maps and primitives.
  A type containing a function, a cell or a continuation cannot be derived, and
  that is a compile error naming the field rather than a partial encoder.
- Generated definitions are hashed from their generated form, so changing a type
  changes its derived function's hash and re-selects its tests.
- Derivers are fixed in v1: `json`, `eq`, `ord`, `row`. User-defined derivers are
  out of scope — a deriver is a compiler plugin, with its own reproducibility and
  trust questions.

### 2. Framework signatures take explicit dictionaries

```ply
fn query<a>(sql: String, from_row: (Row) -> a) -> List<a> / {db.read[..]}
```

This is deliberately the **elaborated form of a typeclass constraint**:

```ply
fn query<a: FromRow>(sql: String) -> List<a>              // constraint
fn query<a>(sql: String, from_row: (Row) -> a) -> List<a> // what it compiles to
```

These are not competing designs at the signature level — the second is what the
first becomes. Writing framework signatures in the elaborated form now is
therefore forward-compatible: adding resolution later is a sugar layer that fills
in an argument, not a rewrite. Nothing built against this is stranded.

### 3. No ambient instances, whatever lands

Pinned now, because it is the property that protects everything else. If a
resolution layer is added, what it resolves against must be determined by what a
module **imports** — no global instance table, no orphan instances, no ambient
scope.

This project's two worst defects were both a definition's meaning depending on
something outside what it could see: the whole-program effect rank (M6), where
adding an unrelated look-alike effect silently changed hashes in untouched
modules, and multi-region `exhaustive: true` (M7), which asserted a proof over
regions never examined. Instance resolution is the same shape of hazard, and
lexical scoping is what defuses it — an instance dependency becomes an ordinary
import edge, which the incremental front end already tracks.

An earlier draft of this ADR claimed typeclasses were structurally incompatible
with content addressing and the incremental front end. That was too strong. The
incompatibility is with *global* instance scope specifically. With mandatory
instance imports and elaboration performed before hashing, typeclasses and
content addressing coexist.

### 4. The resolution layer is deferred, with two candidates

**(a) Elaboration at concrete call sites.** Where the type is concrete at the
call, insert the canonical derived function. `respond(order)` elaborates to
`respond(order, order_to_json)`. Resolution is local, runs before hashing, and
ambiguity is a compile error rather than a silent pick. Handles application code;
leaves abstract-in-body threading explicit.

**(b) Lexically-scoped typeclasses.** Full dispatch, instances explicitly
imported per decision 3, elaborated to dictionaries before hashing. Handles both
cases, at the cost of instance resolution interacting with Hindley–Milner and
row-polymorphic effect inference.

**The evidence that decides it** is empirical and does not exist yet: how often is
a type abstract at the point of dispatch in a real Ply web stack? After W3 and W4
that is a number rather than a guess. If framework code is as large a fraction as
expected, (b) is likely right.

## The argument that carries the most weight

Not verbosity. An agent writing `order_to_json` at a call site costs nothing, and
verbosity was always the weakest argument for dispatch.

**Coherence** is the real one. Typeclasses guarantee one canonical instance per
(class, type). With explicit dictionaries, nothing prevents:

```ply
respond(order, order_to_json)       // module A
respond(order, order_to_json_v2)    // module B, six months later
```

Both typecheck, the same type serializes two ways, and no one notices until a
client breaks. Code generated at volume across many modules makes accidental
inconsistency likelier, not less likely, and coherence is a type-system-level
answer to it.

Until a resolution layer lands, the partial answers are convention plus a lint —
one derived codec per type, and a hand-written codec where a derived one exists
is a warning — and M8, which makes divergence *statable* even though it cannot
prevent it:

```ply
law "the two order encoders agree"
  forall (o: Order) { order_to_json(o) == order_to_json_v2(o) }
```

Detection rather than prevention, which is weaker, and worth naming as weaker.

## Consequences

Derivation stops codecs drifting from their types, which is the concrete win, and
W2 proceeds without the dispatch question being settled. Generic library code is
more verbose until a resolution layer lands, and abstract-in-body dispatch must
be threaded by hand in the interim.

Worth measuring rather than assuming: derived codecs inflate definition counts —
500 types is 1,000 extra definitions, all hashed, cached and checked. The
incremental front end should absorb it (10,000 definitions check from scratch in
0.45 s), but derivation is exactly the feature that makes definition counts jump,
and the cache is already the largest thing in a warm run.

## Not in this ADR

User-defined derivers. Conditional instances. Higher-kinded types. Any form of
ambient or global resolution, which decision 3 rules out permanently rather than
deferring.
