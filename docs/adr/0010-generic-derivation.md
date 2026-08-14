# ADR 0010 — Generic derivation

Status: proposed

## Context

Ply has no typeclasses, traits, interfaces, or `derive`. There is no way to write
one function that works for any type with a known encoding.

For everything built so far this cost nothing — tests and specs are written
against concrete types. A web API cannot avoid it. JSON encoding and decoding,
SQL row mapping, equality, ordering, and hashing all want the same shape: given a
type's structure, produce a function. Without a mechanism, every one of those is
hand-written per type, and the hand-written version drifts from the type the
moment someone adds a field.

This is a type-system decision, not a library gap, and it constrains every
library on the web track. It should be settled before JSON is written, not after.

## Options considered

**(a) Typeclasses.** Haskell-style, with inference-directed instance resolution.
Most expressive: `fn respond<T: ToJson>(t: T) -> Response` works, and generic
code composes. The cost is real — instance resolution interacts with
Hindley-Milner in ways that get subtle fast, and Ply's inference already carries
row-polymorphic effects. Coherence, overlapping instances, and orphan rules are
each their own design argument. This is the largest addition to the type system
since effects.

**(b) Structural interfaces.** Go-style: a type satisfies an interface if it has
the right shape. Ply's records are already structural and its rows are already
row-polymorphic, so this is less foreign than it sounds. But the thing being
abstracted over here is *not* the shape of a value — it is the existence of a
function for a type — and structural typing does not express that naturally.

**(c) Compile-time reflection over normalized definitions.** `derive json for
Order` generates an ordinary Ply function by walking `Order`'s structure at
compile time.

## Decision

**(c), compile-time reflection.** Reasons, in order of weight:

1. **The machinery already exists.** `ply-hash` normalizes every definition into
   a canonical, name-erased, structurally complete form in order to hash it. That
   is exactly the input a derivation needs, and it is already correct and already
   tested.

2. **The output is an ordinary definition.** A derived encoder is a function with
   a hash, a footprint, a type, and a cache entry like any other. It participates
   in content addressing, incremental checking, test selection and specs with no
   new concepts. A typeclass dictionary participates in none of those without
   further design.

3. **It matches the actual problem.** What the web track needs is *derivation* —
   from a type's structure, produce a codec. It does not need ad-hoc polymorphism
   in general. Adding the larger mechanism to get the smaller capability is the
   trade this project has consistently declined.

4. **It composes with what is already here.** A derived function is subject to
   M8 obligations, so `law "decode after encode is identity"` is expressible over
   generated code.

### What it costs, stated plainly

You can derive, but you cannot abstract over "any type that has an encoder."
There is no `fn respond<T: ToJson>`. Instead the function is passed explicitly:

```ply
derive json for Order

fn respond<a>(value: a, encode: (a) -> Json) -> Response = ...

respond(order, order_to_json)
```

That is manual dictionary-passing, and it is more verbose at every call site. The
honest summary is that (c) buys simplicity and integration at the cost of
ergonomics, and that if the verbosity proves intolerable in practice, (a) remains
available later — derived functions are exactly the dictionaries a typeclass
mechanism would need, so this is a step toward it rather than away.

## Design

```ply
derive json  for Order
derive eq    for Order
derive row   for Order          // SQL row mapping
```

- Generates `order_to_json : (Order) -> Json` and `order_from_json : (Json) ->
  Result<Order, DecodeError>`, named by convention from the type and the deriver.
- Derivation is **structural and total**: it walks records, ADT variants, lists
  and primitives. A type containing a function, a cell, or a continuation cannot
  be derived, and that is a compile error naming the offending field rather than
  a partial encoder.
- Generated definitions are hashed from their *generated* form, so a change to
  the type changes the derived function's hash and re-selects its tests, exactly
  as a hand-written one would.
- Derivers are named and fixed in v1: `json`, `eq`, `ord`, `row`. User-defined
  derivers are out of scope — a deriver is a compiler plugin, and the security and
  reproducibility questions that raises deserve their own decision.

## Consequences

JSON and SQL mapping stop drifting from their types, which is the concrete win.
Generic library code becomes more verbose, which is the concrete cost.

The thing to watch is generated-code volume: a derived codec per type across a
large API is a lot of definitions, all of which are hashed, cached, and
type-checked. The incremental front end should absorb it — 10,000 definitions
check in 0.45s from scratch — but it is worth measuring rather than assuming,
because derivation is exactly the feature that makes definition counts jump.

## Not in this ADR

User-defined derivers, typeclasses, and any form of implicit resolution. If (c)
proves insufficient, the follow-on is (a), and this ADR should be revisited
rather than extended.
