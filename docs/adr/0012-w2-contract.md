# ADR 0012 — The W2 implementation contract

Status: accepted

ADR 0010 settled *what* derivation is. This one settles *how*, and settles the
four things around it that W1 proved a payload milestone cannot ship without: a
way to import a module that ships with the compiler, an ordered `Map`, two
numeric types, and byte builtins that fix an algorithm rather than a constant
factor. Where 0010 and this disagree, this wins — it was written after it,
against the code.

## The rule everything else follows from

> Every new thing W2 adds is a **value or a definition like any other**. A
> stdlib definition hashes like a project one, a derived definition hashes like
> a hand-written one, and a `Map` is a value whose canonical form is a function
> of its contents. Nothing here gets a private channel into a cache key, a
> hash, or an iteration order.

Three corollaries, each of which decides a section below:

1. **Nothing outside a definition's own reachable graph may enter its hash.**
   Not the `std` prefix, not a stdlib version, not the module a `derive` was
   written in. This is ADR 0001's rule, and W2 is where it is easiest to break
   by accident (§1, §3).
2. **Any order a program can observe must be a function of the values, not of
   history.** Iteration order, JSON object order, `map_keys`. A hash-ordered map
   would silently break content addressing, test caching, simulation replay and
   `--engine both` at once, and every one of those failures is a green result
   (§2).
3. **A verdict may only get stronger when the evidence does.** The prover's
   fragment is over `Int`; `Float` and `Decimal` do not join it by arriving
   (§4).

---

## 1. The stdlib path

### The problem W1 left

`examples/hello.ply` declares `nondet effect net` inline, and the declaration
`ply_host::tcp` registers against sits in `crates/ply-host/ply/net.ply` — a
directory that cannot be a module name and is not under any project root. There
is no mechanism by which a program imports a module that ships with the
compiler. JSON is a library and hits the same wall, so W2 must open it.

### `std` is a reserved root

`import std.json` names a module whose source ships with the compiler. `std` is
a **reserved first segment**: a project file whose path would derive a module
name of `std` or `std.*` is `E0113 RESERVED_MODULE_NAME`, reported against the
file with the instruction to rename it.

Reserving it is what makes the rest simple. Because no project module can ever
be named `std.*`, there is no precedence order between the project and the
stdlib, no shadowing rule, no flag to pick one, and no way for what
`import std.json` means to depend on where a file happens to sit. The
alternative — searching the project first and the stdlib second — makes adding a
file change the meaning of an unrelated module's imports, which is the exact
shape of hazard ADR 0010 §5 rules out permanently.

### Where the sources live, and how they are found

`crates/ply-std/ply/*.ply`, embedded into the binary at compile time by an
explicit `include_str!` table in one file. Not a directory scan, and not a path
resolved at run time relative to the executable.

- A run-time path makes a program's meaning depend on the installation layout,
  and content addressing cannot pay that: two machines with different
  `/usr/lib/ply` would compute different hashes for one source tree and swap
  cache entries that mean different things.
- An explicit table is a list you can read top to bottom, which is the same
  property that makes `ply-host`'s registry a reviewable trusted computing base.
  `ply std` prints it.

There is **no `--std-path`, and there will not be one.** It would make `ply
check` answer differently depending on a command-line flag, which is ADR 0011
corollary 1 and is settled.

### Loading is demand-driven

After the project's files are parsed, the loader walks the import graph: an
`import std.x` that no project module satisfies pulls `std.x` out of the
embedded table, parses it, and repeats transitively. A `std.x` the table does
not hold is `E0106 UNKNOWN_MODULE`, listing the modules that exist.

Consequences, all of them wanted:

- a program importing nothing from `std` loads nothing, checks nothing extra,
  and has hashes byte-identical to what it has today;
- a stdlib module appears in `CheckOutput`, `HashOutput` and the front-end cache
  exactly as a project module does, under its program-wide name
  (`std.json.parse`), because it *is* one;
- a stdlib module may import only `std.*`. A shipped module importing anything
  else is `E0505 INTERNAL_ERROR` — the user cannot have caused it and cannot fix
  it, and calling it their error would send them looking in their own tree.

`Loaded::entry_points` excludes `std` modules: `main` is a project's, and a
stdlib module that declared one would make `ply run` ambiguous in a directory
the user did not write.

**Tests declared in a `std` module are not selected by a project's run**, and
are not written to a project's cache. They are run by the compiler's own suite,
which loads the whole embedded set as one program and checks and runs it — so a
shipped stdlib is checked in CI rather than in every user's `ply test` output.
`ply test --std` includes them, for someone debugging the stdlib itself. Without
this rule a project's test count changes with a compiler upgrade, for tests the
project did not write and cannot fix.

### What this does to content addressing

A stdlib definition normalizes **exactly** as any other: names erased, locals as
de Bruijn levels, a free reference contributing the referent's hash. No `std`
marker, no stdlib version, nothing about the module enters the bytes.

Two consequences follow, and both are required tests:

1. Copying `std/json.ply`'s source into a project as `json.ply` produces
   definitions with the **same hashes** as the stdlib's, sharing its cache
   entries. That is correct, and it is the same sentence as "moving a definition
   between modules changes no hash."
2. A compiler upgrade that does not change a stdlib source file changes no hash
   and re-runs no test, even though every byte of the compiler moved.

### Upgrading the compiler, under a warm cache

The stdlib is source, so a stdlib edit moves exactly the hashes it should:
the changed definitions', their transitive dependents', and the tests reaching
them. Selection stays exact. Nothing about correctness needs a version number,
and **the stdlib digest is deliberately not in any cache key** — a digest in the
key would invalidate a project on an edit to a `std` module it never imports,
which is precisely the conservative selection Ply exists to beat.

What an upgrade does need is to not be a mystery. So:

- `ply-std` computes a **stdlib digest**: BLAKE3 over the canonical list of
  `(module name, ContentHash of source bytes)` pairs, rendered `b3:` plus twelve
  hex characters. `ply std` lists the modules with their definition counts and
  the digest; `ply std --digest` prints the digest alone, which is what a CI
  check pins, exactly as `ply hosts --digest` is.
- The store records the digest it was last written under. When a run opens a
  cache written under a different one, it warns once with `W0605
  STDLIB_CHANGED`, naming both digests and **the number of definitions this
  program reaches whose hash moved** — which is often zero, and the warning says
  so rather than implying work happened. Correctness came free; this is
  visibility.

The two gates need no new mechanism, provided one thing is done right:

- **Gate 1** keys on a file's raw content hash. An embedded module has no file,
  so its `SourceFingerprint::content_hash` is the hash of the **embedded source
  bytes**, and its store key is the pseudo-path `<std>/json.ply`. A compiler
  upgrade that changes the source changes the content hash and refuses the skip;
  one that does not, does not. `<` is not an identifier character, so no
  discovered file can produce that key — and a project that contained a
  directory literally named `<std>` would be `E0111 INVALID_MODULE_PATH` before
  anything was cached.
