# ADR 0001 — Modules

**Accepted, implemented.** A project was one flat namespace; every `.ply` file
under the path given to `ply` was concatenated into one module. That is fine for
three files and untenable past it — no way to say a helper is internal, no way
to reuse a name, no way to read a file and know what it depends on.

The constraint that shapes every decision below: a definition's identity is its
normalized structure, and a free reference contributes the *referent's hash*,
never its name. A module system must not weaken that. If adding namespaces made
a hash depend on where a definition lives, selection would degrade from exact to
conservative for exactly the edit that gets made most — moving code around.

## One file, one module

`<root>/store/orders.ply` is the module `store.orders`. The name is derived,
never declared: there is no `module` header to drift from the path. A declared
name can disagree with the path, which creates a class of error that deriving it
makes unrepresentable.

`std` is a reserved first segment. `import std.json` names a module whose source
ships with the compiler and which therefore has no project root at all — the one
exception to "a module name is derived from a path under the root". Reserving
the segment is what makes it an exception rather than a precedence order.

Source parsed with no root — a snippet, an editor scratch buffer — is the
**anonymous module**: the empty dotted name. It cannot import and cannot be
imported.

## Imports

```
import store.orders                 // binds the module as `orders`
import store.orders as ord
import store.orders (place, cancel) // binds the names unqualified
```

A selective import binds no module binder; combining `as` with a name list is
rejected. One import declaration does one thing.

Every `import` precedes every item. That buys something concrete: the import
table is complete before any body is parsed, so no pass ever has to look ahead
to know what a name could mean.

## References are qualified with `::`, not `.`

`orders::place(x)`, `store::db.get[users](k)`, `orders::Order` in a type.

This is the one decision here that was not handed down. With `.`, the reference
`orders.place(x)` is *token-identical* to a perform `db.get(k)` and, without the
call, to a field access `record.field`. The parser would have to know whether
`orders` names a module, an effect or a local variable to build the right node —
and for the local-variable case it would need scope information a
recursive-descent parser does not have. The alternative is to leave the
ambiguity in the AST and have inference, hashing and evaluation each re-derive
the answer: four implementations, four chances to disagree, on the question that
decides what a reference *means*.

`::` costs one token and makes every reference unambiguous at parse time. As a
consequence a local variable may be named `orders` without hiding the module
binder; a module `db` and an effect `db` coexist; and module binders live in
their own namespace, so they cannot collide with values, types or effects.

## Resolution order

For a bare name, first match wins: **local binders** innermost outward, then
**module scope** (the current module's own items plus every selectively-imported
name), then **prelude builtins**.

Three builtins are reserved against that rule and cannot be shadowed:
`cell_get`, `cell_set` and `compare_values`. The reasons differ and both are
worth having, because "shadowing is always allowed" is the rule a reader would
otherwise carry into the next design. `cell_get`/`cell_set` are *call forms*
rather than ordinary functions — the atom they perform names the region of their
argument, which no expressible row can be polymorphic in, so there is no scheme
for a module to shadow. `compare_values` is reserved for a soundness reason
found by audit: `derive ord` emits a body that calls it, the unrestricted rule
made a module's own `fn compare` win, and a module that defined one silently
supplied the order of every dictionary derived in it — not reflexive, not
antisymmetric, disagreeing with the order `map_keys` iterates in, while the
type still reported as ordered.

The general shape: **a generated body must not write a name the generating
module could bind.** `compare` remains shadowable as the same operation under a
name a module may claim, exactly as `len` is. The reservation is on the name the
deriver writes, not on the operation.

A collision inside module scope is an error rather than a silent shadowing: two
local items, two imports binding one name, or an import and a local item. The
last is the interesting one, and "local definition wins" was rejected
deliberately — under that rule, adding a local `place` to a file that already
has `import store.orders (place)` would silently steal every existing call site,
and nothing at the call site would look different. Erroring costs one edit;
the alternative is a class of bug content addressing exists to make impossible.

A qualified name `m::x` has no ordering and no shadowing: `m` must be a module
binder introduced in *this file*, and `x` must exist in `m` and be `pub`. A
qualified reference is the escape hatch from every collision above, which is
what lets the collisions be errors instead of silent rules.

## Namespaces

Three: **values** (functions and constructors together — an expression cannot
tell them apart), **types**, and **effects**. Module binders are a fourth,
reachable only through `::`.

**Resource labels are deliberately not namespaced.** `[users]` written in two
modules names the same resource. It must: two footprints conflict iff they name
a common resource and one of them writes, and that is a claim about the world,
not about a file. Namespacing labels would let the scheduler run two contending
tests concurrently.

**Effect names, by contrast, are qualified**, so two modules that each declare
an `effect db` produce atoms that do not contend.

## The critical invariant: the namespace is metadata over hashes

A cross-module reference normalizes to the referent's hash, exactly as a
same-module one does. Module names are erased because they are names; `import`
declarations are erased because they only decide which definition a name
denotes; `pub` is erased because visibility changes nothing a definition
computes.

Two properties follow. The first is the project's headline: **renaming a
definition changes no hash anywhere**, now including across module boundaries.
The second is new, and does not follow from the first: **moving a definition
from one module to another changes no hash anywhere.** Renaming touches one
identifier; moving changes which file a definition lives in, which module owns
it, which imports exist in two files, and the qualified name every downstream
map is keyed by — and still nothing rebuilds and no test re-runs.

A corollary worth stating because it will look like a bug otherwise: two
identical definitions in different modules have the **same** hash and share one
cache entry. That is correct. They compute the same thing.

Downstream maps are keyed by program-wide name, so those keys move when a
definition moves while the hashes do not. Keys are metadata; the cache is keyed
by hash.

## Module cycles are rejected

A simplification, not a principle. Definition-level mutual recursion *within* a
module still works. Lifting it would need nothing from hashing — the hash graph
is already module-blind — two resolution passes instead of one, and a global
Tarjan pass in inference instead of a per-module loop, since inference currently
walks modules in dependency order so a callee's scheme exists before a caller is
inferred. Bounded work, deferred because nothing needs it and because rejecting
cycles keeps the load order a topological sort.

## Alternatives rejected

**`.` for qualified references.** Ambiguous with `perform` and with field
access, resolvable only with scope information the parser lacks.

**Silent shadowing, local over import.** It makes a call site's meaning change
without the call site changing.

**A module declared by a header rather than derived from the path.**

**Hashing the module path into the definition hash.** The obvious way to keep
two same-shaped definitions distinct, and exactly wrong: it would make moving a
definition rebuild it and re-run every test that reaches it, which is the
conservative selection this system exists to beat. Identical definitions
*should* share a hash and a cache entry.
