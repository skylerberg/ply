# The Ply Guide

Ply is a statically typed, functional programming language. Three ideas set it apart:

- **A function's type lists its effects.** Reading a file, writing to the network or reading the
  clock is an *effect*, and a function's signature lists every effect it can have. A signature
  tells you everything a function can touch.
- **Tests are deterministic.** A test replaces the clock, the network or a database with a
  *handler* written in Ply, so it gives the same result every run, and concurrent code is tested
  under a scheduler that can try every order of events.
- **Work is incremental.** Ply identifies each definition by a hash of its content. After an edit,
  `ply` checks, builds and tests only what the edit can affect, and reuses the rest.

The compiler is written in Ply and compiles to C. [`GOALS.md`](../GOALS.md) explains what Ply is
for. This guide is the manual for the language and the `ply` command. Section 2 is a short tour;
the rest is reference.

1. [Getting started](#1-getting-started)
2. [A tour of Ply](#2-a-tour-of-ply)
3. [Syntax](#3-syntax)
4. [Functions and modules](#4-functions-and-modules)
5. [Types](#5-types)
6. [Expressions](#6-expressions)
7. [Effects](#7-effects)
8. [Resources and scopes](#8-resources-and-scopes)
9. [Concurrency and simulation](#9-concurrency-and-simulation)
10. [Testing](#10-testing)
11. [Specifications and proofs](#11-specifications-and-proofs)
12. [Derivation](#12-derivation)
13. [Builtins and the standard library](#13-builtins-and-the-standard-library)
14. [Running programs](#14-running-programs)
15. [Packages](#15-packages)
16. [Building, publishing and signing](#16-building-publishing-and-signing)
17. [The `ply` command](#17-the-ply-command)
18. [Diagnostics](#18-diagnostics)
19. [Limits and sharp edges](#19-limits-and-sharp-edges)
20. [Examples](#20-examples)

## 1. Getting started

### 1.1 Installing

Build `ply` from a checkout of this repository:

```sh
scripts/bootstrap.sh --release
```

This writes `target/release/ply`. Put it on your `PATH`, and run the script again after you pull
changes. To skip the build, `scripts/stage0.sh PATH` downloads the latest build of `main` to
`PATH`; it needs the GitHub CLI (`gh`) and has builds for macOS on Apple silicon and Linux on
x86-64. `ply` also needs a C compiler, which it runs to compile your programs.

### 1.2 A first program

```ply
// hello/main.ply
fn greeting() -> String = "hello from ply"

fn main() -> Unit / {Console} = print(greeting())
```

`ply run hello` runs `main`, which prints `hello from ply`. Standard output contains only what the
program writes; `ply`'s own errors go to standard error. `ply run --json` reports the value `main`
returns.

`/ {Console}` lists `main`'s *effects*: what it may perform. Printing is an effect, so a
function that prints says so in its type. `print`, `read_line` and `Console` come from
`std.console` and need no import. To print a value that is not a `String`, use an interpolated
string: `print(f"n = {n}")`.

### 1.3 Packages, modules and the project root

`ply new demo` creates a package:

- `demo/ply.pkg`, the package's manifest (§15.1);
- `demo/main.ply`, with a `main` that prints and one test.

`cd demo && ply run .` prints `hello from demo`, and `ply test` runs the test. `ply new --lib`
creates a library instead: a `lib.ply` with one `pub` function and no `main`. The last part of the
path is the package's name, which is lowercase letters, digits and `_`, in parts joined by dots.
`--name` sets another name.

**The project root.** A command finds the project's root by looking in the directory it was given,
or the working directory, and then in each parent, for the nearest `ply.pkg`. A path inside the
package narrows what the command does without moving the root: `ply test store` runs the tests of
the modules under `store/`, and `ply run tools/gen.ply` runs that file's `main`.

When no parent directory has a `ply.pkg`:

- a directory you name is the root;
- a file you name is a program on its own, made of that file and the modules it imports;
- a command given no path is an error (`E0502`).

**Modules.** Every `.ply` file under the root is a module. Its name is its path with `/` replaced
by `.` and without `.ply`: `store/orders/place.ply` is the module `store.orders.place`. Every
directory and file name must be a valid identifier (`E0111`). `ply` skips directories whose names
start with `.`, directories named `target`, and directories with their own `ply.pkg`, which are
separate packages.

`std` (the standard library) and `compiler` are built-in packages that every program can import.
A module of your own may not use either name as its first part (`E0133`).

### 1.4 The everyday commands

Each command takes a `.ply` file or a directory, and works on the project that contains it. With no
path, it works on the package that contains the working directory.

| command | what it does |
| --- | --- |
| `ply check` | parses the program, resolves names, and checks types and effects |
| `ply test` | runs the tests that an edit could have changed (§10) |
| `ply run` | runs `main` |
| `ply fmt` | formats source files |
| `ply doc NAME` | prints the documentation of a definition, type, effect or module |
| `ply prove` | proves the program's laws and specifications (§11) |

`ply check --types` prints the type and effects of every definition. §17 lists every command
and flag.

### 1.5 The cache

`ply` stores results in `.ply-cache/` at the project root: checked modules, test results, proofs,
review baselines and fetched dependencies. Add it to `.gitignore`. Deleting it is always safe, and
`ply cache clear` empties it. A run that crashes, or that races another run, can make a later run
redo work, but never gives a wrong result.

A definition's hash covers what it depends on, but not its comments, formatting or name. So a
rename or a comment edit reruns no test, and an edit to one function's body that keeps its
signature rechecks only that function and the functions in a recursion with it. A result is reused
only by a `ply` that would compute it the same way; results from another `ply` are recomputed
(`W0603`).

`PLY_CACHE_UPSTREAM=DIR` adds a second cache that several checkouts or machines share. Test results
and proofs found there are used, and new ones are written there. `PLY_CACHE_UPSTREAM_READONLY=1`
only reads it. `--no-cache` ignores both caches.

## 2. A tour of Ply

This section shows the main features in one pass. Each part links to its full description.

**Types and functions.** Records are structural and sum types are declared with `|`. A `match`
must cover every case (§6.2):

```ply
type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point

fn area(s: Shape) -> Int =
  match s {
    Circle(r) -> 3 * r * r,
    Rect(w, h) -> w * h,
    Point -> 0,
  }
```

There are no loops or mutable variables. A function that calls itself in tail position runs as a
loop (§6.6), and `List::map`, `List::filter` and `List::fold` cover most iteration.

**Effects.** An effect is a set of operations that a handler implements. A function's signature
lists the operations it performs, and a `handle` supplies the implementation (§7):

```ply
nondet effect wall_clock {
  read now() -> Int
}

fn expired(started: Int, ttl: Int) -> Bool / {wall_clock.read} =
  wall_clock.now() - started > ttl

test "a session expires once its time to live has passed" {
  handle {
    assert(expired(1000, 60))
  } with {
    wall_clock.now() -> 1061,
  }
}
```

The test supplies the time, so it gives the same result on every run. `nondet` marks an effect
whose results come from outside the program. A test may not leave one unhandled unless it is
written `test/nondet` (§10.2).

**Failures.** There are no exceptions. A failure is a `raise` operation, so a function's signature
lists every way it can fail. `try` turns a raise into a `Result`, and `?` returns early from a
function that returns `Result` or `Option` (§7.7):

```ply
type SyntaxError = { line: Int, message: String }

effect toml {
  raise syntax(e: SyntaxError)
}

fn digit(b: Int) -> Int / {toml.syntax} =
  if b >= 48 && b <= 57 { b - 48 } else { toml.syntax({ line: 1, message: "not a digit" }) }

fn checked(b: Int) -> Result<Int, SyntaxError> = try { digit(b) }
```

**Concurrency.** Tasks and channels are effects too. Inside `simulate`, a scheduler controls the
order in which tasks run and tries every order that could change the result (§9):

```ply
test "no account is ever overdrawn" {
  simulate {
    let a = task.spawn(|| settle("alice", "bob", 60));
    let b = task.spawn(|| settle("alice", "carol", 60));
    task.join(a);
    task.join(b);
    assert_eq(overdrawn(ledger.balances()), 0)
  }
}
```

**Laws.** A law states a property for every input. `ply prove` tries to prove it, and shows an input
where it fails if it finds one (§11):

```ply
fn reverse(xs: List<Int>) -> List<Int> =
  match xs { [] -> [], [x, ..rest] -> reverse(rest).push(x) }

law "reversing twice gives the list back" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }
```

**Incremental testing.** `ply test` runs a test again only when something the test depends on
changed. A test that reads a file or a database through a handler is cached too, because the
handler is part of the test (§10.3).

## 3. Syntax

### 3.1 Files and comments

A source file is UTF-8 text: its imports, then its items in any order. Items are `fn`, `type`,
`effect`, `effect set`, `test`, `law`, `law schema`, `derive`, `key`, `show`, `invariant`,
`numeric`, `release` and `host` declarations. Definitions may refer to each other in any order,
across the whole program. Whitespace only separates tokens; indentation has no meaning. A byte
sequence that is not valid UTF-8, or a non-ASCII character outside a literal or comment, is
`X0001`.

`//` starts a comment that runs to the end of the line. A *doc comment* documents a declaration
for the people who use it:

- `///` lines document the item below them: a `fn`, `extern fn`, `type`, `effect`, effect
  operation, `effect set`, `law schema`, variant, or field of a declared record.
- `//!` lines at the top of a file, before the first import or item, document the module.
- `////` is an ordinary comment.

```ply
//! Orders, and what a shop does with them.

/// Where an order stands.
pub type Status =
  /// Placed, not paid.
  | Open
  /// Paid, with the receipt's number.
  | Paid(Int)

/// The cents an order is for. Never negative.
pub fn cents_of(o: Order) -> Int = ..
```

Write one space after the marker. The first sentence is the summary, and a bare `///` line starts a
new paragraph. A doc says what the signature cannot: what the result means, when the function
fails, units and edge cases. Leave out examples: the tests and laws that use a definition are its
examples, and `ply callers` lists them. A doc comment that documents nothing, such as one inside a
body or above a `test` or `import`, is `E0003`. Comments never change a hash or a cached result.

### 3.2 Names and keywords

An identifier is an ASCII letter or `_`, followed by ASCII letters, digits or `_`. `_` alone is the
wildcard. Case matters in two places:

- in a type, a lowercase name is a type variable and an uppercase name is a type;
- in a pattern, a lowercase name binds a variable and an uppercase name is a constructor.

By convention, values are `snake_case`, types and constructors are `UpperCamelCase`, and effects
and resource labels are lowercase.

These words are always reserved: `pub` `import` `fn` `type` `effect` `nondet` `test` `let` `if`
`else` `match` `handle` `with` `true` `false`. A record field may still use one (`d.nondet`), but
not in the short form `{nondet}`, which would also bind a variable.

Other words are keywords only in the position where they have a meaning, and are ordinary names
everywhere else: `as`, `read`, `write`, `raise`, `set`, `new`, `opaque`, `law`, `schema`, `forall`,
`where`, `derivable`, `identity`, `cost`, `space`, `bounded`, `derive`, `for`, `in`, `by`, `key`,
`show`, `invariant`, `numeric`, `host`, `release`, `keep`, `once`, `reuse`, `transparent`, `const`,
`returns`, `fresh`, `requires`, `ensures`, `decreases`, `resume`, `return`, `with_scope`,
`simulate`, `try` and `parallel`.

### 3.3 Literals

| literal | type | notes |
| --- | --- | --- |
| `42`, `1_000_000`, `0xFF` | `Int` | 64-bit signed. A hex literal is a bit pattern, so `0xFFFF_FFFF_FFFF_FFFF` is `-1`. |
| `255u8`, `0x6A09_E667u32`, `-1i8` | a fixed width | Suffixes `u8` to `u128` and `i8` to `i128`. The value must fit (`256u8` is `E0211`). |
| `1.5`, `1e9`, `2.5e-3` | `Float` | IEEE-754, 64 bits |
| `1.50m`, `0m` | `Decimal` | Exact base 10; keeps the digits as written |
| `"text"` | `String` | UTF-8, on one line |
| `f"n = {n + 1}"` | `String` | interpolated string |
| `\\text` | `String` | line string: verbatim lines |
| `b"GET "` | `Bytes` | ASCII characters and `\xNN` escapes |
| `b'{'`, `b'\n'` | `Int` | one byte |
| `'a'`, `'\u{1F600}'` | `Char` | one Unicode scalar value |
| `uuid"6ba7b810-.."` | the tag's type | tagged literal (§3.4) |
| `#{"a": 1, k: v}` | `Map<k, v>` | a later entry replaces an earlier one with the same key |
| `#[1, 2]` | `Set<a>` | each element once (§5.4) |
| `true`, `false`, `()` | `Bool`, `Unit` | |

`1`, `1.0`, `1m` and `1u32` have four different types, and Ply never converts between them
implicitly. A numeric literal is never negative: `-3` is `-` applied to `3`, except in a pattern.
So the smallest value of a signed width has no literal; write `I8::of_int(-128)`.

String and character escapes are `\n` `\t` `\r` `\0` `\\` `\"` `\'` and `\u{...}`, with one to six
hex digits. Byte strings also allow `\xNN`, but not `\u{...}` or non-ASCII characters. `b'{'` is the
same literal as `123`, so a byte scanner can `match` on characters (§6.2).

**Interpolated strings.** `f"a {x} b"` means `"a " ++ display(x) ++ " b"`. A `String` or `Char`
goes in as it is, and any other value as `show` writes it. A hole may contain any expression.
`{{` and `}}` are literal braces. A value in a hole must be showable, so a function, `Secret`,
`Task` or `Chan` there is `E0206`. A module that interpolates imports `std.show` automatically, so
`std.show` and the modules it imports cannot interpolate.

**Line strings.** Each line starts with `\\`, after optional spaces. The text after `\\` is taken
exactly as written, with no escapes. Lines are joined with `\n`, and no newline follows the last
line, so a value that ends with a newline ends with a line holding only `\\`:

```ply
fn program() -> String =
  \\fn main() -> Int = 42
  \\
```

A line string cannot be a pattern, and a line that ends in a space or tab is `E0001`.

**Map and set literals.** A map literal is a call of `Map::of_entries`, and a set literal a call of
`Set::of_list`. A module that writes a set literal imports `std.set` automatically.

### 3.4 Tagged literals

A tagged literal is a module name followed directly by a string: `uuid"..."`. Ply runs the
module's parser on the text when it checks the program, so a malformed literal is a compile error:

```ply
import std.uuid
import std.uuid (Uuid)

fn dns() -> Uuid = uuid"6ba7b810-9dad-11d1-80b4-00c04fd430c8"
```

A tagged literal can have holes. The tag then receives the values apart from the text, so it can
escape each value for where it appears. Nothing in a hole is ever read as text, which makes
injection impossible:

```ply
import std.html
import std.html (Html, html, text)

fn row(name: String, kind: String) -> Html / {html.unsafe_url, abort.raise} =
  html"<li class={text(kind)}>{text(name)}</li>"
```

`std.uuid`, `std.base64` and `std.bigint` are tags without holes; `std.html` and `std.sh` take
holes.

**Writing a tag.** `tag"text"` calls `tag::literal("text")`, where `tag` is a module imported under
that name. A literal with holes calls `tag::compile` on the text pieces when the program is checked,
and `tag::fill` with the result and the hole values when it runs.

- `literal` is a `pub fn` taking a `String` and returning `Result<T, {message: String, offset:
  Int}>`.
- `compile` takes the list of text pieces (one more than there are holes) and returns
  `Result<C, {message: String, offset: Int, part: Int}>`.
- `fill` takes the compiled `C` and a `List<H>` of the hole values, and returns the literal's value.
- Each takes no type, label or effect parameters and may only raise (`E0152`).
- An `Err` from the parser is `E0153`, at the character `offset` of the text as written. A parser
  that raises or runs for more than 60 seconds is `E0154`.

The result is cached under the parser's code and the text, so a parser runs again only for new text
or after an edit to what it depends on. `f` and `b` are not tags, and a tagged literal is never a
pattern. A module that defines its own `Ok` cannot use tagged literals (`E0118`).

### 3.5 Operators

From loosest to tightest binding. Every binary operator is left-associative.

| precedence | operators | operands |
| --- | --- | --- |
| 1 | `\|\|` | `Bool` |
| 2 | `&&` | `Bool` |
| 3 | `==` `!=` `<` `<=` `>` `>=` | see below |
| 4 | `\|` | integer |
| 5 | `^` | integer |
| 6 | `&` | integer |
| 7 | `<<` `>>` `>>>` | integer; the shift count is an `Int` |
| 8 | `++` | `String` or `Bytes` |
| 9 | `+` `-` `+%` `-%` `+\|` `-\|` | numeric; the `%` and `\|` forms on integers |
| 10 | `*` `/` `%` `*%` `*\|` | numeric; the `%` and `\|` forms on integers |
| — | prefix `-` `!` `~` | numeric, `Bool`, integer |
| — | postfix `f(x)` `r.field` `e.op[r](x)` `e?` | |

- Both operands have the same type. There is no widening: `U8 + U16` is `E0201`.
- `==` and `!=` compare structurally at every type except functions. `Float` equality follows
  IEEE, so `NaN != NaN`.
- `<`, `<=`, `>` and `>=` work on numbers and `Char`. Use `compare` to order anything else.
  Arithmetic on a `Char` is `E0201`; convert with `Int::of_char`.
- `&&` and `||` short-circuit. `>>` is an arithmetic shift and `>>>` a logical one.
- `++` joins two `String`s or two `Bytes`. Convert between them with `Bytes::of_string` and
  `String::of_bytes`.
- `::` refers to a name in a module (`orders::place`), and cannot be chained.
- `?` binds tightest: `f(x)?.field` is `(f(x)?).field`.

**Arithmetic is checked.**

- Division or remainder by zero raises `abort.raise` (§7.7), unless the divisor is a nonzero
  literal, the operands are `Float`, or the checker can see that an `Int` divisor is not zero.
- Overflow, a `Decimal` result that cannot be exact, and a shift by a negative count or by the
  width or more end the program (`E0502`). These do not appear in a function's effects.
- `+%`, `-%` and `*%` wrap at the type's width, and `+|`, `-|` and `*|` saturate at its limits:
  `255u8 +% 1u8` is `0u8` and `250u8 +| 10u8` is `255u8`. They never fail. Write a space before a
  lambda used as a right operand (`a + |x| x`), since `+|` would otherwise be one operator.
- The `checked_*` builtins return `None` where an operator would fail (§13).
- `/` on `Decimal` is `E0209`; use `Decimal::div`, which takes the result's scale.

## 4. Functions and modules

### 4.1 Functions

```ply
type Account = { name: String, balance: Int }

fn credit(a: Account, amount: Int, note: Option<String> = None) -> Account = {
  let balance = a.balance + amount;
  { name: a.name, balance }
}
```

A body is `=` and an expression. There is no `return`: a function's value is its body's value. A
body with `let`s is a block, in braces.

A top-level `fn` writes every parameter type and its return type (`E0126`, which shows the inferred
type), and its effects, if it has any. A parameter the body does not use may be written `_`.

**Defaults.** A parameter with a default may be left out of a call. A default must be a literal, a
`const` (§4.5), or a constructor, record or list of those (`E0121`). On a `pub fn`, a default may not
use anything its module does not export (`E0122`).

**Functions of a type.** `fn Stack::size(s: Stack) -> Int` declares the function `size` of the type
`Stack`. It is called as `Stack::size(s)`, or on a value as `s.size()` (§6.4), wherever `Stack` is
in scope. It need not take a `Stack`: `Stack::empty()` can make one. Only the module that declares a
type may declare its functions (`E0208`), and the type must be a sum or a `new` record (`E0217`).

**Modifiers.** `reuse fn` promises that every list update in the body happens in place (§6.10).
`transparent fn` lets other packages' proofs read the body (§11).

**The entry point** is `main`, of any type and effects. A program with no `main` is `E0101`, and one
with several is `E0112`. Name a file to choose its `main`: `ply run tools/gen.ply`.

### 4.2 Holes

```ply
pub fn search(g: Game, s: State) -> Move =
  if over(s) { last(g) } else { ??? }

pub fn reward_of(r: Rewards, p: Player) -> Float = ..
```

`???`, or `..` in place of a body, is a *hole*: code not written yet. A hole has whatever type is
needed and performs nothing, so the program still checks and runs. Each hole is a warning (`W0613`)
that reports the type the hole must have, the effects it may perform, and up to eight names in
scope of a fitting type. Reaching a hole at run time raises `abort.raise`, so the tests that run
it fail and the others pass. A file of signatures with hole bodies is a design `ply check` can
judge.

### 4.3 Imports and visibility

```ply
import store.orders                 // binds the module as `orders`
import store.orders as ord          // binds it as `ord`
import store.orders (place, cancel) // binds those names, and no module name
```

Imports come before every item. Use `::` for a name in an imported module: `orders::place(..)`. To
both rename a module and import names from it, write two imports.

The first part of a module path may be a dependency's prefix (§15.2): `import cli.surface` is the
`surface` module of the dependency `cli`. A path without a known prefix is a module of the current
package. Inside a dependency, a bare import means that package's own module. An import that matches
nothing is `E0106`.

Items are private unless marked `pub` (`E0107`). Functions and constructors, types, effects and
modules are separate namespaces, so `fn size`, `type Size` and `effect size` can coexist.

### 4.4 Opaque types

Another module can build a public sum from its constructors, or a public `new` record (§5.2) with a
record literal. `opaque` keeps building values inside the declaring module, so every value comes
from that module's functions, and any property they guarantee holds for every value:

```ply
pub opaque type Token = | Token(String)
pub opaque type Date = new { year: Int, month: Int, day: Int }

pub fn mint(s: String) -> Option<Token> = if s == "" { None } else { Some(Token(s)) }
pub fn text_of(t: Token) -> String = match t { Token(s) -> s }
```

Other modules can name the type, pass its values and call the module's functions. Using a
constructor of an opaque sum outside its module is `E0107`, and building or updating an opaque
record there is `E0220`. Other modules can still read an opaque record's fields; to hide the
contents, use a sum with one constructor.

`opaque` controls who builds values, not who reads them. `==`, `show` and `reflect` read a whole
value, and a codec the module derives (§12) decodes any document of the right shape. A type with
rules beyond its shape should write its codec by hand. A `Secret` is the type nothing can read
(§5.7).

### 4.5 Constants

```ply
type Status = { code: Int, reason: String }

pub const max_redirects: Int = 5
const statuses: Array<Status> = parsed(embed("status_codes.csv"))

fn allowed(hops: Int) -> Bool = hops <= max_redirects
```

A `const` names a value. Its type is required, and it is read by name, with no parentheses. Another
module imports it like a function and reads it as `http::max_redirects`, or as `max_redirects` after
`import http (max_redirects)`.

A `const` is computed the first time a run reads it. Later reads in the same process return the same
value. Use it to parse a data file once.

- Its value may raise and do nothing else, and must terminate. It must be data: no function, `Task`,
  `Chan` or `Secret`. Anything else is `E0156`.
- A raise in the value ends the run, whatever handlers surround the read. Test what it returns.
- A local variable, a parameter or a pattern variable of the same name hides it.

A `const` of type `std.sql.Schema` is a database schema. The SQL statements of its module, and of
every module that imports it, are checked against it before the program runs (§14.8).

`const fn name() -> Type = value` declares the same constant, read with a call: `name()`.

A plain function with no parameters that performs nothing is also computed only once per process.

### 4.6 Embedding files

```ply
fn schema() -> Bytes = embed("schema.sql")
fn fixtures() -> List<{ name: String, bytes: Bytes }> = embed_dir("fixtures")
```

`embed("path")` is a file's contents. `embed_dir("path")` is every file under a directory, named by
its path inside the directory and sorted by that name, skipping names that start with `.`. The path
is a string literal (`E0147`), relative to the module's own file.

The file's contents are part of the definition's hash, so a test that reads an embedded file runs
again exactly when the file changes. A missing or unreadable path, or the wrong kind of path, is
`E0146`.

## 5. Types

Ply infers types, and effects, with Hindley–Milner unification. The types you write in a
signature are checked, never widened by inference (§5.6).

### 5.1 Numbers and other scalars

| type | values |
| --- | --- |
| `Int` | 64-bit signed integers; the type for counting and indexing |
| `U8` `U16` `U32` `U64` `U128` `I8` `I16` `I32` `I64` `I128` | fixed-width integers |
| `Float` | IEEE-754 64-bit floating point |
| `Decimal` | exact base-10 numbers |
| `Bool`, `Unit` | `true` and `false`; `()` |
| `Char` | one Unicode scalar value |
| `String` | UTF-8 text, indexed and sliced by character |
| `Bytes` | an immutable byte sequence, indexed by byte |

There are no implicit conversions. An operand's type comes from the whole definition: in `fn h(a:
U32) -> U32 = a + 1u32` the literal needs its suffix. An operand nothing determines, as in `let g =
|a, b| a + b;`, is `E0210`. Convert explicitly:

- `U32::of_int` and the like raise when the value does not fit. Mask to truncate:
  `U8::of_int(n & 0xFF)`.
- `to_int` reads any integer type as an `Int` (`None` if it does not fit), and `numeric_of_int`
  makes any numeric type from an `Int`.
- `String::of_bytes` raises on invalid UTF-8.

**Decimal** is a 96-bit integer and a *scale*: the number of digits after the point, from 0 to 28.
`1.5m == 1.50m`, but each prints as written. `Decimal::div` and `Decimal::round` take the scale of
their result and a `Rounding` mode.

**Float** gives the same bits on every machine and build. Each operation is rounded once, as
written, and never fused or reordered. The math functions (`Float::sqrt`, `exp`, `ln`, `sin`,
`pow` and the rest) are correctly rounded. A program sees a single `NaN` value.

### 5.2 Records and tuples

Records are structural: two record types with the same fields are the same type, whatever their
field order. `type` declares an alias, which is just another name for the type.

```ply
fn balance_of(a: Account) -> Int = a.balance
fn point(x: Int, y: Int) -> {x: Int, y: Int} = {x, y}    // `{x}` is short for `{x: x}`
fn divmod(a: Int, b: Int) -> (Int, Int) = (a / b, a % b)
```

A tuple is a record with numbered fields: `(A, B)` is `{_0: A, _1: B}`, read as `t._0`. `(A)` is
just `A`, and `()` is `Unit`. An alias can take parameters: `type Index<a> = Map<a, Int>`.

**New records.** `type T = new { .. }` declares a distinct record type:

```ply
pub type Date = new { year: Int, month: Int, day: Int }

fn epoch() -> Date = { year: 1970, month: 1, day: 1 }
fn next(d: Date) -> Date = { ..d, year: d.year + 1 }
```

A `new` record is read, updated and matched like any record, but it is not the same type as a plain
record with the same fields, or as another `new` record (`E0201`). A record literal becomes a
`Date` where a `Date` is expected: a return type, a parameter, a `let` with a written type, a field
of an expected record, a list element after the first, and so on. Such a literal names every field.
A record bound by a `let` without a type is a plain record. A `new` record may contain itself only
through a sum type (`E0214`).

At run time a `new` record is just its fields, so it prints, encodes and compares like a plain
record.

### 5.3 Sum types

```ply
type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point

type Level = Debug | Info | Warn | Error
type Tree<a> = | Leaf | Node(Tree<a>, a, Tree<a>)
```

The leading `|` is optional. A constructor is a value: `Circle(3)` is a call, and `Point` is a
value. `type Id = Int` (one name, no `|`) is an alias, not a sum.

Sums are nominal: two identical declarations in two modules are different types. A sum may contain
itself only at its own parameters: `type Nest<a> = | Leaf(a) | Deeper(Nest<List<a>>)` would need a
new type at every level, and is `E0477`.

### 5.4 Collections

- `List<a>` is an immutable sequence, written `[a, b, c]` (§6.10).
- `Array<a>` stores its elements next to each other, so reading or replacing one by index is a
  single memory access. Build one with `Array::new` or `Array::of_list`.
- `Columns<a>` is an array of records stored one column per field, so a pass over one field reads
  only that field. Build one with `Columns::new` or `Columns::of_list`. The element type must be a
  record (`E0224`).
- `Map<k, v>` is an immutable sorted map, written `#{k: v}`.
- `Set<a>` (`std.set`) is a sorted set, written `#[a, b]`.

`xs.at(i)` returns `None` for an index past the end and for a negative index, so `xs.at(-1)` is
`None`, not the last element. `xs.set(i, v)` with an index out of range raises.

A map iterates in `compare` order. Inserting a key that `compare` finds equal to one already in the
map keeps the existing key and replaces the value: `#{1.50m: "a"}.insert(1.5m, "b")` is
`#{1.50m: "b"}`.

A map's key type must be ordered: `Float`, functions, `Secret`, `Task` and `Chan` are not
(`E0206`). A signature that takes or returns a `Map<k, v>` requires `k` to be ordered, and the body
may compare two `k`s. The same holds for a record or sum that contains such a map keyed by one of
its type parameters.

### 5.5 Generics

```ply
fn apply<a, b | e>(x: a, f: (a) -> b / e) -> b / e = f(x)
```

Type parameters are lowercase names in `<..>`. Effect variables, which stand for lists of effects
(§7.2), come after `|`; write `<| e>` when there are no type parameters. A name in brackets is a
*label parameter*, which stands for a resource label (§7.2):

```ply
fn relay<[l]>(b: Bytes) -> Unit / {net.send[l]} = net.send[l](b)
```

A call fills a label parameter explicitly, `relay[conn](b)`, or from an argument whose type mentions
the label. A scope's label (§8.2) is always filled from the argument that carries the scope. Leaving
a label unfilled is `E0306`. `<[l, k]>` is short for `<[l], [k]>`, and label parameters come before
the `|`.

A type takes parameters the same way:

```ply
type Seq<a | e> = | Seq(() -> Option<{ value: a, next: Seq<a | e> }> / e)
type Sink<a, [l] | e> = | Sink((a) -> Unit / {net.send[l] | e}) | Quiet
type Pair<a> = new { first: a, second: a }
```

A use fills every parameter (`E0202`). An effects argument is a list such as `{fs.read[src]}` or
`{}`. A declared type may mention only its own parameters. An alias's arguments are substituted into
its body, but a sum's or `new` record's arguments are part of the type, so `Seq<Int | {}>` and
`Seq<Int | {fs.read[src]}>` are different types.

**Constraints.** `where numeric(a)` allows `+`, `-`, `*`, unary `-` and ordering on `a`, and
`numeric_of_int(n)` to make an `a`. `where integer(a)` also allows `/`, `%` and the bit operators.
`where derivable(eq, a)`, `derivable(ord, a)`, `derivable(hash, a)` and `derivable(show, a)`
require `a` to support equality, ordering, hashing or display (§12):

```ply
fn sum<a>(xs: List<a>) -> a where numeric(a) = xs.fold(numeric_of_int(0), |s: a, x: a| s + x)
```

A definition with a `numeric` or `integer` constraint receives its type as a hidden argument, so it
must be called directly: using it as a value is `E0310`. A type may put `derivable` constraints on
its own parameters, which every use of the type must meet (`E0206`).

**Recursion.** A recursive call may fill type, label and effect parameters differently, except that
it may not fill a type parameter with a larger type built from it, as `f<a>` calling `f<List<a>>`
(`E0308`).

**Resource parameters.** Filling a type parameter with a resource (§8.1) is `E0317`, unless the
parameter is written `keep`: `fn keep_one<keep a>(x: keep a)`.

### 5.6 Function types, and what you must write

`(A, B) -> C` is a function that performs nothing and cannot raise. `(A) -> B / {db.read[users]}`
and `(A) -> B / e` have effects. A function value may perform less than its type allows, never
more. Functions cannot be compared, encoded, ordered or used as map keys.

You write every parameter type, the return type and the effects of a top-level `fn`, and the type of
every `forall` binder. Everything else is inferred: lambda parameters, `let` types and so on. A
local `let` is not generic, so `let f = |x| x;` used at two types is `E0201`. Callers are checked
against the signature, never against the body.

The written effects are an upper bound: what the body performs must fit inside them (`E0302`, which
shows the effects to write). An effect the body never performs is the warning `W0614`, which shows
the narrower signature. An `effect set` counts as performed when any of its members is.

**Borrowed function parameters.** A function-typed parameter of a top-level `fn` is *borrowed*: the
function may use it only until the call returns. It may call it, pass it to another borrowed
parameter, or capture it in a lambda that is only called. Returning it or storing it is `E0475`.
Write `keep` to let the function keep it:

```ply
fn twice(f: (Int) -> Int, x: Int) -> Int = f(f(x))
fn later(f: keep (Int) -> Int) -> () -> Int = || f(1)
```

`keep` is part of the signature. A parameter whose type is an alias or a type parameter is not
borrowed, so a borrowed function cannot be passed to it.

**`once` functions.** `once (A) -> B` is a function that may be called at most once and never
copied. It is a resource (§8.1). An ordinary function may be passed where a `once` function is
expected, but not the reverse (`E0201`).

### 5.7 Built-in types

These are in scope everywhere:

```ply
Option<a>     = None | Some(a)
Result<a, e>  = Ok(a) | Err(e)
Ordering      = Less | Equal | Greater
Rounding      = HalfEven | HalfUp | Down | Up | Ceiling | Floor
Iter<s, r>    = Continue(s) | Stop(r)
Instant       = Instant(Int)
Duration      = Duration(Int)
```

`Instant` is a clock reading and `Duration` a span of time, both in nanoseconds. `std.time` works
with them. A module that declares its own `Ok`, `Err`, `Some` or `None` cannot use `?`, and one that
declares its own `Ok` or `Err` cannot use `try` (`E0118`).

**`Secret<a>`** contains a credential, a `String` or `Bytes`, that no Ply code can read.

- `Secret::of_string` and `Secret::of_bytes` seal a value. `config.secret` (`std.config`) returns a
  secret the program never sees in plain form, and `entropy.secret(n)` (`std.random`) draws random
  bytes straight into one.
- `Secret::concat`, `Secret::decode`, `Secret::encode` and the key derivations in `std.hash` make
  new secrets from secrets.
- A program can only compare two secrets (`==`, in time that depends only on their lengths), check
  one against a string (`Secret::verify`), and read its length.
- Hashing passwords (`std.password`), signing and key agreement (`std.crypto`), and MACs
  (`std.hash`) use secrets directly. `std.fs`'s `write_secret` and `std.net`'s `send_secret` write
  one out, and `std.fs`'s `read_secret` reads one back.
- A secret cannot be shown, encoded, ordered, hashed, matched or drawn by a `forall`. Its memory is
  zeroed when it is freed.

A `Scoped` value belongs to its scope (§8.2), and a `Task` or `Chan` to its scheduler (§9); none
can outlive it.

### 5.8 Equality, display, invariants and arithmetic for your own types

By default a value is compared, ordered, hashed and shown by its constructor and fields. The module
that declares a sum can change that:

```ply
pub type Deque<a> = | Deque(List<a>, List<a>)   // a front, and a reversed back

key for Deque by to_list       // used by `==`, `compare`, `digest` and as a `Map` key
show for Deque by written      // used by `show`, `display` and interpolation

pub fn to_list<a>(d: Deque<a>) -> List<a> = ..
fn written<a>(d: Deque<a>) -> String = ..
```

- `key for T by f`: two `T`s are equal when `f` returns equal keys, and order as their keys do,
  wherever a `T` appears. `match` still sees the value as built. Keep `f` cheap, since every
  comparison calls it.
- `show for T by g`: `show`, `display` and interpolation use `g`.
- `invariant for T by v`: every `T` satisfies `v`. `T` must be `opaque` or private (`E0480`). Each
  place its module builds a `T` becomes a claim that `ply prove` must prove (§11.5), and other code
  may rely on it. `v` never runs.
- `numeric for T by { add: add, sub: sub, mul: mul, neg: neg, of_int: of_int }`: `+`, `-`, `*`,
  unary `-` and `numeric_of_int` on a `T` call these functions. With a `key` too, `T` is a numeric
  type: it supports `<` and satisfies `numeric(a)`. `/` and `%` are not available.

Only the declaring module writes these, once each (`E0208`, `E0105`), for a sum type (`E0217`). The
named function is in the same module, performs nothing, cannot raise, and takes no label (`E0218`).
A type's hash covers these functions.

## 6. Expressions

Everything is an expression, including `if`, `match`, `handle` and blocks.

### 6.1 Blocks, `let` and `if`

A block `{ statements; tail }` has the value of its last expression, or `Unit` if there is none.
`let pattern = expression;` binds names, and `let name: Type = expression;` states the type. A later
`let` may reuse a name. A statement needs `;` unless it is a block, `if`, `match`, `handle`,
`with_scope` or `simulate`.

A `let` pattern can take a record apart, which is how a function returns several values:

```ply
fn sum_two(input: Bytes) -> Int = {
  let {value, next} = advance(input, 0);
  let {value: second, ..} = advance(input, next);
  value + second
}
```

A `let` whose pattern does not match raises `abort.raise`. `let pattern = expression else { .. };`
gives another value instead: when the pattern does not match, the `else` block becomes the value of
the enclosing block, and the rest of the block does not run.

```ply
fn first_word(line: String) -> String = {
  let [word, ..] = line.split(" ") else { "" };
  word
}
```

`if a { x } else if b { y } else { z }` needs braces, and every branch has the same type. An `if`
without `else` has type `Unit`.

### 6.2 `match` and patterns

```ply
fn value(src: Bytes, i: Int) -> Result<Json, ParseError> / {abort.raise} =
  match src.at(i) {
    b'"' -> string_value(src, i),
    b'-' | b'0'..=b'9' -> number(src, i),
    b'{' -> object_value(src, i),
    _ -> Err(error_at(i, "expected a value")),
  }
```

Arms are separated by commas and may have a guard: `[x, y, ..rest] if x > y -> ..`. A `match` must
cover every case (`E0205`, which shows a missing case).

| pattern | matches |
| --- | --- |
| `_`, `name` | anything; `name` binds it |
| `Some(p)`, `Point`, `shapes::Circle(r)` | a constructor |
| `42`, `-1`, `1.50m`, `"s"`, `b"s"`, `b'{'`, `true`, `()` | a literal |
| `0..=9`, `b'a'..=b'z'` | an integer between two literals, inclusive |
| `[]`, `[a, b]`, `[a, ..]`, `[a, ..rest]` | a list of an exact length, or with a prefix |
| `{a, b}`, `{a: p, ..}` | a record; `..` allows other fields |
| `(p, q)` | a tuple |
| `p \| q` | either pattern, tried left to right |

Both sides of `p | q` bind the same names at the same types (`E0212`). A `let` can use an
or-pattern only if it cannot fail, as in `let Ok(v) | Err(v) = r;` (`E0213`). For `U8`, `U16` and
`U32`, ranges that cover every value make a `match` complete; other integer types need a final `_`.

### 6.3 Lambdas

```ply
|acc, a: Account| acc + a.balance
|| refresh()
|r: Result<Int, E>| -> Result<Int, E> { Ok(r? + 1) }
```

Type annotations are optional. A parameter without one takes the type the called function expects:
in `accounts.map(|a| a.balance)`, `a` is an `Account`. A lambda captures variables by value, and its
effects become part of the enclosing function's effects. A written return type needs a block body,
and allows `?` inside it.

### 6.4 Calls

Positional arguments fill parameters from the left, and later parameters may be named or left to
their defaults: `greet("ada", greeting: "hey")`. A positional argument after a named one is
`E0124`, an unknown or repeated name `E0123`, and too few arguments `E0202`. There is no partial
application, and a function value or effect operation takes no named arguments.

`value.name(args)` is a call on a value:

- if the value is a record with a field `name`, it calls the function in that field:
  `codec.decode(json)`;
- otherwise it calls `T::name(value, args)`, the function `name` of the value's type `T` (§4.1).
  `s.push(1).size()` is `Stack::size(Stack::push(s, 1))`. The value's type must be known where the
  call is written, and no import is needed.

`value.m::f(args)` is `m::f(value, args)` for any function `f` of a module `m`, so a chain can
include functions that are not the type's own: `text.trim().parse::number()`.

A bare name before the dot is read by what it is: a variable makes a call on its value, and an
effect makes a perform (§7.1). Using a name that is both, at such a call, is `E0105`.

### 6.5 Record update

`{..base, deep: {..base.deep, a: 7}}` copies `base` with the listed fields replaced. The base is a
variable, a field path or one call, and its type must be known where the update is written
(`E0116`). A field the base does not have is `E0117`. Updating a `new` record returns the same
`new` record type.

### 6.6 Loops and recursion

There is no `for`, `while` or `break`. A function that calls itself in tail position runs as a
loop, using no stack. Tail position is the body's value, the last expression of a block, an `if` or
`match` branch, or the right side of `&&` or `||`:

```ply
fn sum_to(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { sum_to(n - 1, acc + n) }
```

Functions of one module that tail-call each other in a cycle, like `even` and `odd`, also run as a
loop, unless the cycle has a `handle` whose clause resumes other than in tail position. Every other
call uses stack, and the stack is one gigabyte; a call past it ends the program (`E0502`). `range`,
`List::map`, `List::filter` and `List::fold` use no stack per element.

`iterate` is a loop with an early exit and a limit on passes:

```ply
fn first_gap(xs: List<Int>) -> Int =
  iterate({i: 0, want: 0}, 1000, |s: {i: Int, want: Int}|
    if s.i >= xs.len() { Stop(s.want) }
    else { Continue({i: s.i + 1, want: s.want + 1}) })
```

The step returns `Continue(next)` or `Stop(result)`. Running past the limit raises `abort.raise`.

A test has 60 seconds by default (§10.4). `ply run` has no time limit unless given `--timeout MS`,
since a server runs until stopped.

### 6.7 `?`

```ply
fn int_value(text: String) -> Option<Int> = {
  let d = Decimal::of_string(text)?;
  Int::of_decimal(d, Down)
}
```

`e?` returns early from the function when `e` is `Err` or `None`, and is the `Ok` or `Some` value
otherwise. It uses the function's written return type, and converts no errors: a `Result<_, E1>`
inside a function returning `Result<_, E2>` is `E0201`.

- `?` must sit where the function's result is decided, with nothing conditional between: `let x =
  e?;` works, but `?` inside an `if` branch or a nested block that is not the result, or after an
  argument with effects, is `E0119`. Bind the value first.
- Inside a `handle` or `try`, `?` leaves the whole function; no clause sees it.
- Inside a lambda, `?` needs the lambda's return type written, and leaves the lambda. Inside a
  `with_scope`, `simulate` or `parallel` branch, it is `E0118`.

### 6.8 `parallel`

```ply
fn both(xs: List<Int>) -> (Int, Int) =
  parallel { xs.fold(0, |a: Int, x: Int| a + x), xs.filter(|x: Int| x > 0).len() }
```

`parallel { a, b, .. }` returns the tuple `(a, b, ..)`, and means the same as evaluating its
branches from left to right. The runtime runs the branches at the same time on its own threads
whenever that cannot change the result. The checker allows a block only when no branch writes what
another reads or writes (`E0309`, §7.2), so a block is as deterministic as its branches.

- If several branches fail, the leftmost failure is the block's.
- Branches may perform operations that a `handle` around the block handles, and still run at the
  same time, when that `handle` has no `var` and each clause reached returns without `resume`,
  captures nothing and performs only through handlers of the same kind. Host handlers are written
  this way. Under any other `handle`, or under a `simulate` block, the branches run one after
  another, with the same result.
- A branch may not spawn a task or open a `simulate` block (`E0309`), and may not use `?`.

`std.parallel` splits a list across nested blocks.

### 6.9 Termination

The checker proves that each function returns. A function that might not must say so with
`diverges` in its effects.

```ply
fn count(xs: List<Int>) -> Int = match xs { [] -> 0, [_, ..rest] -> 1 + count(rest) }

fn down(n: Int) -> Int = if n <= 0 { 0 } else { down(n - 1) }

fn climb(a: Int, b: Int) -> Int
  decreases a + b
  = if a + b <= 0 { 0 } else { climb(a + 1, b - 2) }
```

The checker looks for something that gets smaller on every cycle of recursive calls:

- a part of an argument: a field, a list's element or tail, a map's entry;
- an integer moving toward a bound that a condition on the way to the call keeps it beyond, as in
  `down`, or `if i >= xs.len() { .. } else { walk(xs, i + 1) }`.

It follows calls through `List::map`, `List::filter`, `List::fold` and lambdas. Calls a type's `key`
or `numeric` functions make count too.

When no single argument shows the descent, `decreases` states an `Int` measure over the parameters,
as in `climb`. The checker proves that every recursive call lowers it and that it is never negative
there, or reports `E0467`.

A function whose recursion the checker cannot show to end, or that calls such a function, needs
`diverges` in its effects (`E0302`). Write `diverges` only for a loop that waits on something
outside the program, such as a stream or a network peer; otherwise fix the recursion, use `iterate`,
or add `decreases`. Tests, laws and handler clauses need no `diverges`, since their time limit
bounds them. `ply check --types` explains why each function that may not return might not.

### 6.10 Updates in place

Values never change, but an update reuses memory when nothing else can see the old value.
`xs.push(x)` appends in place when the caller has the only reference to `xs`. When something else
refers to `xs`, it takes the next free slot and shares the items, and it copies the list only if
another push already took that slot. `List::set`, `Array::set` and record updates copy whenever
something else refers to the value, and `Map::insert` copies one path of the map's tree. A rest,
`[x, ..rest]`, shares its list instead of copying it.

An update copies when something else still refers to the value:

- a variable used again after the update;
- a variable captured by a closure that is called again;
- a value read out of a collection with `Map::get`, `List::at` or `Array::get` (use `Map::update`
  instead);
- a caller that keeps using what it passed.

"Used again" follows the path the program takes, so a variable used only in another branch does not
count. Arguments are read before the call they are passed to, so `c.set(i, c.get(i) + 1)` updates in
place. Taking a record apart with a pattern, or reading a field that nothing reads again, frees that
field from the record, so `{ ..s, out: s.out.push(x) }` grows `out` in place.

`ply check --costs` lists each update in your modules that copies, with the cause and a fix. A
`reuse fn` turns a copying update into an error (`E0127`):

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n == 0 { xs } else { collect(xs.push(n), n - 1) }
```

A call to another package is judged by its `returns` clause, never its body. `returns fresh`
promises a result nothing else refers to, and `returns xs` says the result may share the parameter
`xs`. The checker verifies the clause against the body (`E0148`):

```ply
pub fn digits(n: Int) -> List<Int>
  returns fresh
= range(0, n)
```

## 7. Effects

An *effect* is a named set of *operations*, such as reading a database or the clock. Code *performs*
an operation the way it calls a function, and a *handler* decides what the operation does. A
function's *effects*, listed in its signature, are the operations it may perform, so its type shows
everything it can touch.

### 7.1 Declaring an effect

```ply
effect db {
  read  get[r](key: Int) -> Option<Row>
  write put[r](key: Int, value: Row) -> Unit
}

effect toml {
  raise syntax(e: SyntaxError)
}
```

Each operation is a `read`, a `write` or a `raise`. A `read` or `write` returns to where it was
performed. A `raise` never returns, so it has no result type (§7.7).

`[r]` gives an operation a *resource label*: each perform gives one, as in `db.get[users](3)`
(`E0304`). Labels name the resource an operation touches, so the checker can tell that writing
`[orders]` does not conflict with reading `[users]`. Labels are global: two modules that write
`[users]` mean one resource.

`nondet effect` marks an effect whose results come from outside the program, such as a clock or the
network (§10.2). An operation may take type parameters, as in `read take[r]<a>(key: Int) -> a`; a
handler clause for it must then work at every type. Effects are nominal, and the names `task`,
`clock`, `random`, `sim`, `abort`, `diverges` and `scoped` belong to built-in effects (`E0105`).

`db.get[users](3)` performs an operation, and `store::db.put[orders](id, row)` performs one of an
effect from another module.

### 7.2 Effects in a signature

The effects after `/` list what a function may perform: `/ {db.read[users], clock.read}`,
`/ {net.write[conn] | e}`, `/ e`, `/ {}`. An *effect variable*, like `e`, stands for "and whatever
else". Each entry is one of:

- a mode, like `net.write[conn]`, which covers every `write` operation of `net` on `conn`;
- one operation, like `net.send[conn]`;
- a raise, like `toml.syntax`. Raises are always listed one by one, so a signature lists every way
  a call can fail;
- `diverges`, for a call that may not return (§6.9).

An effect of another module is written with `::`: `/ {store::db.read[users]}`.

**Inference.** Performing `net.send[conn](..)` adds `net.send[conn]` to the function's effects. A
call adds the effects the callee's signature lists. The written effects must cover what the body
performs, entry by entry: under `/ {net.send[conn]}`, a body may perform `net.send[conn]` but not
`net.recv[conn]`, and may not call a function written `/ {net.write[conn]}` (`E0302`, which
suggests the effect to add).

**Function values** may perform less than their type allows, never more. Two callbacks given to
one effect variable fill it with the effects of both:

```ply
fn both<| e>(f: () -> Unit / e, g: () -> Unit / e) -> Unit / e = { f(); g() }
```

`both(|| a.x(), || b.y())` performs exactly `{a.x, b.y}`. Inside a parameter's function type, a
`Task`, a `Chan` or a sum's arguments, effects must match exactly, because a function stored there
could later be given something its type does not allow.

**Labels.** A label parameter (§5.5) inside the body hides the global label of the same name.
`[*]` in a handler clause or a signature stands for every label (§7.4). Two effects *conflict* when
they use the same resource of the same effect and at least one is a `write`. A label parameter or
`[*]` conflicts with every label of its effect. `parallel` (§6.8) and the test scheduler (§10.4)
use this rule.

**Counts.** The checker also records whether each effect is performed a `bounded` number of times
or a `scaling` number that grows with the input. An effect scales when it is performed once per
element of a list or map (through `List::map`, `List::fold`, `iterate` and similar), inside
recursion, or by a callee where it scales. `ply check --types` prints the count of each effect:

```ply
fn lookup(id: Int) -> Row / {db.query[conn]} = db.query[conn](id)
fn lookup_all(ids: List<Int>) -> List<Row> / {db.query[conn]} = ids.map(lookup)
```

```
     lookup     : (Int) -> Row
                  / {m.db.query[conn] bounded}
     lookup_all : (List<Int>) -> List<Row>
                  / {m.db.query[conn] scaling}
```

A definition's own signature may promise `bounded`: `/ {db.query[conn] bounded}` (`E0466`
elsewhere). A body that performs that effect once per element is `E0465`, which shows the loop.
The usual fix is one query for the whole input.

### 7.3 Effect sets

```ply
pub effect set Persist = {store.read[db], store.write[db]}
effect set Full        = {Persist, log.write[app]}
```

An effect set is a name for a group of effects, for use in any signature. Another module imports
it by name (`import storage (Persist)`) or writes `storage::Persist`. Sets may include other sets,
but not an effect variable.

### 7.4 Handlers

```ply
test "expiry is decided against the deadline, not the wall clock" {
  handle {
    assert(!expired(1000, 60))
  } with {
    wall_clock.now() -> 1060,
  }
}
```

`handle { body } with { clauses }` runs the body. Each operation the body performs runs the
matching clause, `effect.op[label](params) -> value`, and the clause's value is what the operation
returns. A `return x -> value` clause transforms the body's result.

**Wildcard labels.** A clause written `[*]` handles the operation for every label:
`log.note[*](line) -> ..` handles `log.note[users]` and `log.note[orders]`. `[*t]` also binds the
label the call site used, as a `String` named `t`. A database driver uses this to reject a statement
that touches a table the call site did not name:

```ply
handle { load() } with {
  db.query[*table](sql, params) -> match labelled(table, sql) {
    Some(rows) -> rows,
    None -> panic("this statement uses a table the call site did not label"),
  },
}
```

Clauses are tried in order, so put a clause for one label before a `[*]` clause. A signature may
also use `[*]`, to let a library handle an effect for code it is given: `fn sink<a | e>(body: () ->
a / {log.note[*] | e}) -> a / {wire.put[c] | e}` lets the body perform `log.note` on any label.

**What a handler removes.** A clause for `net.send[conn]` removes that effect from the body's
effects. Clauses for every operation of a mode remove the mode. The `handle`'s effects are the
body's effects, minus what the clauses remove, plus the clauses' own effects. When a body may
perform operations of a mode that the clauses handle only partly, and the enclosing function's
effects do not cover the rest, the `handle` is `E0305`, which offers the missing clauses.

**What a handler sees.** A `handle` handles only the operations its body's effects list. An
operation that comes into the body through an effect variable or a label parameter of the enclosing
function belongs to the caller, and goes to the handlers around the call:

```ply
fn quiet<| e>(f: () -> Int / e) -> Int / e =
  handle { f() } with { log.note(s) -> () }
```

In `quiet(|| { log.note("x"); 1 })`, the `log.note` goes to the handlers around the call of
`quiet`. A function that wants to handle its callback's operations lists them in the callback's
type, as `sink` does above.

### 7.5 `resume` and continuations

In `amb.flip[coin]() resume k -> k(true) * 2`, `k` is the *continuation*: the rest of the body
after the perform, as a function. Calling `k(v)` continues the body with `v` as the operation's
result, and the clause's value becomes the `handle`'s value.

- `k` may be called at most once (`E0316`).
- `k` may outlive the `handle` and be called later. What the resumed body performs then goes to the
  handlers around that later call.
- A clause that binds `k` and never calls it abandons the rest of the body.

`bracket(acquire, release, body)` makes sure a body releases what it acquired. `release` runs on
what `acquire` returned when `body` returns, when a raise or an abandoning clause unwinds it, and
when its task is cancelled (§9.3). A cancel never interrupts an `acquire` or `release`. Nested
brackets release innermost first, and a `release` that fails replaces whatever was unwinding.
`with_scope` (§8.2) is a bracket whose body cannot keep the acquired value.

### 7.6 Handler variables

```ply
effect tally {
  write add(n: Int) -> Unit
}

fn total(xs: List<Int>) -> Int =
  handle {
    xs.fold((), |u: Unit, x: Int| tally.add(x))
  } with {
    var sum = 0,
    tally.add(n) -> {
      sum = sum + n;
    },
    return u -> sum,
  }
```

A handler can keep state in variables, declared with `var name = init` at the start of its `with`
block. The initializers run in order when the `handle` starts, outside the handler. The variables
are visible to the clauses and to `return`, never to the body. A clause assigns one with
`name = value;`. An assignment inside a lambda or a nested `handle`, `try`, `simulate`, `parallel`
or `with_scope` is `E0315`, since that code may run after the clause returns.

A clause with `resume k` continues the body with the variables' values at the time it calls `k`.

Tasks spawned under a handler share its variables (§9). A clause is not interrupted by other tasks
until it performs something that may wait, so keep a read of a variable and the assignment that
depends on it on the same side of every wait. `simulate` tries the orders in which tasks use a
handler's variables, so a test finds a lost update.

### 7.7 Failures

Ply has no exceptions. A failure is a `raise` operation, so a function's signature lists every way
it can fail:

```ply
type SyntaxError = { line: Int, message: String }

effect toml {
  raise syntax(e: SyntaxError)
}

fn digit(b: Int, line: Int) -> Int / {toml.syntax} =
  if b >= 48 && b <= 57 { b - 48 } else { toml.syntax({ line: line, message: "not a digit" }) }

fn digit_or(b: Int, fallback: Int) -> Int =
  handle { digit(b, 1) } with { toml.syntax(e) -> fallback }

fn checked(b: Int) -> Result<Int, SyntaxError> = try { digit(b, 1) }
```

A raise never returns to where it was performed, so it has whatever type is needed there. Raises
pass through effect variables like any operation, so `lines.map(parse_line)` raises what
`parse_line` raises.

A clause for a raise runs after the body is abandoned and its scopes are closed. Its value becomes
the `handle`'s value, and it cannot `resume`. A raise nothing handles ends the program with
`E0502`; in a test, it fails the test.

**`try`.** `try { body }` returns `Ok` of the body's value, or `Err` of what a raise carried: the
value of a raise's single parameter, `()` for none, or a tuple for several. It handles the one raise
the body's effects list, other than `abort.raise`. A body with no raise is `E0311`, and one with
several is `E0312`; write `try[toml.syntax] { body }` to choose one. To turn an `Err` back into a
raise, write `r.unwrap_or_else(|e: SyntaxError| toml.syntax(e))`.

**`abort.raise`.** The prelude declares `effect abort { raise raise(message: String) }`.
`abort.raise(m)` is the raise for a failure on a bad value: `panic`, `assert` and `assert_eq`, a
builtin used outside its domain, division by zero, a `let` that does not match, `iterate` past its
limit, and a few task operations (§9). A clause for `abort.raise` receives the message, and `try`
handles it only when named: `try[abort.raise] { .. }`. Unhandled, a failed assertion is `E0501`
and any other is `E0502`. Overflow and running out of stack end the program and cannot be handled.

**Calls that cannot fail.** A call adds no `abort.raise` when the checker can see that it cannot
fail where it is written:

```ply
fn peek(b: Bytes, i: Int) -> Int = if i < 0 || i >= b.len() { -1 } else { b.at(i) }
```

A builtin that can raise states, as a `requires`, when it cannot: `Bytes::at` requires `0 <= i && i
< Bytes::len(b)`. The checker proves such clauses from the conditions on the way to the call (`if`
conditions, guards, matched patterns, and `List::at` returning `Some`), using linear integer
arithmetic and what builtins guarantee about their results. It looks only at the function being
checked, so a function's effects never depend on another function's body. When it cannot prove a
clause, the call raises, which is not an error.

`ply check --types --explain` lists every place a function can raise `abort.raise`, and why.

### 7.8 Host handlers

A standard library module can give an effect a *host handler*: the handler used when the program
does not handle the effect itself. A host handler is where the program reaches the machine it runs
on.

```ply
pub nondet effect os {
  read pid() -> Int
}

host for os by served

fn served<a | e>(go: () -> a / {os.read | e}) -> a / e =
  handle { go() } with { os.pid() -> machine_pid() }
```

Every entry point the runtime starts (`main`, a test, or an entry given to `ply run`) runs under the
host handler of each effect its signature lists, outside the program's own handlers. So an
`os.pid()` that the program does not handle goes to `served`. Only standard library modules declare
host handlers (`E0161`), and §14 lists the effects that have one.

## 8. Resources and scopes

### 8.1 Resources

A *resource* is a value that must be released exactly once, such as a connection. Ply tracks who
owns each resource, so a program cannot use one after releasing it, or release it twice.

```ply
pub opaque type Conn = | Conn(Int)

release for Conn by close

pub fn open(host: String) -> Conn / {net.write} = ..
pub fn send(c: Conn, data: Bytes) -> Int / {net.write} = ..
pub fn close(c: keep Conn) -> Unit / {net.write} = match c { Conn(fd) -> .. }

fn greet(host: String) -> Int / {net.write} = {
  let c = open(host);
  send(c, b"hello")
}
```

`release for T by f` makes `T` a resource, released by `f`. `T` is an opaque sum of the same
module (`E0225`), and `f` takes one `keep T` parameter and returns `Unit`. A value that contains a
resource is a resource too, as is a `once` function.

- A `let`, a pattern binding or a `keep` parameter *owns* its resource.
- Passing it to a `keep` parameter, returning it, or storing it *moves* it. Using the name
  afterwards is `E0316`.
- Passing it to a parameter without `keep` *borrows* it: the callee may use it, but not move it
  (`E0318`).
- A resource cannot be compared, shown or copied (`E0317`).

A resource still owned when its owner's block ends is released there, in reverse order of binding;
in `greet`, `close(c)` runs after `send` returns. It is also released when a raise, a `?` or an
abandoning clause leaves the block. A resource that nothing owns, such as one created as a
statement or bound to `_`, is `E0322`.

A `match` on a resource takes it apart without releasing it. Only the declaring module can see the
constructors, so only it can do this; that is how `close` ends a `Conn`. A lambda that uses up a
resource it captures can be called at most once, and a handler clause, `simulate` body or `parallel`
branch may borrow resources from outside but not move them, since it may run several times.

### 8.2 Scopes

`with_scope` acquires something, lets a body use it, and releases it when the body ends. The body
is a *scope*, and it reads the acquired value through a *scoped value*:

```ply
fn first_byte(path: String)
  -> Bytes / {fs.open[repo], fs.read_chunk[repo], fs.close[repo], io.fail, io.closed} =
  with_scope[file](io::reading[repo](path), io::close_source) { source -> io::read(source, 1) }
```

`with_scope[r](acquire, release) { s -> body }` runs `body` with `s`, a `Scoped<a, [r]>` for what
`acquire` returned, and calls `release` on that value at the closing `}`. A library exports the two
functions as a pair, like `std.io`'s `reading` and `close_source`. `release` runs exactly once,
however the body ends: by returning, by a raise, or by a cancel (§9.3).

`s.get()` reads the value and adds `scoped.read[r]` to the effects. A function that takes a scoped
value has the scope as a label parameter, filled from the argument: `fn first<[r]>(s: Scoped<Bytes,
[r]>) -> Int / {scoped.read[r]}`.

**A scoped value cannot leave its scope.** `Scoped` may be written only as a parameter type
(`E0446`), so a scoped value can be passed into functions and lambdas, and the checker sees every
way it could leave: the value through its type, and a closure that reads it through its effects.
Each scope needs its own name (`E0330`). A declared type or effect cannot mention `Scoped`, `Task`
or `Chan`; use a type parameter, so the scope travels with the type argument.

The value `Scoped::get` returns is an ordinary value, and a copy may leave the scope. So a
library's functions take the `Scoped` value itself and fail once the scope has released it, as
`std.io` does with `io.closed`. A continuation resumed after its scope ended finds the value
released (`E0331`). A task that can read a scoped value must run under a scheduler opened inside
that scope: write `with_scope[r](..) { s -> simulate { .. } }`, not the reverse.

## 9. Concurrency and simulation

### 9.1 Tasks and channels

Concurrency uses built-in effects:

```ply
nondet effect task   { write spawn<a | e>(body: () -> a / e) -> Task<a> / e
                       write join<a>(t: Task<a>) -> a
                       write yield() -> Unit
                       write cancel<a>(t: Task<a>) -> Bool
                       write await<a>(t: Task<a>) -> Option<a>
                       write channel<a>(capacity: Int) -> Chan<a>
                       write send<a>(c: Chan<a>, x: a) -> Bool
                       write recv<a>(c: Chan<a>) -> Option<a>
                       write close<a>(c: Chan<a>) -> Unit
                       write select<a>(arms: List<(Chan<a>, Option<a>)>, wait: Bool)
                         -> Option<(Int, Option<a>)> }
nondet effect clock  { read  now() -> Instant
                       write sleep(d: Duration) -> Unit }
nondet effect random { write next() -> Int
                       write below(bound: Int) -> Int }
effect sim           { read  seed() -> Int }
```

Outside a `simulate` block, the program's own scheduler runs tasks (§14.2), and `clock` is the
real clock. A task's operations go to the handlers around its `task.spawn`. A task's raise that
nothing in the task handles fails the run.

### 9.2 `simulate`

```ply
simulate {
  let a = task.spawn(|| settle("alice", "bob", 60));
  let b = task.spawn(|| settle("alice", "carol", 60));
  task.join(a);
  task.join(b);
  assert_eq(overdrawn(ledger.balances()), 0)
}
```

`simulate { .. }` runs its body under a scheduler that controls the order tasks run in, and handles
`task`, `clock`, `random` and `sim` itself. Its effects include `sim.read`, which a deterministic
test may have.

- Time is virtual: it moves forward only when no task can run, so `clock.sleep` takes no real time.
- Tasks switch only at `task`, `clock` and `random` operations. The search tries the orders of these
  steps that could change the result. When it covers every possibility, the result holds for every
  interleaving, and is reported as `exhaustive`.
- Two accesses to one handler variable, where one writes, are ordered against each other, as are
  any two allocations. A read and a later write with no scheduler operation between them run as
  one step; put a `task.yield()` between them to let the search interleave there.
- When a task raises and nothing in the task handles it, the simulation cancels every other task,
  waits for them to finish releasing their resources, and then raises at the `simulate`, as if its
  body had raised.
- A `parallel` block inside a simulation runs its branches one after another.

Errors:

- `E0413`: a `Task` or `Chan` leaves the simulation, directly or inside a value or closure, or comes
  in from outside.
- `E0414`: no task can make progress (deadlock), or an interleaving ran past `--sim-steps`.
- `E0416`: a `simulate` inside another.
- `E0425`: an operation that reaches the host, such as a file read, inside the simulation.

### 9.3 Cancellation

`task.cancel(t)` stops `t` where it is. A wait it is in (a sleep, a join, a channel operation or a
host operation) is abandoned, and from then on it only unwinds, running the `release` of each
`bracket` it is in (§7.5). A cancel takes effect only when an `acquire` or `release` returns, so
what was acquired is always released.

`cancel` returns `false` for a task that had already finished or was already cancelled.
`task.await(t)` returns `Some` of the result, or `None` if `t` was cancelled; `task.join` of a
cancelled task raises `abort.raise`. A task cannot cancel itself. A deadline is one task that
sleeps and then cancels another, which a third awaits. The search tries each cancel at every point
where it could take effect.

### 9.4 Channels and `select`

`task.channel(n)` makes a channel that stores up to `n` values no receiver has taken yet. A channel
of capacity `0` is a rendezvous: a send waits for the receive that takes its value.

- A send waits while the channel is full, and returns `true` once its value is queued or received.
  A receive waits while the channel is empty, and returns the oldest value.
- `task.close(c)` ends sending. Later sends return `false`, and receives return what was queued,
  then `None`.
- A race is one channel: each worker sends its result, the first receive wins, and the others are
  cancelled.

`task.select(arms, wait)` performs the first of several channel operations that can proceed. An arm
is a channel with `None`, to receive, or `Some(x)`, to send `x`. It returns `Some((i, got))`, where
`i` is the arm and `got` is `Some` of the value received or sent, or `None` if the channel was
closed. When no arm can proceed, `wait: false` returns `None` at once, and `wait: true` waits for
the first arm that can. Arm order is the only priority, so rotate the arms for fairness, as
`std.chan` does.

### 9.5 Searching interleavings

| flag | meaning |
| --- | --- |
| `--sim dpor` | search the orders of steps that can affect each other (dynamic partial-order reduction); the default |
| `--sim random` | one random interleaving per seed |
| `--sim once` | exactly one interleaving |
| `--seeds N` | seeds per test (default 1 under `dpor`, 64 under `random`) |
| `--seed-range FROM..TO` | the seeds from `FROM` up to, but not including, `TO` |
| `--sim-budget N` | interleavings per seed, under `dpor` |
| `--sim-steps N` | steps per interleaving before `E0414` |
| `--seed 7`, `--seed 7:3.0.2` | replay one interleaving |
| `--measure-reduction` | also run the search without its reductions, and report each count |

A failure prints the racing steps and a command that replays the failing interleaving, such as
`ply test --seed 0:0.1.0.2 --filter 'no account is ever overdrawn'`. A search that runs out of
budget passes but is not cached.

## 10. Testing

### 10.1 Writing tests

```ply
test "expiry is decided against the deadline" {
  assert(!expired(1000, 60));
  assert_eq(parse("12"), Some(12))
}
```

A test is a block of type `Unit` that passes when it finishes. `assert(cond)`, `assert(cond,
Some("why"))` and `assert_eq(actual, expected)` fail with `E0501`; `assert_eq` shows both values and
their first difference. Any other failure is `E0502`. A test is named `<module>.<label>`, and two
tests in one module need different labels (`E0105`).

A test that passes but leaves one of its own objects alive when it ends fails with `E0505`. That is
a memory leak, and a bug in Ply rather than in the program.

**Tests over cases.** A test can run once for each element of a table. Each run is a *case*:

```ply
type Resolution = { reference: String, target: String }

test "resolves {c.reference}" for c: Resolution in resolutions() {
  assert_eq(url_join(base(), c.reference), url_parse(c.target))
}
```

- The table is a `List` of the case type, often built from files with `embed_dir` (§4.6).
- The label is read like an interpolated string, and must give each case a different name.
  Otherwise the whole test fails with `E0105`.
- Each case is selected, cached and reported as a test of its own. Adding a case runs only that
  case, and reordering the table or rewording the label runs none.
- The case type must be hashable without a `key`: no `Float`, function, `Task`, `Chan` or `Secret`
  (`E0206`). The table and label may raise and do nothing else (`E0469`).

**Snapshots.** A test can compare text to a stored file, read with `embed_dir`. The test performs
nothing, so it is cached, and runs again when a stored file changes:

```ply
import std.snapshot
import std.snapshot (Snapshots)

fn stored() -> Snapshots = { dir: "snapshots", files: embed_dir("snapshots") }

test "an order renders as stored" {
  snapshot::check(stored(), "order.txt", snapshot::render(order()))
}
```

`check` fails with a diff from the stored text, and never writes files. To store snapshots, write an
entry point that calls `snapshot::accept`, and run it with the directory granted: `ply run . --fs
snapshots=snapshots` (§14.1).

### 10.2 Deterministic and nondeterministic tests

A test must be deterministic. A test whose effects, after its own handlers, still include a
`nondet` effect is `E0412`: handle the effect, or write `test/nondet "label" { .. }`. A
`test/nondet` is cached like any test, but what it reads from the clock or the network does not
count as an input (§10.3).

A test is not a process, so it has no standard streams: a test that prints is `E0424` unless it
handles the console itself. A test may start the programs `--exec` grants (§14.1).

### 10.3 Which tests run

`ply test` runs a test only when nothing recorded shows it passes. A pass is recorded under the
test's hash, which covers everything the test depends on but not names, comments or formatting. A
pass is also recorded under the compiled code the test ran, so an edit or a new `ply` that compiles
the test to the same code runs nothing.

A pass also records what the test read from outside the program, and stays valid only while that
is unchanged:

- each file and directory it read under a file system root;
- each standard library function, type and effect it used;
- everything read by each `ply` process it started;
- for a test that used the host, the permissions it ran with (§14.1).

What the test's own handlers supply, what it wrote before reading, the clock and the network do not
count as inputs.

| flag | meaning |
| --- | --- |
| `--explain` | why each test was selected, which input changed, and where the time went |
| `--filter TEXT` | run tests whose `<module>.<label>` contains `TEXT`; repeatable. A filter like `std.json.` matches only module names. |
| `--shard K/N` | run part `K` of `N`; each test is always in the same part |
| `--no-cache` | ignore recorded results |
| `--watch` | run again whenever a source file, or a file the last run embedded, changes |

`ply hash` prints definitions' hashes.

### 10.4 How tests run

A test's *outside effects* are the `nondet` effects it performs on the world outside the program.
Tests whose outside effects do not conflict (§7.2) run at the same time, one per core. `--jobs N`
(`-j N`) splits the tests into `N` lanes that each run their tests in turn.

The report comes in three parts: what was selected and how it will run, then each group's results
as the group finishes, then a summary with each failure's diagnosis. Under `--json`, the first two
parts go to standard error as the run goes, and one JSON object goes to standard output at the end.

`--timeout MS` limits each test's wall-clock time (default 60000; `0` for none). A test over the
limit does not fail: it *timed out* (`W0612`), is reported apart from failures, and runs again next
time. A run with a test that timed out is not a success.

### 10.5 When a test fails

A failure lists its *changes*: the definitions the test uses that changed since it last passed,
and the test itself if its text changed. Edited definitions come first, then added or removed
ones, then those that changed only because something they use changed. After the failures, `ply
test` prints each change once, as a diff of the old and new text. A test that never passed has no
earlier version, so its changes are the definitions no passing run has seen.

`--json` gives each failure's diagnostic, outside effects, changes and the command that runs it
again, plus the diffs.

### 10.6 Coverage

`--coverage` reports which tests use each definition, and which definitions no test uses.

## 11. Specifications and proofs

### 11.1 Contracts and laws

```ply
fn adjusted(account: Account, amount: Int) -> Account
  requires amount > -1000000000 && amount < 1000000000
  requires account.balance > -1000000000 && account.balance < 1000000000
  ensures result.name == account.name
  ensures result.balance == account.balance + amount
= {name: account.name, balance: account.balance + amount}

law "a credit and a matching debit leave an account as it was"
  forall (account: Account, amount: Int)
  where amount > -1000000000 && amount < 1000000000 {
    adjusted(adjusted(account, amount), -amount) == account
  }
```

- `requires` and `ensures` clauses go between the signature and the body. `result` is the return
  value. A function's `ensures` must hold for every input its `requires` allow. `requires` is not
  checked at call sites.
- A law has a label, `forall` binders with types, an optional `where` guard, and a body. Its body
  may be a `simulate` block.
- Clauses, guards and law bodies must be pure (`E0417`), but may raise or not return.
- `Int` arithmetic is checked, so bound the inputs with guards, as above.
- Clauses do not change a definition's hash.

`identity(a)` at the start of a `where` limits a law to types whose `==` is identity, such as `Int`,
`String` and records and lists of them: `forall (xs: List<a>, n: Int) where identity(a), n > 0`.

### 11.2 `ply prove`

`ply prove` proves every claim of the root package: each law, each `ensures`, and the claims
`invariant`, `cost` and `space` declarations make. It succeeds only when every claim is `proved`.

| outcome | meaning |
| --- | --- |
| `proved` | holds for every input the guard allows |
| `refuted` (`E0419`) | false at the input shown |
| `vacuous` (`E0420`) | the guard allows no input |
| `unproved` (`E0474`) | neither proved nor refuted; the report says where the prover stopped |
| `internal error` | Ply failed, not the program |

A property that cannot be proved is not one a program may rely on. Check it in a test instead, over
values `std.gen` generates.

| flag | meaning |
| --- | --- |
| `--filter TEXT` | prove claims whose owner's name contains `TEXT`; repeatable |
| `--prove-budget N` | reasoning steps per claim |
| `--timeout MS` | wall-clock time for each run a claim's evaluation makes (default 60000) |
| `--static-prover` | also run the static prover alone on every claim, and report what it decided |
| `--jobs N` | split the work into `N` lanes |

`ply prove` prints each claim's outcome as it is decided, then the definitions no proved claim
covers, then totals. A proof is cached under the claim's hash. Only proofs are cached; any other
outcome is computed again on every run.

### 11.3 How claims are proved

The prover combines:

- evaluating terms whose values are known;
- trying every point of a finite domain of up to 4096 points;
- linear `Int` arithmetic, case splits, and equal terms standing for equal values;
- unfolding definitions, taking the matching `match` arm when a value's shape is known, and running
  a terminating recursive definition on known arguments;
- induction on an `Int` binder (`n <= 0`, then `n` from `n - 1`) or a `List` binder (`[]`, then
  `[h, ..t]` from `t`), checked by a small trusted kernel;
- searching every interleaving of a `simulate` block;
- the contracts of the definitions a claim calls.

**Contracts.** The prover uses a callee's `ensures` instead of its body for a recursive definition
or one from another package, when the callee cannot raise and is known to terminate. A contract in
the same package is used once it is proved. A definition from another package is known only by its
contract, unless it is a `transparent fn`, whose body the prover may unfold.

**Running.** Running proves a claim only when it covers every input: a finite domain of at most
4096 points, or an `Int` binder that the guard bounds between two literals (`where n > 0 && n <=
100` is a hundred points). Over any other domain, the prover also tries a few small values (zero,
the literals in the claim and their neighbors, short lists, each variant). A value where the claim
is false refutes it, but passing values prove nothing.

**Equality.** At `Int`, `Bool`, `String`, `Bytes` and data built from them, `==` is identity. At
`Float` it follows IEEE. At `Decimal` it compares values, so `1.5m == 1.50m`. At a type with a
`key` it compares keys, and the prover treats the comparison as a call it cannot see into.

**Lemmas.** A law with no guard, once proved, helps prove the claims below it in its module and in
every module that imports it. The prover applies it at calls that match the law's first call:

```ply
fn reverse(xs: List<Int>) -> List<Int> =
  match xs { [] -> [], [x, ..rest] -> reverse(rest).push(x) }

law "a push reversed leads" forall (ys: List<Int>, x: Int) {
  match reverse(ys.push(x)) { [h, ..t] -> h == x && t == reverse(ys), [] -> false }
}

law "reverse twice is identity" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }
```

Both are proved, the second by induction on `xs` using the first.

### 11.4 Law schemas

A `law schema` states a law once for any definitions that fit it, and a law fills it in:

```ply
pub law schema round_trip<a, b | e>(encode: (a) -> b / e, decode: (b) -> Option<a> / e)
  forall (x: a) { decode(encode(x)) == Some(x) }

law "a URL round-trips" = round_trip(url_encode, url_decode)
```

A schema has parameters with types and at most one effect variable, which may stand only for raises.
Each use is its own claim, proved and cached on its own. A schema also defines a function of its
parameters and binders, so `round_trip(url_encode, url_decode, "a b")` checks the law at one point.
`std.laws` contains the standard library's schemas.

### 11.5 Invariant claims

For a type with an `invariant` (§5.8), each definition of its module that builds a value of the type
must prove the value satisfies the invariant. These claims are named `<definition> builds <type>`.
The definition's `requires` are not assumed. Any value of the type that a claim receives, from a
binder or a call, is assumed to satisfy the invariant.

### 11.6 Cost and space bounds

A `cost` clause bounds the steps a call takes, up to a constant factor:

```ply
fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost xs.len() * (ys.len() + 1)
= xs.fold(0, |a: Int, x: Int| ys.fold(a, |b: Int, y: Int| b + x * y))
```

The bound is an `Int` expression over the parameters' lengths, positive literals, `+`, `*` and
`ilog2`. It must hold at every size, including zero: with `ys` empty, the steps still grow with
`xs`, so `xs.len() * ys.len()` would be wrong. A function parameter can name the steps one call of
it takes, for use in the bound:

```ply
fn each_of<| e>(xs: List<Int>, f: (Int) -> Int / e cost k) -> Int / e
  cost xs.len() * k
= xs.fold(0, |a: Int, x: Int| a + f(x))
```

A `space` clause bounds the memory a call allocates beyond its arguments, assuming no argument is
shared. `space 0` states that a call runs in place:

```ply
type Chain = | Link(Int, Chain) | End

fn reversed(xs: Chain, onto: Chain) -> Chain
  space 0
= match xs { End -> onto, Link(x, rest) -> reversed(rest, Link(x, onto)) }
```

`ply prove` reads both kinds of bound from the body and never proves them by running. It follows
loops over a list, and over an `Int` moving toward a limit the code checks. A bound it cannot read,
such as one over mutual recursion or a call of a function value without a cost name, is `unproved`.
A bound on a definition that performs effects other than raises is `E0463`, and on one that may not
return, `E0468`. If a bound cannot be read from the code, leave it out.

### 11.7 Review

`ply review` reports, for each definition changed since the last `ply review --accept`, whether its
implementation, its contract and its claims changed.

## 12. Derivation

`derive d for T` generates functions for the type `T` by calling the deriver `d` before the rest of
the module is checked. The generated code becomes part of the module:

```ply
import std.json

pub type Line = { sku: String, qty: Int, unit_price: Decimal }
derive json for Line
// generates line_json : () -> json::JsonCodec<Line>
```

| deriver | module | generates |
| --- | --- | --- |
| `json` | `std.json` | `<type>_json`, a `std.json.JsonCodec<T>` |
| `bin` | `std.bin` | `<type>_bin`, a `std.bin.BinCodec<T>` |
| `sql` | `std.sql` | `<type>_row`, a `std.sql.RowCodec<T>` |
| `map` | `std.derive.map` | `T::map`, which maps each type parameter's values |
| `children` | `std.derive.children` | `<type>_children` and related functions, which list or rebuild the `T`s inside a value |

`<type>` is the type's name in `snake_case`. A `derive` goes in the module that declares its type
(`E0208`). A codec for a type with parameters takes one codec per parameter:

```ply
pub type Box<a> = { label: String, inner: a }
derive json for Box
// box_json : <a>(JsonCodec<a>) -> JsonCodec<Box<a>>
```

Codecs are plain values: `json::decode_bytes(body, order_json())`. A codec for a recursive type
takes a nesting limit. Functions, `Task`, `Chan`, `Secret`, types that take a label, and
`Option<Option<a>>` cannot be derived (`E0206`), and neither can a field whose type has no codec.

`==`, `compare`, `digest` and `show` need no derivation; they work on any value whose type allows
them.

**SQL rows.** `derive sql` maps a record to a table row, one column per field, in field order. A
`String`, `Int`, `Bool`, `Float`, `Decimal` or `Bytes` field is a column of that type, an `Option`
is a nullable column, and a `List` of those is an array. Any other field is stored as JSON with its
type's own `json` codec.

**Writing a deriver.** A deriver is a module's `pub fn derive(declaration: derive::Declaration) ->
Result<String, derive::Refused>` that may only raise. `std.derive` describes a declaration. The
checker runs a deriver once per declaration, with a 60-second limit, and caches the result. A
deriver that fails, or generates code that does not check, is `E0155`.

## 13. Builtins and the standard library

### 13.1 Builtins

Builtins are the functions every module can call without an import. `std.prelude` declares them,
and every module imports it automatically. `ply doc std.prelude` lists them all.

- Most are functions of a built-in type: `xs.len()`, `Map::new()`, `Bytes::at(b, i)`. `ply doc List`
  lists a type's functions.
- The rest have plain names, such as `range`, `min`, `panic`, `iterate`, `bracket` and `to_int`.
- An `extern fn` is implemented by the runtime. Some, like `List::map` and most of `Map`, are
  written in Ply in the prelude.

A builtin that can raise states, as a `requires`, when it cannot, and states what is true of its
result as an `ensures`:

```
   Bytes::at(b: Bytes, i: Int) -> Int / {abort.raise}
      requires 0 <= i && i < Bytes::len(b)
      ensures 0 <= result && result <= 255
```

A call whose `requires` the checker can prove adds no `abort.raise` (§7.7).

Only standard library modules may declare an `extern fn` or `extern type` (`E0151`), or import a
module under `std.internal` (`E0159`).

### 13.2 The standard library

The standard library is the built-in package `std`. It ships inside `ply`, and every program can
import its modules: `import std.json`. Its tests and laws belong to it, not to your project.

A module named after a built-in type is that type's *home*: `std.list`, `std.string`, `std.map`,
`std.option`, `std.result`, `std.char`, `std.bytes` and `std.float`. Only it and the prelude
declare the type's functions, which any module can call without an import (`xs.sort()`). A few
other names need no import: `print`, `eprint`, `read_line` and `Console` from `std.console`, `Set`
from `std.set`, and `abs` from `std.math`.

Each module is documented in its source:

- `ply std` lists the modules with a summary of each.
- `ply doc std.json` documents a module, `ply doc std.json.parse` one definition, and `ply doc
  fs.read_at` one operation of an effect.
- `ply std --show std.json` prints a module's source.

A program loads only the standard library modules it imports, and checks only the definitions it
uses, so a change to anything else in the library does not affect it.

## 14. Running programs

The *host* is the machine a program runs on. Effects that reach outside the program, such as files,
the network, processes and the console, are handled by *host handlers* (§7.8), standard library
functions that call into the runtime. `ply run`, `ply test` and `ply hosts` set up the same host
handlers.

### 14.1 Permissions

Some access is open to every program:

- **Environment:** `std.env` reads the process's variables and working directory.
- **Configuration:** `config.get` and `config.secret` (§14.3).
- **Network:** a program can look up names, connect and listen. Every listener binds only
  `127.0.0.1`.
- **Console:** `std.term` uses the terminal, and the run restores the terminal mode it found.

Everything else must be granted with a flag:

| flag | grants |
| --- | --- |
| `--fs NAME=PATH` | a file system root: `std.fs` files, `std.sqlite` databases and Unix sockets under the label `NAME` live below `PATH`. Repeatable. `E0454` if it is not a directory. |
| `--exec NAME=PATH` | a program that `process.spawn` or `process.start` with the label `NAME` may start. Repeatable. `E0457` if it cannot run. |
| `--allow NAME` | a privileged family of operations, which the program must declare (`E0459`). `machine` (`std.machine`) runs another compiled program. `shipped` reads the standard library `ply` carries. `toolchain` compiles and links C. |

Using a label that no `--fs` gives a root is `E0451`, and a path that leaves its root is `E0452`.

### 14.2 Tasks and time

Outside `simulate`, the program's own scheduler runs tasks. The entry point runs as the first task,
and the program ends once every task it spawned has ended. Ready tasks run in the order they became
ready. A task that sleeps or waits on the host waits while the others run.

`clock.now` reads a monotonic clock, in nanoseconds since the run began, and `clock.sleep` waits on
it. A cancel stops a sleep, so a deadline one task sets for another fires while the rest of the
program works. `std.time`'s `time.sleep_ms` blocks the whole thread instead.

### 14.3 Host handlers and tests

These effects have host handlers (§7.8), which run whatever the flags: `std.os`, `std.env`,
`std.certgen`, `std.config`, `std.password`'s `kdf`, `std.dns`, `std.net`, `std.udp`,
`std.sqlite`, `std.term`, `std.signal`, `std.trace`, `std.time`, `std.random`'s `entropy`,
`std.process` and `std.fs`. A host handler that would block the program's thread, as hashing does,
passes the work to the runtime's own threads, and the task that called it waits while other tasks
run.

A test that uses the host is cached with what it read and the permissions it ran with, so its pass
is never reused under other permissions. An operation inside a `simulate` block never reaches the
host (`E0425`). `std.process` (and `std.console`, built on it) is the run's own process under `ply
run`, but a test is not a process (`E0424`), except that it may start the programs `--exec` grants.

### 14.4 Configuration

`config.get[l](key)` and `config.secret[l](key)` read configuration from, highest first: `--set
KEY=VALUE`, `--config FILE` (lines of `KEY=VALUE`), the environment, and the schema's defaults.
`config` is `nondet`, so a test that reads it is a `test/nondet`.

`--config-schema MODULE.FN` names a function returning a `ConfigSpec`. A missing required key is
then `E0441`, a value of the wrong shape `E0442`, and an undeclared key the warning `W0607`. An
unreadable source is `E0440`.

### 14.5 TLS

| flag | meaning |
| --- | --- |
| `--tls NAME=CERT,KEY` | a TLS credential: a PEM certificate chain, leaf first, and its key. Repeatable. A listener uses it as `net.listen_tls[l](port, "NAME")` or through `net.serve_tls`, which `std.http`'s `serve_pooled_tls` uses to offer `h2` and `http/1.1`. Several certificates under one name are chosen by the name the client asks for (SNI). Changed files are reloaded within a second. |
| `--trust CERT.pem` | an extra trusted certificate, for `net.connect_tls` and `net.start_tls`. With any `--trust`, listeners also ask clients for a certificate, which `net.peer_certificate` returns. Repeatable. |
| `--mtls NAME` | listeners using the credential `NAME` require a client certificate that `--trust` verifies. Repeatable. |

A credential that does not load is `E0430`, and naming one that was not given is `E0429`.

### 14.6 Stopping and draining

`ply run` listens for `SIGINT` and `SIGTERM` when the program uses `std.signal` or accepts
connections. On a stop signal, listeners close, so a waiting `net.accept` returns `0`:

- `--drain-lead-ms MS` keeps accepting connections for this long first (default 0);
- `--drain-ms MS` then lets requests in flight finish (default 30000). Running out is `W0608`, and
  exit status 3.

`std.http`'s `serve_pooled` drains this way: it accepts nothing more, finishes the connections it
had accepted with `Connection: close` on the last response, and closes idle ones. An HTTP/2
connection gets `GOAWAY`. Under load, it responds with `503` and `Retry-After` to a connection that
finds its queue full.

A program that starts child processes and drains nothing ends its children and exits at once on a
stop signal, with status 130 or 143. Ctrl-C ends any other program at once.

### 14.7 Tracing

`std.trace` writes spans to the sink `--trace json|text|off` chooses (default `json`, as lines on
standard error), at or above `--trace-level debug|info|warn|error` (default `info`). A span left
open is `E0445`.

### 14.8 Databases

A SQLite database is opened under a file system root (§14.1). `std.db` fails a statement the
server rejects, and one that uses a table its call site did not name in its label.

**SQL checks.** Before a program runs, `ply` checks each SQL statement in the root package against
the schema (§4.5) its module uses: each `db.query`, `db.execute` and similar operation, `std.db`'s
`fetch`, and `std.sqlite`'s `query`, `execute` and `run`. A statement is checked when it can be
computed before the program runs: a string, strings joined with `++`, or a call of a function on
such values. Errors are reported at the call:

| code | meaning |
| --- | --- |
| `E0431` | the statement does not parse |
| `E0433` | a table or column the schema does not declare |
| `E0435` | the statement does not use the table the call's label names |
| `E0436` | the wrong operation: a write run as a query, or a read run as `execute` |
| `E0437` | the columns a `fetch` returns do not match the row type's fields (§12) |
| `E0438` | the parameters do not match the placeholders, or a parameter's type does not match its column |
| `E0447` | a `*` |
| `E0470` | a construct the check does not follow, such as `with` or a subquery in `from` |

`std.db` applies the same rules at run time to a statement the check could not compute. A module
whose statements use two schemas is `E0471`.

### 14.9 `ply hosts`

`ply hosts` lists every operation the program can perform on the host, as `effect.op[label]`. For
each it shows the handler, whether the operation is deterministic, whether it may run more than
once, whether it blocks, and whether it may receive a `Secret`. It also shows the run's file system,
TLS, database, configuration, tracing and shutdown settings. `--digest` prints one `b3:` line for
the whole list.

## 15. Packages

### 15.1 The manifest

A package's root contains `ply.pkg`, its manifest. It contains one function whose body is a value of
`std.pkg`'s `Manifest` (`ply doc std.pkg.Manifest`):

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

The body is data, not code: literals, constructors, records and lists (`E0130`). A manifest of
another shape is `E0129`, and an invalid field `E0131`. A project with no `ply.pkg` is the
*anonymous package*.

### 15.2 Dependencies

Each dependency gives access to its package's module prefix:

```ply
import store.orders        // a module of this package
import cli.surface         // a module of the dependency `cli`
```

- `Path("../cli")` is a package in another directory (`E0135` if it is missing).
- `Git(url, rev)` is fetched with the `git` on your `PATH` into `.ply-cache/git/`. `rev` is a
  commit, a tag or a branch. A fetched copy is reused without contacting the remote, so clear the
  cache to follow a branch that moved.
- `Registry` is a package in the registry `PLY_REGISTRY` names (§16.3), at version `min` or later.

`ply resolve` is the only command that contacts a registry. It downloads each registry dependency
into `.ply-cache/registry/`, after checking its digest against the index and the lock (`E0142`).
Every other command reads the downloaded copy, and fails with `E0135` or `E0136` when a dependency
is missing or too old; `ply resolve` fixes either.

Importing an undeclared package is `E0132`. Two packages with one prefix, or a module of the root
package using a dependency's prefix, is `E0133`. Packages that depend on each other in a cycle are
`E0134`.

A command works on the root package. A dependency's `main` is never an entry point, and `ply test`
and `ply prove` cover only the root package. `--workspace` (on `check`, `test` and `prove`) also
runs the command on every package the root depends on through path dependencies, dependencies
first.

### 15.3 Versions and the lock file

`ply` uses minimal version selection. Each package states a minimum version of each dependency, and
the highest minimum wins. One version of each package serves the whole program (`E0137` names a
package found at two different places). Over a registry, a minimum selects the lowest published
version at or above it that is not yanked.

`ply build` records the resolved versions in `ply.lock`, next to `ply.pkg`, with a BLAKE3 digest of
each dependency's sources. A path dependency inside the same git checkout has no digest, since the
commit already pins it. Before writing anything, a build checks the lock: a dependency whose sources
no longer match is `E0138`. To accept a changed dependency, run `ply resolve`.

### 15.4 Vendoring

`ply vendor` copies every dependency into `vendor/`, with an index, so the project builds with no
cache, network or `git`. `ply why NAME` shows the chain of dependencies that brought in a package,
with its version and digest.

### 15.5 Workspaces

A repository with several packages lists them in `ply.work` at its root: one function returning a
`std.pkg.Workspace` (`ply doc std.pkg.Workspace`), with:

- `packages`: the packages the repository maintains;
- `suites`: packages of tests or examples, each saying whether `ply prove` runs on it;
- `fixtures`: directories of Ply sources that belong to no package;
- `toolchain`: only in the repository that builds `ply`.

`ply work` prints where each package is, one role and path per line. Inside the repository that
builds `ply`, commands read the standard library and the compiler from the checkout instead of from
the running `ply`.

## 16. Building, publishing and signing

### 16.1 Programs

```
$ ply build . --entry app.serve -o app.plyx
$ ply build . --digest              # print `b3:...`; write no file
$ ply build . --diff old.plyx       # what a rebuild would change
$ ply run app.plyx
```

`ply build` writes everything an entry point (default `main`) needs into a `.plyx` file: the
definitions it uses, by hash, as source without tests, laws or comments, and their compiled code.
Running a `.plyx` checks nothing. Its BLAKE3 digest covers all of this, so an edit the program
does not use leaves the digest unchanged. A `.plyx` carries the standard library definitions it
uses, so another `ply` runs it as built. A built program reports failures without line numbers.

A `.plyx` whose parts do not match is `E0443`. One built by another compiler, or for another
runtime, is `E0444`: rebuild it.

### 16.2 Libraries

A package whose manifest names no entry, and that declares no `main`, is a library. `ply build`
writes it as a `.plyz` file: every module's source, the manifest, and compiled code. `ply run`
rejects a library (`E0443`).

### 16.3 The registry

A registry is a directory of files behind an HTTP server:

```
GET  /<name>/index.json                        every version of <name>, newest last
GET  /<name>/<version>/package.plyz            the library, as `ply build` writes it
GET  /<name>/<version>/package.plyz.b3         its digest, one `b3:<hex>` line
GET  /<name>/<version>/interface/<semantics>   the interface its publisher produced
GET  /<name>/<version>/attestation/<semantics> what the registry's attester found
PUT  /<name>/<version>                         publish: the `.plyz` as the body
PUT  /<name>/<version>/interface/<semantics>   the interface
PUT  /<name>/<version>/attestation/<semantics> a signed attestation
POST /<name>/<version>/yank                    mark the version yanked
```

`index.json` lists each version with its digest, whether it is yanked, and the toolchain its
manifest declares. `ply resolve` checks every archive against the index and the lock, so a stale or
hostile index can cause an error but never a wrong build.

`PLY_REGISTRY` is the registry's base URL, `https://host[:port][/prefix]`. The client checks the
server against the built-in roots and the certificates in `PLY_TRUST`. `http://` is accepted only
for a registry on the same machine, since a publish sends a token. An unset or unreachable registry
is `E0141`, and a name or version it does not have is `E0143`.

### 16.4 Publishing

`ply publish` builds the library's `.plyz` and sends it to `PLY_REGISTRY`, with the token in
`PLY_REGISTRY_TOKEN`. Only a library whose dependencies all come from the registry can be published
(`E0145`). The registry rejects different bytes under a version it already has (`E0144`): a
published version never changes.

**Semantic versioning is checked.** `ply publish` compares the contract of each public definition
(its signature and clauses, and its body if it is `transparent`) with the highest published version
below it. A patch release may change no contract, a minor release may only add definitions, and a
major release may change anything. A version that bumps too little is `E0150`, which names the
change and the smallest correct version. `ply contracts NAME FROM TO` lists the contracts that
changed between two published versions.

**Interfaces.** After the archive, `ply publish` sends the package's *interface*: each module's
public names with their signatures and hashes, under the *semantics version* of the `ply` that
produced it. The semantics version changes only when the meaning of hashes, effects or claim
outcomes changes. `--verify-dependencies` (on `check`, `test` and `prove`) rejects a dependency
whose interface does not match its source (`E0149`).

**Attestations.** `ply attest NAME VERSION` downloads a published version, then checks, tests and
proves it, and reports the result. With `--sign KEY`, it signs the result and sends it to the
registry, which keeps it only if its configured attester signed it. `ply resolve` trusts an
attestation only when a key in `PLY_ATTESTERS` signed it.

**Yanking.** `ply yank NAME VERSION` marks a version yanked. New resolutions skip it, but a lock
that pins it keeps it. Nothing is ever deleted.

**Running a registry.** The registry is a Ply program, `packages/registry`:

```
$ ply run packages/registry --fs store=/srv/ply \
    --tls registry=cert.pem,key.pem --set tls=registry --set port=8443 \
    --config tokens.conf
```

`store` is the directory of files, `port` the port (default `8080`), and `tls` the credential to
serve with; without it, the registry serves plain HTTP. Each package's publishing token is the
configuration key `token.<name>`. The registry locks a package around every write, so two
registries can share one store. With `--set attester=<public key>`, `--set attest.key=<key file>`,
`--set url=<its URL>` and `--exec attest=<a ply>`, it attests every published version.

### 16.5 Signing

```
$ ply keygen release.key                       # release.key and release.key.pub
$ ply build . -o app.plyx --sign release.key   # app.plyx and app.plyx.sig
$ ply run app.plyx --require-signer release.key.pub
$ ply build . -o app.plyx --verify             # compare, write nothing
```

`ply keygen PATH` writes an Ed25519 key pair: the secret key at `PATH`, readable only by its owner,
and the public key at `PATH.pub`. It never overwrites a file. The secret key is a `Secret` (§5.7)
from the moment it is generated.

`ply build --sign KEY` writes a detached signature next to what it builds. What is signed is the
artifact's kind, name and digest, the `ply` that built it, its semantics version, and the git
commit of its sources. `ply run --require-signer KEY` runs an artifact only if one of the given keys
signed it (`E0460`). `ply build --verify` builds in memory, checks the file `-o` names against that
build, and checks each signature (`E0461`).

## 17. The `ply` command

`ply [--color auto|always|never] <command> [path] [options]`

### 17.1 Output and exit status

**JSON.** Every command takes `--json`, and then prints exactly one JSON object on standard output.
Progress lines, such as each test result, go to standard error, so a run that is stopped still
shows what it finished.

**Color.** `--color auto` (the default) uses color when `FORCE_COLOR` is set; otherwise not when
`NO_COLOR` is set; otherwise when `CLICOLOR_FORCE` is set; otherwise on a terminal whose `TERM` is
not `dumb`.

| exit status | meaning |
| --- | --- |
| 0 | success |
| 1 | a test failed, or `main` raised |
| 2 | the program did not run: a bad path, a syntax error or a type error |
| 3 | the drain deadline passed with requests in flight (§14.6) |
| *n* | `process.exit[p](n)` under `ply run`: the program's own status, `0` to `125` |
| 130, 143 | `SIGINT` or `SIGTERM` ended a `ply run` |
| 141 | the reader of the output went away, as when output is piped into `head`, and the program left `pipe.broken` unhandled |

### 17.2 Environment variables

| variable | meaning |
| --- | --- |
| `PLY_CACHE_UPSTREAM`, `PLY_CACHE_UPSTREAM_READONLY` | a shared second cache (§1.5) |
| `PLY_REGISTRY`, `PLY_REGISTRY_TOKEN` | the package registry and its publishing token (§16) |
| `PLY_ATTESTERS` | public key files whose attestations `ply resolve` trusts, separated as in `PATH` |
| `PLY_TRUST` | PEM files, separated by `:`, whose certificates `ply`'s own HTTPS connections also trust |
| `PLY_CC` | the C compiler to run instead of `cc` |
| `PLY_C_CACHE` | where compiled code is cached (default `ply/cache` in the user's cache directory) |
| `PLY_C_CACHE_MAX` | a size limit for that cache, in bytes; the least recently used entries go first |
| `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`, `TERM` | color (§17.1) |

### 17.3 Commands

`[path]` is a `.ply` file or a directory, and defaults to the package that contains the working
directory. *Host flags* are those in §14 (`--fs`, `--exec`, `--allow`, `--tls`, `--trust`,
`--mtls`, `--set`, `--config`, `--config-schema`); *simulation flags* are those in §9.5.

| command | options |
| --- | --- |
| `ply new PATH` | `--name NAME`, `--lib` |
| `ply check [path]` | `--types` (each definition's type and effects), `--costs` (copying updates, §6.10), `--explain` (with `--types`, also why a function may raise or not return), `--workspace`, `--verify-dependencies` |
| `ply test [path]` | `--filter`, `--shard`, `--jobs`/`-j`, `--timeout`, `--no-cache`, `--explain`, `--watch`, `--coverage`, `--profile`, `--workspace`, `--verify-dependencies`, host and simulation flags |
| `ply run [path] [-- ARGS]` | `--seed`, `--timeout`, `--profile`, `--explain`, `--require-signer KEY`, host, trace and drain flags. `ARGS` is what `process.args` returns. A `.plyx` path runs a built program. |
| `ply prove [path]` | `--filter`, `--jobs`, `--prove-budget`, `--timeout`, `--no-cache`, `--no-incremental`, `--explain`, `--static-prover`, `--workspace`, `--verify-dependencies`, simulation flags |
| `ply review [path]` | `--changed` (default), `--accept`, `--no-cache`, `--no-incremental`, `--prove-budget`, `--timeout`, simulation flags |
| `ply build [path]` | `--entry NAME`, `-o FILE`, `--config-schema`, `--digest`, `--diff OLD.plyx`, `--sign KEY`, `--verify` |
| `ply hosts [path]` | host, trace and drain flags, `--digest` |
| `ply doc NAME [path]` | a definition's signature, doc, clauses, location, hash and the laws that use it; a type's fields or variants and functions; an effect's operations; a module's exports |
| `ply std` | the standard library's modules. `--show [NAME]` prints a module's source; `--digest`. `--answers DIR` and `--answering` are for the bootstrap, which runs them on the `ply` it builds. |
| `ply explain CODE` | what a diagnostic code means; `--all` lists every code |
| `ply fmt [paths]` | formats files in place; `--check` changes nothing and exits 1 if a file would change; `--json` needs `--check` |
| `ply rename --map FILE [paths]` | renames definitions, variables, fields, operations and modules (§17.6) |
| `ply show NAME [path]` | prints one `fn` or `type` as its file has it |
| `ply replace NAME [path]` | replaces one `fn` or `type` with `--with FILE` or standard input (§17.7) |
| `ply hash [path]` | each definition's hash; `--dependencies` adds what each refers to |
| `ply definitions [path]` | every definition: location, hash, signature, effects and references; `--filter` |
| `ply callers DEF [path]` | what refers to a definition, and every definition, test and law that depends on it |
| `ply unused [path]` | `--workspace`, `--json`: definitions nothing uses, and those only tests and laws use; in a package with `main`, `pub` does not count as a use, and a leading `_` opts a name out. Under `--workspace`, every package `ply.work` names is judged together, except the standard library |
| `ply resolve [path]` | writes `ply.lock`, downloading registry dependencies |
| `ply vendor [path]` | copies dependencies into `vendor/` |
| `ply why NAME [path]` | why a package is a dependency |
| `ply publish [path]` | publishes a library (§16.4) |
| `ply yank NAME VERSION` | marks a published version yanked |
| `ply attest NAME VERSION` | checks, tests and proves a published version; `--sign KEY` |
| `ply contracts NAME FROM TO` | the contracts that changed between two published versions |
| `ply keygen PATH` | writes an Ed25519 key pair |
| `ply cache clear\|stats\|compact [path]` | empties the cache, reports its size, or reclaims unused space |
| `ply cache inspect DEF [path]` | what the cache stores for a definition or test |
| `ply work [path]` | the packages `ply.work` lists |

### 17.4 Reusing results

`ply check`, `ply test`, `ply run` and `ply definitions` remember their last result for the same
command line. While nothing that result depended on has changed, they return it without loading the
program. Otherwise they check again only what depends on a change. `--explain` reports whether a
result was reused. `ply build`, `ply hosts` and `--no-cache` always check from scratch.

### 17.5 Compiling

Every command compiles the program to C and runs the compiled code. The first run after `ply` or
the program changes compiles, which takes a few seconds; later runs reuse the compiled code.
`--profile development` (the default for `ply test` and `ply run`) compiles quickly, with `tcc` on
x86-64 Linux and `cc -O0` elsewhere. `--profile release` uses `cc -O2`.

Every program that passes the checker compiles. A definition Ply fails to compile is a bug in
Ply, reported as `E0505`.

### 17.6 Formatting and renaming

**Formatting.** `ply fmt` rewrites files in the standard layout. It keeps comments, the way each
literal is written, and the order of imports, items and statements. A file that does not parse is
left alone, with an error and exit status 2. It skips directories whose names start with `.` and
directories named `target`.

**Renaming.** `ply rename --map FILE` renames names across many files. Each line of the map is the
old name, a space, and the new name:

```
held_count cached_count
Expr Expression
```

It renames each definition, local variable, effect operation, record field and module with the old
name, where it is declared and wherever it is used. A record field is renamed in every record type
that has it. Renaming a type also renames the functions its `derive` lines generate, such as
`box_bin` for `Box`, wherever they are used.

A line can be narrower:

| line | renames |
| --- | --- |
| `packages/cli/build.ply Made BuildOutput` | only what that file declares under the name, and its uses |
| `packages/std/fs.ply fs.read_file load_file` | one operation of an effect the file declares |
| `packages/shop/order.ply Order.held quantity` | one field of a record type the file declares |
| `.held cached` | only the record field |

Records are structural, so `ply rename` tells records apart by their field names. A record type's
field is renamed in every record with the same field names: literals, reads, updates, patterns and
written record types. When two declared types have the same field names, the map must rename the
field in both. A `new` record type's field is renamed only in that type.
| `packages/app/old_name.ply new_name` | a module: the file moves, and imports follow |
| `packages/compiler/emit/old/ new` | a directory of modules |

`--words` renames words inside names instead: `ty type` changes `ty_vars` to `type_vars` and `TyCon`
to `TypeCon`. A name inside backticks in a comment is renamed too. A string whose whole text is the
qualified name of a renamed definition or of a definition in a moved module, such as
`"app.store.total"`, is renamed with it. Other text in strings is never changed; a string that is
exactly a renamed name, or that contains a word `--words` renames, is reported as a warning.

`ply rename` changes nothing, and exits 2, if the program does not check first, or if the rename
would change what any name refers to: two definitions with one name, a local hidden by another,
a call on a value that could call a different function, or a definition a `derive` writes that
names something the renames remove. Because of these checks, the renamed
program has the same types, so `ply rename` does not check it again. It renames only the files
under its paths, and stops if another file of the package uses a renamed name. `--check` reports
what would change without writing.

### 17.7 Editing one definition

`ply show NAME` prints one `fn` or `type` with its doc and the comments above it, and `--json` adds
its byte range. `ply replace NAME --with FILE` replaces exactly that range with a formatted
definition of the same kind and name, leaving the rest of the file alone. It writes nothing unless
the program still checks and no other definition's name or hash changes (`E0128`).

## 18. Diagnostics

Every command prints diagnostics in one format:

```
Error[E0201]: type mismatch: function body type
  --> app.ply:1:17
   | fn f() -> Int = "x"
   |                 ^^^ expected `Int`, found `String`
```

The first line has the severity, the code and the message. Then each label shows where it points,
the source line, and marks under the span. A note follows as a `  = ` line. Color never changes the
text.

`E` codes are errors, and so is `X0001` (a character that cannot appear in source). `W` codes are
warnings, which never mean the program is wrong. `ply explain CODE` describes a code, and `ply
explain --all` lists them all.

Under `--json`, a diagnostic with a known fix includes `fixes`. Each fix has a `title` and `edits`,
and each edit replaces the text from `start` to `end` of `file` with `text`. On a terminal, a fix is
shown as a `= fix:` line. A runtime diagnostic, such as a failed assertion, cuts a long value short
with `… N more`; under `--json` the whole value is in `values`.

## 19. Limits and sharp edges

What Ply does not have:

- No loops, `break` or `return`: use recursion or `iterate` (§6.6).
- No mutable variables, except handler variables (§7.6).
- No exceptions: every failure is a raise, listed in the function's effects (§7.7).
- No type classes, no implicit arguments, and no dispatch at run time: a call on a value is chosen
  by its type where it is written (§6.4).
- No modules as values, and no first-class effects.
- No `unsafe` and no foreign function interface: everything a program does outside itself is an
  effect, and only the runtime implements builtins.
- No collection of reference cycles.
- A task never moves between threads; only `parallel` branches run on other threads.
- No file handles: `std.fs` reads a range and appends by path.
- No backpressure, database migrations or authentication framework.
- No HTTP/2 over `listen_tls`, no `Upgrade: h2c`, and no server push.
- An expression may nest at most 128 levels deep.

Sharp edges:

- `x.f(y)` where `x` is not a variable is an effect operation, not a function call (§6.4).
- An operation nothing handles is found only when it reaches the host at run time (`E0424`), unless
  its effect is `nondet` and it is in a deterministic test (`E0412`).
- A record update needs the base's type known where it is written (§6.5).
- `Bytes::at`, `String::slice`, `String::find`, `List::set`, `Array::get` and `Array::set` raise
  when out of range, but `List::at` and `Array::at` return `None`.
- Two tasks that allocate are always ordered against each other in a simulation (§9.2).

## 20. Examples

The `examples/` directory has complete programs:

- `clock.ply`: a `nondet` effect, a handler, and `test/nondet`.
- `ledger.ply` and `report.ply`: modules, an `opaque` type, contracts and laws.
- `pipeline.ply`, `bank.ply` and `timeout.ply`: simulation, a race and its fix, and a virtual clock.
- `echo.ply` and `hello.ply`: sockets and an HTTP endpoint.
- `orders.ply`: `derive json`.
- `relay.ply`: one forwarder that is generic over the label it writes under.
- `shout.ply`: a command line declared with `std.cli`.
- `store.ply`: a handler that grants a capability.
- `agreement.ply` and `fake_divergence_audit.ply`: `std.db`'s in-memory fake checked against
  recorded PostgreSQL results.
- `service.ply`: a service over PostgreSQL or its in-memory fake, with configuration, TLS, tracing
  and graceful shutdown.