- **Gate 2** is unchanged: a `DefHash` present in the store is not rechecked,
  whoever wrote the definition.
- `prune`'s `keep` list gains the pseudo-paths of the std modules the run
  loaded. A std module a project stopped importing is pruned, which is correct
  and recomputable.

`RUNTIME_VERSION` covers the other half: a stdlib module that is a thin wrapper
over a builtin depends on the evaluator, and the evaluator is what
`RUNTIME_VERSION` is for. Nothing new is needed there either.

### The `net` effect moves

`crates/ply-host/ply/net.ply` becomes `crates/ply-std/ply/net.ply`, the module
`std.net`, holding the effect declaration and `drain`. Its demo functions
(`greeting`, `serve_once`) move to `examples/`, where they belong.

Effect names are qualified (ADR 0001), so the effect's program-wide name becomes
`std.net.net` and `ply_host::tcp`'s registration must name it. `ply hosts` prints
the qualified name. A program that declares its own `nondet effect net` still
gets `E0421` naming the nearest declared operation — which is now the right
answer for the right reason, because the fix is `import std.net` rather than a
copied declaration that will drift.

---

## 2. `Map`

### Representation

```rust
pub enum Value { /* ... */ Map(RedBlackTreeMap<Value, Value>) }
```

`rpds::RedBlackTreeMap`, the structure `World` already uses, so this adds no
dependency. Persistent with structural sharing: `map_insert` is O(log n) and
allocates O(log n) nodes rather than copying the map, and a `Value::Map` is
cloned by a refcount bump. It uses the same `RcK` shared-pointer kind `World`
does, so a `Value` stays thread-confined exactly as it is today. That is a
strictly better cost model than `Value::List`'s whole-vector sharing, and it is
what "at request volume" in the roadmap asks for.

This requires `Ord for Value`, hand-written, **structural, total and
deterministic**:

- variants compare by a pinned discriminant first, so a `Bytes` is never equal
  to a `Str` and the comparison never depends on what is inside a variant it is
  not comparing;
- `Int`, `Bool`, `Str`, `Bytes`, `Unit` compare by value; `Decimal` compares by
  **numeric value**, so `1.50m` and `1.5m` are one key; `List` lexicographically;
  `Record` field-name-ascending, which is how `BTreeMap` already holds it; `Ctor`
  by variant name then fields left to right;
- `Float` compares by `f64::total_cmp`, which is total and deterministic;
- `Closure`, `Cell`, `Task` and `Continuation` compare by discriminant alone —
  every closure equal to every closure. Not a panic and not a pointer
  comparison: a panic is banned on a reachable path and a pointer is not
  deterministic. These cases are unreachable from a well-typed program, because
  the type system refuses them as key types, and the definition is there so that
  a defect elsewhere produces a wrong answer we can reproduce rather than an
  abort or a different answer per run.

**Required test**: over every value the M8 generator produces,
`Value::cmp(a, b) == Ordering::Equal` iff `values_equal(a, b)`, with one
documented exception — two `Float` NaNs of one bit pattern, where `values_equal`
is false and `cmp` is `Equal`. `values_equal` is not rewritten in terms of
`cmp`; the two are checked
against each other, which is what catches a divergence rather than hiding it.

### Iteration order is the property that matters

**`map_keys` is ascending by the order above, always, everywhere.** Not
insertion order, not hash order, not "unspecified".

This is not a nicety. A hash-ordered map would make `map_keys` a function of a
hasher's seed and of insertion history, and four separate guarantees rest on a
value having one canonical form:

- a derived JSON encoding of a record containing a `Map` would differ run to
  run, so `decode(encode(x)) == x` would hold and `encode(x) == encode(x)` would
  not;
- `assert_eq` over two maps built in different orders would compare unequal, so
  a **passing test would be cached** under one order and re-run red under
  another;
- a simulated replay of a seed would take a different branch on a `map_keys`
  fold, breaking `E0415`'s guarantee that a seed replays exactly;
- `--engine both` would report `E0503` on a program that is entirely correct.

Every one of those is a green result over unexplored space or a red result over
correct code, which is the failure class this project audits for. Ordered
iteration is the whole reason `Map` is a language primitive rather than a
library.

> **Corrected (regression audit, 2026-08-21). Ordered iteration was necessary
> and was not sufficient, and three of the four failures above were reachable
> for four milestones.** `Value::cmp` compares a `Decimal` by numeric value so
> that `1.50m` and `1.5m` are one key, while `Value::write` and
> `decimal_to_string` print the scale as stored — so a map held whichever of the
> two spellings was inserted last, and `map_keys`, a derived JSON encoding and a
> `map_fold`'s branch were functions of insertion history through the **key**
> rather than through the order. Two maps that `assert_eq` as one value served
> two different response bodies. `--engine both` reported nothing, because it
> was not an engine disagreement.
>
> Fixed in the representation: `ply_eval::value::canonical_key` reduces a key to
> one representative per equivalence class on the way in, at every position
> `Value::cmp` descends into. `docs/adr/0019-value-representation.md` §7 is the
> write-up, and `crates/ply-eval/tests/suite/map_order.rs::an_equal_key_replaces_the_value_and_the_key_is_canonical_either_way`
> carries what the tree asserted until then.

### What it requires of key types

A key type must be **ordered**, and "ordered" is exactly `derivable(ord, k)` —
the same predicate §3's `derive ord` walks, one implementation, no second
definition to drift.

| ordered | not ordered |
| --- | --- |
| `Int`, `Bool`, `String`, `Bytes`, `Unit`, `Decimal` | `Float` |
| `List<k>` where `k` is | any function type |
| a record whose every field type is | `Cell<r, a>`, `Task<a>` |
| an ADT whose every field type is | anything containing one of the above |
| `Map<k, v>` where both are | |

`Float` is excluded because NaN makes `<` non-total: with a `Float` key,
`map_insert(m, nan, v)` has no well-defined position, and a total order that
disagrees with `==` on its own keys is a lookup that fails to find what it just
inserted. `total_cmp` makes the *Rust* comparison total; it does not make the
*language's* `==` an equivalence relation, and the map's contract is stated in
the language's terms.

`Map<Float, v>` is `E0206 NOT_DERIVABLE`, naming `Float` and saying the key type
must be ordered.

For a **type parameter**, well-formedness is checked at the signature, not at
the use:

```ply
fn index_by<k, v>(xs: List<v>, key: (v) -> k) -> Map<k, v>
  where derivable(ord, k)
= ...
```

