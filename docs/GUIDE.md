# The Ply Guide

Ply is a general-purpose, statically typed, effect-tracked functional language.
It has no loops,
mutable variables, classes, exceptions or dispatch. A function's type says which
resources it touches, the unit of compilation is the content-hashed
*definition*, and the test runner re-runs exactly the tests whose hash changed.
This guide is the reference for writing Ply and using the `ply` command.

## 1. Getting started

Build with `cargo build --release -p ply-launcher --bins`, then `cargo pack
target/release/ply` to append the shipped modules and the `ply` program to it, and
put `target/release/ply` on your path. After editing Ply sources in the checkout,
`cargo pack` again is the whole rebuild. A Ply file is a module:

```ply
// hello/main.ply
fn greeting() -> String = "hello from ply"

fn main() -> Unit = assert_eq(greeting(), "hello from ply")
```

`ply run hello` evaluates `main`, prints the value it returned (`()`) and exits
`0`. There is no `print`: output is the `std.process` effect (`ply doc std.process`). The everyday commands are
`ply check` (parse, resolve, typecheck, infer rows), `ply test` and `ply run`.
Each takes a `.ply` file or a project root, defaulting to `.`.
`ply check --types` prints every definition's inferred signature, each atom of
its row marked with how many times a call performs it (§6.2), and, under one
whose row says `diverges`, why its calls may not return (§5.10).

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

**The cache.** `.ply-cache/` at the root holds the store — what the front end
filed for each file, the tests' passes and baselines, the discharged
obligations and the review baselines, in one data file (`store.dat`) found
through one index (`store.idx`) — the compiled package the project's loads
read their dependencies and the shipped modules through, and what `ply check`
kept of the project's own modules (`interfaces/`, §16),
and the git dependencies that were fetched;
`vendor/` holds the ones `ply vendor` copied, which is what a checkout that must
not reach the network carries. It is safe to delete (`ply cache clear` discards
the store and the compiled package); add it to `.gitignore`. A run that files
into the store compacts it, as `ply cache compact` does, once more than half of
`store.dat` holds entries a later filing replaced or dropped.
`PLY_CACHE_UPSTREAM=DIR` names a second
cache shared between checkouts and machines, a directory on any storage they all
reach: the passes and discharged obligations found there count here, and this
run's are published there (`PLY_CACHE_UPSTREAM_READONLY=1` reads only). Entries
are keyed by content and by the shape of what is stored, so nothing
machine-specific is ever shared; `--no-cache` ignores it. What the front end
filed is believed only by a `ply` whose evaluator is the same and whose own
definitions that analyse a program and file the answer hash as they did, and a
pass or a discharged obligation only by one whose runtime is the same and whose
definitions that make a unit and decide a verdict hash as they did: another
build files them again (`W0603`), and an upstream answers only builds of its
runtime. A pass is filed as well under the code its test compiled to, believed by a build
whose runtime and definitions that run a test and file its pass hash as they did,
so a compiler change that leaves a test's code as it was does not run it again.
A hash covers what its definition reaches and no comment or
layout (§8.2), so the rest of `ply`, its other commands among it, is in none of these.
What the front end filed for a definition is taken again while the definition's
own text and what it reads of each definition and declaration it references
stand: the signature and specifications written there, how its calls end, and
what it performs or answers that a caller's check counts by. An edited body is
checked again, and what references it only where one of those moved. A dependency's own
modules are keyed by its manifest rather than by where it sits, so moving or
re-checking-out a dependency keeps what was cached for it.

## 2. Lexical structure

### 2.1 Source and identifiers

Source is UTF-8; whitespace only separates tokens and there is no layout rule.
A comment is `//` to end of line.

A *doc comment* documents a declaration for whoever calls or names it. `///`
lines document the `fn`, `extern fn`, `type`, `effect`, effect operation, `effect
set`, `law schema`, variant, or field of a declared record directly below them;
`//!` lines at the head of a file, before its first import or item, document the
module.
`////` is a plain comment.

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

Each line's words follow the marker and one space; a bare `///` separates
paragraphs, and the first sentence is the doc's summary. A doc says what the
signature cannot — what an answer means, when the function raises, units, edge
cases — and holds no examples: the tests and laws that name a definition are
its examples, and `ply doc` lists them (§16). A blank line between a doc and its
declaration does not part them, and `ply fmt` removes it. A doc comment that
documents nothing — one inside a body, above an import, a `test`, a `law`, a
`derive`, a `key`, a `show`, a `gen` or a `numeric`, or at the end of a file, or a `//!` line below
the head of its file — is `E0003`. A doc is trivia like any comment: it moves no hash, key or cached
answer.

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
| `raise` | opening an operation declaration (§6.8) |
| `set` | `effect set X = {..}` |
| `new` | right after the `=` of a `type` declaration |
| `opaque` | before `type` at item position (§3.2) |
| `law`, `host`, `schema`, `forall`, `cost` | `law "..."`, `law/host` or `law schema <name>` at item position; `forall` after a law's label or a schema's parameters; `cost` after a law's binders and guard, among a `fn`'s `requires` and `ensures`, or after a `fn` parameter's function type |
| `bounded` | after an atom or the row variable of a definition's row (§6.2) |
| `derive`, `for`, `reuse`, `transparent`, `const` | `derive <deriver> for <Type>`; `reuse fn`, `transparent fn`, `transparent reuse fn` and `const fn` at item position |
| `for`, `in` | after a test's label: `test "..." for <name>: <Type> in <table>` (§8.1) |
| `key`, `show`, `gen`, `numeric`, `by` | `key for <Type> by <function>`, `show for <Type> by <function>`, `gen for <Type> by <function>` and `numeric for <Type> by { <operation>: <function>, .. }` at item position (§4.4) |
| `where`, `derivable` | after a signature's row, or after a law's binders |
| `returns`, `fresh` | between a `fn` header and its specifications |
| `requires`, `ensures` | between a `fn` header and its body |
| `resume`, `return` | in a handler clause (§6.5, §6.6) |
| `with_cell` | before `[` |
| `simulate` | before `{` where an expression can start |
| `try` | before `{`, or before `[` and an operation's name, where an expression can start (§6.8) |

### 2.3 Literals

| form | type | notes |
| --- | --- | --- |
| `42`, `1_000_000`, `0xFF` | `Int` | 64-bit signed; `_` between digits. Hex is bounded as a 64-bit pattern, so `0xFFFF_FFFF_FFFF_FFFF` is `-1`. |
| `255u8`, `0x6A09_E667u32`, `-1i8` | fixed width | Suffix `u8` `u16` `u32` `u64` `u128` `i8` `i16` `i32` `i64` `i128`. Decimal spellings are bounded by range (`256u8` is `E0211`), hex by width (`0xFFu8` is 255). |
| `1.5`, `1e9`, `2.5e-3` | `Float` | IEEE-754 binary64. |
| `1.50m`, `0m` | `Decimal` | Exact base 10; up to 28 fractional digits, 96-bit mantissa; keeps its written scale. |
| `"text"` | `String` | UTF-8; no line breaks. |
| `f"n = {n + 1}"` | `String` | Interpolated: each `{expr}` hole is what `std.show.display` writes of it, below. |
| `\\text` | `String` | A line string: lines of verbatim text, below. |
| `b"GET "` | `Bytes` | ASCII characters plus `\xNN`. |
| `b'{'`, `b'\n'`, `b'\x1f'` | `Int` | The byte `bytes_at` answers: one ASCII character or one escape. |
| `uuid"6ba7b810-.."`, `html"<b>{x}</b>"` | what its tag answers | A tagged literal: a text its tag's parser read when the program was checked, and holes its tag is handed apart from the text, below. |
| `'a'`, `'\n'`, `'\u{1F600}'` | `Char` | Exactly one character, or one escape. |
| `#{"a": 1, k: v}` | `Map<k, v>` | Keys are expressions; a later entry for a key replaces an earlier one. |
| `#[1, 2]` | `Map<a, Unit>` | A set: each element a key whose value is `()`. |
| `true`, `false` / `()` | `Bool` / `Unit` | |

`1`, `1.0`, `1m` and `1u32` have four types and never convert implicitly
(`fn f() -> Int = 1.0` is `E0201`). A literal is never negative: `-3` is unary
minus, except in a pattern. The smallest signed value of a width cannot be
written as a literal; use `i8_of_int(-128)`.

String and character escapes are `\n` `\t` `\r` `\0` `\\` `\"` `\'` and `\u{...}`, one to six hex
digits naming a Unicode scalar value (a surrogate or a value past `10FFFF` is `E0211`). Byte
strings add `\xNN`, refuse `\u{...}` and refuse source characters above `U+007F`.

A byte literal `b'x'` holds one byte, written as a byte string writes it or as `\'`, and is the
`Int` that byte is: `b'{'` and `123` are one literal with one hash, so
`bytes_at(src, i) == b'{'` reads as it means and a scanner is a `match` (§5.2). An empty one, one
holding more than a byte, or one that never closes is `E0001` or `E0002`, as for a character.

An interpolated string `f"a {x} b"` is the concatenation `"a " ++ display(x) ++ " b"`, with
`std.show`'s `display`: a `String` or a `Char` goes in as itself and any other value as `show`
writes it (`std.show`). The two spellings are one definition with one hash. A hole is any expression,
strings and braces included, and `{{` and `}}` are braces of the text; a lone `}` is `E0001`. A
hole's type must be `derivable(show, ·)`, so a `Secret`, a function, a `Cell`, a `Task` or a `Chan` in one
is `E0206`. A module that interpolates imports `std.show` itself, under a name no source can
write, so `std.show` and the modules it imports cannot interpolate.

A map literal is the call `map_of_entries([{key: k, value: v}, ..])` and a set literal the same
with `()` for each value, so either spelling is one definition with one hash; `ply fmt` keeps the
one written.

A tagged literal `tag"text"` is a name touching a string: `uuid"..."` is one, and `uuid "..."` a
name and then a string. It is the call `tag::literal("text")`, where `tag` is a module binder in
scope (§3.2) and the text an interpolated string's, escapes and all, with `{{` and `}}` for its
braces, and a load runs that call before anything else runs (§16):

```ply
import std.uuid
import std.uuid (Uuid)

fn dns() -> Uuid = uuid"6ba7b810-9dad-11d1-80b4-00c04fd430c8"
```

`literal` is a `pub fn` of one `String` answering `Result<T, {message: String, offset: Int}>`,
with no type, row or label parameter and a row that holds nothing but raises (§6.8); anything else
is `E0152`, as is one in a recursion with the definition that holds the literal; a tag no import
binds is `E0106`, and a module with no `literal`, or a private one, `E0101` or `E0107`. Where it
answers `Ok(v)` the literal has type `T`, is `v`, and performs nothing: what the call could raise,
the check saw it not raise. Where it answers `Err`, the literal is `E0153`, placed at character
`offset` of the text as the source spells it, an escape or a doubled brace being one character, or at the closing
quote for the offset just past the text, with `message` beside it. A parser that raises, or that
makes more than ten million calls, is `E0154`.

The literal hashes with its parser and its text, so an edit to anything the parser reaches moves
every definition that writes the tag, and apart from the call written out, which has another type
and another row. `f` and `b` are no tags, since `f"` and `b"` open the two strings above. A tagged
literal is an expression: no pattern, label or parameter default (`E0121`) is one. It runs as
`let Ok(v) = tag::literal("text")`, so a module that binds its own `Ok` holds none (`E0118`), as
one that binds an `Ok` or an `Err` holds no `try` (§6.8). `std.uuid`, `std.base64` and
`std.bigint` are tags (`ply doc std.uuid.literal`).

A tagged literal takes holes as an interpolated string does, each `{expr}` any expression, and
its tag is handed the values apart from the text, so it binds, escapes or quotes each by where
it stands and nothing a hole holds is read as text:

```ply
import std.html
import std.html (Html, text)
import std.sh

fn row(name: String, kind: String) -> Html = html"<li class={text(kind)}>{text(name)}</li>"

fn search(pattern: String, file: String) -> List<String> = sh"-n --color=never {pattern} {file}"
```

With holes it is the call `tag::fill(c, [h0, h1, ..])`, where `c` is what
`tag::compile(["t0", "t1", ..])` answered `Ok` of for the texts around the holes, one more than
there are holes. `compile` is a `pub fn` of one `List<String>` answering
`Result<C, {message: String, offset: Int, part: Int}>`, and `fill` one of a `C` and a `List<H>`;
each has no type, row or label parameter, a row that holds nothing but raises and no recursion
with the definition that holds the literal (`E0152`). A load runs `compile` as it runs a
`literal`, on the texts alone: its `Err` is `E0153` at character `offset` of text `part`, a brace
written twice being one character, and the offset just past a text is the hole that follows it,
or the closing quote after the last. Each hole is checked against `H` (`E0201` at the hole; a
record literal in one is a `new` record where `H` is one, §4.2). The literal has the type `fill`
answers, performs what its holes perform and raises what `fill` raises; it hashes with
`compile`, `fill`, its texts and its holes, and runs as `let Ok(c) = tag::compile([..])` and then
the call of `fill`. A literal with no hole calls `literal` and one with any calls `compile` and
`fill`: a tag declares either or both, and a literal of a kind its tag does not declare is `E0101`.
`std.html` and `std.sh` are tags with holes (`ply doc std.html`, `ply doc std.sh`).

A line string is a run of lines that each start with `\\`, led only by blanks.
Everything after the `\\` to the end of its line is text, verbatim: nothing is
an escape, so quotes, backslashes and `\\` itself are written as they read. The
lines join with `\n` and the last one ends the value, so a value ending in a
newline ends with a line holding only `\\`:

```ply
fn program() -> String =
  \\fn main() -> Int = 42
  \\
```

A line ending in a space or a tab is `E0001`, since that whitespace cannot be
seen; a `\r` before a line's newline is not text. A line string is an
expression, never a pattern or a label. Nothing can follow it on its last line,
so what comes after it goes on the next one.

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
| 9 | `+%` `-%` `+\|` `-\|` | integer |
| 10 | `*` `/` `%` | numeric |
| 10 | `*%` `*\|` | integer |
| — | prefix `-` `!` `~` | numeric / `Bool` / integer |
| — | postfix `f(x)` `r.field` `e.op[r](x)` `e?` | |

* `==`/`!=` are structural at every type except functions, and a type that
  states a `key` is compared through it (§4.4). `Float` equality is
  IEEE, so `NaN != NaN`. `<` `<=` `>` `>=` work on numeric types and on `Char`,
  by scalar value, and on a type that states `numeric` and a `key`, by its key
  (§4.4); order anything else with `compare`. Arithmetic on a `Char` is
  `E0201`: go through `int_of_char`.
* Both operands have one type; there is no widening (`U8 + U16` is `E0201`).
* `+`, `-`, `*` and prefix `-` at a sum whose module states `numeric` for it are
  the functions it names (§4.4). They perform nothing and cannot raise, and a
  literal beside one is still an `Int`: `a + 1` is `E0201`.
* Arithmetic is checked. A `/` or `%` whose divisor is zero raises (§6.8), so
  either puts `abort.raise` in the row unless its divisor is a literal other
  than zero or its operands are `Float`s. Overflow and a shift count that is
  negative or not less than the type's width are the machine's limit, as the
  call ceiling is: they end the run (`E0502`) and are in no row. `<<` discards
  shifted-out bits and `checked_*` answer `None` (§12).
* `+%`, `-%` and `*%` wrap at the operand type's width, and `+|`, `-|` and `*|`
  saturate at it: `255u8 +% 1u8` is `0u8`, `250u8 +| 10u8` is `255u8` and
  `3u8 -| 10u8` is `0u8`. At `Int` the width is its 64 bits and the ends its
  least and greatest values. They perform nothing, cannot raise and never end
  the run. Each is the call of a builtin, built at the parse, so the two
  spellings are one definition with one hash: `a +% b` is `wrap_add(a, b)`,
  and `wrap_sub`, `wrap_mul`, `saturating_add`, `saturating_sub` and
  `saturating_mul` are the other five. Their operands are one builtin integer
  type, which the call site settles: a type parameter, even under
  `where integer(a)`, is `E0210`, any other type `E0201`, and a type that states
  its own arithmetic (§4.4) `E0223`. A `%` or a `|` touching the `+`, `-` or
  `*` before it is the one operator, so a lambda that is an operator's right
  operand takes a space: `a + |x| x`.
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
`nondet effect`, `effect set`, `test`, `law`, `law schema`, `derive`, `key`,
`show`, `gen` and `numeric`, in any order.
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

