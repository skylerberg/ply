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
creates a library instead: a `lib.ply` with one `pub` function and no `main`. The package's name is
the last part of the path given to `ply new`. A name is one or more parts joined by dots, and each
part contains only lowercase letters, digits and `_`. `--name` sets the package's name to the value
you give it, instead of taking it from the path.

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
| `ply prove` | checks that the program meets the specifications and laws it states about its own behavior (§11) |

`ply check --types` prints the type and effects of every definition. §17 lists every command
and flag.

### 1.5 The cache

`ply` stores results in `.ply-cache/` at the project root: checked modules, test results, proofs,
saved baselines for `ply review` and fetched dependencies. Add it to `.gitignore`. Deleting it is
always safe, and `ply cache clear` empties it. A run that crashes, or that races another run, can
make a later run redo work, but never gives a wrong result.

A definition's hash is a fingerprint of the definition and of everything it depends on. It does not
cover the definition's comments, formatting or name. So a rename or a comment edit reruns no test.
If you edit a function's body and leave its signature unchanged, `ply` rechecks that function and
every function that calls it, directly or through other callers. A `ply` reuses a cached result only
if it would compute exactly that result itself. It recomputes results that a different `ply` cached
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

A test supplies its own time instead of reading the real clock, so it gives the same result on every
run. `nondet` marks an effect whose results come from outside the program, so they can differ
between runs. A test must handle every `nondet` effect it performs, which means supplying that
effect's operations with a `handle`. The one exception is a test written `test/nondet` (§10.2).

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

A source file is UTF-8 text. It consists of its imports, followed by its items in any order. Items
are `fn`, `type`, `effect`, `effect set`, `test`, `law`, `law schema`, `derive`, `key`, `show`,
`invariant`, `numeric`, `release` and `host` declarations. Definitions may refer to each other in
any order, across the whole program. Whitespace only separates tokens; indentation has no meaning. A
byte sequence that is not valid UTF-8, or a non-ASCII character outside a literal or comment, is
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

Write one space after the `///` marker. The first sentence of a doc comment is its summary, and a
bare `///` line starts a new paragraph. A doc comment should say what the signature does not show:
what the result means, when the function fails, the units, and edge cases. Leave out examples: the
tests and laws that use a definition are its examples, and `ply callers` lists them. A doc comment
that documents nothing, such as one inside a body or above a `test` or `import`, is an error
(`E0003`). Comments do not affect a definition's hash or a cached result.

### 3.2 Names and keywords

An identifier is an ASCII letter or `_`, followed by ASCII letters, digits or `_`. `_` alone is the
wildcard. Case matters in two places:

- in a type, a lowercase name is a type variable and an uppercase name is a type;
- in a pattern, a lowercase name binds a variable and an uppercase name is a constructor.

By convention, values are `snake_case`, types and constructors are `UpperCamelCase`, and effects and
the labels in `[r]` brackets (the *resource labels* of operations, as in `db.get[users](3)`) are
lowercase.

