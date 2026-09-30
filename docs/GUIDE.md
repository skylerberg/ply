# The Ply Guide

Ply is a general-purpose, statically typed, effect-tracked functional language.
It has no loops,
mutable variables, classes, exceptions or dispatch. A function's type says which
resources it touches, the unit of compilation is the content-hashed
*definition*, and the test runner re-runs exactly the tests whose hash changed.
This guide is the reference for writing Ply and using the `ply` command.

## 1. Getting started

Build with `cargo build --release` and put `target/release/ply` on your path. A
Ply file is a module:

```ply
// hello/main.ply
fn greeting() -> String = "hello from ply"

fn main() -> Unit = assert_eq(greeting(), "hello from ply")
```

`ply run hello` evaluates `main`, prints the value it returned (`()`) and exits
`0`. There is no `print`: output is the `std.process` effect (§13.9). The everyday commands are
`ply check` (parse, resolve, typecheck, infer rows), `ply test` and `ply run`.
Each takes a `.ply` file or a project root, defaulting to `.`.
`ply check --types` prints every definition's inferred signature.

**Starting a package.** `ply new demo` writes `demo/ply.pkg` and
`demo/main.ply` — a manifest (§3.3), a `main` and one test — and `cd demo &&
ply test` runs that test. The directory's last segment names the package, so
`ply new store/orders` makes `orders`; `--name` overrides it when the
directory is not a name (`ply new lib-src --name core`), and `--lib` writes a
`lib.ply` with a `pub` definition and no `main` instead. A name is what a
package prefix can be — lower-case letters, digits and `_`, joined by dots —
and a directory that is already there is refused rather than written into.

**Projects.** Every `*.ply` file under the root, except inside directories whose
name starts with `.`, is a module named by its relative path with `/` → `.` and
`.ply` dropped: `store/orders/place.ply` is `store.orders.place`. Every
directory name and file stem must be an identifier (`E0111`). `std` and
`compiler` are the built-in packages, pre-seeded for every load: a module
named under either collides with its prefix (`E0133`). Naming a single file
makes its parent the root and loads only that file.

**The cache.** `.ply-cache/` at the root holds the front-end, result and
obligation caches, the review baseline, and the git dependencies that were
fetched; `vendor/` holds the ones `ply vendor` copied, which is what a checkout
that must not reach the network carries. It is safe to delete
(`ply cache clear`); add it to `.gitignore`. `PLY_CACHE_UPSTREAM=DIR` names a
second cache shared between checkouts and machines, a directory on any storage
they all reach: the passes and discharged obligations found there count here,
and this run's are published there (`PLY_CACHE_UPSTREAM_READONLY=1` reads
only). Entries are keyed by content and by the `ply` version, so nothing
machine-specific is ever shared; `--no-cache` ignores it. A dependency's own
modules are keyed by its manifest rather than by where it sits, so moving or
re-checking-out a dependency keeps what was cached for it.

## 2. Lexical structure

### 2.1 Source and identifiers

Source is UTF-8; whitespace only separates tokens and there is no layout rule.
The only comment is `//` to end of line.

An identifier starts with an ASCII letter or `_` and continues with ASCII
letters, digits or `_`; `_` alone is the wildcard. A non-ASCII character outside
a literal is `X0001`. Case matters only in types (a bare lowercase name is a
type variable, uppercase a type constructor) and patterns (lowercase binds,
uppercase is a constructor). Convention: `snake_case` values, `UpperCamelCase`
types and constructors, lowercase effects and resource labels.

### 2.2 Keywords

Reserved: `pub` `import` `fn` `type` `effect` `nondet` `test` `let` `if` `else`
`match` `handle` `with` `true` `false`. A keyword may name a record field
(`{nondet: Bool}`, `d.nondet`), but not in a punned form (`{nondet}`,
`{nondet, ..}`), which also binds a variable.

These are keywords only in the position shown and identifiers elsewhere:

| word | keyword where |
| --- | --- |
| `as` | in an `import`, after the module path |
| `read`, `write` | opening an operation declaration, or after `.` in an atom |
| `set` | `effect set X = {..}` |
| `law`, `host`, `forall` | `law "..."` or `law/host` at item position; `forall` after the label |
| `derive`, `for`, `reuse` | `derive <deriver> for <Type>` and `reuse fn` at item position |
| `where`, `derivable` | after a signature's row, or after a law's binders |
| `requires`, `ensures` | between a `fn` header and its body |
| `resume`, `return` | in a handler clause (§6.5, §6.6) |
| `with_cell` | before `[` |
| `simulate` | before `{` where an expression can start |

### 2.3 Literals

| form | type | notes |
| --- | --- | --- |
| `42`, `1_000_000`, `0xFF` | `Int` | 64-bit signed; `_` between digits. Hex is bounded as a 64-bit pattern, so `0xFFFF_FFFF_FFFF_FFFF` is `-1`. |
| `255u8`, `0x6A09_E667u32`, `-1i8` | fixed width | Suffix `u8` `u16` `u32` `u64` `i8` `i16` `i32` `i64`. Decimal spellings are bounded by range (`256u8` is `E0211`), hex by width (`0xFFu8` is 255). |
| `1.5`, `1e9`, `2.5e-3` | `Float` | IEEE-754 binary64. |
| `1.50m`, `0m` | `Decimal` | Exact base 10; up to 28 fractional digits, 96-bit mantissa; keeps its written scale. |
| `"text"` | `String` | UTF-8; no line breaks. |
| `b"GET "` | `Bytes` | ASCII characters plus `\xNN`. |
| `true`, `false` / `()` | `Bool` / `Unit` | |

`1`, `1.0`, `1m` and `1u32` have four types and never convert implicitly
(`fn f() -> Int = 1.0` is `E0201`). A literal is never negative: `-3` is unary
minus, except in a pattern. The smallest signed value of a width cannot be
written as a literal; use `i8_of_int(-128)`.

String escapes are `\n` `\t` `\r` `\0` `\\` `\"` (no `\u`). Byte strings add
`\xNN` and refuse source characters above `U+007F`.

### 2.4 Operators

Loosest to tightest; all binary operators are left-associative:

| prec. | operators | operand types |
| --- | --- | --- |
| 1 | `\|\|` | `Bool` |
| 2 | `&&` | `Bool` |
| 3 | `==` `!=` `<` `<=` `>` `>=` | see below |
| 4 | `\|` | integer |
| 5 | `^` | integer |
| 6 | `&` | integer |
| 7 | `<<` `>>` `>>>` | integer; the count is `Int` |
| 8 | `++` | `String` or `Bytes` |
| 9 | `+` `-` | numeric |
| 10 | `*` `/` `%` | numeric |
| — | prefix `-` `!` `~` | numeric / `Bool` / integer |
| — | postfix `f(x)` `r.field` `e.op[r](x)` `e?` | |

* `==`/`!=` are structural at every type except functions. `Float` equality is
  IEEE, so `NaN != NaN`. `<` `<=` `>` `>=` work only on numeric types; order
  anything else with `compare`.
* Both operands have one type; there is no widening (`U8 + U16` is `E0201`).
* Arithmetic is checked: overflow, division by zero, and a shift count that is
  negative or not less than the type's width raise `E0502`. `<<` discards
  shifted-out bits; `wrap_*` wrap (§12.3).
* `/` on `Decimal` is `E0209`; use `decimal_div`. `%` is allowed.
* `&&`/`||` short-circuit. `&` `|` `^` `~` are integer-only and act at the
  type's own width (`~0u8` is `255u8`). `>>` is arithmetic, `>>>` logical.
  Shifts are adjacent `<`/`>` tokens, so `Map<Int, List<Int>>` still closes.
* `++` joins two `String`s or two `Bytes`, answering their type. One of each
  is `E0201`; there is no coercion, so cross with `bytes_of_string` or
  `string_of_bytes`. An operand nothing determines is `E0210`.
* `::` qualifies through a module binder (`items::price_of`) and does not chain.
* `?` binds tightest: `f(x)?.field` is `(f(x)?).field`. There is no `?:`.

## 3. Modules and items

A file is its imports followed by its items: `fn`, `type`, `effect`,
`nondet effect`, `effect set`, `test`, `law` and `derive`, in any order.
Definitions may refer to each other and recurse across the whole program.

### 3.1 Functions

```ply
type Account = { name: String, balance: Int }

fn credit(a: Account, amount: Int, note: Option<String> = None) -> Account {
  let moved = a.balance + amount;
  {name: a.name, balance: moved}
}
```

A body is `= expression`, or a block with no `=`. There is no `return`. A body
that binds is written as a block: `fn f() -> Int = { let x = 1; x }`, never
`fn f() -> Int = let x = 1; x`, which does not parse — a `let` statement is not
an expression. Every parameter and return type of a top-level `fn` must be
written (`E0126`, which names the inferred type). A parameter the body never
names may be written `_`, in a `fn`, a lambda or a handler clause, as in a
pattern; it binds nothing and may repeat.

A parameter default lets a call omit the argument. It must be a value — a
literal, a constructor over literals, a record or a list — and may not name
another parameter (`E0121`) or, on a `pub fn`, anything its module does not
export (`E0122`). Only a `fn` takes defaults (`E0120` elsewhere).

`reuse fn` promises that every `push` in the body reuses its list (§5.6). The
entry point is `main`, of any type and row: no `main` is `E0101`, several is
`E0112` (name the file to pick one).

### 3.2 Imports, visibility and namespaces

```ply
import store.orders                 // binds the module as `orders`
import store.orders as ord          // binds it as `ord`
import store.orders (place, cancel) // binds those names, no module binder
```

Reach through a binder with `::` (`orders::place(...)`). `as` and a name list
cannot be combined; write two imports. Imports precede every item.

The first segment of a module path may be a package: a dependency declared in
`ply.pkg` (§3.3) grants its own prefix, and `import cli.cmdline` reaches the
`cmdline` module of the package `cli`. A path whose first segment is neither a
module of this package, the root of one, nor a granted prefix is `E0106`; a
dependency's prefix used without the manifest declaring it is `E0132`. A bare
import is always this package's own: inside a dependency, `import fmt` names
*its* `fmt`, never the importing package's — a package cannot reach back into
what imports it.

Items are private unless `pub` (`E0107`). `pub` applies to `fn`, `type` and
`effect` only. Values (functions and constructors), types, effects and module
binders are separate namespaces, so `fn size`, `type Size` and `effect size`
coexist.

### 3.3 Packages and the manifest

A project root may hold a `ply.pkg` file: the package's manifest, exactly one
definition whose body is a literal of `std.pkg`'s `Manifest` (§13.15):

```ply
import std.pkg (Manifest)

fn package() -> Manifest = {
  name: "orders",
  version: {major: 0, minor: 1, patch: 0},
  prefix: None,
  runtime: {major: 0, minor: 1, patch: 0},
  dependencies: [],
  entry: None,
}
```

The body is data, not code: a literal, a constructor applied to literals, a
record or a list — the judgment §3.1 states for parameter defaults — refused
with `E0130` otherwise, as a manifest with any other shape is `E0129` and a
field that does not decode or fails validation is `E0131`. A project without
`ply.pkg` is the anonymous package. `ply new` (§1) writes this file, with the
toolchain it was made by as the `runtime`.

Each dependency grants its package's module prefix, and only what is declared
may be imported. A package's own modules answer to its sibling names:

```ply
import store.orders        // a module of this package, as before
import cli.cmdline         // a module of the declared dependency `cli`
```

A dependency is another package root: `Path("../cli")` names the directory
its `ply.pkg` stands in, and the dependency must hold one (`E0135`). Its
modules answer to `<prefix>.<name>` for every package that declares it — the
same dependency's own `import args` means *its* sibling `args`, never the
importing package's. A bare import inside a dependency names that package's
own modules only: the loading package's bare modules are unreachable from a
dependency. Reaching a package the manifest does not declare is
`E0132`, two packages granting one prefix is `E0133` — as is a module of the
root package squatting on a dependency's prefix — and a cycle of packages is
`E0134`. A `Git(url, rev)` dependency is fetched into the project's own cache
(`.ply-cache/git/`, one directory per url and revision) and read like any other
package root: `rev` may be a commit, a tag or a branch, and a branch means what
it means the day it is fetched — the fetched tree is reused without asking the
remote again, so a cleared cache is what picks up a moved branch, and `ply.lock`'s
digest is what catches it when that happens. A fetch that git cannot do is
`E0140`.

A `Registry` dependency is the package of that `name` from the registry
`PLY_REGISTRY` names (§15.1), at least `min`; its `name` must be a package name
(`E0131`). `ply resolve` is the one command that fetches from the registry: it
selects a version, fetches its archive, checks it and unpacks it into the
project's own cache, `.ply-cache/registry/<name>`, one directory per name,
where every other command reads it without the network. A registry dependency the cache does not
hold, or holds below a floor the closure asks for, is `E0135` or `E0136`, and
`ply resolve` is what fixes either. The archives themselves are kept beside it
as `<name>@<version>.plyz`, and one the cache already holds is not fetched
again. What the registry answered is checked before anything in it is read:
an archive whose bytes are not the digest the index lists, not the digest
`ply.lock` pins for that version, or not the package and version it was asked
for is `E0142` — a published version never changes, so different bytes for a
pinned one are refused rather than accepted. An unset, malformed or silent
registry is `E0141`, and a name the registry does not hold, or a floor no
unyanked version meets, is `E0143`.

The manifest is checked on every load. Resolution is minimal version selection:
two manifests may ask different floors of one package — the highest floor wins —
and a dependency below its importer's floor is `E0136`. One version of a package
serves a whole closure, so a package *of one name at two places* is `E0137`,
naming both requesters and both paths, while a package two others both depend on
is an ordinary diamond and resolves to the one version they agree on. Over a
registry the same rule selects: each floor reaches the lowest published version
at or above it that is not yanked — or the version `ply.lock` already pins, while
that is at or above it — each version reached adds its own manifest's floors, and
each name settles on the highest version reached. The order the manifests are
read in changes none of it. A yanked version is passed over by a new resolution
and kept by a lock that already pins it.

`ply build` records what it resolved in `ply.lock`, beside the package's own
`ply.pkg`: every dependency's name, its version, and the BLAKE3 digest of the
modules it contributed, sorted by name, and for a registry dependency the
`archive` digest it was fetched as. A package is pinned by *what* it is and
never by where it was found, so a moved checkout keeps its pin. A build verifies
the lock before it writes an artifact — a dependency whose sources moved since it
was pinned is `E0138`, and a lock this `ply` cannot read is `E0139` — and writes
one when the closure it resolved is not the one on file. `ply resolve` pins what is on
disk now, and is how a change to a dependency is accepted, deliberately —
deleting the lockfile, or one package's entry in it, does the same. `ply vendor`
copies the closure into `vendor/`, one directory per package named by the prefix
the closure granted it, whole — its `ply.pkg`, its modules and the data it ships,
but not the repository a fetch came from — plus `vendor/index`, one line per
package saying which want that directory answers. A walk that finds the index
reads those trees and asks for nothing else, so a vendored checkout builds with
no cache, no network and no git; the lockfile's digest still says the sources are
the ones that were pinned. A registry dependency is vendored like any other, and
`ply resolve` in a vendored project writes the version it selects into the
vendored copy as well as the cache, so the next walk reads what the lock pins. `ply why
NAME` says how a package got here: the path from the root package to it through
the packages that declare it, then the version and digest it resolved to.

A command acts on the root package, the one whose tree it was given: a
dependency's `main` is no entry point, and `ply test` runs the root package's
tests and never a dependency's, which are that package's own to run.

## 4. Types

Types are inferred by Hindley–Milner unification with row polymorphism. Written
signatures are checked, not inferred (§4.7).

### 4.1 Scalars and numbers

| type | values |
| --- | --- |
| `Int` | 64-bit signed; the type to count and index with |
| `U8` `U16` `U32` `U64` `I8` `I16` `I32` `I64` | fixed widths, for data defined in a width |
| `Float` | IEEE-754 binary64 |
| `Decimal` | exact base 10; `+ - * %` are exact or raise |
| `Bool`, `Unit` | `true`/`false`, `()` |
| `String` | UTF-8, indexed and sliced by character |
| `Bytes` | immutable bytes, indexed by byte |

There is no numeric tower. An operator's operand type is settled from the whole
definition, so `fn h(a: U32) -> U32 = a + 1u32` checks and `a + 1` does not. An
operand nothing determines, as in `let g = |a, b| a + b;`, is `E0210` — the same
code a `++` that says neither `String` nor `Bytes` raises; there is no default. Conversions are explicit builtins (§12.3). `u32_of_int` and its
siblings raise when the value does not fit (mask to truncate:
`u8_of_int(n & 0xFF)`); two fixed widths convert through `Int`.
`string_of_bytes` raises on invalid UTF-8.

### 4.2 Records and tuples

Records are structural; `type` names an alias, not a new type. Field order does
not matter.

```ply
fn f(a: Account) -> Int = a.balance
fn g(r: {name: String, balance: Int}) -> Int = f(r)     // same type
fn point(x: Int, y: Int) -> {x: Int, y: Int} = {x, y}   // `x` is `x: x`
fn divmod(a: Int, b: Int) -> (Int, Int) = (a / b, a % b)
```

A tuple is a record with positional fields: `(A, B)` is `{_0: A, _1: B}` in
types, values and patterns, accessed as `t._0`. `(A)` only groups; `()` is
`Unit`.

### 4.3 Lists and maps

`List<a>` is an immutable homogeneous sequence `[a, b, c]` (§5.6). `Map<k, v>`
is an immutable sorted map with no literal; build it with `map_new`,
`map_insert` or `map_of_entries`. It iterates in `compare` order. Its key type
must be ordered (`derivable(ord, k)`): `Float`, `Secret`, functions, `Cell` and
`Task` are refused (`E0206`).

### 4.4 Sum types

```ply
type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point

type Level = Debug | Info | Warn | Error
```

The leading `|` is optional. Constructors are values: `Circle(3)` is a call,
`Point` a reference. `type Id = Int` (one name, no payload, no `|`) is an alias.
Sums are the only nominal types: identical sums in two modules differ.

### 4.5 Generics

`fn apply<a, b | e>(x: a, f: (a) -> b / e) -> b / e = f(x)`: type parameters are
lowercase names in `<...>`, and row parameters follow `|` (`<| e>` if there are
no type parameters); a row variable among the type parameters is `E0301`.
Aliases may be parameterized: `pub type Route<a> = { ... endpoint: a }`.