`reuse fn` promises that every `push` in the body reuses its list (§5.6), and
`transparent fn` puts the body into what another package reads of it (§10). The
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
`ply.pkg` (§3.3) grants its own prefix, and `import cli.surface` reaches the
`surface` module of the package `cli`. A path whose first segment is neither a
module of this package, the root of one, nor a granted prefix is `E0106`; a
dependency's prefix used without the manifest declaring it is `E0132`. A bare
import is always this package's own: inside a dependency, `import fmt` names
*its* `fmt`, never the importing package's — a package cannot reach back into
what imports it.

Items are private unless `pub` (`E0107`). `pub` applies to `fn`, `type`,
`effect` and `law schema` only. Values (functions, constructors and the
definitions a schema declares, §10), types, effects and module binders are
separate namespaces, so `fn size`, `type Size` and `effect size` coexist.

A `pub type` exports a sum's constructors with it, and a `new` record (§4.2) is
built by any module that writes a literal where one is expected. `opaque` before
`type` keeps the building to the declaring module, so a value of the type is
one that module's functions answered, and whatever they hold of it holds:

```ply
pub opaque type Token = | Token(String)
pub opaque type Date = new { year: Int, month: Int, day: Int }

pub fn mint(s: String) -> Option<Token> = if s == "" { None } else { Some(Token(s)) }
pub fn text_of(t: Token) -> String = match t { Token(s) -> s }
pub fn of_ymd(year: Int, month: Int, day: Int) -> Option<Date> = ..
```

Inside the module nothing changes. Another module names the type, holds and
passes its values and calls the module's functions, and is refused what would
make a value or take a sum apart, each time with the functions the module
publishes for the type:

* a constructor of an opaque sum, called, passed, imported or matched, is
  `E0107`: the type's module keeps it, as it keeps a private name;
* a record literal where an opaque record is expected, and an update of one,
  is `E0220`;
* a `forall` over a type that holds one is `E0418` (§10): a value drawn there
  would be built there. The law is stated in the type's module, or over what
  its functions take; or the module states a `gen` for the type (§4.4), whose
  values are then the module's own wherever they are drawn.

An opaque record's fields are still read, by `d.year` and by a record pattern,
in any module; a module that keeps what a value holds to itself declares a sum
of one constructor. `opaque` is not secrecy: `==`, `compare`, `digest`, `show`
and `reflect` read a whole value as they read any other, through a `key` and a
`show` where the module states them (§4.4), and a `Secret` is the type nothing
reads (§4.6). A codec the module derives (§11) is one it publishes, and its
decoder builds a value of any document of the type's shape, so a type whose
functions hold more than its shape says writes its codec by hand.

A default on a `pub fn` is copied into each caller, so one that names an opaque
constructor or builds an opaque record is `E0122`. A type need not be `pub` to
be opaque: its values still leave the module in what its functions answer. An
alias is the type it names and has no values of its own to keep, so `opaque` on
one is `E0221`. Unlike `pub`, `opaque` is part of a type's hash (§4.2): it
decides what a definition that never names the type may write, so every
definition that can hold one is checked again when a type is opened or closed.

### 3.3 Packages and the manifest

A project root may hold a `ply.pkg` file: the package's manifest, exactly one
definition whose body is a literal of `std.pkg`'s `Manifest` (`ply doc std.pkg.Manifest`):

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
import cli.surface         // a module of the declared dependency `cli`
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
digest is what catches it when that happens. The fetch runs the `git` on
`PATH`, with the run's own environment. A fetch that git cannot do, or a run
with no `git` to do it, leaves a dependency that was not fetched (`E0135`).

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
modules it contributed, sorted by name, with what they embed (§3.4), and for a
registry dependency the `archive` digest it was fetched as. A package is pinned
by *what* it is and
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
dependency's `main` is no entry point; `ply test` runs the root package's tests
and never a dependency's, which are that package's own to run; and `ply prove`
discharges the root package's obligations, claiming a dependency's definitions
by their `requires` and `ensures` (§10). A dependency's unused definitions and
`reuse fn` promises are its own run's to report too. `--workspace` (`ply
check`, `ply test`, `ply prove`) runs the command for every package the path
reaches by a path dependency as well, each as its own root with its own cache,
dependencies first; a fetched or vendored dependency is never one of them.

### 3.4 Embedding files

```ply
fn schema() -> Bytes = embed("schema.sql")
fn fixtures() -> List<{ name: String, bytes: Bytes }> = embed_dir("fixtures")
```

`embed("path")` is the bytes of a file, and `embed_dir("path")` every file under
a directory by its path below it (`a/b.txt`), in that order; nothing under a
name starting with `.` is read. The path is a string literal (`E0147`), read
relative to the module's own file when the program is loaded, and the call is
written out as what was read before anything hashes or checks the module. The
bytes are therefore part of the definition's hash: a test reading an embedded
file reruns exactly when the file changes, and is cached while it does not. A
path that does not exist, a directory handed to `embed`, a file handed to
`embed_dir`, or a file that cannot be read is `E0146`, which refuses the load. A
module that declares or imports its own `embed` or `embed_dir` calls that one.

### 3.5 Constants a build keeps

```ply
type Status = { code: Int, reason: String }

const fn statuses() -> Array<Status> = parsed(embed("status_codes.csv"))
```

A `const fn` is a definition a build evaluates once and keeps the value of: a
table made from a data file is made when the program is built, and a run reads
it. It takes no parameter, binds no type, label or row, and writes no row; its
body may raise and performs nothing else, a call that may not return included
(§5.10); and it answers a value that is data, so no function, `Cell`, `Task`,
`Chan` or `Secret` at any depth. Anything else is `E0156`. Its row is empty
whatever its body could raise, as a tagged literal's is (§2.3): the build saw it
raise nothing. `pub const fn` publishes it; `const` goes with neither
`transparent` nor `reuse`.

`ply check`, `ply run`, `ply test`, `ply prove` and `ply build` evaluate each
`const fn` the program holds before they do anything else with the load (§16),
under a budget of 100000000 calls. One that raises is `E0157`, saying what it
raised and where; one that spends the budget, or whose value takes more than
16777216 bytes kept, is `E0158`. Each is placed at the definition, and nothing
of the program runs. The value is kept in the toolchain's cache (§8.6) under the
definition's hash, which covers its body, all it reaches and the bytes of every
file it embeds (§3.4): an edit to any of those evaluates it again, and nothing
else does but another `ply`.

Every unit emitted after that holds the value as data in place of the body, a
built artifact's among them (§15). A call of the definition reads the value,
laid out by the run's first read of it, and enters nothing of the body:
`metered` (§8.1) counts no step and no allocation for it. A program no build
kept the values of, such as a mutant (§8.5) or the mixture a bisection runs
(§8.4), evaluates the body in its place, once a run, as it does any definition
that takes nothing; the value is the same either way, since the body is pure.

A value is kept as it is laid out, so a table that is compact is one that reads
quickly. An `Array` holds a word an element, and an `Int` within 63 bits or a
width below 64 is held in that word: an `Array` of those is one object, kept as
its words and read back in one piece, as a `Bytes` or a `String` is. Any other
element is an object of its own, made when the value is read: an `Array` of
30,000 records is 30,001 objects. A map is kept in its order and read back
without comparing a key, and what a value shares is kept once.

## 4. Types

Types are inferred by Hindley–Milner unification with row polymorphism. Written
signatures are checked, not inferred (§4.7).

### 4.1 Scalars and numbers

| type | values |
| --- | --- |
| `Int` | 64-bit signed; the type to count and index with |
| `U8` `U16` `U32` `U64` `U128` `I8` `I16` `I32` `I64` `I128` | fixed widths, for data defined in a width and for exact answers past `Int` |
| `Float` | IEEE-754 binary64 |
| `Decimal` | exact base 10; `+ - * %` are exact or raise |
| `Bool`, `Unit` | `true`/`false`, `()` |
| `Char` | one Unicode scalar value: `U+0000` to `U+10FFFF` without the surrogates |
| `String` | UTF-8, indexed and sliced by character |
| `Bytes` | immutable bytes, indexed by byte |

There is no numeric tower. An operator's operand type is settled from the whole
definition, so `fn h(a: U32) -> U32 = a + 1u32` checks and `a + 1` does not. An
operand nothing determines, as in `let g = |a, b| a + b;`, is `E0210` — the same
code a `++` that says neither `String` nor `Bytes` raises; there is no default. Conversions are explicit builtins (§12). `u32_of_int` and its
siblings raise (§6.8) when the value does not fit (mask to truncate:
`u8_of_int(n & 0xFF)`); two fixed widths convert through `Int`, which `to_int`
reads any of them as and `numeric_of_int` writes at any (§12), and a 128-bit
value past `Int` through its decimal text (`u128_of_string`).
`string_of_bytes` raises on invalid UTF-8.

A `Decimal` is a 96-bit mantissa and a scale, the count of digits after its
point, from 0 to 28: `1.5m == 1.50m`, and they print as written. `decimal_div`
and `decimal_round` take the scale of their answer, and answer with exactly that
many digits after the point, the exact value rounded once by the `Rounding`
given; they raise where 96 bits do not hold the answer at that scale.

### 4.2 Records and tuples

Records are structural; `type` names an alias, not a new type, unless its body
opens with `new` (below). Field order does not matter.

```ply
fn f(a: Account) -> Int = a.balance
fn g(r: {name: String, balance: Int}) -> Int = f(r)     // same type
fn point(x: Int, y: Int) -> {x: Int, y: Int} = {x, y}   // `x` is `x: x`
fn divmod(a: Int, b: Int) -> (Int, Int) = (a / b, a % b)
```

A tuple is a record with positional fields: `(A, B)` is `{_0: A, _1: B}` in
types, values and patterns, accessed as `t._0`. `(A)` only groups; `()` is
`Unit`.

An alias may take parameters and name a type that constrains them, as
`std.set`'s `type Set<a> = Map<a, Unit>` names a map keyed by `a`. The alias carries no
constraint: each signature that uses it promises what its expansion needs,
`where derivable(ord, a)` here, and one that does not is `E0206` where it names
the alias. It may take label and row parameters too (§4.5):
`type Step<a | e> = () -> Option<a> / e` is the function type it expands to, with
the row a use gives it in place of `e`.

`type T = new { .. }` declares a record of its own, with at least one field:

```ply
pub type Date = new { year: Int, month: Int, day: Int }

fn epoch() -> Date = { year: 1970, month: 1, day: 1 }
fn next(d: Date) -> Date = { ..d, year: d.year + 1 }
fn held() -> Date = {
  let r = { year: 1970, month: 1, day: 1 };   // a `{day: Int, month: Int, year: Int}`
  r                                           // E0201: not a `Date`
}
```

It is read (`d.year`), updated and matched as any record is, and it is no other
type: not a record of the same fields, and not another declaration of them
(`E0201`). A record literal is one where the place it is written in says so
before the literal is checked:

* the body of a `fn`, and of a lambda whose return type is written or expected;
  a `let` with a written type; a parameter's default;
* an argument of a call or an operation, at its parameter's type. A type
  parameter of the callee is what the arguments before this one, and the type
  the call is itself expected at, make it, so `assert_eq(d, {..})`,
  `push(days, {..})` and a `Some({..})` answering `Option<Date>` each take one;
* a field of a literal that is itself expected, and a field an update writes;
* an element of a list, a branch of an `if` and an arm of a `match`, at the type
  expected of the whole or of the elements, branches and arms before it; the
  right side of an operator, at its left's;
* a block's tail, a `let ... else` block, and the body of a `handle` (its
  `return` clause, where it has one), a `with_cell` or a `simulate`, where the
  whole is expected; the body of a `try`, where a `Result` of it is; a handler
  clause, at what it answers: the operation's result, or the `handle`'s value
  for a raise and for a clause that binds `resume`.

Such a literal names every field (`E0201`) and no other (`E0101`). A record
bound without its type is a plain record wherever it goes next.

The declaration is closed, and judged where it is written: a field names only
the declaration's own type, label and row parameters (§4.5), holds no `Cell`,
`Task` or `Chan` (`E0446`, §4.6), and does not reach the record itself except
through a sum (`E0214`), since whatever reads a record's shape reads its fields
whole. A `Map` key it leaves to a parameter is promised where a value is built,
as a constructor's is, not by each signature that names the type.

At run time a `new` record is the record it is written as. `==`, `compare`,
`digest`, `show`, `reflect` and every derived codec (§11) read its fields as
they read a plain record's, so two `new` records of the same fields print,
encode and digest alike, and only the checker tells them apart. A definition's
hash does tell them apart: a `new` record is hashed with its name, as a sum is,
and either with whether it is `opaque` (§3.2).

### 4.3 Lists, arrays and maps

`List<a>` is an immutable homogeneous sequence `[a, b, c]` (§5.6). `Array<a>`
is a fixed number of elements laid out one after another, so reading or
replacing one by its index is a load or a store; it is a value like a list,
compared, ordered and derived element by element, and has no literal: build it
with `array_new` or `array_of_list` (§12). `Map<k, v>`
is an immutable sorted map, written `#{k: v}` or built with `map_new`,
`map_insert` or `map_of_entries`; a set is a `Map` whose values are `()`,
written `#[a, b]`, and `std.set` names its type `Set<a>`. It iterates in `compare` order. Its key type
must be ordered (`derivable(ord, k)`): `Float`, `Secret`, functions, `Cell`,
`Task` and `Chan` are refused (`E0206`).

### 4.4 Sum types

```ply
type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point

type Level = Debug | Info | Warn | Error
```

The leading `|` is optional. Constructors are values: `Circle(3)` is a call,
`Point` a reference; an `opaque` sum's are its module's alone (§3.2).
`type Id = Int` (one name, no payload, no `|`) is an alias.
A sum is nominal, as a `new` record is (§4.2): identical sums in two modules
differ. A sum takes parameters as an alias does,
`type Tree<a> = | Leaf | Node(Tree<a>, a, Tree<a>)`, and a use that fills them
with other arguments is another type (§4.5).

A value is compared, ordered, digested and shown as its constructor and fields
say, which is wrong for a type that can hold one thing two ways. The module that
declares a sum may state what its values are read through instead:

```ply
pub type Deque<a> = | Deque(List<a>, List<a>)   // a front, and a back reversed

key for Deque by to_list       // `==`, `!=`, `compare`, `min`, `max`, `digest`, a `Map`'s keys
show for Date by written       // `show`, `display`, an interpolated string's hole

pub fn to_list<a>(d: Deque<a>) -> List<a> = ..
fn written(d: Date) -> String = ..
```

`key for T by f` names a function of the same module, `f: (T<a, ..>) -> K`. Two
values of `T` are equal exactly when `f` answers equal keys for them, they order
as their keys do, and a value's digest is its key's, so the three cannot
disagree: two deques holding one sequence in different splits are `==`, share a
`digest` and are one `Map` key. The key is read wherever a `T` sits — in a
list, a record, a `new` record, a tuple, another sum, a map's key or value, the key of another
type, or behind a type parameter — and by `assert_eq`, which tells two keyed
values apart as wholes. `derivable(eq, T)`, `derivable(ord, T)` and
`derivable(hash, T)` hold exactly when they hold of `K`, whatever `T`'s fields
are, and `K` is both ordered and hashed (`E0206`). A `match` still reads the
value as it was built, as `reflect` does, and a test over cases tells its cases
apart that way (§8.1). Each comparison calls `f`, so a key is worth keeping
cheap.

`show for T by g` names `g: (T<a, ..>) -> String`: `show`, `display` and a hole
write what `g` answers wherever a `T` is shown, and `derivable(show, T)` holds
whatever `T` holds. It decides nothing about comparison, `reflect` (§12) still
answers the value as it was built, and a diagnostic prints that.

Both are found by the type alone, with no search, so only the module declaring
`T` may write them (`E0208`), and it states each at most once (`E0105`). `T` is
a sum: an alias is the type it names, and a `new` record (§4.2) runs as its
plain record, with no constructor in a value to find them by, so it cannot
state one yet (`E0217`). The function is one that module declares
(`E0101`); it takes one `T`, at the type's own type and row parameters and
nothing narrower (`f: (Held<a | e>) -> K` for `type Held<a | e>`), answers a type
over those parameters alone, has no `where`, and its row is empty, with no
raise of any kind (§6.8) and no `diverges`: `==` and `show` perform nothing,
cannot raise and always return, and a key that raised inside a `Map` insert
would have nowhere to go (`E0218`). It binds
no resource label either: a value does not carry the label its type was given,
so nothing could call the function at it, and a type that binds a label states
neither (`E0218`). A key that is, or holds, a value of
its own type would be compared by asking for its key again, and is refused
where it is stated, through whatever types and keys lie between (`E0219`). A
type's hash covers the functions it states, so a definition that can hold a
`T` is re-checked and its tests re-run when either changes.