These words are always reserved: `pub` `import` `fn` `type` `effect` `nondet` `test` `let` `if`
`else` `match` `handle` `with` `true` `false`. A record field may still be named with a reserved
word, as in `d.nondet`. A record pattern may not write a field by its name alone, as in `{nondet}`,
because that would also bind a variable named `nondet`, and a reserved word cannot be a variable
name.

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
| `255u8`, `0x6A09_E667u32`, `-1i8` | the integer type named by the suffix | Suffixes `u8` to `u128` and `i8` to `i128`. The value must fit in that type (`256u8` is an error, `E0211`). |
| `1.5`, `1e9`, `2.5e-3` | `Float` | IEEE-754, 64 bits |
| `1.50m`, `0m` | `Decimal` | Exact base 10; keeps the digits as written |
| `"text"` | `String` | UTF-8, on one line |
| `f"n = {n + 1}"` | `String` | interpolated string |
| `\\text` | `String` | line string: a string written as one or more lines, each starting with `\\`. The `\\` marker is not part of the string. The rest of each line is used exactly as written, so `\` and quotes have no special meaning |
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
hex digits. Byte strings also allow `\xNN`, but not `\u{...}` or non-ASCII characters. The byte
literal `b'{'` has the same value as the integer `123`, the ASCII code of `{`. A program that scans
bytes can therefore write byte literals such as `b'{'` as patterns in `match` instead of numbers
(§6.2).

**Interpolated strings.** `f"a {x} b"` means `"a " ++ display(x) ++ " b"`. Each `{...}` in the
string is a *hole*. Here `display(x)` is `x` itself when `x` is a `String` or `Char`, and otherwise
the text that `show` writes for `x`. A hole may contain any expression. `{{` and `}}` are literal
braces. The value in a hole must be *showable*, meaning `show` can write it. A function, `Secret`,
`Task` or `Chan` in a hole is an error (`E0206`). A module that interpolates imports `std.show`
automatically. If `std.show` or a module it imports interpolated, that would make an import cycle,
so those modules cannot interpolate.

**Line strings.** Each line starts with `\\`, after optional spaces. The text after `\\` is taken
exactly as written, with no escapes. Lines are joined with `\n`, and no newline follows the last
line, so a value that ends with a newline ends with a line holding only `\\`:

```ply
fn program() -> String =
  \\fn main() -> Int = 42
  \\
```

A line string cannot be a pattern, and a line that ends in a space or tab is an error (`E0001`).

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

A tagged literal can have holes. The tag's parser receives the values from the holes separately from
the literal's text, so it can escape each value as the output requires, such as HTML escaping for
`std.html` or shell quoting for `std.sh`. The tag never reads a hole's value as part of the text.
This means a value cannot change the structure of the output, which prevents injection, where
untrusted input is interpreted as markup or commands:

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
- A tag's parser, `compile` and `fill` take no type, label or effect parameters. The only effect any
  of them may have is `raise`, which reports a failure. Any other effect is an error (`E0152`).
- An `Err` from the parser is an error (`E0153`), at the character `offset` of the text as written.
  A parser that raises or runs for more than 60 seconds is an error (`E0154`).

Ply caches the parser's result under two keys: the parser's code and the literal's text. A parser
therefore runs again only for new text, or after an edit to code the parser depends on. `f` and `b`
are not tags. They are the prefixes of interpolated strings and byte strings, and a module cannot be
named `f` or `b` to use as a tag. A tagged literal is never a pattern. The parser's result is built
from `Ok` and `Err` in the parser's own module. A module that defines its own `Ok` therefore cannot
use tagged literals (`E0118`).

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
| — | postfix: call `f(x)`, field access `r.field`, operation call `e.op[r](x)` (operation `op` of `e` with resource label `r`), and `e?` | the expression to the left of the postfix |

- Both operands have the same type. There is no widening: `U8 + U16` is an error (`E0201`).
- `==` and `!=` compare structurally at every type except functions. `Float` equality follows
  IEEE, so `NaN != NaN`.
- `<`, `<=`, `>` and `>=` work on numbers and `Char`. Use `compare` to order anything else.
  Arithmetic on a `Char` is an error (`E0201`); convert with `Int::of_char`.
- `&&` and `||` short-circuit. `>>` is an arithmetic shift and `>>>` a logical one.
- `++` joins two `String`s or two `Bytes`. Convert between them with `Bytes::of_string` and
  `String::of_bytes`.
- `::` refers to a name in a module (`orders::place`), and cannot be chained.
- `?` binds tightest: `f(x)?.field` is `(f(x)?).field`.

**Arithmetic is checked.**

- Division or remainder by zero raises `abort.raise` (§7.7), unless the divisor is a nonzero
  literal, the operands are `Float`, or the checker can see that an `Int` divisor is not zero.
- Three things end the program with `E0502`. The first is integer overflow, where the result does
  not fit in the type's range. The second is a `Decimal` result that cannot be represented exactly
  in `Decimal`. The third is a shift by a negative count, or by a count at least as large as the
  type's width in bits. Division by zero is different, because it raises `abort.raise`. These three
  do not raise `abort.raise`: they end the program directly, so they do not appear in a function's
  effects.
- `+%`, `-%` and `*%` wrap at the type's width, and `+|`, `-|` and `*|` saturate at its limits:
  `255u8 +% 1u8` is `0u8` and `250u8 +| 10u8` is `255u8`. They never fail. Write a space before a
  lambda used as a right operand (`a + |x| x`), since `+|` would otherwise be one operator.
- The `checked_*` builtins return `None` where an operator would fail (§13).
- `/` on `Decimal` is an error (`E0209`); use `Decimal::div`, which takes the result's scale.

## 4. Functions and modules

### 4.1 Functions

```ply
type Account = { name: String, balance: Int }

fn credit(a: Account, amount: Int, note: Option<String> = None) -> Account = {
  let balance = a.balance + amount;
  { name: a.name, balance }
}
```

A function's body is written after `=`, and it is a single expression. There is no `return`: a
function's value is its body's value. A body with `let`s is a block, in braces.

A top-level `fn` must write the type of every parameter, its return type, and its effects, if it has
any. Leaving out a type is an error (`E0126`), which shows the type the compiler inferred. A
parameter the body does not use may be written `_`.

**Defaults.** A parameter with a default may be left out of a call. A default must be a literal, a
`const` (§4.5), or a constructor, record or list of those (`E0121`). On a `pub fn`, a default may not
use anything its module does not export (`E0122`).

**Functions of a type.** `fn Stack::size(s: Stack) -> Int` declares the function `size` of the type
`Stack`. It is called as `Stack::size(s)`, or on a value as `s.size()` (§6.4), wherever `Stack` is
in scope. The function need not take a `Stack`: `Stack::empty()` can make one. Only the module that
declares a type may declare functions of that type (`E0208`). Functions can be declared only for a
sum type (a type declared with constructors) or for a record type declared with `new`. Declaring
functions for any other type is an error (`E0217`).

**Modifiers.** `reuse fn` promises that every list update in the body happens in place (§6.10).
`transparent fn` lets the proofs of other packages read the function's body, so that they can reason
about what the function does (§11).

**The entry point** is `main`, of any type and effects. A program with no `main` is an error
(`E0101`), and a program with several is an error (`E0112`). To choose which file's `main` runs,
name the file: `ply run tools/gen.ply`.

### 4.2 Holes

```ply
pub fn search(g: Game, s: State) -> Move =
  if over(s) { last(g) } else { ??? }

pub fn reward_of(r: Rewards, p: Player) -> Float = ..
```

`???`, or `..` in place of a body, is a *hole*: code not written yet. A hole has whatever type is
needed and performs no effects, so the program still checks and runs. Each hole produces a warning
(`W0613`). The warning reports three things: the type the hole must have, the effects the hole may
perform, and up to eight names in scope whose type could fill the hole. Running a hole raises
`abort.raise`, so the tests that run a hole fail and all other tests pass. You can write a file of
signatures whose bodies are all holes and run `ply check` on it. The check shows whether the types
and effects of the design fit together before any body exists.

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
nothing is an error (`E0106`).

Items are private unless marked `pub` (`E0107`). Functions and constructors, types, effects and
modules are separate namespaces, so `fn size`, `type Size` and `effect size` can coexist.

### 4.4 Opaque types

Another module can build a value of a public sum type from its constructors. It can build a value of
a public `new` record type (§5.2) with a record literal. `opaque` removes this: only the declaring
module can build values of the type. Every value therefore comes from that module's functions, so
any property those functions guarantee holds for every value:

```ply
pub opaque type Token = | Token(String)
pub opaque type Date = new { year: Int, month: Int, day: Int }

pub fn mint(s: String) -> Option<Token> = if s == "" { None } else { Some(Token(s)) }
pub fn text_of(t: Token) -> String = match t { Token(s) -> s }
```

Other modules can name the type, pass its values and call the module's functions. Using a
constructor of an opaque sum outside its module is an error (`E0107`), and building or updating an
opaque record there is an error (`E0220`). Other modules can still read an opaque record's fields;
to hide the contents, use a sum with one constructor.

`opaque` controls who builds values, not who reads them. Other modules can still compare an opaque
value with `==` and print it with `show`, and `reflect` can still inspect all of its contents. A
codec that the module derives (§12) converts a document to a value and back. The derived codec
decodes any document of the right shape, so it can build an opaque value from data that the module's
functions would never produce. If a type has validity rules beyond its shape, write its codec by
hand so that decoding checks those rules. To hide the contents of a value from every reader, use a
`Secret` (§5.7).

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
again exactly when the file changes. `E0146` is reported when the path does not exist, cannot be
read, or names a directory for `embed` or a file for `embed_dir`.

## 5. Types

Ply infers types and effects by unification, as Hindley–Milner type systems do: it works out each
unknown type from how the value is used. The types you write in a signature are checked against the
body. Inference never replaces them with a wider type (§5.6).

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

There are no implicit conversions. The whole definition determines the type of each operand. In `fn
h(a: U32) -> U32 = a + 1u32`, the literal must be written with the suffix `u32` to be a `U32`. If
nothing in the definition determines the type of an operand, as in `let g = |a, b| a + b;`, the
checker reports error `E0210`. Convert explicitly:

- The `of_int` functions of the sized integer types, such as `U32::of_int`, raise when the value
  does not fit. To truncate instead, apply a bit mask first: `U8::of_int(n & 0xFF)`.
- `to_int` converts a value of any integer type to an `Int`, or returns `None` if the value does not
  fit. `numeric_of_int` converts an `Int` to any numeric type.
- `String::of_bytes` raises on invalid UTF-8.

**Decimal** is a 96-bit integer and a *scale*: the number of digits after the point, from 0 to 28.
`1.5m == 1.50m`, but each prints as written. `Decimal::div` and `Decimal::round` take the scale of
their result and a `Rounding` mode.

**Float** gives the same bits on every machine and build. Each operation is computed exactly and
then rounded once, as written. A compiler or machine never combines two operations into one (for
example, a multiply and an add into a single fused multiply-add, which rounds only once), and never
changes their order. The math functions (`Float::sqrt`, `exp`, `ln`, `sin`, `pow` and the rest) are
correctly rounded: each returns the representable `Float` closest to the exact mathematical result.
A program cannot tell NaN values apart: every NaN, whatever its bit pattern, is the same `NaN`
value.

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

A `new` record is read, updated and matched like any record. It is a different type from a plain
record with the same fields, and from any other `new` record (`E0201`).  A record literal becomes a
`new` record, such as a `Date`, wherever the type checker expects that type. It expects a `Date` in
a return type, a parameter, a `let` with a written type, a field of a record whose type is known,
and a list element after the first (the first element sets the list's element type). The literal
must name every field of the `new` record. A record bound by a `let` without a written type is a
plain record.  A `new` record may contain itself only inside a sum type (`E0214`). For example, a
field of type `T` directly in `type T = new { .. }` is rejected.

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

Sums are nominal: each declaration is its own type, so two identical declarations in two modules are
different types. A sum may mention itself only with its own type parameters unchanged. `type Nest<a>
= | Leaf(a) | Deeper(Nest<List<a>>)` is an error (`E0477`), because `Deeper` uses `Nest<List<a>>`,
which changes the parameter. The type would then need a different type at every level: `Nest<a>`
contains a field of type `Nest<List<a>>`, which contains a field of type `Nest<List<List<a>>>`, and
so on without end.

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

A map's key type must be ordered: `Float`, functions, `Secret`, `Task` and `Chan` are not (`E0206`).
A signature that takes or returns a `Map<k, v>` requires `k` to be ordered, and the body may compare
two `k`s. A record or sum with a field of type `Map<k, v>`, where `k` is one of its own type
parameters, has the same requirement: `k` must be ordered.

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

A call supplies a value for each label parameter. It can write the label explicitly, as in
`relay[conn](b)`, or the compiler can take it from an argument whose type mentions the label. The
label of a scope (§8.2) is always taken from the argument whose type is that scope. A call that
leaves a label parameter without a value is an error (`E0306`).  Label parameters come before the
`|` that separates them from effect variables. `<[l, k]>` means the same as `<[l], [k]>`: both
declare two label parameters, `l` and `k`.

A type takes parameters the same way:

```ply
type Seq<a | e> = | Seq(() -> Option<{ value: a, next: Seq<a | e> }> / e)
type Sink<a, [l] | e> = | Sink((a) -> Unit / {net.send[l] | e}) | Quiet
type Pair<a> = new { first: a, second: a }
```

Every mention of a generic type must give an argument for each of its parameters (`E0202`). An
effects argument is a list of effects, such as `{fs.read[src]}` or `{}`. A type declaration may
mention only the type parameters that it declares itself.  For an alias, the compiler substitutes
the arguments into the alias's body, so the alias is the same type as that body. For a sum or a
`new` record, the arguments are part of the type's identity. In `Seq<Int | {}>`, the `|` separates
the type argument `Int` from the effects argument `{}`. So `Seq<Int | {}>` and `Seq<Int |
{fs.read[src]}>` are different types.

**Constraints.** `where numeric(a)` allows `+`, `-`, `*`, unary `-` and ordering on `a`, and
`numeric_of_int(n)` to make an `a`. `where integer(a)` also allows `/`, `%` and the bit operators.
`where derivable(eq, a)`, `derivable(ord, a)`, `derivable(hash, a)` and `derivable(show, a)`
require `a` to support equality, ordering, hashing or display (§12):

```ply
fn sum<a>(xs: List<a>) -> a where numeric(a) = xs.fold(numeric_of_int(0), |s: a, x: a| s + x)
```

A definition with a `numeric` or `integer` constraint receives the type as a hidden argument at run
time. A value that is not called directly has no place to receive that argument, so the definition
must be called directly: using it as a value is an error (`E0310`).  A type may put `derivable`
constraints (`eq`, `ord`, `hash` or `show`) on its own type parameters. Every use of the type must
give type arguments that satisfy those constraints (`E0206`).

**Recursion.** A recursive call may fill type, label and effect parameters differently, except that
it may not fill a type parameter with a larger type built from it, as `f<a>` calling `f<List<a>>`
(`E0308`).

**Resource parameters.** Filling a type parameter with a resource (§8.1) is an error (`E0317`),
unless the parameter is written `keep`: `fn keep_one<keep a>(x: keep a)`.

### 5.6 Function types, and what you must write

`(A, B) -> C` is a function that performs nothing and cannot raise. `(A) -> B / {db.read[users]}`
and `(A) -> B / e` have effects. A function value may perform less than its type allows, never
more. Functions cannot be compared, encoded, ordered or used as map keys.

You write every parameter type, the return type and the effects of a top-level `fn`, and the type of
every `forall` binder. Everything else is inferred: lambda parameters, `let` types and so on. A
local `let` is not generic, so `let f = |x| x;` used at two types is an error (`E0201`). Callers are
checked against the signature, never against the body.

The written effects are an upper bound: what the body performs must fit inside them (`E0302`, which
shows the effects to write). An effect the body never performs is the warning `W0614`, which shows
the narrower signature. An `effect set` counts as performed when any of its members is.

**Borrowed function parameters.** A function-typed parameter of a top-level `fn` is *borrowed*: the
function may use it only until the call returns. It may call it, pass it to another borrowed
parameter, or capture it in a lambda that is only called. Returning it or storing it is an error
(`E0475`). Write `keep` to let the function keep it:

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

- `Secret::of_string` and `Secret::of_bytes` wrap a `String` or `Bytes` value in a secret.
  `config.secret` (`std.config`) returns a secret whose contents the program never sees in plain
  form. `entropy.secret(n)` (`std.random`) generates `n` random bytes and stores them directly in a
  new secret.
- `Secret::concat`, `Secret::decode`, `Secret::encode` and the key derivations in `std.hash` make
  new secrets from secrets.
- A program can only compare two secrets (`==`, in time that depends only on their lengths), check
  one against a string (`Secret::verify`), and read its length.
- Hashing passwords (`std.password`), signing and key agreement (`std.crypto`), and MACs
  (`std.hash`) use secrets directly. `std.fs`'s `write_secret` and `std.net`'s `send_secret` write
  one out, and `std.fs`'s `read_secret` reads one back.
- A secret cannot be shown, encoded, ordered, hashed or matched, and a `forall` cannot generate one
  as a test value. Its memory is zeroed when it is freed.

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

- `key for T by f`: two `T`s are equal when `f` returns equal keys, and they sort in the order of
  their keys. This holds everywhere: in `==`, in `compare`, and in every collection that holds `T`s.
  `match` still sees the constructor and fields the value was built with, not its key. Keep `f`
  cheap, since every comparison calls it.
- `show for T by g`: `show`, `display` and interpolation use `g`.
- `invariant for T by v`: `v` is a function that returns whether a `T` is valid, and every `T` must
  satisfy it. `T` must be `opaque` or private (`E0480`). Every place in the declaring module that
  builds a `T` must be proved correct: `ply prove` must prove that the new `T` satisfies `v`
  (§11.5). Code in other modules can then rely on every `T` satisfying `v`. `v` is used only by the
  prover, so it never runs when the program runs.
- `numeric for T by { add: add, sub: sub, mul: mul, neg: neg, of_int: of_int }`: `+`, `-`, `*` and
  unary `-` on a `T` call `add`, `sub`, `mul` and `neg`. `numeric_of_int` on a `T` calls `of_int`.
  If `T` also has a `key`, `T` is a numeric type. A numeric type supports `<`, and it can be passed
  to a function that requires its argument type `a` to satisfy `numeric(a)`. `/` and `%` are not
  available.

Only the module that declares `T` may declare these, and it may declare each one once (`E0208`,
`E0105`). `T` must be a sum type, which is a type declared with several constructors (`E0217`). Each
named function must be in the same module, must have no effects, must not raise, and must not take a
resource label (`E0218`). Changing any of these functions changes the type's identity, which is what
makes it a different type from every other. Changing the type's fields does the same.

## 6. Expressions

Everything is an expression, including `if`, `match`, `handle` and blocks.

### 6.1 Blocks, `let` and `if`

A block `{ statements; tail }` has the value of its last expression, or `Unit` if there is none.
`let pattern = expression;` binds names, and `let name: Type = expression;` states the type. A later
`let` may reuse a name. A statement needs `;` unless it is a block, `if`, `match`, `handle`,
`with_scope` or `simulate`.

A `let` pattern can split a record into its fields. A function uses this to return several values,
which the caller binds with one `let`:

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
effects become part of the enclosing function's effects. If a lambda has a return type annotation,
its body must be a block. Inside that block, `?` is allowed.

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

If the name before the dot is a bare name, Ply decides what it refers to. If the name is a variable,
the call is made on the variable's value. If the name is an effect, the call performs one of the
operations of that effect (§7.1). If the name is both a variable and an effect, the call is error
`E0105`.

### 6.5 Record update

`{..base, deep: {..base.deep, a: 7}}` copies `base` with the listed fields replaced. The base must
be a variable, a field path or a call, and its type must be known where the update is written
(`E0116`). Naming a field that the base does not have is an error (`E0117`). If `base` is a record
of a type declared with `new`, which makes a distinct named type, the update returns a record of
that same type.

### 6.6 Loops and recursion

There is no `for`, `while` or `break`. A function that calls itself in tail position runs as a
loop, using no stack. Tail position is the body's value, the last expression of a block, an `if` or
`match` branch, or the right side of `&&` or `||`:

```ply
fn sum_to(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { sum_to(n - 1, acc + n) }
```

Functions of one module that tail-call each other in a cycle, like `even` and `odd`, also run as a
loop. A tail call is a call that is the last thing a function does. The exception is a cycle that
contains a `handle` with a clause that calls `resume` anywhere except as the last thing the clause
does. Every other call uses space on the call stack, which holds one gigabyte. A call that needs
more than that ends the program (`E0502`). `range`, `List::map`, `List::filter` and `List::fold` use
no stack per element.

`iterate` is a loop with an early exit and a limit on the number of iterations:

```ply
fn first_gap(xs: List<Int>) -> Int =
  iterate({i: 0, want: 0}, 1000, |s: {i: Int, want: Int}|
    if s.i >= xs.len() { Stop(s.want) }
    else { Continue({i: s.i + 1, want: s.want + 1}) })
```

The function passed to `iterate` returns `Continue(next)` to run another iteration with `next`, or
`Stop(result)` to end the loop with `result`. If the loop runs more iterations than the limit that
was passed to `iterate`, it fails by performing the `abort.raise` operation, which reports a failure
on a bad value.

A test has 60 seconds by default (§10.4). `ply run` has no time limit unless given `--timeout MS`,
since a server runs until stopped.

### 6.7 `?`

```ply
fn int_value(text: String) -> Option<Int> = {
  let d = Decimal::of_string(text)?;
  Int::of_decimal(d, Down)
}
```

`e?` returns early from the function when `e` is `Err` or `None`. Otherwise `e?` evaluates to the
`Ok` or `Some` value. The early return uses the return type that the function is annotated with, and
`?` does not convert the error from one type to another. If a function returns `Result<_, E2>` and
contains `?` on a `Result<_, E1>`, the checker reports error `E0201`. To fix it, convert the `E1` to
an `E2` yourself before using `?`.

- `?` must be in a position where its result becomes the function's result, so no conditional code
  may come between `?` and the end of the function. `let x = e?;` is allowed. The checker reports
  error `E0119` when `?` is inside an `if` branch or a nested block that is not the function's
  result, or when `?` comes after a call argument that performs effects. To avoid the error, bind
  the value of `e?` to a variable with `let` before that code.
- Inside a `handle` or `try`, `?` leaves the whole function. The clauses of the `handle` or `try` do
  not intercept it.
- Inside a lambda, `?` requires the lambda to have a return type annotation, and `?` leaves only the
  lambda. Using `?` inside a `with_scope`, `simulate` or `parallel` branch is an error (`E0118`).

### 6.8 `parallel`

```ply
fn both(xs: List<Int>) -> (Int, Int) =
  parallel { xs.fold(0, |a: Int, x: Int| a + x), xs.filter(|x: Int| x > 0).len() }
```

`parallel { a, b, .. }` returns the tuple `(a, b, ..)`, and means the same as evaluating its
branches from left to right. The runtime runs the branches at the same time on its own threads
whenever that cannot change the result. The checker accepts a `parallel` block only when no branch
writes something that another branch reads or writes (`E0309`, §7.2). So a `parallel` block is
exactly as deterministic as its branches.

- If several branches fail, the `parallel` block fails with the failure of the leftmost failing
  branch.
- Branches may perform operations that a `handle` around the block handles, and still run at the
  same time, when that `handle` has no `var` and each clause it reaches returns without `resume`,
  captures nothing and performs only through handlers of the same kind. Host handlers are written
  this way. Under any other `handle`, or under a `simulate` block, the branches run one after
  another, and the block returns the same result.
- A branch may not spawn a task or open a `simulate` block (`E0309`), and may not use `?`.

`std.parallel` splits a list into parts and processes each part in its own nested `parallel` block.

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
- an integer that moves toward a bound. A condition on the path to the call must guarantee that the
  integer has not yet reached the bound. `down` is an example, and so is `if i >= xs.len() { .. }
  else { walk(xs, i + 1) }`: the recursive call runs only when `i` is still below `xs.len()`.

The checker also applies these rules to calls made through `List::map`, `List::filter`, `List::fold`
and lambdas. It does the same for calls made by a type's `key` or `numeric` functions.

Sometimes no single argument gets smaller on every recursive call. Then `decreases` states an `Int`
measure computed from the parameters, as in `climb`. The checker proves two things about every
recursive call: the call makes the measure smaller, and the measure is not negative at the call. If
it cannot prove both, it reports `E0467`.

A function needs `diverges` in its effects (`E0302`) if the checker cannot show that its recursion
ends, or if it calls such a function. Write `diverges` only for a loop that waits on something
outside the program, such as a stream or a network peer. Otherwise fix the recursion, use `iterate`,
or add `decreases`. Tests, laws and handler clauses need no `diverges`. Each of them runs under a
time limit, and the limit stops any that does not terminate. For each function that the checker
cannot show to return, `ply check --types` prints the reason.

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

A variable counts as "used again" only if the program can use it after the update, on the path it
actually takes. A variable used only in a different branch does not count. Arguments are evaluated
before the call they are passed to, so `c.set(i, c.get(i) + 1)` updates in place. Suppose the
program matches a record against a pattern, or reads a field that nothing reads afterward. Then the
record no longer needs that field's old value, and the field can be replaced without copying the
record. So `{ ..s, out: s.out.push(x) }` grows `out` in place.

`ply check --costs` lists each update in your modules that copies, with the cause and a fix. A
function declared `reuse fn` must not copy: any update in it that copies is an error (`E0127`):

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n == 0 { xs } else { collect(xs.push(n), n - 1) }
```

The checker judges a call to a function in another package by the `returns` clause in that
function's signature, never by its body. `returns fresh` promises that nothing else refers to the
result, and `returns xs` says the result may refer to the same value as the parameter `xs`. The
checker verifies the clause against the body (`E0148`):

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

`[r]` gives an operation a *resource label*. Every call of the operation must supply a label, as in
`db.get[users](3)`; leaving it out is an error (`E0304`). Labels name the resource an operation
touches, so the checker can tell that writing `[orders]` does not conflict with reading `[users]`.
Labels are global: two modules that write `[users]` mean one resource.

`nondet effect` marks an effect whose results come from outside the program, such as a clock or the
network (§10.2). An operation may take type parameters, as in `read take[r]<a>(key: Int) -> a`. The
handler clause for that operation must be written so that it works for every type `a`: it cannot
assume `a` is any particular type. An effect is identified by its name. The names `task`, `clock`,
`random`, `sim`, `abort`, `diverges` and `scoped` belong to built-in effects, and declaring an
effect with one of them is an error (`E0105`).

`db.get[users](3)` performs an operation, and `store::db.put[orders](id, row)` performs an operation
of the effect `db` from the module `store`.

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

`both(|| a.x(), || b.y())` performs exactly `{a.x, b.y}`. In a parameter's function type, and in the
type arguments of a `Task`, a `Chan` or a sum type, the effects of a function must equal the
declared effects. Fewer effects are not accepted there. The reason is that a function stored in such
a value could later be passed an argument that its type does not allow.

**Labels.** A label parameter (§5.5) inside the body hides the global label of the same name.
`[*]` in a handler clause or a signature stands for every label (§7.4). Two effects *conflict* when
they use the same resource of the same effect and at least one is a `write`. A label parameter or
`[*]` conflicts with every label of its effect. `parallel` (§6.8) and the test scheduler (§10.4)
use this rule.

**Counts.** The checker also records whether each effect is performed a `bounded` number of times or
a `scaling` number that grows with the input. An effect scales when it is performed once per element
of a list or map (through `List::map`, `List::fold`, `iterate` and similar), when it is performed
inside recursion, or when a called function performs it a scaling number of times. `ply check
--types` prints the count of each effect:

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

A definition's own signature may promise `bounded`, as in `/ {db.query[conn] bounded}`. If the body
performs that effect once per element, the checker reports `E0465` and points to the loop. If the
body breaks the promise in any other way, the checker reports `E0466`. The usual fix is one query
for the whole input.

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
also use `[*]`, so that a library function can handle an effect for the code it is given. In `fn
sink<a | e>(body: () -> a / {log.note[*] | e}) -> a / {wire.put[c] | e}`, the parameter `body` may
perform `log.note` on any label.

**What a handler removes.** A clause for `net.send[conn]` removes that effect from the body's
effects. A *mode*, like `net.write[conn]`, covers every `write` operation of `net` on `conn` (§7.2).
If the clauses handle every operation of a mode, the handler removes the mode itself. The `handle`'s
effects are the body's effects, minus what the clauses remove, plus the effects the clauses perform.
The `handle` is an error (`E0305`) when all three of these hold: the body may perform operations of
a mode, the clauses handle only some of those operations, and the enclosing function's effects do
not cover the rest. The error offers the missing clauses.

**What a handler sees.** A `handle` handles only the operations that its body's effects list.
Sometimes the body performs an operation that comes from the enclosing function's caller. This
happens when the operation arrives through an effect variable (like `e` in §7.2) or through a label
parameter (like `[r]` in a signature) of the enclosing function. The `handle` does not handle that
operation. The operation goes to the handlers around the call of the enclosing function:

```ply
fn quiet<| e>(f: () -> Int / e) -> Int / e =
  handle { f() } with { log.note(s) -> () }
```

In `quiet(|| { log.note("x"); 1 })`, the `log.note` goes to the handlers around the call of `quiet`.
A function that wants to handle the operations of its callback must list them in the callback's
type. The function `sink`, declared earlier in this section, does this by writing `log.note[*]` in
the type of `body`.

### 7.5 `resume` and continuations

In `amb.flip[coin]() resume k -> k(true) * 2`, `k` is the *continuation*: the rest of the body
after the perform, as a function. Calling `k(v)` continues the body with `v` as the operation's
result, and the clause's value becomes the `handle`'s value.

- `k` may be called at most once (`E0316`).
- `k` may outlive the `handle` and be called later. What the resumed body performs then goes to the
  handlers around that later call.
- A clause that binds `k` and never calls it abandons the rest of the body.

`bracket(acquire, release, body)` makes sure a body releases what it acquired. `release` runs on the
value that `acquire` returned in each of these cases: `body` returns, `body` ends early because of a
raise or because a handler clause binds `k` and never calls it (§7.5), or its task is cancelled
(§9.3). A cancel never interrupts an `acquire` or a `release`. Nested brackets release the innermost
first. If a `release` itself fails, its failure replaces the raise or the early end that was already
stopping the body. `with_scope` (§8.2) is a bracket whose body cannot return the acquired value to
its caller: the body can only use that value through the scoped value it is given.

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
or `with_scope` is an error (`E0315`), since that code may run after the clause returns.

A clause with `resume k` continues the body with the variables' values at the time it calls `k`.

Tasks spawned under a handler share its variables (§9). Other tasks cannot interrupt a clause until
it performs an operation that may wait. So in a clause, do not put a wait between reading a variable
and assigning a value computed from that read. Another task could change the variable during the
wait, and the assignment would overwrite that change. `simulate` tries the different orders in which
tasks use a handler's variables, so a test finds such an overwritten update.

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

**`try`.** `try { body }` returns `Ok` of the body's value, or `Err` of the value that the raise
carried. That value is the raise's single parameter, `()` if the raise has none, or a tuple if it
has several. `try` handles the one raise that the body's effects list, not counting `abort.raise`.
If the body has no such raise, the error is an error (`E0311`). If the body has more than one, the
error is `E0312`; in that case, write `try[toml.syntax] { body }` to name the raise that `try`
handles. To turn an `Err` back into a raise, take a `Result` value `r` and write
`r.unwrap_or_else(|e: SyntaxError| toml.syntax(e))`.

**`abort.raise`.** The prelude declares `effect abort { raise raise(message: String) }`.
`abort.raise(m)` is the raise for a failure on a bad value: `panic`, `assert` and `assert_eq`, a
builtin used outside its domain, division by zero, a `let` that does not match, `iterate` past its
limit, and a few task operations (§9). A clause for `abort.raise` receives the message, and `try`
handles it only when named: `try[abort.raise] { .. }`. Unhandled, a failed assertion is an error
(`E0501`) and any other is an error (`E0502`). Overflow and running out of stack end the program and
cannot be handled.

**Calls that cannot fail.** A call adds no `abort.raise` when the checker can see that it cannot
fail where it is written:

```ply
fn peek(b: Bytes, i: Int) -> Int = if i < 0 || i >= b.len() { -1 } else { b.at(i) }
```

A builtin that can raise declares a `requires` clause stating the condition under which it does not
raise. For example, `Bytes::at` requires `0 <= i && i < Bytes::len(b)`. The checker tries to prove
each such clause from the conditions that are known at the call: `if` conditions, guards, matched
patterns, and `List::at` returning `Some`. It reasons with linear integer arithmetic (addition,
subtraction and comparison of integers, and multiplication by constants) and with what builtins
guarantee about their results. It looks only at the function being checked, so a function's effects
never depend on another function's body. If the checker cannot prove a clause, it treats the call as
able to raise `abort.raise`. This is not a compile error.

`ply check --types --explain` lists every place a function can raise `abort.raise`, and why.

### 7.8 Host handlers

A standard library module can give an effect a *host handler*: the handler used when the program
does not handle the effect itself. A host handler connects the program to the machine it runs on,
such as its operating system.

```ply
pub nondet effect os {
  read pid() -> Int
}

host for os by served

fn served<a | e>(go: () -> a / {os.read | e}) -> a / e =
  handle { go() } with { os.pid() -> machine_pid() }
```

Every entry point the runtime starts (`main`, a test, or an entry given to `ply run`) runs under the
host handler of each effect its signature lists. Host handlers sit outside the program's own
handlers. So if the program does not handle `os.pid()`, the call goes to the host handler of the
`os` effect. That host handler is declared with the keyword `served`. Only standard library modules
declare host handlers (`E0161`), and §14 lists the effects that have one.

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

`release for T by f` makes `T` a resource, released by `f`. `T` must be a sum type declared in the
same module, and code outside that module must not be able to see its constructors (`E0225`). `f`
takes one parameter of type `keep T` (§8.1 explains `keep` below) and returns `Unit`. A value that
contains a resource is a resource too. So is a function that can be called only once, which is a
function declared `once`.

- A `let`, a pattern binding or a `keep` parameter *owns* its resource.
- Passing it to a `keep` parameter, returning it, or storing it *moves* it. Using the name
  afterwards is an error (`E0316`).
- Passing it to a parameter without `keep` *borrows* it: the callee may use it, but not move it
  (`E0318`).
- A resource cannot be compared, shown or copied (`E0317`).

A resource still owned when its owner's block ends is released there, in reverse order of binding;
in `greet`, `close(c)` runs after `send` returns. It is also released when a raise, a `?` or an
abandoning clause leaves the block. A resource that nothing owns, such as one created as a
statement or bound to `_`, is an error (`E0322`).

A `match` on a resource inspects its constructors without releasing the resource. Only the declaring
module can see the constructors, so only that module can write this `match`. A `close` function in
that module uses such a `match` to end a `Conn`. A lambda that uses up a resource it captures can be
called at most once. A handler clause, a `simulate` body or a `parallel` branch may borrow resources
from outside, meaning use them without consuming them. It may not move them, meaning consume them,
because that clause, body or branch may run several times.

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
(`E0446`). So a scoped value can be passed into functions and lambdas, and the checker can see every
way the value could leave its scope. There are two ways. The value itself can leave through a type,
such as a return type. A closure that reads the value can leave through its effects. Each scope
needs its own name (`E0330`), so the checker can tell scopes apart. A declared type or effect cannot
mention `Scoped`, `Task` or `Chan`. Use a type parameter instead, so the scope is part of whatever
type argument the caller supplies.

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

`simulate { .. }` runs its body under a scheduler that controls the order in which tasks run. The
scheduler handles `task`, `clock`, `random` and `sim` itself, so no handler outside the `simulate`
sees those operations. The effects of the `simulate` expression include `sim.read`. A deterministic
test may have the `sim.read` effect, so a test can contain a `simulate`.

- Time is virtual: it moves forward only when no task can run, so `clock.sleep` takes no real time.
- Tasks switch only at `task`, `clock` and `random` operations. The search tries the orders of these
  steps that could change the result. When it covers every possibility, the result holds for every
  interleaving, and is reported as `exhaustive`.
- The search never reorders two accesses to one handler variable when at least one of them writes.
  It never reorders any two allocations. A read followed by a write, with no scheduler operation
  between them, runs as one step, so no other task runs in between. To let the search interleave
  other tasks between the read and the write, put a `task.yield()` between them.
- When a task raises and nothing in the task handles it, the simulation cancels every other task,
  waits for them to finish releasing their resources, and then raises at the `simulate`, as if its
  body had raised.
- A `parallel` block inside a simulation runs its branches one after another.

Errors:

- `E0413`: a `Task` or `Chan` is used outside the simulation that created it. This happens when the
  program returns one from the simulation, stores one in a returned value or closure, or passes one
  in from outside.
- `E0414`: no task can make progress (deadlock), or an interleaving ran past `--sim-steps`.
- `E0416`: a `simulate` appears inside another `simulate`.
- `E0425`: the simulation calls an operation that reads or changes something outside the simulation,
  such as reading a file from disk.

### 9.3 Cancellation

`task.cancel(t)` stops the task `t` where it is. If `t` is waiting (in a sleep, a join, a channel
operation or a host operation), `t` stops waiting. From then on `t` does nothing except clean up: it
runs the `release` of each `bracket` it is inside (§7.5). The cancel stops `t` only at the moment an
`acquire` or a `release` returns. So everything that `t` acquired is released.

`cancel` returns `false` for a task that had already finished or was already cancelled.
`task.await(t)` returns `Some` of the result, or `None` if `t` was cancelled. `task.join` of a
cancelled task raises `abort.raise`. A task cannot cancel itself. A deadline needs three tasks. The
first task sleeps and then cancels the second task. The third task awaits the second task, so it
learns whether the deadline passed. The interleaving search (§9.5) tries each cancel at every point
where the cancel could stop the task.

### 9.4 Channels and `select`

`task.channel(n)` makes a channel that stores up to `n` values no receiver has taken yet. A channel
of capacity `0` is a rendezvous: a send waits for the receive that takes its value.

- A send waits while the channel is full, and returns `true` once its value is queued or received.
  A receive waits while the channel is empty, and returns the oldest value.
- `task.close(c)` ends sending. Later sends return `false`, and receives return what was queued,
  then `None`.
- A race is built from one channel. Each worker sends its result on the channel. The receiver keeps
  the first result it receives, and then cancels the other workers.

`task.select(arms, wait)` performs one of several channel operations, the first one in the list
`arms` that can proceed. An arm is a pair of a channel and an optional value. With `None`, the arm
receives from the channel. With `Some(x)`, the arm sends `x` on the channel. `select` returns
`Some((i, got))`, where `i` is the index of the arm that proceeded. `got` is `Some` of the value
received or sent, or `None` if the channel was closed. When no arm can proceed, `wait: false`
returns `None` at once, and `wait: true` waits until some arm can proceed.  When several arms can
proceed, `select` always picks the earliest one, so a fixed order always favours the first arm. For
fairness, change which arm comes first on each call, as `std.chan` does.

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
| `--measure-reduction` | also run the search with its pruning of equivalent orderings turned off, and report how many interleavings (distinct orders of the tasks' steps) each of the two searches ran |

A failure prints the racing steps, which are the steps of different tasks whose order changes the
result. It also prints a command that runs the failing interleaving again, such as `ply test --seed
0:0.1.0.2 --filter 'no account is ever overdrawn'`. If a search stops because it used up its budget
before it tried every interleaving, its test passes, but `ply test` does not cache that result.

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
their first difference. Any other failure is an error (`E0502`). A test is named `<module>.<label>`,
and two tests in one module need different labels (`E0105`).

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
- `ply test` caches each case by the hash of its value, so the case type must be hashable without a
  `key` declaration. A case cannot contain a `Float`, a function, a `Task`, a `Chan` or a `Secret`
  (`E0206`). The expressions that build the table and each case's label may raise, but must perform
  no other effects (`E0469`).

**Snapshots.** A test can compare text to a stored file, read with `embed_dir`. The test has no
effects, so `ply test` caches it. It runs again when a stored file changes:

```ply
import std.snapshot
import std.snapshot (Snapshots)

fn stored() -> Snapshots = { dir: "snapshots", files: embed_dir("snapshots") }

test "an order renders as stored" {
  snapshot::check(stored(), "order.txt", snapshot::render(order()))
}
```

`snapshot::check` fails with a diff between the text the test computed and the stored text, and
never writes files. To store snapshots, write an entry point that calls `snapshot::accept`. Run it
with permission to write to the snapshot directory, which the flag `--fs` grants: `ply run . --fs
snapshots=snapshots` (§14.1).

### 10.2 Deterministic and nondeterministic tests

A test must be deterministic. After the test's own handlers have handled effects, the test must not
have a `nondet` effect left. If it does, the error is an error (`E0412`). To fix it, handle that
effect, or write `test/nondet "label" { .. }`. A `test/nondet` is cached like any test. However, the
values it reads from the clock or the network are not part of the cache key, so a change in those
values does not make the test run again (§10.3).

A test is not a process, so it has no standard streams: a test that prints is an error (`E0424`)
unless it handles the console itself. A test may start the programs `--exec` grants (§14.1).

### 10.3 Which tests run

`ply test` runs a test only when no passing result is recorded for it. A passing result is recorded
under two keys. The first key is the test's hash, which covers everything the test depends on but
not names, comments or formatting. The second key is the compiled code the test ran. If an edit or a
new `ply` compiles the test to the same code, the hash changes but the code does not, so the
recorded pass still applies and `ply test` does not run the test.

A pass also records what the test read from outside the program, and stays valid only while that
is unchanged:

- each file and directory it read under a file system root;
- each standard library function, type and effect it used;
- everything read by each `ply` process it started;
- for a test that used the host (the real machine, through the real handlers, rather than handlers
  the test supplies), the permissions the test ran with (§14.1).

The following do not count as inputs: values supplied by the test's own handlers, files the test
wrote and then read, the clock, and the network.

| flag | meaning |
| --- | --- |
| `--explain` | why each test was selected, which input changed, and where the time went |
| `--filter TEXT` | run tests whose `<module>.<label>` contains `TEXT`; repeatable. Tests whose `<module>.<label>` does not contain `TEXT` do not run. The trailing dot in `std.json.` is part of the text, so the filter matches every test in the module `std.json` and no test in a module such as `std.jsonl`. |
| `--shard K/N` | run part `K` of `N`; each test is always in the same part |
| `--no-cache` | ignore recorded results |
| `--watch` | run again whenever a source file, or a file the last run embedded, changes |

`ply hash` prints definitions' hashes.

### 10.4 How tests run

A test's *outside effects* are the `nondet` effects it performs on the world outside the program.
Tests whose outside effects do not conflict (§7.2) run at the same time, one per core. `--jobs N`
(`-j N`) splits the tests into `N` lanes that each run their tests in turn.

The report has three parts. The first says which tests were selected and how they will run. The
second lists the results of each lane (§10.4) as that lane finishes. The third is a summary that
gives the diagnosis of each failure. Under `--json`, the first two parts go to standard error as the
run goes, and one JSON object goes to standard output at the end.

`--timeout MS` limits each test's wall-clock time (default 60000; `0` for none). A test that exceeds
the limit does not fail. It *timed out* (`W0612`), and `ply test` lists it separately from the
failures and does not count it as a failure. It runs again the next time. A run in which any test
timed out is not a success.

### 10.5 When a test fails

A failure lists its *changes*: the definitions the test uses that changed since the test last
passed, and the test itself if its text changed. Edited definitions come first, then added or
removed ones, then those that changed only because a definition they use changed. After the
failures, `ply test` prints each change once, as a diff of the old and new text. A test that has
never passed has no earlier version to compare with, so each definition it uses counts as a change.
The exception is a definition that any earlier passing run of any test has already used. That
definition does not count.

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

`identity(a)` at the start of a `where` limits a law to types for which `==` compares the whole
value, such as `Int`, `String` and records and lists of them: `forall (xs: List<a>, n: Int) where
identity(a), n > 0`.

### 11.2 `ply prove`

`ply prove` proves every claim of the root package, the package whose directory you run it in. The
claims are each law, each `ensures`, and each claim made by an `invariant`, `cost` or `space`
declaration. It succeeds only when every claim is `proved`.

| outcome | meaning |
| --- | --- |
| `proved` | holds for every input the guard allows |
| `refuted` (`E0419`) | false at the input shown |
| `vacuous` (`E0420`) | the guard allows no input |
| `unproved` (`E0474`) | neither proved nor refuted; the report says where the prover stopped |
| `internal error` | Ply failed, not the program |

A program may not rely on a property that `ply prove` cannot prove. Check the property in a test
instead, over values that `std.gen` generates.

| flag | meaning |
| --- | --- |
| `--filter TEXT` | prove only the claims whose declaring definition (the function, law or type the claim is written on) has a name containing `TEXT`; repeatable |
| `--prove-budget N` | reasoning steps per claim |
| `--timeout MS` | wall-clock time limit for each evaluation the prover runs while checking a claim, not for the whole claim (default 60000) |
| `--static-prover` | run the static prover (the part of `ply prove` that reasons about the code without running it) on every claim, in addition to the normal run, and report whether it proved each claim |
| `--jobs N` | split the work into `N` lanes |

`ply prove` prints each claim's outcome as it is decided, then the definitions no proved claim
covers, then totals. A proof is cached under the claim's hash. Only proofs are cached; any other
outcome is computed again on every run.

### 11.3 How claims are proved

The prover combines:

- evaluating terms whose values are known;
- trying every point of a finite domain of up to 4096 points;
- linear `Int` arithmetic (sums and differences of `Int` values and multiplication by literals, with
  comparisons), splitting a claim into separate cases, and treating terms known to be equal as
  having equal values;
- replacing a call with the body of the definition it calls, taking the matching `match` arm when a
  value's shape is known, and running a terminating recursive definition on known arguments;
- induction on an `Int` variable of the claim (prove it for `n <= 0`, then for `n` assuming it holds
  for `n - 1`) or on a `List` variable (prove it for `[]`, then for `[h, ..t]` assuming it holds for
  `t`), where a small trusted part of the prover checks each induction proof;
- searching every interleaving of a `simulate` block;
- the contracts of the definitions a claim calls.

**Contracts.** A contract is a definition's `ensures` clauses, which state what is true of its
result. For a call to a recursive definition, or to a definition from another package, the prover
uses the callee's `ensures` instead of the callee's body. It does so only when the callee cannot
raise and is known to terminate. For a callee in the same package, the prover uses the `ensures`
only after it has proved them. For a definition from another package, the prover knows only the
contract, unless the definition is a `transparent fn`. For a call to a `transparent fn`, the prover
may substitute the function's body.

**Running.** The prover proves a claim by running it only when the runs cover every input. That is
the case for a finite domain of at most 4096 points.
The same holds when the claim has an `Int` variable and its guard (the `where` condition) bounds
that variable between two literals: `where n > 0 && n <= 100` is a hundred points.
Over any other domain, the prover also tries a few small values: zero, the literals in the claim and
their neighbors, short lists, and each variant of a type declared with variants. A value where the
claim is false refutes the claim, but values where it passes prove nothing.

**Equality.** At `Int`, `Bool`, `String`, `Bytes` and data built from them, `==` is true exactly
when the two values are the same. At `Float`, `==` follows the IEEE 754 standard. At `Decimal`, `==`
compares values, so `1.5m == 1.50m`. At a type declared with a `key`, `==` compares the keys, and
the prover treats the comparison as a call whose body it cannot inspect.

**Lemmas.** A law with no `where` guard is a lemma. Once the prover has proved the law, it uses the
law to prove the claims below it in its module and in every module that imports it. The prover
applies the law at any call that has the same shape as the first call in the law's claim, with the
law's variables replaced by the call's arguments. The two laws below show this:

```ply
fn reverse(xs: List<Int>) -> List<Int> =
  match xs { [] -> [], [x, ..rest] -> reverse(rest).push(x) }

law "a push reversed leads" forall (ys: List<Int>, x: Int) {
  match reverse(ys.push(x)) { [h, ..t] -> h == x && t == reverse(ys), [] -> false }
}

law "reverse twice is identity" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }
```

The prover proves both laws above. It proves the second by induction on `xs`, using the first.

### 11.4 Law schemas

A `law schema` states a law once for any definitions that fit it. A `law` uses a schema by supplying
the definitions for the schema's parameters:

```ply
pub law schema round_trip<a, b | e>(encode: (a) -> b / e, decode: (b) -> Option<a> / e)
  forall (x: a) { decode(encode(x)) == Some(x) }

law "a URL round-trips" = round_trip(url_encode, url_decode)
```

A schema has typed parameters and at most one effect variable. An effect variable stands for a set
of effects, and in a schema it can stand only for `raises`. Each use of a schema is a separate
claim, which the prover proves and caches on its own. A schema also defines a function. Its
arguments are the schema's parameters, followed by one argument for each variable in the claim. So
`round_trip(url_encode, url_decode, "a b")` checks the law at one point. `std.laws` contains the
standard library's schemas.

### 11.5 Invariant claims

For a type with an `invariant` (§5.8), each definition in the type's module that builds a value of
the type must prove that the value satisfies the invariant. These claims are named `<definition>
builds <type>`. The prover does not assume the definition's `requires`. It assumes that every value
of the type that the claim is given satisfies the invariant, whether the value is a variable of the
claim or the result of a call.

### 11.6 Cost and space bounds

A `cost` clause bounds the steps a call takes, up to a constant factor:

```ply
fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost xs.len() * (ys.len() + 1)
= xs.fold(0, |a: Int, x: Int| ys.fold(a, |b: Int, y: Int| b + x * y))
```

The bound is written as an `Int` expression. It may use the lengths of the parameters, positive
literals, `+`, `*` and `ilog2` (the base-2 logarithm, rounded down). The bound must hold at every
size, including zero. For example, if `ys` is empty the steps still grow with `xs`, so `xs.len() *
ys.len()` would be wrong, because it is zero when `ys` is empty. If a parameter is a function, you
can give it a name for the number of steps one call of that function takes, and the bound can use
that name:

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

`ply prove` reads both the `cost` bound and the `space` bound from the body of the definition, and
never proves them by running the code. It can read loops over a list, and loops over an `Int` that
moves toward a limit that the code checks. If it cannot read a bound, the result is `unproved`. This
happens for a bound over mutual recursion, and for a bound over a call of a function-typed parameter
that has no name for the number of steps of one call. A bound on a definition that performs effects
other than raises is an error (`E0463`), and a bound on a definition that may not return is error
`E0468`. If a bound cannot be read from the code, leave the bound out of the definition.

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

A codec is a value that encodes a type to a format and decodes it back, such as the JSON codec that
`derive json` generates. Codecs are plain values: `json::decode_bytes(body, order_json())`. A codec
for a recursive type takes a nesting limit, the greatest depth of nested values the codec accepts.
You cannot derive a codec for any of these types, which is an error (`E0206`): functions, `Task`,
`Chan`, `Secret`, types that take a resource label, and `Option<Option<a>>`. You also cannot derive
a codec for a type that has a field whose type has no codec.

`==`, `compare`, `digest` and `show` need no derivation; they work on any value whose type allows
them.

**SQL rows.** `derive sql` maps a record to a table row, one column per field, in field order. A
`String`, `Int`, `Bool`, `Float`, `Decimal` or `Bytes` field is a column of that type, an `Option`
is a nullable column, and a `List` of those is an array. Any other field is stored as JSON with its
type's own `json` codec.

**Writing a deriver.** A deriver is a function in a module with the signature `pub fn
derive(declaration: derive::Declaration) -> Result<String, derive::Refused>`. Its only allowed
effect is raising an error. The module `std.derive` defines the type `derive::Declaration`, which
describes the declaration the deriver is run on, and the type `derive::Refused`. The checker runs a
deriver once per declaration, with a 60-second limit, and caches the result. If the deriver fails,
or the code it generates does not pass the checker, the error is an error (`E0155`).

## 13. Builtins and the standard library

### 13.1 Builtins

Builtins are the functions every module can call without an import. `std.prelude` declares them,
and every module imports it automatically. `ply doc std.prelude` lists them all.

- Most are functions of a built-in type: `xs.len()`, `Map::new()`, `Bytes::at(b, i)`. `ply doc List`
  lists a type's functions.
- The rest have plain names, such as `range`, `min`, `panic`, `iterate`, `bracket` and `to_int`.
- An `extern fn` is a function whose body is part of the runtime, not written in Ply. Other
  builtins, such as `List::map` and most functions of `Map`, are ordinary functions written in Ply
  in `std.prelude`, the module that declares the builtins.

A builtin that can raise states in a `requires` the condition under which it does not raise. It
states in an `ensures` what is true of its result:

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
import its modules: `import std.json`. The library's own tests and its property checks (`law`
declarations) are part of `std`, not of your project.

A module named after a built-in type is that type's *home*: `std.list`, `std.string`, `std.map`,
`std.option`, `std.result`, `std.char`, `std.bytes` and `std.float`. Only the home module and the
prelude, the built-in definitions every module sees without an import, declare the type's functions.
Any module can call them without an import (`xs.sort()`). A few other names also need no import:
`print`, `eprint`, `read_line` and `Console` from `std.console`, `Set` from `std.set`, and `abs`
from `std.math`.

Each module is documented in its source:

- `ply std` lists the modules with a summary of each.
- `ply doc std.json` documents a module, `ply doc std.json.parse` documents one definition, and `ply
  doc fs.read_at` documents one operation of the `fs` effect.
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
| `--allow NAME` | a group of operations that need explicit permission. The program's source must also declare the group `NAME`, or the run fails with `E0459`. The groups are `machine`, `shipped` and `toolchain`. `machine` (`std.machine`) runs another compiled program. `shipped` reads the standard library that `ply` carries. `toolchain` compiles and links C. |

Using a label that no `--fs` gives a root is an error (`E0451`), and a path that leaves its root is
an error (`E0452`).

### 14.2 Tasks and time

Outside a `simulate` block, the program's own scheduler runs tasks. The entry point runs as the
first task, and the program ends once every task it spawned has ended. Ready tasks run in the order
they became ready. While a task sleeps or waits on the host, the other tasks keep running.

`clock.now` reads a monotonic clock, in nanoseconds since the run began, and `clock.sleep` waits on
that clock. Cancelling a task ends its sleep early. This lets one task set a deadline for another:
the first task cancels the second when the time is up, and the second stops sleeping at once. The
rest of the program keeps running meanwhile. `std.time`'s `time.sleep_ms` is different: it blocks
the whole thread, so no task runs while it sleeps.

### 14.3 Host handlers and tests

These effects have host handlers (§7.8), which are always available, whatever flags you pass:
`std.os`, `std.env`, `std.certgen`, `std.config`, `std.password`'s `kdf`, `std.dns`, `std.net`,
`std.udp`, `std.sqlite`, `std.term`, `std.signal`, `std.trace`, `std.time`, `std.random`'s
`entropy`, `std.process` and `std.fs`. Some host handlers would block the program's thread, such as
`kdf`, which hashes a password. A handler like that runs its work on the runtime's own threads
instead. The task that called the handler waits for the result, and other tasks keep running.

A test that uses the host is cached together with what it read and the permissions it ran with. A
cached pass is therefore never reused under other permissions. An operation inside a `simulate`
block never reaches the host (`E0425`). Under `ply run`, `std.process` (and `std.console`, which is
built on it) acts on the process that runs the program. A test has no such process, so using
`std.process` in a test is an error (`E0424`). The only use of `std.process` that a test may make is
starting the programs that the `--exec` flag grants (§14.1).

### 14.4 Configuration

`config.get[l](key)` and `config.secret[l](key)` read configuration, where `l` is the operation's
resource label (§7.8). They take values from these sources, highest priority first: `--set
KEY=VALUE`, `--config FILE` (lines of `KEY=VALUE`), the environment, and the schema's defaults. The
`config` effect is nondeterministic (`nondet`): its result can differ between runs. A test that
reads it is therefore a `test/nondet`.

`--config-schema MODULE.FN` names a function returning a `ConfigSpec`. A missing required key is
then `E0441`, a value of the wrong shape `E0442`, and an undeclared key the warning `W0607`. An
unreadable source is an error (`E0440`).

### 14.5 TLS

| flag | meaning |
| --- | --- |
| `--tls NAME=CERT,KEY` | a TLS credential: a PEM certificate chain, with the server's own certificate first, and its key. Repeatable. A listener can use the credential in two ways. It can call `net.listen_tls[l](port, "NAME")`, or it can call `net.serve_tls`. `std.http`'s `serve_pooled_tls` calls `net.serve_tls` to offer `h2` (HTTP/2) and `http/1.1`. You can give several certificates under one name. The server then picks one by the host name the client requests, sent in the TLS extension called SNI (server name indication). Changed files are reloaded within a second. |
| `--trust CERT.pem` | an extra trusted certificate, for `net.connect_tls` and `net.start_tls`. With any `--trust`, listeners also ask clients for a certificate, which `net.peer_certificate` returns. Repeatable. |
| `--mtls NAME` | listeners using the credential `NAME` require a client certificate that `--trust` verifies. Repeatable. |

A credential that does not load is an error (`E0430`), and naming one that was not given is an error
(`E0429`).

### 14.6 Stopping and draining

`ply run` listens for `SIGINT` and `SIGTERM` when the program uses `std.signal` or accepts
connections. On a stop signal, listeners close, so a waiting `net.accept` returns `0`:

- `--drain-lead-ms MS` keeps accepting connections for this long first (default 0);
- `--drain-ms MS` sets how long requests already in flight may keep running after the drain lead
  time ends (default 30000). If this time runs out first, `ply` reports `W0608` and exits with
  status 3.

`std.http`'s `serve_pooled` drains this way: it accepts nothing more, finishes the connections it
had accepted with `Connection: close` on the last response, and closes idle ones. An HTTP/2
connection gets `GOAWAY`. Under load, it responds with `503` and `Retry-After` to a connection that
finds its queue full.

A program that starts child processes and does not drain ends its children and exits at once when it
receives `SIGINT` or `SIGTERM`, with status 130 or 143. Draining means waiting for requests in
flight to finish, as described above. A program that does not start child processes and does not
drain also exits at once when you press Ctrl-C.

### 14.7 Tracing

`std.trace` records spans, which are timed units of work. It writes them to the output that `--trace
json|text|off` chooses (default `json`, as lines on standard error), at or above the level
`--trace-level debug|info|warn|error` sets (default `info`). Leaving a span open when the program
ends is an error (`E0445`).

### 14.8 Databases

A SQLite database is opened under a file system root, which a `--fs` flag grants (§14.1). `std.db`
fails a statement in two cases. The first is a statement that SQLite rejects. The second is a
statement that uses a table that the resource label at the call does not name. The label is the one
written at the call site, such as `users` in `db.get[users](3)`.

**SQL checks.** Before a program runs, `ply` checks each SQL statement in the root package, the
package you run, against the schema (§4.5) that the statement's module uses. It checks these
operations: each `db.query`, `db.execute` and similar operation, `std.db`'s `fetch`, and
`std.sqlite`'s `query`, `execute` and `run`. A statement is checked when it can be computed before
the program runs: a string, strings joined with `++`, or a call of a function on such values. Errors
are reported at the call:

| code | meaning |
| --- | --- |
| `E0431` | the statement does not parse |
| `E0433` | a table or column the schema does not declare |
| `E0435` | the statement does not use the table named by the call's *resource label*. For example, `db.get[users](3)` must run a statement that reads or writes the table `users` |
| `E0436` | the wrong operation: a write run as a query, or a read run as `execute` |
| `E0437` | the columns a `fetch` returns do not match the row type's fields (§12) |
| `E0438` | the parameters do not match the placeholders, or a parameter's type does not match its column |
| `E0447` | the statement selects columns with `*`, which means all columns. The check rejects it, so name each column |
| `E0470` | a construct the check does not follow, such as `with` or a subquery in `from` |

`std.db` applies the same rules when the program runs, to a statement that the check could not
analyse. A module whose statements use two different database schemas is an error (`E0471`).

### 14.9 `ply hosts`

`ply hosts` lists every operation the program can perform on its host, written `effect.op[label]`.
For each operation it shows five things:  - the handler that runs the operation; - whether the
operation is deterministic, meaning it always returns the same result for the same input; - whether
the operation may run more than once; - whether the operation blocks, meaning it may wait for an
outside event before it returns; - whether the operation may receive a `Secret`.  It also shows the
file system, TLS, database, configuration, tracing and shutdown settings of the run. `--digest`
prints a single line that starts with `b3:`. The line is a BLAKE3 digest of the whole list.

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

The body of the `ply.pkg` function is data, not code: it may contain only literals, constructors,
records and lists. Anything else is an error (`E0130`). A `ply.pkg` whose body is not a `Manifest`
value is an error (`E0129`), and a field with an invalid value is an error (`E0131`). A project with
no `ply.pkg` is the *anonymous package*.

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

Importing an undeclared package is an error (`E0132`). Two packages with one prefix, or a module of
the root package using a dependency's prefix, is an error (`E0133`). Packages that depend on each
other in a cycle are `E0134`.

A command works on the root package. A dependency's `main` is never an entry point, and `ply test`
and `ply prove` cover only the root package. `--workspace` (on `check`, `test` and `prove`) also
runs the command on every package the root depends on through path dependencies, dependencies
first.

### 15.3 Versions and the lock file

`ply` uses minimal version selection. Each package states a minimum version of each dependency, and
the highest of those minimums is the version `ply` picks. One version of each package serves the
whole program (`E0137` names a package found at two different places). Over a registry, `ply` picks
the lowest published version that is at or above the minimum and has not been yanked. A *yanked*
version is one the registry owner has marked so that new resolutions skip it (see Publishing).

`ply build` records the resolved versions in `ply.lock`, next to `ply.pkg`, with a BLAKE3 digest of
each dependency's sources. A path dependency inside the same git checkout has no digest, since the
commit already pins it. Before writing anything, a build checks the lock: a dependency whose sources
no longer match is an error (`E0138`). To accept a changed dependency, run `ply resolve`.

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

`ply build` writes a `.plyx` file that contains everything the program needs to run from an entry
point (default `main`). The `.plyx` contains:  - the definitions the entry point uses, identified by
hash and stored as source without tests, laws (properties the program states about its definitions)
or comments; - the compiled code of those definitions.  Running a `.plyx` checks nothing. The
`.plyx` has a BLAKE3 digest of that content. An edit to a definition the program does not use is not
part of the content, so the digest stays the same. A `.plyx` also contains the standard library
definitions the program uses, so any other `ply` runs it as built. A built program reports failures
without line numbers.

A `.plyx` whose contents do not match its digest is an error (`E0443`). A `.plyx` built by another
compiler, or for another runtime, is an error (`E0444`): rebuild it.

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

`PLY_REGISTRY` is the registry's base URL, `https://host[:port][/prefix]`. `ply` connects to the
registry over TLS. It accepts the server's TLS certificate only if one of two things signed it: a
root certificate built into `ply`, or a certificate listed in `PLY_TRUST`. `http://` is accepted
only for a registry on the same machine, because a publish sends a token. If `PLY_REGISTRY` is unset
or the registry is unreachable, the error is an error (`E0141`). If the registry does not have the
requested name or version, the error is an error (`E0143`).

### 16.4 Publishing

`ply publish` builds the library's `.plyz` and sends it to `PLY_REGISTRY`, with the token in
`PLY_REGISTRY_TOKEN`. Only a library whose dependencies all come from the registry can be published
(`E0145`). The registry rejects different bytes under a version it already has (`E0144`): a
published version never changes.

**Semantic versioning is checked.** The *contract* of a public definition is its signature and its
clauses (the conditions the definition states about itself). If the definition is `transparent`,
meaning callers can see its body, the body is part of the contract too. `ply publish` compares the
contract of each public definition with the contract in the highest published version below the new
one. A patch release may change no contract. A minor release may only add definitions. A major
release may change anything. If the new version number is too low for the changes, the error is
`E0150`. It names the change and the smallest correct version. `ply contracts NAME FROM TO` lists
the contracts that changed between two published versions.

**Interfaces.** After `ply publish` sends the archive, it also sends the package's *interface*. The
interface lists each module's public names with their signatures and hashes. It is sent under the
*semantics version* of the `ply` that published the package. The semantics version changes only when
the meaning of hashes, effects or claim results changes. A claim result says whether a claim was
proved, tested or failed. `--verify-dependencies` (on `check`, `test` and `prove`) rejects a
dependency whose interface does not match its source (`E0149`).

**Attestations.** `ply attest NAME VERSION` downloads a published version, then checks, tests and
proves it, and reports the result. With `--sign KEY`, it signs the result and sends it to the
registry. The signed result is an *attestation*: a statement, signed by a party called an
*attester*, that the version passed. The registry stores the attestation only if the registry's
configured attester signed it. `ply resolve` trusts an attestation only when a key in
`PLY_ATTESTERS` signed it.

**Yanking.** `ply yank NAME VERSION` marks a version yanked. New resolutions skip a yanked version,
but a lock that pins it still uses it. Nothing is ever deleted.

**Running a registry.** The registry is a Ply program, `packages/registry`:

```
$ ply run packages/registry --fs store=/srv/ply \
    --tls registry=cert.pem,key.pem --set tls=registry --set port=8443 \
    --config tokens.conf
```

`store` is the directory where the registry keeps its files. `port` is the port to listen on
(default `8080`). `tls` is the TLS certificate and key to serve with. Without `tls`, the registry
serves plain HTTP. Each package's publishing token is set by the configuration key `token.<name>`.
The registry locks a package around every write, so two registries can share one store.  The
registry also signs an attestation for every published version. To enable this, pass all four flags:
- `--set attester=<public key>` sets the public key that identifies the attester; - `--set
attest.key=<key file>` sets the file that holds the key to sign with; - `--set url=<its URL>` sets
the registry's own URL; - `--exec attest=<a ply>` sets the path of a `ply` executable that the
registry runs to produce each attestation.

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

`ply build --sign KEY` writes a signature file next to the artifact it builds. `KEY` is the path of
a secret key file made by `ply keygen`. The signature covers the artifact's kind, name and digest,
the version of `ply` that built the artifact, the semantics version of that `ply`, and the git
commit of the artifact's sources. `ply run --require-signer KEY` runs an artifact only if one of the
given keys signed it (`E0460`). `ply build --verify` builds the artifact in memory and compares the
result with the file that `-o` names. `ply build --verify` also checks each signature (`E0461`).

## 17. The `ply` command

`ply [--color auto|always|never] <command> [path] [options]`

### 17.1 Output and exit status

**JSON.** Every command takes `--json`, and then prints exactly one JSON object on standard output.
Progress lines, such as each test result, go to standard error, so a run that is stopped still
shows what it finished.

**Color.** `--color auto` (the default) decides in this order, and the first rule that applies wins.
If `FORCE_COLOR` is set, it uses color. Otherwise, if `NO_COLOR` is set, it uses no color.
Otherwise, if `CLICOLOR_FORCE` is set, it uses color. Otherwise it uses color only on a terminal
whose `TERM` is not `dumb`.

| exit status | meaning |
| --- | --- |
| 0 | success |
| 1 | a test failed, or `main` raised |
| 2 | the program did not run: a bad path, a syntax error or a type error |
| 3 | the program was shutting down and requests were still in progress when the deadline for finishing them passed (§14.6) |
| *n* | `process.exit[p](n)` under `ply run`: the program's own status, `0` to `125` |
| 130, 143 | `SIGINT` or `SIGTERM` ended a `ply run` |
| 141 | the program tried to write output after the output pipe was closed, as when output is piped into `head` and `head` exits, and the program did not handle the `pipe.broken` effect, which reports a closed output pipe |

### 17.2 Environment variables

| variable | meaning |
| --- | --- |
| `PLY_CACHE_UPSTREAM`, `PLY_CACHE_UPSTREAM_READONLY` | a shared second cache (§1.5) |
| `PLY_REGISTRY`, `PLY_REGISTRY_TOKEN` | the package registry and its publishing token (§16) |
| `PLY_ATTESTERS` | public key files that `ply resolve` trusts to vouch for packages, separated as in `PATH` |
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
| `ply check [path]` | `--types` (each definition's type and effects), `--costs` (where an update to a value copies it instead of changing it in place, §6.10), `--explain` (with `--types`, also why a function may raise or not return), `--workspace`, `--verify-dependencies` |
| `ply test [path]` | `--filter`, `--shard`, `--jobs`/`-j`, `--timeout`, `--no-cache`, `--explain`, `--watch`, `--coverage` |
| `ply run [path] [-- ARGS]` | `--seed`, `--timeout`, `--profile`, `--explain`, `--require-signer KEY`, and the flags for choosing the program's hosts (the handlers that supply its real effects), for tracing, and for draining on shutdown. `ARGS` is what `process.args` returns. A `.plyx` path runs a built program. |
| `ply prove [path]` | `--filter`, `--jobs`, `--prove-budget`, `--timeout`, `--no-cache`, `--no-incremental`, `--explain`, `--static-prover`, `--workspace`, `--verify-dependencies`, simulation flags |
| `ply review [path]` | `--changed` (default), `--accept`, `--no-cache`, `--no-incremental`, `--prove-budget`, `--timeout`, simulation flags |
| `ply build [path]` | `--entry NAME`, `-o FILE`, `--config-schema`, `--digest`, `--diff OLD.plyx`, `--sign KEY`, `--verify` |
| `ply hosts [path]` | the flags for choosing the program's hosts (the handlers that supply its real effects), for tracing, and for draining on shutdown, and `--digest` |
| `ply doc NAME [path]` | for a definition: its signature, doc comment, clauses (the separate cases of its body), location, hash and the laws (the properties declared about it) that use it. For a type: its fields or variants and its functions. For an effect: its operations. For a module: its exports. |
| `ply std` | the standard library's modules. `--show [NAME]` prints a module's source; `--digest`. `--answers DIR` and `--answering` are options that only the bootstrap passes to `ply std`; do not use them yourself. |
| `ply explain CODE` | what a diagnostic code means; `--all` lists every code |
| `ply fmt [paths]` | formats files in place; `--check` changes nothing and exits 1 if a file would change; `--json` needs `--check` |
| `ply rename --map FILE [paths]` | renames definitions, variables, fields, operations and modules (§17.6) |
| `ply show NAME [path]` | prints one `fn` or `type` as its file has it |
| `ply replace NAME [path]` | replaces one `fn` or `type` with `--with FILE` or standard input (§17.7) |
| `ply hash [path]` | each definition's hash; `--dependencies` adds what each refers to |
| `ply definitions [path]` | every definition: location, hash, signature, effects and references; `--filter` |
| `ply callers DEF [path]` | what refers to the definition named `DEF`, and every definition, test and law (a property that `ply test` checks) that depends on it |
| `ply unused [path]` | `--workspace`, `--json`: definitions nothing uses, and those only tests and laws use; in a package with `main`, `pub` does not count as a use, and a leading `_` opts a name out. Under `--workspace`, every package `ply.work` names is judged together, except the standard library |
| `ply resolve [path]` | writes `ply.lock`, downloading registry dependencies |
| `ply vendor [path]` | copies dependencies into `vendor/` |
| `ply why NAME [path]` | why a package is a dependency |
| `ply publish [path]` | publishes a library (§16.4) |
| `ply yank NAME VERSION` | marks a published version yanked |
| `ply attest NAME VERSION` | checks, tests and proves a published version; `--sign KEY` |
| `ply contracts NAME FROM TO` | what changed in the public interface between two published versions |
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

Every program that passes the checker compiles. If Ply fails to compile a definition, that is a bug
in Ply, and Ply reports it as `E0505`.

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

A line of a rename map can rename only some of the names that share its old name:

| line | renames |
| --- | --- |
| `packages/cli/build.ply Made BuildOutput` | only what that file declares under the name, and its uses |
| `packages/std/fs.ply fs.read_file load_file` | one operation of an effect the file declares |
| `packages/shop/order.ply Order.held quantity` | one field of a record type the file declares |
| `packages/shop/order.ply:40 held quantity` | the parameter or local declared on line 40, its uses, and the named arguments of a parameter |
| `.held cached` | the record field `held`, renamed to `cached` in every record that has it |
| `packages/app/old_name.ply new_name` | a module: the file moves, and imports follow |
| `packages/compiler/emit/old/ new` | a directory of modules |

A rename type-checks the program, because the types say which record a field belongs to and which
function a call on a value means. A map of only parameter and local lines needs no types, so it
skips the type check and runs on a program that does not check.

Records are structural: two record types with the same field names are the same type. A record type
declared with `new` is not structural. Because a structural record type has no identity beyond its
field names, `ply rename` identifies a record by its field names. A rename of a field applies to
every record that has the same field names: literals, reads, updates, patterns and written record
types. When two declared types have the same field names, the rename map must rename the field in
both. For a record type declared with `new`, `ply rename` renames the field only in that type.

`--words` renames words inside names instead: `ty type` changes `ty_vars` to `type_vars` and `TyCon`
to `TypeCon`. A name inside backticks in a comment is renamed too. A string is renamed when its
whole text is the full dotted name of a renamed definition, such as `"app.store.total"`. A string is
also renamed when its whole text is the full dotted name of a definition in a module that the rename
moves. `ply rename` never changes any other text in a string. It reports a warning, and leaves the
string unchanged, for a string whose whole text is a renamed name but not a full dotted name, and
for a string that contains a word `--words` renames.

`ply rename` changes nothing and exits with code 2 in either of two cases. In the first case, the
program does not check before the rename. In the second case, the rename would change what a name
refers to. Examples are two definitions that would get the same name, a local variable that would be
hidden by another local variable, a call on a value that could then call a different function, and
code that a `derive` line generates for a type that uses a name the rename removes. These checks
ensure that the renamed program has the same types, so `ply rename` does not check the program again
afterwards. `ply rename` changes only the files under the paths you give it. It stops if another
file of the package uses a renamed name. `--check` reports what would change without writing any
file.

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

- No loops, `break` or `return`. To repeat work, use recursion or `iterate`, the standard function
  that applies a step again and again (§6.6).
- No mutable variables, except the variables that a handler declares to keep state between the
  operations it handles (§7.6).
- No exceptions. A function that can fail declares the failure in its signature, as an *effect* (an
  operation the function may perform, such as raising an error). The caller must handle that effect
  or declare it too (§7.7).
- No type classes and no implicit arguments. Method calls are never looked up while the program
  runs: when you write `x.f(y)`, the compiler picks the function `f` from the type of `x` at the
  place where the call is written (§6.4).
- No modules as values, and no first-class effects.
- No `unsafe` and no foreign function interface. Everything a program does outside itself is an
  effect. The builtins, the functions the language provides without Ply source, are implemented only
  by the runtime, so a program cannot add its own.
- Ply does not reclaim the memory of values that refer to each other in a cycle.
- A task, one unit of concurrent work, always runs on a single thread. Only the branches of
  `parallel` run on other threads.
- No file handles. `std.fs` reads a given range of bytes from a file named by its path, and appends
  to a file named by its path, without opening it first.
- No backpressure, database migrations or authentication framework.
- No HTTP/2 over `listen_tls`, no `Upgrade: h2c`, and no server push.
- An expression may nest at most 128 levels deep.

Sharp edges:

- `x.f(y)` where `x` is not a variable is an effect operation, not a function call (§6.4).
- If no handler handles an operation, the compiler does not reject the program before it runs. The
  error happens while the program runs, when the operation reaches the host, which is the world
  outside the program (`E0424`). One case differs: an operation whose effect is `nondet` has a
  result that varies between runs, such as reading the clock. A deterministic test must give the
  same result every run, so the compiler rejects a `nondet` operation in such a test before the test
  runs (`E0412`).
- A record update `{ base with ... }` needs the type of `base`, the record being copied, to be known
  where the update is written (§6.5).
- `Bytes::at`, `String::slice`, `String::find`, `List::set`, `Array::get` and `Array::set` raise
  when out of range, but `List::at` and `Array::at` return `None`.
- In a simulation, two tasks that allocate memory always run in one fixed order relative to each
  other, never at the same time (§9.2).

## 20. Examples

The `examples/` directory has complete programs:

- `clock.ply`: declares an effect marked `nondet` (its result varies between runs), a handler that
  implements the operations of that effect, and a test that runs with `test/nondet`.
- `ledger.ply` and `report.ply`: modules, an `opaque` type (a type whose representation other
  modules cannot see), contracts (conditions a function requires and guarantees) and laws
  (properties that tests check).
- `pipeline.ply`, `bank.ply` and `timeout.ply`: simulation, a race and its fix, and a virtual clock.
- `echo.ply` and `hello.ply`: sockets and an HTTP endpoint.
- `orders.ply`: `derive json`.
- `relay.ply`: defines one function that forwards a call and works with any *resource label*. A
  resource label names which resource an operation uses. The caller chooses the label, and the
  function performs its writes using that label.
- `shout.ply`: a command line declared with `std.cli`.
- `store.ply`: a handler that grants a capability.
- `agreement.ply` and `fake_divergence_audit.ply`: `std.db`'s in-memory fake checked against
  recorded PostgreSQL results.
- `service.ply`: a service over PostgreSQL or its in-memory fake, with configuration, TLS, tracing
  and graceful shutdown.