Omitting the `where` is `E0206` against the signature, with the clause to add
spelled out in the diagnostic. This is §3's mechanism doing load-bearing work
rather than decorating: the error is at the boundary, and the body may then
assume the constraint.

### Operations

All pure — an empty row — except `map_fold`, which threads its function's row.
`k` carries `derivable(ord, k)` throughout.

| builtin | type | notes |
| --- | --- | --- |
| `map_new` | `() -> Map<k, v>` | Ply has no top-level constants, so the empty map is a call |
| `map_insert` | `(Map<k, v>, k, v) -> Map<k, v>` | replaces an equal key's entry **key and value both** |
| `map_get` | `(Map<k, v>, k) -> Option<v>` | |
| `map_contains` | `(Map<k, v>, k) -> Bool` | |
| `map_remove` | `(Map<k, v>, k) -> Map<k, v>` | absent key is a no-op, not an error |
| `map_len` | `(Map<k, v>) -> Int` | |
| `map_keys` | `(Map<k, v>) -> List<k>` | ascending |
| `map_values` | `(Map<k, v>) -> List<v>` | in ascending key order |
| `map_entries` | `(Map<k, v>) -> List<{key: k, value: v}>` | ascending; Ply has no tuples |
| `map_of_entries` | `(List<{key: k, value: v}>) -> Map<k, v>` | later entries win, matching a fold of `map_insert` |
| `map_merge` | `(Map<k, v>, Map<k, v>) -> Map<k, v>` | the right side wins a shared key |
| `map_fold` | `(Map<k, v>, b, (b, k, v) -> b / e) -> b / e` | ascending key order |

One rule, stated once: **the last write wins for the key as well as the value.**
It only becomes visible where two equal keys are distinguishable, which is
`Decimal` — inserting `1.5m` into a map holding `1.50m` leaves the key `1.5m`,
and `map_keys` then renders `1.5` where it rendered `1.50`. Deterministic, and
the alternative (keep the key already present) costs a lookup on every insert to
preserve a distinction nobody asked for.

### The rest of the surface

- `values_equal` over maps compares length and then entries in key order.
- `Value::render` prints `{k: v, ...}` in key order, truncating past 32 entries
  as a list does. `Value::type_name` is `"Map"`.
- **Quantifiable in a `forall`**: 0..=8 entries drawn from the key and value
  generators, shrinking entries out before shrinking values, `map_new()` minimal.
  Leaving it ungeneratable would regress M8's guarantee on contact with a new
  primitive — the same argument, and the same required test, as `Bytes` in W1.
- The prover treats maps as opaque: there is no theory of arrays here, so
  `map_get(map_insert(m, k, v), k) == Some(v)` is `property`, not `proved`.
- **No map literal syntax in W2.** A literal is sugar over `map_of_entries`, it
  would put a new node in `Lit` and the normalizer for no semantic gain, and
  adding it later moves only the hashes of definitions rewritten to use it.

---

## 3. Derivation

Per ADR 0010 decisions 1–4, made precise.

### Surface

```
item       := "pub"? (fnDef | typeDef | effectDef) | testDef | lawDef | deriveDef
deriveDef  := "derive" IDENT "for" IDENT
fnDef      := "fn" IDENT generics? "(" params ")" ("->" type)? ("/" row)?
              whereClause? specClause* ("=" expr | block)
whereClause:= "where" constraint ("," constraint)*
constraint := "derivable" "(" IDENT "," IDENT ")"
```

`where` sits between the effect row and any `requires`, and is unambiguous
against a `law`'s `where` guard because the two cannot occur in one item.

The derivers are `json`, `eq` and `ord`, fixed; anything else is `E0207
UNKNOWN_DERIVER` listing them. **`row` is named in ADR 0010 and lands in W4**,
with the `Row` type it is a codec over — a `RowCodec<a>` in W2 would be a
placeholder, and a placeholder in the contract is worse than an absence.

`derive` carries no `pub`. **A generated definition takes its target type's
visibility**, so a type you can name from another module is a type you can
encode, and the two cannot drift.

**The orphan rule**: a `derive` may only name a type its own module declares.
Otherwise `E0208 ORPHAN_DERIVE`. This is the cheapest coherence available and it
costs nothing to have now: without it two modules each deriving for one type
produce two *names* for one canonical encoding — which is exactly the divergence
ADR 0010 has no resolution layer to prevent, and which its "one derived codec per
type" lint would have nothing to stand on. With it, coherence is a local property
checkable from what a module can see, which is Rust's orphan rule and the
closest existing fit to Ply's constraint.

A second `derive json for Order` in one module is `E0105 DUPLICATE_DEFINITION`,
because it generates a definition under a name already taken.

### A dictionary is a record, and a derivation is a function

```ply
type JsonCodec<a> = { encode: (a) -> Json, decode: (Json) -> Result<a, DecodeError> }
type EqDict<a>    = { eq: (a, a) -> Bool }
type OrdDict<a>   = { compare: (a, a) -> Ordering }
```

ADR 0010 writes `order_json : JsonCodec<Order>`. Ply has **no top-level value
definitions** — the grammar has `fn`, `type`, `effect`, `test` and `law`, and
nothing else — so a derivation generates a *function*:

```ply
fn order_json() -> JsonCodec<Order>
fn pair_json<x, y>(x: JsonCodec<x>, y: JsonCodec<y>) -> JsonCodec<Pair<x, y>>
```

Call sites write `respond(order, order_json())`. The cost is rebuilding a
two-field record of closures per call, which is a few allocations against a parse
— and the alternative is a new kind of top-level definition with initialization
order and purity questions, in the milestone that can least afford one. A nullary
constant form, if it ever lands, changes the generated form and therefore
re-selects the tests that reach it; that is an honest edit, not a free one.

A type with parameters generates one dictionary parameter per type parameter, in
declaration order, and the corresponding `where derivable(D, ·)` constraints.

### Naming

`snake_case(TypeName) ++ "_" ++ deriver`. Snake case is: insert `_` before an
uppercase letter that follows a lowercase letter or a digit, and before the last
uppercase of a run that is followed by a lowercase; then lowercase everything.
`Order` → `order_json`; `OrderLine` → `order_line_json`; `HTTPRequest` →
`http_request_json`.

The rule is total, so it can collide: `HTTPRequest` and `HttpRequest` in one
module both yield `http_request_json`. That is `E0105 DUPLICATE_DEFINITION`,
pointing at both `derive` lines and saying to rename one of the types. Loud, and
the alternative — a disambiguating suffix — is a name nobody can predict.

### Structural and total

The deriver walks the target type's declaration.

- **Leaves**: `Int`, `Bool`, `String`, `Bytes`, `Unit`, `Float`, `Decimal`.
- **Structural**: records, `List<a>`, `Map<k, v>`, ADT variants, `Option<a>`,
  `Result<a, e>`, and a type alias, through its body.