A number type states its arithmetic the same way:

```ply
pub type BigInt = | BigInt(Bool, List<Int>)

numeric for BigInt by { add: add, sub: sub, mul: mul, neg: neg, of_int: of_int }
```

`a + b`, `a - b`, `a * b` and `-a` at the type are then `add`, `sub`, `mul` and
`neg`, and `numeric_of_int(n)` there is `of_int(n)`. The record names those five
operations, each once and nothing else (`E0215`), and each function is one the
module declares (`E0101`): `add`, `sub` and `mul` are `(T, T) -> T`, `neg` is
`(T) -> T` and `of_int` is `(Int) -> T`, with no parameter of its own and an
empty row, so `a + b` gains no row where the type is this one (`E0218`). The
type is a sum the module declares, as for a `key` (`E0208`, `E0217`), stated
once (`E0105`), and takes no parameter: an operator has the values alone, and
`numeric_of_int` not even one, so nothing would say what a parameter is
(`E0216`).

`/` and `%` are no operator of such a type, since they have no answer at every
pair of values: its module has a function for each, as `std.bigint.div` answers
an `Option`. The bit operators stay the builtin integers' (`E0201`), and so do
the wrapping, saturating and checked builtins, the operators written in them
(§2.4) and the rotations (`E0223`). A type is ordered by its `key`, so `<`, `<=`, `>` and `>=`
work at one that states both, and compare the keys. A type that states both is
a numeric type: it fills `numeric(a)` (§4.5), as `std.math.sum(xs)` over a
`List<BigInt>` does. One with no `key` has its operators and fills no
`numeric(a)`, which holds the ordered comparisons. `==`, `compare` and `digest`
are the key's, whatever arithmetic is stated. Each of these is `E0201` where
the type does not have it.

A type states how its values are drawn, where a claim over it is sampled (§10):

```ply
import std.gen
import std.gen (Gen)

pub type Date = new { year: Int, month: Int, day: Int }

gen for Date by dates

fn dates() -> Gen<Date> =
  gen::map2(gen::int_between(1, 12), gen::int_between(1, 28), |month: Int, day: Int|
    { year: 2000, month: month, day: day })
```

`gen for T by f` names a function of the same module that answers a `std.gen`
generator of the type (`ply doc std.gen`): `f: () -> Gen<T>`, or for a type with
parameters one that takes a generator for each, `f: (Gen<a>, Gen<b>) -> Gen<T<a, b>>`.
It performs nothing but `abort.raise`. A `forall` over `T` then draws each `T`
through `f` and never from the type's fields, wherever the type sits in a
binder: a `List<T>`, a record or a sum that holds one, a `T<U>`. So a sampled
claim sees only values the module makes, a month 13 never among them, and a law
in any module may quantify over an `opaque` type (§3.2) that states one: its
generator is the module building the value. A module
that states a `gen` imports `std.gen` itself, under a name no source can write,
so `std.gen` states none.

`T` is a sum or a `new` record the module declares (`E0208`), and states one
generator (`E0105`). An alias is the type it names, and nothing says which label
or row a drawn value of a type that binds one is at, so neither states one; a
function that is no generator of the type is refused where it is stated, with
what it was held to (`E0473`). A generator holds a function, which no build
keeps, so `f` is no `const fn` (`E0156`, §3.5). The statement is also the definition `_sample_T`
(`E0105` if the module declares one): `f`'s generator sampled at a root, a key
and a size, or from a record of draws, each generator `f` takes choosing among
values handed in. It is what `ply prove` enters to draw a `T`, and what a test
calls to see what a generator makes.

A generator decides what a sample draws and nothing a program computes. It is
no part of its type's hash: an edit to one re-checks no definition that holds a
`T` and re-runs no test, and draws again exactly the samples drawn through it
(§10).

### 4.5 Generics

`fn apply<a, b | e>(x: a, f: (a) -> b / e) -> b / e = f(x)`: type parameters are
lowercase names in `<...>`, and row parameters follow `|` (`<| e>` if there are
no type parameters); a row variable among the type parameters is `E0301`.
Aliases may be parameterized, `pub type Route<a> = { ... endpoint: a }`, and so
may `new` records, `type Pair<a> = new { first: a, second: a }`.

A bracketed name binds a resource label:
`fn relay<[l]>(b: Bytes) -> Unit / {net.send[l]} = net.send[l](b)`. Binders sit
among the type parameters and before the `|` — `fn serve<a, [l], [k] | e>(..)` —
and one bracket may hold several, so `<[l, k]>` is `<[l], [k]>`. A call fills
them left to right, either written, `relay[conn](b)`, or from an argument whose
row or type names one (§6.2). A call inside a recursive group — to the definition itself
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
with: an argument may perform less than it, and one that performs anything the
group's row does not hold is `E0308`. Type parameters are *not*
shared — each definition keeps its own — so a call inside a group that would
need the callee's type parameter at another type is polymorphic recursion, which
Ply does not infer. That is `E0308` as well: break the cycle so the callee is
checked on its own before the call, or monomorphise it.

A `type` binds all three kinds where a `fn` does, on an alias, a sum and a `new`
record, and a use fills them in the same places:

```ply
type Step<a | e> = () -> Option<{ value: a, next: Seq<a | e> }> / e
type Seq<a | e>  = | Seq(Step<a | e>)
type Sink<a, [l] | e> = | Sink((a) -> Unit / {net.send[l] | e}) | Quiet
type Stepper<a | e> = new { step: () -> Option<a> / e }

fn next<a | e>(s: Seq<a | e>) -> Option<{ value: a, next: Seq<a | e> }> / e =
  match s { Seq(step) -> step() }
fn chunks<[l]>(path: String) -> Seq<Bytes | {fs.read_at[l], diverges}> / {diverges} = ..
fn quiet() -> Sink<Int, [conn] | {}> = Quiet
```

A row argument is a row as §6.2 writes one, `{fs.read_at[src]}`, `{log.write | e}`,
a bare `e`, or `{}` for functions that perform nothing; a label argument is a
resource or a binder in scope. A use fills every parameter the type binds, and one
that fills another number of types, labels or rows is `E0202`: a type with a
row parameter has no short form that leaves it out. A declared type is closed: a
row variable in it is one of its own row parameters (`E0301`), and a label in it is
one of its own label parameters or the resource of that name, never a binder of
the definition that uses the type. An alias's arguments are written into its
expansion, so `Step<Int | {}>` is `() -> Option<..>`. A sum's arguments, and a
`new` record's, are part of which type it is, so a `Seq<Int | {}>` is not a
`Seq<Int | {fs.read_at[src]}>` (§6.2), though a function a `Stepper<Int | e>`
literal is built of may perform less than `e`, as any function meeting a type
may. A printed type shows them as written, `m.Sink<Int, [conn] | {}>`.

`where numeric(a)` lets a type parameter take arithmetic and the ordered
comparisons: `+`, `-`, `*`, unary `-`, `<` and the rest, and
`numeric_of_int(n)` writes a constant at it. `where integer(a)` adds `/`, `%`
and the bit operators; either under `numeric(a)` alone is `E0209`. A call fills
`a` with one of the numeric types — `Int`, the fixed-width integers, `Float`,
`Decimal` and a type that states `numeric` and a `key` (§4.4; `integer`: the
first two) — or with a parameter of its own the same constraint is on; any other
type is `E0201`.
Each operator fails where it fails at that type: a `/` or `%` by zero raises
(§6.8), and a width's overflow ends the run:

```ply
fn sum<a>(xs: List<a>) -> a where numeric(a) = fold(xs, numeric_of_int(0), |s: a, x: a| s + x)
```

The type a call fills a constrained parameter with is passed as a hidden
argument, so such a definition must be called directly; used as a value it is
`E0310`, and a lambda that calls it is the value. A call can only fill a
parameter its signature's parameters or answer mention, and inside a recursive
group only the definition itself holds the type, so a constrained parameter the
signature never mentions, or a call from another member of the group, is
`E0310` too. A spec clause assumes its definition's constraints.

### 4.6 Types the language declares

In scope everywhere; redeclaring one is `E0105`:

```ply
Option<a>     = None | Some(a)
Result<a, e>  = Ok(a) | Err(e)
Ordering      = Less | Equal | Greater
Rounding      = HalfEven | HalfUp | Down | Up | Ceiling | Floor
Iter<s, r>    = Continue(s) | Stop(r)
Instant       = Instant(Int)
Duration      = Duration(Int)
```

`Instant` is a reading of a clock and `Duration` the span between two, both in
nanoseconds; they are separate types so a deadline cannot be added to a byte
count. `std.time` builds and reads them.

A module that declares or unqualified-imports its own `Ok`, `Err`, `Some` or
`None` loses `?`; one that binds its own `Ok` or `Err` loses `try` (§6.8), and
tagged literals with its own `Ok` (§2.3), each `E0118`; one that declares its
own `Stop` loses `iterate`.

**`Secret<a>`** is made by `secret_of_string` and observed only by
`secret_verify`, `secret_is_empty` and `==`. It cannot be rendered, encoded or
ordered, and reaches a host operation only if that operation's registration
allows it (`E0439`).

**`Cell<a>`** (§7), **`Task<a>`** and **`Chan<a>`** (§9) are branded by their
region and cannot outlive it; the brand prints as `Cell[users]<Int>`. A
declaration is outside every region, so a variant's field, a `new` record's
field or an operation's parameter or result that mentions any of them, at any
depth, is `E0446`. Take it as a type parameter instead,
`type Held<t> = Held(t)`: the type argument carries the brand where the escape
checks see it. A sum hides the rows its fields write, so a field whose function
names `cell`, or joins a task or works a channel, in its row is `E0446` as well:
take the row as a row parameter, `type Held<| e> = Held(() -> Int / e)`, and a
closure that reaches a region is seen leaving it in the row the type is given.
A `new` record's fields are read wherever it goes, so a row one of them writes
is seen leaving a region as a plain record's is.

### 4.7 Function types, and what is written

`(A, B) -> C` is pure and cannot raise; `(A) -> B / {abort.raise}`,
`(A) -> B / {db.read[users]}` and `(A) -> B / e` carry rows. With no `/` the row
is empty, in a signature as in a declared type. A function value may perform less
than the type it meets says (§6.2).
Functions cannot be compared, encoded, ordered or used as map keys.

* **Written:** every parameter and return type of a top-level `fn` (`E0126`),
  every `forall` binder type, and every top-level `fn`'s effect row, so a
  caller, in this package or another, is checked against the signature and never
  against the body. The row is an upper bound: what the body performs must fit
  inside it (`E0302`, with the row to write, naming an `effect set` wherever the
  module can), and it may be wider than the body needs. Each written atom covers
  what it names: the mode atom `net.write[conn]` covers every `write` operation
  of `net` on `conn`, the operation atom `net.send[conn]` covers `send` alone
  (§6.2).
* **Inferred inside bodies:** lambda binders and rows, `let`s, everything else.
  A local `let` is monomorphic, so `let f = |x| x;` used at two types is
  `E0201`.

## 5. Expressions

Everything is an expression, including `if`, `match`, `handle` and blocks.
An expression nests at most 128 levels deep: each operand, parenthesis and
block, a `let ... else` block among them, is a level, and the parser refuses
one level more ("input is nested too deeply to parse"). An `else if` chain and
a run of operators of one precedence are each one level, however long.

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
pattern does not match raises (§6.8); `let <pattern> = <expr> else { .. };` says what
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
| `42`, `-1`, `1.5`, `1.50m`, `"s"`, `b"s"`, `b'{'`, `true`, `()` | a literal |
| `0..=9`, `b'a'..=b'z'`, `1u8..=9u8` | an integer from the first bound through the second |
| `[]`, `[a, b]`, `[a, ..]`, `[a, ..rest]` | a list of exact length, or a prefix |
| `{a, b}`, `{a: p, b: q}`, `{a, ..}` | a record; `..` allows other fields |
| `(p, q)` | a tuple |
| `p \| q` | either alternative, the leftmost first; parentheses nest a choice |

Every alternative binds the same names at one type (`E0212`). An arm is tried
once per alternative, so a guard runs again for a later alternative when an
earlier one matched and the guard refused it. A plain `let` may use an
or-pattern only where its alternatives cover the type, as in
`let Ok(v) | Err(v) = r;`; one that can fail needs an `else` (`E0213`).

A range's bounds are two integer literals of one type, the first no greater than
the second and neither in parentheses (`E0222`); it binds nothing:

```ply
fn value(src: Bytes, i: Int) -> Result<Json, ParseError> / {abort.raise} =
  match bytes_at(src, i) {
    b'"' -> string_value(src, i),
    b'-' | b'0'..=b'9' -> number(src, i),
    b'{' -> object_value(src, i),
    _ -> Err(error_at(i, "expected a value")),
  }
```

Literals and ranges that between them hold every value of a `U8`, a `U16` or a
`U32` exhaust it, and one they leave out is named:
`match b { 0u8..=9u8 -> .., 20u8..=255u8 -> .. }` is `E0205`, not covered:
`10u8..=19u8`. No other integer type's least and greatest values are both
literals, so a match over one ends in a `_` arm.

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
An update of a `new` record (§4.2) answers that record, so a field it writes
keeps its declared type (`E0201`), and it builds one: outside the module of an
`opaque` record it is `E0220` (§3.2).

### 5.6 Lists

`list_at(xs, i)` answers `None` for an index past the end **or negative**:
`list_at(xs, -1)` is `None`, not the last element (that is
`list_at(xs, len(xs) - 1)`). `list_set(xs, i, v)` raises `E0502` out of range.

`push(xs, x)` appends in place when the caller holds the last reference, and
otherwise copies one path of the list's trie; `list_set` is the same, and
`array_set` copies the whole array. A copy is caused by a second owner: a
binding read again after the update, a closure capture, a value read out with
`cell_get`/`map_get`/`list_at`/`array_get` (use `cell_update`/`map_update`), or
a caller that keeps using what it passed. `ply check --costs` reports every
copying `push`, `list_set` and `array_set` in the run's own modules with its
cause and fix. A `reuse fn` there turns that into an error, `E0127`; a
dependency's, the shipped modules' included, is checked when that package is
the one checked:

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n == 0 { xs } else { collect(push(xs, n), n - 1) }   // kept: xs is a parameter at its last use

reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {
  let ys = push(xs, n);
  if len(xs) < 0 { xs } else { ys }                       // E0127: xs is read again after the update
}
```

A call of another package's definition answers what its `returns` clause says,
never what its body does: `returns fresh`, a value nothing else holds, or
`returns xs`, one that may share the parameter `xs`. Without the clause an
append onto the answer cannot be shown to reuse. The clause stands between the
signature and its specifications, and the cost checker shows it of the body
when that package is the one checked (`E0148`):

```ply
pub fn digits(n: Int) -> List<Int>
  returns fresh
= range(0, n)
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
* `E0118`: inside a `handle`, `try`, `with_cell` or `simulate`; inside a
  lambda without a written return type; or where `Ok`/`Err`/`Some`/`None` are
  rebound. A lambda with a written return type exits the lambda.
* `E0119`: in an `if` branch, `match` arm or right of `&&` not in return
  position; after an impure argument (`g(h(x), k(x)?)`); or in a nested block.
  Bind the value first.

### 5.9 `parallel`

```ply
fn both(xs: List<Int>) -> (Int, Int) =
  parallel { fold(xs, 0, |a: Int, x: Int| a + x), len(filter(xs, |x: Int| x > 0)) }
```

`parallel { a, b, .. }` answers the tuple `(a, b, ..)` and means exactly that:
the branches evaluated left to right. The runtime runs them at once, on threads
of its own, wherever that cannot change the answer: the checker admits a block
only when no two branches' rows conflict (§6.2), cells of a region opened
outside the block included, so a branch reads nothing another writes. It takes
two or more branches, and `parallel` is a name wherever no `{` follows it.

* The block's row is the union of its branches'; it is as deterministic as they
  are, so a deterministic test may hold one, and its result may be cached.
