# ADR 0001 — Modules

Status: accepted — **implemented**, and **amended by W2 in two places**: `std`
is a reserved first path segment (ADR 0012 §1), and a small set of builtin names
is reserved against the shadowing rule below (ADR 0012 §A5). Both are marked
inline where they apply.
Supersedes: the loader behaviour where `ply test <dir>` concatenated every
`*.ply` file under the directory into one module.
Amended by: `docs/adr/0012-w2-contract.md` §1 and §A5.

**"Resolution order" and "One file, one module" are the two rules the rest of the
language is written against, and W2 cut a hole in each.** Both amendments are
marked inline. The `compare_values` one is load-bearing rather than cosmetic:
**the unrestricted rule below produced a real defect — a module defining
`fn compare` supplied the order of every dictionary derived in it.**

## Context

Until now a project was one flat namespace. Every `.ply` file under the path
given to `ply` was parsed and its items appended to a single `Module`, so a
definition in `a.ply` could call one in `b.ply` with no declaration of intent.
That is fine for a three-file example and untenable past it: there is no way to
say a helper is internal, no way to reuse a name, and no way to read a file and
know what it depends on.

The constraint that shapes every decision below is DESIGN.md §3. A definition's
identity is its normalized structure; a free reference contributes the
*referent's hash*, never its name. A module system must not weaken that. If
adding namespaces made a definition's hash depend on where it lives, the
selection story — a test re-runs iff its hash is absent from the cache — would
degrade from exact to conservative for exactly the edit agents make most often:
moving code around.

## Decision

### One file, one module

A file at `<root>/store/orders.ply` is the module `store.orders`. The name is
derived, never declared: there is no `module` header to drift from the path.
Every directory component and the file stem must be an identifier; anything else
is `INVALID_MODULE_PATH` (E0111), because a module name has to be writable in an
`import`.

The project root is the directory passed to `ply`, or the parent directory of a
single named file.

> **Amended by ADR 0012 §1.** `std` is a **reserved first segment**. A project
> file whose path would derive a module name of `std` or `std.*` is
> `E0113 RESERVED_MODULE_NAME` (`ply_span::codes::RESERVED_MODULE_NAME`),
> reported against the file. `import std.json` names a module whose source ships
> with the compiler — `crates/ply-std/ply/*.ply` — and which therefore has no
> project root at all. This is the one exception to "a
> module name is derived from a path under the root"; reserving the segment is
> what makes it an exception rather than a precedence order.

Source parsed with no root — a snippet handed to `ply_syntax::parse`, an editor
scratch buffer — is the **anonymous module**: the empty dotted name. It cannot
import and cannot be imported, and `ModuleName::qualify` leaves its names bare,
so every existing single-module test keeps its exact present behaviour.

### Imports

```
import store.orders                 // binds the module as `orders`
import store.orders as ord          // binds it as `ord`
import store.orders (place, cancel) // binds `place` and `cancel` unqualified
```

A selective import binds **no** module binder: after
`import store.orders (place)`, the name `orders` is not in scope. Combining `as`
with a name list is rejected; one import declaration does one thing.

Every `import` precedes every item. This is a real restriction and it buys
something concrete: the import table is complete before any body is parsed, so
no pass ever has to look ahead to know what a name could mean.

### Visibility

`pub` on `fn`, `type` or `effect` exports it. Private by default. The
constructors of a `pub type` are public with it — a type you can name but whose
variants you cannot match on is not a useful export, and Ply has no abstract
types yet to make the distinction pay for itself. `test` cannot be `pub`; it has
no name a reference could reach.

### References are qualified with `::`, not `.`

`orders::place(x)`, `store::db.get[users](k)`, `orders::Order` in a type,
`orders::Placed(x)` in a pattern.

This is the one decision here that was not handed down, so it is worth stating
why. With `.`, the reference `orders.place(x)` is *token-identical* to a perform
`db.get(k)` and, without the call, to a field access `record.field`. The parser
would have to know whether `orders` names a module, an effect, or a local
variable in order to build the right node — and for the local-variable case it
would need scope information a recursive-descent parser does not have. The
choice would then be to leave the ambiguity in the AST and have inference,
hashing and evaluation each re-derive the answer. Four implementations, four
chances to disagree, on the question that decides what a reference *means*.

`::` costs one token and makes every reference unambiguous at parse time. As a
consequence:

- a local variable may be named `orders` without hiding the module binder
  `orders`, because `::` is only ever preceded by a module binder;
- a module `db` and an effect `db` coexist: `db.get[users](k)` is the effect,
  `db::get(k)` the module's function;
- module binders live in their own namespace, so they cannot collide with
  values, types or effects at all.

### Resolution order

For a **bare** name, first match wins:

