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

**Projects.** Every `*.ply` file under the root, except inside directories whose
name starts with `.`, is a module named by its relative path with `/` → `.` and
`.ply` dropped: `store/orders/place.ply` is `store.orders.place`. Every
directory name and file stem must be an identifier (`E0111`); `std` is reserved
(`E0113`). Naming a single file makes its parent the root and loads only that
file.

**The cache.** `.ply-cache/` at the root holds the front-end, result and
obligation caches and the review baseline. It is safe to delete
(`ply cache clear`); add it to `.gitignore`. `PLY_CACHE_UPSTREAM=DIR` names a
second cache shared between checkouts and machines, a directory on any storage
they all reach: the passes and discharged obligations found there count here,
and this run's are published there (`PLY_CACHE_UPSTREAM_READONLY=1` reads
only). Entries are keyed by content and by the `ply` version, so nothing
machine-specific is ever shared; `--no-cache` ignores it.

## 2. Lexical structure

### 2.1 Source and identifiers

Source is UTF-8; whitespace only separates tokens and there is no layout rule.
The only comment is `//` to end of line.

An identifier starts with a letter (Unicode allowed) or `_` and continues with
alphanumerics or `_`; `_` alone is the wildcard. Case matters only in types (a
bare lowercase name is a type variable, uppercase a type constructor) and
patterns (lowercase binds, uppercase is a constructor). Convention: `snake_case`
values, `UpperCamelCase` types and constructors, lowercase effects and resource
labels.

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
| `with_cell`, `with_region` | before `[` |
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
| 8 | `++` | `String` |
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
* `++` concatenates `String`s only.
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

A body is `= expression`, or a block with no `=`. There is no `return`. Every
parameter and return type of a top-level `fn` must be written (`E0126`, which
names the inferred type).

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

Items are private unless `pub` (`E0107`). `pub` applies to `fn`, `type` and
`effect` only. Values (functions and constructors), types, effects and module
binders are separate namespaces, so `fn size`, `type Size` and `effect size`
coexist.

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
operand nothing determines, as in `let g = |a, b| a + b;`, is `E0210`; there is
no default. Conversions are explicit builtins (§12.3). `u32_of_int` and its
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
(`if`, `match`, `handle`, a block, `with_cell`, `with_region`, `simulate`).

A `let` pattern can take apart a record, which is how a function returns several
values:

```ply
fn sum_two(input: Bytes) -> Int = {
  let {value, next} = advance(input, 0);
  let {value: second, ..} = advance(input, next);
  value + second
}
```

A record pattern names every field or ends with `..` (`E0201`).

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

`{..base, deep: {..base.deep, a: 7}}` copies `base` with fields replaced. It
expands to a record literal. The base is a variable, a field path, or a call of
a `fn` declared in this file with a written return type (the call runs once).
Its shape must be readable from this file's own `type` items and written types
(`E0116` otherwise); a `let` without a written type takes the written type of
its value when that is such a variable, path, call or update. A field the base
lacks is `E0117`.

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
A loop that never ends fails with `E0503` when its time budget is spent (§8.4).
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
* `E0118`: inside a `handle`, `with_cell`, `with_region` or `simulate`; inside a
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
`return x -> body` clause maps the result. A handler discharges an
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
  with_region[work] {
    with_cell[work](0) { c -> {
      cell_set(c, cell_get(c) + n);
      cell_get(c)
    } }
  }