* The leftmost failing branch is the block's failure, whichever failed first.
  A branch to its right may already have performed its effects.
* The branches spend one step budget between them (`E0503`, §8.4), as they would
  in turn.
* A block whose branches could reach a `handle` around it or a `simulate` region
  (§9) runs them in turn, which answers the same. So does one whose branches
  hold a cell, a task or a continuation. Under `--host` each branch waits on a
  reactor of its own.
* `E0309`: two branches perform operations of one effect on one resource, one
  of them a `write`; or a branch's row is open (it calls a function value whose
  row is a variable) while another performs anything; or a branch opens a
  `simulate` region or performs a `task` operation, since a task belongs to the
  scheduler of the thread that spawned it. `E0118`: a `?` inside a branch.

`std.parallel` splits a list across nested blocks.

### 5.10 Termination

```ply
fn count(xs: List<Int>) -> Int = match xs { [] -> 0, [_, ..rest] -> 1 + count(rest) }

fn down(n: Int) -> Int = if n <= 0 { 0 } else { down(n - 1) }

fn climb(a: Int, b: Int) -> Int
  decreases a + b
  = if a + b <= 0 { 0 } else { climb(a + 1, b - 2) }
```

The checker reads each recursive group, mutual recursion included, for a
measure that every loop of calls back into the group lowers (size-change
termination): a part of an argument — a constructor's field, a record's field,
a list's element or tail, a map's key, value or entry, at any depth — or an
integer moving toward a bound that a guard on the way to the call holds it
beyond, as `down` does, or as `if i >= len(xs) { .. } else { walk(xs, i + 1) }`
does. A value with no parts — a nullary constructor, an empty list, a literal —
is no larger than any argument, and a constructor of one field no larger than
what its field is a part of. A quotient by a literal greater than one, a shift right by one, and a
remainder by the measure itself lower an integer the guard holds at one or
more. A loop may lower different measures at different calls, as Ackermann's
function does, and a guard in one member of a group bounds the loops through
the others. A guard of `n == 0` bounds nothing: from `-1`, `n - 1` never
meets it.

A guard is a condition on the way to the call: an `if`, a `match` arm's guard,
the failed guard of an earlier arm whose pattern always matches, a condition
matched against `true` or `false`, or `list_at` or `array_at` answering
`Some`, which holds the index below the length. `list_set` and `array_set`
keep the length. A list written of parts, one they are pushed onto, one
`filter` keeps, one `map` makes of a part of each element, and what `fold`
answers when each step answers the accumulator or a part (a lookup that starts
at `None`) are made of parts: each element is a part, though neither the list
nor its tail need be smaller than what its parts are parts of. A value an `if`
or a `match` chooses is what each branch is, and what a member of the group
answers is read to a fixed point over the group, so a member that answers a
part of its argument hands its caller a part.

A function handed to `map`, `filter` or `fold` is called with each element, and
one handed to `map_fold` with each key and value; one handed to a definition
outside the group is called as that definition calls it, with the parts of its
arguments it hands on; a function an `if` chooses is handed on as each branch
is; and a lambda bound by `let` is read where it is called, eight lambdas deep
at most. A member of the group handed anywhere else, or one called from a
lambda whose calls the checker cannot see, is called with nothing known.

A definition whose group descends ends, and so does one calling only
definitions that end. Any other may not return, and its row says so with the
atom `diverges` (§6.2). One whose own recursion is not seen to descend, or is
past what the check follows (a lambda deeper than it reads, or calls that
compose into more than 10,000 size-change graphs), must write it: without it
the definition is `E0302`, with no fix offered, since what usually fixes it is
a recursion that descends, a budget spent with `iterate`, or a `decreases` a
proof shows. `diverges` belongs in the row only of a loop that waits on
something outside, such as a stream or a peer. A caller inherits it as it
inherits any atom, and there `E0302` offers the fix that writes it. A test, law
or clause that may not return supplies nothing for it: its steps bound it.
`ply check --types` says under each such definition why, at the call's place:
its own recursion is not seen to descend there, or is past what the check
follows, or it calls there a definition, which it names, that may not return.
`ply check --json` gives each definition's `ending`: its `kind` (`ends`,
`stated` for one that ends by its group's measures, or `diverges`) and, for one
that diverges, `through` and `at`.

`decreases <measure>`, after the other clauses, states an `Int` over the
parameters for a descent no one argument makes, as in `climb`: at every call
the group makes back into itself, the callee's measure at the call's arguments
lies below the caller's, which is not negative. The checker proves it, as `ply
prove` proves a claim (§10), over the body and the guards on the way to each
call, reading the group's calls before that one as values; a group that
descends with its proved measures counted is read as ending. A measure no proof
shows is `E0467`, noting where the proof stopped, and so is one on a definition
the checker sees end without it, which can be deleted. A call into the group
from a lambda, a region or an arm's guard leaves nothing to prove. A measure is
pure, as a clause is (`E0417`), and part of the definition's hash, since it
decides what the definition is read to do.

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

effect toml {
  raise syntax(e: SyntaxError)
}
```

Each operation is `read`, `write` or `raise`. A `read` or a `write` answers
where it was performed; a `raise` does not come back, so it writes a name and
its parameters and no result (§6.8). `[r]` makes a `read` or a `write`
resource-parameterized: a perform must supply a label (`E0304`). `nondet` marks
results that are not a function of program state (§8.3). Effects are nominal.
`task`, `clock`, `random`, `sim`, `abort`, `diverges` and `cell` are taken
(`E0105`).

An operation's type parameters sit just before its parameters,
`read take[r]<a>(key: Int) -> a`, and are its only type variables: its
signature resolves names as a `fn` signature does, so a lowercase name its list
does not declare, or a type name not in scope, is `E0102`. Each perform picks
its own `a`, so a clause for the operation has to answer every type (`E0201`).

### 6.2 Atoms and rows

An atom is `effect.mode[resource]`, or `effect.mode` for a singleton. A row is a
set of atoms with an optional tail variable: `/ {db.read[users], clock.read}`,
`/ {net.write[conn] | e}`, `/ e`, `/ {}`. Qualified atoms use `::`:
`/ {store::db.read[users]}`. An atom may instead name an operation,
`net.send[conn]`, which the mode atom of the same effect and resource
(`net.write[conn]`) covers; naming an operation the effect does not declare is
`E0104`. No mode atom stands for a `raise`: a row names each one, `toml.syntax`,
so a row is the set of ways a call can fail (§6.8). `diverges`, written bare,
is the atom of a call that may not return (§5.10); it names no operation, and
nothing handles it.

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
row as `body performs`.

A function value may perform less than the type it meets says, though not more
(`E0302`). It meets one as an argument to a call or an operation, against a
written return type or a `let` annotation, and as one of the elements of a list
or the branches of an `if`, a `match` or a `let ... else`, which meet each
other; so does any function such a value holds in a record, a tuple, a `List`,
an `Array`, an `Option`, a `Result`, a `Map` or an `Iter`, and the function a
function returns. So `run(|| a.x())` checks against `fn run(k: () -> Unit / {a.x, a.z})`,
a callback whose row names `net.send[conn]` is one a row written
`/ {net.write[conn]}` admits, and two callbacks fill one row variable with both
their rows: given `fn both<| e>(f: () -> Unit / e, g: () -> Unit / e) -> Unit / e`,
`both(|| a.x(), || b.y())` performs exactly `{a.x, b.y}`, and
`[|| a.x(), || b.y()]` is a `List<() -> Unit / {a.x, b.y}>`.

Inside a function's parameter, a `Cell`, a `Task`, a `Chan` or a sum type the
program declares, rows must match exactly, because a function held there can be
handed what its own type does not admit: a parameter typed
`() -> Unit / {net.send[conn]}` is not one typed `() -> Unit / {net.write[conn]}`,
in either direction.

The row a sum is given (§4.5) is an argument, not a bound, and is held the same
way: a `Seq<Int | {net.send[conn]}>` is not a `Seq<Int | {net.write[conn]}>`, nor
a `Seq<Int | {}>` one that performs. A constructor gives the sum exactly the row
of the function it is handed, so `Seq(|| None)` is a `Seq<a | {}>`; a function
that performs less than the sum's row should say is given the wider type first,
by a `let` annotation, which is one of the places a narrower function is
admitted. An alias's row argument is written into its expansion, and is held as
the place it lands in holds any row: `fn both<| e>(f: Thunk<| e>, g: Thunk<| e>)`
over `type Thunk<| e> = () -> Unit / e` takes two callbacks that perform
different things, as it does written out.

Resource labels are global — two modules writing `[users]` name one resource —
and a definition may be generic over one (§4.5). Its binder shadows that global
namespace inside the body: under `fn relay<[l]>`, the `[l]` of a row, of a
perform `net.send[l](b)`, of a handler clause `net.send[l](x) -> ..` and of a
nested call `inner[l](..)` is that parameter, while a label no binder holds is
the global one of that name. A call fills it with a label it writes,
`relay[conn](b)`, or with the one an argument's row names: a parameter typed
`() -> Unit / {net.send[l]}` given an argument whose row is `{net.send[conn]}`
fills `l` with `conn`, and one typed `Sink<[l]>` given a `Sink<[conn]>` does the
same. A label left unfilled is `E0306`.

The standard library is generic over its labels: `std.net` and `std.http` name
no resource of their own, so a program may answer its connections under one
label and talk upstream under another, over one serve loop and one writer
(`std.net`, `std.http`).

Two atoms **conflict** iff they name the same resource of the same effect and
one is a `write`; a `raise` conflicts with nothing. A label parameter or `[*]`
may be any label, so it conflicts with every label of its effect. `parallel` (§5.9) and the test scheduler (§8.4)
both decide by this.

A row also counts. Each atom a call performs runs a `bounded` number of times,
one no input decides, or a `scaling` one that grows with its input:
performed inside a callback that `map`, `filter`, `fold`, `map_fold`,
`bytes_position` or `iterate` calls once per element (unless the list,
`range` or budget is written out literally), inside a definition that calls
back into its own recursive group, or by a callee that performs it so. A
higher-order definition counts its callbacks the same way, so `map(ids,
lookup)` scales `lookup`'s query, a definition that calls its callback in a
`fold` scales whatever it is handed, and one that calls it once does not.
`ply check --types` prints each atom and row variable with its count:

```ply
fn lookup(id: Int) -> Row / {db.query[conn]} = db.query[conn](id)
fn lookup_all(ids: List<Int>) -> List<Row> / {db.query[conn]} = map(ids, lookup)
```

```
     lookup     : (Int) -> Row
                  / {m.db.query[conn] bounded}
     lookup_all : (List<Int>) -> List<Row>
                  / {m.db.query[conn] scaling}
```

A definition's row may promise an atom, or its row variable, `bounded`:
`/ {db.query[conn] bounded}`, `/ {log.write | e bounded}`, `/ e bounded`. A
body that performs it a scaling number of times is `E0465`, naming the
operation and the iteration that repeats it — batch it into one operation over
the whole input, or move it out of the iteration. `bounded` belongs to a
definition's own row; in a function type or an effect set it is `E0466`. A
definition of another package is read by what its row writes: an atom it does
not promise `bounded` may scale, and so may each callback it takes unless its
row variable is promised.

### 6.3 Performing

`db.get[users](3)`, `clock.now()`, `store::db.put[orders](id, row)` add their
atom to the enclosing definition's row. A written row such as
`fn stale(s: Session) -> Bool / {clock.read}` is an upper bound (§4.7).

### 6.4 Effect sets

```ply
pub effect set Persist = {store.read[db], store.write[db]}
effect set Full        = {Persist, log.write[app]}
```

A set stands for its atoms wherever a row names it. `pub` exports it: another
module imports it by name (`import storage (Persist)`) or names it through the
module (`/ {storage::Persist}`), and each atom means what it means in the module
that declared the set, so the importer need not import the effects. Sets may
nest, across modules too (a cycle is `E0115`), and may not hold a row variable.
A set nothing declares is `E0114`; one its module keeps private is `E0107`.

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
number of times. Without `resume`, a clause's value returns to the perform site,
except a clause for a `raise` operation (§6.8).

A clause that binds `resume` and never calls it abandons the body where it stood,
and `bracket(acquire, release, body)` is how a body that holds something lets it
go anyway: `release` runs on what `acquire` answered when `body` returns, when a
clause unwinds through it this way or a raise does (§6.8), and when its task is
cancelled (§9), where the bracket stands and with the handlers around it. A `release` that fails
replaces whatever was unwinding. A runtime failure ends the run, so nothing more
runs then, `release` included. Nested brackets release innermost first.

### 6.7 Unhandled effects

`E0302`: the body performs an atom, or an operation, its written row does not
cover. `E0303`: an effect
escaped inference (a compiler defect). `E0305`: a `handle` lacks a clause for
an operation its body performs on an atom it handles. `E0424`: an operation
reached the host boundary with nothing bound — pass `--host` or handle it
(§14).

### 6.8 Raising

```ply
type SyntaxError = { line: Int, why: String }

effect toml {
  raise syntax(e: SyntaxError)
}

fn digit(b: Int, line: Int) -> Int / {toml.syntax} =
  if b >= 48 && b <= 57 { b - 48 } else { toml.syntax({ line: line, why: "not a digit" }) }

fn digit_or(b: Int, fallback: Int) -> Int / {} =
  handle { digit(b, 1) } with { toml.syntax(e) -> fallback }

fn checked(b: Int) -> Result<Int, SyntaxError> = try { digit(b, 1) }
```

A `raise` operation does not come back to where it was performed. Its perform
has whatever type is asked of it and puts the operation in the row, as any
perform does, so a row names each way a call can fail: inference unions them,
and they pass through a row variable, so `map(lines, parse_line)` raises what
`parse_line` raises. A declaration is the name and its parameters; it writes no
result, and takes no resource label and no type parameters (`E0001`).

A clause for a raise has the `handle`'s type: its value is the `handle`'s,
`return` is not applied to it, and it cannot bind `resume` (`E0201`). Its
parameters are what the raise carried. The clause runs outside its `handle`,
once the body is abandoned and the regions the body opened are closed, so a
raise in another clause goes to a `handle` further out than the one whose
clause raised. A clause answers the raise it names and leaves the effect's
others to pass. A raise no clause answers ends the run with `E0502`, naming the
operation and what it carried; a test's is its failure, which is why a raise is
in no footprint (§8.4).

`try { body }` answers `Ok` of what `body` answers, or `Err` of what a raise in
it carried: the value for a raise of one parameter, `()` for one of none, and
the tuple for one of several. It answers the one `raise` operation the body's
row names, `abort.raise` aside, and takes that atom out of the row; a body that
names none is `E0311`, and one that names several is `E0312`, where
`try[toml.syntax] { body }` says which and lets the others pass. Naming an
operation that is not a `raise` is `E0313`. A `try` is the `handle` with that
one clause and `return v -> Ok(v)`, so what holds of a `handle` holds of it: a
recursive group with one in a member's body nests (§5.7), a `?` inside one is
`E0118`, and so is a `try` in a module that binds an `Ok` or an `Err` of its
own. An `Err` goes back to its raise through a function:
`result_unwrap_or_else(r, |e: SyntaxError| toml.syntax(e))` (`ply doc std.result`).

The prelude declares `effect abort { raise raise(message: String) }`.
`abort.raise(m)` is that raise, and so is everything else that can fail on a
value it is given: `panic`, `assert` and `assert_eq`, a builtin outside what it
is defined for (§12), a `/` or `%` by zero (§2.4), a `let` whose pattern misses
(§5.1), an `iterate` past its budget, a `task.join` of a cancelled task, a
`task.channel` of a negative capacity and a `random.below` of a bound below one
(§9). What a literal argument settles does not raise: a divisor other than
zero, a capacity or bound in range, and a narrowing such as `u8_of_int(200)` of
a value its type holds. A signature that leaves `abort.raise` out of its row
where its body can raise is `E0302`, which names the operation and offers the
row to write. Overflow, the call ceiling and a spent step budget end the run
whatever the row says. A clause for `abort.raise` is handed the message:
`panic`'s argument, or what the run would otherwise have reported, a value it
names told by its kind. Unanswered, it is `E0501` for an assertion and `E0502`
for anything else. Nearly every body can raise it, so a `try` answers it only
by name: `try[abort.raise] { body }`.