A bracketed name binds a resource label:
`fn relay<[l]>(b: Bytes) -> Unit / {net.send[l]} = net.send[l](b)`. Binders sit
among the type parameters and before the `|` — `fn serve<a, [l], [k] | e>(..)` —
and one bracket may hold several, so `<[l, k]>` is `<[l], [k]>`. A call fills
them left to right, either written, `relay[conn](b)`, or from an argument whose
row names one (§6.2). A call inside a recursive group — to the definition itself
or to one it is mutually recursive with — keeps the labels the group was called
with: it writes none, or names this definition's own binders in order, and any
other label there is `E0306`. The members of such a group are checked with one
set of binders, positionally, whatever each calls them, so each binds the same
number of them; members that disagree are `E0307`. A printed signature shows
the binders, `<[l]>(Bytes) -> Unit / {net.send[l]}` and
`<a, [l] | e>(a) -> Unit / {net.send[l] | e}`, naming label variables `l`, `m`,
`n`, then `l1` — stepping past any of those a resource in the same signature
holds, so a row naming `[l]` prints its variable as `[m]` and the two stay
apart. Filling the wrong number of labels, leaving one unfilled, or
using a label-generic definition as a value instead of calling it, is `E0306`.

Row parameters are shared the same way: one recursive group is checked with one
set of row binders, taken in order whatever each member calls them, so a
definition generic over a row may call a mutually recursive sibling and the row
crosses the cycle. Members binding different numbers of them are `E0307`, as
with labels, and a call inside the group keeps the row the group was called
with: an argument carrying another row is `E0308`. Type parameters are *not*
shared — each definition keeps its own — so a call inside a group that would
need the callee's type parameter at another type is polymorphic recursion, which
Ply does not infer. That is `E0308` as well: break the cycle so the callee is
checked on its own before the call, or monomorphise it.

### 4.6 Types the language declares

In scope everywhere; redeclaring one is `E0105`:

```ply
Option<a>     = None | Some(a)
Result<a, e>  = Ok(a) | Err(e)
Ordering      = Less | Equal | Greater
Rounding      = HalfEven | HalfUp | Down | Up | Ceiling | Floor
Iter<s, r>    = Continue(s) | Stop(r)
```

A module that declares or unqualified-imports its own `Ok`, `Err`, `Some` or
`None` loses `?`; one that declares its own `Stop` loses `iterate`.

**`Secret<a>`** is made by `secret_of_string` and observed only by
`secret_verify`, `secret_is_empty` and `==`. It cannot be rendered, encoded or
ordered, and reaches a host operation only if that operation's registration
allows it (`E0439`).

**`Cell<a>`** (§7) and **`Task<a>`** (§9) are branded by their region and cannot
outlive it; the brand prints as `Cell[users]<Int>`.

### 4.7 Function types, and what is written

`(A, B) -> C` is pure; `(A) -> B / {db.read[users]}` and `(A) -> B / e` carry
rows. With no `/`, the row is inferred in a signature and empty in a declared
type. Functions cannot be compared, encoded, ordered or used as map keys.

* **Written:** every parameter and return type of a top-level `fn` (`E0126`),
  and every `forall` binder type.
* **Inferred:** effect rows. A written row is an upper bound; the inferred row
  must fit inside it (`E0302`), and it may be wider than the body needs. Each
  written atom covers what it names: the mode atom `net.write[conn]` covers
  every `write` operation of `net` on `conn`, the operation atom
  `net.send[conn]` covers `send` alone (§6.2).
* **Inferred inside bodies:** lambda binders, `let`s, everything else. A local
  `let` is monomorphic, so `let f = |x| x;` used at two types is `E0201`.

## 5. Expressions

Everything is an expression, including `if`, `match`, `handle` and blocks.

### 5.1 Blocks and `let`

A block `{ statements... tail }` has its tail's value, or `Unit` with no tail.
`let <pattern> [: Type] = <expr>;` binds once (the `;` is required) and may
shadow. An expression statement needs `;` unless it is the tail or block-like
(`if`, `match`, `handle`, a block, `with_cell`, `simulate`).

A `let` pattern can take apart a record, which is how a function returns several
values:

```ply
fn sum_two(input: Bytes) -> Int = {
  let {value, next} = advance(input, 0);
  let {value: second, ..} = advance(input, next);
  value + second
}
```

A record pattern names every field or ends with `..` (`E0201`). A `let` whose
pattern does not match raises; `let <pattern> = <expr> else { .. };` says what
happens instead. Where the pattern misses, the `else` block is the value of the
block the statement is in, and the statements after it do not run; it has that
block's type and sees none of the pattern's names. `?` in the `else` exits as it
would from the block's tail.

```ply
fn first_word(line: String) -> String = {
  let [word, ..] = string_split(line, " ") else { "" };
  word
}
```

`if a { x } else if b { y } else { z }` requires braces and one type for every
branch; without `else` its type is `Unit`.

### 5.2 `match` and patterns

```ply
fn area(s: Shape) -> Int =
  match s {
    Circle(r) -> 3 * r * r,
    Rect(w, h) -> w * h,
    Point -> 0,
  }
```

Arms are comma-separated (optional after a block-like arm) and may carry a
guard: `[x, y, ..rest] if x > y -> x + len(rest),`. A `match` must be exhaustive
(`E0205`, naming a missing case).

| pattern | matches |
| --- | --- |
| `_` / `name` | anything; `name` binds it |
| `Ctor`, `Ctor(p, q)`, `mod::Ctor(p)` | a constructor |
| `42`, `-1`, `1.5`, `1.50m`, `"s"`, `b"s"`, `true`, `()` | a literal |
| `[]`, `[a, b]`, `[a, ..]`, `[a, ..rest]` | a list of exact length, or a prefix |
| `{a, b}`, `{a: p, b: q}`, `{a, ..}` | a record; `..` allows other fields |
| `(p, q)` | a tuple |
| `p \| q` | either alternative, the leftmost first; parentheses nest a choice |

Every alternative binds the same names at one type (`E0212`). An arm is tried
once per alternative, so a guard runs again for a later alternative when an
earlier one matched and the guard refused it. A plain `let` may use an
or-pattern only where its alternatives cover the type, as in
`let Ok(v) | Err(v) = r;`; one that can fail needs an `else` (`E0213`).

### 5.3 Lambdas

```ply
|acc, a: Account| acc + a.balance
|| do_something()
|r: Result<Int, E>| -> Result<Int, E> { Ok(r? + 1) }
```

Annotations are optional (usually needed on the element of a `fold` or `map`
over records). A lambda captures by value, and its row flows into the enclosing
function's. A written return type requires a block body and enables `?` inside
it.

### 5.4 Calls

A bare variable followed by `.name(` is an effect perform, so a function in a
record field is called as `(codec.decode)(json)`; `int_json().decode(j)` needs
no parentheses because its base is a call.

Positional arguments fill parameters left to right; the rest must be named
(`greet("ada", greeting: "hey")`) or defaulted. A positional argument after a
named one is `E0124`; an unknown or repeated name `E0123`; too few arguments
`E0202`; a parameter left unfilled in a call that used names `E0125`. Only a
callee reached by name takes named arguments. There is no partial application
and no method syntax.

### 5.5 Record update

`{..base, deep: {..base.deep, a: 7}}` copies `base` with the written fields
replaced, answering a record of the base's field names. The base is a variable,
a field path, or one call (the call runs once); its fields come from the type
the checker infers for it, wherever that type was declared — another module's
`type` included. A base whose type nothing in the program determines is
`E0116`, and so is one that is not a record. A field the base lacks is `E0117`.

### 5.6 Lists

`list_at(xs, i)` answers `None` for an index past the end **or negative**:
`list_at(xs, -1)` is `None`, not the last element (that is
`list_at(xs, len(xs) - 1)`). `list_set(xs, i, v)` raises `E0502` out of range.

`push(xs, x)` appends in place when the caller holds the last reference, and
otherwise copies one path of the list's trie. A copy is caused by a second
owner: a binding read again after the `push`, a closure capture, a value read
out with `cell_get`/`map_get` (use `cell_update`/`map_update`), or a caller that
keeps using what it passed. `ply check --costs` reports every copying `push`
with its cause and fix. A `reuse fn` turns that into an error, `E0127`:

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n == 0 { xs } else { collect(push(xs, n), n - 1) }   // kept: xs is a parameter at its last use

reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {
  let ys = push(xs, n);
  if len(xs) < 0 { xs } else { ys }                       // E0127: xs is read again after the append
}
```

### 5.7 Iteration

There is no `for`, `while` or `break`. A call of the enclosing function in tail
position runs as a loop: it does not nest, so it may run any number of times.
Tail position is the body's own value, the tail of a block, an `if` or `match`
arm, or the right operand of `&&`/`||`:

```ply
fn sum_to(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { sum_to(n - 1, acc + n) }
```

A tail call of another function in the same module is a loop too when the two
are in one recursive group, a set of definitions whose tail calls lead back to
each other, so `even`/`odd` mutual recursion does not nest either. A group with
a `handle` in a member's body is compiled definition by definition and nests.

Every other call nests, at most 10,000 deep (then `E0502`): `1 + f(n - 1)`, a
call inside `handle`, `with_cell` or a lambda, and a call of another function.
Depth and work are two bounds: one entry may also make only so many calls — a
billion by default, and none under `ply run`, where an entry that serves forever
is a program — and a loop that never ends fails with `E0503` when that budget is
spent. `--steps N` sets it (`0` is no bound), and the count is the same on every
machine, so the verdict is too (§8.4).
`map`/`filter`/`fold`/`range` and the byte scanners do not nest calls, and
`iterate` is a loop with an early exit and a step budget:

```ply
fn first_gap(xs: List<Int>) -> Int =
  iterate({i: 0, want: 0}, 1000, |s: {i: Int, want: Int}|
    if s.i >= len(xs) { Stop(s.want) }
    else { Continue({i: s.i + 1, want: s.want + 1}) })
```

`step` answers `Continue(next)` or `Stop(result)`; spending the budget is
`E0502`.

### 5.8 `?`

```ply
fn int_value(text: String) -> Option<Int> = {
  let d = decimal_of_string(text)?;
  int_of_decimal(d, Down)
}
```

`e?` is sugar for `match e { Err(er) -> Err(er), Ok(x) -> rest }` (or the
`Option` form), with constructors taken from the enclosing function's
**written** return type. It may appear wherever nothing conditional sits between
it and the function's result and everything evaluated before it is pure:
`let x = e?;`, or `parse_or_more(parse_and(ts)?)`.

* It converts no errors: `Result<_, E1>` inside `-> Result<_, E2>` is `E0201`.
* `E0118`: inside a `handle`, `with_cell` or `simulate`; inside a
  lambda without a written return type; or where `Ok`/`Err`/`Some`/`None` are
  rebound. A lambda with a written return type exits the lambda.
* `E0119`: in an `if` branch, `match` arm or right of `&&` not in return
  position; after an impure argument (`g(h(x), k(x)?)`); or in a nested block.
  Bind the value first.

## 6. Effects and handlers

### 6.1 Declaring an effect

```ply
effect db {
  read  get[r](key: Int) -> Option<Row>
  write put[r](key: Int, value: Row) -> Unit
}

nondet effect clock {
  read now() -> Int
}
```

Each operation is `read` or `write`. `[r]` makes it resource-parameterized: a
perform must supply a label (`E0304`). `nondet` marks results that are not a
function of program state (§8.3). Effects are nominal. `task`, `clock`,
`random`, `sim` and `cell` are taken (`E0105`).

### 6.2 Atoms and rows

An atom is `effect.mode[resource]`, or `effect.mode` for a singleton. A row is a
set of atoms with an optional tail variable: `/ {db.read[users], clock.read}`,
`/ {net.write[conn] | e}`, `/ e`, `/ {}`. Qualified atoms use `::`:
`/ {store::db.read[users]}`. An atom may instead name an operation,
`net.send[conn]`, which the mode atom of the same effect and resource
(`net.write[conn]`) covers; naming an operation the effect does not declare is
`E0104`.

An inferred row names operations: `net.send[conn](..)` adds `net.send[conn]`,
a call adds the callee's row as written (a mode atom stays a mode atom), and a
lambda's type carries the operations its body performs. A definition with a
written row publishes that row, so a caller of one written `/ {net.write[conn]}`
gets the mode atom, which says the callee may perform any `write` of `net` on
`conn`. A written row must cover the inferred one atom by atom: under
`/ {net.send[conn]}` a body may `net.send[conn](..)` and not `net.recv[conn](..)`
(`E0302`, naming the operation and offering the atom to add), and may call a
callee written `/ {net.send[conn]}` but not one written `/ {net.write[conn]}`
(`E0302`, naming the callee). `ply check --types --explain` prints the inferred
row as `body performs`. Rows in types unify atom for atom: a function value
whose row names an operation is not the same type as one whose row names the
mode.

Resource labels are global — two modules writing `[users]` name one resource —
and a definition may be generic over one (§4.5). Its binder shadows that global
namespace inside the body: under `fn relay<[l]>`, the `[l]` of a row, of a
perform `net.send[l](b)`, of a handler clause `net.send[l](x) -> ..` and of a
nested call `inner[l](..)` is that parameter, while a label no binder holds is
the global one of that name. A call fills it with a label it writes,
`relay[conn](b)`, or with the one an argument's row names: a parameter typed
`() -> Unit / {net.send[l]}` given an argument whose row is `{net.send[conn]}`
fills `l` with `conn`. A label left unfilled is `E0306`.

The standard library is generic over its labels: `std.net` and `std.http` name
no resource of their own, so a program may answer its connections under one
label and talk upstream under another, over one serve loop and one writer
(§13.1, §13.2).

Two atoms **conflict** iff they name the same resource of the same effect and
one is a `write`.

### 6.3 Performing

`db.get[users](3)`, `clock.now()`, `store::db.put[orders](id, row)` add their
atom to the enclosing definition's row. A written row such as
`fn stale(s: Session) -> Bool / {clock.read}` is an upper bound (§4.7).

### 6.4 Effect sets

```ply
effect set Persist = {store.read[db], store.write[db]}
effect set Full    = {Persist, log.write[app]}
```

Sets are module-local (`pub` or `::` is `E0114`), may nest (a cycle is `E0115`),
and may not hold a row variable.

### 6.5 Handlers

```ply
test "expiry is decided against the deadline, not the wall clock" {
  handle {
    assert(!expired(1000, 60))
  } with {
    clock.now() -> 1060,
  }
}
```

A clause is `effect.op[resource](params) -> body`; an optional
`return x -> body` clause maps the result. A clause's label may be `[*]`, which
answers the operation on **every** label: `log.note[*](line) -> …` discharges
`log.note` wherever it is performed, `log.note[users]` and `log.note[orders]`
alike. This is what lets a library serve an effect whose atoms are
per-resource — a database driver over tables, a trace sink over channels —
where the label is chosen at the call site rather than by the handler. A clause
that names a label answers that label alone, and clauses are matched in order,
so a clause naming a label belongs before a `[*]` one that would answer it.
`[*]` on an operation declared without a label is `E0304`.

`[*t]` answers the same set of labels and **binds** the one the call site named to `t`, which is
in scope in the clause's body as a `String` — so a handler can check what it was asked rather
than only answer it. That is what a database driver's table check is: it answers `db.query` on
every table and refuses a statement that reaches one the call site did not label.

```ply
handle { load() } with {
  db.query[*table](sql, params) -> match labelled(table, sql) {
    Some(answer) -> answer,
    None -> panic("this statement reaches a table the call site did not label"),
  },
}
```

The label bound is the one the *call site* used, not the name of a label parameter: a clause
answering `relay[users]("k")` binds `"users"` even when the call site is inside
`fn relay<[l]>(…)`. A clause may either name a label or bind one, never both.

A **row** may name `[*]` too, and that is what lets a library handle an effect for code it is
given: `fn sink<a | e>(body: () -> a / {log.note[*] | e}) -> a / {wire.put[c] | e}` says the body
may perform `log.note` on any label and `sink` answers it. A row written with `[*]` is a licence
rather than a demand, so a body that performs nothing satisfies it; what the body may *not* do is
perform an operation the row does not name. `[*]` in a row on an operation declared without a
label is `E0304`.

A handler discharges an
**operation**: a clause for `net.send[conn]` removes that atom from the body's
row, and a clause set covering every operation of a mode (`send`, `recv` and
`close` for `net.write[conn]`) discharges the mode atom. The `handle`'s row is
the body's minus what the clauses discharge plus every clause's row. What
remains under a handled mode atom runs past the `handle`, so it is refused
(`E0305`, naming the operation, where it is performed or which callee reaches
it, and the clause to add) unless the enclosing function's written row covers
it: then it is forwarded to the caller's handler, as `std.db`'s `transaction`,
written `/ {db.begin, db.commit, db.abort | e}`, forwards `begin`, `commit`
and `abort` while answering `rollback`. A body that calls a function written
with a mode atom, or a function value whose type carries one, may perform any
operation of that mode: `E0305` then names the mode atom, says which callee or
value reaches it, and offers a clause per operation not yet named. A `handle`
is judged by rows alone, so a lambda or a function-typed parameter is judged
by its type.

### 6.6 `resume`

In `amb.flip[coin]() resume k -> k(true) + k(false)`, `resume k` binds the
continuation; the clause then has the `handle`'s type and may call `k` any
number of times. Without `resume`, a clause's value returns to the perform site.

### 6.7 Unhandled effects

`E0302`: the body performs an atom, or an operation, its written row does not
cover. `E0303`: an effect
escaped inference (a compiler defect). `E0305`: a `handle` lacks a clause for
an operation its body performs on an atom it handles. `E0424`: an operation
reached the host boundary with nothing bound — pass `--host` or handle it
(§14).

## 7. Cells and regions

```ply
fn counted(n: Int) -> Int =
  with_cell[work](0) { c -> {
    cell_set(c, cell_get(c) + n);
    cell_get(c)
  } }
```

`with_cell[r](init) { c -> body }` allocates a cell for the duration of `body`,
in an allocation scope named `r` that closes at the body's `}`. `cell_get`,
`cell_set` and `cell_update` are builtins whose atoms never leave the region.
Nest `with_cell`s for several cells; reusing the name allocates into the region
already open.

The scheduler that runs a spawned task must be younger than the region whose
cell the task is handed: open it inside the region,
`with_cell[r](init) { c -> simulate { .. } }`, and not around it,
`simulate { with_cell[r](init) { c -> .. } }`. An older scheduler — an enclosing
`simulate` region (§9), or the production one under `--host` — drains the tasks
nobody joined after the region's `}`, and a `task.join` inside the region does
not license it, because no type records the join.

* `E0201`: the cell escapes its `with_cell[r]` region.
* `E0446`: a region-branded value outlives the region (stored in an older
  binding, handed to an operation, put in a declared type, or handed to a
  `task.spawn` whose scheduler is older than the region).
* `E0449`: a region handle (a cell, a task, or the continuation a clause's
  `resume` binds) reaches a host operation, a host answer, or an entry point's
  argument or answer (at run time). A continuation's type is an ordinary
  function's, so this is the one check that sees it.
* `W0610`: a reference cycle; cycles are never freed.

## 8. Tests

### 8.1 Writing tests

`test "label" { ... }` is an item with a block body (§6.5 shows one); it cannot
be `pub`, referenced or given arguments. The body is `Unit`: a test passes when
it finishes, so one that ends on a value, such as a comparison missing its
`assert`, is `E0201`. `assert(cond)` / `assert(cond, Some("why"))` and
`assert_eq(actual, expected)` fail with `E0501`, the latter reporting both
values and their first difference. Any other failure is `E0502`.

### 8.2 Selection

A definition's hash covers its normalized form: names, comments, formatting,
imports, `pub`, specs and test labels are erased, and references are replaced by
their referent's hash. A test runs exactly when its hash has no recorded pass,
so renames and comment edits run nothing. `ply hash` prints the hashes.
`--explain` says why each test was selected; `--filter SUBSTRING` matches
`<module>.<label>`; `--no-cache` bypasses both the result and the front-end
cache.

### 8.3 Determinism

A test whose row, after handling, retains a `nondet` atom is `E0412`. Handle the
effect, or write `test/nondet "label" { ... }`, which is never cached.

### 8.4 Scheduling and failures

Tests whose footprints do not conflict run concurrently; a test whose effects
are all discharged in a region conflicts with nothing. `--jobs N`/`-j` sets
workers (default one per core).

`--steps N` is the calls each test may make (default 1000000000; `0` is no
bound); a test past it fails with `E0503`, which is a program error like any
other, and is recorded as one, because the count is a property of the program.

`--timeout MS` is the wall clock each test may take (default 60000; `0` is no
clock). It is not a verdict: a test past it is *abandoned* (`W0612`), reported
apart from the failures, recorded nowhere, and run again next time. A run with
an abandoned test is not a success, since it decided nothing about that test.

A failing deterministic test that has passed before is bisected over the
definitions that changed to name a culprit. `--bisect auto|always|never`
(default `auto`) and `--bisect-budget N` (evaluations, default 64) control
this. `--json` prints one object with each failure's diagnostic, declared
footprint, suspects, culprit and replay command (`schema_version` 6); the
suspects are ranked culprits first, then an edited definition before one whose
hash only moved. Each result counts the operations its test performed,
handled ones included, as `performs`. `--watch` re-runs on every `.ply` change,
keeping caches in memory.

### 8.5 Coverage and mutants

`--coverage` reports, from the hash closure alone, which tests reach each
definition and which definitions no test reaches. `--mutate [DEF]` runs after a
green run: each definition (or `DEF`, a program-wide or unique simple name) is
changed one operator or literal at a time (`+`/`-`, `<`/`<=`, `>`/`>=`,
`==`/`!=`, `&&`/`||`, `true`/`false`, `!` dropped, an integer raised by one),
the program is checked again, and the tests that reach the definition run
against the mutant on a scratch store that never touches the cache. A mutant
every one of them passes is a survivor, reported with its place and the tests
that let it through, and fails the run. A mutant that does not check is
skipped; `--mutate-budget N` (default 64) caps how many are judged.

### 8.6 Compiled backend

The program is compiled to C and runs there: compiled code is the only
evaluator, and every command uses it. `--profile development` (default;
fastest compiler) or `release` (`cc -O2`) selects the C toolchain.

A definition the backend cannot compile is `E0448`, raised where the program is
built and naming the construct that refused it, since nothing could ever enter
that body. `PLY_C_ONLY`/`PLY_C_SKIP` below ask for a partial unit on purpose,
and a definition dropped because one of those took its callee away is reported
rather than raised.

| variable | effect |
| --- | --- |
| `PLY_C_PROFILE=development\|release` | the profile, overriding `--profile` |
| `PLY_CC=cmd`, `PLY_CC_OPT=flag` | the C compiler and its optimisation flag, overriding the profile's |
| `PLY_C_CACHE=DIR` | compiled unit cache (default under the temp directory) |
| `PLY_C_STAGE=DIR` | the compiler's own stages, kept apart from the cache so a fresh cache reuses them, and the front-end answers `ply run` files (§16) (default under the temp directory) |
| `PLY_C_CACHE_MAX=BYTES` | cap on the cache and on the stages, each swept oldest first, a stage never within an hour of its last use; `0` is no cap |
| `PLY_C_KEEP=1` | keep and print the emitted `.c` and shared object |
| `PLY_C_REFUSALS=1` | print which definitions the backend refused, and how many it took |
| `PLY_C_DUMP=NAME` | print one body's emitted C, or `*` for the unit's largest bodies |
| `PLY_C_ONLY=a,b`, `PLY_C_SKIP=prefix,...` | compile only the named definitions, or drop those with a prefix; the unit is then partial and a caller of what was dropped is declined, not raised |
| `PLY_C_PHASES=1` | print compile phases, body-cache hits and misses, and allocation counts |
| `PLY_HEAP_POISON=1` | poison released blocks and fail on a read of one |
| `PLY_HEAP_DELAY=N` | reuse a released block only after `N` more releases |
| `PLY_C_EMITTER=ply:DIR` | use emitter sources from `DIR` instead of the built-in ones |
| `PLY_CODEGEN_REGISTER=narrow` | enter compiled code only for scalar signatures |

## 9. Simulation

```ply
simulate {
  let a = task.spawn(|| settle("alice", "bob", 60));
  let b = task.spawn(|| settle("alice", "carol", 60));
  task.join(a);
  task.join(b);
  assert_eq(overdrawn(cell_get(ledger)), 0)
}
```

`simulate { ... }` handles the language's concurrency effects with a seeded
scheduler:

```ply
nondet effect task   { write spawn<a | e>(body: () -> a / e) -> Task<a> / e
                       write join<a>(t: Task<a>) -> a
                       write yield() -> Unit }