- **Refused**: any function type, `Cell<r, a>`, `Task<a>`, a continuation.
  `E0206 NOT_DERIVABLE`, naming **the field**:

```
error[E0206]: `json` cannot be derived for `Order`
  ┌─ src/orders.ply:12:3
12│   on_complete: (Order) -> Unit,
  │   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ a function has no JSON encoding
  = `derive json for Order` requires every field to be derivable
  = remove the field from `Order`, or write the codec by hand
```

`ord` additionally refuses `Float`, for §4's reason.

**Derivation composes through named types, never by inlining them.** A field of
type `users::User` generates a call to `users::user_json()`, not a copy of
`User`'s structure. So `order_json`'s body depends on `user_json`'s *hash*, which
is what makes a change to `User` re-select exactly the tests that reach an
`Order` — and it keeps each type's codec one definition rather than a blob that
grows with the graph.

A reference a generated body emits that fails to resolve is reported as `E0206`
against the `derive` item, naming the field and the missing `derive`, rather than
as a bare `E0101 UNKNOWN_NAME` pointing at generated source the user never wrote.

### Where expansion happens

**Immediately after parse, before resolution and before inference.** Expansion is
purely syntactic: it reads the module's own type declarations — which the orphan
rule guarantees is where the target is — and emits references to other types'
codecs by name, leaving resolution to check them.

Generated `FnDef`s are **appended to `Module::items`** after every source item,
in the order of their `derive` declarations. One list, not two: a second list is
a thing every walker can forget, and forgetting it drops a definition silently.
Appending leaves the index of every `test` and `law` untouched, which
`HashOutput::tests` is parallel to. The `Item::Derive` stays in `items` as the
declaration and contributes no definition of its own, so every consumer that
enumerates definitions is right to skip it.

Each generated definition carries `FnDef::derived: Some(Derived { deriver,
target })`. Provenance for `--explain` and `ply check --types` only, and **erased
by normalization** — a hand-written definition byte-identical to a generated one
is the same computation and must share its hash.

**A generated definition that fails to typecheck is `E0505 INTERNAL_ERROR`,
Ply's fault.** Derivation is total and structural, so if generation succeeded,
checking must succeed; the user did not write the body and cannot fix it. This
makes the deriver's correctness checked on every run rather than in a test suite.

### Incremental checking and the cache

A generated definition is an ordinary definition, so it needs no special case —
but four specific behaviours follow, and each is a required test.

- Its `DefHash` is over its **generated form**. Its `DefEntry::span` is the
  `derive` item's `FileSpan`, and every span inside its body is that span too, so
  an error inside it points at the line the user can edit.
- **Renaming the type re-runs no test.** `Order` → `Purchase` changes the
  generated definition's *name* (`purchase_json`) and not its body: field names
  and constructor names are what the encoding contains, and neither moved. This
  is the headline invariant, now covering derivation.
- **Renaming a variant re-runs its tests.** `Placed` → `Created` changes the JSON
  tag, so the generated body changes, so its hash changes. That is an observable
  protocol change and re-selecting is correct. The pair with the previous point
  is the sharpest available demonstration that the hash tracks meaning.
- Adding, removing or reordering a field changes the generated form. Reordering
  counts: JSON object order is observable.

One hazard has to be closed explicitly. Gate 1 keys on **raw file content**, so a
compiler upgrade that changes what the deriver emits would let a file be skipped
and a stale generated definition be reused. So: **any change to a deriver bumps
`FRONTEND_VERSION`**, added to the list beside normalization, inference, `Scheme`,
`Footprint` and the prelude's signatures — and a golden pin test renders the
generated form of a fixture type, fails when it moves, and says to bump. The
front-end cache is recomputable, so the cost of the bump is bounded; the cost of
missing it is a wrong type.

### Constraints are checked at the signature

```ply
fn respond<a>(value: a, codec: JsonCodec<a>) -> Response
  where derivable(json, a)
```

- At a call site instantiating `a` with a concrete `T`, `derivable(json, T)` is
  checked. Failure is `E0206` **at the call site**, naming `T` and the field that
  blocks it, with the signature's `where` clause as a secondary label.
- Inside the body the constraint is *assumed*, so a nested call requiring it is
  satisfied. There is no resolution and no search: this is a predicate on a type,
  checked at a boundary, which is C++'s concepts and nothing more.
- A constraint naming a parameter the signature does not bind is `E0102
  UNKNOWN_TYPE`. A constraint on a concrete type is `E0206` or is redundant, and
  the grammar admits only an identifier so it cannot be written.

**Constraints are kept by normalization**, unlike specs, and the reason is
soundness rather than taste. Adding a `where` narrows the call sites a signature
admits. Gate 2 rechecks a definition only when its own hash moved, and a caller's
hash moves only when a callee's does. If a constraint were erased, adding one
would move no hash, so a caller already checked against the unconstrained
signature would never be rechecked — and would stay accepted against a signature
that no longer admits it. That is the same reason declared type and effect
annotations are in the hash.

The encoding is `tag::CONSTRAINT` (95) plus the parameter's **de Bruijn level**
and the deriver's pinned tag, **sorted and deduplicated**, so renaming a type
parameter and reordering two constraints both change no hash. `BODY_ENCODING`
bumps.

---

## 4. Numeric types

### Which, and why

**`Float`** is IEEE-754 binary64.

**`Decimal`** is `rust_decimal::Decimal` 1.42.1: sign, a 96-bit unsigned
mantissa, and a scale of `0..=28`. Chosen over `bigdecimal` because it is
**bounded**. That sounds like the weaker property and is the deciding one here: an
arbitrary-precision decimal has a size that depends on the operations performed
on it, so a value inside a deterministic replay could grow without bound, and a
value that enters a hash and a cache key needs a finite, canonical, allocation-free
form. Money needs twenty-eight significant digits, not infinity.

`ryu` and `itoa` were considered and rejected: Rust's own `f64` and `i64`
formatting already produce the shortest round-tripping representation, so both
buy speed in a place W2 has not measured and does not suspect. The measured
problem is §5's pass count.

### Literals, and how they hash

`1.5` is a `Float`. `1.50m` is a `Decimal` — the `m` suffix, unambiguous and
borrowed from C#.

```rust
pub enum Lit { /* ... */ Float(f64), Decimal { mantissa: i128, scale: u32 } }
```

`Decimal` is carried as mantissa-and-scale rather than as a `rust_decimal::Decimal`
so that `ply-syntax` and `ply-hash` take no numeric dependency; `ply-eval`
converts. A literal whose mantissa exceeds 96 bits or whose scale exceeds 28 is
`E0001 UNEXPECTED_TOKEN` naming the limit.