A `parallel` branch's raise is answered around the block. A task may raise:
its raise goes to the task's own `handle`s and then to those around its
`simulate` region, never to the copies its spawn inherited (§9); outside
`simulate` it ends the run. A `cell_update` whose function raised leaves the
cell as it was.

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
cell the task can reach: open it inside the region,
`with_cell[r](init) { c -> simulate { .. } }`, and not around it,
`simulate { with_cell[r](init) { c -> .. } }`. An older scheduler — an enclosing
`simulate` region (§9), or the production one under `--host` — drains the tasks
nobody joined after the region's `}`, and a `task.join` inside the region does
not license it, because no type records the join. A task reaches a cell through
the closure it is handed, and through the handlers around its `task.spawn`: a
clause answering an operation the task performs runs on the task's behalf, so
under an older scheduler it may not touch the region's cells either. This judges
the spawns a body performs itself, not those of a function it calls.

A task or a channel, in turn, may be kept only in a cell younger than the
`simulate` region that made it. A `simulate` region may not write a cell of a
region opened around it while that cell can hold a `Task` or a `Chan`, whether it
writes the cell itself or calls a function whose row writes it: keep the handle in
a cell opened inside the region, or finish with it there and store what it gave.
Nor may one come in from outside: the region may not name a binding from outside
it whose value holds a `Task` or a `Chan` (a parameter, or a local, whether the
body or a closure inside it names it), nor read a cell of a region opened around
it while that cell can hold one (`E0413`, §9).

* `E0201`: the cell escapes its `with_cell[r]` region.
* `E0446`: a value branded by the region outlives it (stored in an older
  binding, handed to an operation, or reached by a task whose scheduler is
  older than the region, through the closure it runs or a handler around its
  spawn), a task is stored in a cell older than its `simulate` region, a
  declared type's field or an operation's signature mentions a `Cell`, a
  `Task` or a `Chan`, or a variant's field holds a function whose written row
  reaches one (§4.6).
* `E0449`: a region handle (a cell, a task, a channel, or the continuation a clause's
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

`metered(f)` answers `f()` with what it cost, in resources the runtime counts
rather than time: `steps`, the calls it made, each counted as `--steps`
counts them; `allocations`, the objects it built; and `performs`, each atom it
performed with how many times, ordered by the atom's qualified name. A
memoized constant costs what computing it costs, whether or not an earlier call
computed it, so a cost is the same however often it is read, and the same
under either profile (§8.6); a `const fn` a build kept the value of costs
nothing to read (§3.5):

```ply
test "ten more elements cost twenty more steps: one in each closure" {
  let small = metered(|| total(squares(10)));
  let large = metered(|| total(squares(20)));
  assert_eq(large.steps - small.steps, 20)
}
```

A cost law (§10) states how `steps` grows with a size instead of pinning it.

A test may range over a table, and is then a test for each of its elements, a
*case*:

```ply
type Resolution = { reference: String, target: String }

test "resolves {c.reference}" for c: Resolution in resolutions() {
  assert_eq(url_resolve(base(), c.reference), c.target)
}
```

`for <name>: <Type> in <table>` stands between the label and the body, on a
`test/nondet` too. The table is an expression of type `List<Type>` (`E0201`),
usually a call of the definition that builds it, often from `embed_dir` (§3.4);
a nullary definition is computed once however many cases read it, and a record
literal in it is a `new` record where the case's type is one (§4.2). The name is
bound in the body and in the label, and a label before `for` is read as an
interpolated string is (§2.3): each `{expr}` is a hole, `{{` and `}}` are
braces, and a label with no hole names every case alike. A case is told from
another by its value as it was built, which is finer than `==`: `1.5m` and
`1.50m` are two cases, as are two values of a type whose `key` (§4.4) answers
the same, so no case's pass stands for a case its body can tell from it. Two
cases built alike are one. A case's type is `derivable(hash, ·)` read through
no `key` (`E0206`): it holds no `Float`, function, `Cell`, `Task`, `Chan` or
`Secret`, and neither does a keyed type in it. The cases
are listed before any test runs, so
the table and the label may raise, by any `raise` operation (§6.8), and perform
nothing else (`E0469`). A table that raises fails as one test, under the label
as written, and an empty one is no test. A tagged literal (§2.3) in a table, a
label's hole or a body is settled with the load, as any other is.
A module with a test over cases imports `std.cases` itself, under a name
no source can write, so `std.cases` cannot hold one. A doc comment above a test
over cases documents nothing, as above any `test` (`E0003`, §2.1).

A test may hold a rendering to a file the package stores, a *snapshot*, read
with `embed_dir` (§3.4), so the test performs nothing, is cached, and runs
again when a stored file changes:

```ply
import std.snapshot
import std.snapshot (Stored)

fn stored() -> Stored = { dir: "snapshots", files: embed_dir("snapshots") }

test "an order renders as stored" {
  snapshot::check(stored(), "order.txt", snapshot::render(order()))
}
```

`check` fails with the unified diff from the stored text to the rendering, or
with the diff that creates a file nothing stores yet, and never writes one.
Storing is `snapshot::accept`, which writes through `std.fs`: an entry calls
it, and a person runs that entry with the directory lent, as
`ply run . --host --fs snapshots=snapshots` lends it (§14). `ply doc
std.snapshot` has the rest.

### 8.2 Selection

A definition's hash covers its normalized form: names, comments, formatting,
imports, `pub`, specs and test labels are erased, and references are replaced by
their referent's hash. A test runs exactly when neither its hash nor the code it compiles to has a
recorded pass that still stands, so renames and comment edits run nothing, and neither does an edit
or a new `ply` that compiles a test to the same code; the selection line counts those `by code`
(`by_code` in `--json`, reason `same code`). `ply hash` prints the hashes.

A pass is filed with what its run read of the world: each file and directory a handler read under a
root (by the root's name, so a pass reads the same from another checkout), each shipped module it
asked for, and what every `ply` it started read in turn, that `ply`'s own program and shipped
modules included; and for a run that reached a host handler, the binding it ran under (whether
`--host`, and the names `--fs`, `--exec` and `--allow` lend). A load that read a shipped module for
what its program reaches (§13) files the module's types, effects and imports and each function the
program reached, by hash, in place of the module, so an edit to a function no program of the test
reached runs nothing. It stands while each of those still answers as it did, and otherwise the test
runs again (reason `changed`). A read of something the test wrote first is not an input, and neither
is the clock, the network, a program other than `ply`, or what a run keeps for the next
(`PLY_C_CACHE`, `PLY_C_STAGE`). A `ply` that ended before reporting what it read files no pass.
`--explain` says why each test was selected, naming for a `changed` one the read that moved
(`moved` in `--json`), what a pass is filed under (the test's hash and the runtime stamp,
`filed_under` in `--json`, and the code it ran), and where the run's time went, phase by phase
from the process's start (`phases` in the `--json` report);
`--filter SUBSTRING` matches `<module>.<label>`, and repeated it runs every test
any of them matches; `--no-cache` bypasses both the result and the front-end
cache.

A case of a test over cases (§8.1) is selected as a test of its own. Its hash is
its test's, which covers the body and the case's type and neither the table nor
the label, taken with the case's value as built. A case added to the table runs
alone, wherever it is added; a reordered table or a reworded label runs
nothing; an edit to the body runs every case, and an edit to what the table is
built from runs the cases whose values it changed. Its label is the one its
holes make, which is what `--filter` reads. Listing the cases runs the table,
so a run that reports on a test over cases builds the program even when every
case stands on a pass.

### 8.3 Determinism

A test whose row, after handling, retains a `nondet` atom is `E0412`. Handle the
effect, or write `test/nondet "label" { ... }`, which is cached as any test is
(§8.2): what it reads of the clock or the network is no part of its pass.

### 8.4 Scheduling and failures

Tests whose footprints do not conflict run concurrently, under `parallel`
blocks (§5.9) on one thread per core; a test whose effects are all discharged in
a region conflicts with nothing. A raise is in no footprint, since the run
answers it as the test's failure. `--jobs N`/`-j` deals them into `N` lanes, each
lane's tests in turn (default: a lane per test). `ply prove --jobs N` deals its
claims' points the same way.

The report is printed as the run goes. Before a test runs come what was selected
and how it will run: the groups and workers, isolation, the host binding, and
what `--explain` says of the selection. Each group's results follow as that
group finishes, in the order the groups ran, so a long run shows its progress
and its first failure. Then come the run's own figures (handshakes, the backend,
the simulation, and with `--explain` where its time went), the summary, and each
failure's diagnosis. `--json` writes its one object when the run ends, and
`--workspace` prints each package's heading before it runs.

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
handled ones included, as `performs`. `--watch` re-runs on every `.ply` change
and on every change to what the last run embedded, keeping caches in memory.

Each case of a test over cases (§8.1) passes, fails and is reported under its
own label, so a failing table names every failing case. In `--json` a case's
entry under `selection.tests` carries `case`: the key of the test it is a case
of (`of`), its place in the table (`at`) and its `identity`, the BLAKE3 of its
value as built. A failing case is
suspected against what it runs, which leaves out its table, and is named a
culprit when one change explains it; it is otherwise not bisected (`skipped`
is `case`), since a mixture is printed from a test's body, which holds no case.

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
skipped; `--mutate-budget N` (default 64) caps how many are judged. A case
(§8.1) reaches what its test does, its table included, and a mutant that
changes a case's value is killed by that case, which the table no longer holds.

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
| `PLY_C_CACHE=DIR` | compiled objects, the emitter's answers, each kept under the hashes of the definitions that emit it, the runtime and what it was asked, the cost checker's report on a program, kept under the checker's hash and the program's text, each tagged literal's verdict, kept under the hash of all its parser reaches and its text, and each `const fn`'s value, kept under the hash of all it reaches (default under the temp directory) |
| `PLY_C_STAGE=DIR` | the compiler's own stages, kept apart from the cache so a fresh cache reuses them; the front-end answers `ply run` files (§16); and, when the binary's `ply` program is behind its sources, the one a builder made of them and the rows that seed its next build, kept by the front end that published them (default under the temp directory) |
| `PLY_C_CACHE_MAX=BYTES` | cap on the cache and on the stages, each swept oldest first, a stage never within an hour of its last use; `0` is no cap |
| `PLY_C_KEEP=1` | keep and print the emitted `.c` and shared object |
| `PLY_C_REFUSALS=1` | print which definitions the backend refused, and how many it took |
| `PLY_C_ONLY=a,b`, `PLY_C_SKIP=prefix,...` | compile only the named definitions, or drop those with a prefix; the unit is then partial and a caller of what was dropped is declined, not raised |
| `PLY_C_PHASES=1` | print how many of the emitter's answers were read back and how many it was asked for, what emitting took, what the builder's steps took when it builds a stage, and allocation counts by kind |
| `PLY_HEAP_CENSUS=1` | count allocations by constructor, record shape and length class as well, which `PLY_C_PHASES` then prints; a map insert per allocation |
| `PLY_HEAP_POISON=1` | poison released blocks and fail on a read of one |
| `PLY_HEAP_DELAY=N` | reuse a released block only after `N` more releases |

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
                       write yield() -> Unit
                       write cancel<a>(t: Task<a>) -> Bool
                       write await<a>(t: Task<a>) -> Option<a>
                       write channel<a>(capacity: Int) -> Chan<a>
                       write send<a>(c: Chan<a>, x: a) -> Bool
                       write recv<a>(c: Chan<a>) -> Option<a>
                       write close<a>(c: Chan<a>) -> Unit }
nondet effect clock  { read  now() -> Instant
                       write sleep(d: Duration) -> Unit }
nondet effect random { write next() -> Int
                       write below(bound: Int) -> Int }
effect sim           { read  seed() -> Int }
```

* The region's row gains `sim.read`, which a deterministic test may carry.
* Virtual time advances only when no task is enabled, so `clock.sleep` costs no
  wall clock.
* A task performs against the handlers around its `task.spawn`, so a clause it
  reaches touches only the cells the task itself may (§7); a clause that binds
  `resume` is unreachable from a task (`E0502`), and a clause for a raise is
  not among them (§6.8).
* `E0413`: a `Task` or a `Chan` escapes in the region's answer: directly,
  inside a value, or inside a closure that captured it. A closure's type shows a
  captured task only as the `task.join`, `task.await` or `task.cancel` in its
  row, a captured channel as its `task.send`, `task.recv` or `task.close`, and
  either as the `sim.read` of a `simulate` it opens to use one, so a function
  whose row carries any of these may not leave the region. A task or channel
  from outside may not come in either: the region may not name a binding from
  outside it whose value holds one, nor read one from a cell older than the
  region (§7). Every region numbers its own tasks and channels, so a handle that
  reaches another region past the checker, in a closure or through a type
  parameter, fails there at run time with `E0413` rather than name one of that
  region's. `E0446`: a task is stored in a cell older than the region
  (§7). `E0414`: no progress, or a spent step budget. `E0416`: nested
  `simulate`. `E0425`: a host operation inside the region, refused before the
  handler runs and whether or not it is bound; the region answers `task`,
  `clock`, `random` and `sim.seed` itself.
* A `parallel` block (§5.9) inside a region runs its branches in turn, so the
  scheduler sees nothing of it. A branch may not open a region (`E0309`): a
  region's schedule is drawn from its entry's seed in the order regions open.

`task.cancel(t)` stops `t` where it stands: a sleep, a join or a host operation
it waits on is let go, and it performs nothing more but the `release` of each
`bracket` it stands in (§6.6). When it next runs it only unwinds, releasing what
it holds. The cancel answers `false` for a task that had
already ended and leaves its answer alone. `task.await(t)` is a join that answers
`Some` of what `t` answered, or `None` once it was cancelled; `task.join` of a
cancelled task has nothing to answer and raises (§6.8). A task cannot cancel
itself or the region's body. A deadline is the two together: one task sleeps and
cancels the other, which a third awaits. Every step a cancelled task took is read
against the cancel, so the search tries cancelling it earlier and later.

`task.channel(n)` makes a channel holding up to `n` values no receiver has taken;
`0` is a rendezvous, where a send waits for the receive that takes it. A send
waits while the channel is full and answers `true` once its value is queued or
handed over, and a receive waits while it is empty and answers the oldest value
sent. `task.close(c)` ends sending: a waiting receiver hears `None` and a waiting
sender `false`, a later send answers `false` without sending, and receives take
what was queued before the close, then `None`. Closing twice changes nothing,
and a negative capacity raises (§6.8). Cancelling a task that waits on a
channel lets go of its wait, and a value it was sending is dropped. Every
operation on one channel is ordered against every other on it, so the search
tries each order two senders or two receivers could take. A race is one channel:
each worker sends what it answers, the first receive wins, and cancelling the
others stops them.

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
is not cached. A failure prints the racing steps — each one's task, and the
definition and position where it first touched shared state — and a replay
command such as
`ply test --seed 0:0.1.0.2 --filter 'no account is ever overdrawn'`.

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

* `requires`/`ensures` go between the signature and body, in any number, as
  does `cost` (below); `result` is bound in `ensures`. `requires` restricts the domain of its
  `ensures`; it is not checked at call sites and laws do not inherit it.
* A law has a label, optional `forall` binders (typed; `E0418` if a type cannot
  be quantified, as a sum whose function performs the type's row parameter
  cannot, whatever row it is given, nor a type that holds one another module
  declares `opaque` and states no `gen` for, §3.2), an optional `where` guard,
  an optional `cost` bound (below) and a block body.
* Specs, guards and law bodies must be pure (`E0417`), except that they may
  raise (§6.8) or may not return (§5.10), and a law body may be a `simulate`
  region; a proposition that raises, or runs past its steps, is a gap in the
  claim. `law/host "..." { }` allows any effect but is
  never `proved` or cached, and is `W0604` under a hermetic run. Under `--host`
  its guard and body run against the host the run binds, and what their entries
  end with, such as a span left open (`W0609`), is reported once.
* Specs do not change a definition's hash. An `ensures` implies every resource
  outside the footprint is unchanged; there is no `old()`.
* `Int` arithmetic is checked, so bound the domain with guards as above.

| tier | claim |
| --- | --- |
| `proved` | holds for every input satisfying the guard, or a `cost` clause read off the body |
| `property` | randomized cases passed; failures shrink |
| `example` | concrete cases passed |
| `fitted` | a cost law's steps kept its bound's pace over eight or more sizes |
| `unattempted` (`W0604`) | undecided; never green, never cached |
| `defect` | Ply failed rather than the program: nothing is claimed, never cached, exit 1 |

`proved` covers ground evaluation, enumeration of finite domains up to 4096
points, linear `Int` arithmetic, case splits, congruence, constructor
injectivity, unfolding non-recursive definitions, a `match` taking its arm over
a value whose shape is in view, exhaustive interleaving, and induction: a
definition that calls only itself, and that the checker reads as ending
(§5.10), is unrolled, and over values in view as deep as the unfolding goes. On
an `Int` binder the claim is proved at `n <= 0` and then at `n > 0` from itself
at `n - 1`; on a `List` binder (structural induction) at `[]` and then at
`[h, ..t]` from itself at `t`, with `len` and `push` reduced over a spine in
view or one the proof comes to know, and `len` known to lie below `i64::MAX`.
A kernel checks each induction from the claim alone before refuting its cases,
whatever proposed it: the binder it names, a hypothesis strictly below the case
it proves, and only definitions the checker reads as ending unrolled.

Congruence and injectivity read `==` as what a value's constructors and fields
say, which a `Float` (`NaN != NaN`) and a type that states a `key` (§4.4) are
not, and arithmetic as the integers', which an operator at a type that states
`numeric` is not. A claim that holds a value of any of them, at any depth, is
never `proved`: it is run, over every point of a finite domain or over a sample.
Under `--reach` the place is `float_term` or `keyed_term`.

A sampled claim draws each binder from its type: a scalar, a list, a record or
a sum from its structure, and a type that states a generator (§4.4) through it,
at any depth. The generator is handed a size, the case's place in its run up to
63, so early cases are small, and for each of its type's parameters a generator
that chooses among sixteen values drawn of the argument. A counterexample
shrinks as it was drawn: a structural value by its parts, and a generated one by
replaying a shorter or lower record of the draws that made it, and by shrinking
the values it chose among, so what a refutation shows is still a value its
generator makes. A domain that holds such a type is what its generator draws,
so it is sampled, however small, and never enumerated; the static tier reads
the type as it reads any other. A generator that draws no value, as
`such_that` does past its budget, leaves the claim `unattempted` (`gave_up`)
rather than narrowed, and a sample counts the values its generators turned away
as `discarded`. A value of a type another module declares `opaque` (§3.2) is
drawn through the generator that module states and no other way: an `ensures`
over a parameter that holds one with none stated is sampled nowhere
(`ungeneratable`), where a law would be `E0418`. Inside the type's own module
it is drawn from its structure, as any type that states no generator is.

A law with no guard, once proved, is a lemma for every claim written below it
in its module. Its trigger is the first call its body always makes whose
arguments name every binder; where a claim makes a call that fits it, the law
at that call is a fact, and a trigger that takes an argument apart, as
`reverse(push(ys, x))` does, stands for the call in place of its body:

```ply
fn reverse(xs: List<Int>) -> List<Int> =
  match xs { [] -> [], [x, ..rest] -> push(reverse(rest), x) }