nondet effect clock  { read  now() -> Int
                       write sleep(nanos: Int) -> Unit }
nondet effect random { write next() -> Int
                       write below(bound: Int) -> Int }
effect sim           { read  seed() -> Int }
```

* The region's row gains `sim.read`, which a deterministic test may carry.
* Virtual time advances only when no task is enabled, so `clock.sleep` costs no
  wall clock.
* A task performs against the handlers around its `task.spawn`; a clause that
  binds `resume` is unreachable from a task (`E0502`).
* `E0413`: a `Task` escapes. `E0414`: no progress, or a spent step budget.
  `E0416`: nested `simulate`. `E0425`: a host operation inside the region,
  refused before the handler runs and whether or not it is bound; the region
  answers `task`, `clock`, `random` and `sim.seed` itself.

Tasks interleave only at `task`, `clock` and `random` operations; any two
allocations, and two accesses to one cell with a write, are ordered. A
read-then-write with no scheduler operation between runs as one step, so put a
`task.yield()` there. When the default search exhausts its frontier the result
holds for **every** interleaving and is reported `exhaustive`.

| flag | meaning |
| --- | --- |
| `--sim dpor` | footprint-guided partial-order reduction (default) |
| `--sim random` | one interleaving per seed |
| `--sim once` | exactly one interleaving |
| `--seeds N` | seeds per test, from 0 (default 1 under `dpor`, 64 under `random`) |
| `--sim-roots FROM..TO` | the seeds from `FROM` up to but not including `TO` instead: `5..6` is seed 5 alone |
| `--sim-budget N` | interleavings per seed (`dpor` only) |
| `--sim-steps N` | steps per interleaving before `E0414` |
| `--seed 7`, `--seed 7:3.0.2` | replay one interleaving; implies `--sim once` |
| `--measure-reduction` | also run the search twice more, unpruned (`naive`) and blind to the order spawns and joins impose (`blind`), and report each count; a failure only they reach fails the test |

Results are cached per search plan; a search that spends its budget passes but
is not cached. A failure prints the racing steps, their tasks and positions, and
a replay command such as
`ply test --seed 0:0.1.0.2 --filter "no account is ever overdrawn"`.

## 10. Specifications, laws and proof

```ply
fn adjusted(account: Account, amount: Int) -> Account
  requires amount > -1000000000 && amount < 1000000000
  requires account.balance > -1000000000 && account.balance < 1000000000
  ensures result.name == account.name
  ensures result.balance == account.balance + amount
= {name: account.name, balance: account.balance + amount}

law "a credit and a matching debit leave an account exactly as it was"
  forall (account: Account, amount: Int)
  where amount > -1000000000 && amount < 1000000000 {
    adjusted(adjusted(account, amount), -amount) == account
  }
```

* `requires`/`ensures` go between the signature and body, in any number;
  `result` is bound in `ensures`. `requires` restricts the domain of its
  `ensures`; it is not checked at call sites and laws do not inherit it.
* A law has a label, optional `forall` binders (typed; `E0418` if a type cannot
  be quantified), an optional `where` guard and a block body.
* Specs, guards and law bodies must be pure (`E0417`), except that a law body
  may be a `simulate` region. `law/host "..." { }` allows any effect but is
  never `proved` or cached, and is `W0604` under a hermetic run.
* Specs do not change a definition's hash. An `ensures` implies every resource
  outside the footprint is unchanged; there is no `old()`.
* `Int` arithmetic is checked, so bound the domain with guards as above.

| tier | claim |
| --- | --- |
| `proved` | holds for every input satisfying the guard |
| `property` | randomized cases passed; failures shrink |
| `example` | concrete cases passed |
| `unattempted` (`W0604`) | undecided; never green, never cached |
| `defect` | Ply failed rather than the program: nothing is claimed, never cached, exit 1 |

`proved` covers ground evaluation, enumeration of finite domains up to 4096
points, linear `Int` arithmetic, case splits, congruence, constructor
injectivity, unfolding non-recursive definitions, exhaustive interleaving, and
induction on an `Int` binder: a definition that calls only itself with some
`Int` argument non-negative and smaller at every self call is unrolled, and the
claim is proved at `n <= 0` and then at `n > 0` from itself at `n - 1`; and
induction on a `List` binder: a definition whose self calls take a tail its
list patterns exposed is unrolled, and the claim is proved at `[]` and then at
`[h, ..t]` from itself at `t`, with `len` and `push` reduced over the spine in
view and `len` known to lie below `i64::MAX`.

`ply prove` reports the definitions carrying no obligation, then each
obligation's tier; `E0419` is a counterexample and `E0420` a guard admitting no
values. A proposition that raises is a gap in the claim; one the compiled tier
declines, or any other failure that is Ply's own, is a `defect` reported under
Ply's code (`E0505`), as `ply test` reports one. Under `--json` a gap carries
its sentence as `gap` and its kind as `gap_kind` (`unhandled_effect`,
`ungeneratable`, `raised`, `guard_not_sampled`, `reaches_host`, `not_drawn`), a
defect carries `defect` — its `code`, `message`, the `bindings` Ply failed at,
and a `summary` — and `summary` counts defects as `defect`. A claim's type
variables are lettered by where they first appear among
its binders (`forall (x: a, y: List<b>)`); a sample draws each as `Int`
(`a := Int`), and a proof leaves each an uninterpreted sort
(`uninterpreted a, b`). Flags: `--prove-cases N` (below 25 kept cases only `example`),
`--prove-roots N`, `--prove-budget N` (spent reports `property`),
`--shrink-budget N`, `--prove-steps N` (calls per evaluation of a claim, default
1000000000; an evaluation past it leaves the obligation `unattempted`, and the
number keys the cached result, so more budget is a stronger claim). `--reach`
asks the static tier alone about every obligation the run reports on, cached or
not, and under `--json` each then carries `reach`: what it decided (`proved`,
`guard_unsatisfiable`, `open` or `budget_spent`), the steps it spent, and each
place it left the decidable fragment as `{kind, about}` — `null` for a law over
interleavings, which the static tier never sees.

`ply review` reports, per definition changed since the last
`ply review --accept`, whether the implementation, the spec and the obligations
changed. The baseline is keyed by name.

## 11. Derivation

```ply
import std.json

pub type Line = { sku: String, qty: Int, unit_price: Decimal }
derive json for Line
```

| deriver | generates | type |
| --- | --- | --- |
| `json` | `<snake_case(T)>_json` | `std.json.JsonCodec<T>` |
| `eq` | `<snake_case(T)>_eq` | `{eq: (T, T) -> Bool}` |
| `ord` | `<snake_case(T)>_ord` | `{compare: (T, T) -> Ordering}` |

There are no other derivers (`E0207`). A name collision (`HTTPRequest` and
`HttpRequest` both give `http_request`) is `E0105`. A `derive` must be in the
module declaring its type (`E0208`). A parameterized type's function takes one
dictionary per parameter:

```ply
pub type Box<a> = { label: String, inner: a }
derive json for Box
// box_json : <a>(JsonCodec<a>) -> JsonCodec<Box<a>>

fn encode<a>(b: Box<a>, c: json::JsonCodec<a>) -> String
  where derivable(json, a) =
  json::encode_string(b, box_json(c))