1. **Local binders**, innermost outward — `fn` params, `let`, lambda params,
   match bindings, handler clause params, the `with_cell` binder. Locals win
   over everything. This is existing behaviour and does not change.
2. **Module scope**: the current module's own `fn` / `type` / `effect` /
   constructors, *plus* every selectively-imported name. These form one flat
   table per namespace.
3. **Prelude builtins** (`len`, `map`, `assert`, …). Anything in step 2 shadows
   the prelude, which is also existing behaviour.

> **Amended by ADR 0012 §A5: three builtins are reserved and step 2 cannot
> shadow them.** Redefining `cell_get`, `cell_set` or `compare_values` is
> `E0105 DUPLICATE_DEFINITION` at the definition
> (`crates/ply-core/src/infer.rs`), not a shadowing.
>
> The reasons differ and both are worth stating, because "shadowing is always
> allowed" is the rule a reader will otherwise carry into the next design.
> `cell_get` / `cell_set` are *call forms* rather than ordinary functions: the
> atom they perform names the region of their argument, which no row expressible
> in `Row` can be polymorphic in, so there is no scheme for a module to shadow.
> `compare_values` is reserved for a soundness reason found by audit rather than
> by design — `derive ord` emits a body that calls it, ADR 0001's own rule made
> a module's `fn compare` win, and a module that defined one silently supplied
> the order of every dictionary derived in it: not reflexive, not antisymmetric,
> disagreeing with the order `map_keys` iterates in, while `derivable(ord, T)`
> still reported the type as ordered.
>
> The general shape is the one ADR 0012 §A5 names: **a generated body must not
> write a name the generating module could bind.** `compare` remains shadowable
> as the same operation under a name a module may claim, exactly as `len` is —
> the reservation is on the name the deriver writes, not on the operation.

A collision inside step 2 is an error rather than a silent shadowing:

| collision | code |
| --- | --- |
| two local items | `DUPLICATE_DEFINITION` (E0105) |
| two imports binding the same name | `DUPLICATE_IMPORT` (E0110) |
| an import and a local item | `AMBIGUOUS_IMPORT` (E0108) |

The last is the interesting one, and "local definition wins" was rejected
deliberately. Under that rule, adding a local `place` to a file that already has
`import store.orders (place)` would silently steal every existing call site, and
nothing at the call site would look different. Erroring costs one edit —
qualify it, or `as`-rename the import — and the alternative is a class of bug
that content addressing exists to make impossible.

For a **qualified** name `m::x`, there is no ordering and no shadowing:

1. `m` must be a module binder introduced by an `import` in **this file**, else
   `UNKNOWN_MODULE` (E0106);
2. `x` must exist in `m` — else `UNKNOWN_NAME` (E0101), noting the module — and
   must be `pub` — else `PRIVATE_NAME` (E0107), pointing at the declaration.

A qualified reference is the escape hatch from every collision above, which is
what lets the collisions be errors instead of silent rules.

### Namespaces

Three, as today: **values** (functions and constructors together — an expression
cannot tell them apart), **types**, and **effects**. Module binders are a fourth
namespace reachable only through `::`. A name may be reused across namespaces
exactly as it may today.

**Resource labels are deliberately not namespaced.** `[users]` written in two
different modules names the same resource. It must: two footprints conflict iff
they name a common resource and one of them writes, and that is a claim about
the world, not about a file. Namespacing resource labels would let the scheduler
run two contending tests concurrently.

**Effect names, by contrast, are qualified.** An `EffectAtom`'s `effect` field
holds the program-wide name `store.db`, so two modules that each declare an
`effect db` produce atoms that do not contend.

### The critical invariant: the namespace is metadata over hashes

A cross-module reference normalizes to the referent's `DefHash`, exactly as a
same-module reference does. Nothing about a module survives into a hash:

- module names are erased — they are names, and names are erased;
- `import` declarations are erased — they only decide which definition a name
  denotes, and the hash records the definition, not the name;
- `pub` is erased — visibility changes nothing a definition computes.

Two properties follow. The first is already the project's headline:

1. **Renaming a definition changes no hash anywhere**, now including across
   module boundaries.

The second is new, and is the strongest evidence so far that the
content-addressing model is real rather than an accounting trick:

2. **Moving a definition from one module to another changes no hash anywhere.**

The second does not follow from the first. Renaming touches one identifier;
moving changes which file a definition lives in, which module owns it, which
imports exist in two files, and the qualified name every downstream map is keyed
by — and still, provably, nothing rebuilds and no test re-runs. Both are
required tests, listed below.

A corollary worth stating because it will look like a bug otherwise: two
identical definitions in different modules have the **same** hash, and share one
cache entry. That is correct. They compute the same thing.

### Module cycles are rejected