law "a push reversed leads" forall (ys: List<Int>, x: Int) {
  match reverse(push(ys, x)) { [h, ..t] -> h == x && t == reverse(ys), [] -> false }
}

law "reverse twice is identity" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }
```

Both are `proved`, the second by structural induction on `xs` citing the
first. Under `--json` a certificate's `rules` name each lemma as `lemma` with
its `law` and `label`, each induction as `induction` with its `binder`,
`def`, `over` (`int` or `list`) and `step`, and a `cost` clause read off its
body (below) as `cost_bound` with its `def`.

A `law schema` states a law once, over the definitions it is about, and a law
instantiates it:

```ply
pub law schema round_trip<a, b | e>(encode: (a) -> b / e, decode: (b) -> Option<a> / e)
  forall (x: a) { decode(encode(x)) == Some(x) }

law "a URL round-trips" = round_trip(url_encode, url_decode)
```

A schema has a name, type parameters and at most one row parameter, parameters
whose types are written, and then what a law has but a `cost`: binders, a guard
and a body, pure as a law's are (`E0417`), so what a function parameter's
written row performs past a raise or `diverges` is refused where the body calls
it. The row parameter is what the definitions a schema is given may raise:
`abort.raise`, a raise of their own (§6.8), or nothing, and an instantiation
that fills it with anything else is `E0417`. A schema binds no label. It may be
`pub`, and is imported and qualified as a definition is. An instantiation gives
one argument for each parameter, in order (`E0202`), each an expression of the
parameter's type (`E0201`, or `E0302` for a function that performs more than a
written row admits): a definition's name, a lambda, a value. It writes no
binders: the schema's are quantified at the types the arguments settle, a `new`
record's among them, which a `forall` must be able to range over (`E0418`, at
the instantiation, naming the schema's binder), and a type parameter no argument
settles stays a variable of the claim. What is instantiated must be a schema
(`E0472`), and an instantiation is never `law/host`.

An instantiation is the law its schema's guard and body are with each parameter
replaced by what it was given. It has its own label, key, tier, certificate and
cache entry; it is proved, sampled and shrunk as that law written out is, and,
proved with no guard, it is a lemma by the trigger that body has. Its hash
covers the schema's guard and body, in this package or another, with what it
was given. Under `--json` its obligation carries `schema`, the schema's
program-wide name, which `--explain` and a refutation print.

A schema also declares the definition its body is, over its parameters and then
its binders, so `round_trip(url_encode, url_decode, "a b")` is the claim at one
point, and its guard as `<name>_where`. Both are `transparent` (below), are
named among the module's other definitions (`E0105`), and are no part of what a
run counts as carrying an obligation or not. What a schema's body calls in its
own package is read from another package as any definition there is, by its
clauses unless it is `transparent` too. `std.laws` ships the schemas the
standard library states its laws with (`ply doc std.laws`).

A definition of another package
— a dependency's, or outside `--std` a shipped module's — is claimed by its
`requires` and `ensures` alone: a proof may use what it promises and never
unfolds its body, which that package's own run proves. A `transparent fn` is
claimed by its body too: a proof anywhere may unfold it, so its body, and all
it reaches, is part of what its dependents are checked and proved against.

`ply prove` reports the definitions carrying no obligation, then each
obligation's tier; `E0419` is a counterexample and `E0420` a guard admitting no
values. A proposition that raises is a gap in the claim; one the C backend
declines, or any other failure that is Ply's own, is a `defect` reported under
Ply's code (`E0505`), as `ply test` reports one. Under `--json` a gap carries
its sentence as `gap` and its kind as `gap_kind` (`unhandled_effect`,
`ungeneratable`, `raised`, `guard_not_sampled`, `reaches_host`, `not_drawn`,
`unfitted`, `gave_up`), a
defect carries `defect` — its `code`, `message`, the `bindings` Ply failed at,
and a `summary` — and `summary` counts defects as `defect`. A claim's type
variables are lettered by where they first appear among
its binders (`forall (x: a, y: List<b>)`); a sample draws each as `Int`
(`a := Int`), and a proof leaves each an uninterpreted sort
(`uninterpreted a, b`). Flags: `--prove-cases N` (below 25 kept cases only `example`),
`--prove-roots N`, `--prove-budget N` (spent reports `property`),
`--shrink-budget N`, `--prove-steps N` (calls per evaluation of a claim, default
1000000000; an evaluation past it leaves the obligation `unattempted`, and the
number keys the cached result, so more budget is a stronger claim). A `proved`
obligation is cached under its claim's hash, which reads another package's
definitions by their contracts, so it stands across an edit to a dependency's
body; a sampled one is cached under the hash of every implementation its cases
run and of each generator its points are drawn through, and is drawn again when
any of them moves. `--reach`
asks the static tier alone about every obligation the run reports on, cached or
not, and under `--json` each then carries `reach`: what it decided (`proved`,
`guard_unsatisfiable`, `open` or `budget_spent`), the steps it spent, and each
place it left the decidable fragment as `{kind, about}` — `null` for a law over
interleavings, which the static tier never sees.

A cost law states how fast a body's steps may grow with a size:

```ply
import std.list (sort)
import std.math (ilog2)

law "sorting is n log n" forall (n: Int) where n > 1 cost n * ilog2(n) {
  sort(map(range(0, n), |i: Int| i * 7919 % n))
}
```

`cost` follows the guard with the bound, an `Int` over the law's one binder,
which is an `Int` size; a cost law is never `law/host`, and its body performs
nothing (`E0463`). `ply prove` runs the body at each of the sizes 1, 2, 4 …
4096 the guard keeps, counting its steps as `metered` (§8.1) does, and stops at
a size that takes more than 10000000 (or `--prove-steps`, if lower). A size
whose bound is not positive is not read. Over the last two spans between the
sizes read, the steps may grow no faster than the bound does, give or take a
twentieth of a doubling each, so constant factors and lower-order terms do not
count. Steps that outgrow the bound over both spans are `outgrown` (`E0464`),
which fails the run as a refutation does and lists every size with its steps
and bound. A law that keeps pace is `fitted` over eight sizes or more and
`example` over fewer, and one with fewer than three sizes to read is the gap
`unfitted`. Under `--json` each carries `fit`: its `measures` (`size`, `steps`,
`bound`), the size that took more steps than it was allowed as `spent` (`size`,
`limit`), and a `summary`; and `summary` counts `fitted` and `outgrown`.

A `cost` clause bounds the steps a call of its definition takes:

```ply
fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost len(xs) * (len(ys) + 1)
= fold(xs, 0, |a: Int, x: Int| fold(ys, a, |b: Int, y: Int| b + x * y))
```

The bound is an `Int` over the parameters, and the steps may be at most a
constant times it plus a constant at every size, zero included: with `ys`
empty these steps still grow with `xs`, so `len(xs) * len(ys)` would not hold.
A clause on a definition whose row writes `diverges` is `E0468`, and on one
that performs more than a raise `E0463`, since a handler could change the
steps.

A function parameter may name the steps a call of it takes, after its type,
and only the definition's `cost` clauses read that name, as an `Int` of at
least 1 (`E0105` if it is named like a parameter or another cost name):

```ply
fn each_of<| e>(xs: List<Int>, f: (Int) -> Int / e cost k) -> Int / e
  cost len(xs) * k
= fold(xs, 0, |a: Int, x: Int| a + f(x))

fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost len(xs) * (len(ys) + 1)
= each_of(xs, |_x: Int| total(ys))
```

At a call the name stands for the steps of the function given: a lambda's
body, a named definition's, or the caller's own parameter's cost name, so
`pairs` composes `each_of`'s bound with its lambda's. A function given whose
steps grow with what it is given leaves the call unread.

`ply prove` first reads the steps off the body. A builtin is at most a step;
`map`, `filter`, `fold`, `map_fold`, `iterate`, `map_update`, `bytes_position`
and `metered` call a literal lambda or a named definition back as many times as
an argument's size allows; a call of a definition costs its steps at the sizes
of its arguments, and of one whose body another package keeps, its first `cost`
bound read from above; a raise ends the call. A recursion is read when every
call back takes a part of one list parameter that a list pattern took a head
off, no path calls back twice, and every other parameter its steps grow with is
handed on unchanged or as a part of itself: it takes that list's length in
calls. The bound is read from below: the lengths of parameters (`len`,
`map_len`, `string_len`, `bytes_len`), cost names, positive literals, `+`, `*`,
and `ilog2` alone or beside sizes of the one length it is of. Steps within the
bound are `proved`. Anything else, such as another operation, a call of a
function value with no cost name, a group of definitions calling each other, a
body another package keeps with no `cost` bound, an `Int` parameter the steps
grow with, or a definition that takes, answers or builds a value of a type that
states a `key` or a `numeric` (§4.4), whose comparisons and operators call it
unseen, leaves the clause to
the cost law `cost of <name>` (`#2` and on for
later clauses): it makes each parameter of one size `n`, an `Int` being `n` and
a list, map, string or bytes holding `n` elements made of their index, and a
function parameter with a cost name a closure of one step, its cost name 1, and
meters the call alone, so it is `fitted`, `outgrown` (`E0464`) or a gap as
above. A parameter of any other type is not made, which leaves the law
`unattempted`. Under `--reach` a clause the body does not show carries the
blocker `unbounded`, with why.

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
| `bin` | `<snake_case(T)>_bin` | `std.bin.BinCodec<T>` |
| `show` | `<snake_case(T)>_show` | `{show: (T) -> String}`, writing what `std.show.show` does |
| `hash` | `<snake_case(T)>_hash` | `{hash: (T) -> Bytes}`, the value's `digest` |

There are no other derivers (`E0207`). `snake_case` is `std.text`'s, so a
name collision (`HTTPRequest` and `HttpRequest` both give `http_request`) is
`E0105`. A `derive` must be in the
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
plain values: `json::decode_bytes(body, order_json())`. A `new` record (§4.2)
derives as an alias of its fields does: the same documents, bytes and shape.

A `json` or `bin` codec whose type reaches itself, directly or through the
other types the module derives, is two definitions: `tree_json()` starts
`tree_json_at(json::max_depth())`, and each level hands the next one less, so
the recursion is seen to end (§5.10). Past the bound, a decode is an error and an
encode panics. `json`'s bound is 128 levels, as deep as `parse` reads, and
`bin`'s is 10,000, as deep as calls nest (§5.7).

`E0206` names the field that blocks a derivation: function types, `Cell`, `Task`
and `Chan` (all derivers); `Float` (`ord`, `hash`); `Secret` (`json`, `ord`,
`bin`, `show`, `hash`); `Option<Unit>` and `Option<Option<a>>` (`json`). `json`
and `bin` need their module imported (`import std.json`, `import std.bin`), or
the `derive` is `E0206`; `show` imports `std.show` itself. A type that binds a
label (§4.5), or a field whose type is given one, is `E0206` too: only a function's
row has a use for a label. A row parameter the fields leave unused is carried
through, `tagged_eq : <a | e>({eq: ..}) -> {eq: (Tagged<a | e>, ..) -> Bool}`.

A type that states a `key` (§4.4) derives `eq`, `ord` and `hash` from what the
key answers, and one that states a `show` derives `show` from that function: the
dictionaries call `==`, `compare`, `digest` and `show`, which read them. `json`
and `bin` encode the fields as they are.

## 12. Builtins

The functions every module calls without importing them. Each is declared in the
compiler's prelude as an `extern fn`: a signature the runtime implements, with a
row and a `where` like any other and no body, and a doc. Only the prelude
declares one; an `extern fn` in a module is `E0151`. A module may shadow any
except `compare_values` and `map_of_entries`, which the map and set literals are
written in, and the six the wrapping and saturating operators are written in
(§2.4), which no binding around such an operator may hide either (`E0105`).
`to_int` reads a value of any integer type as an `Int`, answering `None` past
its range, so with `numeric_of_int` it converts between any two.
`ply doc prelude` lists them all, each with the summary of
its doc, and `ply doc NAME` prints one: its signature with the parameters' names,
and its doc.

## 13. The standard library

The built-in package, shipped inside `ply` and pre-seeded for every load — an
implicit dependency of every package, no declaration needed: `import
std.<name>`. Its tests and obligations are skipped unless you pass `--std`.
Each module is documented in its source: `ply std` lists the modules, each with
the summary of its doc; `ply doc std.json` documents a module and everything it
publishes, `ply doc std.json.parse` one definition, `ply doc fs.read_at` one
operation of an effect; and `ply std --show std.json` prints one source, `ply
std --show` alone every one. A load reads only the shipped modules its modules
import, what those import in turn, and what they embed, so a change to any other
leaves it alone; a change to one it reads warns `W0605`. Of those it checks,
counts and hashes only the functions the program reaches and the names its
modules import, beside every type and effect and the functions a type's `key`,
`show`, `numeric` or `gen` names (§4.4), and none of their tests or laws; a
function it reaches is read with what its specifications name (§10), which a
dependent's proof is owed; `--std` reads each one whole.

## 14. The host boundary

Without `--host`, an operation that reaches the boundary is `E0424`, naming the
handler that would serve it. With `--host`, a test that can reach a bound
nondeterministic handler is cached with what it read and the binding it ran
under (§8.2), so its pass never answers for a hermetic run; one that reaches only
deterministic handlers, whose answers are a function of what they are handed, is
cached like any other. An operation performed inside a `simulate`
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
| `--allow NAME` | repeatable privileged family lent to the program, which must declare the effect it lends: `machine`, `tester`, `claims` (effect `prover`), `hosts` (`tcb`) or `shipped` (declared in `compiler.unit`) (`ply run`, `ply test`); `E0459` otherwise. `machine`, `tester`, `claims` and `hosts` also lend a deterministic `hermetic_` half of the same operations (`hermetic_machine` …), which answers from what it is handed alone: no host, clock, file or cache. A test's handler answers the family with it and stays cached. `shipped` is deterministic: the modules, the version, the C runtime and the builtins this binary ships, and `reached`, which tells what traces the run what a load read of those modules |
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
`<name>.plyz`. A consumer compiles those sources. It is a package and never a
program: `ply run lib.plyz` refuses it (`E0443`) rather than reading a
container as text.