```

`where derivable(D, p)` goes after the row and before any `requires`. Codecs are
plain values: `json::decode_bytes(body, order_json())`.

`E0206` names the field that blocks a derivation: function types, `Cell` and
`Task` (all derivers); `Float` (`ord`); `Secret` (`json`, `ord`); `Option<Unit>`
and `Option<Option<a>>` (`json`).

## 12. Builtins

In scope everywhere; a module may shadow any except `compare_values` (`E0105`).
Out-of-range indexes and slices raise `E0502` unless noted; nothing is clamped.
`ply doc NAME` prints any of these from the compiler's own table, which is the
authority when this page and it disagree.

### 12.1 Core, lists and maps

| signature | notes |
| --- | --- |
| `assert(cond: Bool, message: Option<String> = None) -> Unit` | `E0501` |
| `assert_eq<a>(actual: a, expected: a) -> Unit` | `E0501` |
| `panic<a>(message: String) -> a` | `E0502` |
| `compare<a>(x: a, y: a) -> Ordering` | total order; needs `derivable(ord, a)` |
| `compare_values<a>(x: a, y: a) -> Ordering` | the same, under a reserved name |
| `min`, `max` `(a: Int, b: Int) -> Int` | |
| `cell_get<a>(c: Cell<a>) -> a` | |
| `cell_set<a>(c: Cell<a>, v: a) -> Unit` | |
| `cell_update<a \| e>(c: Cell<a>, f: (a) -> a / e) -> Unit / e` | the cell is unreadable while `f` runs |
| `secret_of_string(s: String) -> Secret<String>` | |
| `secret_verify(stored: Secret<String>, supplied: String) -> Bool` | constant-time, not rate-limited |
| `secret_is_empty<a>(s: Secret<a>) -> Bool` | |
| `len<a>(xs: List<a>) -> Int` | |
| `push<a>(xs: List<a>, x: a) -> List<a>` | |
| `list_at<a>(xs: List<a>, i: Int) -> Option<a>` | `None` if negative or past the end |
| `list_set<a>(xs: List<a>, i: Int, v: a) -> List<a>` | |
| `map<a, b \| e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e` | |
| `filter<a \| e>(xs: List<a>, f: (a) -> Bool / e) -> List<a> / e` | |
| `fold<a, b \| e>(xs: List<a>, init: b, f: (b, a) -> b / e) -> b / e` | |
| `range(lo: Int, hi: Int) -> List<Int>` | `[lo, hi)` |
| `iterate<a, b \| e>(seed: a, budget: Int, step: (a) -> Iter<a, b> / e) -> b / e` | |
| `map_new<k, v>() -> Map<k, v>` | |
| `map_insert<k, v>(m: Map<k, v>, key: k, value: v) -> Map<k, v>` | |
| `map_get<k, v>(m: Map<k, v>, key: k) -> Option<v>` | |
| `map_contains<k, v>(m: Map<k, v>, key: k) -> Bool` | |
| `map_remove<k, v>(m: Map<k, v>, key: k) -> Map<k, v>` | |
| `map_len<k, v>(m: Map<k, v>) -> Int` | |
| `map_keys<k, v>(m: Map<k, v>) -> List<k>` | ascending |
| `map_values<k, v>(m: Map<k, v>) -> List<v>` | key order |
| `map_entries<k, v>(m: Map<k, v>) -> List<{key: k, value: v}>` | key order |
| `map_of_entries<k, v>(es: List<{key: k, value: v}>) -> Map<k, v>` | |
| `map_merge<k, v>(a: Map<k, v>, b: Map<k, v>) -> Map<k, v>` | `b` wins |
| `map_fold<k, v, c \| e>(m: Map<k, v>, init: c, f: (c, k, v) -> c / e) -> c / e` | key order |
| `map_update<k, v \| e>(m: Map<k, v>, key: k, f: (v) -> v / e) -> Map<k, v> / e` | no-op if absent |
| `observe<a>(v: a) -> a` | the identity, through a call the C tier cannot see into: a pure computation whose value is only observed still runs |

### 12.2 Strings and bytes

Strings are indexed by character, bytes by byte.

| signature | notes |
| --- | --- |
| `string_len(s: String) -> Int` | |
| `string_slice(s: String, start: Int, end: Int) -> String` | |
| `string_split(s: String, sep: String) -> List<String>` | |
| `string_trim`, `string_lower`, `string_upper` `(s: String) -> String` | |
| `string_starts_with`, `string_ends_with`, `string_contains` `(s: String, t: String) -> Bool` | |
| `string_find(s: String, needle: String) -> Int` | raises if absent |
| `string_concat(a: String, b: String) -> String` | `a ++ b` |
| `int_to_string(n: Int) -> String` | |
| `float_to_string(f: Float) -> String` | shortest round-trip; `Infinity`, `-Infinity` and `NaN` |
| `bytes_len(b: Bytes) -> Int` | |
| `bytes_at(b: Bytes, i: Int) -> Int` | `0..=255` |
| `bytes_u32_le(b: Bytes, i: Int) -> U32` | four bytes, little-endian |
| `bytes_slice(b: Bytes, start: Int, end: Int) -> Bytes` | |
| `bytes_concat(a: Bytes, b: Bytes) -> Bytes` | `a ++ b` |
| `bytes_concat_all(bs: List<Bytes>) -> Bytes` | one allocation |
| `byte_of_int(n: Int) -> Bytes` | raises outside `0..=255` |
| `bytes_of_string(s: String) -> Bytes` | |
| `string_of_bytes(b: Bytes) -> String` | raises on invalid UTF-8 |
| `string_of_bytes_lossy(b: Bytes) -> String` | U+FFFD for invalid UTF-8 |
| `bytes_is_utf8(b: Bytes) -> Bool` | |
| `bytes_index_of(hay: Bytes, needle: Bytes) -> Option<Int>` | |
| `bytes_index_of_from(hay: Bytes, needle: Bytes, from: Int) -> Option<Int>` | |
| `bytes_index_of_byte(hay: Bytes, byte: Int) -> Option<Int>` | |
| `bytes_starts_with`, `bytes_ends_with` `(b: Bytes, t: Bytes) -> Bool` | |
| `bytes_split(b: Bytes, sep: Bytes) -> List<Bytes>` | |
| `bytes_scan`, `bytes_scan_until` `(hay: Bytes, from: Int, class: Bytes, budget: Int) -> Int` | stop at the first byte not in / in `class`; `from + budget` if none |
| `bytes_position<\| e>(b: Bytes, from: Int, f: (Int) -> Bool / e) -> Option<Int> / e` | |
### 12.3 Numbers

| signature | notes |
| --- | --- |
| `decimal_div(a: Decimal, b: Decimal, scale: Int, mode: Rounding) -> Decimal` | |
| `decimal_round(d: Decimal, scale: Int, mode: Rounding) -> Decimal` | |
| `decimal_of_int(n: Int) -> Decimal` | |
| `int_of_decimal(d: Decimal, mode: Rounding) -> Option<Int>` | |
| `float_of_decimal(d: Decimal) -> Float` | |
| `decimal_of_float(f: Float) -> Option<Decimal>` | |
| `decimal_of_string(s: String) -> Option<Decimal>` | |
| `float_of_string(s: String) -> Option<Float>` | `Float` literal syntax with a sign; `None` for `inf`/`NaN` |
| `decimal_to_string(d: Decimal) -> String` | |
| `bits_of_float(f: Float) -> Int`, `float_of_bits(n: Int) -> Float` | IEEE-754 bit pattern; total |
| `u8_of_int(n: Int) -> U8` … `i64_of_int(n: Int) -> I64` | eight; raise if out of range |
| `int_of_u8(n: U8) -> Int` … `int_of_i64(n: I64) -> Int` | eight; total but `int_of_u64` |
| `wrap_add`, `wrap_sub`, `wrap_mul` `(a: t, b: t) -> t` | any integer `t`; wraps at `t`'s width |
| `rotr(x: t, n: Int) -> t` | rotate right at `t`'s width, count modulo the width |
| `rotr32(x: Int, n: Int) -> Int` | rotate the low 32 bits of an `Int` |

## 13. The standard library

The built-in package, shipped inside `ply` and pre-seeded for every load — an
implicit dependency of every package, no declaration needed: `import
std.<name>`. Its tests and obligations are skipped unless you pass `--std`.
`ply std` lists it, `ply std --show std.json` prints one source and `ply std
--show` alone prints every one; a changed standard library warns `W0605`.

### 13.1 `std.net` — sockets

```ply
pub nondet effect net {
  write listen[s](port: Int) -> Int
  write listen_tls[s](port: Int, credential: String) -> Int
  write connect[s](host: String, port: Int, timeout_ms: Int) -> Option<Int>
  write connect_tls[s](host: String, port: Int, timeout_ms: Int) -> Option<Int>
  write handshake[s](conn: Int) -> Option<Int>
  write accept[s](listener: Int) -> Int
  write recv[s](conn: Int, max: Int, timeout_ms: Int) -> Option<Bytes>
  write send[s](conn: Int, payload: Bytes, timeout_ms: Int) -> Option<Int>
  write close[s](socket: Int) -> Unit
  read  local_port[s](socket: Int) -> Option<Int>
}
pub fn drain<[l]>(c: Int, so_far: Bytes, timeout_ms: Int) -> Bytes / {net.recv[l]}
pub fn send_all<[l]>(c: Int, payload: Bytes, timeout_ms: Int) -> Bool / {net.send[l]}
```

Both take their label from the caller (§6.2): `send_all[conn](..)` writes to a
connection this program answered, `send_all[upstream](..)` to one it opened.
`None` is a deadline expiring; an empty `Some` is EOF; `timeout_ms <= 0` is a
runtime error. `send` may write fewer bytes than given; `send_all` loops.
`connect` resolves the host and tries each address until the deadline; `None`
is a host not reached for any reason, and the connection it answers is used
under the label it was opened under. `connect_tls` is the same over TLS: the
server is verified as `host` against the built-in roots and any `--trust`
certificate, and a failed handshake reads as EOF and writes `0`. The handshake
itself is left to the first `send` or `recv`, so a peer that connects and says
nothing costs nothing; `handshake` completes it there and then and answers what
it took in microseconds, which is what a program that times a connection wants
(`None` for a connection with none to complete: a plaintext one, or a session
whose handshake failed, which ends it). `local_port` answers the port a socket
is bound to on its own end — for a listener asked for port `0`, the one it was
given, so a server can listen anywhere free and say where — and `None` for a
listener the drain has closed.

### 13.2 `std.http` — HTTP/1.1

Parsing and encoding are pure; the serve loop and the client perform `net`
under the labels a caller fills. An ambiguous message is refused and the
connection closed; every loop is bounded by `Limits`.

```ply
pub fn read_head<[l]>(c: Int, carried: Bytes, limits: Limits, idle: Bool)
  -> HeadRead / {net.recv[l]}
pub fn read_body<[l]>(c: Int, framing: Framing, buf: Bytes, limits: Limits)
  -> BodyResult / {net.recv[l]}
pub fn serve_connection<[l] | e>(c: Int, limits: Limits, app: (Request) -> Response / e)
  -> Unit / {net.recv[l], net.send[l], net.close[l] | e}
pub fn serve<[l], [k] | e>(listener: Int, count: Int, limits: Limits,
                           app: (Request) -> Response / e)
  -> Int / {net.accept[l], net.recv[k], net.send[k], net.close[k] | e}
pub fn listen_and_serve<[l], [k] | e>(port: Int, count: Int, limits: Limits,
                                      app: (Request) -> Response / e)
  -> Int / {net.listen[l], net.accept[l], net.close[l],
            net.recv[k], net.send[k], net.close[k] | e}
pub fn respond_chunked<s, [l] | e>(c: Int, version: Version, r: Response, keep_alive: Bool,
                                   limits: Limits, seed: s,
                                   produce: (s) -> Option<{ chunk: Bytes, next: s }> / e)
  -> Bool / {net.send[l] | e}
pub fn request<[l]>(host: String, port: Int, req: Request, limits: Limits)
  -> Result<Response, ClientError>
  / {net.connect[l], net.send[l], net.recv[l], net.close[l]}
```

`serve` and `listen_and_serve` take two labels, the listener's and then each
accepted connection's: `listen_and_serve[listener, conn](8080, 64, l, app)`.
Types: `Method`, `Version`, `Headers`, `Request`, `Response`, `Limits`,
`Refusal`, `Framing`, `Head`, `HeadResult`, `BodyState`, `BodyStep`,
`ResponseHead`, `ResponseHeadResult`, `ClientError`. Functions:
`default_limits`, `parse_head`, `body_start`, `body_step`, `header`,
`header_lines`, `has_header`, `set_header`, `add_header`, `response`,
`text_response`, `refusal_response`, `method_not_allowed`, `reason_phrase`,
`encode`, `encode_chunked_head`, `encode_chunk`, `last_chunk`,
`continue_response`, `read_head`, `read_body`, `serve_connection`, `serve`,
`listen_and_serve`, `request_to`, `encode_request`, `parse_response_head`,
`request`. `request[upstream](host, port, req, limits)` opens one connection,
sends the request with `Connection: close`, reads the answer and closes; a
response without a length field is `UntilClose` and read until the server
closes, up to `max_body`. A malformed response is `Malformed` with a 502 refusal. No
TLS above the socket, compression, `Upgrade` or `Content-Encoding`.

### 13.3 `std.router` — routes as data

A table is a `List<Route<a>>` with
`Route<a> = { method: http::Method, path: List<Segment>, endpoint: a }`.
`route(table, method, path)` is pure and answers `NotFound`,
`MethodNotAllowed(methods)` or `Found({endpoint, params})`; the endpoint is a
tag you `match` on. Segments are `Literal(String)`, `Param(String)`,
`Typed(Binding)` and `Rest(String)`; `Typed({name: "id", kind: IntParam})`
matches only an `Int`, read back with `int_param`/`int_param_or` (also
`decimal_` and `bool_`). `conflicts`, `well_formed`, `listing` and `surface`
inspect a table. Paths split before percent-decoding and are never normalized
implicitly (`normalize_path` is explicit).

### 13.4 `std.json`

`Json` has the constructors `Null`, `Bool(Bool)`, `Number(Decimal)`,
`Str(String)`, `Array(List<Json>)` and `Object(Map<String, Json>)`. Numbers are
`Decimal`; objects are maps, so key order is canonical. A codec is
`JsonCodec<a> = {encode: (a) -> Json, decode: (Json) -> Result<a, DecodeError>}`.
Codecs: `int_json`, `string_json`, `bool_json`, `decimal_json`, `float_json`,
`bytes_json`, `unit_json`, `json_json`, and combinators `list_json`,
`option_json`, `result_json`, `map_json`, `string_map_json`. Entry points:
`decode_bytes`, `decode_string`, `encode_bytes`, `encode_string`, `parse`,
`parse_string`, `to_bytes`, `to_string`. `error_to_string` gives
`$.lines[2].unit_price: expected a number, found a string`.

### 13.5 `std.db` — PostgreSQL

```ply
pub nondet effect db {
  read  query[t](s: Stmt, ps: List<Param>)      -> Answer
  write execute[t](s: Stmt, ps: List<Param>)    -> Answer
  write returning[t](s: Stmt, ps: List<Param>)  -> Answer
  write begin(level: Isolation, access: Access) -> Answer
  write commit()                                -> Answer
  write abort()                                 -> Answer
  write rollback(reason: String)                -> Unit
}
```

The resource label is a table (`db.query[items]` is `db.read[items]`).
Transaction control is on the singleton resource, so transactions conflict.
`transaction` handles `rollback`; a `rollback` performed in its body still
reaches the caller's row through `e`, so a handler around a transaction names
it too. SQL errors are values: a `DbError`'s `code` is the SQLSTATE,
`constraint` the constraint a violation names, and `detail` the server's message
and detail. `is_retryable(e)` is true for a serialization failure (`40001`) and
a deadlock (`40P01`). `MemDb` is an in-memory twin (`open`, `step`,
`begin_step`, `commit_step`, `abort_step`).
The driver that runs in Ply reads a statement before it sends it. A statement
that writes, performed as `db.query`, is refused because the endpoints that
perform it would be scheduled as if they only read, and text the reader cannot
account for — a second statement after a `;`, `for update`, `on conflict`, a
call it will not vouch for, a function whose value is not a function of the
program's state — is refused because a footprint it guessed at is a scheduler
that runs two writers beside each other. So is a statement whose tables do not
include the label the call site named: a label is the atom the scheduler
records, and a statement's tables are a function of its text, so a join reaches
tables the label never named. The reader is the language's `std.db`, and the
labels it checks against are the ones the clauses bind (`db.query[*table]`),
which is why a driver can be written in Ply at all.

Each refusal is raised, so a program cannot ignore it; it carries the
runtime-error code, because a library has no raise of its own to name a code
with.

A `db` effect is served by `serve`: `with_server(url, size, body)` reads a
connection string (`server_of`), draws a nonce, and answers the six operations
over `std.pg` — the pool, the transaction scope and the text of every value are
the language's, and the host is left with `net`. The effect is nominal, so a
program that wants a server handles it: `with_server` is how, and
`{db.read[*], db.write[*] | e}` in its signature is what lets one handler answer
every table at once.

A statement outside a transaction takes an idle connection, or opens one while
fewer than `size` are open, and gives it back once the server has answered; with
none to take it is `53300`. A `begin` takes a connection for its transaction, a
`begin` inside it is a savepoint on that connection, and the `commit` or `abort`
that closes the transaction gives the connection back whatever the server
answered. A `commit` the server turned into a rollback, because a statement in
the transaction had failed, is `25P02`, as it is in the twin. Tasks in
transactions at once each hold their own connection. A handler cannot see which
task performs an operation, so `serve` takes an operation to belong to the one
open transaction whose connection is not waiting on the server, and to no
transaction when every one is: a task holding a transaction open must wait on
nothing but its own statements. One that waits on anything else — `task.yield`,
`task.join`, `clock.sleep`, another host operation — leaves its transaction
between statements while other tasks run, so an operation from a task with no
transaction is taken for its own, and one performed while two transactions are
between statements is raised.

A connection that fails is class `08`: `08001` it could not be opened, `08006`
it broke, `08P01` a reply could not be read, `08003` the transaction's
connection is gone and the transaction with it, and `08007` a `commit` whose
outcome is unknown. A server that asks for a password when none was given is
`28000`.

The connection string is `postgres://user[:password]@host[:port]/database`,
and its query carries what every connection the pool opens starts with, told
to the server in the start-up message rather than by a `SET` the reader would
refuse:

| parameter | meaning |
| --- | --- |
| `statement_timeout=MS` | the server cancels a statement running longer (`57014`) |
| `idle_in_transaction_session_timeout=MS` | the server ends a session idle this long inside a transaction |
| `application_name=NAME` | what the server lists the sessions under |
| `sslmode=disable\|prefer` | read and nothing more: TLS to postgres is not wired up |

A timeout is a whole number of milliseconds, `0` for none, as postgres reads
one; any other key, a key given twice, or a timeout past 2147483647 is refused.
The same settings are the `Server` record's `statement_timeout_ms`,
`idle_in_transaction_timeout_ms` and `application_name` for a program that
calls `serve` itself, and `startup_parameters` is what they are sent as.

### 13.6 `std.config`

`pub nondet effect config` has `read get[k](key: String) -> Option<String>` and
`read secret[k](key: String) -> Option<Secret<String>>`. Values are read once at
start-up. The label is a namespace you choose (`config.read[credentials]`).
Describe keys with `spec`, `required`, `optional` and `with_default`, and pass
the `ConfigSpec` with `--config-schema`. A key the schema declares secret is
readable only through `config.secret`; without a schema no key is secret.

### 13.7 `std.trace`

```ply
pub nondet effect trace {
  write event[c](level: Level, name: String, fields: Fields)   -> Unit
  write enter[c](name: String, fields: Fields)                 -> Span
  write exit[c](span: Span, outcome: Outcome)                  -> Unit
  write count[c](name: String, delta: Int, fields: Fields)     -> Unit
  write gauge[c](name: String, value: Decimal, fields: Fields) -> Unit
  write time[c](name: String, micros: Int, fields: Fields)     -> Unit
}
```

The label is a channel; every perform is written at its call site. Tests collect
records with `Sink` and `event_step`, `enter_step`, `exit_step`, `count_step`,
`gauge_step`, `time_step`, `drain`, `named`, `on_channel`, `counter_total`.

### 13.8 `std.signal`

`pub nondet effect signal` has `read stopping() -> Bool` and
`read deadline_ms() -> Int`. It is never bound under `ply test`, even with
`--host` (`E0424`). Handle it over a `Stop` value with `running()`,
`draining(ms)`, `stopping_step`, `deadline_step` and `has_time_for`.

### 13.9 `std.process`

```ply
pub type Var = { name: String, value: String }
pub type Ended = Exited(Int) | Signalled(Int)
pub type Finished = { ended: Ended, out: Bytes, err: Bytes }
pub type Output = Keep | Discard | Inherit | Lines | File(String)
pub type Io = { input: Bool, out: Output, err: Output }
pub type Signal = Hangup | Interrupt | Terminate | Kill
pub type Heard = Said(String) | Quiet | Closed

pub nondet effect process {
  read  args[p]()             -> List<String>
  read  bound[e]()            -> Bool
  write out[p](text: String)  -> Unit
  write err[p](text: String)  -> Unit
  write line[p]()             -> Option<String>
  write exit[p](code: Int)    -> Unit
  write spawn[e](args: List<String>, dir: String, env: List<Var>) -> Finished
  write start[e](args: List<String>, dir: String, env: List<Var>, io: Io) -> Result<Int, String>
  write wait[e](child: Int, timeout_ms: Int)        -> Option<Finished>
  write signal[e](child: Int, signal: Signal)       -> Bool
  write input[e](child: Int, bytes: Bytes)          -> Bool
  write end_input[e](child: Int)                    -> Unit
  write output_line[e](child: Int, timeout_ms: Int) -> Heard
}
```

The label names the process, `[proc]` by convention. `args` answers what
followed `--` on the `ply run` command line, `out` and `err` each write one
line, `line` reads one line of the program's standard input without its line
ending and answers `None` at end of input (a `write`, because each line is
consumed and two readers of one input race for it), and `exit` ends the program
there: nothing after it runs, no value is
printed, and `ply run` exits with the code (`0` to `125`, else `E0502`). These
are bound only by `ply run --host`; `ply test` withholds them, even with
`--host` (`E0424`). The operations whose label is an executable — `bound`,
`spawn`, `start` and those on a started child — are bound by `ply test --host`
too: a label `--exec` does not name is unbound (`E0456`), and `bound` answers
`false` for it. Under `ply run --json` the lines `out` writes go to
stderr, so stdout still carries the one object. Handle it over a `Captured`
value: `captured(args)`, `args_step`, `out_step`, `err_step`, `line_step` and `exit_step` keep each line,
hand out `with_input`'s scripted input lines, and the first exit code; a clause `process.exit[proc](c) resume k -> ...` that never calls `k`
ends the handled body as the host would.

`spawn` starts another program and waits for it. Its label is not the process
but the executable: `--exec cc=/usr/bin/cc` binds one program to `cc`, and
`process.spawn[cc](..)` can start that program and no other. A label with no
executable bound is `E0456`, and an `--exec` path that is missing, is not a file
or has no execute bit is `E0457` before anything runs. Nothing in the call names
a program, so the run decides what a footprint's `process.spawn[cc]` may do.
`bound` answers whether the run bound a program to its label, so a program that
can do without one asks `process.bound[cc]()` rather than ending at `E0456`; the
table is settled before anything runs, so the answer holds for the whole run.