- Normalization tags `LIT_FLOAT = 45` (the IEEE **bit pattern**, eight bytes) and
  `LIT_DECIMAL = 46` (mantissa then scale). Distinct from `LIT_INT`, so `1` and
  `1.0` and `1m` are three definitions — they have three types and must not share
  a hash.
- **The literal's scale is preserved**: `1.50m` has scale 2 and renders `1.50`.
  So `1.50m` and `1.5m` are equal in value, differently hashed, and one map key.
  All three of those are consequences of the same decision and all three are
  stated rather than smoothed over.
- `0.0` and `-0.0` are **different definitions**, because their bit patterns
  differ. A normalizer that folded them would make two textually distinct
  programs one definition while `1.0 / -0.0` still distinguishes them.

### Float at the edges

- IEEE semantics, unmodified: `1.0 / 0.0` is `Infinity`, `0.0 / 0.0` is `NaN`,
  and neither is a `RUNTIME_ERROR`. Refusing them would make `Float` a worse
  `Decimal` rather than a different type.
- `==` is IEEE `==`: `NaN != NaN`. This is the source of every restriction below.
- Rendering is shortest-round-tripping, **always with a `.0` or an exponent**, so
  a `Float` never prints as an `Int`. `Infinity`, `-Infinity` and `NaN` print as
  those words.
- JSON has no non-finite literals, so **encoding a non-finite `Float` is a
  `RUNTIME_ERROR`** naming the value. Substituting `null` is silent data loss.
- Not an ordered key type; not derivable for `ord`.

### Decimal at the edges

- `+`, `-` are **exact**, or `E0502 RUNTIME_ERROR` on mantissa overflow. Never a
  silent wrap and never a silent rounding: a total that quietly lost a cent is
  the failure this type exists to prevent.
- `*` is exact when the result's scale is at most 28, and otherwise rounded
  **half-to-even** at scale 28; mantissa overflow is `RUNTIME_ERROR`.
- `%` is exact, and is therefore allowed where `/` is not: the *remainder* of a
  decimal division is a decimal even when the quotient is not. A zero divisor
  is `RUNTIME_ERROR`, as for `Int`.
- **`/` is refused**, with `E0209 DECIMAL_DIVISION`:

```
error[E0209]: `/` is not defined on `Decimal`
  ┌─ src/billing.ply:14:20
14│   let unit = total / count;
  │                    ^ the exact quotient of two decimals is not a decimal
  = an operator would have to round, and a rounding nobody wrote down is the
    defect `Decimal` exists to prevent
  = call `decimal_div(total, count, 2, HalfEven)` and say how to round
```

  This is the one place W2 refuses something every other language allows, and it
  is deliberate: a silently-rounded division is the single most common money bug,
  and the operator is where it hides.

- `decimal_div(a, b, scale: Int, rounding: Rounding) -> Decimal`, and
  `decimal_round(d, scale: Int, rounding: Rounding) -> Decimal`. A scale outside
  `0..=28` is `RUNTIME_ERROR`.
- `decimal_of_int(Int) -> Decimal` total; `int_of_decimal(Decimal, Rounding) ->
  Option<Int>`, `None` outside `i64`.
- `float_of_decimal(Decimal) -> Float` total and lossy; `decimal_of_float(Float)
  -> Option<Decimal>`, `None` for NaN, infinity and out of range, and otherwise
  the **shortest decimal that round-trips the float** — the only defensible
  choice, since any other is an arbitrary number of digits of a binary
  approximation.
- `decimal_of_string(String) -> Option<Decimal>` and `decimal_to_string(Decimal)
  -> String`, which round-trips exactly and preserves scale.

### The prelude gains four ADTs

`map_get` returns an `Option`, `decode` returns a `Result`, `compare` returns an
`Ordering` and `decimal_div` takes a `Rounding`. A **builtin** whose type
mentions a type the user must import is incoherent, so these are prelude types
with prelude constructors, in scope everywhere:

```ply
type Option<a>    = None | Some(a)
type Result<a, e> = Ok(a) | Err(e)
type Ordering     = Less | Equal | Greater
type Rounding     = HalfEven | HalfUp | Down | Up | Ceiling | Floor
```

They join `BUILTIN_TYPES`, so a user `type Option<a>` becomes `E0105
DUPLICATE_DEFINITION`. That is a **breaking change to existing code**:
`examples/ledger.ply` declares `pub type Option<a> = None | Some(a)`, and so do
fixtures in `ply-syntax` and `ply-cli`. Deleting those declarations and using the
prelude's is part of the numerics work, not a follow-up — a language with two
`Option`s is worse than one with none.

A prelude constructor normalizes as a free reference by name, exactly as `len`
does, so a change to the prelude's shape is a `FRONTEND_VERSION` bump by the rule
that already exists.

### What this may not do to `proved`

The prover's linear-arithmetic fragment is over `Int`. It does not extend by a
type arriving.

- **`Float` is excluded from `proved` entirely.** `==` on `Float` is not
  reflexive, and congruence closure over a relation that is not reflexive is
  unsound. Any obligation whose term graph mentions a `Float`-typed term reports
  `property`, never `proved`. This is a **structural refusal**: lowering returns
  "unsupported" on a `Float`-typed term, so the certificate cannot be
  constructed. A tier is computed from the evidence a discharge carries, and this
  is what makes that sentence true here rather than a convention someone has to
  remember.
- **`Decimal` may appear in a `proved` obligation only as an uninterpreted
  term.** Its `==` *is* an equivalence relation, so structural equality and
  congruence closure are sound over it: `f(x) == f(x)` is provable. No arithmetic
  and no ordering rules: `x + 0m == x` is `property`.
- Generators: `Float` draws finite values **and the specials** — `0.0`, `-0.0`,
  `±Infinity`, `NaN`, `±MIN`, `±MAX` — because a generator that never produced
  NaN would make `property` a lie about the type; it shrinks toward `0.0`.
  `Decimal` draws scale `0..=6` around zero plus `MIN`/`MAX`, shrinking toward
  `0m` and toward scale 0.
- `PROVER_VERSION` bumps: what a `property` discharge sampled changed.

### JSON's number representation, and its limit

```ply
type Json = Null | Bool(Bool) | Number(Decimal) | Str(String)
          | Array(List<Json>) | Object(Map<String, Json>)
```

`Number` holds a `Decimal` and **never an `f64`**. This is the whole reason
`Decimal` exists: a parser that routed numbers through binary64 would decode
`0.1` to the nearest double and no amount of care downstream recovers the
hundredth of a cent.

`Object` is a `Map<String, Json>`, so a JSON object's key order is ascending and
canonical, and re-encoding a decoded document is stable.

