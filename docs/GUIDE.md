# The Ply Guide

Ply is a small, statically typed, effect-tracked functional language. It has no
loops, no mutable variables, no classes, no exceptions and no dispatch
mechanism. What it has instead is an effect system precise enough that the
compiler knows which resources every function touches, a content-addressed
compilation model in which the unit of work is the *definition* rather than the
file, and a test runner that can prove a test does not need to run again.

The bet the language makes is stated in [DESIGN.md](../DESIGN.md): writing code
is getting cheaper, and knowing whether it is correct is not. Everything below
is downstream of that.

This document is the user-facing manual. It assumes you can program, and it
does not assume you know anything about effect systems. [DESIGN.md](../DESIGN.md)
is the design rationale, [ROADMAP.md](../ROADMAP.md) is the development record,
and `docs/adr/` holds the decision records each section cites.

**Contents**

1.  [Getting started](#1-getting-started)
2.  [A tour of the language](#2-a-tour-of-the-language)
3.  [Lexical structure](#3-lexical-structure)
4.  [Modules, files and visibility](#4-modules-files-and-visibility)
5.  [Types](#5-types)
6.  [Expressions](#6-expressions)
7.  [Effects and handlers](#7-effects-and-handlers)
8.  [Cells and regions](#8-cells-and-regions)
9.  [Tests](#9-tests)
10. [Simulation and concurrency](#10-simulation-and-concurrency)
11. [Specifications, laws and proof](#11-specifications-laws-and-proof)
12. [Derivation](#12-derivation)
13. [The builtin library](#13-the-builtin-library)
14. [The standard library](#14-the-standard-library)
15. [The host boundary](#15-the-host-boundary)
16. [Building and shipping](#16-building-and-shipping)
17. [The `ply` command](#17-the-ply-command)
18. [Diagnostics](#18-diagnostics)
19. [Limits and things Ply does not have](#19-limits-and-things-ply-does-not-have)
20. [Where to go next](#20-where-to-go-next)

---

## 1. Getting started

### 1.1 Build the compiler

Ply ships as a Rust workspace. There is no toolchain file and no build script:

```
$ cargo build --release
$ target/release/ply --version
ply 0.1.0
```

Two binaries land in `target/release/`: `ply`, the language driver, and
`ply-corpus`, the measurement harness (which you do not need). Put `ply` on your
path, or call it by path — this guide writes `ply`.

### 1.2 Hello, Ply

A Ply file is a module. Make a directory with one file in it:

```ply
// hello/main.ply

fn greeting() -> String = "hello from ply"

fn main() -> Unit = assert_eq(greeting(), "hello from ply")
```

```
$ ply run hello
   ()
```

`ply run` evaluates `main`. It prints the value `main` returned — here `()`, the
unit value — and exits `0`.

There is no `print`. Output is an effect, and §7 is where effects are introduced;
until then, programs communicate by returning values and by asserting inside
tests.

### 1.3 The loop

Three commands do almost all of the work:

```
$ ply check .        # parse, resolve, typecheck, infer effect rows
$ ply test .         # select, schedule and run the tests
$ ply run .          # evaluate `main`
```

Each takes a path: a `.ply` file, or a directory that is the project root. The
path defaults to `.`, so inside a project you can write `ply test`.

`ply check --types` prints the inferred signature of every definition, which is
the fastest way to see what the compiler thinks your program does:

```
$ ply check demo --types
   checked 1 module, 3 definitions, 1 test

   main demo/main.ply
     credit : ({balance: Int, name: String}, Int) -> {balance: Int, name: String}
     total  : (List<{balance: Int, name: String}>) -> Int
     main   : () -> Int
     test "credit moves one account" : {}
```

### 1.4 Projects, roots and the cache

A **project root** is a directory. Every `*.ply` file underneath it — at any
depth, except inside directories whose name starts with `.` — is a module, named
after its path relative to the root with `/` replaced by `.` and the `.ply`
extension dropped:

```
proj/                     root
  main.ply                module `main`
  store/items.ply         module `store.items`
  store/orders/place.ply  module `store.orders.place`
```

Every directory name and every file stem must be a plain identifier, or the file
is `E0111`. The module name `std` and anything under it is reserved for the
standard library (`E0113`).

When you name a single file instead of a directory, that file's parent is the
root and it is the only module loaded.

The first `ply check` or `ply test` in a project creates a `.ply-cache/`
directory at the root. It holds the front-end cache (parsed and typed
definitions), the result cache (which tests have passed), the obligation cache
(which specifications have been discharged) and the review baseline. It is safe
to delete; `ply cache clear` is the supported way. Add it to `.gitignore`.

---

## 2. A tour of the language

This section is a working introduction. Everything in it is spelled out again,
precisely, in §3 onwards.

### 2.1 Definitions

```ply
type Account = { name: String, balance: Int }

fn credit(a: Account, amount: Int) -> Account =
  {name: a.name, balance: a.balance + amount}

fn total(accounts: List<Account>) -> Int =
  fold(accounts, 0, |acc, a: Account| acc + a.balance)
```

`fn name(params) -> Ret = expression` is the whole of a function. There is no
`return`: a function *is* its expression. When the body is a block, write it
without the `=`:

```ply
fn credited(a: Account, amount: Int) -> Account {
  let moved = a.balance + amount;
  {name: a.name, balance: moved}
}
```

A parameter may carry a default, which a call is then free to leave out:

```ply
fn credit(a: Account, amount: Int, note: Option<String> = None) -> Account =
  {name: a.name, balance: a.balance + amount}
```

The default is spliced into the call before anything else sees it, so
`credit(a, 5)` and `credit(a, 5, None)` are the *same definition* with the same
hash and the same cache entry — adopting a default re-runs nothing. It has to be
a value rather than something that runs: a literal, a constructor applied to
literals, a record or a list. A call or a `perform` in a default would run at
the caller rather than where it was written, and is `E0121`. It also may not
mention the signature's other parameters, which do not exist at a call site.

Only a `fn` may carry one. A lambda is reached through a value rather than by
name, so there is no signature for a call to be matched against, and a default
written on one is `E0120`.

Everything is a value and everything is immutable. `credit` does not modify the
account it is given; it builds a new one. There are no loops, so `fold`, `map`,
`filter` and recursion are how you iterate (§6.9).

### 2.2 Tests are part of the language

```ply
test "credit moves one account" {
  assert_eq(credit({name: "ada", balance: 10}, 5).balance, 15)
}
```

`test` is an item, like `fn`. There is no test framework, no discovery
convention and no decorator. The runner sees tests because the parser did.

```
$ ply test demo
   selected 1 of 1 (0 cached)
   1 group · 10 workers
   isolated 1 of 1

   ok    credit moves one account      0.5ms

   0 failed, 1 passed, 0 cached (0.02s)
```

Run it again and nothing runs:

```
$ ply test demo
   selected 0 of 1 (1 cached)
   isolated 1 of 1

   0 failed, 0 passed, 1 cached (0.00s)
```

That is not a timestamp heuristic. Every definition has a **content hash** taken
over its normalized structure — locals renamed to de Bruijn levels, references
replaced by the hash of what they refer to, names and comments and formatting
erased. A test's hash therefore covers everything it can reach. The cache maps
`(runtime version, test hash) → pass`, and a test is selected exactly when its
hash is not in the cache.

### 2.3 The rename that re-runs nothing

Because a reference is stored as the *hash* of its referent rather than as a
name, renaming a function changes no hash anywhere:

```
$ ply hash demo
     37d6c55a2653  credit
     43d26e128360  total
     bd839a4142eb  main
     0ec363399cd6  test "credit moves one account"

$ sed -i '' 's/credit/credit_account/g' demo/main.ply
$ ply hash demo
     37d6c55a2653  credit_account
     43d26e128360  total
     bd839a4142eb  main
     0ec363399cd6  test "credit moves one account"

$ ply test demo
   selected 0 of 1 (1 cached)
   0 failed, 0 passed, 1 cached (0.00s)
```

Selecting zero tests after a project-wide rename is a property the compiler's
own test suite asserts, not a heuristic that usually works.

### 2.4 Effects, in sixty seconds

A function's type carries an **effect row**: the set of things it may do.

```ply
nondet effect clock {
  read now() -> Int
}

fn expired(started: Int, ttl: Int) -> Bool / {clock.read} =
  clock.now() > started + ttl
```

`/ {clock.read}` is the row. `clock.read` is an *atom*: an effect, a resource and
a mode. Rows are inferred; writing one down makes it a published signature that
inference must fit inside.

A **handler** discharges an effect, which is how a real resource is replaced by
an in-memory one with no mock library:

```ply
test "expiry is decided against the deadline, not the wall clock" {
  handle {
    assert(!expired(1000, 60))
  } with {
    clock.now() -> 1060,
  }
}
```

The handler removes `clock.read` from the row, so this test is deterministic and
cacheable. Leave the handler out and it does not become flaky — it fails to
compile, with `E0412`, because `clock` was declared `nondet` and a deterministic
test may not retain a nondeterministic atom.

That is the shape of the whole language: what a piece of code can do is in its
type, and everything else — exact test selection, provable test isolation,
concurrent scheduling, race search — is computed from that.

---

## 3. Lexical structure

### 3.1 Encoding, whitespace and comments

Source files are UTF-8. Whitespace is insignificant except as a token separator;
there is no layout rule.

There is exactly one comment form:

```ply
// a line comment, to the end of the line
```

There is no block comment and no documentation comment. Comments are erased by
normalization, so editing one changes no hash and re-runs no test.

### 3.2 Identifiers

An identifier starts with an alphabetic character or `_` and continues with
alphanumerics or `_`. Unicode letters are allowed. `_` on its own is the
wildcard token, not an identifier.

Case is meaningful in two places, and only in those two:

* a **type** written as a bare lowercase name is a type *variable* bound by the
  enclosing `<...>`; a name starting uppercase is a type constructor;
* a **pattern** written as a bare lowercase name binds a variable; one starting
  uppercase is a constructor pattern.

Elsewhere, case is convention. The conventions the standard library follows are
`snake_case` for functions and values, `UpperCamelCase` for types and
constructors, and lowercase for effects and resource labels.

### 3.3 Keywords

Reserved everywhere a name is bound or referenced:

```
pub  import  fn  type  effect  nondet  test  let  if  else  match  handle  with
true  false
```

A **field name** is the one exception: a keyword names a record field in a
type, a literal, a pattern, after `.` and in an update, because a field position
has no other reading — `{nondet: Bool}` and `d.nondet` are fine. The punned
forms `{nondet}` and `{nondet, ..}` are not, since they also bind a *variable*
of that name; write `nondet: n`.

The following are **contextual**: they are keywords only in the one position
where nothing else is grammatical, and are ordinary identifiers everywhere else.

| word | where it is a keyword |
| --- | --- |
| `read`, `write` | opening an operation declaration, or after `.` in an effect atom |
| `set` | between `effect` and a name, in `effect set X = {..}` |
| `law` | at item position, followed by a string or by `/host` |
| `derive` | at item position, followed by an identifier |
| `reuse` | at item position (after `pub`, if any), followed by `fn` |
| `for` | in `derive <deriver> for <Type>` |
| `where` | after a signature's row (constraints), or after a `law`'s binders (guard) |
| `derivable` | inside a `where` constraint |
| `requires`, `ensures` | between a `fn` header and its body |
| `forall` | after a `law`'s label |
| `resume` | between a handler clause's `)` and its `->` |
| `return` | as the first word of a handler clause, followed by a binder |
| `host` | after `law/` |
| `with_cell`, `with_region` | followed by `[` |
| `simulate` | followed by `{` where a `{` can open an expression |

So `fn law(x: Int)`, a local named `resume` and a field named `set` all keep
their meaning.

### 3.4 Literals

| form | type | notes |
| --- | --- | --- |
| `42`, `1_000_000` | `Int` | 64-bit signed. `_` separators anywhere between digits. A literal that does not fit is a lex error. |
| `0xFF`, `0xdead_beef` | `Int` | The same type and the same value as the decimal spelling — `0xFF` and `255` are one literal and one definition hash. The bound is 64 bits *as a bit pattern*, so `0xFFFF_FFFF_FFFF_FFFF` is `-1`. No hex `Float` and no hex `Decimal`. |
| `1.5`, `1e9`, `2.5e-3` | `Float` | IEEE-754 binary64. A fraction or an exponent is what makes a literal a `Float`. |
| `1.50m`, `0m`, `12345m` | `Decimal` | Base-10, exact. Up to 28 fractional digits and a 96-bit mantissa. No exponent form. |
| `"text"` | `String` | UTF-8. May not span a line break. |
| `b"GET "` | `Bytes` | ASCII source characters plus `\xNN`. |
| `true`, `false` | `Bool` | |
| `()` | `Unit` | |

**The three numeric literal forms have three distinct types**, and there is no
implicit conversion between them: `1`, `1.0` and `1m` are three different
values with three different definition hashes. `fn f() -> Int = 1.0` is
`E0201`.

`Decimal` keeps the scale it was written with. `1.50m` is mantissa 150 scale 2
and `1.5m` is mantissa 15 scale 1: equal in value, differently hashed.

A literal is never negative: `-3` is unary minus applied to `3`. The one
exception is inside a **pattern**, where a leading `-` on a numeric literal is
part of the pattern, because a pattern is not an expression and there is nothing
to apply.

**String escapes** are `\n`, `\t`, `\r`, `\0`, `\\` and `\"`. Anything else is an
error; there is no `\u` escape (write the character).

**Byte-string escapes** are those six plus `\xNN` with exactly two hex digits. A
source character above `U+007F` inside `b"..."` is refused, so the bytes of a
literal never depend on how the file was saved — the diagnostic tells you the
`\xNN` sequence to write instead.

### 3.5 Operators and precedence

From loosest to tightest:

| precedence | operators | associativity | operand types |
| --- | --- | --- | --- |
| 1 | `\|\|` | left | `Bool` |
| 2 | `&&` | left | `Bool` |
| 3 | `==` `!=` `<` `<=` `>` `>=` | left | see below |
| 4 | `\|` | left | `Int` |
| 5 | `^` | left | `Int` |
| 6 | `&` | left | `Int` |
| 7 | `<<` `>>` `>>>` | left | `Int` |
| 8 | `++` | left | `String` |
| 9 | `+` `-` | left | `Int`, `Float` or `Decimal` |
| 10 | `*` `/` `%` | left | `Int`, `Float` or `Decimal` |
| — | unary `-`, unary `!`, unary `~` | prefix | numeric / `Bool` / `Int` |
| — | `f(x)`, `r.field`, `e.op[r](x)`, `e?` | postfix | |

Notes that matter:

* `==` and `!=` are **structural equality** at any type except a function type
  (`E0201`, "functions cannot be compared for equality"). They work on records,
  sums, lists, maps and `Secret` — a `Secret` comparison answers one bit, which
  is the only thing a credential is allowed to tell you. At `Float` they are
  IEEE equality, so `NaN != NaN`, which is why a `Float` cannot be a `Map` key
  (§5.4).
* `<`, `<=`, `>`, `>=` are defined at `Int`, `Float` and `Decimal` and nowhere
  else. To order other values use `compare`, which returns an `Ordering`.
* `++` is string concatenation only. There is no list or bytes `++`; use
  `bytes_concat` / `push` / `fold`.
* `/` applied to `Decimal` is refused (`E0209`): the exact quotient of two
  decimals is not in general a decimal, so the operator would have to round, and
  a rounding nobody wrote down is the defect the type exists to prevent. Use
  `decimal_div(a, b, scale, mode)`. `%` on `Decimal` *is* allowed.
* `&&` and `||` short-circuit. The **bitwise** operators are `&`, `|`, `^` and
  unary `~`, and they are defined at `Int` and nowhere else — a `&` between two
  `Bool`s is `E0201`, not a non-short-circuiting `&&`. They operate on the
  two's-complement bit pattern, so `~0` is `-1`.
* The shifts are `<<`, `>>` (arithmetic, sign-propagating) and `>>>` (logical,
  zero-filling). A count outside `0..=63` **raises** `E0502`, for the reason a
  zero divisor does: there is no answer, and C's undefined behaviour, Rust's
  panic and Java's silent mask by 63 are three different inventions of one.
  `<<` is the one place arithmetic is *not* checked — it discards the bits
  shifted out rather than raising, because a shift is a bit operation and a
  hash's mixing step is defined to drop them (ADR 0033 §2.2).
* **`>>` is not a token.** It is two adjacent `>`, joined only where an operator
  can appear, which is what lets `Map<Int, List<Int>>` keep closing on two of
  them. `a > > b` is still the syntax error it always was, because the two must
  be written together.
* `::` qualifies a name through a module binder (`items::price_of`). It is not an
  operator and cannot be chained: a module binder is a single name.
* `.` is field access, unless it is followed by an operation name and then a `[`
  or `(`, in which case it is an effect perform (§7.2).
* `?` is postfix and binds tightest, so `f(x)?.field` is `(f(x)?).field`, `-x?`
  is `-(x?)` and `a == b?` is `a == (b?)`. It is not a ternary — Ply has no
  `?:` — and it is not an operator on a value: it is sugar the parser expands
  (§6.10).

`,` `;` `:` `->` `|` `=` `..` `_` `?` `[` `]` `{` `}` `(` `)` complete the token
set.

---

## 4. Modules, files and visibility

### 4.1 One file, one module

A module has no header. Its name comes from its path (§1.4). A file is:

```
<imports>
<items>
```

Imports must come before every item. Items are `fn`, `type`, `effect`,
`nondet effect`, `effect set`, `test`, `law` and `derive`, in any order —
there is no declare-before-use rule, and definitions may be mutually recursive
across the whole program.

### 4.2 Imports

```ply
import store.orders                 // binds the module as `orders`
import store.orders as ord          // binds it as `ord`
import store.orders (place, cancel) // binds those names unqualified, no module binder
```

* A plain `import` binds the module under its **last** path segment. Reach into
  it with `::`: `orders::place(...)`.
* `as` renames the binder.
* The selective form binds the listed names directly and introduces *no* module
  binder. You may write both forms for one module if you want both.
* Module binders live in their own namespace, so a local variable named `orders`
  does not shadow the module binder `orders`.
* You may not combine `as` and a name list in one import.

Imports are metadata. They are erased by normalization, so adding, removing or
reordering them changes no definition hash.

### 4.3 Visibility

Items are private to their module unless marked `pub`:

```ply
pub type Item = { sku: String, price: Int }
pub fn price_of(items: List<Item>, sku: String) -> Int = ...
fn hidden() -> Int = 1
```

Reaching a private name from another module is `E0107`, and the diagnostic points
at the declaration and tells you to add `pub`.

`pub` applies to `fn`, `type` and `effect`. It is refused on `test`, `law`,
`derive` and `effect set`, none of which has a name another module could
reference.

Visibility is erased by normalization too: adding `pub` re-runs nothing.

### 4.4 Namespaces

There are three: **values** (functions and constructors together), **types**, and
**effects**. A module may declare `fn size`, `type Size` and `effect size`
without collision. Constructors live in the value namespace, which is why an
expression cannot tell a nullary constructor from a function reference.

### 4.5 Entry point

`ply run` evaluates `main`. A project must contain exactly one `main`; zero is
`E0101` ("no `main` to run"), and more than one is `E0112`, which names the
candidates. Since naming a single file loads only that file, a project with
several `main`s is run by naming the file you mean:

```
$ ply run examples/hello.ply --host
```

`main` may have any return type and any effect row; a hermetic run refuses any
effect that reaches the host boundary (§15).

---

## 5. Types

Ply's types are inferred by Hindley–Milner unification with row polymorphism over
effects. You may annotate anything; you must annotate almost nothing. Where you
do write an annotation it becomes the published signature, and inference is
checked against it.

### 5.1 Scalars

| type | values |
| --- | --- |
| `Int` | 64-bit signed integers. Arithmetic is **checked**: overflow raises `E0502` rather than wrapping. Two exceptions, both deliberate: `<<` discards the bits it shifts out, and `wrap_add`/`wrap_sub`/`wrap_mul` (§13.10) wrap by definition. |
| `Float` | IEEE-754 binary64. `NaN != NaN`; not orderable as a map key. |
| `Decimal` | Exact base-10 with a scale. Money. `+`, `-`, `*`, `%` are exact or they raise; `/` is `E0209`. |
| `Bool` | `true`, `false` |
| `String` | UTF-8 text. Indexed and sliced by **character** (Unicode scalar value). |
| `Bytes` | An immutable byte string. Indexed and sliced by byte. |
| `Unit` | one value, `()` |

**`String` and `Bytes` are different types all the way down**, including in
definition hashes. Data that arrived from outside the program is `Bytes` until
something decodes it, because a peer is free to send bytes that are not UTF-8.
`bytes_of_string` always succeeds; `string_of_bytes` raises on invalid UTF-8 and
`string_of_bytes_lossy` substitutes U+FFFD.

### 5.2 Numbers, and why there are three

There is no numeric tower and no implicit widening. `a + b` requires both sides
to have the same numeric type, and mixing two is one `E0201`.

The operand type of an arithmetic operator is not decided at the node — it is
usually still unknown there — but once the enclosing definition has been
inferred. So both of these check:

```ply
fn f(a: Float) -> Float = a + 1.0
fn g(a: Decimal) -> Decimal = 1m + a
```

An operand type **nothing pins** is `E0210`, not a default. Since every
top-level signature is written (§5.9) the only way to reach it is a lambda
binder or a `let` no annotation and no literal constrains:

```ply
fn f() -> Int = { let g = |a, b| a + b; 1 }   // E0210 on `a + b`
```

This used to default to `Int`. A default is a tiebreak taken inside the compiler
that then appears in a published signature, which is exactly the kind of claim
nobody wrote and nobody can review; annotate the binder, or write a literal that
pins it.

Conversions are explicit: `decimal_of_int`, `int_of_decimal` (takes a rounding
mode, answers `Option`), `float_of_decimal`, `decimal_of_float` (`Option`),
`decimal_of_string` (`Option`), `decimal_to_string`, `int_to_string`, and the
IEEE 754 bit pattern both ways, `bits_of_float` and `float_of_bits`, which are
total: every pattern is a `Float`, NaNs included.

### 5.3 Records

Records are **structural**. There is no record declaration — a record type is
written as its field list, and two records with the same fields are the same
type whatever they are called:

```ply
type Account = { name: String, balance: Int }

fn f(a: Account) -> Int = a.balance
fn g(r: {name: String, balance: Int}) -> Int = f(r)     // fine, same type
```

`type Account = ...` is an **alias**, not a nominal type. `ply check --types`
prints the expanded structural form, and it prints fields in sorted order,
because field order in a record type is not significant and reordering one
changes no hash.

Construction is `{field: value, ...}`. A field written bare is shorthand for
`field: field`:

```ply
fn point(x: Int, y: Int) -> {x: Int, y: Int} = {x, y}
```

A field may be named with a keyword (`{type: 1, nondet: false}`); only the bare
shorthand needs an ordinary name, since it binds a variable too (§3.3).

Field access is `r.field`. There is no field update in place — see §6.6 for the
record-update form.

**A tuple is a record with positional fields.** `(A, B)` is the type
`{_0: A, _1: B}`, `(a, b)` the value `{_0: a, _1: b}`, and `(p, q)` the pattern
`{_0: p, _1: q}` — the same mechanism in every position, so a tuple hashes,
derives and unifies as the record it is. Two or more elements make one: `(A)`
still groups, `()` is still `Unit`, and `(A, B) -> C` is still a function
type. Field access is `t._0`. A type or a value whose fields are
exactly `_0..` prints as the tuple it was written as, in `ply check --types`
and in a failing assertion alike:

```ply
fn divmod(a: Int, b: Int) -> (Int, Int) = (a / b, a % b)

fn half(p: (Int, Int)) -> Int = match p { (q, _) -> q }

test "quotient and remainder" {
  let (q, r) = divmod(17, 5);
  assert_eq((q, r), (3, 2))
}
```

Name the fields when the pair is worth a name; a tuple is for the pair that
is not.

### 5.4 Lists and maps

`List<a>` is an immutable sequence, written `[a, b, c]`. It is homogeneous, and
it is indexed by **position** — `list_at(xs, i)` (§6.7). Reach for a `Map<k, v>`
when your keys are not positions. Not for the speed: a `Map<Int, v>` used as an
array costs within about a tenth of what the list index costs (§6.7), so the
reason to prefer the list is that a position is what you actually have.

`Map<k, v>` is an immutable sorted map. It has no literal — build it from
`map_new()` and `map_insert`, or from `map_of_entries` over a
`List<{key: k, value: v}>`.

A map iterates in the canonical total order over keys, which is the order
`compare` gives. That is what makes `map_keys`, `map_entries` and `map_fold`
functions of the map's contents rather than of how it was built.

**A map's key type must be ordered**: `derivable(ord, k)`. `Float` is refused
(`E0206`) because `NaN` is not equal to itself, so a `Float` key would have no
position the next lookup could find it at. `Secret` is refused for the same
family of reasons (§5.7). A type containing a function, a `Cell` or a `Task` is
refused because those name locations rather than values.

### 5.5 Sum types

```ply
type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point

type Level = Debug | Info | Warn | Error
```

The leading `|` is optional. A variant with no payload takes no parentheses.
Constructors are ordinary values in the value namespace: `Circle(3)` is a call,
`Point` is a reference.

`type T = A` with a single uppercase name and no payload and no `|` is an
**alias**, not a one-variant sum — so `type Id = Int` means what it looks like. A
sum needs a leading `|`, a payload, or a second variant.

**Sum types are the only nominal types in the language.** Two structurally
identical sums declared in two modules are two different types, and a diagnostic
says so by module-qualified name (`expected `a.Colour`, found `b.Colour``).
Records, aliases and every builtin type are structural.

### 5.6 Type parameters and generics

```ply
fn map_pair<a, b>(x: a, f: (a) -> b) -> b = f(x)
```

Type parameters are declared in `<...>` and written as bare lowercase names. A
lowercase bare name in type position is always a type variable; there is no way
to write a lowercase concrete type.

Effect-row parameters go after a `|` in the same list:

```ply
fn apply<a, b | e>(x: a, f: (a) -> b / e) -> b / e = f(x)
```

Writing `<a, b, e>` puts `e` in the *type* namespace, and using it as a row is
then `E0301 unbound row variable`. The `|` is what separates the two kinds. A
function with only effect parameters is written `<| e>`, as `std.http`'s
`serve_connection<| e>` does.

Type aliases may be parameterized: `pub type Route<a> = { ... endpoint: a }`.

### 5.7 The types the language declares

**`Option<a>`**, **`Result<a, e>`**, **`Ordering`**, **`Rounding`** and
**`Iter<s, r>`** are declared by the language rather than by a module. They are in
scope everywhere with no import, and a project may not declare a type of the same
name (`E0105`). Their constructors are:

```ply
Option<a>     = None | Some(a)
Result<a, e>  = Ok(a) | Err(e)
Ordering      = Less | Equal | Greater
Rounding      = HalfEven | HalfUp | Down | Up | Ceiling | Floor
Iter<s, r>    = Continue(s) | Stop(r)
```

Constructor names are *not* globally reserved, so a module may declare its own
`Stop` — at the cost of shadowing the prelude's and losing `iterate` in that
module. A module that declares its own `Ok`, `Err`, `Some` or `None` also loses
`?` (§6.10) in that module, because the expansion would name its constructors
rather than the prelude's — and so does one that *imports* any of those four
names unqualified (`import m (Err)`), which binds them the same way. Import the
module and write `m::Err` instead, and `?` keeps working.

`?` (§6.10) binds one of `Option` or `Result` inside a function that returns the
same one.

**`Secret<a>`** is a credential. It is introduced by `secret_of_string` and
eliminated only by `secret_verify` (a constant-time comparison answering one
bit), `secret_is_empty`, and `==` — which also answers one bit, and is the only
deriver a `Secret` admits. Nothing renders it, nothing encodes it and nothing
orders it: `"password: " ++ s` where `s: Secret<String>` is a type error, and
`derive json` and `derive ord` refuse it, because an encoding writes the value
out and an ordering recovers it in calls proportional to its length. A host
operation may receive one only if its registration declares it may (`E0439`).

**`Cell<a>`** is a mutable slot inside a region, and **`Task<a>`** is a handle to
a spawned task. Both are branded by the region that created them and neither can
outlive it (§8, §10). The brand is not a type argument you can write — it is
carried alongside, and printed in brackets, so a diagnostic reads
`Cell[users]<Int>`. You very rarely write either type: they appear in inference
and in diagnostics.

### 5.8 Function types

```ply
(A, B) -> C            // pure: an empty row
(A) -> B / {db.read[users]}
(A) -> B / e           // effect-polymorphic
() -> A
```

A function type with no `/` written has an *inferred* row in a signature
position and an empty row in a declared type. Functions are first-class: they can
be passed, returned and stored in records. They cannot be compared, encoded,
ordered or used as map keys.

### 5.9 What is checked, and what is inferred

The line runs between what a definition **means** and what it **does**, and it
is not the line most languages draw:

* **Types are written.** Every parameter type and every return type on a
  top-level `fn` is mandatory. Omitting one is `E0126`, and the diagnostic names
  the type inference would have given, so the fix is the text of the error.
* **Effect rows are inferred.** Omit the `/ {...}` and the row is derived from
  what the body performs. Write one and it becomes the published row, checked as
  an **upper bound**: inference must produce a **subset**. So declaring
  `/ {net.write[conn]}` on a function whose body performs nothing is allowed and
  useful (it constrains callers); declaring less than the body needs is `E0302`.
* **Inside a body, everything is inferred.** Lambda binders, `let` bindings and
  every intermediate expression. Nothing there is published, so nothing there
  has to be written.

**Why the asymmetry.** A row is *derived* — it is a summary of what you called,
and it changes for good reasons, so nearly every row in the shipped tree is left
to inference. A type is *chosen*; it is a claim about what a definition means. Ply's
premise is that what a human reviews is a specification (§11), and
`ply review --changed`'s load-bearing row is *implementation changed, spec
unchanged*. A signature inferred from the body it describes cannot hold still
for that row to mean anything — editing the body would silently republish the
claim. So: infer what is mechanical, write what is meant.

Two consequences worth knowing:

* **A local `let` binds monomorphically.** `let f = |x| x;` used at two
  different types is `E0201`. A polymorphic helper is a `fn`, where its
  signature is written and therefore reviewable.
* **There is no numeric defaulting.** An operand no annotation and no literal
  pins is `E0210` rather than silently becoming `Int` (§5.2).

Also mandatory: `forall` binders in a `law`. And where a record update's base
needs a shape, it must be readable from this file — see §6.6.

---

## 6. Expressions

Ply is expression-oriented. `if`, `match`, `handle`, a block — all of them are
expressions with a value.

### 6.1 Blocks, `let` and `;`

```ply
fn settle(a: Account, amount: Int) -> Account {
  let moved = a.balance - amount;
  let name = a.name;
  {name: name, balance: moved}
}
```

A block is `{ statements... tail }`. Its value is the tail expression. The rules:

* a `let` statement is `let <pattern> = <expr>;` — the `;` is required, and the
  pattern may be any pattern, with an optional `: Type` annotation before the
  `=`;
* an expression statement needs a `;` **unless** it is the tail, or unless it is
  "block-like" (an `if`, `match`, `handle`, block, `with_cell`, `with_region` or
  `simulate`), which may be followed directly by the next statement;
* a block with no tail expression has type `Unit`.

`let` shadowing is allowed. There is no mutable binding: `let` binds once.

**"Any pattern" includes a record, and that is how a function returns several
things.** A function answers with a record — or a tuple, which is a record with
positional fields (§5.3) — and the caller takes it apart in the `let` that
receives it.

```ply
type Step = { value: Int, next: Int }

fn advance(input: Bytes, at: Int) -> Step =
  {value: bytes_at(input, at), next: at + 1}

fn sum_two(input: Bytes) -> Int = {
  let {value, next} = advance(input, 0);
  let {value: second, ..} = advance(input, next);
  value + second
}
```

Three forms, all of them patterns from the table in §6.3 and all of them legal
in a `let`:

* `let {value, next} = ...` binds each field to its own name;
* `let {value: second, ..} = ...` renames one and ignores the rest;
* a record pattern must name **every** field or end with `..`, or it is `E0201`
  — the type is the checklist, so adding a field to the record makes every
  exhaustive pattern over it a compile error rather than a silent hole.

The alternative — `let s = advance(input, 0); ... s.value ... s.next` — is legal
and costs an identifier and a field access per call. Prefer the pattern; a
five-thousand-line program written the other way is what prompted this
paragraph.

### 6.2 `if`

```ply
if condition { a } else { b }
if a { x } else if b { y } else { z }
```

Both branches must have the same type. The branches are blocks — braces are
required — and the condition is parsed without allowing a bare `{` to start a
record literal, so `if p { .. }` is never ambiguous.

`if` with no `else` has type `Unit` and its `then` branch must too.

### 6.3 `match` and patterns

```ply
fn area(s: Shape) -> Int =
  match s {
    Circle(r) -> 3 * r * r,
    Rect(w, h) -> w * h,
    Point -> 0,
  }
```

Arms are `pattern -> expression`, separated by `,`. A trailing comma is fine, and
an arm whose body is block-like may omit the comma. An arm may carry a guard:

```ply
[x, y, ..rest] if x > y -> x + len(rest),
```

Pattern forms:

| pattern | matches |
| --- | --- |
| `_` | anything, binds nothing |
| `name` | anything, binds it (lowercase) |
| `Ctor`, `Ctor(p, q)`, `mod::Ctor(p)` | a constructor |
| `42`, `-1`, `1.5`, `1.50m`, `"s"`, `b"s"`, `true`, `()` | a literal (a leading `-` on a numeric literal is part of the pattern) |
| `[]`, `[a, b]`, `[a, ..]`, `[a, ..rest]` | a list of exact length, or a prefix with a rest binder |
| `{a, b}`, `{a: p, b: q}`, `{a, ..}` | a record; `..` allows unlisted fields |
| `(p, q)` | a tuple: the exact record pattern `{_0: p, _1: q}` (§5.3) |

`match` is checked for **exhaustiveness** (`E0205`), and the diagnostic names the
constructor you missed:

```
[E0205] Error: match does not cover every case
 4 │ ╭─▶   match s {
 7 │ ├─▶   }
   │ ╰───────── not covered: `Point`
   │     Note: add the missing arms, or a `_` arm
```

### 6.4 Lambdas

```ply
|x: Int| x + 1
|acc, a: Account| acc + a.balance
|| do_something()                     // no parameters
|r: Result<Int, E>| -> Result<Int, E> { Ok(r? + 1) }   // a written return type
```

Parameter annotations are optional and are usually needed only where inference
has nothing else to go on — in practice, on the element parameter of a `fold` or
`map` over a record type. A lambda closes over its environment by value.

A lambda may write its return type after the parameters, `|x| -> T { .. }`, and
then takes a **block** body, as a `fn` does after its `->` — the type's end and
the body's start are otherwise ambiguous. The body must fit the type, and the
written type is what gives a `?` inside the lambda its meaning (§6.10). It is
not part of the lambda's identity: normalization erases it, as it erases a
`requires`, so writing one moves no hash.

A lambda's row is inferred and flows into the enclosing function's row, which is
what makes `map`, `filter`, `fold` and `iterate` usable with effectful
callbacks.

### 6.5 Calls

```ply
f(a, b)
r.field
items::price_of(catalogue(), "b")
(codec.decode)(json)
```

The last line is a wart worth knowing: a **bare variable followed by `.name(`**
parses as an effect perform, because the parser decides that before it knows
anything about types. So calling a function stored in a record field needs
parentheses around the field access. `int_json().decode(j)` needs none, because
its base is a call rather than a variable.

An argument may be given by name, which is how a parameter that is not last
gets filled without writing out the ones before it:

```ply
greet("ada")                       // greeting takes its default
greet("ada", "hey")                // positional
greet("ada", greeting: "hey")      // by name — the same definition as above
```

The rule is one sentence: **positional arguments fill parameters left to right,
and any parameter left over must be named or have a default.** A positional
argument after a named one is `E0124`; a name that is not a parameter, or one
given twice, is `E0123`.

Leaving a parameter with neither an argument nor a default is `E0202`, the same
arity mismatch it has always been — writing `f(1)` where `f` takes two is
under-application whether or not defaults exist. The one exception is a hole
left when a *name* was used, as in `f(b: 2)` with `a` unfilled: that call cannot
be read as a positional one, so it is `E0125` and names the parameter.

Names are erased before anything hashes, so the second and third lines above are
one definition. A named argument needs a callee reached *by name*: a call
through a value, a lambda or a constructor is positional only.

There is no partial application and no operator section. There is no method
syntax: `x.f(y)` is not `f(x, y)`.

### 6.6 Records, field access and record update

```ply
let widened: Limits = {..base, max: 99};
let deeper: Limits  = {..base, deep: {..base.deep, a: 7}};
```

`{..b, f: e}` copies `b` and replaces the listed fields. It is **sugar**: the
parser rewrites it into the record literal you would have written by hand, so
the two spellings are one definition with one hash. Consequences:

* the base must be a **path** — a variable, or a chain of field accesses off one
  — never a call or a perform, because a base with a call in it would run once
  per copied field;
* the expansion needs the base's field list, and it reads that from **this
  module's own `type` items and the type annotations written in this file**, and
  nothing else. `{..cfg, x: 1}` where `cfg` has a type declared in another module
  is `E0116`, and `{..b, ...}` where `b` came from an unannotated `let` is
  `E0116` too — annotate the binder;
* a field the base does not have is `E0117`. Update replaces; it does not widen.

An update **reuses the base's record** when nothing else holds it (ADR 0034):
the written fields are set into the record the base binding is giving up, and
no new one is built. A base something else still holds is copied once. A
literal that rewrites every field, `{k: s.k + 1, out: push(s.out, i)}`, gets
the same treatment when `s` dies there: it is the shape a state record
threaded through a loop takes, and it allocates nothing per round.

### 6.7 Lists

```ply
[]
[1, 2, 3]
[greeting(), other()]
```

Lists are homogeneous. `push(xs, x)` appends and returns a new list; `len`,
`list_at`, `map`, `filter`, `fold` and `range` are the rest of the surface
(§13).

A list is indexed by position, and the index is **total**: it answers rather
than raises.

```ply
fn third(xs: List<Int>) -> Option<Int> = list_at(xs, 2)

fn third_or_zero(xs: List<Int>) -> Int =
  match list_at(xs, 2) { Some(v) -> v, None -> 0 }

test "an index inside the list, and one outside it" {
  assert_eq(third([10, 20, 30]), Some(30));
  assert_eq(third([10, 20]), None);
  assert_eq(third_or_zero([10, 20]), 0)
}
```

`list_at` answers `None` for an index at or past the end **and for a negative
one**. So `list_at(xs, -1)` is `None`, not the last element — if you came from
Python, that is the one thing to unlearn here. The last element is
`list_at(xs, len(xs) - 1)`, and the first is `list_at(xs, 0)`; there is no
`head` and no `last`, because those are the two lines you just read.

An index costs the same whatever the position for a list no longer than a
leaf, and a few pointer hops past that: a `List` is a radix trie of 32-wide
nodes with its newest leaf held apart (ADR 0034), so `list_at(xs, 99999)` walks
four nodes where `list_at(xs, 0)` on a short list walks none. What the
representation buys is the bound on the *other* operations — see the end of
this section.

**It is not, however, much faster than the `Map<Int, v>` you might reach for
instead**, and the GUIDE says so because the number surprised the people who
added it: about 1.7 µs a peek either way, almost all of it interpreter dispatch
rather than container access. At 14,742 elements the two came out 2% apart,
which is inside what that measurement could resolve; at 128,000, where it can,
`list_at` is about a tenth ahead (ADR 0027). Index a list because positions
are what you have, not because you were promised a speed-up.

`push` grows the list in place when the caller is its last owner, and copies
otherwise. The machine moves a binding's value out of its slot at its last use
(ADR 0034), so *where* the append sits — in a call, in a record literal, first
or last — decides nothing: an accumulator threaded through a loop is linear
however you spell it. What still copies is a genuine second owner — a binding
you read again after the append, a value a closure captured, a cell's contents
or a map's entry read out through `cell_get` / `map_get` (`cell_update` and
`map_update` are the fix, §13.8 and §13.3), a caller that keeps reading what
it passed.

**And a copy is bounded.** A `List` is a radix trie of 32-wide nodes with its
newest leaf held apart (ADR 0034): a push onto a shared list copies one leaf
and one node per level, never the whole list, so an accumulator with a second
owner is still linear — slower than one without, by a constant, and not
quadratic. A `[x, ..rest]` pattern (§6.3) shares the list too: `rest` is an
offset into the same nodes, so walking a list by pattern costs what walking
it with `fold` does. Neither is a rule you have to remember; both are what
lets you not remember one.

**Run `ply check --costs`** to see it per `push` site: every copy is reported
with its cause and, where a source edit removes it, the edit.

**`reuse fn` turns that report into an obligation.** A function marked `reuse`
promises that every `push` in its body reuses its list for every reason the
body controls, and `ply check` refuses the program with `E0127` when the cost
checker cannot show it — naming the append, the promise, and the edit that
would keep it. The promise says nothing about callers: an append onto the
function's own parameter keeps it whatever a caller does with what it passed,
because that copy is the caller's to remove (§13.3, §13.8), and a multi-shot
handler that copies at run time is the semantics, not a broken promise.

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n == 0 { xs } else { collect(push(xs, n), n - 1) }   // kept: xs is a parameter at its last use

reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {
  let ys = push(xs, n);
  if len(xs) < 0 { xs } else { ys }                       // E0127: xs is read again after the append
}
```

`ply test` and `ply run` refuse a broken promise among the modules they parse;
`ply check` parses every module a promise needs and checks all of them. The
standard library's lexer, parser and encoder loops are marked, so a compiler
upgrade that made one of them copy would fail to build.

### 6.8 Effect performs, `handle`, `with_cell`, `with_region`, `simulate`

These are expressions too, and they are covered in §7, §8 and §10.

### 6.9 Iteration: there is no loop

Ply has no `for`, no `while` and no `break`. Iteration is one of four things:

**Recursion.** A tail call is an ordinary call and is charged against the call
budget like any other, so recursion is bounded: **10,000 nested calls**, after
which the run reports a diagnostic naming the innermost frames rather than
overflowing a stack. Write the bound into your program where a reader can see
it, as `examples/hello.ply` does with `max_chunks()`.

**Random access, when you want one element rather than all of them.**
`list_at(xs, i)` (§13.2). There is no `for i in 0..n`; a sweep by index is
`fold(range(0, len(xs)), ..)` with a `list_at` inside it, and the builtins below
are what to reach for when you are visiting every element anyway.

**The list builtins.** `map`, `filter`, `fold` and `range` visit every element
and never stop early. The loop itself does not nest — one round is popped before
the next is pushed — so a `fold` over a hundred thousand elements is not a
hundred thousand nested calls and runs fine.

**`iterate`, for a loop that ends early.**

```ply
iterate(seed, budget, step)     // (a, Int, (a) -> Iter<a, b> / e) -> b / e
```

`step` answers `Continue(next_seed)` or `Stop(result)`. `budget` is the maximum
number of rounds; spending it is a diagnostic naming the builtin (never
"recursion limit" — nothing nested). The budget is an argument rather than a
flag precisely so that it is in the source, and therefore in the definition's
hash: a program that raises its own bound invalidates its own cached results.

```ply
fn first_gap(xs: List<Int>) -> Int =
  iterate({i: 0, want: 0}, 1000, |s: {i: Int, want: Int}|
    if s.i >= len(xs) { Stop(s.want) }
    else { Continue({i: s.i + 1, want: s.want + 1}) })
```

**The bytes scanners**, when you are walking a buffer. `bytes_scan`,
`bytes_scan_until`, `bytes_index_of` and friends do in one native SIMD pass what a
`fold` over `range` does in one boxed integer per byte, and they stop early.
`examples/hello.ply` records what that was worth: 84× the header bytes cost 39×
the time as folds and 0.95× the time as scanners.

### 6.10 `?`: binding a `Result` or an `Option`

`std.db`'s expression parser, before and after:

```ply
fn parse_expr_text(s: String) -> Result<Expr, DbError> =
  match lex_all(s) {
    Err(e) -> Err(e),
    Ok(ts) -> match parse_or(ts) {
      Err(e) -> Err(e),
      Ok(c) -> if len(c.rest) == 0 { Ok(c.value) }
               else { Err(expected("the end of the expression", c.rest)) },
    },
  }
```

```ply
fn parse_expr_text(s: String) -> Result<Expr, DbError> = {
  let ts = lex_all(s)?;
  let c = parse_or(ts)?;
  if len(c.rest) == 0 { Ok(c.value) }
  else { Err(expected("the end of the expression", c.rest)) }
}
```

Those are the **same definition**. `?` is sugar: the parser rewrites `e?` into

```ply
match e { Err(er) -> Err(er), Ok(x) -> rest }
```

failure arm first, before anything else in the compiler sees the program — so
the two spellings have one hash, one cache entry and one set of test results.
That is the same bargain `{..b, f: e}` takes in §6.6. Converting `std.db`,
`std.json`, `std.http`, `std.config`, `std.router` and `examples/desk.ply` to
`?` — 139 sites — moved **no definition hash at all**.

**Which constructors it names is read off the enclosing function's written
return type.** `-> Result<..>` gives `Ok`/`Err`; `-> Option<..>` gives
`Some`/`None`. Expansion follows this file's own `type` aliases to get there and
goes no further, for §6.6's reason: a meaning read across a module boundary
could go stale in a file that never changed.

```ply
fn int_value(text: String) -> Option<Int> = {
  let d = decimal_of_string(text)?;
  int_of_decimal(d, Down)
}
```

**Where you may write one**, as a rule you can apply without knowing any types:
`?` may stand wherever nothing conditional sits between it and the value the
function returns, and wherever everything evaluated before it is pure — a
literal, a variable, a field read, or an operator over those. `let x = e?;`
always qualifies, and so does a `?` in the argument of a call whose other
arguments are pure:

```ply
fn parse_or(ts: List<Tok>) -> Result<Cut<Expr>, DbError> =
  parse_or_more(parse_and(ts)?)
```

It composes with record destructuring, which is the tidiest way to return two
things from a function that can fail:

```ply
fn f(i: Int) -> Result<Int, E> = {
  let {p, node} = parse_thing(i)?;
  Ok(p + node)
}
```

#### What `?` does not do

**It converts no errors.** There is no `From` in Ply and `?` does not invent
one, so a `Result<_, E1>` bound inside a function returning `Result<_, E2>` is
an ordinary `E0201` — the same one, in the same place, that the `match` it
stands for would have got. A site that maps its error keeps its `match`:

```ply
Err(e) -> Err(in_index(index, e))     // `?` cannot express this; leave it
```

**It is not a `return`.** §19.2 still holds: there is no `return` statement and
no `break`. `?` exits the expression it is written in and nothing more. It
cannot leave a `handle` clause or body, a `with_cell`, a `with_region` or a
`simulate` — every one of those is `E0118`, because none of them has a written
return type to read the constructors off — and it cannot leave a lambda unless
the lambda writes one (§6.4), in which case `?` reads the lambda's type and
exits the lambda:

```ply
fn decode_all(js: List<Json>, c: JsonCodec<a>) -> Result<List<a>, DecodeError> =
  map(js, |j: Json| (c.decode)(j)?)   // E0118: this lambda has no written return
                                      // type; `?` needs one

fn decode_each(js: List<Json>, c: JsonCodec<a>) -> List<Result<a, DecodeError>> =
  map(js, |j: Json| -> Result<a, DecodeError> { Ok(inspect((c.decode)(j)?)) })
```

**It will not move work across a branch.** `?` lifts what it unwraps to the head
of the statement it is written in, so a `?` inside an `if` branch, a `match` arm
or the right operand of `&&` is `E0119` unless the branch is itself in return
position. Bind it first:

```ply
fn f(n: Int, c: Bool) -> Result<Int, E> = {
  let y = if c { g(n)? } else { 0 };   // E0119
  Ok(y)
}

fn f(n: Int, c: Bool) -> Result<Int, E> =
  if c { let y = g(n)?; Ok(y) } else { Ok(0) }   // fine: the branch returns
```

**It will not run an impure expression out of order.** `g(h(x), k(x)?)` is
`E0119` — `h(x)` is written before the `?` and the expansion would evaluate it
after. The lift is one line: `{ let a = h(x); g(a, k(x)?) }`.

**It will not leave a nested block.** `let y = { let z = f(n); g(z)? };` is
`E0119`, because lifting `g(z)` to the head of the statement would take it out
of `z`'s scope. A block that is itself in return position — the body, or the
tail of the body — is not nested in this sense, and a `?` in one is fine.

**It will not name a constructor you rebound.** A module that declares its own
`Ok`, `Err`, `Some` or `None` — or imports one of those names unqualified — is
`E0118` at every `?`, because the `match` would name that binding instead of the
prelude's (§5.7).

**It will not swallow a written type.** `let x: T = e?;` is `E0119`: the
expansion has no `let` left to carry `T` on. Write `let x = e?;`, or annotate
the value being unwrapped. A `?` *inside* an annotated `let`'s value is fine —
`let x: Int = g(n)? + 1;` keeps its annotation.

---

## 7. Effects and handlers

This is the centre of the language.

### 7.1 Declaring an effect

```ply
type Row = { id: Int, name: String }

effect db {
  read  get[r](key: Int) -> Option<Row>
  write put[r](key: Int, value: Row) -> Unit
}

nondet effect clock {
  read now() -> Int
}
```

* Each operation is `read` or `write`. That is the mode, and it decides
  conflicts.
* `[r]` marks an operation as **resource-parameterized**: call sites must supply
  a resource label, and the atom performed is keyed by it. The name inside the
  brackets in the declaration is documentation; only the fact that there is one
  matters. An operation without `[r]` performs a singleton atom named for the
  effect.
* Parameters may be written `name: Type` for readability; only the type is part
  of the signature.
* `nondet` marks an effect whose results are not a function of the program's
  state. This is what makes flakiness statically detectable (§9.3).
* Effects are the one **nominal** thing in Ply: `db` and `audit` may declare
  byte-identical operations and are still different capabilities.
* An effect may be `pub`. Declaring one whose name collides with a prelude effect
  (`task`, `clock`, `random`, `sim`) or with `cell` is `E0105`.

### 7.2 Atoms and rows

An **atom** is `(effect, resource, mode)`, written `effect.mode[resource]` or
`effect.mode` for a singleton. A **row** is a set of atoms plus an optional tail
variable:

```ply
/ {db.read[users]}
/ {db.read[users], db.write[orders], clock.read}
/ {net.write[conn] | e}
/ e
/ {}
```

Qualified atoms use `::` like any other reference: `/ {store::db.read[users]}`.

**Resource labels are not namespaced.** Two modules writing `[users]` name the
same resource, and they must, or the scheduler would run contending tests
concurrently. Labels are ground identifiers in the source — you cannot abstract
over one.

Two atoms **conflict** iff they name the same resource of the same effect and at
least one is a `write`. That single predicate decides which tests may run
concurrently (§9.4) and which task interleavings are worth exploring (§10.3).

### 7.3 Performing

```ply
db.get[users](3)
clock.now()
store::db.put[orders](id, row)
```

Syntactically this is `<effect>.<op>[<resource>](<args>)`. The parser recognizes
it by shape — a bare name, a `.`, a name, and then a `[` or `(` — which is why a
function stored in a record field needs `(r.f)(x)` (§6.5).

Performing an operation adds its atom to the enclosing definition's row.

### 7.4 Rows in signatures

If a function omits `/ {...}`, its row is inferred. If it carries one, the
annotation is the published signature and inference must produce a **subset** of
it. So an annotation is an upper bound you are promising callers, and it may be
wider than the body needs:

```ply
// `stale` never reads the clock when ttl <= 0, but the signature permits it,
// and the signature is all a caller and the checker get to look at.
fn stale(s: Session) -> Bool / {clock.read} =
  if s.ttl <= 0 { false } else { expired(s) }
```

Effect-polymorphic functions thread the tail:

```ply
fn map<a, b | e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e = ...
```

### 7.5 Effect sets

A named abbreviation for a fixed list of atoms:

```ply
effect set Persist = {store.read[db], store.write[db]}
effect set Full    = {Persist, log.write[app]}

fn record(k: Int, v: Int) -> Unit / {Full} { ... }
```

* Sets are **module-local**. They may not be `pub` and may not be named through
  `::`; both are `E0114`, and the diagnostic explains why (expansion has to be a
  function of the file, or the incremental cache could go stale).
* A set may include other sets; a cycle is `E0115`.
* A set may not carry a row variable — write the variable at the row that names
  the set.
* Sets are **erased by normalization**: a row written `{Full}` and one written
  with `Full`'s expansion are the same definition with the same hash. The set
  survives only as provenance for `--explain`.

### 7.6 Handlers

`handle <body> with { <clauses> }` discharges atoms. In the fragment below,
`cell` comes from an enclosing `with_cell[users]` (§8):

```ply
handle {
  assert_eq(len(active_users()), 2)
} with {
  db.get[users](k)    -> map_get(cell_get(cell), k),
  db.put[users](k, v) -> cell_set(cell, map_insert(cell_get(cell), k, v)),
  return x            -> x,
}
```

* A clause is `effect.op[resource](params) -> body`. The parameter names are
  binders; their types come from the operation's declaration.
* An optional `return x -> body` clause transforms the body's value. Without one,
  the `handle` expression's value is the body's. At most one is allowed.
* A clause with no `resume` is **tail-resumptive**: its body's value goes
  straight back to the perform site. That covers state, readers, writers and
  every in-memory test double, and is the shape nearly every shipped handler
  uses.

**Typing rule.** The `handle` expression's row is

```
(row(body) \ handled atoms) ∪ ⋃ row(clause_i)
```

The second term is what makes this honest: a handler backed by a real socket
still reports network access, and one backed by a test-local cell reports
nothing that escapes the test — which is precisely why such a test is provably
isolated.

**A handler discharges an *atom*, not an operation.** `recv`, `send` and `close`
in `std.net` are all `net.write[conn]`, so a handler with a `recv` clause and no
`send` clause type-checks and then fails at run time when `send` is performed.
The type system's granularity is the atom; the operation-level check is a runtime
one.

### 7.7 `resume`: multi-shot continuations

A clause may bind its delimited continuation:

```ply
handle { two_paths() } with {
  amb.flip[coin]() resume k -> k(true) + k(false)
}
```

`resume k` binds the continuation as `k`; the clause body then has the whole
`handle` expression's type rather than the operation's, and may invoke `k` zero,
one or many times. `std.db.transaction`'s `db.rollback(reason) resume k -> ...`
is the zero-shot case: a rollback declines to resume its body.

`resume` is contextual — a keyword only between a clause's `)` and its `->` — so
it remains an ordinary identifier everywhere else.

### 7.8 Unhandled effects

Three different things can go wrong and they have three different codes, because
they call for different responses:

* **`E0302` effect not permitted** — the body performs an atom the declared
  signature does not allow. Widen the annotation or stop performing it.
* **`E0303` unhandled effect** — inference should have prevented this and did
  not. A compiler defect.
* **`E0424` hermetic boundary** — the operation reached the host boundary with
  nothing bound. Inference was right and the *run* was configured hermetically;
  pass `--host`, or handle the effect (§15).

---

## 8. Cells and regions

Ply is a value language, so state is not a variable — it is a **cell**, scoped to
a region, and its atoms are discharged at the region boundary so they provably
cannot escape.

### 8.1 `with_cell`

```ply
with_cell[users](initial) { cell ->
  handle { body() } with {
    db.get[users](k)    -> map_get(cell_get(cell), k),
    db.put[users](k, v) -> cell_set(cell, map_insert(cell_get(cell), k, v)),
  }
}
```

`with_cell[r](init) { c -> body }` allocates a cell holding `init`, binds it as
`c` for the duration of `body`, and closes the region when `body` ends. The
label in brackets brands the region.

`cell_get(c)` reads, `cell_set(c, v)` writes, and `cell_update(c, f)` replaces
the contents with `f` applied to them (§13.8). All three are builtins rather than
effect operations, so the atoms they perform name the region of their argument
and never appear in a row that outlives it.

The idiom for several cells is nesting, and the extra braces you see in the
examples are the block form of the body:

```ply
with_cell[inbox]([]) { inbox -> {
with_cell[outbox]([]) { outbox -> {
  ...
} }
} }
```

### 8.2 `with_region`

`with_region[r] { body }` opens a lexical allocation scope whose brand `r`
appears in the types of the values allocated inside it. A `with_cell[r]` written
under a `with_region[r]` allocates *into* that region rather than opening one of
its own, so a `with_cell` written on its own is unchanged:

```ply
fn counted(n: Int) -> Int =
  with_region[work] {
    with_cell[work](0) { c -> {
      cell_set(c, cell_get(c) + n);
      cell_get(c)
    } }
  }
```

Regions are bump arenas, and each is one of two **kinds**, inferred rather than
written: a region the compiler can prove no continuation is captured across is
`unique` and costs a bump pointer and nothing else, and every region it cannot
decide is `shared` and keeps its slots alive past its close, because a
continuation may be resumed there and read them. The imprecision runs in the
safe direction on purpose. There is no surface syntax for asking for a kind.

### 8.3 What a region refuses

* **`E0446` region escape.** A value branded with a region's name that would
  outlive it — returned from it, stored into a binding that predates it, captured
  by a closure that leaves it, or written as a field of a declared type. Reported
  at the point it would escape, naming the value's type and the region:

  ```
  [E0446] Error: the cell escapes its `with_cell[work]` region
   10 │   with_cell[work](0) { c -> c }
      │                             ╰── this has type `Cell[work]<Int>`
      │  Note: read the cell inside the region and return the value instead
  ```

* **`E0447` region already open.** Two regions in scope at once under one name.
  The brand *is* the name, so there is no reading under which the two mean
  different things.
* **`E0449` region escape at a boundary.** A handle into a region reaching a
  place where no type is left to check it: a host operation's argument, a host
  handler's answer, or an entry point's argument. This one fires at run time,
  because it is at a boundary a type does not cross.
* **`W0610` reference cycle.** A value made to reach itself. Ply reference-counts
  and does not collect cycles, so the leak is reported as a fact on the run
  rather than left to be inferred from memory growth.

---

## 9. Tests

### 9.1 Writing one

```ply
// from examples/hello.ply
test "a well-formed request line is split into its three parts" {
  assert_eq(parse(get_root()), Complete({
    method: b"GET", target: b"/", version: b"HTTP/1.1",
  }))
}
```

A `test` has a quoted label and a block body. It cannot be `pub`, cannot be
referenced, and takes no arguments. Two assertions exist:

* `assert(cond: Bool, message: Option<String> = None) -> Unit`
* `assert_eq<a>(actual: a, expected: a) -> Unit`

`assert`'s message is a defaulted parameter, so `assert(ok)` is the common
form and `assert(ok, Some("why"))` — or `assert(ok, message: Some("why"))` —
attaches a note to the failure report.

`assert_eq`'s failure report gives both values, and the **first structural
difference** inside them when they are compound:

```
   assertion failed: expected 12, found 27
     at fail/main.ply:6:3
   = expected: 12
   = actual:   27
```

Any other failure — `panic("...")`, integer overflow, division by zero, an
out-of-range index, a spent `iterate` budget, the recursion limit — is `E0502`
and fails the test the same way.

### 9.2 Selection and the cache

A test is selected exactly when its hash is absent from the result cache. There
is no file graph and no heuristic. Editing a comment, renaming a function,
reformatting, moving a definition between modules, adding or removing an import,
adding a `pub`, writing a specification, and **rewording a test's own label** all
change no hash and select nothing. Editing any expression a test can reach
selects exactly the tests that reach it.

`ply test --explain` prints why each test was selected:

```
   run  0ec363399cd6 new              credit moves one account       isolation: region

   why
     new              this hash has never gone green, so nothing is known about it
```

`--no-cache` neither reads nor writes results. `--no-incremental` does the same
for the front-end cache (parse and typecheck) and leaves results alone.
`--filter <substring>` selects on the `<module>.<label>` key.

### 9.3 Determinism

A `test` is deterministic by default. If its row retains any atom from a
`nondet` effect after handling, that is a **compile error**, `E0412`:

```
[E0412] Error: nondeterministic effect in a deterministic test
 19 │   assert(!stale({user: "ada", started: 1000, ttl: 0}))
    │           ╰── reaches `clock.read`, and `clock` is declared `nondet`
    │
    │  Note 1: `clock.read` is performed inside something this expression calls
    │  Note 2: handle it here, e.g. `handle <body> with { clock.now() -> <value> }`
    │  Note 3: or declare this `test/nondet`, which opts out of the cache and re-runs every time
```

The two remedies are the ones the diagnostic names. `test/nondet "label" { ... }`
opts out of the check, is never cached, and runs every time.

Note that the atom is printed **module-qualified**, because effects are nominal
in their module.

### 9.4 Scheduling and isolation

The runner builds a conflict graph over the selected tests' footprints and
colours it. Tests whose footprints are disjoint, or that only *read* a shared
resource, run concurrently by construction rather than by convention. A test
whose effects are all discharged inside a region is **region-isolated** and
conflicts with nothing:

```
   selected 4 of 4 (0 cached)
   1 group · 10 workers
   isolated 4 of 4
```

`--jobs N` sets the worker count (default: one per core). `--explain` prints the
groups and the footprint each was formed on.

### 9.5 When a test fails

Ply attributes the failure. Because it has both the definition graph and the
cache, it knows which definitions changed since the last pass and which of them
lie in the failing test's closure — the **suspect set** — and it will bisect over
hybrid programs to name a **culprit**:

```
   main.quadruple multiplies by four
     culprit: main.double   fail/main.ply:1:1
       only one change could be flipped: main.double
     assertion failed: expected 12, found 27
       at fail/main.ply:6:3
     = expected: 12
     = actual:   27
     suspects: main.quadruple (derived)
```

`--bisect auto|always|never` controls that search (`auto` bisects a failing
deterministic test that has passed before) and `--bisect-budget N` caps it in
*evaluations* rather than seconds, so two runs over one failure agree.
`--trace auto|always|never` records which definitions a failing test actually
entered.

### 9.6 Machine-readable output

Every command takes `--json` and then emits exactly one JSON object on stdout and
nothing else. For `ply test` that object carries, per failure: the structured
diagnostic with spans and snippets, the expected/actual pair, the footprint at
the point of failure, the suspect set, the culprit with its search statistics,
and the replay command.

### 9.7 Auditing a compiled backend

`--backend` attaches a compiled backend (§17). `--audit-backend` runs each test
twice — once with it and once without — and fails the run on any disagreement
(`E0503`). It is off by default because it doubles what a run costs, and a run
with a backend attached neither reads nor writes the result cache either way.
Every function the fragment compiles is entered when a call's arguments and
answer are carried, and a test whose body the fragment compiles is entered
whole, the backend's answer being the pass. A test the fragment refused is
left to the machine, which raises the diagnostic as it always did; one the
backend ran and raised in — an assertion included — is run by the machine
too, and a pass there fails the test with `E0503`, since a backend that fails
a test the machine passes is the disagreement `--backend` exists to surface.
A `wrong:` corruption (§17) leaves every test to the machine, where each call
crosses the seam it corrupts. `PLY_CODEGEN_REGISTER=narrow` limits entry to
scalar signatures, the measurement arm ADR 0030 shipped.

---

## 10. Simulation and concurrency

### 10.1 `simulate`

Concurrency in Ply is an effect, so the scheduler is a test double like any
other. `simulate { ... }` installs a seeded scheduler over the three effects the
language can model — `task`, `clock` and `random`:

```ply
// abridged from examples/bank.ply — the handler bodies are elided
test "the guarded transfer never overdraws, under every interleaving" {
  with_cell[accounts](opening()) { ledger -> {
    handle {
      simulate {
        let a = task.spawn(|| settle("alice", "bob", 60));
        let b = task.spawn(|| settle("alice", "carol", 60));
        task.join(a);
        task.join(b);
        assert_eq(overdrawn(cell_get(ledger)), 0)
      }
    } with {
      bank.take[accounts](who, amount) -> ...,
      bank.credit[accounts](who, amount) -> ...,
    }
  } }
}
```

`simulate` is `handle` with a fixed clause set and no new typing rule. There is
no seed in the syntax — a seed written in the source would be part of the
definition's hash, making every seed a different definition.

### 10.2 The three simulated effects

Declared by the language rather than by a module, and in scope everywhere. This
is how the compiler declares them, not a declaration you could write — an
operation cannot carry its own type parameters in surface syntax:

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

A `simulate` region's own row gains `sim.read` — the seed dependency, in the
type. That is the one nondeterministic-looking atom a `det` test may carry,
because a seed is an *input* rather than a nondeterminism.

**Virtual time advances only when no task is enabled.** A `clock.sleep` is a
jump, not a wait, so a test can assert a second and a half of retry backoff and
cost no measurable wall clock, and a simulated timeout can never fire early —
the exact opposite of a wall-clock timeout, whose entire failure mode is firing
because the machine was busy. `examples/timeout.ply` is built on this.

`Task<a>` may not escape its region (`E0413`), and a region that makes no
progress — nothing enabled and no timer that can fire, or a spent step budget —
is `E0414 deadlock`. A `simulate` inside a `simulate`, lexically or through a
call, is `E0416`.

### 10.3 The search, and why it is a proof

Two tasks whose footprints do not conflict **commute**, so exploring both orders
is provably redundant. Partial-order reduction algorithms usually spend their
complexity approximating that relation; Ply computes it exactly, at resource
granularity, with the same conflict predicate that decides which tests may run
concurrently. When the search exhausts its frontier, the result is not a sample
but a statement about **every** interleaving:

```
   ok    two unguarded transfers conserve the money whichever order they run in   0.9ms
       9 interleavings · exhaustive
   ok    the guarded transfer never overdraws, under every interleaving           0.6ms
       6 interleavings · exhaustive
```

Two extra dependences widen the search beyond what the atoms alone predict, and
both *add* order rather than removing it. Two cell accesses contend when they
touch the same location and one writes. **Two allocations always contend**,
because they draw from one bump pointer — so two tasks that each open a
`with_cell` are ordered even when nothing in their rows conflicts.

Tasks interleave at the operations the scheduler answers — `task`, `clock`,
`random`. A task that reads shared state and writes it back with none of those in
between runs the two as one step, and **no schedule separates them**. That is a
real limit: put a `task.yield()` in the window, or a `clock.now()` stamp the code
was going to write anyway, or push the check into the resource so there is
nothing to separate. `examples/bank.ply` is that last fix, written out.

### 10.4 Controlling the search

| flag | meaning |
| --- | --- |
| `--sim dpor` | footprint-guided partial-order reduction (default) |
| `--sim random` | one interleaving per seed, seeds independent |
| `--sim once` | exactly one interleaving |
| `--seeds N` | seeds per simulated test (default 1 under `dpor`, 64 under `random`) |
| `--sim-budget N` | interleavings per seed; only `dpor` searches more than one |
| `--sim-steps N` | scheduling steps one interleaving may take before `E0414` |
| `--seed 7`, `--seed 7:3.0.2` | replay exactly one interleaving; implies `--sim once` |
| `--measure-reduction` | also run with the dependence relation forced to `true`, and report what an unpruned search would have cost |

A simulated test is keyed on `(test hash, plan)` and never on the hash alone: a
green run under one plan is not a green run under another. **A search that spends
its budget is reported green and is not cached** — it proved nothing about the
interleavings it did not reach. That is the only green deterministic test in the
language that re-runs, and it is correct that it does.

### 10.5 A failure report from a race

```
   bank_race.no account is ever overdrawn
     assertion failed: expected 0, found 1
       at tests/fixtures/bank_race.ply:68:9
     = failed in task @0 of a simulated region, with 0 other task(s) unfinished; replay with seed 0:0.1.0.2
     seed: 0:0.1.0.2
     race: @1  bank_race.transfer   bank_race.bank.write[accounts]   .../bank_race.ply:52:5
           @2  bank_race.transfer   bank_race.bank.read[accounts]    .../bank_race.ply:48:6
     replay: ply test --seed 0:0.1.0.2 --filter "no account is ever overdrawn"
```

The two contending steps, the tasks they ran in, the source positions, and the
exact command to replay it.

### 10.6 What this costs, plainly

A test that depends on time, order or randomness no longer fails to compile — it
becomes a test over a seed set, and a green run is a claim about the seeds that
were run. The risk that a seed you did not run would have failed is real. It is
also visible on every run, often zero when the search is exhaustive, and widened
with one flag — where wall-clock flakiness was none of those.

---

## 11. Specifications, laws and proof

§1–§10 make the verification loop cheap. This section is the other half: making
the thing a human reads a *specification* rather than an implementation.

### 11.1 `requires` and `ensures`

```ply
fn adjusted(account: Account, amount: Int) -> Account
  requires amount > -1000000000 && amount < 1000000000
  requires account.balance > -1000000000 && account.balance < 1000000000
  ensures result.name == account.name
  ensures result.balance == account.balance + amount
= {name: account.name, balance: account.balance + amount}
```

Clauses go between the signature and the body, in any order and any number.
`result` is bound only in an `ensures`.

**A spec expression must be pure** — an empty row (`E0417`). A spec that can
perform effects can change what it observes.

**A spec is a claim *about* a definition, not part of it.** Specs are erased by
normalization, so writing one changes no definition hash and re-runs no test.
The *claim* gets its own hash, which covers the definition's — so an obligation
invalidates when the implementation moves, while the implementation does not
invalidate when the claim moves. That asymmetry is exactly the asymmetry review
has.

`requires` is a **filter on the domain** of the `ensures` clauses beside it. It
is not a contract checked at every call site, and a law does not inherit the
`requires` of the definitions it names.

### 11.2 Laws

```ply
law "a credit and a matching debit leave an account exactly as it was"
  forall (account: Account, amount: Int)
  where amount > -1000000000 && amount < 1000000000 {
    adjusted(adjusted(account, amount), -amount) == account
  }
```

A `law` has a quoted label, optional `forall` binders (whose types are
**mandatory**), an optional `where` guard, and a block body. It has no name
anything can reference and cannot be `pub`.

The guard's row must be empty. The body's row must be empty too, with one
exception: a body that is a `simulate` region has row `{sim.read}` and is a claim
about every interleaving, discharged by §10's search.

`law/host "label" { ... }` relaxes the *body* to any row, making the law a claim
about the world rather than about the program alone. Three things follow, each
enforced elsewhere: it can never be `proved`, it is never cached in either
direction, and under a hermetic run it is reported `W0604 unattempted` rather
than green. The guard stays pure.

### 11.3 Tiers

Each obligation is discharged at the strongest tier the system can
**demonstrate**:

| tier | claim |
| --- | --- |
| `proved` | an argument covering **every** input satisfying the guard |
| `property` | randomized cases, the count reported, shrinking on failure |
| `example` | concrete cases, and no coverage claim |
| `unattempted` (`W0604`) | a **gap**, not a weak tier: never green, never cached, counted on its own |

`proved` is a small, exactly stated fragment. The rules a certificate may name
are, in full: ground evaluation of a closed Boolean term; exhaustive enumeration
of a finite domain up to 4096 points; linear arithmetic over `Int` (`+`, `-`,
unary `-`, multiplication by a literal, and the six comparisons — `x * y` at two
symbolics, `/` and `%` excluded); propositional reasoning by case split; a case
split on a scrutinee's outermost constructor; congruence closure; constructor
injectivity; unfolding of **non-recursive** definitions only; and
`ExhaustiveInterleaving`, the one rule that comes from execution rather than
from a static argument.

Recursion over unbounded data needs induction, which is not here, so
`reverse(reverse(xs)) == xs` is `property` and should be.

**A tier label is a truth claim**, and it is computed from the evidence a
discharge carries rather than stored. When in doubt the system reports the weaker
tier.

### 11.4 Bounding the domain

A spec that looks obviously true is not the same as one the prover can close.
`Int` is `i64` and `+` is checked, so at the bottom of the range an expression
*raises* and there is no result for a postcondition to be true of. A proof
covering "every input satisfying the guard" has to cover those too — which is
why `examples/bank.ply` carries explicit `> -1000000000 && < 1000000000` guards.
Without them the honest report is `unattempted`, and the prover says so rather
than certifying an identity over ℤ.

### 11.5 Running the prover

```
$ ply prove examples/bank.ply
   8 definitions · 7 carry an obligation · 1 do not
   1 not covered by a claim that holds: bank.transfer
   6 obligations · 4 proved · 1 property · 1 example   (0.00s)

   ✓ proved      bank.adjusted ensures #0    congruence · propositional · linear arithmetic · 203 steps
   ✓ proved      law "a credit and a matching debit leave an account exactly as it was"
                                              propositional · congruence · linear arithmetic · 1 unfolding · 511 steps
   ✓ property    law "crediting a name the bank does not hold moves nothing"   190 cases · 10 rejected
   ✓ example     law "a movement between two accounts leaves the bank's total alone"
                                              6 of 200 cases kept · guard rejected 194
   ✓ proved      law "no interleaving of two guarded settlements can overdraw the account"
                                              exhaustive over 6 interleavings · 6 steps

   6 held (0.00s)
```

**The first line is the honest number.** The count of definitions carrying no
obligation is the surface where review still costs what it costs today, so it is
in the default output ahead of the results and never behind a flag.

Knobs: `--prove-cases N` (candidates per root; fewer than 25 kept can only report
`example`), `--prove-roots N` (generator roots, each drawing its own case set),
`--prove-budget N` (static inference steps; a spent budget reports `property`,
never `proved` and never `refuted`), `--shrink-budget N` (counterexample
shrinking, in evaluations).

Two failure codes: `E0419` an obligation refuted by a counterexample (the
program's fault, attributed like any other failure), and `E0420` a guard that
admits no values, which is always a defect in the spec — reporting it `proved`
would turn a typo into a proof of everything.

### 11.6 `ply review`

```
$ ply review
   3 definitions · 0 carry an obligation · 3 do not
   3 of 3 definitions changed since the last accepted review · 0 of them have a baseline

   main.credit_account · never reviewed
     → read the implementation, line by line, exactly as today
```

`ply review` reports, per changed definition, whether the implementation changed,
whether the spec changed, and whether the obligations still hold. The row that
matters is *implementation changed, spec unchanged*, where the review is reading
the obligations rather than the diff. `ply review --accept` records the current
state as the baseline; the baseline is keyed by name, so renaming a definition
loses its baseline and reports it as unreviewed rather than as unchanged.

### 11.7 Frame conditions, and no `old()`

The classic tarpit of program verification is the frame problem: an `ensures`
says what changed, and a caller needs to know what did not. Ply has computed that
set for every definition since §7 — it is the footprint, at resource granularity
— and it is checked as an upper bound by inference rather than asserted by a
user. So an `ensures` means *this holds of the result, and every resource outside
the footprint's writes is unchanged*, and the second half is not an obligation at
all.

Ply also needs no `old()`: it is a value language, so the pre-state of
`withdraw(acct, amount)` is `acct`, still in scope and still exactly what it was.

**What this is not:** a general-purpose theorem prover, an SMT integration, or a
termination checker.

---

## 12. Derivation

`derive` generates ordinary definitions from a type's structure. There are
exactly three derivers and no user-defined ones.

```ply
import std.json

pub type Line = { sku: String, qty: Int, unit_price: Decimal }
pub type Order = { customer: String, lines: List<Line> }

derive json for Line
derive json for Order
```

### 12.1 What each deriver generates

| deriver | generated name | type |
| --- | --- | --- |
| `json` | `<snake_case(T)>_json` | `std.json.JsonCodec<T>` |
| `eq` | `<snake_case(T)>_eq` | `{eq: (T, T) -> Bool}` |
| `ord` | `<snake_case(T)>_ord` | `{compare: (T, T) -> Ordering}` |

`snake_case` inserts `_` before an uppercase letter that follows a lowercase
letter or a digit, and before the last uppercase of a run followed by a
lowercase. It is total and can therefore collide — `HTTPRequest` and
`HttpRequest` both yield `http_request` — which is `E0105` naming both.

For a parameterized type the generated function takes one dictionary per
parameter and carries `where derivable(<deriver>, <param>)`:

```ply
pub type Box<a> = { label: String, inner: a }
derive json for Box
// box_json : <a>(JsonCodec<a>) -> JsonCodec<Box<a>>
// (`ply check --types` prints that structurally, since records and `Box` are aliases)
```

`eq` and `ord` delegate to `==` and to `compare_values`, the canonical total
order — deliberately, so that a derived ordering and the order a `Map` iterates
in are one order rather than two that can drift. `json` walks the structure and
composes through other types **by name**, so an edit to `Line` moves
`order_json`'s hash through the reference and re-selects exactly the tests that
reach it.

### 12.2 The orphan rule

A `derive` may only name a type **its own module declares** (`E0208`). That is
what makes "one type, one canonical encoding" checkable from what a module can
see. It is a local property, not a global guarantee — see §19.

### 12.3 Using a derived codec

There is no dispatch. A codec is a plain value and you pass it:

```ply
pub fn reply_for(body: Bytes) -> Reply =
  match json::decode_bytes(body, order_json()) {
    Ok(o) -> accept(o),
    Err(e) -> Rejected(json::error_to_string(e)),
  }
```

### 12.4 What has no derivation

`E0206` names the field that blocks a derivation rather than the type as a whole.

| refused | why |
| --- | --- |
| a function type | there is nothing to encode |
| `Cell`, `Task` | a name for a location, not a value |
| `Float` under `ord` | `NaN != NaN`, so there is no total order |
| `Secret` under `json` and `ord` | an encoding writes the value out; an ordering recovers it in calls proportional to its length. `eq` is allowed. |
| `Option<Unit>`, `Option<Option<a>>` under `json` | the inner value encodes as `null`, which is how `Option` writes `None`, so `Some` and `None` would be the same document. Wrap the inner value in a record or a one-field variant. |

The same predicate `derivable(D, t)` is what a `Map`'s key rule and a `where`
clause at a call site check, so there is one answer rather than three.

### 12.5 Constraints on your own signatures

```ply
fn encode<a>(b: Box<a>, c: json::JsonCodec<a>) -> String
  where derivable(json, a) =
  json::encode_string(b, box_json(c))
```

`where derivable(D, p)` goes after the effect row and before any `requires`. It
is checked at the **signature** rather than at instantiation, so a mistake is
reported where you can read it rather than deep inside an expansion. A constraint
is part of the published signature and is **kept** by normalization: adding one
narrows the call sites the signature admits, so callers must be rechecked.

---

## 13. The builtin library

These names are in scope in every module with no import. A module may declare a
name that shadows one, and a local binding always wins — with a single
exception: `compare_values` is reserved (`E0105`), because `derive ord` builds a
dictionary out of it and a module that could redefine it would give one type two
orders. `compare` is the same operation under a name you may shadow.

### 13.1 Assertions and failure

| signature | notes |
| --- | --- |
| `assert(cond: Bool, message: Option<String> = None) -> Unit` | the message becomes a note on the failure |
| `assert_eq<a>(actual: a, expected: a) -> Unit` | reports both values and the first structural difference |
| `panic<a>(message: String) -> a` | raises `E0502` |

### 13.2 Lists

| signature | notes |
| --- | --- |
| `len<a>(xs: List<a>) -> Int` | |
| `push<a>(xs: List<a>, x: a) -> List<a>` | appends; in place when the caller is the last owner |
| `list_at<a>(xs: List<a>, i: Int) -> Option<a>` | `None` for a negative index or one at or past the end; `list_at(xs, len(xs) - 1)` is the last element |
| `map<a, b \| e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e` | |
| `filter<a \| e>(xs: List<a>, f: (a) -> Bool / e) -> List<a> / e` | |
| `fold<a, b \| e>(xs: List<a>, init: b, f: (b, a) -> b / e) -> b / e` | visits every element |
| `range(lo: Int, hi: Int) -> List<Int>` | `[lo, hi)`; empty when `hi <= lo` |
| `iterate<a, b \| e>(seed: a, budget: Int, step: (a) -> Iter<a, b> / e) -> b / e` | the early-exit loop (§6.9) |

There is one index and no defaulting variant. A `list_at_or(xs, i, default)`
was designed alongside `list_at` and refused: it has to be spelled with a
`match` instead,

```ply
type Ctx = { toks: List<Int>, eof: Int }

fn kind_at(c: Ctx, pos: Int, n: Int) -> Int =
  match list_at(c.toks, pos + n) { Some(t) -> t, None -> c.eof }
```

and the whole case for a second builtin was that this `match` costs something on
a hot path. It costs **0.34 µs per peek out of 1.66**, which is a 1.26× saving
against a bar of 1.5× fixed before the number existed, so the second name was
not worth it. ADR 0027 has the measurement.

`list_at` does not raise, and where that shows up is the prover. A `law` over a
function that peeks with `list_at` runs its randomized cases and reaches
`property`; the same law over a `bytes_at` peek hits an out-of-range case, the
peek *raises*, and the obligation comes back `unattempted` — a gap rather than a
weak tier (§11.3), with the definition reported as covered by no claim that
holds. Guarding the `bytes_at` gets the `property` back, and that guard is the
wrapper `std.json` and `std.db` each write by hand. That is the whole difference
§19.4 warns about, and `docs/adr/0027-a-list-index.md` §2 has the two laws
side by side.

### 13.3 Maps

```ply
map_new<k, v>() -> Map<k, v>
map_insert<k, v>(m: Map<k, v>, key: k, value: v) -> Map<k, v>
map_get<k, v>(m: Map<k, v>, key: k) -> Option<v>
map_contains<k, v>(m: Map<k, v>, key: k) -> Bool
map_remove<k, v>(m: Map<k, v>, key: k) -> Map<k, v>
map_len<k, v>(m: Map<k, v>) -> Int
map_keys<k, v>(m: Map<k, v>) -> List<k>
map_values<k, v>(m: Map<k, v>) -> List<v>
map_entries<k, v>(m: Map<k, v>) -> List<{key: k, value: v}>
map_of_entries<k, v>(es: List<{key: k, value: v}>) -> Map<k, v>
map_merge<k, v>(a: Map<k, v>, b: Map<k, v>) -> Map<k, v>   // b wins on a shared key
map_fold<k, v, c | e>(m: Map<k, v>, init: c, f: (c, k, v) -> c / e) -> c / e
map_update<k, v | e>(m: Map<k, v>, key: k, f: (v) -> v / e) -> Map<k, v> / e
```

`map_fold` visits entries in ascending key order, so a fold over a map is a
function of its contents rather than of how it was built.

`map_update(m, k, f)` replaces the entry under `k` with `f` applied to it, and
leaves a map with no such key as it was. The entry leaves the map before `f`
sees it, so a `push` inside `f` finds its list at one owner when nothing else
holds the map — which `push(map_get(m, k), x)` never does, because `map_get`
answers a clone the map still holds.

### 13.4 Ordering

| signature | notes |
| --- | --- |
| `compare<a>(x: a, y: a) -> Ordering` | the canonical total order over values; requires `derivable(ord, a)` |
| `compare_values<a>(x: a, y: a) -> Ordering` | the same order under a name a module may not declare; what `derive ord` emits |
| `min(a: Int, b: Int) -> Int`, `max(a: Int, b: Int) -> Int` | the smaller and the larger of two integers; a module may declare its own `min` and it wins inside that module |

### 13.5 Strings

`String` is indexed by **character**, not by byte. Ranges are never clamped — an
out-of-range index or slice raises `E0502`.

```ply
string_len(s) -> Int                          // characters
string_slice(s, start, end) -> String         // 0 <= start <= end <= len
string_split(s, sep) -> List<String>
string_trim(s) -> String
string_lower(s) -> String
string_upper(s) -> String
string_starts_with(s, prefix) -> Bool
string_ends_with(s, suffix) -> Bool
string_contains(s, needle) -> Bool
string_find(s, needle) -> Int                 // raises if absent; guard with string_contains
string_concat(a, b) -> String                 // same as `a ++ b`
int_to_string(n) -> String
```

### 13.6 Bytes

```ply
bytes_len(b) -> Int
bytes_at(b, i) -> Int                         // 0..=255; raises out of range
bytes_slice(b, start, end) -> Bytes           // never clamped
bytes_concat(a, b) -> Bytes
bytes_concat_all(bs: List<Bytes>) -> Bytes    // one allocation over the whole list
byte_of_int(n) -> Bytes                       // one byte; raises outside 0..=255
bytes_of_string(s) -> Bytes
string_of_bytes(b) -> String                  // raises on invalid UTF-8
string_of_bytes_lossy(b) -> String            // substitutes U+FFFD
bytes_is_utf8(b) -> Bool
bytes_index_of(hay, needle) -> Option<Int>
bytes_index_of_from(hay, needle, from) -> Option<Int>
bytes_index_of_byte(hay, byte) -> Option<Int>
bytes_starts_with(b, prefix) -> Bool
bytes_ends_with(b, suffix) -> Bool
bytes_split(b, sep) -> List<Bytes>
bytes_scan(hay, from, class, budget) -> Int
bytes_scan_until(hay, from, class, budget) -> Int
bytes_position<| e>(b, from, f: (Int) -> Bool / e) -> Option<Int> / e
```

`bytes_scan` stops at the first byte **not** in `class`; `bytes_scan_until` stops
at the first byte **in** it. Both take a byte class as a `Bytes` of its members
(so there is no closed set to extend) and a `budget` bounding the window. Both
answer the index they stopped at, or the end of the window when they did not —
so a caller distinguishes "the class ended" from "the budget ran out" by
comparing against `from + budget`. Neither allocates per byte, and both stop
early; `bytes_concat_all` exists because folding `bytes_concat` over a read loop
is quadratic in a message length a peer chooses.

### 13.7 Decimal

```ply
decimal_div(a, b, scale: Int, mode: Rounding) -> Decimal
decimal_round(d, scale: Int, mode: Rounding) -> Decimal
decimal_of_int(n) -> Decimal
int_of_decimal(d, mode: Rounding) -> Option<Int>
float_of_decimal(d) -> Float
decimal_of_float(f) -> Option<Decimal>
decimal_of_string(s) -> Option<Decimal>
decimal_to_string(d) -> String
bits_of_float(f) -> Int
float_of_bits(n) -> Float
```

The scale and the rounding mode are arguments because `/` on `Decimal` is
`E0209`: naming the rounding is the whole point. `bits_of_float` is the IEEE 754
pattern as the signed 64-bit `Int` it fits in, and `float_of_bits` is its
inverse; both are total, so a NaN round-trips bit for bit.

### 13.8 Cells

```ply
cell_get<a>(c: Cell<a>) -> a
cell_set<a>(c: Cell<a>, v: a) -> Unit
cell_update<a | e>(c: Cell<a>, f: (a) -> a / e) -> Unit / e
```

Builtins rather than effect operations, so their atoms are discharged at the
region boundary (§8). `cell_update` performs both the read and the write atom,
plus whatever `f` performs.

`cell_update(c, f)` replaces the contents with `f` applied to them, and it is
the only way an append onto a cell's contents grows in place: the contents
leave the region for the length of the call, so `push` inside `f` finds one
owner, where `cell_set(c, push(cell_get(c), x))` copies every time because the
cell still holds it. While the update runs the cell is
unreadable — a `cell_get` reached through an effect `f` performs, or a nested
update of the same cell, is refused rather than answered with a placeholder.

### 13.9 Secrets

```ply
secret_of_string(s: String) -> Secret<String>
secret_verify(stored: Secret<String>, supplied: String) -> Bool   // constant-time
secret_is_empty<a>(s: Secret<a>) -> Bool
```

`secret_verify` is constant-time over the compared bytes and is **not**
rate-limited, so a program that loops it over candidates recovers the value.
Presence is deliberately observable: an operator must be able to tell a missing
credential from a wrong one.

### 13.10 Wrapping arithmetic

```
wrap_add(a: Int, b: Int) -> Int
wrap_sub(a: Int, b: Int) -> Int
wrap_mul(a: Int, b: Int) -> Int
```

Two's complement, modulo 2^64, and the only arithmetic in the language that
cannot raise. `+`, `-` and `*` stay checked, which is the point: the easy
spelling is the safe one, and a step that is *defined* to wrap — a 64-bit mixing
function, a linear congruential generator — says so in the name it calls rather
than in a comment beside an operator that means something else.

You will reach for these less often than you expect. Arithmetic on values masked
to 32 bits cannot overflow an `Int` at all, so `std.hash`'s BLAKE3 — the one
thing in this tree written to need them — uses none: it masks with `& 0xFFFF_FFFF`
and adds with `+`.

---

## 14. The standard library

Ten modules ship compiled into the `ply` binary. They are not part of your
project: loading is demand-driven, `ply test` does not select their tests unless
you pass `--std`, and `ply prove` does not count their definitions in your
coverage line unless you pass `--std` there too. That is deliberate — a project's
test and obligation counts must not change with a compiler upgrade.

```
$ ply std
   10 modules · 825 definitions · shipped with this compiler

   MODULE      DEFINITIONS  TESTS  BYTES
   std.config  15           5      4810
   std.db      292          34     110073
   std.fs      25           9      12652
   std.hash    32           5      8970
   std.http    166          53     102957
   std.json    137          38     55912
   std.net     7            3      3720
   std.router  105          31     44919
   std.signal  7            2      1416
   std.trace   39           10     12218

   `import std.<name>` to use one; `ply std --show <name>` prints its source

   digest: b3:2b15c1c16074
```

`ply std --show std.json` prints a module's source — the full name, `std`
included; `ply std --digest` prints the one line a CI check pins.

A stdlib definition is content-addressed like any other, so a compiler upgrade
re-runs exactly the tests that reach a definition that changed — and warns
(`W0605`) with both digests, so an upgrade that re-runs work is a fact rather
than a mystery.

### 14.1 `std.net` — sockets

```ply
pub nondet effect net {
  write listen[s](port: Int) -> Int
  write listen_tls[s](port: Int, credential: String) -> Int
  write accept[s](listener: Int) -> Int
  write recv[s](conn: Int, max: Int, timeout_ms: Int) -> Option<Bytes>
  write send[s](conn: Int, payload: Bytes, timeout_ms: Int) -> Option<Int>
  write close[s](socket: Int) -> Unit
}

pub fn drain(c: Int, so_far: Bytes, timeout_ms: Int) -> Bytes / {net.write[conn]}
pub fn send_all(c: Int, payload: Bytes, timeout_ms: Int) -> Bool / {net.write[conn]}
```

Every operation is a `write`, `recv` included — a read consumes bytes from the
kernel's receive buffer, so two tasks reading one socket race exactly as two
writers do. Every operation is resource-parameterized, which is what lets the
scheduler run two connections at once.

**A deadline is an argument, not a cancellation.** One rule, stated once:
`None` is a deadline expiring, and an empty `Some` is an ending (EOF).
`timeout_ms <= 0` is a runtime error, so a caller who wants no deadline writes a
large number where a reader can see it.

`send_all` loops over a partial write; one `send` is free to take fewer bytes
than it was given, and a short write nobody looked at is a truncated response the
client reads as a complete one.

### 14.2 `std.http` — HTTP/1.1 framing

Framing is a pure function from bytes to a request, so `parse_head`, `body_step`
and `encode` have empty rows and a smuggling defect is a failing test rather than
a line in the trusted computing base. Only the serve loop performs `net`.

Key types: `Method`, `Version`, `Headers` (`Map<String, List<String>>`),
`Request`, `Response`, `Limits`, `Refusal`, `Framing`, `Head`, `HeadResult`,
`BodyState`, `BodyStep`.

Key functions: `default_limits`, `parse_head`, `body_start`, `body_step`,
`header`, `header_lines`, `has_header`, `set_header`, `add_header`, `response`,
`text_response`, `refusal_response`, `method_not_allowed`, `reason_phrase`,
`encode`, `encode_chunked_head`, `encode_chunk`, `last_chunk`,
`continue_response`, `read_head`, `read_body`, `serve_connection`, `serve`,
`listen_and_serve`.

Three rules decide the shape of the module: **every ambiguity is a refusal**
(request smuggling is a parser disagreeing with itself about where a message
ends, so where RFC 9112 permits a choice this takes the branch that refuses, and
a refusal closes the connection); **a bound costs the bound, not the buffer** (a
20 MB header line costs `max_header_bytes`, not 20 MB); and **recursion is
bounded by a limit, never by the interpreter** — so the header loop, the read
loop and the chunk decoder each carry a stated, tested bound.

Not present: routing (that is `std.router`), TLS above the socket, compression,
`Upgrade`, and any decoding of `Content-Encoding`.

### 14.3 `std.router` — routes as data

A route table is a `List<Route<a>>` and `route` is a **pure function** over it, so
a table can be inspected (`listing`, `surface`), asserted about (`conflicts`,
`well_formed`), quantified over in a `law`, and printed. A macro, a decorator or a
global registry can do none of those.

**The endpoint is a tag, not a closure.** `List` is homogeneous and a function
type carries its row, so a table of handlers would force every handler to declare
the union of the whole service's row. With a tag, the program writes its own
`match`, and that `match` is exhaustiveness-checked — so a route with no handler
is a compile error rather than a 500 at 3am.

Parameters are typed in the pattern: `Typed({name: "id", kind: IntParam})`
matches a segment only when it is an `Int`, and the handler reads it back with
`int_param` / `int_param_or`.

Two rules stop a path having two meanings: splitting happens before
percent-decoding, per segment, so `%2F` can never introduce a boundary; and
nothing is normalized silently — `/orders` and `/orders/` are different paths and
both route, and `normalize_path` is a call the program makes.

### 14.4 `std.json` — the JSON value and codecs

`Json` is `Null | Bool | Number(Decimal) | Str | Array | Object(Map<String,
Json>)`.

**A number is a `Decimal` and never an `f64`.** A parser that routed numbers
through binary64 would decode `0.1` to the nearest double, and nothing downstream
recovers the hundredth of a cent. A number outside `Decimal`'s range rejects the
document, naming the byte offset.

An object is a `Map<String, Json>`, so key order is ascending and canonical
rather than a record of the order it arrived in — which is what makes `to_bytes`
a function of the value, and therefore what makes a derived encoding
byte-identical across runs and engines.

`JsonCodec<a>` is `{encode: (a) -> Json, decode: (Json) -> Result<a,
DecodeError>}`. Primitive codecs (`int_json`, `string_json`, `bool_json`,
`decimal_json`, `float_json`, `bytes_json`, `unit_json`, `json_json`) and
combinators (`list_json`, `option_json`, `result_json`, `map_json`,
`string_map_json`) are what `derive json` composes. `decode_bytes`,
`decode_string`, `encode_bytes`, `encode_string` are the ends of the pipe, and
`error_to_string` renders a `DecodeError` as `$.lines[2].unit_price: expected a
number, found a string`.

### 14.5 `std.db` — postgres, and its twin

```ply
pub nondet effect db {
  read  query[t](s: Stmt, ps: List<Param>)     -> Answer
  write execute[t](s: Stmt, ps: List<Param>)   -> Answer
  write returning[t](s: Stmt, ps: List<Param>) -> Answer
  write begin(level: Isolation, access: Access) -> Answer
  write commit()  -> Answer
  write abort()   -> Answer
  write rollback(reason: String) -> Unit
}
```

**The resource label is a table.** `db.query[items]` performs
`(db, items, Read)`. That is the whole reason to put a database behind an effect:
an endpoint's declared signature names the tables it touches, where a driver
answering `db.write[db]` for everything has thrown that away and kept the
ceremony.

The transaction control operations take no resource, so their atom is the
singleton `db.write` — a real scheduling cost, stated rather than discovered:
any two tests that open a transaction conflict even when their tables are
disjoint. They also do contend, for the pool. Read-only endpoints do not open
transactions and keep their concurrency.

**A SQLSTATE is a value, never a diagnostic.** A unique violation, a foreign-key
violation, a serialization failure, a connection that died mid-statement — all of
them are `Failed(e)` the program matches on. `is_retryable(e)` covers the two
serialization codes.

The module also ships **`MemDb`**, an in-memory twin: `open(schema)`, `step(db,
stmt, params)`, `begin_step`, `commit_step`, `abort_step`. The twin and the real
driver satisfy the same declared signature because there is only one signature.
`examples/agreement.ply` is the claim that they agree, checked against recorded
PostgreSQL 18.3 answers, and `ply prove examples/agreement.ply --host --db ...`
is how you run it against a live server.

The driver refuses statement text it cannot account for (`E0432`) rather than
guessing — its answer is a footprint, and a construct it silently ignored would
produce a row that under-reports, which corrupts scheduling with a green result
rather than a red one.

### 14.6 `std.config` — configuration as an effect

```ply
pub nondet effect config {
  read get[k](key: String)    -> Option<String>
  read secret[k](key: String) -> Option<Secret<String>>
}
```

`read`, so two readers never conflict — sound because the host reads its sources
**once**, at bind time, into an immutable map. There is no `config.set`.

The resource is a namespace the call site writes: `config.read[credentials]`
versus `config.read[server]`. It buys no scheduling; what it buys is that
`ply check --types` says which definitions read *credentials*.

`secret` answers `Option<Secret<String>>` and there is no operation that answers
a credential as a `String`, so there is no window in which one exists as an
ordinary `String`.

Declare a `ConfigSpec` with `spec`, `required`, `optional`, `with_default`, point
the run at it with `--config-schema module.fn`, and a missing required key is
`E0441` at start-up rather than a `None` two hundred requests in.

### 14.7 `std.trace` — observability as an effect

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

**The resource label is a channel**, so a function's row says which channels it
records on exactly as it already says which tables it touches. The cost, stated
where it is paid: a channel label cannot be abstracted over, so this module ships
**no function that performs** — every perform is written at its call site with
its channel, which is what makes a handler's clause list a capability grant.

`Sink` plus `event_step`, `enter_step`, `exit_step`, `count_step`, `gauge_step`,
`time_step`, `drain`, `named`, `on_channel`, `counter_total` are the collecting
twin a test installs.

There is no disabled path: a row cannot be conditional on a flag, so `--trace
off` binds a discarding host handler rather than removing the perform.

### 14.8 `std.signal` — the stop signal

```ply
pub nondet effect signal {
  read stopping()    -> Bool
  read deadline_ms() -> Int
}
```

A way to stop is ambient in every other language. Here it is an effect, so a
route that sheds load when the service is stopping says so in its row.

**`signal` does not bind under `ply test`, with or without `--host`.** A test that
could be ended by the suite's own ctrl-C would be a test whose verdict depends on
the terminal. Reaching an operation under `ply test` is `E0424`; the remedy is to
handle it over a `Stop` value, and `running()`, `draining(ms)`, `stopping_step`,
`deadline_step` and `has_time_for` are what the module ships for that.

### 14.9 `std.fs` — the filesystem, rooted

```ply
pub nondet effect fs {
  read  read_file[r](path: String) -> Option<Bytes>
  read  list_dir[r](path: String) -> Option<List<String>>
  read  exists[r](path: String) -> Bool
  read  file_size[r](path: String) -> Option<Int>
  read  modified_ms[r](path: String) -> Option<Int>
  write write_file[r](path: String, body: Bytes) -> Bool
  write create_dir[r](path: String) -> Bool
  write remove[r](path: String) -> Bool
  write rename[r](from: String, to: String) -> Bool
}
```

**A resource label is a root, and the root is the capability.**
`fs.read_file[src]("a.ply")` reads somewhere under whatever `src` names, and what
it names is bound beside the run — `ply run build.ply --host --fs src=./crates
--fs out=./target` — never in the program. A path written into a definition would
put a filesystem location into its hash and into a store designed never to
forget, and the same program would then mean two things on two machines.

Three consequences, and the third is the one that pays:

1. an operation naming a label no root is bound to is `E0451`, naming the label
   and the flag that would bind it;
2. a path that escapes its root — `..` anywhere in it, an absolute path, or a
   symlink that resolves outside — is `E0452`, refused before the syscall;
3. **two roots that do not overlap do not conflict**, so tests over `src` and
   tests over `out` run concurrently, and two readers of one root run
   concurrently while a writer serialises against both. That is §9.4's
   readers-writers rule applied to directories for free.

`nondet` is load-bearing exactly as it is in `std.net`: a `det` test that reaches
an operation here is `E0412` until a handler discharges it, and what a test
handles it with is the **twin** this module also ships — `mem_empty`, `mem_of`,
`mem_read`, `mem_write`, `mem_list`, `mem_create_dir`, `mem_remove`,
`mem_rename` and `mem_modified` over a `MemFs` value, so an in-memory filesystem
is one implementation everybody shares rather than one per test file.

Both import forms, because the test names two things: `fs` the effect and `fs`
the module its twin lives in. They are different namespaces (§4.4), so one name
serves both.

```ply
import std.fs
import std.fs (fs)

test "the manifest names every source file" {
  let tree = fs::mem_of([{path: "a.ply", body: b"fn f() -> Int = 1"}]);
  handle {
    assert_eq(fs.read_file[src]("a.ply"), Some(b"fn f() -> Int = 1"))
  } with {
    fs.read_file[src](p) -> fs::mem_read(tree, p),
  }
}
```

What v1 refuses, each because a compiler driver does not need it: file handles
and streaming (a read is whole-file, and a file over the bound is `E0453`), a
recursive walk (`list_dir` answers one directory), permissions and modes,
watching, `stdin`/`stdout`, and `argv`. `rename` is within one root, which is
what makes a cache write atomic — write under a temporary name, then rename into
place.

### 14.10 `std.hash` — BLAKE3, written in Ply

```ply
pub fn blake3(input: Bytes) -> Bytes     // 32 bytes, for input of any length
```

The hash this language's own content addressing is defined by (§2.3), written in
the language rather than exposed as a builtin — so the function deciding what a
definition *is* is a definition you can read. It is also the demonstration that
§3.5's operators and §13.10's builtins are enough to write real bit-level code:
before ADR 0033 this module could not have existed.

**It is slower than the compiler's own hash by orders of magnitude when
interpreted, and by a large factor under `--backend`**, which is a property of
where it runs rather than of the code: seven rounds of eight mixing functions per
64-byte block, each round an ordinary Ply call. Reach for it when a hash is what
you need and the input is small. `ply` itself hashes in Rust; ADR 0033 carries
the measurement and the bar it was taken against, and ADR 0035 narrowed the
gap under `--backend` by an order of magnitude, using this module as one of
its kernels, and records what remains of it and why.

What holds it to the truth is `crates/ply-eval-tests/tests/suite/blake3_differential.rs`,
which hashes the same input with this module and with the `blake3` crate the
compiler links, at every structural boundary the algorithm has — the block, the
chunk, and the tree above it. Zero disagreements, or the suite is red.

---

## 15. The host boundary

### 15.1 Hermetic by default

Nothing in Ply reaches the outside world unless you say so. Without `--host`, an
operation that reaches the boundary is `E0424`, and the diagnostic names the
handler that *would* have served it.

```
$ ply run examples/hello.ply           # E0424: nothing is bound
$ ply run examples/hello.ply --host    # binds the real handlers
$ curl -i localhost:8080
```

The program does not change; the binding does. The same code that serves a real
socket is what the tests at the bottom of that file drive over an in-memory one,
and those tests are deterministic and cacheable.

`ply test --host` exists, and a test that reaches a bound handler always runs and
is never cached. The default is the point: a suite that silently acquires a live
dependency is the failure mode this language exists to prevent.

### 15.2 The trusted computing base is a list

```
$ ply hosts examples/desk.ply --host
   25 host handlers · 47 operations · trusted computing base

   OPERATION                    ATOM                          HANDLER                 DET  LINEAR        BLOCKING  SECRETS
   std.db.db.query[items]       std.db.db.read[items]         ply_host::db::query     no   at-most-once  yes       no
   std.net.net.send[conn]       std.net.net.write[conn]       ply_host::tcp::send     no   at-most-once  yes       no
   ...
```

Every handler the binary can bind, the atom it answers, whether it is
deterministic, whether it is `at-most-once` or `repeatable`, whether it blocks,
and whether it may receive a `Secret`. `ply hosts --digest` prints one line a CI
check can pin.

Registration is checked before anything runs. A handler bound to a triple the
program does not declare is `E0421`; two handlers claiming one atom is `E0422`; a
handler declaring itself nondeterministic for an effect the program did not
declare `nondet` is `E0423` — the *declaration* is the authority, or `ply check`
would answer differently under `--host` and every cache would split on a flag.

### 15.3 TLS

```
$ ply run app.ply --host --tls api=certs/api.pem,certs/api.key
```

`--tls NAME=CERT,KEY` is repeatable, one credential per listener. PEM: a
certificate chain leaf first, and a private key in PKCS#8, PKCS#1 or SEC1. The
program calls `net.listen_tls[listener](port, "api")` with the *name*, so no certificate
byte reaches a definition's hash or the content-addressed store and a rotation
moves nothing. Credentials are loaded and validated at bind time (`E0430`), and
naming one the run does not hold is `E0429`.

`--tls` without `--host` is refused rather than silently ignored.

### 15.4 Database

```
$ ply run app.ply --host --db postgres://user@localhost/app --db-schema app.schema
```

The connection string is configured beside the run rather than written in the
program, for the reason a private key is: a password in a definition's hash is in
a store designed never to forget. `--db` reads `PLY_DB_URL` when it is absent,
and `PLY_DB_PASSWORD` for the password — which keeps the secret out of `ps` and
out of a shell history. `--db` unset under `--host` makes a `db` operation
`E0431`.

The pool and its deadlines are run configuration too: `--db-pool N`,
`--db-acquire-ms MS` (after which a waiting operation is `E0437`),
`--db-connect-ms MS`, `--db-statement-ms MS` (the server-side
`statement_timeout`), `--db-idle-txn-ms MS`
(`idle_in_transaction_session_timeout`) and `--db-statement-cache N`.

`--db-schema module.fn` names a nullary function returning a `db::Schema`. It is
resolved, checked and evaluated at start-up. **It does not compare against a live
server** — see §19.

`E0434` fires when a statement touches a table outside the declared footprint of
the entry point that reached it, at prepare time and again at answer time.

### 15.5 Configuration

```
$ ply run app.ply --host --config-schema app.config --set PORT=8137 --config app.env
```

`--set KEY=VALUE` and `--config FILE` (one `KEY=VALUE` per line) supply values,
and the environment is the third source. `--config-schema` names a `ConfigSpec`;
a missing required key is `E0441` at bind time, a value that does not satisfy its
declared shape is `E0442` (and the message never prints the value when the shape
is a secret), and a `--set` of a key the schema does not declare is `W0607` — the
classic silent deploy failure, made loud.

### 15.6 Tracing and shutdown

`--trace json` (the default under `--host`) binds a sink that writes one object
per line to **stderr**, so a run can be piped into `jq` while every `ply`
command's `--json` owns stdout. `--trace text` is the human form and
`--trace off` binds a *discarding handler* — a listed member of the trusted
computing base, not an absence, because a row cannot be conditional on a flag.
`--trace-level debug|info|warn|error` filters **in the sink**, so a filtered
`Debug` event still costs one perform and one `Fields` map.

`--drain-ms N` and `--drain-lead-ms N` control what a `SIGINT` or `SIGTERM` does
to a serving run: the lead is how long accept keeps running after the signal, so
a readiness route can answer 503 and a load balancer can take the instance out;
the drain is how long in-flight requests have to finish. A drain that expires is
`W0608` and exit code **3**, so a deployment can tell a clean stop from one that
dropped requests.

---

### 15.7 Filesystem roots

`--fs NAME=PATH`, repeatable, and refused without `--host` the way `--tls` is.
Each root is resolved **once**, before anything runs: a path that does not exist
or is not a directory is `E0454` there rather than a failure on the first write.
The resolved path is what every confinement check is against, and `ply hosts`
prints it:

```
   filesystem
   out  /home/you/project/target
   src  /home/you/project/crates
```

A run that bound none prints `none — an \`fs\` operation is E0451 until \`--fs
NAME=PATH\` binds its label`, for the same reason the credentials block says so.

**What the digest covers is the root names and not the paths.** A path is where
one machine was pointed — absolute, canonical, different in a checkout and in CI
— so hashing it would make the digest disagree between two runs of the same
command over the same program. Binding a root or removing one moves the digest;
pointing an existing one somewhere else does not. The listing above is the
instrument for that, and this is the same trade §15.3 makes for a certificate
fingerprint.

## 16. Building and shipping

```
$ ply build . --entry app.serve -o app.plyx
   built app.serve · b3:73e94e213e36
   artifact 3 definitions · 930 B · app.plyx
```

`ply build` writes the transitive closure of one entry point as a `.plyx`
artifact, identified by a BLAKE3 digest and verifiable against it. Every body is
checked against its own key when the artifact is loaded, so a corrupted transfer
is a refusal naming one definition (`E0443`) rather than a plausible wrong
program. An artifact built under a different front-end or runtime version is
`E0444`, which is a different code precisely because the responses differ:
rebuild it, versus transfer it again.

```
$ ply build . --digest              # print `b3:...` and nothing else; writes no file
$ ply build . --diff old.plyx       # added, changed, dropped, unchanged
$ ply run app.plyx --host           # run it out of its own definitions
```

`--sources` embeds the project's source text so a diagnostic raised in production
carries a line number. It is off, and a flag, because it is a disclosure
decision — and it changes the digest, so "was this built with sources" is
answerable from the digest alone.

`--config-schema` and `--db-schema` on `ply build` ship those functions' closures
too, so the deployed artifact keeps the start-up refusals: a schema function is
nullary and nothing in the entry point's closure calls it, so without the flag it
would not be in the artifact.

---

## 17. The `ply` command

```
ply [--color auto|always|never] <command> [path] [options]
```

`--color` is global and may follow the subcommand. `auto` uses colour and the
✓/✗ marks only when stdout is a terminal and `NO_COLOR` is unset.

Nearly every command takes a path defaulting to `.` — `ply std` takes none,
because what it reports is a property of the binary. Every command takes `--json`
and then emits exactly one JSON object on stdout and nothing else.

### Exit codes

| code | meaning |
| --- | --- |
| 0 | success |
| 1 | at least one test failed, or `main` raised |
| 2 | the program did not get as far as running: a bad path, a syntax error, a type error |
| 3 | the drain deadline expired with requests still in flight |

### `ply check [path]`

A program that declares a `reuse fn` is checked whole: every module is parsed
whatever the front-end cache says, and a promise the cost checker cannot show
is `E0127` with exit code 2 (§6.7).

| flag | meaning |
| --- | --- |
| `--types` | print the inferred signature and footprint of every definition |
| `--costs` | for every `push`, whether it grows its list in place or copies it, and what would remove the copy (§6.7) |
| `--explain` | which files were parsed and which definitions rechecked, with the reason a skip was refused |
| `--no-incremental` | neither read nor write the front-end cache |
| `--json` | |

### `ply test [path]`

| flag | meaning |
| --- | --- |
| `--filter SUBSTRING` | only tests whose `<module>.<label>` contains it |
| `--jobs N`, `-j N` | worker threads (default: one per core) |
| `--no-cache` | neither read nor write the result cache |
| `--no-incremental` | neither read nor write the front-end cache |
| `--explain` | selection reasons, concurrency groups, front-end timings |
| `--bisect auto\|always\|never` | attribute a failure to the change that caused it |
| `--bisect-budget N` | hybrid programs a bisection may evaluate (default 64) |
| `--trace auto\|always\|never` | record which definitions a failing test entered |
| `--backend BACKEND` | attach a compiled backend to the machine |
| `--audit-backend` | also run each test without the backend and fail on any disagreement |
| `--host` | bind the real host handlers |
| `--std` | also select the tests the shipped modules declare |
| `--seed`, `--sim`, `--seeds`, `--sim-budget`, `--sim-steps`, `--measure-reduction` | §10.4 |
| `--tls`, `--fs`, `--db`, `--config`, `--set`, `--config-schema`, `--db-schema` | §15 |
| `--json` | |

### `ply run [path]`

`--host`, `--seed`, the TLS/db/config flags, `--fs NAME=PATH` (§15.7,
repeatable, refused without `--host`), `--trace`, `--drain-ms`,
`--drain-lead-ms`, `--backend BACKEND`, `--json`. A `.plyx` path is run out
of its own verified definitions rather than out of a source tree it may not be
next to. `--backend` attaches a compiled backend exactly as `ply test`'s flag
does (§9.7), to a source tree or to an artifact: `main` runs with the machine
dropping into compiled code at the leaves, and the value printed is what it
produced. There is no audit under `ply run`; a program whose answer must be
checked against the interpreter's is a test.

`ply run` explores exactly one interleaving whatever `--seed` says — exploration
is a test-time activity — so the flag chooses which one rather than how many.

### `ply prove [path]`

`--filter`, `--jobs`, `--no-cache`, `--no-incremental`, `--explain`, `--std`,
`--host`, `--prove-cases`, `--prove-roots`, `--prove-budget`,
`--shrink-budget`, the simulation flags, the host flags, `--json`. §11.5.

### `ply review [path]`

`--changed` (the default; naming it is how a script says what it meant),
`--accept`, `--no-cache`, `--no-incremental`, `--std`, the prove and simulation
flags, `--json`. §11.6.

### `ply build [path]`

`--entry NAME`, `-o FILE`, `--config-schema`, `--db-schema`, `--sources`,
`--digest`, `--diff OLD.plyx`, `--json`. §16.

### `ply hosts [path]`

`--host` (list the handlers as bound rather than reporting that nothing is;
resolution and therefore any registration error happens either way), the TLS/db/
config/trace/shutdown flags, `--digest`, `--json`. §15.2.

### `ply std`

`--show MODULE`, `--digest`, `--json`. Needs no project: what it reports is a
property of the binary.

### `ply hash [path]`

`--deps` (also print each definition's direct references and transitive closure),
`--json`.

### `ply cache <action>`

| action | meaning |
| --- | --- |
| `clear [path]` | discard every cached result |
| `stats [path]` | where the cache lives, how much it holds, what is reclaimable |
| `compact [path]` | reclaim the space nothing points at |
| `inspect <DEF> [path]` | what the cache holds for one definition — by program-wide name (`store.orders.place`), simple name (`place`), or a hash prefix of four or more hex characters |

---

## 18. Diagnostics

Every diagnostic carries a stable code. `E` is an error and `W` is a warning; the
distinction is about **fault**, not severity — a `W` is never a fault in your
program.

### Lexical and syntactic

| code | meaning |
| --- | --- |
| `E0001` | unexpected token |
| `E0002` | unterminated string or byte-string literal |

### Names, modules and resolution

| code | meaning |
| --- | --- |
| `E0101` | unknown name |
| `E0102` | unknown type |
| `E0103` | unknown effect |
| `E0104` | unknown operation |
| `E0105` | duplicate definition (or a name the language reserves) |
| `E0106` | unknown module |
| `E0107` | private name |
| `E0108` | ambiguous import |
| `E0109` | module cycle |
| `E0110` | duplicate import |
| `E0111` | a file path that cannot name a module |
| `E0112` | ambiguous entry point |
| `E0113` | a project module under a reserved root (`std`) |
| `E0114` | unknown `effect set` — including `pub` on one, or a qualified reference to one |
| `E0115` | an `effect set` cycle |
| `E0116` | a record update whose base has no record shape this file can name |
| `E0117` | a record update naming a field the base does not have |
| `E0118` | a `?` whose enclosing function or lambda has no written return type this file can read as `Result` or `Option` |
| `E0119` | a `?` written where its early exit would change what runs, or would discard a written annotation |
| `E0120` | a parameter default written where no call could fill it in — on a lambda, an operation or a handler clause |
| `E0121` | a parameter default that is not a pure, closed expression, or that names another parameter of the same signature |
| `E0122` | a default on a `pub fn` mentioning a name its module does not export |
| `E0123` | a named argument that names no parameter, or names one twice |
| `E0124` | a positional argument after a named one |
| `E0125` | a parameter left unfilled by a call that used a name (plain under-application stays `E0202`) |
| `E0126` | a top-level `fn` that left a parameter type or its return type to inference — the diagnostic names the type it would have given |
| `E0127` | a `reuse fn` with an append its body cannot keep in place: the list is read again, captured, or held by a cell or map — the diagnostic names the append, the promise and the fix |

### Types

| code | meaning |
| --- | --- |
| `E0201` | type mismatch |
| `E0202` | arity mismatch |
| `E0203` | occurs check |
| `E0204` | not a function |
| `E0205` | non-exhaustive match |
| `E0206` | not derivable — including a `Map` key that is not ordered |
| `E0207` | unknown deriver |
| `E0208` | orphan `derive` |
| `E0209` | `/` applied to `Decimal` |
| `E0210` | an arithmetic or comparison operand whose numeric type nothing determines |

### Effects

| code | meaning |
| --- | --- |
| `E0301` | unbound row variable |
| `E0302` | effect not permitted by the declared signature |
| `E0303` | unhandled effect |
| `E0304` | a resource label is required |

### Tests, simulation and specs

| code | meaning |
| --- | --- |
| `E0412` | a nondeterministic effect in a deterministic test |
| `E0413` | a `Task` escapes its region |
| `E0414` | deadlock, or a spent step budget |
| `E0415` | replay did not reproduce the recorded schedule (Ply's fault) |
| `E0416` | nested `simulate` |
| `E0417` | an effect in a spec, guard or law body |
| `E0418` | a `forall` binder whose type cannot be quantified over |
| `E0419` | an obligation refuted by a counterexample |
| `E0420` | a vacuous obligation — the guard admits no values |

### The host boundary

| code | meaning |
| --- | --- |
| `E0421` | a host registration names something the program does not declare |
| `E0422` | two host registrations claim one atom |
| `E0423` | a host handler's determinism disagrees with the declaration |
| `E0424` | an operation reached the boundary with nothing bound (a hermetic run) |
| `E0425` | a host operation reached from a test the search re-runs |
| `E0426` | a continuation resumed twice across an at-most-once host operation |
| `E0427` | a host handler answered an atom outside the entry point's declared footprint |
| `E0428` | a `blocking: true` handler answered inline |
| `E0429` | `net.listen_tls` named a credential the run does not hold |
| `E0430` | a `--tls` credential that does not load |
| `E0431` | no database configured |
| `E0432` | statement text the driver refuses before preparing |
| `E0433` | the server refused to prepare a statement |
| `E0434` | a statement touches a table outside the declared footprint |
| `E0435` | the live database differs from the named schema *(reserved; see §19)* |
| `E0436` | a database operation from a task that does not own the transaction scope |
| `E0437` | the connection pool was exhausted |
| `E0438` | the live schema carries a trigger, rule or cascade nothing can model *(reserved)* |
| `E0439` | a `Secret` reached a host operation whose registration does not allow one |
| `E0440` | a configuration source that could not be read |
| `E0441` | a required configuration key that no source supplies |
| `E0442` | a configuration value that does not satisfy its declared shape |
| `E0443` | an artifact that does not verify |
| `E0444` | an artifact built under a different version |
| `E0445` | `trace.exit` naming a span that is not open on this task's stack |

### Regions

| code | meaning |
| --- | --- |
| `E0446` | a value would outlive its region |
| `E0447` | two regions in scope at once under one name |
| `E0448` | a region forced `unique` across which a continuation capture is reachable (there is no surface syntax for the annotation yet; the kind is inferred) |
| `E0449` | a handle into a region reaching a runtime boundary |
| `E0450` | a compiled backend that cannot be attached |
| `E0451` | an `fs` operation named a resource label no `--fs` bound a root to |
| `E0452` | a path that leaves the root its label names |
| `E0453` | a whole-file read of a file over the bound |
| `E0454` | a `--fs NAME=PATH` root that does not resolve, or is not a directory |

### Running

| code | meaning |
| --- | --- |
| `E0501` | assertion failed |
| `E0502` | runtime error — `panic`, division by zero, integer overflow, an out-of-range index, a spent `iterate` budget, the recursion limit |
| `E0503` | a compiled backend and the machine disagree (never a warning: the cache would record whichever ran first) |
| `E0505` | Ply broke one of its own invariants |

### Warnings

| code | meaning |
| --- | --- |
| `W0601` | the cache was unreadable |
| `W0602` | the cache is corrupt |
| `W0603` | the cache was written by another version |
| `W0604` | an obligation the system could not decide at any tier |
| `W0605` | the shipped modules changed since the cache was written |
| `W0606` | a host runtime could not hand every resource back at teardown |
| `W0607` | a configuration key supplied explicitly that the schema does not declare |
| `W0608` | the drain deadline expired with requests in flight (exit 3) |
| `W0609` | spans were still open when an entry point ended |
| `W0610` | a value was made to reach itself, so reference counting will never free it |

---

## 19. Limits and things Ply does not have

Ply is a research language. This section is the honest ceiling, and it is here
rather than left to be discovered.

### 19.1 Performance

* **One machine is one core.** A value holds a reference count and a continuation
  is a shared vector, so a Ply task cannot move between OS threads. Throughput
  scales by processes, where every runtime you would compare this against scales
  by threads.
* The evaluator is an interpreter — two of them — and native code generation is
  the one deferred mechanism. It is deferred on a measurement: the interpreter is
  about 35% of a served request, which caps any execution-strategy change at
  1.55×, and a Cranelift spike projected 1.48× against a 1.50× bar.
* Serving HTTP over TLS against real postgres runs at roughly **17–38×** the cost
  of the same syscalls in Rust with no interpreter under them, depending on what
  is being compared. `README.md` has the measurements and the exact conditions.
* **The in-memory database twin is slower than the database it replaces.** Use it
  for isolation and determinism, which it does deliver; not for speed.
* Recursion is capped at 10,000 nested calls, and there is no per-test timeout —
  a hang hangs the suite. Give every loop a written bound.

### 19.2 Language features that are absent

* **No dispatch mechanism at all** — no typeclasses, no implicits, no instance
  resolution, no method syntax. A derived codec is a plain function and you pass
  it. The risk that accepts is **coherence**: nothing stops module A calling
  `order_json` and module B calling `order_json_v2`, both type-checking, one type
  serializing two ways. The orphan rule is the coherence there is, and it is a
  local property.
* **A tuple is a record** with positional fields (§5.3): `(a, b)` is
  `{_0: a, _1: b}` everywhere, so there is no second kind of value to derive,
  hash or match on.
* **No loops and no `break`.**

  > **This bullet read "No loops, no `break`, no early `return`", and the third
  > of those is now narrowed rather than wrong.** There is still no `return`
  > statement: nothing in Ply transfers control out of a function. But `?`
  > (§6.10) is an early exit, and it is the one the language has. It is not a
  > control transfer — the parser rewrites it into a `match` — so it cannot
  > leave a lambda, a handler, a region or a loop-shaped recursion, and it exits
  > only the expression it is written in. Everything the original bullet was
  > warning about still holds; what changed is that binding a `Result` no longer
  > costs a `match`.
* **No mutable variables.** Cells, in regions.
* **No exceptions.** A failure is `E0502` and ends the run; recoverable failure is
  a `Result` or a domain type.
* **No modules-as-values, no first-class effects, no abstraction over a resource
  label.** Labels are ground identifiers in the source.

* **No unsigned integer type.** `Int` is signed, and the bit operators (§3.5)
  work on its two's-complement pattern; `>>` and `>>>` are both in the language
  because there is no `UInt` to choose between them for you.
* **No `unsafe`, no FFI.** Everything below the boundary is in the compiler's
  trusted computing base, which `ply hosts` prints in full.
* **Nothing in a spec may name mutable state**, so a function whose whole job is
  to move a resource carries a `requires` and no `ensures`, and counts as
  uncovered in `ply prove`'s first line. That is the honest artifact.

### 19.3 Runtime and platform gaps

* **The filesystem is whole-file and rooted, and nothing more.** `std.fs`
  (§14.9) has no file handles and no streaming — a read is the whole file, and
  one over the bound is `E0453` — no recursive walk, no permissions or modes, no
  watching, no `stdin`/`stdout`, and no `argv`. A program reaches only what is
  under a root the run bound.

* **No cancellation, no backpressure, no load shedding.** A request still live at
  the drain deadline loses its connection with no response and the process exits
  `3`. An overloaded service queues until something times out.
* **No migrations, and the start-up database schema check does not exist.**
  `--db-schema` resolves, checks and evaluates the schema function; it never opens
  a connection to compare. `E0435` is reserved and raised nowhere. What you get
  instead is real but later and narrower: a statement whose shape the database
  disagrees with fails at prepare time with `E0433`, per statement and on first
  execution.
* **HTTP/1.1 only.** ALPN advertises `http/1.1` and nothing else. Also missing:
  compression in either direction, WebSockets, `Upgrade`, `CONNECT`, mTLS,
  SNI-based certificate selection, and session resumption.
* **No authentication or authorization framework.** There is a typed-secret API
  key comparison in the example service and nothing else.
* **Also not built:** a query builder or ORM, `LISTEN`/`NOTIFY`, cursors, a time
  type, a database per test, a template language, metrics backends, log shipping,
  distributed tracing propagation, trace sampling, live config reload, artifact
  signing, and secret zeroization.

### 19.4 Sharp edges worth knowing before you hit them

* A bare variable followed by `.name(` parses as an effect perform, so calling a
  function stored in a record field needs `(r.f)(x)`.
* A handler discharges an *atom*, not an operation, so a missing clause is a run
  time failure rather than a compile error.
* `{..b, f: e}` needs the base's shape to be readable from the same file, so a
  base whose type is declared in another module is `E0116`.
* `?` inside a lambda needs the lambda's written return type (`|x| -> T { .. }`,
  §6.4); a lambda without one is `E0118`. An `iterate` step answers `Iter`, so
  it can never carry a `?`.
* `?` in a call argument refuses if anything impure is evaluated to its left
  (`E0119`); the fix is a `let`.
* `?` on a `let` with a written type is `E0119`. Drop the annotation, or put it
  on the value being unwrapped.
* Two tasks that each allocate are ordered by the search even when nothing in
  their rows conflicts, because allocation draws from one bump pointer.
* A task that reads shared state and writes it back with no scheduler operation
  in between runs both as one step, and no schedule separates them.
* An append copies only when something else still owns the list — a binding
  read again after the `push`, a closure's capture, a cell or map entry, a
  caller that keeps reading what it passed. Position in the enclosing
  expression decides nothing: `ply check --costs` names each copy's cause.
* `string_find` raises when the needle is absent; guard with `string_contains`.
* `bytes_at` and `string_slice` **raise** out of range. `list_at` does not — it
  answers `None`. The two containers are indexed by different conventions on
  purpose; §13.2 and §13.6 say which is which, and
  `docs/adr/0027-a-list-index.md` says why.
* A negative list index is **absent**, not counted from the end:
  `list_at(xs, -1)` is `None`, not the last element.
* Slices and indices are never clamped — and a list index is not clamped either:
  an out-of-range one is absent, not the nearest element.

---

## 20. Where to go next

### The examples, in the order they are worth reading

| file | what it shows |
| --- | --- |
| `examples/clock.ply` | the smallest complete picture: a `nondet` effect, a handler, `E0412`, and `test/nondet` |
| `examples/ledger.ply`, `examples/report.ply` | two modules, `pub`, imports, and pure business logic — plus `requires`/`ensures` and laws over it |
| `examples/pipeline.ply` | the reduction, made visible: three workers that share nothing explore one interleaving |
| `examples/bank.ply` | the oldest concurrency bug there is, its fix, and four laws — one of them proved by exhaustive interleaving |
| `examples/timeout.ply` | deadlines and backoff against a virtual clock |
| `examples/echo.ply` | the smallest program that reaches a socket |
| `examples/hello.ply` | a real HTTP endpoint, hand-parsed, with an in-memory socket under its tests |
| `examples/orders.ply` | a JSON endpoint whose entire wire format is four `derive json` lines |
| `examples/store.ply` | a handler as a capability grant: three resources, one effect, and a schedule read off the types |
| `examples/agreement.ply`, `examples/twin_divergence_audit.ply` | `std.db`'s in-memory twin, and the claim — checked against recorded postgres answers — that it agrees with the real thing |
| `examples/desk.ply` | the whole thing: a multi-route service over postgres, with TLS, config, tracing and a clean shutdown |

`examples/serve.sh --memory` starts the desk service with no database.
`examples/same-tests.sh` runs the same suite against the twin and against a live
server.

### The documents

| file | what it is for |
| --- | --- |
| [`DESIGN.md`](../DESIGN.md) | the design rationale, mechanism by mechanism |
| [`README.md`](../README.md) | the measured claims, and where they do not hold |
| [`docs/ONBOARDING.md`](ONBOARDING.md) | clone to first change, every command run and its output recorded |
| [`CONTRIBUTING.md`](../CONTRIBUTING.md) | how the project works on itself |
| [`ROADMAP.md`](../ROADMAP.md) | the milestone record |
| `docs/adr/` | 27 decision records; each section above cites the one that specifies it |

The ADRs most worth reading alongside this guide are 0006 (deterministic
simulation), 0007 (specs), 0008 (the host effect boundary), 0010 (generic
derivation), 0017 (regions), 0022 (the call ceiling and `iterate`), and 0027
(the list index, why `bytes_at` raises where `list_at` does not, and the
measurement that refused a second builtin).