`args` is the argument vector after the program; `dir` is the working
directory, and `""` is the run's own. `env` is the *whole* environment: a spawn
inherits none of the run's, so what the child reads is in the program's text and
its configuration rather than in the shell that started `ply`. A spawn captures
both streams whole, and a stream over 64MiB is `E0458`; its standard input is
empty, and the run's own is what `line` reads. A program that reads a child as it
runs, writes to it or stops it starts it instead.
`Exited(code)` is the program's own answer and `Signalled(n)` the signal that
killed it; neither is a diagnostic, because what a compiler says about a source
file is a value the driver reads. The label is the capability and nothing
narrower: a spawned process does what that program can do, so `dir` is not
confined the way an `fs` path is. Handle it over a `Runs` value: `runs(replies)`
hands out planned `Finished` values in order and records each `Launch`; a spawn
with no reply planned answers `Exited(127)`, as a shell does for a command it
could not run. Build replies with `exited(code, out, err)` and
`signalled(signal, out, err)`, and read one back with `exit_code`.
`runs_bound_step` answers `bound`: `true` once a reply is planned, unless the
test sets the twin's `bound`, and no spawn changes it.

`start` launches a child beside the program, by the same label, `dir` and `env`
rules, and answers its handle — an `Int`, as a socket's is — or `Err` with why
it could not start, such as a missing directory or a `File` that cannot be
opened. With `io.input` the program writes the child's input: `input` answers
`false` once that input is closed, by `end_input` or by the child, or once the
child has ended; without it the input is empty and `input` always answers
`false`. Each output stream goes where its `Output` says. `Keep` holds it for
`wait`, drained as the child writes, so a chatty child never blocks on a full
pipe. `Lines` hands it over a line at a time: `output_line` answers `Said(line)`
from any `Lines` stream in the order the host read them, without the line's
`\n` or `\r\n` and with anything not UTF-8 read as U+FFFD, `Quiet` when the
timeout passes first, and `Closed` once every `Lines` stream has ended and each
line has been read — at once for a child with none. `Inherit` writes where the
program's own `out` and `err` do, `Discard` drops it, and `File(path)` writes it
to `path`, relative to `dir`, from empty; one path named for both streams is
shared, as `2>&1` shares it. What the host holds of a stream, kept or unread, is
bounded at 64MiB: the excess is dropped, and the next `wait` or `output_line` is
`E0458`.

`wait` answers `Some(Finished)` once the child has ended and every stream the
host holds has reached its end — a grandchild that keeps one open keeps `wait`
waiting — with a `Keep` stream whole and a `Lines` stream's unread lines. It
reaps the child, and the handle is spent. It answers `None` when the timeout
passes first; a negative timeout, here and in `output_line`, waits for as long
as it takes. `signal` delivers `SIGHUP`, `SIGINT`, `SIGTERM` or `SIGKILL` and
answers `false` when the child had already ended. None of these stops the
machine while it waits. A handle `start` never answered, one already waited on,
and one used under another label than it was started under are each `E0502`.

No child outlives the run that started it. However the run ends — its entry
returns, `exit`, a raise, a drain that ran out of time, a second `SIGINT` or
`SIGTERM` — every child still running is killed with `SIGKILL` and reaped by
the host, so a program that stops what it started does so for its own reasons,
not to avoid a leak.

Handle the children over a `Children` value: `children(planned)` hands each
`start` the next planned `Script` — `script(polls, finished, heard)`: how many
`wait`s find the child running, the `Finished` the next one answers, and what
`output_line` hears in order, `Closed` once that runs out — and a start with
none planned answers `Err`. `start_step`, `wait_step`, `signal_step`,
`input_step` and `output_line_step` each answer an `Answered` of the twin and
what the host would have said, `end_input_step` answers the twin, `bound_step`
answers `bound` as `runs_bound_step` does, `true` once a script is planned, and
each `Child` records its launch, the bytes written to it, whether its input is
open and the signals it was sent. A `Kill` ends a scripted child, so the next `wait` hands it back, and
a spent or unknown handle panics, as the host refuses one.

### 13.10 `std.time`

```ply
pub nondet effect time {
  read now_ms()           -> Int
  read elapsed_ms()       -> Int
  write sleep_ms(ms: Int) -> Unit
}
pub fn deadline_in(ms: Int) -> Int / {time.elapsed_ms}
pub fn expired(deadline: Int) -> Bool / {time.elapsed_ms}
pub fn since(started: Int) -> Int / {time.elapsed_ms}
```

The host's real time, in two readings and a wait, none of them a function of the
program state, so a definition that takes one is `nondet` and a `test` over it
must handle it. `now_ms` is milliseconds since the Unix epoch: a date to stamp a
record with, and nothing to measure with, since the system clock can be set
backwards. `elapsed_ms` counts from the moment the run's host was built and never
goes back, so the difference of two readings is a span; a single reading means
nothing on its own. Handle both over a `Ticks` value: `ticks(wall, mono)` reads
each list in order through `now_step` and `elapsed_step`, each answering a `Tick`
of the reading and the ticks left, and repeats the last reading once a list runs
out. `sleep_ms` parks the thread that performs it for that many milliseconds — a
span no clock can run backwards over, so a negative one is no wait at all — and a
test handles it with a clause of its own, so a program that polls is tested
without waiting.

This is not the language's `clock` (§9), which is virtual time: `clock.sleep`
arms a timer the scheduler advances, and `clock.now` reads where the schedule has
got to. `time` is what the operating system says, and nothing advances it. They
are named apart because they are different things, and a program that wants a
deadline inside `simulate { .. }` wants `clock`.

### 13.11 `std.fs`

```ply
pub type Kind = | File | Dir | Symlink | Missing

pub nondet effect fs {
  read  read_file[r](path: String) -> Option<Bytes>
  read  read_at[r](path: String, offset: Int, len: Int) -> Option<Bytes>
  read  list_dir[r](path: String) -> Option<List<String>>
  read  kind[r](path: String) -> Kind
  read  resolved[r](path: String) -> Kind
  read  exists[r](path: String) -> Bool
  read  file_size[r](path: String) -> Option<Int>
  read  modified_ms[r](path: String) -> Option<Int>
  write write_file[r](path: String, body: Bytes) -> Bool
  write append[r](path: String, body: Bytes) -> Option<Int>
  write create_dir[r](path: String) -> Bool
  write remove[r](path: String) -> Bool
  write rename[r](from: String, to: String) -> Bool
  write sync[r](path: String) -> Bool
  write lock[r](path: String) -> Bool
  write unlock[r](path: String) -> Bool
}
```

The label is a root bound with `--fs NAME=PATH`. Unbound label: `E0451`; a path
escaping its root (`..`, absolute, or a symlink outside): `E0452`. Different
roots do not conflict. `list_dir` is one level, `rename` stays in one root.
`kind` says what a path names in one call and does not follow a symlink, so a
walk can pass one over; `Missing` is also what this run cannot read. Every other
operation follows one.

`append` and `read_at` are what make a file a log rather than a value. `append`
costs the size of what is new rather than the size of the file, creates the file
if it is not there, and answers the offset the bytes landed at — so a writer
needs no separate call to find out where its frame went, and a second appender
racing it moves neither the bytes nor the answer. `read_at` reads `len` bytes
from `offset`. Together they carry an append-only cache: frames appended and
their offsets recorded, read back one frame at a time instead of a file at a
time.

`read_at` answers **what is there**, which may be less than was asked for: a
range that runs past the end is short, and one that starts at or past the end is
`kind` does not follow a symlink: it answers `Symlink` for one, so a walk that
must not leave the root can refuse it. `resolved` follows, and answers what the
path names at the end — `File`, `Dir` or `Missing`, never `Symlink`. Following
cannot leave the root either way, because a path resolving outside it is refused
before any operation runs.

empty. That is deliberate, and the one place `std.fs` parts company with
`bytes_slice`, which never clamps a range: a value's length is known and fixed,
while a file's is neither, so a reader holding an offset it recorded earlier
would otherwise be raised at for a file that shrank — and a shrunken cache is a
warning to its reader, not a fault in the program. Compare the answer's length
with the one you asked for to tell a short read from a whole one. A negative
offset or length is the other way round: no file can answer it, so it is
arithmetic that went wrong and raises `E0502`. Two reads of one range may differ;
the effect is `nondet` and a file can change under it.

`E0453` bounds one call, not a file: `read_file` refuses a file whose whole
contents would be the answer, and `read_at` refuses a `len`, each above 64 MiB.
A larger file is read a range at a time.

`sync` makes what was written durable, and is what a paired cache needs to be
honest rather than lucky. A write reaches the page cache, not the disk, and
`rename` orders the *name* rather than the bytes behind it — so an index renamed
into place can name frames a crash then loses. Sync the data file before the
index that names it. On a directory it flushes the names in it, which is what
makes a `rename` itself durable. `false` means the path names nothing to flush.

`lock` and `unlock` serialise a read-merge-write across processes, which a
`rename` alone cannot: two runs that flush a cache at once lose one of them.
The path names the lock file itself, under the same root as what it guards.
`lock` answers `true` when it created that file, and `false` when a holder still
had it after two seconds of waiting — contention is a value, never a diagnostic.
There is no reentrancy: a run that asks twice for a lock it already holds waits
the same two seconds and gets `false`. A lock file older than thirty seconds is
one whose holder died, and the next taker removes it and takes it. `unlock`
releases the claim this run took, answering `true`, and answers `false` — and
removes nothing — for a lock it does not hold, so one run cannot break another's.
A run that dies holding a lock leaves the file behind, and the stale age is what
recovers it.

The twin is `MemFs` (`mem_empty`, `mem_of`, `mem_read`, `mem_write`, `mem_list`,
`mem_kind`, `mem_exists`, `mem_size`, `mem_create_dir`, `mem_remove`,
`mem_rename`, `mem_modified`, `mem_read_at`, `mem_append`, `mem_sync`,
`mem_lock`, `mem_unlock`); it holds no symlinks, so `mem_kind` never answers
`Symlink`, it has no wall clock, so no lock in it goes stale, and it was never on
a disk, so `mem_sync` only says whether the path names something. `mem_append`
answers an `Appended` of the tree and the offset. A test imports both `std.fs`
and `std.fs (fs)` to name the module and the effect.

### 13.12 `std.path`

```ply
pub fn join(dir: String, name: String) -> String
pub fn file_name(path: String) -> String
pub fn stem(path: String) -> String
pub fn with_extension(path: String, ext: String) -> String
pub fn parent(path: String) -> String
pub fn extension(path: String) -> Option<String>
pub fn components(path: String) -> List<String>
pub fn resolve(base: String, path: String) -> String
pub fn strip_dot(path: String) -> String
```

Text, not a filesystem: nothing here performs an effect. `join` places exactly
one separator and adds none for a root spelled `"."` or `""`. `file_name` is the
last segment, `""` for a path ending in a separator, and `stem` is that name
with its extension taken off, so `stem("a.tar.gz")` is `"a.tar"` and
`with_extension` puts another one back. `parent` is the directory holding the
path, `"."` for a name with no separator and for a root, so
`join(parent(p), file_name(p))` puts back what the two took apart. `extension`
follows the last dot of the file name, and a dotfile has none. `components` is
the separators' parts, a leading separator an empty first segment.
`resolve(base, p)` is `p` against `base`, an absolute `p` as itself and a
relative one appended and normalized; it reads no directory, so `..` is resolved
by segment rather than by what is there. `strip_dot`
removes a leading `./`, so `./m.ply` and `m.ply` are one key in a set.

### 13.13 `std.hash`

```ply
pub fn blake3(input: Bytes) -> Bytes
pub fn sha256(input: Bytes) -> Bytes
pub fn hmac_sha256(key: Bytes, message: Bytes) -> Bytes
pub fn pbkdf2_sha256(password: Bytes, salt: Bytes, iterations: Int) -> Bytes
```

`blake3` and `sha256` answer 32 bytes. `hmac_sha256` is HMAC over SHA-256 as RFC
2104 defines it, and `pbkdf2_sha256` is its single-block PBKDF2: thirty-two
bytes, which is the salted password SCRAM asks for and the only length anything
here needs.

All of it is written in Ply, and the vectors the SHA-256 standard and RFC 4231
publish are the tests. It is slow — a compression round walks a list of words
rather than living in scalars — so use it for small inputs: a key, a proof, a
nonce, not a file.

### 13.14 `std.bytes`

```ply
pub fn u32_le(n: Int) -> Bytes
pub fn u64_le(n: Int) -> Bytes
pub fn u32_at(b: Bytes, at: Int) -> Option<Int>
pub fn u64_at(b: Bytes, at: Int) -> Option<Int>
pub fn u16_be(n: Int) -> Bytes
pub fn u32_be(n: Int) -> Bytes
pub fn u64_be(n: Int) -> Bytes
pub fn i16_be(n: Int) -> Bytes
pub fn i32_be(n: Int) -> Bytes
pub fn i64_be(n: Int) -> Bytes
pub fn u16_be_at(b: Bytes, at: Int) -> Option<Int>
pub fn u32_be_at(b: Bytes, at: Int) -> Option<Int>
pub fn u64_be_at(b: Bytes, at: Int) -> Option<Int>
pub fn i16_be_at(b: Bytes, at: Int) -> Option<Int>
pub fn i32_be_at(b: Bytes, at: Int) -> Option<Int>
pub fn i64_be_at(b: Bytes, at: Int) -> Option<Int>
pub fn slice_at(b: Bytes, at: Int, n: Int) -> Option<Bytes>
pub fn join(pieces: List<Bytes>, sep: Bytes) -> Bytes
pub fn compare(a: Bytes, b: Bytes) -> Ordering
pub fn repeat(b: Bytes, n: Int) -> Bytes
pub fn hex_of(b: Bytes) -> String
pub fn bytes_of_hex(text: String) -> Bytes
pub fn int_of_ascii(b: Bytes) -> Option<Int>
```

Integers in a byte string, little-endian and big-endian: how a binary format and
a network protocol are written and read back. The writers take the low two, four
or eight bytes of `n`; the `i` writers are the `u` ones' bytes, since two's
complement is the representation, and exist so a call site says which it meant.
Nothing here raises: a read past either end is `None`, and so is a `u64` past
what an `Int` holds, so an answer is never a negative length. The `i` readers
carry the sign, and `i64_be_at` answers for every `Int`. `int_of_ascii` reads a
decimal integer written in ASCII — an optional `-`, then digits, and nothing else
— and is `None` for anything else and for a number past what an `Int` holds; it
is the one integer parser the shipped modules share, and `std.string`'s
`int_of_string` is it over a `String`.

### 13.15 `std.pkg`

```ply
type Version = { major: Int, minor: Int, patch: Int }
type Source = | Path(String) | Git(String, String) | Registry
type Dep = { name: String, prefix: Option<String>, min: Version, source: Source }
type Manifest = {
  name: String,
  version: Version,
  prefix: Option<String>,
  runtime: Version,
  dependencies: List<Dep>,
  entry: Option<String>,
}
type Release = { version: Version, digest: String, yanked: Bool, runtime: Version }
type Index = { name: String, versions: List<Release> }
```

The package manifest as typed data: a `ply.pkg` file is one literal of
`Manifest`, checked with the same judgment §3.1 states for parameter defaults
(`E0129`–`E0131` when it is not). Every manifest type derives `json`; `Version`
also derives `ord`, ordered major, then minor, then patch. `prefix_of` and
`entry_of` answer the defaults (`name` and `main`), `render_version` writes a
version dotted and `parse_version` reads one back (three counts, no leading
zero, nothing else). `Index` is a registry's `index.json` (§15.1): every
published `Release` of one package, newest last, read and written by
`index_json` (`release_json` for one entry), with each version as its dotted
text.

### 13.16 `std.pg` — the postgres wire protocol

```ply
pub fn startup(user: String, database: String, parameters: List<Set>) -> Bytes
pub fn query(sql: String) -> Bytes
pub fn parse(statement: String, sql: String, param_types: List<Int>) -> Bytes
pub fn bind(portal: String, statement: String, params: List<Option<String>>) -> Bytes
pub fn describe(statement: Bool, name: String) -> Bytes
pub fn execute(portal: String, max_rows: Int) -> Bytes
pub fn sync() -> Bytes
pub fn terminate() -> Bytes
pub fn read(buf: Bytes) -> Frames

pub fn connect<[l]>(
  host: String, port: Int, user: String, database: String, parameters: List<Set>,
  password: Option<String>, nonce: String, client: Client,
) -> Result<Session, ClientError> / {net.connect[l], net.send[l], net.recv[l], net.close[l]}
pub fn simple_query<[l]>(s: Session, sql: String, client: Client)
  -> Result<Reply, ClientError> / {net.send[l], net.recv[l]}
pub fn extended_query<[l]>(s: Session, sql: String, params: List<Option<String>>, client: Client)
  -> Result<Reply, ClientError> / {net.send[l], net.recv[l]}
pub fn finish<[l]>(s: Session, client: Client) -> Unit / {net.send[l], net.close[l]}

pub fn scram_first(user: String, nonce: String) -> String
pub fn scram_first_bare(user: String, nonce: String) -> String
pub fn scram_challenge(server_first: String) -> Option<Challenge>
pub fn scram_final(password: String, first_bare: String, server_first: String, challenge: Challenge)
  -> Proof
pub fn scram_verify(server_final: String, expected: Bytes) -> Bool
```

The protocol as framing and nothing else. Every front-end message after start-up
is a kind byte, an `Int32` length that counts itself, and a body; `startup` is
the exception, its length first, because the server has agreed no protocol
version yet. `ssl_request` and `cancel_request` are the two other unframed
messages. Values travel as text in both directions, so a parameter is the text
the server would have printed and a column is the text it printed: this layer
never decodes a value.

`read` takes what a socket returned and answers the whole messages in it plus
the bytes that are not yet one, so a short read is not an error. The back-end
readers are `auth_code` and `auth_body`, `ready_status`, `parameter_status`,
`backend_key`, `row_description`, `data_row`, `command_tag`, `parameter_types`
and `diagnostic_fields` — the last shared by `ErrorResponse` and
`NoticeResponse`, with `field_of` for one field such as the SQLSTATE under `C`.
A kind this module does not name is kept as `Other(byte)` rather than dropped.

`connect` opens the socket and gets to where the server will answer a query:
start-up, whatever authentication it asks for, its parameters, and
`ReadyForQuery`; the parameters are left on the session and `setting` reads one.
`parameters` are run-time settings the start-up message carries after `user`
and `database` (`statement_timeout`, say), which the server applies to the
session before it answers.
The client's SCRAM nonce is the caller's to draw, so a run's `random` seed decides
it and a test can fix it. Authentication is answered for `AuthenticationOk`, a
clear-text password, and SCRAM-SHA-256; md5 is refused with a message naming it.
`simple_query` runs one statement, `extended_query` runs one with parameters
bound as text, and both answer the columns, the rows and the command tag; NULL is
`None` and every column is `Some` bytes. A statement the server refuses is
`Rejected(session, server)`, which carries the connection back because it is
still usable; one the server hangs up after, as it does after a `FATAL` error, is
`Refused(server)`. A `Server` is the refusal's `severity`, its SQLSTATE `code` —
what a program branches on — its `message` and `detail`, and the `constraint` a
violation names. A session's `status` is where its transaction stood at the last
`ReadyForQuery`: `Idle`, `InTransaction` or `FailedTransaction`. `finish` sends
`Terminate` and closes.