**The limit, stated plainly**: a JSON number outside `Decimal`'s range or needing
more than 28 significant digits — `1e100`, a 40-digit integer — is a decode error
naming the byte offset. The document is rejected **whole**, even when the codec
would never have read that field. That is a real cost and the tail is small
(nanosecond timestamps, `i64` ids and money all fit), and the alternative —
`Number(String)`, lossless and total — moves a parse into every consumer and
makes `1.0` and `1.00` unequal `Json` values. If a real payload demands it,
`Number(String)` is the escape hatch and it is a W3 decision with W3's evidence.

---

## 5. Byte-oriented builtins

### What W1 measured, and what fixes it

A served request cost 5.41 microseconds **per byte of head**. The cause is
algorithmic, not interpretive: the request head is scanned five times, each scan
an O(n) `fold` over the whole buffer that boxes a `Value::Int` per byte and has
no early exit. A faster interpreter divides that; only a smaller number of passes
removes it. JSON parsing written the same way would be unusable, which is why
these belong in the milestone that adds JSON.

### The builtins

All pure — an empty row — except `bytes_position`, which threads its predicate's
row. Out-of-range positions are `E0502 RUNTIME_ERROR` and are never clamped,
following W1's `bytes_slice`.

| builtin | type | notes |
| --- | --- | --- |
| `bytes_index_of` | `(Bytes, Bytes) -> Option<Int>` | first occurrence; an empty needle is `Some(0)` |
| `bytes_index_of_from` | `(Bytes, Bytes, Int) -> Option<Int>` | the index returned is absolute; `from` in `0..=len` |
| `bytes_index_of_byte` | `(Bytes, Int) -> Option<Int>` | the byte is `0..=255` |
| `bytes_starts_with` | `(Bytes, Bytes) -> Bool` | |
| `bytes_ends_with` | `(Bytes, Bytes) -> Bool` | |
| `bytes_split` | `(Bytes, Bytes) -> List<Bytes>` | an empty separator is `RUNTIME_ERROR`, as `string_split` is |
| `bytes_scan` | `(Bytes, Int, Bytes, Int) -> Int` | the bounded scan: first index at or after `from` whose byte is **not** in the set |
| `bytes_scan_until` | `(Bytes, Int, Bytes, Int) -> Int` | first index at or after `from` whose byte **is** in the set |
| `bytes_position` | `(Bytes, Int, (Int) -> Bool / e) -> Option<Int> / e` | the early-exiting find |

`bytes_scan(b, from, set, max)` takes the byte class as a **`Bytes` of its
members** rather than as an enum: `b"0123456789"`, `b" \t"`, `b"\r\n"`. That is
totally general with no new type and no closed set to extend, and a literal set
costs a bitmap build proportional to the set, not to the buffer. It returns
`min(bytes_len(b), from + max)` when it did not stop, so a caller distinguishes
"the class ended" from "the budget ran out" by comparison. `max` is what stops a
20-megabyte header line from being a denial of service, and it is an argument
rather than a global because the right bound is the caller's to know.

### Cost model

| builtin | time | allocation |
| --- | --- | --- |
| `bytes_index_of`, `bytes_index_of_from` | `memchr::memmem`, SIMD with a skip table; O(n) worst case and sublinear in practice | one `Value` for the `Option` |
| `bytes_index_of_byte` | `memchr::memchr`, SIMD | one `Value` |
| `bytes_starts_with`, `bytes_ends_with` | O(min(n, m)), exits at the first mismatch | none |
| `bytes_split` | O(n) over `memmem::find_iter` | one `Value::Bytes` **copy** per piece |
| `bytes_scan`, `bytes_scan_until` | O(min(max, n − from)) over a 256-bit membership bitmap built in O(len(set)) | **none per byte** |
| `bytes_position` | O(k), k = bytes examined | one boxed `Int` and one machine frame **per byte examined** |

`bytes_position` is the escape hatch and the ADR says so plainly: prefer
`bytes_scan` wherever the predicate is a byte set, because `bytes_position` pays
an allocation per byte and is the very cost W1 measured — reduced by early exit,
not removed.

`bytes_split` copies each piece because `Value::Bytes` is `Arc<[u8]>` with no
slicing, which ADR 0011 §8 chose deliberately and deferred to W3's streaming
bodies. W2 does not reopen it; it notes that a split of an n-byte buffer into k
pieces still copies n bytes once.

`ply-eval` gains `memchr` and `rust_decimal`. ADR 0011 said "`ply-eval` gains no
dependency"; W2 reverses that for two crates whose whole content is a thing it
would be foolish to write here — a SIMD substring search and an exact decimal —
and neither carries a runtime, a reactor or a value type that enters `Value`.

### The exit criterion is a re-measurement

The claim to be checked is that the per-request cost stops being proportional to
the head's length and becomes proportional to the number of **fields** parsed.
W1's per-request benchmark is re-run against a head parser written with these
builtins, and the number is reported. No target is invented here; W6 is the
milestone that decides what to do with it.

---

## Versions

| constant | to | why |
| --- | --- | --- |
| `RUNTIME_VERSION` | `0.7.0` | `Value` gains `Float`, `Decimal` and `Map`; the evaluator gains twenty-odd builtins. A cached `Pass` is a claim about what the evaluator did |
| `FRONTEND_VERSION` | `0.9.0` | `Lit` gains two variants, `FnDef` gains `constraints`, the prelude gains four types, and **the derivers' output is part of what the front end computes** |
| `BODY_ENCODING` | `6` | `tag::CONSTRAINT`, `LIT_FLOAT`, `LIT_DECIMAL`, and the cyclic-component payload below |
| `PROVER_VERSION` | `0.3.0` | new generators for `Float`, `Decimal` and `Map` change what a `property` discharge sampled |

### Amendment: the cyclic-component payload

`BODY_ENCODING` reads `6` rather than the `5` this ADR was written with, and the
reason is a defect W2's stdlib was the first program large enough to hit.
`std.json`'s parser is mutually recursive, and the refinement that assigns a
component member its index stopped as soon as the partition finished splitting —
one round *before* re-encoding under the labels that split it. The stored bytes
were therefore written under a coarser labelling than the one naming the members,
with two consequences: `f → g, g → h, h → f` and `f → h, h → g, g → f` hashed
identically, and no decoder could rewire either. So refinement now runs one round
past the settle, the payload holds one encoding per class **in class order**
rather than sorted by bytes, and a member's position in the payload *is* its
class. The hash of every mutually recursive definition moves once; nothing
acyclic moves at all. `ply_hash::component_hashes` is the single implementation,
which `ply-test`'s renormalizer had a copy of and no longer does.