`ply build` writes the closure of one entry point (default `main`) as a `.plyx`
file (default `<entry module>.plyx`): its definitions by hash, the same
definitions printed back to source without tests, laws, comments or anything
unreached, and the runnable `ply run` loads — that source checked again, its
front end's answer and its compiled unit, which holds the value of each
`const fn` (§3.5) — so a run of it runs no front end and evaluates none of them.
The BLAKE3 digest covers those and the entry point, so an edit nothing reaches
leaves it unchanged; a failure raised by a run of it carries no line number. A
part that does not agree with the rest — a body under a hash that does not name
it, a name with no body, a closure or runnable that is not the one these
definitions make — is `E0443`, as is a build whose closure holds two identical
declarations it cannot tell apart (two effects, or two members of one recursive
group); an artifact built by another compiler, or compiled for another runtime,
is `E0444`: rebuild it with this `ply`. The standard library definitions it draws
on are its own, so a `ply` that ships other ones runs it as it was built, reading
none of its own; `--diff` names what a rebuild would change.
`--config-schema` ships that function too, resolved as a run resolves it: a name
that is not a nullary pure function returning a `ConfigSpec` is `E0440`.

### 15.1 The registry

A library is published to a registry and depended on from it (§3.3). A registry
is a directory of files behind an HTTP server, laid out statically:

```
GET  /<name>/index.json                      every version of <name>, newest last
GET  /<name>/<version>/package.plyz          the library's `.plyz`, as `ply build` writes it
GET  /<name>/<version>/package.plyz.b3       its digest, one `b3:<hex>` line
GET  /<name>/<version>/interface/<semantics> the interface its publisher cut under <semantics>
GET  /<name>/<version>/attestation/<semantics> what the registry's attester found of it
PUT  /<name>/<version>                       publish: the `.plyz` as the body
PUT  /<name>/<version>/interface/<semantics> the interface beside it
PUT  /<name>/<version>/attestation/<semantics> an attestation its attester's key signed
POST /<name>/<version>/yank                  mark the version yanked
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
registry recomputes the digest from the body and refuses a mismatch, refuses
other bytes under a version it already lists — a published version never
changes, and the fix is a new version; the same bytes again are the publish it
holds — and refuses an archive whose manifest is not the package and version it
was sent as, names an entry, or depends on anything but the registry; each
refusal is `E0144` with the registry's reason.

A version is held to what it changes. `ply publish` compares the contract of
every public definition — its signature and specifications, and its body when
it is `transparent`; a type's or effect's whole declaration — with those of the
highest unyanked version published below it, each derived from its archive by
this `ply` as a consumer reads it: a patch moves no contract, a minor only adds
definitions, and a major may change or remove any. A version that bumps less
than its changes need is `E0150`, naming what moved and the least version that
says so, before anything is sent; 0.x versions follow the same places.
`ply contracts NAME FROM TO` lists what moved between two published versions.

After the archive, `ply publish` sends the package's **interface**: what a load
of it as a dependency cuts (§16), each module's stub keyed by the sources and
manifests its analysis read, framed with the semantics version of the `ply` that
cut it and the archive's digest. The registry keeps one per version and
semantics and refuses a frame that names another archive or semantics; the same
bytes again are the one it holds, so a publish whose interface was refused runs
again whole. The **semantics version** names what a definition's hashes, its
checked rows and a claim's verdict mean — `ply publish --json` reports it — and
moves only when one of them does, so a `ply` that changes nothing they mean
reads an interface another cut. `ply resolve` fetches the interface beside each
archive into the dependency's slot, and the first load reads the dependency
through it rather than analysing its source. `--verify-deps` (`check`, `test`,
`prove`) reads every dependency from source instead and refuses one whose
interface does not re-derive from it, `E0149`.

A registry with an **attester** vouches for what it serves. `ply attest NAME
VERSION` lays the published version out as a project of its own, fetched and
checked as a resolve fetches it, and runs `ply test` and `ply prove` over it:
whether it checks, whether its `reuse fn` and `returns` promises are kept, its
tests and the tier each claim was discharged at, under this `ply`'s semantics.
With `--sign KEY` the answer, a `std.pkg.Attestation`, is signed as §15.2 signs
a build and sent back; the registry keeps it only when its `attester`'s public key
signed it, for the version's own archive and the semantics the path names.
Without `--sign` nothing is sent, which is how anyone runs an attestation again.
A registry run with `--set attester=<public key hex>`, `--set attest.key=<secret
key file>`, `--set url=<where the attester reaches it>` and `--exec
attest=<a ply>` attests every version published, one at a time between
connections, with that `ply`. A version whose tests fail or whose claim is refuted
is published all the same, and its attestation says so. `ply resolve` fetches
each dependency's attestation and believes it only when a key `PLY_ATTESTERS`
names (public key files, separated as `PATH` separates directories) signed it for
the archive the lock pins; `--json` reports each one as `attestation`
(`attested`, `trusted`, `signer`, and what was run).
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

### 15.2 Signing

```
$ ply keygen release.key                       # release.key and release.key.pub
$ ply build . -o app.plyx --sign release.key   # app.plyx and app.plyx.sig
$ ply run app.plyx --require-signer release.key.pub
$ ply build . -o app.plyx --verify             # compare, write nothing
```

`ply keygen PATH` writes an Ed25519 key pair (`std.ed25519`): the secret key at `PATH`,
readable by its owner alone, and the public key at `PATH.pub`, each one line
naming what it holds and 64 hex digits. It never writes over a file, and a key
file that cannot be read, decoded or written is `E0462`.

`ply build --sign KEY` signs what it writes, a program or a library, in
`<artifact>.sig` beside it. A signature is detached, so the artifact's digest
is the same whoever signs it, and signing the same build again with another key
adds a signature beside the first. What is signed is the artifact's provenance:
its kind and name, its full digest, the `ply` that built it, the semantics
version (§15.1), and the commit `HEAD` named when its sources were in a git
repository. `ply run ARTIFACT --require-signer KEY` runs a built artifact only
when one of the public keys named (the flag repeats) signed that provenance for
the artifact's own digest; an artifact with no signatures, signatures for other
bytes, or none by a trusted key is `E0460`, before anything of it is loaded.
`ply build --verify` builds in memory and holds the file `-o` names to what these
sources build, and every signature beside it to that build; it writes nothing and
answers which keys signed. A file that is not that build, or a signature that does
not hold, is `E0461`.

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
| 4 | `ply test --kept`: what earlier runs kept does not answer the run |
| *n* | `process.exit[p](n)` under `ply run --host`: the program's own, `0` to `125` |

Flag groups: *simulation* (§9), *host* (`--host` and §14's flags except trace
and drain), *prove* (`--prove-cases`, `--prove-roots`, `--prove-budget`,
`--shrink-budget`, `--prove-steps`), *trace* (`--trace`, `--trace-level`),
*drain* (`--drain-ms`, `--drain-lead-ms`).

| command | flags |
| --- | --- |
| `ply new PATH` | `--name NAME` (default: the path's last segment), `--lib` (no `main`, a `pub` definition instead); refuses a name that is not a package name and a directory that is already there |
| `ply check [path]` | `--types`, `--costs`, `--explain` (front-end phases, how many definitions the front-end cache seeded and how many were checked, and the modules a compiled package stood for; with `--types`, effect sets and provenance), `--workspace`, `--verify-deps` |
| `ply test [path]` | `--filter`, `--jobs`/`-j`, `--steps`, `--timeout`, `--no-cache`, `--kept`, `--explain`, `--watch`, `--bisect`, `--bisect-budget`, `--coverage`, `--mutate [DEF]`, `--mutate-budget`, `--profile`, `--std`, `--workspace`, `--verify-deps`, host, simulation |
| `ply run [path] [-- ARGS]` | `--seed` (one interleaving always), `--steps` and `--timeout` (both default to no bound: an entry that serves forever is a program), `--profile`, `--explain` (whether the front end ran or an earlier run's answer was reused, and the load's phases), `--require-signer KEY` (repeatable; §15.2), host, trace, drain; `ARGS` is what `process.args` answers; a `.plyx` path runs the artifact |
| `ply prove [path]` | `--filter`, `--jobs`, `--no-cache`, `--no-incremental`, `--explain`, `--reach`, `--std`, `--workspace`, `--verify-deps`, host, trace, prove, simulation |
| `ply review [path]` | `--changed` (default), `--accept`, `--no-cache`, `--no-incremental`, `--std`, prove, simulation |
| `ply build [path]` | `--entry NAME`, `-o FILE` (default `<entry module>.plyx` for a program, `<package>.plyz` for a library), `--config-schema`, `--digest`, `--diff OLD.plyx`, `--sign KEY` (signatures in `<FILE>.sig`), `--verify` (compare, write nothing; §15.2) |
| `ply hosts [path]` | host, trace, drain, `--digest` |
| `ply std` | `--show [MODULE]`, `--digest`; no path |
| `ply explain CODE` | one line on what the code means; `--all` lists every code; no path |
| `ply doc NAME [path]` | what a full or unique simple name names (§2.1): a definition's signature with the written parameter names, its doc, `returns` and specification clauses, place, hash, footprint, and the tests and laws that name it; a law schema as it is written, with the laws that instantiate it; a type with its fields or variants (an `opaque` sum's are its module's, and are not listed), an effect with its operations (one is `effect.op`), an effect set, or a module with what it publishes, each with its doc; a builtin as the prelude declares it, and `prelude` every builtin. A name the program does not hold is looked up among the builtins, then the shipped modules |
| `ply fmt [paths]` | rewrite every `.ply` file under the paths in the canonical layout; `--check` writes nothing and exits 1 naming the files that would change, and `--json` is a report of exactly that, so it requires `--check` |
| `ply show NAME [path]` | one `fn` or `type` as its file holds it: its doc and the comment lines above it, `pub`, the body, and a comment ending its last line; `--json` adds the byte range |
| `ply replace NAME [path]` | rewrite one `fn` or `type` from `--with FILE` or stdin, formatted, every other byte of the file kept; refused with `E0128` (exit 2, nothing written) unless the program still checks and no other definition's name or hash moves; `--check` writes nothing |
| `ply resolve [path]` | write `ply.lock` from this project's manifest closure, listing every dependency's name, version and source digest, with no module parsed or checked; the one command that fetches registry dependencies, from `PLY_REGISTRY`, each with the interface and the attestation published for it under this `ply`'s semantics when there are (`interface` and `attestation` in `--json`, the attestation believed only as `PLY_ATTESTERS` says; §15.1) |
| `ply vendor [path]` | copy the closure into `vendor/`, one directory per package plus an index, so the project builds with no cache and no network |
| `ply why NAME [path]` | why a package is in the closure: the path from the root package to it, then the version and digest the closure pins |
| `ply publish [path]` | build this library's `.plyz` and upload it, then its interface, to `PLY_REGISTRY` under `PLY_REGISTRY_TOKEN`, once its version is the bump its changes need (§15.1) |
| `ply yank NAME VERSION` | mark a published version yanked, under `PLY_REGISTRY_TOKEN`; no path |
| `ply attest NAME VERSION` | run a published version's tests, claims and promises over what the registry serves and report the attestation; `--sign KEY` signs it and sends it to the registry (§15.1); no path |
| `ply keygen PATH` | an Ed25519 key pair: the secret key at `PATH`, the public key at `PATH.pub` (§15.2); no path |
| `ply contracts NAME FROM TO` | the public definitions whose contracts were added, changed or removed between two published versions, the bump that needs, and whether `TO` makes it (`needs`, `kept`); no path |
| `ply hash [path]` | `--deps` (references and transitive closure) |
| `ply defs [path]` | every definition: place, hash, signature, footprint, references; `--filter SUBSTRING` |
| `ply callers DEF [path]` | what mentions a definition directly, and every definition, and every test and law of the run's own modules, whose closure reaches it |
| `ply bootstrap <path>` | writes a program this binary ships as its launcher enters it: the builder (`build.main`) or `ply` (`ply.main`), as `<module>.run` beside the `<module>.digest` the launcher gates it on and the `<module>.key` a builder takes it under; `--out DIR` (default `bootstrap`), `--verify` (compare, write nothing) |
| `ply cache clear\|stats\|compact [path]` | discard the store and the compiled package / report what it holds and its reclaimable space / reclaim it |
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
`ply prove` drive a nested program on a machine of their own, `ply hosts` and
`ply cache` are answered what a run would bind and what the store holds, and
`ply replace` is lent the text it puts in a definition's place, from
`--with FILE` or stdin. The program emits the C of every unit a command runs,
and of what `ply bootstrap` writes, itself: the emitter's answer for a
definition is kept under the toolchain's cache and read back while the
definition, the emitter and the runtime are the ones it was made by, and the
machine compiles the C it is handed and loads it. What the emitter needs of a
body beyond its text — the type each operator reads its operands at, the shape
of each record update, the labels and witnesses each call passes — the front
end's check files with the body's row, placed from where the definition
starts, so a body is walked once however often it is lowered or moved. The launcher enters `ply`
from a runnable — its front end's answer, its sources and its unit's C, which
reading runs no compiler: the committed one when it was built from the
binary's own sources, else one a builder made of them for an earlier run, or
makes now.
The builder is the compiler's own `build.main`, entered the same way: it
reads a program's sources and hashes them, and where the definition the program
enters hashes as one of a program it already built, for this runtime and by
this compiler, that runnable is the program: a hash covers all its definition
reaches and no comment, layout, test or definition nothing reaches. Otherwise it
checks the sources, seeded with the rows its last build of that program
kept, emits its unit with the emitter's answers kept, and writes the runnable,
keeping it under that key in `programs/` below the stage root. A program it
ships holds no test and no law, which nothing it enters reaches, and a module
whose text has not moved since its last build of the program, and that imports
none whose text has, it reads as that build cut it: its signatures and
declarations, with the rows and hashes kept for its bodies. What a module embeds
counts as its text does: a file it embeds that reads otherwise has the module,
and every module importing it, read from source. The answer is the
one a build from every source gives. The launcher
lays the committed runnable there under the key committed beside it.
The committed builder builds `ply`, so each build reads back what the ones
before it kept; where `ply`'s sources need a rule that builder lacks, it builds
the builder of the binary's own compiler first, and that one builds `ply`. `ply std`
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
An allocation with no `ply_*` frame on its stack is counted under the site
`<no ply frame>`.
What a host may lend is a policy with names, one family each:
`machine` (load, bind, enter and call a nested program), `tester`, `claims`,
`hosts` and `shipped`, each with a summary a
reviewer can read. The launcher lends its own program every family; another host
names the ones it means, so `machine` — which drives another machine — is
granted on purpose and not by accident. A program lent `machine` hands it a
program as a front end's answer and the C of its unit: `compiler.load`'s
`load[r](root)` reads the `.ply` files under `root` through the `fs` root `r`,
the path dependencies its `ply.pkg` names and what its modules embed, checks
them against the shipped modules and emits their unit afresh, so a run of it is
granted `--fs r=PATH` and `--allow shipped`, and `machine.load` compiles the C
it is handed and runs no front end of its own. A git dependency is `ply`'s to
fetch, and `compiler.load` refuses one.
The first run after `ply` or the program itself changes compiles the program's unit,
which needs the C toolchain `ply run` needs and takes a few seconds; every later
run loads the compiled object and the front end it filed beside it. A command
that loads a program reads the front-end cache under `.ply-cache` before it
analyses and files what it answered after, unless the cache already holds all of
it from the same load: a definition whose hash has not moved
since it was filed is taken from its filed rows, so a run checks what an edit
moved and what reaches it, and a definition generic over an effect row or a
label every time. The hash it is filed under reads a definition of another
package by its contract, its signature and specifications, so an edit to a
dependency's body checks that package's definitions again and leaves its
dependents' rows standing. A load reads its dependencies and the shipped modules it
pulls through the compiled package an earlier load kept in
`.ply-cache/interfaces/`: each module with its function bodies cut out, beside
every definition's hash, references, effects and specifications, which the
front end takes as they are. A module whose source or package manifest moved
since, or that embeds a file that reads otherwise, or that imports a module
either is true of, is read from source, as is one the package
lacks; a shipped module that moved only by gaining definitions, every one the
package fixed of it hashing as it did, is read from source alone, and what
imports it still reads its stub. The package is cut again from the load's own
analysis, so a package
costs no analysis of its own; a project keeps one per semantics version (§15.1),
so a `ply` that changes nothing a hash or a row means reads the one another
kept. A registry dependency's first load reads it through the interface its
publisher sent. `ply check` of a whole project reads the project's own modules
the same way, from what its last check kept beside the package: a module that
still stands enters with its bodies cut out and its tests and laws as written,
its rows answer for the bodies, and what the check warned of it is said again.
Its answer is the one a check of every source gives. A run about the
shipped modules — `--std`, or a project whose own modules ship — reads them from
source. `ply build`, `ply hosts`, `ply test --no-cache` and `--no-incremental`
read and file neither, and neither does `compiler.load`. A cache that will not read
is a warning and a cold check, never a failure; the run that files over one
that filed a shipped module this load reads with other bytes says so once, as
`W0605`, naming the modules and how many definitions the change reached.

`ply run` over sources goes further: once a load holds, the front end's answer
is filed under a key of everything it and the `reuse fn` promise check (`E0127`)
read — the name and bytes of every module the walk read, the root's manifest,
each dependency's key, manifest and modules, what the modules embed, the root's
absolute path, the `ply`
program and the modules it ships as the launcher gates them, the binary's
version, and `--config-schema`. A later run whose walk hashes
the same takes that answer and runs neither the front end nor the promise check,
which the filed load passed; it binds, grants (`--allow`, `--exec`, `--fs`) and
picks its entry anew, and reports exactly what a run that built the answer
reports. Any edit to a module, a dependency or a manifest, another schema or
another `ply` is a new key, and the front end runs again; `ply.lock` is not
read by a run and is not in the key. A single `.ply` file keys that one module.
The answers live under the stage root (`PLY_C_STAGE`, §8.6) in `reused/`,
one file per key, each written beside itself and renamed into place, so two runs
of one package never read half of one; an entry that does not read is rebuilt
and written over. They are swept with the stages, least recently used first, down to
`PLY_C_CACHE_MAX`, never one used within the hour, and deleting them is always
safe. `ply run --explain` says `reused` or `built`, the key, and what reading,
the front end, filing into `.ply-cache` and the machine's load each took, on
stderr before the entry runs, or as `front_end` in the `--json` document.

`ply check` takes its own answer back the same way. A check the front end
answered whole is filed in `reused/` under the walk's key, the interface
each registry dependency's slot holds, the paths its reports name, and the flags
that shape what it prints (`--types`, `--costs`, `--json`, `--verify-deps`,
color). What the front-end cache said of itself (`W0601`, `W0602`, `W0603`,
`W0605`) is left out of what is filed, since a check that does not read the cache
has nothing to say about it. A later check whose key matches prints that answer
and exits with its code without running the front end, so a tree unchanged since
its last check is answered in the time its walk takes. `--explain` always checks
afresh, since what it reports is this run's. In the `--json` document,
`front_end` carries `reused` and `key`: alone for an answer taken back, and
beside the phases of a check that ran, with `filed`, whether the next check can
take its answer back (false for a refused load, or when the entry could not be
written).

`ply test` keeps its answer for the next run over the same command line, files
and `ply`, in the test store beside its passes: the answer of a run that ran no
test in scope or, for a green `--json` run, the one selecting again after it
filed its passes gives, without what the front-end cache said of itself. A later run whose key matches answers with it
without loading the program while every pass it took still stands and every
shipped definition its program reached hashes as it did (§13); otherwise it
loads and selects as usual. `--watch`, `--explain`, `--coverage`, `--mutate`,
`--no-cache` and a configuration file or schema always load. In the `--json`
document, `front_end` carries `reused` and `key` for an answer taken back,
beside the phases of this run, all of it read. A `--json` run of several
`--filter`s also keeps, for each, the answer a run of it alone gives. `--kept`
answers only from what was kept: where no kept answer of the whole command line
stands, a run of several filters asks each what a run of it alone kept, exits 0
where every one is answered and otherwise lists the rest under `unanswered`;
exit 4 is a run nothing kept answers, with nothing loaded, so a caller can tell
which runs have work before starting them. `ply defs` keeps its listing the same
way, beside every embed the program read, and a later listing over the same
command line, files and `ply` takes it back while every shipped definition the
program reached hashes as it did; its `--json` document then carries
`front_end` with `reused` and `key`.

A load that checked has not yet run a tagged literal's parser, its `literal` or the
`compile` of one with holes (§2.3). `ply check`,
`ply run`, `ply test`, `ply build` and `ply prove` settle the literals of the
root package's modules before they do anything else with the load, and refuse it
with what a parser refused (`E0153`, `E0154`): each literal is the call of its
parser, entered on a machine of its own, lent to nothing, over a unit of the
definitions the parsers reach, under a budget of ten million calls a literal.
A verdict is a function of the parser and the texts, so it is kept in the
toolchain's cache (§8.6) under the hash of all the parser reaches, the texts and
the toolchain, and a parser is entered again only for a text it has not read or
after an edit to something it reaches; a check that touches no literal's parser
enters nothing. A module that writes a tagged literal is read from its source
by every check, never as its stub. A dependency's literals are its own run's
to settle, as its tests are, and `compiler.load` settles none: there, as in any
load nothing settled, a literal whose parser refuses it ends the run where it
is read (`E0502`).

The same commands evaluate each `const fn` the program holds (§3.5), whichever
package declares it, on a machine of its own after the literals' and under its
own budget: its value is kept in the toolchain's cache under the definition's
hash and the toolchain, and a definition is entered again only after an edit to
something it reaches or to a file it embeds. One that raises is entered by each
load until it is mended, so its diagnostic names the place as the sources stand.
A module that declares one is read as its stub like any other: the value is the
definition's, wherever its body is read from. `compiler.load` evaluates none, and
there a `const fn` is a definition that takes nothing, evaluated once a run.

`ply fmt` keeps comments, the spelling of every literal, and the order of
imports, items and statements, and keeps a doc comment directly above what it
documents; it prints `formatted PATH` per file it changed
and leaves a file that does not parse alone, exiting 2 with the diagnostic. A
line string starts a line of its own, each of its lines at the first's indent,
and as the last item of a list it takes no trailing comma. A file it cannot read
or write back is an error too, exiting 2, so `--check` never passes over a file
it did not read. A directory whose name starts with
`.`, and one named `target`, are not walked; a symlink found while walking is
passed over, and one named on the command line is an error rather than a file to
rewrite.

`ply show NAME` and `ply replace NAME` are the edit loop for one definition: read
it, rewrite it, and touch nothing else in the file. The replacement is one item
of the same kind and name, with its own doc above it; `replace` prints it
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
| `E0003` | a doc comment documents nothing: no declaration follows a `///` line, or a `//!` line is not at the head of its file |
| `E0101` | unknown name (including no `main` to run) |
| `E0102` | unknown type |
| `E0103` | unknown effect |
| `E0104` | unknown operation |
| `E0105` | duplicate definition, or a reserved name |
| `E0106` | unknown module |
| `E0107` | private name, or a constructor of an `opaque` type outside its module |
| `E0108` | ambiguous import |
| `E0109` | module cycle |
| `E0110` | duplicate import |
| `E0111` | file path that cannot name a module |
| `E0112` | ambiguous entry point |
| `E0114` | unknown `effect set` |
| `E0115` | `effect set` cycle |
| `E0116` | record update base that is not a record of a known type |
| `E0117` | record update naming a field the base lacks |
| `E0118` | `?` with no written `Result`/`Option` return type to exit through, or a `try` or a tagged literal where `Ok` or `Err` is rebound |
| `E0119` | `?` where its early exit would change what runs or drop an annotation |
| `E0120` | parameter default on a lambda, operation or handler clause |
| `E0121` | parameter default that is not a pure, closed value |
| `E0122` | default on a `pub fn` naming something its module does not export, or building an `opaque` record |
| `E0123` | named argument naming no parameter, or one twice |
| `E0124` | positional argument after a named one |
| `E0125` | parameter left unfilled by a call that used a name |
| `E0126` | top-level `fn` missing a parameter or return type |
| `E0127` | `reuse fn` with an update that cannot happen in place |
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
| `E0141` | a registry that could not be asked: unset, malformed or not answering |
| `E0142` | a registry archive that is not the one the lock pins or the index lists |
| `E0143` | a registry dependency no published version satisfies |
| `E0144` | a publish or a yank the registry refused |
| `E0145` | a package or a version no registry takes |
| `E0146` | an embed whose file or directory could not be read |
| `E0147` | an embed whose path is not a string literal |
| `E0148` | a `returns` clause the body does not keep |
| `E0149` | a dependency's published interface that does not re-derive from its source |
| `E0150` | a version whose changes need a larger bump than it makes |
| `E0151` | an `extern fn` outside the prelude |
| `E0152` | a tagged literal whose tag's `literal`, or `compile` or `fill` for one with holes, is not what a tagged literal calls |
| `E0153` | a tagged literal its parser refuses |
| `E0154` | a tagged literal whose parser raised or spent its budget |
| `E0156` | a `const fn` that takes or binds a parameter, writes a row, performs more than a raise, may not return, or answers a value a build cannot keep |
| `E0157` | a `const fn` that raised when the build evaluated it |
| `E0158` | a `const fn` that spent the build's budget, in calls or in the size of its value |
| `E0201` | type mismatch |
| `E0202` | arity mismatch |
| `E0203` | occurs check |
| `E0204` | not a function |
| `E0205` | non-exhaustive match |
| `E0206` | not derivable, including an unordered `Map` key, a `key` that is not ordered and hashed, and a test's case of a type hashed only through a `key` |
| `E0207` | unknown deriver |
| `E0208` | orphan `derive`, `key` or `show` |
| `E0209` | `/` or `%` where no number has it: `/` on `Decimal`, either on a `numeric` type parameter |
| `E0210` | operand type nothing determines |
| `E0211` | integer literal out of range for its fixed width |
| `E0212` | the alternatives of an or-pattern bind different names |
| `E0213` | a `let` whose or-pattern can fail has no `else` |
| `E0214` | a `new` record whose fields reach the record itself |
| `E0215` | a `numeric` that does not name each of `add`, `sub`, `mul`, `neg` and `of_int` once |
| `E0216` | a `numeric` for a type that takes parameters |
| `E0217` | a `key`, a `show` or a `numeric` for a type that is not a sum |
| `E0218` | a `key`, a `show` or a `numeric` naming a function that does not fit: it takes more than the value, is not over the type's own parameters, has a `where`, binds a resource label, or does not only answer; a `numeric`'s is not `(T, T) -> T`, `(T) -> T` for `neg` or `(Int) -> T` for `of_int` |
| `E0219` | a `key` whose answer is compared through the type it is the key of |
| `E0220` | a record literal or an update that would build an `opaque` record outside its module |
| `E0221` | `opaque` on an alias, which has no values of its own |
| `E0222` | a range pattern whose bounds are not two integer literals of one type, the first no greater than the second |
| `E0223` | a wrapping, saturating or checked builtin, an operator written in one, or a rotation at a type that states its own arithmetic |
| `E0301` | unbound row variable |
| `E0302` | effect not permitted by the written row |
| `E0303` | unhandled effect (compiler defect) |
| `E0304` | resource label required |
| `E0305` | `handle` leaves an operation, or a mode atom, under a handled mode atom unanswered |
| `E0306` | label instantiation: a call leaves a label unfilled or writes the wrong number of them, or a label-generic definition is used as a value |
| `E0307` | mutually recursive definitions binding different label or row parameters |
| `E0308` | polymorphic recursion: a call inside a recursive group asks for another row or type parameter than the group was checked with |
| `E0309` | `parallel` branches that may not run at once: they touch one resource where one writes, or one opens a `simulate` region or performs a `task` operation |
| `E0310` | a `numeric` or `integer` constraint no call can pass the type of: the definition used as a value, a parameter its signature never mentions, or a call from another member of its recursive group |
| `E0311` | `try` whose body's row names no `raise` operation to answer |
| `E0312` | `try` whose body's row names several `raise` operations, and it names none |
| `E0313` | `try` naming an operation that is not a `raise` |
| `E0412` | nondeterministic effect in a deterministic test |
| `E0413` | `Task` or `Chan` escapes its region, or enters another |
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
| `E0444` | artifact built by another compiler or for another runtime |
| `E0445` | `trace.exit` of a span not open on this task |
| `E0446` | value outlives its region |
| `E0448` | definition the C backend cannot compile |
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
| `E0460` | artifact `--require-signer` refuses: unsigned, signed for other bytes, or by no trusted key |
| `E0461` | `ply build --verify`: a file or signature that is not what these sources build |
| `E0462` | key file that cannot be read, decoded or written |
| `E0463` | cost law or `cost` bound that cannot be measured: not one `Int` size, `law/host`, or a body or definition that performs |
| `E0464` | cost law or `cost` bound whose steps outgrew it |
| `E0465` | an operation a row promises `bounded` that grows with the input |
| `E0466` | `bounded` outside a definition's own row |
| `E0467` | `decreases` no proof shows descends at every call its group makes |
| `E0468` | `cost` bound on a definition whose row says it may not return |
| `E0469` | a test's table or label that performs more than raises |
| `E0472` | a law instantiating a definition that is not a `law schema` |
| `E0473` | `gen` for an alias or a type that binds a label or a row, or one whose function is no generator of the type |
| `E0501` | assertion failed |
| `E0502` | runtime error: `panic`, a raise nothing answers, division by zero, overflow, bad index, spent budget, call limit |
| `E0503` | spent its step budget without finishing |
| `E0505` | Ply broke one of its own invariants |
| `W0601` | cache unreadable |
| `W0602` | cache corrupt |
| `W0603` | cache from another build |
| `W0604` | obligation undecided at every tier |
| `W0605` | a shipped module the load reads changed since the cache was written |
| `W0607` | supplied configuration key the schema does not declare |
| `W0608` | drain deadline expired with requests in flight |
| `W0609` | spans still open when their task or the entry point ended |
| `W0610` | reference cycle, never freed |
| `W0611` | definition no `pub` item, `main`, test or law reaches; a leading `_` in its name keeps it quiet |
| `W0612` | run abandoned at its wall clock; nothing recorded |