The SCRAM steps are exposed because they are pure: `scram_first_bare` writes the
message the client's nonce goes in, `scram_challenge` reads the server's first
message, `scram_final` answers the reply and the server signature to expect, and
`scram_verify` checks the signature the server sent. RFC 7677's worked example —
its nonce, salt, 4096 iterations, client proof and server signature — is the
test.

A connection is `std.net`'s: `connect`, `send_all`, `drain`. Which statements to
send, what a transaction is and when to retry are `std.db`'s.

### 13.17 `std.certgen`

```ply
pub nondet effect certgen {
  read issue() -> Issued
}

pub type Issued = {
  certificate: String,
  key: String,
  der: Bytes,
  fingerprint: String,
}

pub fn localhost() -> Issued / {certgen.issue}
```

A throwaway self-signed certificate for `localhost`, generated where the run
is, so nothing checked in is either expired or shipping its private key.
`--tls NAME=CERT,KEY` wants the two PEM strings as files, and `der` is what a
client trusts exactly this one by. Each call makes a new key. It is bound under
`ply run --host`, and a test handles it over `canned(certificate, key, der,
fingerprint)`.

### 13.18 `std.random`

```ply
pub nondet effect entropy {
  read next() -> Int
  read below(n: Int) -> Int
}
pub fn next() -> Int / {entropy.next}
pub fn below(n: Int) -> Int / {entropy.below}
pub fn nonce() -> String / {entropy.next}
pub type Rand = { root: Int, key: Bytes, counter: Int }
pub fn rand(root: Int) -> Rand
pub fn rand_keyed(root: Int, key: Bytes) -> Rand
pub fn rand_int(r: Rand) -> { value: Int, rand: Rand }
pub fn rand_u64(r: Rand) -> { value: U64, rand: Rand }
pub fn rand_bytes(r: Rand, n: Int) -> { bytes: Bytes, rand: Rand }
pub fn rand_below(r: Rand, n: Int) -> Option<{ value: Int, rand: Rand }>
```

The randomness a run that is not simulated has: `next` is sixty-three bits and
never negative, `below` is uniform below a bound above zero (the host draws
again rather than folding a value into range with a remainder, which would make
the low values likelier), and `nonce` is two draws, which is what a SASL
exchange and a cache key want.

A `Rand` is the third thing: a value, drawn by a pure function of its root, for
code that wants a seeded stream without a `simulate` region — a fuzz case, a
shuffled order, a scatter. It is counter-mode BLAKE3, the construction ADR 0006
chose for the simulation's *own* stream (`ply-eval`'s `sim::Stream`), so a result
is a function of the root on every machine and every version; `rand_int` is
sixty-three bits like the effect's, `rand_u64` all sixty-four of the same draw,
`rand_bytes` advances by the blocks it needs, and `rand_below` is uniform by
rejection rather than by a remainder. A key names one stream among many from a
root — one claim's draws apart from every other's — and no two keys, nor a key
and the simulation, share a block; `rand` is the empty key, the simulation's own
stream. The counter is a field, so a stream may start at any draw.

`simulate` answers the *prelude's* `random` with values a seed decides, which is
what makes a simulation reproducible; that is why it is the scheduler's and not
the host's. A run that is not simulated draws here, and `--host` binds it.

### 13.19 `std.uuid`

```ply
pub type Uuid = { octets: Bytes }
pub fn uuid_render(u: Uuid) -> String
pub fn uuid_parse(text: String) -> Option<Uuid>
pub fn uuid_v4() -> Uuid / {entropy.next}
```

A 128-bit identifier as its sixteen octets. `uuid_render` writes the canonical
lower-case `8-4-4-4-12` form; `uuid_parse` reads it and accepts upper case, as
RFC 9562 requires of a reader, while anything that is not that form is `None`
rather than a raise. `uuid_v4` draws two machine words of `std.random` entropy
and overwrites the version and variant bits, so a test pins it by handling
`entropy.next`.

### 13.20 `std.base64`

```ply
pub fn base64_encode(data: Bytes) -> String
pub fn base64_decode(text: String) -> Option<Bytes>
pub fn base64url_encode(data: Bytes) -> String
pub fn base64url_decode(text: String) -> Option<Bytes>
```

RFC 4648: the standard alphabet (`A-Za-z0-9+/`) with padding, and the URL-safe
alphabet (`A-Za-z0-9-_`) without, which is the form a JWT and a URL carry. A
decoder is total and answers `None` for what the RFC does not call canonical — a
padding character anywhere but the end, a length not divisible by four (for the
padded form), a letter outside the alphabet, or bits left over in the final
group. So a decode of an encode is the identity, and so is an encode of a
decode.

### 13.21 `std.url`

```ply
pub fn url_encode(text: String) -> String
pub fn url_decode(text: String) -> Option<String>
pub fn form_encode(text: String) -> String
pub fn form_decode(text: String) -> Option<String>
pub type Pair = { name: String, value: String }
pub fn query_parse(query: String) -> Option<List<Pair>>
pub fn query_build(pairs: List<Pair>) -> String
```

RFC 3986 percent-encoding — every byte outside `A-Za-z0-9-._~` as `%XX`
— and the `application/x-www-form-urlencoded` dialect, which spells a space `+`
and is what a query string and a form body carry. A decoder is total and answers
`None` for a `%` not followed by two hex digits or for bytes that are not UTF-8.
`query_parse` splits a query string into pairs in the order it names them (an
empty part is passed over, a part with no `=` is a name with an empty value) and
`query_build` spells them back with `form_encode` on both sides.

### 13.22 `std.csv`

```ply
pub fn csv_parse(text: String) -> Option<List<List<String>>>
pub fn csv_build(rows: List<List<String>>) -> String
```

RFC 4180: records of fields, a field quoted with `"` exactly when it holds a
comma, a quote, CR or LF, and an embedded quote doubled. A reader is total and
answers `None` for what the RFC does not name — an unterminated quoted field,
text after a closing quote, a quote inside an unquoted field, a bare carriage
return. A bare line feed also ends a record, because files in the world have
one; the writer always writes CRLF. A trailing line break adds no record and an
empty line is one empty field.

### 13.23 `std.msgpack`

```ply
pub type Value =
  | Nil | Flag(Bool) | Int(Int) | Float(Float)
  | Text(String) | Bits(Bytes) | Items(List<Value>) | Fields(List<Field>)
pub type Field = { key: Value, value: Value }
pub fn msgpack_encode(v: Value) -> Bytes
pub fn msgpack_decode(data: Bytes) -> Option<Value>
```

MessagePack: the compact binary form of the same data JSON carries. The subset
is nil, booleans, integers in every width, float64, UTF-8 text, binary, arrays
and maps; the writer emits the shortest *signed* encoding a value fits, and the
reader accepts the `uint` widths too. A reader is total and answers `None` for a
truncated input, a length that claims more bytes than remain, a value nested
past 64, a `float32` or an extension type, or a `uint64` above `i63` — checking
every length against what is left before walking it, so a hostile header cannot
make it loop.

### 13.24 `std.parse`

```ply
pub type Step<a> = { value: a, at: Int }
pub type Parser<a> = (String, Int) -> Option<Step<a>>
pub fn run<a>(p: Parser<a>, input: String) -> Option<a>
pub fn at<a>(p: Parser<a>, input: String, position: Int) -> Option<Step<a>>
pub fn satisfy(ok: (String) -> Bool) -> Parser<String>
pub fn char(c: String) -> Parser<String>
pub fn text(t: String) -> Parser<String>
pub fn map<a, b>(p: Parser<a>, f: (a) -> b) -> Parser<b>
pub fn and_then<a, b>(p: Parser<a>, f: (a) -> Parser<b>) -> Parser<b>
pub fn or_else<a>(p: Parser<a>, q: Parser<a>) -> Parser<a>
pub fn many<a>(p: Parser<a>) -> Parser<List<a>>
pub fn many1<a>(p: Parser<a>) -> Parser<List<a>>
pub fn opt<a>(p: Parser<a>) -> Parser<Option<a>>
pub fn sep_by<a, b>(p: Parser<a>, sep: Parser<b>) -> Parser<List<a>>
pub fn after<s, a>(s: Parser<s>, p: Parser<a>) -> Parser<a>
pub fn before<a, s>(p: Parser<a>, s: Parser<s>) -> Parser<s>
pub fn digit() -> Parser<String>
pub fn digits() -> Parser<String>
pub fn spaces() -> Parser<String>
pub fn word(stop: String) -> Parser<String>
```

A parser is a value: the input, a position, and a `Step` or `None`. `run` wants
the whole input consumed and `at` reads a prefix. `or_else` runs its second
parser from the same position as the first, so there is no half-consumed state
to unwind; `many` stops when its parser consumes nothing, so a parser that can
match the empty string still cannot loop, and nothing needs a fuel argument.
This is the shape `std.json`, `std.db` and `std.http` already write by hand, as
a module a user's parser can share.

### 13.25 `std.string`

```ply
pub fn join(parts: List<String>, sep: String) -> String
pub fn is_empty(text: String) -> Bool
pub fn replace(text: String, from: String, to: String) -> String
pub fn replace_first(text: String, from: String, to: String) -> String
pub fn split_once(text: String, sep: String) -> Option<{ head: String, tail: String }>
pub fn trim(text: String) -> String
pub fn trim_start(text: String) -> String
pub fn trim_end(text: String) -> String
pub fn to_lower(text: String) -> String
pub fn to_upper(text: String) -> String
pub fn repeat(text: String, n: Int) -> String
pub fn pad_left(text: String, width: Int, fill: String) -> String
pub fn pad_right(text: String, width: Int, fill: String) -> String
pub fn split(text: String, sep: String) -> List<String>
pub fn contains(text: String, needle: String) -> Bool
pub fn starts_with(text: String, prefix: String) -> Bool
pub fn ends_with(text: String, suffix: String) -> Bool
pub fn index_of(text: String, needle: String) -> Option<Int>
pub fn lines(text: String) -> List<String>
pub fn words(text: String) -> List<String>
pub fn count(text: String, needle: String) -> Int
pub fn capitalize(text: String) -> String
pub fn int_of_string(text: String) -> Option<Int>
pub fn code_points(text: String) -> List<Int>
pub fn of_code_points(points: List<Int>) -> Option<String>
```

The whole the prelude's string builtins do not make: `join` (which three shipped
modules were each writing for themselves), `replace`, trim, ASCII case fold,
repeat and pad, and then the two names a caller reaches for that the prelude
spells `string_split`/`string_contains`/`string_find`. `index_of` answers `None`
rather than the prelude's `-1`, which is not a position. `lines` takes a trailing
`\r` off each line, so a CRLF file reads as an LF one, and the empty text has no
lines rather than one empty line; `words` is the runs that are not whitespace and
never empty. `count` does not overlap. Every one is total — `replace` with an
empty needle is the text unchanged rather than a loop, and the case fold touches
`A-Z`/`a-z` and leaves every other character as it is. `std.bytes.join` is the
same operation over `Bytes`. `int_of_string` is `std.bytes.int_of_ascii` over the
text: `"7.5"`, `" 7"` and `"+7"` are `None`. `code_points` is each character's
Unicode scalar value, and `of_code_points` spells them back, `None` when one is
negative, a surrogate (`U+D800` to `U+DFFF`) or past `U+10FFFF`.

### 13.26 `std.option`

```ply
pub fn option_map<a, b>(o: Option<a>, f: (a) -> b) -> Option<b>
pub fn option_and_then<a, b>(o: Option<a>, f: (a) -> Option<b>) -> Option<b>
pub fn option_filter<a>(o: Option<a>, ok: (a) -> Bool) -> Option<a>
pub fn option_or<a>(o: Option<a>, fallback: Option<a>) -> Option<a>
pub fn option_unwrap_or<a>(o: Option<a>, fallback: a) -> a
pub fn option_expect<a>(o: Option<a>, message: String) -> a
pub fn option_is_some<a>(o: Option<a>) -> Bool
pub fn option_is_none<a>(o: Option<a>) -> Bool
pub fn option_ok_or<a, e>(o: Option<a>, err: e) -> Result<a, e>
pub fn option_or_else<a>(o: Option<a>, fallback: () -> Option<a>) -> Option<a>
pub fn option_unwrap_or_else<a>(o: Option<a>, fallback: () -> a) -> a
pub fn option_map_or<a, b>(o: Option<a>, fallback: b, f: (a) -> b) -> b
```

`Option`'s constructors and `?` are the prelude's; this is the chain a caller reads
with. Each is `option_`-prefixed because `map` and `and_then` are names a program
already has (for lists, and for `std.parse`), and an unqualified `map` that
silently took an `Option` would be a trap. `option_expect` is the one place an
absent value is a defect, so its message says why it cannot happen.

### 13.27 `std.result`

```ply
pub fn result_map<a, b, e>(r: Result<a, e>, f: (a) -> b) -> Result<b, e>
pub fn result_map_err<a, e, f>(r: Result<a, e>, g: (e) -> f) -> Result<a, f>
pub fn result_and_then<a, b, e>(r: Result<a, e>, f: (a) -> Result<b, e>) -> Result<b, e>
pub fn result_unwrap_or<a, e>(r: Result<a, e>, fallback: a) -> a
pub fn result_expect<a, e>(r: Result<a, e>, message: String) -> a
pub fn result_ok<a, e>(r: Result<a, e>) -> Option<a>
pub fn result_err<a, e>(r: Result<a, e>) -> Option<e>
pub fn result_is_ok<a, e>(r: Result<a, e>) -> Bool
pub fn result_is_err<a, e>(r: Result<a, e>) -> Bool
pub fn result_or_else<a, e>(r: Result<a, e>, fallback: (e) -> Result<a, e>) -> Result<a, e>
pub fn result_unwrap_or_else<a, e>(r: Result<a, e>, fallback: (e) -> a) -> a
pub fn result_map_or<a, b, e>(r: Result<a, e>, fallback: b, f: (a) -> b) -> b
```

The same shape over `Ok`/`Err`. `result_map_err` is how a low-level failure
becomes the one a caller names.

### 13.28 `std.math`

```ply
pub fn min_int() -> Int
pub fn max_int() -> Int
pub fn abs(n: Int) -> Int
pub fn sign(n: Int) -> Int
pub fn clamp(n: Int, lo: Int, hi: Int) -> Int
pub fn even(n: Int) -> Bool
pub fn odd(n: Int) -> Bool
pub fn gcd(a: Int, b: Int) -> Int
pub fn lcm(a: Int, b: Int) -> Int
pub fn pow(base: Int, exponent: Int) -> Int
pub fn is_prime(n: Int) -> Bool
pub fn factorial(n: Int) -> Int
pub fn isqrt(n: Int) -> Int
```

`min` and `max` are prelude builtins and stay there. Everything here is `Int`,
which is `i64`, and the arithmetic wraps at that width rather than raising, so
`pow` and `abs(min_int())` answer a wrapped value — a checked variant would have
to say what it answers instead, and that belongs with `B10`'s numeric
predicates. `gcd` and `lcm` are never negative, and `gcd(0, 0)` is `0`.
`is_prime` says no for zero, one and every negative, `factorial` is `1` at and
below one, and `isqrt` is the greatest `r` with `r * r <= n` — `0` for a negative
`n`, which has none.

### 13.29 `std.list`

```ply
pub fn first<a>(xs: List<a>) -> Option<a>
pub fn last<a>(xs: List<a>) -> Option<a>
pub fn take<a>(xs: List<a>, n: Int) -> List<a>
pub fn drop<a>(xs: List<a>, n: Int) -> List<a>
pub fn reverse<a>(xs: List<a>) -> List<a>
pub fn concat<a>(xs: List<a>, ys: List<a>) -> List<a>
pub fn flat_map<a, b>(xs: List<a>, f: (a) -> List<b>) -> List<b>
pub fn zip<a, b>(xs: List<a>, ys: List<b>) -> List<{ first: a, second: b }>
pub fn any<a>(xs: List<a>, ok: (a) -> Bool) -> Bool
pub fn all<a>(xs: List<a>, ok: (a) -> Bool) -> Bool
pub fn count<a>(xs: List<a>, ok: (a) -> Bool) -> Int
pub fn find<a>(xs: List<a>, ok: (a) -> Bool) -> Option<a>
pub fn find_index<a>(xs: List<a>, ok: (a) -> Bool) -> Option<Int>
pub fn contains<a>(xs: List<a>, x: a) -> Bool where derivable(eq, a)
pub fn index_of<a>(xs: List<a>, x: a) -> Option<Int> where derivable(eq, a)
pub fn remove_first<a>(xs: List<a>, x: a) -> Option<List<a>> where derivable(eq, a)
pub fn sum(xs: List<Int>) -> Int
pub fn max_of<a>(xs: List<a>) -> Option<a> where derivable(ord, a)
pub fn min_of<a>(xs: List<a>) -> Option<a> where derivable(ord, a)
pub fn is_sorted<a>(xs: List<a>) -> Bool where derivable(ord, a)
pub fn sort<a>(xs: List<a>) -> List<a> where derivable(ord, a)
pub fn sort_by<a>(xs: List<a>, before: (a, a) -> Bool) -> List<a>
pub fn flatten<a>(xss: List<List<a>>) -> List<a>
pub fn partition<a>(xs: List<a>, ok: (a) -> Bool) -> { yes: List<a>, no: List<a> }
pub fn split_at<a>(xs: List<a>, n: Int) -> { head: List<a>, tail: List<a> }
pub fn chunks<a>(xs: List<a>, n: Int) -> List<List<a>>
pub fn intersperse<a>(xs: List<a>, sep: a) -> List<a>
pub fn unique<a>(xs: List<a>) -> List<a> where derivable(ord, a)
```