## New diagnostic codes

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0113 | `RESERVED_MODULE_NAME` | a project file whose module name would be `std` or under it | the program's |
| E0206 | `NOT_DERIVABLE` | `derivable(D, t)` does not hold: at a `derive`, at a constrained call site, or at a `Map` key type | the program's |
| E0207 | `UNKNOWN_DERIVER` | a `derive` or `where` naming something that is not a deriver | the program's |
| E0208 | `ORPHAN_DERIVE` | a `derive` outside the module declaring its target | the program's |
| E0209 | `DECIMAL_DIVISION` | `/` applied to `Decimal` | the program's |
| W0605 | `STDLIB_CHANGED` | the cache was written under a different stdlib digest | nobody's |

`E0206` covers three shapes because they are one claim, and a consumer's response
to all three is the same: make the type derivable, or stop asking.

## Required tests

The ones whose absence would let W2 ship broken rather than merely incomplete.

**Stdlib**

1. `import std.net` from a project module resolves, checks and runs.
2. A project file at `std/json.ply` is `E0113` against the file.
3. A program importing nothing from `std` has hashes byte-identical to the same
   program before this milestone.
4. Copying a `std` module's source into the project produces **identical**
   `DefHash`es and shares its cache entries.
5. A compiler upgrade that changes no `std` source changes no hash and re-runs no
   test; changing one `std` definition re-selects exactly the tests reaching it
   and no others.
6. A `std` module's tests are not selected by a project run and write nothing to
   the project's cache; `--std` selects them.
7. `ply std --digest` is stable across runs and moves when any embedded source
   moves. A cache written under a different digest warns `W0605` once.
8. A `std` module importing a project module is `E0505`.

**Map**

9. `map_keys` is ascending regardless of insertion order, over 10,000 random
   insertion permutations of one key set.
10. `Value::cmp(a, b) == Equal` iff `values_equal(a, b)`, over the generator's
    whole range, with the `Float` NaN exception asserted explicitly rather than
    excluded.
11. A `Map` in a derived JSON encoding produces byte-identical output across
    runs and across `--engine both`.
12. `Map<Float, v>` is `E0206`; `Map<k, v>` under an unconstrained `k` is `E0206`
    naming the `where` clause to add, and adding it fixes it.
13. A `forall (m: Map<String, Int>)` law is discharged rather than `E0418`, and
    its counterexample shrinks toward `map_new()`.
14. Two maps built by different insertion orders are `values_equal`, and a test
    asserting so is cached under one order and read from cache under the other.

**Derivation**

15. `derive json for Order` generates `order_json`, and `decode(encode(x))` is
    `Ok(x)` for every `Order` the generator produces.
16. **Renaming the type re-runs no test.** **Renaming a variant re-runs exactly
    the tests reaching it.** Both, on one corpus, in one test.
17. Reordering two fields changes the generated definition's hash.
18. A type with a function field is `E0206` naming the field, not a partial
    encoder.
19. A recursive type derives, and its codec terminates on a value of depth 100.
20. A `derive` for another module's type is `E0208`; two `derive json` for one
    type are `E0105`.
21. A generated definition and a hand-written one with the same normalized form
    have the same hash.
22. `where derivable(json, a)` fails at the **call site** with the signature as a
    secondary label, and the body may assume the constraint.
23. Adding a `where` clause changes the definition's hash; reordering two `where`
    clauses does not; renaming the constrained type parameter does not.
24. A generated body that fails to typecheck is `E0505`, not a user-facing error.
25. The deriver's golden output pin fails when the deriver changes, and says to
    bump `FRONTEND_VERSION`.

**Numerics**

26. `1`, `1.0` and `1m` are three definitions with three hashes. `0.0` and `-0.0`
    are two.
27. `0.1m + 0.2m == 0.3m`, and `0.1 + 0.2 != 0.3` as `Float`.
28. `total / count` on `Decimal` is `E0209` naming `decimal_div`.
29. `decimal_div(1m, 3m, 2, HalfEven)` is `0.33m`; half-to-even rounds
    `0.125m` to `0.12m` at scale 2 and `0.135m` to `0.14m`.
30. A `Decimal` addition that overflows the mantissa is `RUNTIME_ERROR`, not a
    wrap and not a rounding.
31. **A law mentioning a `Float` is never `proved`**, including one that is
    trivially true, and the differential prover audit covers it.
32. A law over `Decimal` congruence is `proved`; a law over `Decimal` arithmetic
    is `property`.
33. `decimal_to_string` after `decimal_of_string` is identity, scale included.
34. A JSON number that does not fit a `Decimal` is a decode error naming the byte
    offset.
35. `1.50m` and `1.5m` are one map key, and the retained key is the last
    inserted.

**Bytes**

36. `bytes_index_of` agrees with a naive search over 10,000 random
    haystack/needle pairs, empty needle included.
37. `bytes_scan` never examines more than `max` bytes — asserted by a counting
    harness, not by timing.
38. `bytes_position` early-exits: a predicate that counts its calls is called
    once for a match at index 0 of a megabyte buffer.
39. `bytes_split` round-trips against a join for every separator the generator
    produces, and an empty separator is `RUNTIME_ERROR`.
40. `--engine both` on a corpus using every new builtin reports no `E0503`.
41. The re-measured per-request cost is recorded, and the head parser's cost is
    proportional to fields rather than to bytes.

**Everything else W2 must not regress**

42. `Store::open` at 10,000 definitions stays under 5 ms with `Map`, `Float` and
    `Decimal` in the encoding.
43. Incremental and `--no-incremental` agree byte-for-byte over a corpus using
    every W2 feature, across the full mutation sequence.
44. Renaming a top-level function still selects zero tests; moving a definition
    between modules still changes no hash — both on a corpus with derivations
    and stdlib imports.

Plus one `tests/fixtures/` entry per new code, as every milestone owes.

## Amendments, from the W2 audits

Six defects found against the implementation. Each is a place this ADR was
under-specified rather than wrong, and the amendment is what the code now does.

### A1. `json` refuses an `Option` whose payload can also be `null`

§3 lists `Option<a>` as structural without qualification. `option_json` writes
`None` as `null` and `Some(x)` as `encode(x)`, and `unit_json` writes `()` as
`null` — so `Option<Unit>` and `Option<Option<a>>` write `Some` and `None` as
the same document, and decode both back as `None`. Accepted, type-checking,
running, and losing the value with no error anywhere; worst as a `Map` key,
where a two-entry map decodes to one and `map_len` changes across the wire.

Both are now `E0206 NOT_DERIVABLE`. Tagging the encoding was the alternative and
is rejected: it would change the wire format of every optional field, and an
optional field being `null` is what a client already means by one. The refusal
is asked of one predicate in two places — `ply_derive::walk` names the field
before a body exists, and `ply_core::derivable` catches the spelling the
syntactic walk cannot see, because `type MyUnit = Unit` reaches the same
encoding through an alias and an alias is expanded only by the time a `Type`
exists. `ply_core` also now runs `derivable(D, T)` over each `derive`'s target
as a solved type, which is where the alias route surfaces.