## 18. What Ply does not have

* No loops, `break` or `return` (`?` is the only early exit, and a raise the
  only one past the caller, §6.8); no mutable variables; no exceptions
  outside the row; no typeclasses, implicits or method syntax; no
  modules-as-values or first-class effects; no `unsafe` or FFI: what a program
  reaches outside itself is an effect a handler answers, and the builtins are the
  only functions the runtime implements (§12).
* Specs cannot name mutable state. Cycles are not collected, and a task never
  moves between OS threads; only a `parallel` block's branches run on threads
  of the runtime's own.
* No file handles — `fs` reads a range and appends by path, with nothing open
  between calls; no backpressure; no migrations or live schema
  check; HTTP/1.1 only; no authentication framework.

Sharp edges: `x.f(y)` with a bare variable `x` is a perform; an operation no
`handle` names is found only when it reaches the host boundary at run time
(`E0424`), unless its effect is `nondet` in a deterministic test (`E0412`); a
record update needs the base's type to be known where it stands; two allocating tasks are always ordered; `bytes_at`, `bytes_u32_le`, `string_slice`,
`string_find`, `list_set`, `array_get` and `array_set` raise where `list_at` and
`array_at` answer `None`.

## 19. Examples

In `examples/`: `clock.ply` (a `nondet` effect, a handler, `test/nondet`);
`ledger.ply` and `report.ply` (modules, an `opaque` type, specs, laws); `pipeline.ply`, `bank.ply`
and `timeout.ply` (simulation, a race and its fix, a virtual clock); `echo.ply`
and `hello.ply` (sockets, an HTTP endpoint); `orders.ply` (`derive json`);
`relay.ply` (one forwarder generic over the label it writes under);
`shout.ply` (a command line declared with `std.cli`, read into the program's
own type);
`store.ply` (a handler as a capability grant); `agreement.ply` and
`twin_divergence_audit.ply` (`std.db`'s twin against recorded PostgreSQL
answers); `desk.ply` (a service over PostgreSQL or its in-memory twin, whose
store, TLS and accept loop are configuration, with tracing and shutdown).