The prelude has the pieces — `map`, `filter`, `fold`, `iterate`, `range`, `push`
and `list_at` — and not the wholes. A `List` is a vector, not a linked list: the
cheap end is the back, `push` appends and nothing prepends, so every function
here folds left to right and appends, which is one pass and linear. That is why
building the same list from the front is a shape to avoid in Ply as well: it
copies the accumulator every step and is quadratic. `take` and `drop` are the two
halves of a list (`concat(take(xs, n), drop(xs, n))` is `xs`), `reverse` walks its
index down while it appends, and `sort` is a merge sort — `n log n` comparisons
whatever the input order is, and equal elements keep their relative order.
`sort_by` is the same sort under a caller's `before`, which is how a key sort is
written. `find` and `find_index` keep the first answer a scan meets. `partition`,
`split_at` and `chunks` divide one list into others and keep the order;
`flatten` is `flat_map` of the identity, `intersperse` puts its separator
between the elements, and `unique` keeps each element's first occurrence — its
membership test is a map's, so it is `n log n` rather than the `n²` a scan
through the output would be.

### 13.30 `std.map`

```ply
pub fn is_empty<k, v>(m: Map<k, v>) -> Bool
pub fn size<k, v>(m: Map<k, v>) -> Int
pub fn get<k, v>(m: Map<k, v>, key: k) -> Option<v>
pub fn get_or<k, v>(m: Map<k, v>, key: k, fallback: v) -> v
pub fn contains<k, v>(m: Map<k, v>, key: k) -> Bool
pub fn insert<k, v>(m: Map<k, v>, key: k, value: v) -> Map<k, v>
pub fn remove<k, v>(m: Map<k, v>, key: k) -> Map<k, v>
pub fn merge<k, v>(a: Map<k, v>, b: Map<k, v>) -> Map<k, v>
pub fn keys<k, v>(m: Map<k, v>) -> List<k>
pub fn values<k, v>(m: Map<k, v>) -> List<v>
pub fn entries<k, v>(m: Map<k, v>) -> List<{ key: k, value: v }>
pub fn from_entries<k, v>(entries: List<{ key: k, value: v }>) -> Map<k, v>
```

Every signature carries `where derivable(ord, k)`: a `Map` key has to be an
ordered type, and a wrapper has to say so as the map builtins do. This is the
prelude's `map_`-prefixed builtins under the module's name — `map.get(m, key)`
reads where `map_get(m, key)` spells a word twice — with the composed ones
(`size`, `get_or`) and the names a caller reaches for (`contains`, `keys`,
`entries`). Every answer comes back in the map's key order. The effectful folds
stay in the prelude, as `map_fold` and `map_update`: a wrapper's signature closes
the effect row, and a fold that could not perform an effect would not be the
prelude's.

### 13.31 `std.set`

```ply
pub fn empty<a>() -> Map<a, Unit> where derivable(ord, a)
pub fn is_empty<a>(s: Map<a, Unit>) -> Bool
pub fn size<a>(s: Map<a, Unit>) -> Int
pub fn contains<a>(s: Map<a, Unit>, x: a) -> Bool
pub fn insert<a>(s: Map<a, Unit>, x: a) -> Map<a, Unit>
pub fn remove<a>(s: Map<a, Unit>, x: a) -> Map<a, Unit>
pub fn union<a>(a: Map<a, Unit>, b: Map<a, Unit>) -> Map<a, Unit>
pub fn elements<a>(s: Map<a, Unit>) -> List<a>
pub fn of_list<a>(xs: List<a>) -> Map<a, Unit>
pub fn contains_all<a>(s: Map<a, Unit>, xs: List<a>) -> Bool
pub fn contains_none<a>(s: Map<a, Unit>, xs: List<a>) -> Bool
pub fn intersection<a>(a: Map<a, Unit>, b: Map<a, Unit>) -> Map<a, Unit>
pub fn difference<a>(a: Map<a, Unit>, b: Map<a, Unit>) -> Map<a, Unit>
pub fn symmetric_difference<a>(a: Map<a, Unit>, b: Map<a, Unit>) -> Map<a, Unit>
pub fn is_subset<a>(a: Map<a, Unit>, b: Map<a, Unit>) -> Bool
pub fn is_superset<a>(a: Map<a, Unit>, b: Map<a, Unit>) -> Bool
```

A set is a `Map` whose values are `Unit` and nothing else, and there is no set
type in the language yet: an alias would read better — `type Set<a>` — but a
`Map` key has to be ordered, the constraint would have to ride on the alias, and a
type alias cannot carry `derivable` (card `313874be`). Until it can, the parameter
is the map and the constraint is on the signature. The key order is the set's
order, so `elements` is stable. `union` is `map_merge`, which is why a duplicate
is inserted once however many times it appears in the `of_list`; `intersection`
and `difference` walk one set's elements and ask the other, so each is `n log n`.
There is no `fold` here: the fold is the prelude's `map_fold`, whose open effect
row a wrapper would close.

### 13.32 `std.bigint`

```ply
pub type BigInt = { negative: Bool, limbs: List<Int> }
pub fn zero() -> BigInt
pub fn one() -> BigInt
pub fn is_zero(value: BigInt) -> Bool
pub fn is_negative(value: BigInt) -> Bool
pub fn sign(value: BigInt) -> Int
pub fn abs_value(value: BigInt) -> BigInt
pub fn neg(value: BigInt) -> BigInt
pub fn of_int(n: Int) -> BigInt
pub fn to_int(value: BigInt) -> Option<Int>
pub fn to_string(value: BigInt) -> String
pub fn of_string(text: String) -> Option<BigInt>
pub fn compare(a: BigInt, b: BigInt) -> Ordering
pub fn less_than(a: BigInt, b: BigInt) -> Bool
pub fn add(a: BigInt, b: BigInt) -> BigInt
pub fn sub(a: BigInt, b: BigInt) -> BigInt
pub fn mul(a: BigInt, b: BigInt) -> BigInt
pub fn div_mod(a: BigInt, b: BigInt) -> Option<{ q: BigInt, r: BigInt }>
pub fn div(a: BigInt, b: BigInt) -> Option<BigInt>
pub fn modulo(a: BigInt, b: BigInt) -> Option<BigInt>
pub fn floor_mod(a: BigInt, b: BigInt) -> Option<BigInt>
pub fn pow(base: BigInt, exponent: Int) -> BigInt
pub fn mod_pow(base: BigInt, exponent: Int, modulus: BigInt) -> Option<BigInt>
pub fn gcd(a: BigInt, b: BigInt) -> BigInt
pub fn shift_left(value: BigInt, bits: Int) -> BigInt
pub fn shift_right(value: BigInt, bits: Int) -> BigInt
```

Arbitrary precision, where `Int` wraps at `i64`. A value is a sign and base-2^30
limbs, least significant first, with no high zero limb and with zero positive and
limbless, so the representation is canonical and `derive eq` is the right
equality. The base is a power of two so a limb boundary is a bit boundary — shifts
and bit tests are limb arithmetic, and only the decimal conversions pay for the
base. `ord` is deliberately **not** derived: the number's order is not the
record's (with the sign first, `-10` would sort after `-5` by magnitude), so
`compare` is the ordering and a hand-written instance would be card `C4`'s ground.
`of_string` takes an optional sign and refuses anything else, `to_string` is its
inverse, and `-0` is zero. Division is truncated toward zero — `q` takes the sign
of `a * b` and `r` the sign of `a`, so `a == q * b + r` and `|r| < |b|` — and a
zero divisor is `None` rather than a raise; `floor_mod` is the remainder with the
divisor's sign, which is what `mod_pow` reduces with. A negative exponent is one,
as integer arithmetic has it. `to_int` answers `None` outside `i64`, `min_int`
included as exact on the way in and out. One implementation note worth keeping:
`limbs_of` is written as a plain recursion rather than a tail loop because a self
tail call that passes an `Int` at or beyond 2^62 back to itself releases the word
and reuses it (card `6298c4dd`).

### 13.33 `std.decimal`

```ply
pub fn scale(d: Decimal) -> Int
pub fn normalize(d: Decimal) -> Decimal
pub fn trunc(d: Decimal) -> Decimal
pub fn fract(d: Decimal) -> Decimal
pub fn is_negative(d: Decimal) -> Bool
pub fn abs(d: Decimal) -> Decimal
pub fn of_parts(mantissa: Int, scale: Int) -> Option<Decimal>
pub fn max_value() -> Decimal
pub fn min_value() -> Decimal
```

What a `Decimal` is made of. A value keeps the scale it was written or computed
at, so `1.5m == 1.50m` while `scale` answers 1 and 2; `normalize` is the same
value at the least scale that holds it. `trunc` goes toward zero and `fract` is
what it leaves, with the value's sign. `of_parts(150, 2)` is `1.50m`, and a
scale outside `0..=28` is `None`. The extremes are the 96-bit mantissa at scale
zero and its negation.

### 13.34 `std.float`

```ply
pub fn is_nan(f: Float) -> Bool
pub fn is_infinite(f: Float) -> Bool
pub fn is_finite(f: Float) -> Bool
pub fn is_sign_negative(f: Float) -> Bool
pub fn abs(f: Float) -> Float
pub fn trunc(f: Float) -> Float
pub fn pow2(k: Int) -> Float
pub fn nan() -> Float
pub fn infinity() -> Float
pub fn neg_infinity() -> Float
pub fn max_value() -> Float
```

A `Float`'s classification, sign and magnitude, read off its IEEE-754 bits, so
nothing here raises. `is_sign_negative` holds for `-0.0` and a negative NaN too.
`trunc` goes toward zero and keeps the sign, leaving a NaN or an infinity as it
is. `pow2(k)` is `2^k` exactly, through the subnormals down to `2^-1074`, zero
below that and an infinity above `2^1023`.

## 14. The host boundary

Without `--host`, an operation that reaches the boundary is `E0424`, naming the
handler that would serve it. With `--host`, a test that reaches a bound handler
always runs and is never cached. An operation performed inside a `simulate`
region reaches no handler at all: it is `E0425` (§9), since the region is run
once per interleaving. `std.signal` and `std.process` are bound only
by `ply run --host`; `ply test --host` withholds them (`E0424`), except that a
test run binds `process.bound`, `process.spawn`, `process.start` and the
operations on a started child, which reach only the programs `--exec` names. All
flags below require `--host`.

`ply hosts` lists every bindable operation (`effect.op[resource]`: one row per
operation and label some row of the program names, where a written mode atom
names every operation of its mode) with its handler, determinism,
`at-most-once`/`repeatable`, blocking and `Secret` permission, plus the run's
TLS, filesystem, database, configuration, tracing and shutdown settings;
`--digest` prints one `b3:` line. A handler for something undeclared is `E0421`,
two for one atom `E0422`, and a determinism mismatch `E0423`.

| flag | meaning |
| --- | --- |
| `--tls NAME=CERT,KEY` | repeatable TLS credential (PEM, leaf first; key PKCS#8, PKCS#1 or SEC1), used as `net.listen_tls[l](port, "NAME")`; `E0430` if it does not load, `E0429` if unnamed |
| `--trust CERT.pem` | repeatable certificate `net.connect_tls` accepts beside the built-in roots; `E0430` if it does not parse; the `ply` command's own connections take `PLY_TRUST` instead (§16) |
| `--fs NAME=PATH` | repeatable filesystem root; `E0454` if not a directory |
| `--exec NAME=PATH` | repeatable program a `process.spawn` or `process.start` label may start (`ply run`, `ply test`); `E0457` if it cannot be executed |
| `--allow NAME` | repeatable privileged family lent to the program, which must declare the effect it lends: `machine`, `tester`, `claims` (effect `prover`), `builder`, `cache` (`store`), `bootstrap` (`archive`), `hosts` (`tcb`) or `edit` (`ply run`, `ply test`); `E0459` otherwise |
| `--set KEY=VALUE` | configuration value; repeatable, highest precedence |
| `--config PATH` | `KEY=VALUE` file; repeatable, above the environment |
| `--config-schema MODULE.FN` | a `ConfigSpec`: missing key `E0441`, bad value `E0442`, undeclared key `W0607` |
| `--trace json\|text\|off` | trace sink: JSON lines on stderr (default), text, or discard |
| `--trace-level debug\|info\|warn\|error` | lowest level written (default `info`) |
| `--drain-lead-ms MS` | after `SIGINT`/`SIGTERM`, keep accepting this long (default 0) |
| `--drain-ms MS` | then let in-flight requests finish (default 30000); expiry is `W0608`, exit 3 |

An unreadable configuration source is `E0440`. A statement the server rejects
and one that touches a table outside what the call site labelled are refusals
from `std.db`, which reads and runs its own statements.

## 15. Building and shipping

```
$ ply build . --entry app.serve -o app.plyx
$ ply build . --digest              # print `b3:...`; writes no file
$ ply build . --diff old.plyx       # added, changed, dropped, unchanged
$ ply run app.plyx --host
```

A **library** — a package whose manifest names no entry and whose own modules
declare no `main` — is built as the package itself: `ply build` writes a
`.plyz`, the same container under a magic (`PLYLIB01`) and a digest domain of
its own, holding every module's source, the package's `ply.pkg` text, and a
compiled unit of every definition those modules declare (a library has no entry
to prune against, so nothing is left out). `-o FILE` names it; the default is
`<name>.plyz`. A consumer compiles those sources — always correct — or reuses
the unit when the runtime matches, the `E0444` gate. It is a package and never
a program: `ply run lib.plyz` refuses it (`E0443`) rather than reading a
container as text.

`ply build` writes the closure of one entry point (default `main`) as a `.plyx`
file (default `<entry module>.plyx`): its definitions, printed back to source
without tests, laws, comments or anything unreached, and the compiled unit. The
BLAKE3 digest covers those and the entry point, so an edit nothing reaches
leaves it unchanged; a failure raised by a run of it carries no line number. A
body or closure that fails verification is `E0443`, as is a build whose closure
holds two identical declarations it cannot tell apart (two effects, or two
members of one recursive group); an artifact from another version is `E0444`.
`--config-schema` ships that function too, resolved as a run resolves it: a name
that is not a nullary pure function returning a `ConfigSpec` is `E0440`.

### 15.1 The registry

A library is published to a registry and depended on from it (§3.3). A registry
is a directory of files behind an HTTP server, laid out statically:

```
GET  /<name>/index.json                  every version of <name>, newest last
GET  /<name>/<version>/package.plyz      the library's `.plyz`, as `ply build` writes it
GET  /<name>/<version>/package.plyz.b3   its digest, one `b3:<hex>` line
PUT  /<name>/<version>                   publish: the `.plyz` as the body
POST /<name>/<version>/yank              mark the version yanked
```

`index.json` is `{"name": .., "versions": [{"version": "0.2.0", "digest":
"b3:..", "yanked": false, "runtime": {"major": .., "minor": .., "patch": ..}}]}`
in publication order, `runtime` being the toolchain the version's manifest
declares. It is advisory: a resolve checks every archive against it and against
the lock, so a stale or hostile index can cause a refusal and never a wrong
build. A version's identity is the digest `ply build` seals its `.plyz` with —
BLAKE3 under the library domain over the entry field and every byte from the
section table on, the same framing a `.plyx` digest takes — written out in full.

`ply publish [path]` builds the library's `.plyz` exactly as `ply build` does and
sends it to the registry `PLY_REGISTRY` names, under the token
`PLY_REGISTRY_TOKEN` holds, as `Authorization: Bearer <token>` with
`X-Ply-Digest: b3:<hex>`. Only a library is published, and one whose
dependencies are all `Registry` ones, since whoever depends on it resolves them
from the registry alone; a program, the anonymous package or a path or git
dependency is `E0145`, as is a `ply yank` name or version that is not one. The
registry recomputes the digest from the body and refuses a mismatch, refuses a
version it already lists — a published version never changes, and the fix is a
new version — and refuses an archive whose manifest is not the package and
version it was sent as, names an entry, or depends on anything but the
registry; each refusal is `E0144` with the registry's reason.
`ply yank NAME VERSION` sets the version's `yanked` field under the same token:
a new resolution passes it over and a lock that pins it keeps it, and its archive
is served exactly as before. Nothing is ever deleted.

`PLY_REGISTRY` is one base URL, `https://host[:port][/prefix]`; the client
verifies the server against the built-in roots and the certificates `PLY_TRUST`
names (§16), so a registry under a private CA is reached by pointing `PLY_TRUST`
at the CA's certificate. A handshake the client cannot complete is `E0141`, and
says so. `http://` is accepted only for a registry on this machine
(`localhost`, `127.x.x.x`, `[::1]`), because a publish carries a token.

The registry is a Ply program, `crates/ply-registry/ply`:

```
$ ply run crates/ply-registry/ply --host --fs store=/srv/ply \
    --tls registry=cert.pem,key.pem --set tls=registry --set port=8443 \
    --config tokens.conf
```

`store` is the tree above; `port` is where it listens (default `8080`); `tls`
names the `--tls` credential it serves with, and without it the registry serves
plain HTTP, for a proxy that terminates TLS in front of it or a registry on this
machine. Each package's token is the configuration key `token.<name>` — one exact
name per key, so a `--config` file of `token.orders=...` lines is the whole of
who may publish what. It serves one connection at a time, and takes a package's
lock around every write, so two registries over one store never interleave one.
Like every Ply listener it binds `127.0.0.1`: another machine reaches it through a
proxy in front of it, one that passes TLS through to a `--tls` registry or
terminates it for a plain one.

## 16. The `ply` command

`ply [--color auto|always|never] <command> [path] [options]`. `--color` is
global; `auto` colours only a terminal with `NO_COLOR` unset. The path defaults
to `.`. Every command takes `--json` and then prints exactly one JSON object on
stdout, compact and with its keys sorted.

The command reads its own environment: `NO_COLOR`, `PLY_CACHE_UPSTREAM` (§1),
`PLY_REGISTRY` and `PLY_REGISTRY_TOKEN` (§15.1), the backend's `PLY_C_*`
(§8.6), and `PLY_TRUST` — PEM files, colon-separated, whose certificates the
command's own HTTPS connections (a registry's, for `ply publish`, `ply yank` and
`ply resolve`) accept beside the built-in roots, as `--trust` does for a
program's `net.connect_tls`. Every file it names must load: one that does not is
`E0430` before the command runs.

| exit | meaning |
| --- | --- |
| 0 | success |
| 1 | a test failed, or `main` raised |
| 2 | the program did not run: bad path, syntax or type error |
| 3 | the drain deadline expired with requests in flight |
| *n* | `process.exit[p](n)` under `ply run --host`: the program's own, `0` to `125` |

Flag groups: *simulation* (§9), *host* (`--host` and §14's flags except trace
and drain), *prove* (`--prove-cases`, `--prove-roots`, `--prove-budget`,
`--shrink-budget`, `--prove-steps`), *trace* (`--trace`, `--trace-level`),
*drain* (`--drain-ms`, `--drain-lead-ms`).