```

`with_cell[r](init) { c -> body }` allocates a cell for the duration of `body`.
`cell_get`, `cell_set` and `cell_update` are builtins whose atoms never leave
the region. `with_region[r] { body }` opens an allocation scope that a
`with_cell[r]` inside it allocates into. Nest `with_cell`s for several cells.

* `E0446`: a region-branded value outlives the region (returned, stored in an
  older binding, captured by an escaping closure, or put in a declared type).
* `E0447`: two regions in scope under one name.
* `E0449`: a region handle reaches a host operation, a host answer or an entry
  point's argument (at run time).
* `W0610`: a reference cycle; cycles are never freed.

## 8. Tests

### 8.1 Writing tests

`test "label" { ... }` is an item with a block body (§6.5 shows one); it cannot
be `pub`, referenced or given arguments. `assert(cond)` /
`assert(cond, Some("why"))` and `assert_eq(actual, expected)` fail with `E0501`,
the latter reporting both values and their first difference. Any other failure
is `E0502`.

### 8.2 Selection

A definition's hash covers its normalized form: names, comments, formatting,
imports, `pub`, specs and test labels are erased, and references are replaced by
their referent's hash. A test runs exactly when its hash has no recorded pass,
so renames and comment edits run nothing. `ply hash` prints the hashes.
`--explain` says why each test was selected; `--filter SUBSTRING` matches
`<module>.<label>`; `--no-cache` and `--no-incremental` bypass the result and
front-end caches.

### 8.3 Determinism

A test whose row, after handling, retains a `nondet` atom is `E0412`. Handle the
effect, or write `test/nondet "label" { ... }`, which is never cached.

### 8.4 Scheduling and failures

Tests whose footprints do not conflict run concurrently; a test whose effects
are all discharged in a region conflicts with nothing. `--jobs N`/`-j` sets
workers (default one per core).

`--timeout MS` is the wall clock each test may take (default 60000; `0` is no
bound); a test past it fails with `E0503`, which is a program error like any
other. A failing deterministic test that has passed before is bisected over the
definitions that changed to name a culprit. `--bisect auto|always|never`
(default `auto`), `--bisect-budget N` (evaluations, default 64), and
`--trace auto|always|never` (record which definitions a failure entered) control
this. `--json` prints one object with each failure's diagnostic, values,
footprint, suspects, culprit and replay command. `--watch` re-runs on every
`.ply` change, keeping caches in memory.

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

`--backend c` compiles the program to C and runs it there: compiled code is the
only evaluator. Its passes share the evaluator's cache.
`--profile development` (default; fastest compiler) or `release`
(`cc -O2`) requires `--backend`.

| variable | effect |
| --- | --- |
| `PLY_C_PROFILE=development\|release` | the profile, overriding `--profile` |
| `PLY_CC=cmd`, `PLY_CC_OPT=flag` | the C compiler and its optimisation flag, overriding the profile's |
| `PLY_C_CACHE=DIR` | compiled unit cache (default under the temp directory) |
| `PLY_C_STAGE=DIR` | the compiler's own stages, kept apart from the cache so a fresh cache reuses them (default under the temp directory) |
| `PLY_C_CACHE_MAX=BYTES` | cap on that cache, oldest entries swept first; `0` is no cap |
| `PLY_C_KEEP=1` | keep and print the emitted `.c` and shared object |
| `PLY_C_REFUSALS=1` | print which definitions the backend refused |
| `PLY_C_DUMP=NAME` | print one body's emitted C, or `*` for the unit's largest bodies |
| `PLY_C_ONLY=a,b`, `PLY_C_SKIP=prefix,...` | compile only the named definitions, or drop those with a prefix |
| `PLY_C_PHASES=1` | print compile phases, body-cache hits and misses, and allocation counts |
| `PLY_HEAP_POISON=1` | poison released blocks and fail on a read of one |
| `PLY_HEAP_DELAY=N` | reuse a released block only after `N` more releases |
| `PLY_C_EMITTER=ply:DIR` | use emitter sources from `DIR` instead of the built-in ones |
| `PLY_TIER_ONLY=1` | compiled code is the only engine; a missing body is `E0505` |
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
| `--seeds N` | seeds per test (default 1 under `dpor`, 64 under `random`) |
| `--sim-budget N` | interleavings per seed (`dpor` only) |
| `--sim-steps N` | steps per interleaving before `E0414` |
| `--seed 7`, `--seed 7:3.0.2` | replay one interleaving; implies `--sim once` |
| `--measure-reduction` | also run unpruned and report the cost |

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
values. Flags: `--prove-cases N` (below 25 kept cases only `example`),
`--prove-roots N`, `--prove-budget N` (spent reports `property`),
`--shrink-budget N`, `--timeout MS` (wall clock per evaluation, default 5000; an
evaluation past it leaves the obligation `unattempted`), and `--backend`.

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
| `bytes_len(b: Bytes) -> Int` | |
| `bytes_at(b: Bytes, i: Int) -> Int` | `0..=255` |
| `bytes_u32_le(b: Bytes, i: Int) -> U32` | four bytes, little-endian |
| `bytes_slice(b: Bytes, start: Int, end: Int) -> Bytes` | |
| `bytes_concat(a: Bytes, b: Bytes) -> Bytes` | |
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

Shipped inside `ply`; `import std.<name>`. Their tests and obligations are
skipped unless you pass `--std`. `ply std` lists them, `ply std --show std.json`
prints a source, and a changed standard library warns `W0605`.

### 13.1 `std.net` — sockets

```ply
pub nondet effect net {
  write listen[s](port: Int) -> Int
  write listen_tls[s](port: Int, credential: String) -> Int
  write connect[s](host: String, port: Int, timeout_ms: Int) -> Option<Int>
  write connect_tls[s](host: String, port: Int, timeout_ms: Int) -> Option<Int>
  write accept[s](listener: Int) -> Int
  write recv[s](conn: Int, max: Int, timeout_ms: Int) -> Option<Bytes>
  write send[s](conn: Int, payload: Bytes, timeout_ms: Int) -> Option<Int>
  write close[s](socket: Int) -> Unit
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
certificate on the first `send` or `recv`, and a failed handshake reads as EOF
and writes `0`.

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
it too. SQL errors are values; `is_retryable(e)`
covers serialization failures. `MemDb` is an in-memory twin (`open`, `step`,
`begin_step`, `commit_step`, `abort_step`). Statement text the driver cannot
account for is `E0432`.

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
pub nondet effect process {
  read args[p]()             -> List<String>
  write out[p](text: String) -> Unit
  write err[p](text: String) -> Unit
  write exit[p](code: Int)   -> Unit
}
```

The label names the process, `[proc]` by convention. `args` answers what
followed `--` on the `ply run` command line, `out` and `err` each write one
line, and `exit` ends the program there: nothing after it runs, no value is
printed, and `ply run` exits with the code (`0` to `125`, else `E0502`). It is
bound only by `ply run --host`; `ply test` withholds it, even with `--host`
(`E0424`). Under `ply run --json` the lines `out` writes go to stderr, so stdout
still carries the one object. Handle it over a `Captured` value: `captured(args)`,
`args_step`, `out_step`, `err_step` and `exit_step` keep each line and the first
exit code; a clause `process.exit[proc](c) resume k -> ...` that never calls `k`
ends the handled body as the host would.

### 13.10 `std.fs`

```ply
pub type Kind = | File | Dir | Symlink | Missing

pub nondet effect fs {
  read  read_file[r](path: String) -> Option<Bytes>
  read  list_dir[r](path: String) -> Option<List<String>>
  read  kind[r](path: String) -> Kind
  read  exists[r](path: String) -> Bool
  read  file_size[r](path: String) -> Option<Int>
  read  modified_ms[r](path: String) -> Option<Int>
  write write_file[r](path: String, body: Bytes) -> Bool
  write create_dir[r](path: String) -> Bool
  write remove[r](path: String) -> Bool
  write rename[r](from: String, to: String) -> Bool
}
```

The label is a root bound with `--fs NAME=PATH`. Unbound label: `E0451`; a path
escaping its root (`..`, absolute, or a symlink outside): `E0452`; a file over
the read bound: `E0453`. Different roots do not conflict. Reads are whole-file,
`list_dir` is one level, `rename` stays in one root. `kind` says what a path
names in one call and does not follow a symlink, so a walk can pass one over;
`Missing` is also what this run cannot read. Every other operation follows one.
The twin is `MemFs` (`mem_empty`, `mem_of`, `mem_read`, `mem_write`, `mem_list`,
`mem_kind`, `mem_exists`, `mem_size`, `mem_create_dir`, `mem_remove`,
`mem_rename`, `mem_modified`); it holds no symlinks, so `mem_kind` never answers
`Symlink`. A test imports both `std.fs` and `std.fs (fs)` to name the module and
the effect.

### 13.11 `std.path`

```ply
pub fn join(dir: String, name: String) -> String
pub fn file_name(path: String) -> String
pub fn extension(path: String) -> Option<String>
pub fn strip_dot(path: String) -> String
```

Text, not a filesystem: nothing here performs an effect. `join` places exactly
one separator and adds none for a root spelled `"."` or `""`. `file_name` is the
last segment, `""` for a path ending in a separator. `extension` follows the last
dot of the file name, and a dotfile has none. `strip_dot` removes a leading
`./`, so `./m.ply` and `m.ply` are one key in a set.

### 13.12 `std.hash`

`pub fn blake3(input: Bytes) -> Bytes` answers 32 bytes. It is written in Ply
and slow; use it for small inputs.

## 14. The host boundary

Without `--host`, an operation that reaches the boundary is `E0424`, naming the
handler that would serve it. With `--host`, a test that reaches a bound handler
always runs and is never cached. An operation performed inside a `simulate`
region reaches no handler at all: it is `E0425` (§9), since the region is run
once per interleaving. `std.signal` and `std.process` are bound only
by `ply run --host`; `ply test --host` withholds them (`E0424`). All flags
below require `--host`.

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
| `--trust CERT.pem` | repeatable certificate `net.connect_tls` accepts beside the built-in roots; `E0430` if it does not parse |
| `--fs NAME=PATH` | repeatable filesystem root; `E0454` if not a directory |
| `--db URL` | database (else `PLY_DB_URL`; password from `PLY_DB_PASSWORD`); `E0431` if absent when used |
| `--db-pool N` | pool size |
| `--db-acquire-ms MS` | wait for a connection before `E0437` |
| `--db-connect-ms MS` | connection timeout |
| `--db-statement-ms MS` | server `statement_timeout` |
| `--db-idle-txn-ms MS` | server `idle_in_transaction_session_timeout` |
| `--db-statement-cache N` | prepared statements per connection |
| `--db-schema MODULE.FN` | a nullary function returning a `db::Schema`, evaluated at start-up (not compared with the server) |
| `--set KEY=VALUE` | configuration value; repeatable, highest precedence |
| `--config PATH` | `KEY=VALUE` file; repeatable, above the environment |
| `--config-schema MODULE.FN` | a `ConfigSpec`: missing key `E0441`, bad value `E0442`, undeclared key `W0607` |
| `--trace json\|text\|off` | trace sink: JSON lines on stderr (default), text, or discard |
| `--trace-level debug\|info\|warn\|error` | lowest level written (default `info`) |
| `--drain-lead-ms MS` | after `SIGINT`/`SIGTERM`, keep accepting this long (default 0) |
| `--drain-ms MS` | then let in-flight requests finish (default 30000); expiry is `W0608`, exit 3 |

An unreadable configuration source is `E0440`. A statement the server rejects is
`E0433`; one touching a table outside the entry point's footprint is `E0434`.

## 15. Building and shipping

```
$ ply build . --entry app.serve -o app.plyx
$ ply build . --digest              # print `b3:...`; writes no file
$ ply build . --diff old.plyx       # added, changed, dropped, unchanged
$ ply run app.plyx --host
```

`ply build` writes the closure of one entry point (default `main`) as a `.plyx`
file (default `<entry module>.plyx`): its definitions, printed back to source
without tests, laws, comments or anything unreached, and the compiled unit. The
BLAKE3 digest covers those and the entry point, so an edit nothing reaches
leaves it unchanged; a failure raised by a run of it carries no line number. A
body or closure that fails verification is `E0443`, as is a build whose closure
holds two identical declarations it cannot tell apart (two effects, or two
members of one recursive group); an artifact from another version is `E0444`.
`--config-schema` and `--db-schema` ship those functions too.

## 16. The `ply` command

`ply [--color auto|always|never] <command> [path] [options]`. `--color` is
global; `auto` colours only a terminal with `NO_COLOR` unset. The path defaults
to `.`. Every command takes `--json` and then prints exactly one JSON object on
stdout, compact and with its keys sorted.

| exit | meaning |
| --- | --- |
| 0 | success |
| 1 | a test failed, or `main` raised |
| 2 | the program did not run: bad path, syntax or type error |
| 3 | the drain deadline expired with requests in flight |
| *n* | `process.exit[p](n)` under `ply run --host`: the program's own, `0` to `125` |

Flag groups: *simulation* (§9), *host* (`--host` and §14's flags except trace
and drain), *prove* (`--prove-cases`, `--prove-roots`, `--prove-budget`,
`--shrink-budget`, `--timeout`), *trace* (`--trace`, `--trace-level`), *drain*
(`--drain-ms`, `--drain-lead-ms`).

| command | flags |
| --- | --- |
| `ply check [path]` | `--types`, `--costs`, `--explain` (front-end phases; with `--types`, effect sets and provenance), `--no-incremental` |
| `ply test [path]` | `--filter`, `--jobs`/`-j`, `--timeout`, `--no-cache`, `--no-incremental`, `--explain`, `--watch`, `--bisect`, `--bisect-budget`, `--coverage`, `--mutate [DEF]`, `--mutate-budget`, `--trace auto\|always\|never`, `--backend`, `--profile`, `--std`, host, simulation |
| `ply run [path] [-- ARGS]` | `--seed` (one interleaving always), `--timeout` (default no bound), `--backend`, `--profile`, host, trace, drain; `ARGS` is what `process.args` answers; a `.plyx` path runs the artifact |
| `ply prove [path]` | `--filter`, `--jobs`, `--no-cache`, `--no-incremental`, `--explain`, `--std`, `--backend`, host, trace, prove, simulation |
| `ply review [path]` | `--changed` (default), `--accept`, `--no-cache`, `--no-incremental`, `--std`, `--backend`, prove, simulation |
| `ply build [path]` | `--entry NAME`, `-o FILE`, `--config-schema`, `--db-schema`, `--digest`, `--diff OLD.plyx` |
| `ply hosts [path]` | host, trace, drain, `--digest` |
| `ply std` | `--show MODULE`, `--digest`; no path |
| `ply explain CODE` | one line on what the code means; `--all` lists every code; no path |
| `ply doc NAME [path]` | a definition or builtin: signature with the written parameter names, the `//` lines above it, place, hash, footprint; a builtin's note comes from the compiler's table |
| `ply fmt [paths]` | rewrite every `.ply` file under the paths in the canonical layout; `--check` writes nothing and exits 1 naming the files that would change |
| `ply show NAME [path]` | one `fn` or `type` as its file holds it: the `//` lines above it, `pub`, the body, and a comment ending its last line; `--json` adds the byte range |
| `ply replace NAME [path]` | rewrite one `fn` or `type` from `--with FILE` or stdin, formatted, every other byte of the file kept; refused with `E0128` (exit 2, nothing written) unless the program still checks and no other definition's name or hash moves; `--check` writes nothing |
| `ply hash [path]` | `--deps` (references and transitive closure) |
| `ply defs [path]` | every definition: place, hash, signature, footprint, references; `--filter SUBSTRING` |
| `ply callers DEF [path]` | what mentions a definition directly, and every definition, test and law whose closure reaches it |
| `ply bootstrap <path>` | writes the front end as C: `--out DIR` (default `bootstrap`), `--verify` (compare, write nothing), `--profile` (default `release`) |
| `ply cache clear\|stats\|compact [path]` | discard results / report size and reclaimable space / reclaim it |
| `ply cache inspect <DEF> [path]` | one definition's entries, by full name, simple name or 4+ hex hash prefix |

`ply fmt`, `ply defs`, `ply hash`, `ply doc` and `ply explain` are one Ply
program (`crates/ply-cli/ply`, entered at `ply.main`). `ply` parses the command
line, binds the directory the load is rooted at as the program's one writable
filesystem root and the modules it ships as a read-only second one, and answers
with the code the program asked to exit with. A path is therefore relative to
its root, and one that leaves it is refused with `E0452`. The first run after
`ply` or the program itself changes compiles the program's unit, which needs the
C toolchain `ply run` needs and takes a few seconds; every later run loads the
compiled object and the front end it filed beside it.

`ply fmt` keeps comments, the spelling of every literal, and the order of
imports, items and statements; it prints `formatted PATH` per file it changed
and leaves a file that does not parse alone, exiting 2 with the diagnostic. A
directory whose name starts with `.`, and one named `target`, are not walked; a
symlink found while walking is passed over, and one named on the command line is
an error rather than a file to rewrite.

`ply show NAME` and `ply replace NAME` are the edit loop for one definition: read
it, rewrite it, and touch nothing else in the file. The replacement is one item
of the same kind and name, with its own `//` lines above it; `replace` prints it
through `ply fmt` into the range `show` reports.

## 17. Diagnostics

`E` is an error; `W` is a warning and never a fault in your program.
`ply explain CODE` prints a code's line from this table, and `--all` the whole
table, from the registry the compiler raises from.

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
| `E0113` | project module under the reserved root `std` |
| `E0114` | unknown `effect set`, including a `pub` or qualified one |
| `E0115` | `effect set` cycle |
| `E0116` | record update base with no shape this file can name |
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
| `E0201` | type mismatch |
| `E0202` | arity mismatch |
| `E0203` | occurs check |
| `E0204` | not a function |
| `E0205` | non-exhaustive match |
| `E0206` | not derivable, including an unordered `Map` key |
| `E0207` | unknown deriver |
| `E0208` | orphan `derive` |
| `E0209` | `/` on `Decimal` |
| `E0210` | numeric operand type nothing determines |
| `E0211` | integer literal out of range for its fixed width |
| `E0301` | unbound row variable |
| `E0302` | effect not permitted by the written row |
| `E0303` | unhandled effect (compiler defect) |
| `E0304` | resource label required |
| `E0305` | `handle` leaves an operation, or a mode atom, under a handled mode atom unanswered |
| `E0306` | label instantiation: a call leaves a label unfilled or writes the wrong number of them, or a label-generic definition is used as a value |
| `E0307` | mutually recursive definitions binding different label parameters |
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
| `E0430` | `--tls` credential that does not load |
| `E0431` | no database configured |
| `E0432` | statement text the driver refuses |
| `E0433` | server refused to prepare a statement |
| `E0434` | statement touches a table outside the footprint |
| `E0435` | live database differs from the schema (reserved) |
| `E0436` | database operation from a task not owning the transaction |
| `E0437` | connection pool exhausted |
| `E0438` | live schema has an unmodellable trigger, rule or cascade (reserved) |
| `E0439` | `Secret` passed to a host operation not allowed one |
| `E0440` | configuration source unreadable |
| `E0441` | required configuration key missing |
| `E0442` | configuration value of the wrong shape |
| `E0443` | artifact does not verify |
| `E0444` | artifact built under another version |
| `E0445` | `trace.exit` of a span not open on this task |
| `E0446` | value outlives its region |
| `E0447` | two regions in scope under one name |
| `E0449` | region handle reaching a runtime boundary |
| `E0450` | compiled backend cannot be attached |
| `E0451` | `fs` label with no root bound |
| `E0452` | path leaves its root |
| `E0453` | whole-file read over the bound |
| `E0454` | `--fs` root that is not a directory |
| `E0455` | the program asked to exit with a code |
| `E0501` | assertion failed |
| `E0502` | runtime error: `panic`, division by zero, overflow, bad index, spent budget, call limit |
| `E0503` | ran past its time budget |
| `E0505` | Ply broke one of its own invariants |
| `W0601` | cache unreadable |
| `W0602` | cache corrupt |
| `W0603` | cache from another version |
| `W0604` | obligation undecided at every tier |
| `W0605` | standard library changed since the cache was written |
| `W0606` | host runtime could not release every resource |
| `W0607` | supplied configuration key the schema does not declare |
| `W0608` | drain deadline expired with requests in flight |
| `W0609` | spans still open when an entry point ended |
| `W0610` | reference cycle, never freed |
| `W0611` | definition no `pub` item, `main`, test or law reaches; a leading `_` in its name keeps it quiet |

## 18. What Ply does not have

* No loops, `break` or `return` (`?` is the only early exit); no mutable
  variables; no exceptions; no typeclasses, implicits or method syntax; no
  modules-as-values or first-class effects; no `unsafe` or FFI.
* Specs cannot name mutable state. Cycles are not collected, and a task never
  moves between OS threads.
* No file handles, streaming, recursive walk, permissions or `stdin`; no
  cancellation or backpressure; no migrations or live schema check; HTTP/1.1
  only; no authentication framework.

Sharp edges: `x.f(y)` with a bare variable `x` is a perform; a missing handler
clause fails at run time; record update needs a locally readable shape; two
allocating tasks are always ordered; `bytes_at`, `bytes_u32_le`, `string_slice`,
`string_find` and `list_set` raise where `list_at` answers `None`.

## 19. Examples

In `examples/`: `clock.ply` (a `nondet` effect, a handler, `test/nondet`);
`ledger.ply` and `report.ply` (modules, specs, laws); `pipeline.ply`, `bank.ply`
and `timeout.ply` (simulation, a race and its fix, a virtual clock); `echo.ply`
and `hello.ply` (sockets, an HTTP endpoint); `orders.ply` (`derive json`);
`relay.ply` (one forwarder generic over the label it writes under);
`store.ply` (a handler as a capability grant); `agreement.ply` and
`twin_divergence_audit.ply` (`std.db`'s twin against recorded PostgreSQL
answers); `desk.ply` (a PostgreSQL service with TLS, config, tracing and
shutdown).