If module A imports B and B imports A, that is `MODULE_CYCLE` (E0109). The
diagnostic names the cycle in order and points at the import that closes it. A
self-import is the length-one case.

This is a simplification, not a principle. Definition-level mutual recursion
**within** a module still works, through the existing Tarjan SCC path in
`ply-hash`, and is unaffected.

What it would take to lift it, should a real program want to:

- **Hashing needs no change at all.** `ply-hash` builds its graph over resolved
  references and is already module-blind; an SCC that happens to span two files
  is hashed exactly like one that does not.
- **Resolution** would become two passes rather than one: collect every module's
  declarations first, then bind imports. Name resolution never actually needs a
  dependency order — only the current single-pass implementation does.
- **Inference** is the real cost. `check_program` walks modules in dependency
  order so that a callee's scheme exists before a caller is inferred. A cycle has
  no such order, so the definition-level SCC would have to be built globally,
  over every definition in the program, ignoring module boundaries — replacing
  the per-module loop with one global Tarjan pass in `infer`.

The work is bounded and confined to `resolve` and `infer`'s driver. It is
deferred because nothing in the vertical slice needs it, and rejecting cycles
keeps the load order a topological sort, which every consumer benefits from.

## Consequences

### `ply test <dir>` no longer concatenates

Every `*.ply` under the root is its own module. A name in one file is invisible
in another until it is exported and imported. This is a breaking change to every
multi-file project, and it is the point.

`examples/` needs **no changes to work**: `clock.ply`, `ledger.ply` and
`store.ply` are independent — none references a definition in another — so they
become the modules `clock`, `ledger` and `store` and check exactly as before. No
module header is required, and `pub` is optional on a definition nobody imports.

Note that `examples/clock.ply` declares `nondet effect clock` while its module
is also named `clock`. Under `::` that is not a collision, and it is a good
accident: it exercises the module-binder namespace on day one.

To exercise imports at all, one new example is required — see the required work
below.

### Everything downstream is keyed by program-wide name

`CheckOutput`'s maps, `HashOutput`'s `defs` / `deps` / `closure`, and
`Failure::suspects` are all keyed by `store.orders.place` rather than `place`. A
`.` cannot occur in an identifier, so a qualified name can never collide with a
name written in source. Tests are keyed `<module>.<label>`, which is what makes
two identically-labelled tests in different modules distinct.

Note the asymmetry that makes this safe: the *keys* change when a definition
moves, and the *hashes* do not. Keys are metadata. The cache is keyed by hash.

### `ply run` needs a rule for `main`

With one flat namespace there was one `main`. Now: if the path argument is a
file, `main` is looked up in that file's module. If it is a directory, exactly
one module may declare a top-level `main`; if several do, `ply run` reports the
candidates and asks for a specific file. Silently picking the first would depend
on load order.

## Required tests

The first four are the invariant; without them this ADR is a claim rather than a
design.

1. Moving a definition from one module to another changes **no** hash in the
   program — not the moved definition's, not its callers', not any test's.
2. Renaming a definition that is referenced from another module changes no hash.
3. Renaming a module (renaming its file) changes no hash.
4. Adding or removing `pub`, adding, removing or reordering `import`s, and
   `as`-renaming an import all change no hash.
5. Two modules each declaring `effect db` produce atoms that do not conflict,
   and a `[users]` label shared across two modules produces atoms that do.
6. Two structurally identical definitions in different modules hash identically.
7. A module cycle `a -> b -> a` is rejected with `MODULE_CYCLE` naming both
   modules; a self-import is rejected the same way.
8. One fixture per new code: `UNKNOWN_MODULE`, `PRIVATE_NAME`,
   `AMBIGUOUS_IMPORT`, `MODULE_CYCLE`, `DUPLICATE_IMPORT`,
   `INVALID_MODULE_PATH`.
9. A local binder named `orders` does not hide the module binder `orders`.
10. A selectively-imported name and a local definition of that name is
    `AMBIGUOUS_IMPORT`, and qualifying the reference fixes it.
11. `ply test examples/` still selects zero tests after a top-level rename.

## Alternatives considered

**`.` for qualified references.** Rejected above: ambiguous with `perform` and
with field access, and resolvable only with scope information the parser lacks.

**Silent shadowing, local over import.** Rejected: it makes a call site's
meaning change without the call site changing.

**Module declared by a header rather than derived from the path.** Rejected: a
declared name can disagree with the path, which creates a class of error that
deriving it makes unrepresentable.

**Hashing the module path into the definition hash.** This is the obvious way to
keep two same-shaped definitions in different modules distinct, and it is
exactly wrong. It would make moving a definition rebuild it and re-run every
test that reaches it, which is the conservative selection Ply exists to beat.
Identical definitions *should* share a hash and a cache entry.