| command | flags |
| --- | --- |
| `ply new PATH` | `--name NAME` (default: the path's last segment), `--lib` (no `main`, a `pub` definition instead); refuses a name that is not a package name and a directory that is already there |
| `ply check [path]` | `--types`, `--costs`, `--explain` (front-end phases, and how many definitions the front-end cache seeded and how many were checked; with `--types`, effect sets and provenance) |
| `ply test [path]` | `--filter`, `--jobs`/`-j`, `--steps`, `--timeout`, `--no-cache`, `--explain`, `--watch`, `--bisect`, `--bisect-budget`, `--coverage`, `--mutate [DEF]`, `--mutate-budget`, `--profile`, `--std`, host, simulation |
| `ply run [path] [-- ARGS]` | `--seed` (one interleaving always), `--steps` and `--timeout` (both default to no bound: an entry that serves forever is a program), `--profile`, `--explain` (whether the front end ran or an earlier run's answer was reused, and the load's phases), host, trace, drain; `ARGS` is what `process.args` answers; a `.plyx` path runs the artifact |
| `ply prove [path]` | `--filter`, `--jobs`, `--no-cache`, `--no-incremental`, `--explain`, `--reach`, `--std`, host, trace, prove, simulation |
| `ply review [path]` | `--changed` (default), `--accept`, `--no-cache`, `--no-incremental`, `--std`, prove, simulation |
| `ply build [path]` | `--entry NAME`, `-o FILE` (default `<entry module>.plyx` for a program, `<package>.plyz` for a library), `--config-schema`, `--digest`, `--diff OLD.plyx`, `--stamp FILE` (the digest the launcher gates its shipped artifact on; the CLI's own build) |
| `ply hosts [path]` | host, trace, drain, `--digest` |
| `ply std` | `--show [MODULE]`, `--digest`; no path |
| `ply explain CODE` | one line on what the code means; `--all` lists every code; no path |
| `ply doc NAME [path]` | a definition or builtin: signature with the written parameter names, the `//` lines above it, place, hash, footprint; a builtin's note comes from the compiler's table |
| `ply fmt [paths]` | rewrite every `.ply` file under the paths in the canonical layout; `--check` writes nothing and exits 1 naming the files that would change, and `--json` is a report of exactly that, so it requires `--check` |
| `ply show NAME [path]` | one `fn` or `type` as its file holds it: the `//` lines above it, `pub`, the body, and a comment ending its last line; `--json` adds the byte range |
| `ply replace NAME [path]` | rewrite one `fn` or `type` from `--with FILE` or stdin, formatted, every other byte of the file kept; refused with `E0128` (exit 2, nothing written) unless the program still checks and no other definition's name or hash moves; `--check` writes nothing |
| `ply resolve [path]` | write `ply.lock` from this project's manifest closure, listing every dependency's name, version and source digest; the one command that fetches registry dependencies, from `PLY_REGISTRY` |
| `ply vendor [path]` | copy the closure into `vendor/`, one directory per package plus an index, so the project builds with no cache and no network |
| `ply why NAME [path]` | why a package is in the closure: the path from the root package to it, then the version and digest the closure pins |
| `ply publish [path]` | build this library's `.plyz` and upload it to `PLY_REGISTRY` under `PLY_REGISTRY_TOKEN` (§15.1) |
| `ply yank NAME VERSION` | mark a published version yanked, under `PLY_REGISTRY_TOKEN`; no path |
| `ply hash [path]` | `--deps` (references and transitive closure) |
| `ply defs [path]` | every definition: place, hash, signature, footprint, references; `--filter SUBSTRING` |
| `ply callers DEF [path]` | what mentions a definition directly, and every definition, test and law whose closure reaches it |
| `ply bootstrap <path>` | writes the front end as the bundle the runtime builds it from: `unit.c.gz` beside `SOURCES.digest`; `--out DIR` (default `bootstrap`), `--verify` (compare, write nothing), `--profile` (default `release`) |
| `ply cache clear\|stats\|compact [path]` | discard results / report size and reclaimable space / reclaim it |
| `ply cache inspect <DEF> [path]` | one definition's entries, by full name, simple name or 4+ hex hash prefix |

`ply new`, `ply check`, `ply fmt`, `ply defs`, `ply hash`, `ply doc`,
`ply show`, `ply replace`, `ply resolve`, `ply vendor`, `ply why`, `ply publish`,
`ply yank`, `ply callers`,
`ply std`, `ply explain`, `ply hosts`, `ply cache` and `ply bootstrap` are one
Ply program (`crates/ply-cli/ply`, entered at `ply.main`). The program itself parses the command line, prints help and
refusals, and resolves the paths it is given against the working directory — a
relative path reads under it, an absolute one reads where it points. The binary
answers with the code the program asked to exit with. What a command needs of
the machine is lent to the program as an effect: `ply run`, `ply test` and
`ply prove` drive a nested program on a machine of their own, `ply hosts`,
`ply cache` and `ply bootstrap` are answered what a run would bind, what the
store holds and the bundle the emitter produced, and `ply replace` is lent the
text it puts in a definition's place, from `--with FILE` or stdin. `ply std`
needs no project: it reads the shipped modules off a second, read-only root.
`--count-allocs=PATH` is the launcher's own flag rather than the program's: it is
taken out of the line before the program parses it, and the run writes what the
entry allocated — every thread's allocations, in a window around the entry — as
`allocations` and `bytes`. `--count-alloc-sites=PATH` writes the same totals and
adds `sites`, the nearest few `ply_*` C frames each allocation came from, most
first, with `exact` and `sampled_every` saying how they were read: it walks a
stack per allocation, so it is for measurements, not for production, and the
walk's own allocations are the walker's and are not counted. The walk is most of
what a site census costs, so this flag samples — every allocation is counted and
one in `sampled_every` is walked, with the rows scaled back up, so they read in
the totals' units and their sum is near the total rather than equal to it.
`--count-alloc-sites-exact=PATH` walks every allocation instead, for a census
that is exact and slower.
A run whose work is interpreted has no such frames: its allocations are the
interpreter's, and they are what the totals are made of.
What a host may lend is a policy with names, one family each:
`machine` (load, bind, enter and call a nested program), `tester`, `claims`,
`builder`, `cache`, `bootstrap`, `hosts` and `edit`, each with a summary a
reviewer can read. The launcher lends its own program every family; another host
names the ones it means, so `machine` — which drives another machine — is
granted on purpose and not by accident.
The first run after `ply` or the program itself changes compiles the program's unit,
which needs the C toolchain `ply run` needs and takes a few seconds; every later
run loads the compiled object and the front end it filed beside it. A command
that loads a program reads the front-end cache under `.ply-cache` before it
analyses and files what it answered after: a definition whose hash has not moved
since it was filed is taken from its filed rows, so a run checks what an edit
moved and what reaches it, and a definition generic over an effect row every
time. `ply build`, `ply hosts`, `ply test
--no-cache` and `--no-incremental` neither read nor file it, and a program's own
`machine.load` of a program runs the whole front end. A cache that will not read
is a warning and a cold check, never a failure; the run that files over one
filed by a compiler whose shipped modules differed says so once, as `W0605`.

`ply run` over sources goes further: once a load holds, the front end's answer
is filed under a key of everything it and the `reuse fn` promise check (`E0127`)
read — the name and bytes of every module the walk read, the root's manifest,
each dependency's key, manifest and modules, the root's absolute path, the `ply`
program and the modules it ships as the launcher gates them (so `PLY_C_EMITTER`
too), the binary's version, and `--config-schema`. A later run whose walk hashes
the same takes that answer and runs neither the front end nor the promise check,
which the filed load passed; it binds, grants (`--allow`, `--exec`, `--fs`) and
picks its entry anew, and reports exactly what a run that built the answer
reports. Any edit to a module, a dependency or a manifest, another schema or
another `ply` is a new key, and the front end runs again; `ply.lock` is not
read by a run and is not in the key. A single `.ply` file keys that one module.
The answers live under the stage root (`PLY_C_STAGE`, §8.6) in `run-fronts/`,
one file per key, each written beside itself and renamed into place, so two runs
of one package never read half of one; an entry that does not read is rebuilt
and written over. They are swept with the stages, least recently used first, down to
`PLY_C_CACHE_MAX`, never one used within the hour, and deleting them is always
safe. `ply run --explain` says `reused` or `built`, the key, and what reading,
the front end, filing into `.ply-cache` and the machine's load each took, on
stderr before the entry runs, or as `front_end` in the `--json` document.

`ply fmt` keeps comments, the spelling of every literal, and the order of
imports, items and statements; it prints `formatted PATH` per file it changed
and leaves a file that does not parse alone, exiting 2 with the diagnostic. A
file it cannot read or write back is an error too, exiting 2, so `--check`
never passes over a file it did not read. A directory whose name starts with
`.`, and one named `target`, are not walked; a symlink found while walking is
passed over, and one named on the command line is an error rather than a file to
rewrite.

`ply show NAME` and `ply replace NAME` are the edit loop for one definition: read
it, rewrite it, and touch nothing else in the file. The replacement is one item
of the same kind and name, with its own `//` lines above it; `replace` prints it
through `ply fmt` into the range `show` reports.

## 17. Diagnostics

`E` is an error; `W` is a warning and never a fault in your program.
`ply explain CODE` prints a code's line from this table, and `--all` the whole
table.

Every command writes a diagnostic the same way, in one shape:

```
Error[E0201]: type mismatch: function body type
  --> app.ply:1:17
   | fn f() -> Int = "x"
   |                 ^^^ expected `Int`, found `String`
  = the body must answer the written type
```

The heading is the severity, the code and the message. Then one block per label:
where it points, the line it points into, and a caret run under the span — one
caret for an empty span, and never past the end of the line the span opens on. A
label whose span runs past the end of its file, or names no file at all, is
dropped, as it is under `--json`; one that cuts a character in half is still
placed, on the line it opens. A diagnostic left with no label is its heading and
notes alone, and each note is a `  = ` line. Colour is paint on that shape and
never changes it: the heading and the caret run take the severity's colour, and a
pipe, `NO_COLOR` or `--color never` leaves exactly these bytes.

A diagnostic that knows its own remedy carries `fixes` under `--json`: each has a
`title` and `edits`, and an edit replaces the text between `start` and `end` of
`file` (an empty range inserts) with `text`. Applied as they are, the edits leave
a program the diagnostic no longer holds for. On a terminal a fix is a
`= fix:` line under the message.

| code | meaning |
| --- | --- |
| `E0001` | unexpected token |
| `E0002` | unterminated string or byte-string literal |
| `E0101` | unknown name (including no `main` to run) |
| `E0102` | unknown type |
| `E0103` | unknown effect |
| `E0104` | unknown operation |
| `E0105` | duplicate definition, or a reserved name |
| `E0106` | unknown module |
| `E0107` | private name |
| `E0108` | ambiguous import |
| `E0109` | module cycle |
| `E0110` | duplicate import |
| `E0111` | file path that cannot name a module |
| `E0112` | ambiguous entry point |
| `E0114` | unknown `effect set`, including a `pub` or qualified one |
| `E0115` | `effect set` cycle |
| `E0116` | record update base that is not a record of a known type |
| `E0117` | record update naming a field the base lacks |
| `E0118` | `?` with no written `Result`/`Option` return type to exit through |
| `E0119` | `?` where its early exit would change what runs or drop an annotation |
| `E0120` | parameter default on a lambda, operation or handler clause |
| `E0121` | parameter default that is not a pure, closed value |
| `E0122` | default on a `pub fn` naming something its module does not export |
| `E0123` | named argument naming no parameter, or one twice |
| `E0124` | positional argument after a named one |
| `E0125` | parameter left unfilled by a call that used a name |
| `E0126` | top-level `fn` missing a parameter or return type |
| `E0127` | `reuse fn` with an append that cannot reuse its list |
| `E0128` | `ply replace` refused: the result would not check or would move another definition |
| `E0129` | a `ply.pkg` that is not exactly one `fn package` returning `Manifest` |
| `E0130` | a manifest body that runs rather than being a value |
| `E0131` | a manifest field that does not decode or fails validation |
| `E0132` | an import of a package the manifest does not declare as a dependency |
| `E0133` | two packages granting one module prefix |
| `E0134` | packages depending on one another in a cycle |
| `E0135` | a dependency that is missing, unmanifested or not fetched |
| `E0136` | a dependency below the version floor its importer asks for |
| `E0137` | one package reached at two places, where a closure pins one version |
| `E0138` | a dependency whose sources are not what `ply.lock` pinned |
| `E0139` | a `ply.lock` that does not decode or is from another format |
| `E0140` | a git dependency that could not be fetched |
| `E0141` | a registry that could not be asked: unset, malformed or not answering |
| `E0142` | a registry archive that is not the one the lock pins or the index lists |
| `E0143` | a registry dependency no published version satisfies |
| `E0144` | a publish or a yank the registry refused |
| `E0145` | a package or a version no registry takes |
| `E0201` | type mismatch |
| `E0202` | arity mismatch |
| `E0203` | occurs check |
| `E0204` | not a function |
| `E0205` | non-exhaustive match |
| `E0206` | not derivable, including an unordered `Map` key |
| `E0207` | unknown deriver |
| `E0208` | orphan `derive` |
| `E0209` | `/` on `Decimal` |
| `E0210` | operand type nothing determines |
| `E0211` | integer literal out of range for its fixed width |
| `E0212` | the alternatives of an or-pattern bind different names |
| `E0213` | a `let` whose or-pattern can fail has no `else` |
| `E0301` | unbound row variable |
| `E0302` | effect not permitted by the written row |
| `E0303` | unhandled effect (compiler defect) |
| `E0304` | resource label required |
| `E0305` | `handle` leaves an operation, or a mode atom, under a handled mode atom unanswered |
| `E0306` | label instantiation: a call leaves a label unfilled or writes the wrong number of them, or a label-generic definition is used as a value |
| `E0307` | mutually recursive definitions binding different label or row parameters |
| `E0308` | polymorphic recursion: a call inside a recursive group asks for another row or type parameter than the group was checked with |
| `E0412` | nondeterministic effect in a deterministic test |
| `E0413` | `Task` escapes its region |
| `E0414` | deadlock, or spent step budget |
| `E0415` | replay did not reproduce the schedule (Ply's fault) |
| `E0416` | nested `simulate` |
| `E0417` | effect in a spec, guard or law body |
| `E0418` | `forall` binder type that cannot be quantified |
| `E0419` | obligation refuted by a counterexample |
| `E0420` | vacuous obligation: the guard admits nothing |
| `E0421` | host registration for something undeclared |
| `E0422` | two host registrations for one atom |
| `E0423` | host handler determinism disagrees with the declaration |
| `E0424` | operation reached the host boundary with nothing bound |
| `E0425` | host operation inside a `simulate` region, or in a test the search re-runs |
| `E0426` | continuation resumed twice across an at-most-once host operation |
| `E0427` | host handler answered an atom outside the entry point's footprint |
| `E0428` | `blocking` host handler answered inline |
| `E0429` | `net.listen_tls` named a credential the run lacks |
| `E0430` | `--tls` credential, or certificate to trust, that does not load |
| `E0439` | `Secret` passed to a host operation not allowed one |
| `E0440` | configuration source unreadable |
| `E0441` | required configuration key missing |
| `E0442` | configuration value of the wrong shape |
| `E0443` | artifact does not verify |
| `E0444` | artifact built under another version |
| `E0445` | `trace.exit` of a span not open on this task |
| `E0446` | value outlives its region |
| `E0448` | definition the compiled tier cannot compile |
| `E0449` | region handle or continuation reaching a runtime boundary |
| `E0450` | compiled backend cannot be attached |
| `E0451` | `fs` label with no root bound |
| `E0452` | path leaves its root |
| `E0453` | read over the bound |
| `E0454` | `--fs` root that is not a directory |
| `E0455` | the program asked to exit with a code |
| `E0456` | `process.spawn` or `process.start` label with no executable bound |
| `E0457` | `--exec` path that cannot be executed |
| `E0458` | captured output over the bound |
| `E0459` | `--allow` family the program does not declare |
| `E0501` | assertion failed |
| `E0502` | runtime error: `panic`, division by zero, overflow, bad index, spent budget, call limit |
| `E0503` | spent its step budget without finishing |
| `E0505` | Ply broke one of its own invariants |
| `W0601` | cache unreadable |
| `W0602` | cache corrupt |
| `W0603` | cache from another version |
| `W0604` | obligation undecided at every tier |
| `W0605` | standard library changed since the cache was written |
| `W0607` | supplied configuration key the schema does not declare |
| `W0608` | drain deadline expired with requests in flight |
| `W0609` | spans still open when their task or the entry point ended |
| `W0610` | reference cycle, never freed |
| `W0611` | definition no `pub` item, `main`, test or law reaches; a leading `_` in its name keeps it quiet |
| `W0612` | run abandoned at its wall clock; nothing recorded |

## 18. What Ply does not have

* No loops, `break` or `return` (`?` is the only early exit); no mutable
  variables; no exceptions; no typeclasses, implicits or method syntax; no
  modules-as-values or first-class effects; no `unsafe` or FFI.
* Specs cannot name mutable state. Cycles are not collected, and a task never
  moves between OS threads.
* No file handles — `fs` reads a range and appends by path, with nothing open
  between calls — and no recursive walk or permissions; no cancellation or
  backpressure; no migrations or live schema check; HTTP/1.1 only; no
  authentication framework.

Sharp edges: `x.f(y)` with a bare variable `x` is a perform; an operation no
`handle` names is found only when it reaches the host boundary at run time
(`E0424`), unless its effect is `nondet` in a deterministic test (`E0412`); a
record update needs the base's type to be known where it stands; two allocating tasks are always ordered; `bytes_at`, `bytes_u32_le`, `string_slice`,
`string_find` and `list_set` raise where `list_at` answers `None`.

## 19. Examples

In `examples/`: `clock.ply` (a `nondet` effect, a handler, `test/nondet`);
`ledger.ply` and `report.ply` (modules, specs, laws); `pipeline.ply`, `bank.ply`
and `timeout.ply` (simulation, a race and its fix, a virtual clock); `echo.ply`
and `hello.ply` (sockets, an HTTP endpoint); `orders.ply` (`derive json`);
`relay.ply` (one forwarder generic over the label it writes under);
`store.ply` (a handler as a capability grant); `agreement.ply` and
`twin_divergence_audit.ply` (`std.db`'s twin against recorded PostgreSQL
answers); `desk.ply` (a service over PostgreSQL or its in-memory twin, whose
store, TLS and accept loop are configuration, with tracing and shutdown).