Residual, stated rather than discovered later: `Option<p>` under a type
parameter instantiated at `Unit` is not refused. The constraint that would say
so is about `p`, not about `Option<p>`, and the language cannot express the
second.

### A2. `to_bytes` is bounded by the depth `parse` accepts

§4 bounds the parser and says nothing about the serializer, so encode was total
where decode was partial: a derived codec over a recursive type wrote documents
its own parser refused, at about half of `max_depth`, and the failure appeared
at the consumer. `to_bytes` now raises past `max_depth`, naming the bound. Test
19 asked only that the codec terminate on a value of depth 100, which it did at
the `Json` level; the wire is where it broke, and that is what the required test
did not look at.

### A3. `float_of_decimal` is correctly rounded, and a `Float` law is statable

§4 makes a non-finite `Float` a `RUNTIME_ERROR` and leaves two things unsaid.
A finite `f64` outside `Decimal`'s range raises too — the same limit §4 already
states for a JSON *number*, now stated for the encoder as well. And
`float_of_decimal` went through `Decimal::to_f64`, which divides a mantissa by a
power of ten in binary: `float_of_decimal(decimal_of_float(f))` was not `f` for
a value with a long scale, so the codec was **lossy** as well as partial. It now
goes through the decimal's digits, which Rust's correctly-rounded `f64` parser
turns into the nearest double.

With that, the codec is lossless on its whole domain, the domain is
`decimal_of_float(f) != None`, and a law that guards on it is discharged as
`property`. That is the finite-only mode §4's generator decision needs a law to
be able to ask for; it is a `where` guard, and it needed the losslessness fix to
work. An unguarded round-trip law over a `Float`-bearing type stays
`unattempted` with the value named, which is honest and now closable.

### A4. A `Map` key's wire form follows its type, not its spelling

§3 puts expansion before resolution and decides `Map<String, v>`-as-object from
the type as **written**. An alias is transparent to the checker, so
`type Key = String` made `Map<Key, Int>` and `Map<String, Int>` one type with
two wire formats — two codecs of the same type, interchangeable at every call
site, disagreeing about the protocol, with nothing at the `derive` line to read.

The deriver now resolves the key through **this module's own parameterless
aliases** before choosing. Only this module's: gate 1 keys on raw file content,
so a decision that read another module's aliases would leave a stale codec
behind when that module changed — expansion has to be a function of the file.
A cross-module alias to `String` therefore still gets the pair form, and that is
the price of the incremental design rather than an oversight.

### A5. A derived dictionary names nothing the deriving module can supply

Two instances of one mistake: a generated body wrote **bare** names, and ADR
0001 says a module's own items shadow the prelude.

`derive ord` emitted `compare(a, b)`, so a module declaring `fn compare`
supplied the order of every dictionary derived in it — not reflexive, not
antisymmetric, disagreeing with the order `map_keys` iterates in, while
`derivable(ord, T)` still reported the type as ordered. §2 rests on those being
one question with one answer. `derive ord` now emits `compare_values`, a
**reserved** builtin: redefining it is `E0105`, so no module can claim it.
`compare` stays as the same operation under a name a module may shadow, exactly
as `len` is. This imitates `derive eq`, which was safe only because `==` is a
token rather than a name.

`Expander::runtime_prefix` returned an **empty** prefix for
`ImportKind::Names(_)`, on the reasoning that a selective import binds the names
bare. But the prefix is what every leaf codec is written with, so under
`import std.json (..)` a module that defined `int_json` or `string_json` — and
did not selectively import the shipped one, so `AMBIGUOUS_IMPORT` never fired —
supplied the leaves its own `derive json` composed with. That is the divergence
§3's orphan rule exists to prevent, reintroduced silently. Expansion now
**synthesizes** `import std.json as <binder>` when the file bound no module name
for the runtime module, and the generated body always writes through a binder.
The binder is a function of the file's own imports, and enters no hash: a free
reference normalizes to its referent's hash rather than to the name it was
written under.

### A6. "Mentions a `Float`" is a question about a declaration

§4's structural refusal is stated over "any obligation whose term graph mentions
a `Float`-typed term", and the implementation asked it of a type's *written*
form: a `Type::Con`'s arguments, a `Type::Record`'s fields, a `Type::Fn`'s
positions. `type Money = Cents(Float)` is `Type::Con("Money", [])` and answered
no, so the prover certified `forall (m: Money) { m == m }` — false at
`Cents(NaN)`, and its own sampler finds the value unaided. The same held for an
`ensures`, for `List<Money>`, `Option<Money>` and `Map<String, Money>`. The
belt-and-braces check over the finished graph called the same function, so it
closed nothing.

`ply_prove::prove::Context::reaches_float` now walks the declaration, as the
least fixed point over `CheckOutput::ctors`, which settles a chain, a recursive
declaration and a container over one. It over-approximates a parameterised
declaration, which costs completeness and never soundness.
`ply_core::derivable` already answered this question correctly — it is what
refuses `Map<Money, v>` — and the two must not disagree.

### Versions the amendments move

| constant | to | why |
| --- | --- | --- |
| `RUNTIME_VERSION` | `0.8.0` | `compare_values` is a new builtin, and `float_of_decimal` answers differently |
| `FRONTEND_VERSION` | `0.10.0` | the derivers' output moved: `compare_values`, the `Map` key rule, the runtime binder |
| `PROVER_VERSION` | `0.4.0` | an obligation over a hidden `Float` is refused where it was certified |

## Not in W2

- **Type-directed dispatch.** ADR 0010 decision 6 stands: the deciding evidence
  is how often a type is abstract at the point of dispatch in a real stack, and
  W3 and W4 produce it. `++` stays `String`-only and `len` stays
  `(List<a>) -> Int` for the same reason.
- **`derive row`.** Named in ADR 0010, lands in W4 with the `Row` type.
- **User-defined derivers**, conditional instances, higher-kinded types.
- **A map literal.**
- **Byte-slice patterns**, and cheap slicing of a shared `Bytes` buffer. Both
  wait for W3.
- **A JSON number outside `Decimal`'s range.** Stated above; the escape is
  `Number(String)` and it is W3's call.
- **Top-level value definitions.** Derivation generates functions instead.
- **`string_find`'s inconsistency.** The prelude now has `Option`, so a partial
  lookup should return one — and `string_find` returns an `Int` and raises when
  absent, which is W1's shape. Changing it would move every hash that uses it for
  no behavioural gain, and adding a second name for one operation is worse than
  the wart. W3 may unify them; this ADR records it rather than leaving it to be
  rediscovered.
