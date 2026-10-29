# The Ply Guide

Ply is a general-purpose, statically typed functional language with tracked effects. A function's
type says which resources it touches. The compiler and the test runner work one *definition* at a
time, identified by a hash of its content, so a test runs again only when something it depends on
changed. This guide is the reference for writing Ply and using the `ply` command. For what Ply is
for, read [`GOALS.md`](../GOALS.md).

## 1. Getting started

Build `ply` from a checkout with `scripts/bootstrap.sh --release` and put `target/release/ply` on
your path. Run the same script again after you edit the checkout.

A Ply file is a module:

```ply
// hello/main.ply
fn greeting() -> String = "hello from ply"

fn main() -> Unit / {Console} = print(greeting())
```

`ply run hello` calls `main`. It prints `hello from ply` and exits with status `0`. Standard
output holds only what the program writes. Errors and warnings go to standard error, and `ply run
--json` reports the value `main` returned.

Printing is an effect, not a builtin. `/ {Console}` in the signature says `main` uses the console.
`Console` is a set of effects for standard input, output and error (§6.4); `print` writes a
`String` to standard output and `read_line` reads a line from standard input. `print`,
`read_line` and `Console` come from `std.console` and need no import (§13). To print something
that is not a `String`, interpolate it: `print(f"n = {n}")`. `ply run` connects these effects to
the terminal. A test is not a process, so a test that prints is an error (`E0424`) unless it
handles the effect itself (§14).

The everyday commands are:

- `ply check`: parse, resolve names, check types and infer effects.
- `ply test`: run the tests.
- `ply run`: run the program.

Each takes a `.ply` file or a directory and works on the project that contains it. With no path,
it works on the package that contains the working directory. `ply check --types` prints the
inferred signature of every definition, with how many times each effect is performed (§6.2), and
explains why a definition that may not return might not (§5.10).

**Starting a package.** `ply new demo` creates `demo/ply.pkg` (the package's manifest, §3.3) and
`demo/main.ply`, which has a `main` that prints and one test. `cd demo && ply run .` prints `hello
from demo`, and `ply test` runs the test.

- The last part of the path names the package: `ply new store/orders` makes a package called
  `orders`. Use `--name` when the directory name is not a valid package name: `ply new lib-src
  --name core`.
- `--lib` creates a library instead: a `lib.ply` with one `pub` definition and no `main`.
- A package name is lowercase letters, digits and `_`, in parts joined by dots.
- `ply new` refuses a directory that already exists.

**Projects.** A command finds the project's root by walking up from the path it was given (or the
working directory) to the nearest directory that holds a `ply.pkg`. So `ply test` means the same
package from its root and from any directory inside it. A path inside the package narrows what the
command does without moving the root: `ply test store` runs the tests of the modules under
`store/`, and `ply run tools/gen.ply` runs that file's `main`.

When no directory above holds a `ply.pkg`:

- A directory you name is the root.
- A file you name is a program rooted at its own directory. It includes only that file and the
  modules it imports, so `ply run gen.ply` reads `shared.ply` for `import shared` and no other
  file.
- A command with no path loads nothing and says so (`E0502`), rather than treating whatever is in
  the working directory as a program.

**Modules.** Every `*.ply` file under the root is a module named by its path, with `/` replaced by
`.` and `.ply` removed: `store/orders/place.ply` is `store.orders.place`. Every directory name and
file name must be an identifier (`E0111`). The search skips directories whose names start with `.`,
directories named `target`, and directories with their own `ply.pkg`, which are separate packages
reached only as dependencies (§3.3).

`std` and `compiler` are built-in packages, available to every program. A module of your own named
under either prefix is an error (`E0133`).

**The cache.** `.ply-cache/` at the root holds what `ply` keeps between runs: checked modules, test
results, proofs, review baselines and fetched git dependencies. Add it to `.gitignore`. It is
always safe to delete, and `ply cache clear` empties it. A run that crashes or races another run
can make a later run redo work, but never gives a wrong answer. `vendor/` holds the dependencies
`ply vendor` copied, for builds without network access (§3.3).

A definition's hash covers what it depends on, but not comments or formatting (§8.2). Checking is
incremental: an edit to a function's body that keeps its signature and effects the same rechecks
only that function and any functions in a recursion with it.

Results are trusted only by a `ply` that would compute them the same way. A test result or proof
recorded by a `ply` with a different runtime, or whose relevant code changed, is recomputed
(`W0603`). A test result is also recorded under the compiled code the test ran, so a new `ply`
that compiles a test to the same code does not run it again.

`PLY_CACHE_UPSTREAM=DIR` names a second cache shared between checkouts and machines: any directory
they can all reach. Test results and proofs found there count here, and this run's results are
written there. `PLY_CACHE_UPSTREAM_READONLY=1` only reads it. Entries are keyed by content, so
nothing machine-specific is shared. `--no-cache` ignores both caches.

## 2. Lexical structure

### 2.1 Source and identifiers

Source is UTF-8. A file that is not valid UTF-8 is read up to the first invalid byte, which is an
error (`X0001`). Whitespace only separates tokens: there is no indentation rule. A comment runs from
`//` to the end of the line.

A *doc comment* documents a declaration for the people who call or use it.

- `///` lines document the item directly below them: a `fn`, `extern fn`, `type`, `effect`, effect
  operation, `effect set`, `law schema`, variant, or field of a declared record.
- `//!` lines at the top of a file, before the first import or item, document the module.
- `////` is a plain comment.

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

Put one space after the marker. A bare `///` line separates paragraphs, and the first sentence is
the summary. A doc says what the signature cannot: what the result means, when the function
raises, units, edge cases. It holds no examples: the tests and laws that name a definition are its
examples, and `ply callers` lists them (`ply doc` lists the laws, §16). A blank line between a doc
and its declaration is allowed, and `ply fmt` removes it.

A doc comment with nothing to document is an error (`E0003`): one inside a body; one above an
`import`, `test`, `law`, `derive`, `key`, `show`, `invariant` or `numeric`; one at the end of a
file; or a `//!` line below the top of its file. Like any comment, a doc comment does not change any
hash or cached result.

An identifier starts with an ASCII letter or `_` and continues with ASCII letters, digits or `_`.
`_` alone is the wildcard. A non-ASCII character outside a literal is an error (`X0001`).

Case matters in two places:

- In types, a lowercase name is a type variable and an uppercase name is a type.
- In patterns, a lowercase name binds a variable and an uppercase name is a constructor.

By convention, values are `snake_case`, types and constructors are `UpperCamelCase`, and effects
and resource labels are lowercase.

### 2.2 Keywords

These words are always reserved: `pub` `import` `fn` `type` `effect` `nondet` `test` `let` `if`
`else` `match` `handle` `with` `true` `false`. A keyword may still name a record field
(`{nondet: Bool}`, `d.nondet`), but not in the short form `{nondet}`, which would also bind a
variable.

The following words are keywords only where shown, and ordinary identifiers everywhere else:

| word | is a keyword |
| --- | --- |
| `as` | in an `import`, after the module path |
| `read`, `write` | at the start of an operation declaration, or after `.` in an atom |
| `raise` | at the start of an operation declaration (§6.8) |
| `set` | in `effect set X = {..}` |
| `new` | right after the `=` of a `type` declaration |
| `opaque` | before `type` (§3.2) |
| `law`, `schema`, `forall`, `cost`, `space` | in `law "..."` and `law schema <name>`; `forall` after a law's label or a schema's parameters; `cost` and `space` among a function's `requires` and `ensures`, and `cost` after a function-typed parameter |
| `bounded` | after an atom or row variable in a definition's row (§6.2) |
| `derive`, `for`, `reuse`, `transparent`, `const` | in `derive <deriver> for <Type>`, and before `fn` as `reuse fn`, `transparent fn`, `transparent reuse fn` and `const fn` |
| `for`, `in` | after a test's label: `test "..." for <name>: <Type> in <table>` (§8.1) |
| `key`, `show`, `invariant`, `numeric`, `by` | in `key for <Type> by <function>`, `show for ...`, `invariant for ...` and `numeric for <Type> by { <operation>: <function>, .. }` (§4.4) |
| `host`, `by` | in `host for <effect> by <function>` (§6.10) |
| `release` | in `release for <Type> by <function>` (§7.1) |
| `keep` | before a parameter's type, a function type's parameter, an operation's parameter, or a type parameter (§4.7, §7.1) |
| `once` | before the `(` of a function type (§7.1) |
| `where`, `derivable`, `identity` | after a signature's row, after a type's parameter list, or after a law's binders; `identity(a)` at the start of a law's or schema's `where` |
| `returns`, `fresh` | between a function's signature and its specifications |
| `requires`, `ensures` | between a function's signature and its body |
| `resume`, `return` | in a handler clause (§6.5, §6.6) |
| `with_hold` | before `[` |
| `simulate` | before `{` where an expression can start |
| `try` | before `{`, or before `[` and an operation's name, where an expression can start (§6.8) |

### 2.3 Literals

| form | type | notes |
| --- | --- | --- |
| `42`, `1_000_000`, `0xFF` | `Int` | 64-bit signed. `_` may separate digits. A hex literal is a 64-bit pattern, so `0xFFFF_FFFF_FFFF_FFFF` is `-1`. |
| `255u8`, `0x6A09_E667u32`, `-1i8` | fixed width | Suffixes: `u8` `u16` `u32` `u64` `u128` `i8` `i16` `i32` `i64` `i128`. A decimal literal must fit the type's range (`256u8` is `E0211`); a hex literal must fit its width (`0xFFu8` is 255). |
| `1.5`, `1e9`, `2.5e-3` | `Float` | IEEE-754 64-bit. |
| `1.50m`, `0m` | `Decimal` | Exact base 10, up to 28 digits after the point, 96-bit mantissa. Keeps the number of digits you wrote. |
| `"text"` | `String` | UTF-8. Cannot contain a line break. |
| `f"n = {n + 1}"` | `String` | Interpolated string; see below. |
| `\\text` | `String` | Line string: lines of verbatim text; see below. |
| `b"GET "` | `Bytes` | ASCII characters and `\xNN` escapes. |
| `b'{'`, `b'\n'`, `b'\x1f'` | `Int` | One byte, as `Bytes::at` returns it. |
| `uuid"6ba7b810-.."`, `html"<b>{x}</b>"` | the tag's result type | Tagged literal: text parsed by its tag when the program is checked; see below. |
| `'a'`, `'\n'`, `'\u{1F600}'` | `Char` | Exactly one character or one escape. |
| `#{"a": 1, k: v}` | `Map<k, v>` | Keys are expressions. A later entry replaces an earlier one with the same key. |
| `#[1, 2]` | `Set<a>` | `std.set`'s set: each element once, in order (§4.3). |
| `true`, `false` / `()` | `Bool` / `Unit` | |

`1`, `1.0`, `1m` and `1u32` have four different types, and Ply never converts between them
implicitly: `fn f() -> Int = 1.0` is an error (`E0201`). A numeric literal is never negative:
`-3` is unary minus applied to `3`, except in a pattern. So the smallest value of a signed width
cannot be written as a literal; use `I8::of_int(-128)`.

String and character escapes are `\n` `\t` `\r` `\0` `\\` `\"` `\'` and `\u{...}` (one to six
hex digits naming a Unicode scalar value; a surrogate or a value above `10FFFF` is `E0211`). Byte
strings also allow `\xNN`, but not `\u{...}` or characters above `U+007F`.

**Byte literals.** `b'x'` is one byte, written as in a byte string, and its value is that byte as
an `Int`. `b'{'` and `123` are the same literal with the same hash, so `src.at(i) == b'{'` reads
naturally and a byte scanner can be a `match` (§5.2). An empty byte literal, one with more than one
byte, or one that never closes is `E0001` or `E0002`, as for a character.

**Interpolated strings.** `f"a {x} b"` means `"a " ++ display(x) ++ " b"`, using `std.show`'s
`display`: a `String` or `Char` is inserted as is, and any other value as `show` writes it. Both
spellings are the same definition with the same hash.

- A hole `{...}` may hold any expression, including strings and braces.
- `{{` and `}}` are literal braces; a lone `}` is an error (`E0001`).
- A hole's type must be showable (`derivable(show, ·)`, §11), so a `Secret`, a function, a `Task`
  or a `Chan` in a hole is an error (`E0206`).
- A module that interpolates imports `std.show` automatically, so `std.show` and the modules it
  imports cannot use interpolation themselves.

**Map and set literals.** A map literal is a call of `Map::of_entries([{key: k, value: v}, ..])`,
and a set literal is the same with `()` as each value. Either spelling is the same definition with
the same hash, and `ply fmt` keeps the one you wrote.

**Tagged literals.** A tagged literal is a name immediately followed by a string: `uuid"..."` is
one, while `uuid "..."` is a name followed by a string. It calls the tag module's parser on the
text when the program is loaded, before anything runs, so a malformed literal is a compile-time
error:

```ply
import std.uuid
import std.uuid (Uuid)

fn dns() -> Uuid = uuid"6ba7b810-9dad-11d1-80b4-00c04fd430c8"
```

`tag"text"` means `tag::literal("text")`, where `tag` is a module imported under that name (§3.2)
and the text is read like an interpolated string's (escapes apply, `{{` and `}}` are braces).

- `literal` must be a `pub fn` taking one `String` and returning `Result<T, {message: String,
  offset: Int}>`. It may take no type, row or label parameters, and may perform nothing except
  raise (§6.8). Anything else is an error (`E0152`), as is a parser that is part of a recursion
  with the definition that contains the literal.
- An unknown tag is `E0106`. A tag module with no `literal`, or a private one, is `E0101` or
  `E0107`.
- If the parser returns `Ok(v)`, the literal has type `T`, its value is `v`, and it performs
  nothing.
- If the parser returns `Err`, the literal is an error (`E0153`) reported at character `offset` of
  the text as written, with `message`. An escape or a doubled brace counts as one character, and
  the offset just past the end points at the closing quote.
- A parser that raises, or does not finish within 60 seconds of wall clock, is an error (`E0154`).

The literal's hash covers its parser and its text. Editing anything the parser depends on rechecks
every definition that uses the tag. `f` and `b` are not tags, since `f"` and `b"` start the strings
above. A tagged literal is an expression, never a pattern, label or parameter default (`E0121`). It
runs as `let Ok(v) = tag::literal("text")`, so a module that defines its own `Ok` cannot use tagged
literals (`E0118`). `std.uuid`, `std.base64` and `std.bigint` are tags (`ply doc
std.uuid.literal`).

A tagged literal can also have holes. The tag receives the values separately from the text, so it
can escape or quote each one according to where it appears, and nothing in a hole is ever read as
text. This makes injection impossible:

```ply
import std.html
import std.html (Html, html, text)
import std.sh

fn row(name: String, kind: String) -> Html / {html.unsafe_url, abort.raise} =
  html"<li class={text(kind)}>{text(name)}</li>"

fn search(pattern: String, file: String) -> List<String> = sh"-n --color=never {pattern} {file}"
```

With holes, the literal calls two functions instead of `literal`:

- `tag::compile(["t0", "t1", ..])` receives the text pieces around the holes (one more piece than
  there are holes) and returns `Result<C, {message: String, offset: Int, part: Int}>`. It runs when
  the program is loaded, on the text alone. An `Err` is `E0153` at character `offset` of piece
  `part`.
- `tag::fill(c, [h0, h1, ..])` receives the compiled `C` and a `List<H>` of the hole values when
  the literal runs. Each hole is checked against `H` (`E0201`).

`compile` and `fill` follow the same rules as `literal` (`E0152`). The literal's type is what `fill`
returns, and it performs what its holes perform plus what `fill` raises. A tag may declare
`literal`, `compile` and `fill`, or only some of them; using a kind of literal the tag does not
support is `E0101`. `std.html` and `std.sh` are tags with holes.

**Line strings.** A line string is one or more lines that each start with `\\` (after optional
spaces). Everything after `\\` up to the end of the line is text, exactly as written: there are no
escapes. The lines are joined with `\n`, and the value ends after the last line, so a value that
ends with a newline ends with a line holding only `\\`:

```ply
fn program() -> String =
  \\fn main() -> Int = 42
  \\
```

A line ending in a space or tab is an error (`E0001`), since the whitespace would be invisible. A
`\r` before a newline is not part of the text. A line string is an expression, never a pattern or
a label, and nothing can follow it on its last line.

### 2.4 Operators

From loosest to tightest binding. All binary operators are left-associative.

| precedence | operators | operand types |
| --- | --- | --- |
| 1 | `\|\|` | `Bool` |
| 2 | `&&` | `Bool` |
| 3 | `==` `!=` `<` `<=` `>` `>=` | see below |
| 4 | `\|` | integer |
| 5 | `^` | integer |
| 6 | `&` | integer |
| 7 | `<<` `>>` `>>>` | integer; the shift count is `Int` |
| 8 | `++` | `String` or `Bytes` |
| 9 | `+` `-` | numeric |
| 9 | `+%` `-%` `+\|` `-\|` | integer |
| 10 | `*` `/` `%` | numeric |
| 10 | `*%` `*\|` | integer |
| — | prefix `-` `!` `~` | numeric / `Bool` / integer |
| — | postfix `f(x)` `r.field` `e.op[r](x)` `e?` | |

- `==` and `!=` compare structurally at every type except functions. A type with a `key` is
  compared through its key (§4.4). `Float` equality follows IEEE, so `NaN != NaN`.
- `<`, `<=`, `>` and `>=` work on numeric types, on `Char` (by Unicode scalar value), and on a
  type that declares both `numeric` and a `key` (§4.4). Use `compare` to order anything else.
  Arithmetic on a `Char` is an error (`E0201`); convert with `Int::of_char`.
- Both operands have the same type. There is no widening: `U8 + U16` is `E0201`.
- On a sum type with `numeric` (§4.4), `+`, `-`, `*` and prefix `-` call the functions it names.
  They perform nothing and cannot raise. A literal next to such a value is still an `Int`, so
  `a + 1` is `E0201`.
- Arithmetic is checked:
  - Division or remainder by zero raises (§6.8), so `/` and `%` add `abort.raise` to the effects
    unless the operands are `Float`, the divisor is a nonzero literal, or the surrounding
    conditions show an `Int` divisor is not zero.
  - Overflow, a `Decimal` result that cannot be represented exactly, and a shift count that is
    negative or at least the type's width end the program (`E0502`). These are machine limits
    like the call depth limit, and do not appear in effect rows.
  - `<<` discards bits shifted out. The `checked_*` builtins return `None` instead of failing
    (§12).
- `+%`, `-%` and `*%` wrap at the type's width, and `+|`, `-|` and `*|` saturate at its limits:
  `255u8 +% 1u8` is `0u8`, `250u8 +| 10u8` is `255u8` and `3u8 -| 10u8` is `0u8`. For `Int`, the
  width is 64 bits. They never fail.
  - Each is a builtin call: `a +% b` is `wrap_add(a, b)`, with the same hash. The others are
    `wrap_sub`, `wrap_mul`, `saturating_add`, `saturating_sub` and `saturating_mul`.
  - The operands must be one built-in integer type known at the call site, or, where the builtin is
    named as a value, the type it is used at. A type parameter, even
    one constrained by `integer(a)`, is `E0210`; any other type is `E0201`; a type with its own
    `numeric` is `E0223`.
  - Because `%` and `|` directly after `+`, `-` or `*` form one of these operators, write a space
    before a lambda used as a right operand: `a + |x| x`.
- `/` on `Decimal` is an error (`E0209`); use `Decimal::div`. `%` is allowed.
- `&&` and `||` short-circuit. `&`, `|`, `^` and `~` work only on integers, at the type's own width
  (`~0u8` is `255u8`). `>>` is an arithmetic shift and `>>>` a logical one. Shifts are written as
  adjacent `<` or `>` tokens, so `Map<Int, List<Int>>` still parses.
- `++` joins two `String`s or two `Bytes`. Mixing them is `E0201`: convert with
  `Bytes::of_string` or `String::of_bytes`. Operands of unknown type are `E0210`.
- `::` qualifies a name through a module (`items::price_of`) and cannot be chained.
- `?` binds tightest: `f(x)?.field` is `(f(x)?).field`.

## 3. Modules and items

A file is its imports followed by its items, in any order: `fn`, `type`, `effect`, `nondet
effect`, `effect set`, `test`, `law`, `law schema`, `derive`, `key`, `show`, `invariant` and
`numeric`. Definitions can refer to each other and recurse across the whole program.

### 3.1 Functions

```ply
type Account = { name: String, balance: Int }

fn credit(a: Account, amount: Int, note: Option<String> = None) -> Account = {
  let moved = a.balance + amount;
  {name: a.name, balance: moved}
}
```

A body is `= expression`. There is no `return`. A body with bindings is written as a block: `fn
f() -> Int = { let x = 1; x }`. Without braces, `fn f() -> Int = let x = 1; x` does not parse,
because `let` is a statement, not an expression.

Every parameter type and the return type of a top-level `fn` must be written (`E0126`, which shows
the inferred type). A parameter the body never uses may be written `_`, in a `fn`, a lambda or a
handler clause; it binds nothing and may appear more than once.

**Defaults.** A parameter default lets a call leave the argument out. It must be a constant value:
a literal, a constructor applied to literals, a record or a list (`E0121` otherwise). It may not
use another parameter (`E0121`). On a `pub fn`, it may not use anything its module does not export
(`E0122`). It may not use a constructor of a type with an `invariant` (`E0480`, §4.4). Only `fn`
parameters take defaults (`E0120` elsewhere).

**Functions of a type.** A function can be declared in a type's namespace: `fn Stack::size(s:
Stack) -> Int = ..` is the function `size` of `Stack`. It is named `Stack::size` wherever `Stack`
is in scope, and called on a value as `s.size()` (§5.4). It need not take a value of the type:
`Stack::empty()` makes one.

- Only the module that declares the type may declare its functions (`E0208`). For a built-in type,
  that is the type's home module in the standard library (§13).
- The type must be a sum or a `new` record, since an alias is just the type it names (`E0217`).
- A `new` record cannot have a function with the same name as one of its fields, since `r.name(..)`
  reads the field first (`E0105`).
- Such a function is `pub` or private like any other. `key`, `show` and `numeric` (§4.4) may name
  one: `key for Stack by Stack::size`.

`reuse fn` promises that every `List::push` in the body updates its list in place (§5.6).
`transparent fn` lets other packages see the body, not just the signature (§10).

**The entry point** is `main`, of any type and effects. No `main` is `E0101`, and several are
`E0112`. Name a file to choose one: `ply run tools/gen.ply` runs that file's `main`.

### 3.2 Imports, visibility and opaque types

```ply
import store.orders                 // binds the module as `orders`
import store.orders as ord          // binds it as `ord`
import store.orders (place, cancel) // binds those names, and no module name
```

Reach into an imported module with `::`: `orders::place(...)`. `as` and a name list cannot be
combined; write two imports. Imports come before every item.

The first part of a module path may be a package. Each dependency declared in `ply.pkg` (§3.3)
gives access to its own prefix, so `import cli.surface` reaches the `surface` module of the package
`cli`.

- An import whose first part is not a module of this package or a declared dependency's prefix is
  `E0106`. Using a dependency's prefix that the manifest does not declare is `E0132`.
- An import without a package prefix always means the current package's own module. Inside a
  dependency, `import fmt` means *that* package's `fmt`. A package can never reach into the
  package that imports it.

Items are private unless marked `pub` (`E0107`). `pub` applies to `fn`, `type`, `effect` and `law
schema`. Values (functions, constructors and schema definitions, §10), types, effects and module
names are separate namespaces, so `fn size`, `type Size` and `effect size` can coexist.

**Opaque types.** A `pub type` exports a sum's constructors with it, and any module can build a
`new` record (§4.2) by writing a record literal where one is expected. Writing `opaque` before
`type` keeps construction inside the declaring module. Every value of the type then comes from that
module's functions, so any property they guarantee holds for every value:

```ply
pub opaque type Token = | Token(String)
pub opaque type Date = new { year: Int, month: Int, day: Int }

pub fn mint(s: String) -> Option<Token> = if s == "" { None } else { Some(Token(s)) }
pub fn text_of(t: Token) -> String = match t { Token(s) -> s }
pub fn of_ymd(year: Int, month: Int, day: Int) -> Option<Date> = ..
```

Inside the declaring module nothing changes. Other modules can name the type, hold and pass its
values, and call the module's functions, but cannot build or take apart a value:

- Calling, passing, importing or matching on a constructor of an opaque sum is `E0107`.
- A record literal where an opaque record is expected, or an update of one, is `E0220`.

Other modules can still read an opaque record's fields, with `d.year` or a record pattern. To hide
the contents too, declare a sum with one constructor.

`opaque` is not secrecy. `==`, `compare`, `digest`, `show` and `reflect` read the whole value, as
they do any value (through a `key` or `show` the module declares, §4.4). A `Secret` is the type
nothing can read (§4.6). A codec the module derives (§11) is public, and its decoder builds a value
from any document of the right shape, so a type with rules beyond its shape should write its codec
by hand.

A law in another module may still quantify over an opaque type. Its proof covers every value the
declaration allows, which includes every value the module builds; such a law is never proved by
running it on sample values (§10).

A default on a `pub fn` is copied into each caller, so one that uses an opaque constructor or
builds an opaque record is `E0122`. A type does not have to be `pub` to be opaque, since its values
can still leave the module through its functions. An alias has no values of its own, so `opaque` on
an alias is `E0221`. Unlike `pub`, `opaque` is part of a type's hash: making a type opaque or not
rechecks every definition that can hold one.

### 3.3 Packages and the manifest

A project root may hold a `ply.pkg` file: the package's manifest. It holds exactly one definition,
whose body is a literal of `std.pkg`'s `Manifest` (`ply doc std.pkg.Manifest`):

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

The body is data, not code: a literal, a constructor applied to literals, a record or a list, as
for parameter defaults (§3.1). Code there is `E0130`, a manifest of another shape is `E0129`, and a
field that does not decode or is invalid is `E0131`. A project without `ply.pkg` is the anonymous
package. `ply new` writes this file with the current toolchain as `runtime`.

**Dependencies.** Each dependency gives access to its package's module prefix, and only declared
dependencies can be imported:

```ply
import store.orders        // a module of this package
import cli.surface         // a module of the declared dependency `cli`
```

There are three kinds of dependency:

- `Path("../cli")` names the directory holding the dependency's `ply.pkg`, which must exist
  (`E0135`).
- `Git(url, rev)` is fetched into `.ply-cache/git/`, one directory per URL and revision, using the
  `git` on your `PATH`. `rev` may be a commit, a tag or a branch. A branch means whatever it pointed
  to when it was fetched; the fetched copy is reused without asking the remote again, so clear the
  cache to pick up a moved branch (`ply.lock` notices the change). A fetch that fails, or no `git`
  on the path, is `E0135`.
- `Registry` names a package in the registry `PLY_REGISTRY` points to (§15.1), at version `min` or
  later. Its `name` must be a valid package name (`E0131`).

`ply resolve` is the only command that contacts the registry. It picks a version, downloads the
archive (`.ply-cache/registry/<name>@<version>.plyz`), verifies it, and unpacks it into
`.ply-cache/registry/<name>`, where every other command reads it without the network. A registry
dependency that is missing from the cache, or older than the closure needs, is `E0135` or `E0136`;
`ply resolve` fixes either. Before reading anything from a downloaded archive, `ply` checks it: an
archive whose bytes do not match the digest in the index or in `ply.lock`, or that is not the
package and version requested, is `E0142`. A published version never changes, so different bytes
for a pinned version are always refused. An unset, malformed or unreachable registry is `E0141`. A
name the registry does not have, or a minimum no non-yanked version meets, is `E0143`.

Inside a dependency, every module is named `<prefix>.<name>`, and its own bare imports refer to its
own modules. Errors:

- Importing a package the manifest does not declare: `E0132`.
- Two packages using the same prefix, or a module of the root package using a dependency's prefix:
  `E0133`.
- Packages depending on each other in a cycle: `E0134`.

**Version selection.** The manifest is checked on every load. `ply` uses minimal version selection:

- When two manifests ask for different minimum versions of one package, the highest minimum wins.
  A dependency below what its importer asks for is `E0136`.
- One version of each package serves the whole program. The same package name found at two
  different places is `E0137`, naming both places and who asked for each. A package that two
  others both depend on (a diamond) is fine and resolves to one version.
- Over a registry, each minimum selects the lowest published, non-yanked version at or above it
  (or the version `ply.lock` already pins, if that is high enough). Each selected version adds its
  own dependencies' minimums, and each name settles on the highest version reached. The order the
  manifests are read in does not matter.
- A new resolution skips yanked versions, but a lock that already pins one keeps it.

**The lock file.** `ply build` records what it resolved in `ply.lock`, next to `ply.pkg`: each
dependency's name and version, sorted by name, with a BLAKE3 digest of its modules and embedded
files (§3.4). A registry dependency also records the digest of its archive.

- A path dependency inside the same git checkout is recorded without a digest, like Cargo does:
  the commit that holds your project also holds its sources, so editing it does not change the
  lock. "Inside" means at or below the nearest directory with a `.git`, with no symlink or nested
  `.git` on the way.
- Git, registry and outside-the-checkout path dependencies keep their digest. A package is pinned
  by its content, not by where it was found, so moving a checkout keeps its pins.
- Before writing any output, a build verifies the lock. A dependency whose digest no longer matches
  its sources is `E0138`. A lock this `ply` cannot read, or of a format other than 2, is `E0139`.
  The build writes a new lock when what it resolved differs from the file.
- To accept a changed dependency, run `ply resolve`, which pins what is on disk now. Deleting the
  lock, or one package's entry in it, does the same.

**Vendoring.** `ply vendor` copies every dependency into `vendor/`, one directory per package
named by its prefix: its `ply.pkg`, its modules and its data files, but not git history. It also
writes `vendor/index`, one line per package. When the index exists, `ply` reads dependencies from
`vendor/` and needs no cache, network or `git`. Each vendored directory has its own `ply.pkg`, so
it is a separate package, not part of yours. `ply resolve` in a vendored project also updates the
vendored copy. `ply why NAME` shows how a package got into the closure: the chain of packages from
yours to it, then its version and digest.

**What a command covers.** A command works on the root package, the one you pointed it at. A
dependency's `main` is never an entry point. `ply test` runs only the root package's tests, and
`ply prove` proves only the root package's claims, relying on dependencies' `requires` and
`ensures` (§10). `--workspace` (on `ply check`, `ply test` and `ply prove`) also runs the command
on every package reachable through path dependencies, each as its own root with its own cache,
dependencies first. Fetched and vendored dependencies are never included.

**Workspaces.** A repository with several packages lists them in a `ply.work` at its root. It
holds one definition, `fn workspace() -> Workspace = ...`, a literal of `std.pkg`'s `Workspace`
(`ply doc std.pkg.Workspace`), checked like a manifest (`E0129`–`E0131`). It lists:

- `packages`: the directories of the packages the repository maintains;
- `suites`: packages of tests or examples, each saying whether `ply prove` runs on it;
- `fixtures`: directories of Ply sources that belong to no package;
- `toolchain`: only in the repository that builds `ply`; where the standard library, the compiler,
  the CLI package and its entry point, their test suites and the runtime's cargo package are.

Every path is relative to `ply.work`, below it, and listed once. `ply work [PATH]` prints the
workspace at or above a directory, one role and path per line (`--json` for one document); with no
`ply.work` it is `E0502`. A workspace changes nothing about how a command treats a package, with one
exception: inside the repository that builds `ply`, a command reads the standard library and the
compiler from the checkout instead of from the running `ply`. That way the CLI, the compiler and the
library are always checked against each other. `ply check packages/std` checks every module of
the standard library, and `ply test packages/compiler` runs every test of the compiler.

### 3.4 Embedding files

```ply
fn schema() -> Bytes = embed("schema.sql")
fn fixtures() -> List<{ name: String, bytes: Bytes }> = embed_dir("fixtures")
```

`embed("path")` is the contents of a file. `embed_dir("path")` is every file under a directory,
named by its path inside it (`a/b.txt`) and sorted by that path; names starting with `.` are
skipped. The path must be a string literal (`E0147`), relative to the module's own file.

The file's contents are part of the definition's hash. A test that reads an embedded file runs
again exactly when the file changes, and stays cached while it does not. The file's location is
not part of the hash.

A missing path, a directory passed to `embed`, a file passed to `embed_dir`, an unreadable file, or
a file that changed between checking and building is an error (`E0146`). A module that defines or
imports its own `embed` or `embed_dir` uses that one instead.

### 3.5 Constants

```ply
type Status = { code: Int, reason: String }

const fn statuses() -> Array<Status> = parsed(embed("status_codes.csv"))
```

A `const fn` is a value the program computes once. Its body runs the first time a run reads it.
Every later read in the same process returns the same value. This is how to turn a data file into a
table that is parsed once, not on every read.

- It takes no parameters, has no type, label or row parameters, and writes no row.
- Its body may raise, and may do nothing else. It must return, so no call that may not return
  (§5.10).
- Its value must be data: no function, `Task`, `Chan` or `Secret`, at any depth.
- Anything else is `E0156`.

`pub const fn` exports it. `const` cannot be combined with `transparent` or `reuse`.

Its row is empty, whatever its body could raise. A raise that leaves the body ends the run, and no
handler around the read can catch it. Nothing checks a `const fn` before it runs, so test what it
returns.

A plain function of no parameters that performs nothing is also computed once per process.

A `const fn` of type `std.sql.Schema` is a database schema: the SQL statements of its module, and
of every module that imports it, are checked against it before anything runs (§16). A module whose
statements reach two schemas is `E0471`.

## 4. Types

Types are inferred with Hindley–Milner unification and row polymorphism. Signatures you write are
checked, not inferred (§4.7).

### 4.1 Scalars and numbers

| type | values |
| --- | --- |
| `Int` | 64-bit signed; the type for counting and indexing |
| `U8` `U16` `U32` `U64` `U128` `I8` `I16` `I32` `I64` `I128` | fixed widths, for data defined in a width and for exact values beyond `Int` |
| `Float` | IEEE-754 64-bit |
| `Decimal` | exact base 10; `+ - * %` are exact, or end the program on overflow |
| `Bool`, `Unit` | `true`/`false`, `()` |
| `Char` | one Unicode scalar value: `U+0000` to `U+10FFFF`, excluding surrogates |
| `String` | UTF-8, indexed and sliced by character |
| `Bytes` | immutable bytes, indexed by byte |

There are no implicit numeric conversions. An operator's operand type is worked out from the whole
definition, so `fn h(a: U32) -> U32 = a + 1u32` checks and `a + 1` does not. An operand nothing
determines, as in `let g = |a, b| a + b;`, is an error (`E0210`); there is no default type.

Conversions are explicit builtins (§12):

- `U32::of_int` and its siblings raise (§6.8) when the value does not fit. Mask to truncate:
  `U8::of_int(n & 0xFF)`.
- Convert between two fixed widths through `Int`: `to_int` reads any integer type as an `Int`, and
  `numeric_of_int` converts an `Int` to any numeric type.
- A 128-bit value beyond `Int` converts through its decimal text (`U128::of_string`).
- `String::of_bytes` raises on invalid UTF-8.

**Decimal.** A `Decimal` is a 96-bit mantissa and a *scale*, the number of digits after the
point, from 0 to 28. `1.5m == 1.50m`, but each prints as written, just as a `Map` keeps a key the
way it was first given (§4.3). `Decimal::div` and `Decimal::round` take the scale of their result,
and return exactly that many digits after the point, rounded once by the `Rounding` mode given.
They raise when 96 bits cannot hold the result at that scale. `Decimal::scale` returns a value's
scale, and rounding to no more digits than that only drops digits, so it cannot raise.

**Float.** Every `Float` operation gives the same bits on every machine, target and build profile.
Arithmetic is IEEE-754 64-bit with each operation rounded once, as written: operations are never
fused or reordered, and subnormal numbers are kept. `Float::sqrt`, `exp`, `ln`, `log2`, `log10`,
`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `pow` and `hypot` are correctly rounded:
each returns the `Float` nearest the exact value, ties to even. A program sees only one `NaN`:
whatever NaN the machine produces, `bits_of_float`, encodings and ordering all read it as
`0x7FF8_0000_0000_0000`.

### 4.2 Records and tuples

Records are structural: two record types with the same fields are the same type. `type` declares
an alias, not a new type, unless its body starts with `new` (below). Field order does not matter. A
record, a record type or a record pattern names each field only once (`E0004`).

```ply
fn f(a: Account) -> Int = a.balance
fn g(r: {name: String, balance: Int}) -> Int = f(r)     // same type
fn point(x: Int, y: Int) -> {x: Int, y: Int} = {x, y}   // `x` is short for `x: x`
fn divmod(a: Int, b: Int) -> (Int, Int) = (a / b, a % b)
```

A tuple is a record with numbered fields: `(A, B)` is `{_0: A, _1: B}` in types, values and
patterns, and its fields are read as `t._0`. `(A)` is just grouping; `()` is `Unit`.

An alias can take parameters: `type Index<a> = Map<a, Int>`. A signature that uses an alias is
held to the same requirements as the type it expands to (§4.3): `Index<k>` requires `k` to be
ordered, and `Index<Float>` is `E0206`. An alias can also take label and row parameters (§4.5):
`type Step<a | e> = () -> Option<a> / e` is that function type with whatever row is given for `e`.

**New records.** `type T = new { .. }` declares a distinct record type, with at least one field:

```ply
pub type Date = new { year: Int, month: Int, day: Int }

fn epoch() -> Date = { year: 1970, month: 1, day: 1 }
fn next(d: Date) -> Date = { ..d, year: d.year + 1 }
fn held() -> Date = {
  let r = { year: 1970, month: 1, day: 1 };   // a `{day: Int, month: Int, year: Int}`
  r                                           // E0201: not a `Date`
}
```

A `new` record is read (`d.year`), updated and matched like any record, but it is a different type
from a plain record with the same fields, and from another `new` record with the same fields
(`E0201`). A record literal becomes a `new` record when the place it appears already expects that
type:

- the body of a `fn`, or of a lambda whose return type is written or expected;
- a `let` with a written type; a parameter default;
- a function or operation argument, at the parameter's type. Type parameters are filled from the
  earlier arguments and from the type the call is expected to have, so `assert_eq(d, {..})`,
  `days.push({..})` and `Some({..})` returning `Option<Date>` all work;
- a field of a literal that is itself expected, and a field written by an update;
- a list element, an `if` branch or a `match` arm, when the whole or an earlier element, branch or
  arm settles the type; the right operand of an operator, at the left operand's type;
- the last expression of a block, a `let ... else` block, and the body of a `handle` (or its
  `return` clause), `with_hold` or `simulate`, when the whole is expected; the body of a `try`,
  when a `Result` of it is expected; a handler clause, at what it returns.

Such a literal must name every field (`E0201`) and no others (`E0101`). A record bound without a
type annotation is a plain record from then on.

The declaration is closed and checked where it is written:

- A field can use only the declaration's own type, label and row parameters (§4.5).
- A field cannot hold a `Hold`, `Task` or `Chan` (`E0446`, §4.6).
- A `new` record cannot contain itself except through a sum type (`E0214`).

**Types that carry an order.** If a record or sum holds a `Map` keyed by one of its type
parameters, anywhere inside, then that parameter must be ordered wherever the type is used, just as
a `Map`'s key must be. `std.set`'s `Set<a>` is an example. With `type Opt<a> = | Plain(a) |
Keyed(Map<a, Int>)`, both `Opt<Float>` and `Plain(1.5)` are `E0206`, because `Float` is not
ordered. A signature that uses such a type at one of its own type parameters gets `where
derivable(ord, a)` automatically: callers must meet it and the body may assume it.

At run time a `new` record is just its fields. `==`, `compare`, `digest`, `show`, `reflect` and
derived codecs (§11) treat it like a plain record, so two `new` records with the same fields print,
encode and hash alike; only the type checker tells them apart. A definition's hash does tell them
apart: a `new` record is hashed with its name, as a sum is, and with whether it is `opaque` (§3.2).

### 4.3 Lists, arrays, columns, maps and sets

- `List<a>` is an immutable sequence, written `[a, b, c]` (§5.6).
- `Array<a>` is a fixed number of elements stored next to each other, so reading or replacing one
  by index is a single load or store. It compares, orders and derives element by element like a
  list. It has no literal: build one with `Array::new` or `Array::of_list` (§12).
- `Columns<a>` is an array of records stored by column: one array per field. `xs.get(i).f` reads
  field `f` of record `i` without touching the other fields, so a pass over one field reads only
  that field's array. It compares and orders like the array of its records. Build one with
  `Columns::new` or `Columns::of_list`. The element type must be a record with at least one field
  (`E0224`).
- `Map<k, v>` is an immutable sorted map, written `#{k: v}` or built with `Map::new`,
  `Map::insert` or `Map::of_entries`.
- `#[a, b]` is a set: `std.set`'s `Set<a>`, each element once, in `compare` order (`ply doc
  std.set`).

**Maps.** A map iterates in `compare` order. A key that `compare` finds equal to a key already in
the map counts as that key: inserting it keeps the existing key and replaces the value. So
`#{1.50m: "a"}.insert(1.5m, "b")` is `#{1.50m: "b"}`. The same applies to two values of a type
whose `key` (§4.4) is equal.

A map's key type must be ordered (`derivable(ord, k)`): `Float`, `Secret`, functions, `Task` and
`Chan` are refused (`E0206`). A signature that mentions a `Map`, or a type that carries an order
(§4.2), in a parameter or result automatically requires each type parameter the key ordering
depends on to be ordered. So `fn size<k>(m: Map<k, Int>) -> Int` can only be called with an
ordered `k`, and its body may `compare` two `k`s.

Ordering a key reads exactly what `derivable(ord, ..)` reads (§11): a record's fields, a sum's
variants, or a type's `key` (§4.4). So `Map<Id<a>, v>` needs nothing of `a` if `Id<a>` holds no
`a`; a body that compares two `a`s there must write `where derivable(ord, a)` itself. Writing a
`where derivable(ord, k)` the signature already implies is allowed. Calling with an unordered type
is one `E0206` for the `Map` that needs it.

**Sets.** `show` writes a set back as its literal. A set is not a `Map`. A set literal calls
`std.set`'s `Set::of_list`, and a module that writes one imports `std.set` automatically, so
`std.set` itself cannot use set literals.

### 4.4 Sum types

```ply
type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point

type Level = Debug | Info | Warn | Error
```

The leading `|` is optional. Constructors are values: `Circle(3)` is a call and `Point` is a value.
An `opaque` sum's constructors belong to its module (§3.2). `type Id = Int` (one name, no payload,
no `|`) is an alias, not a sum.

A sum is nominal: two identical sums declared in two modules are different types. A sum can take
parameters, `type Tree<a> = | Leaf | Node(Tree<a>, a, Tree<a>)`, and a use with different
arguments is a different type (§4.5).

A sum may contain itself only at its own parameters or at fixed types. `type Nest<a> = | Leaf(a) |
Deeper(Nest<List<a>>)` contains itself at a type built from `a`, so each level would be a new type
and compiling it would never end; it is refused (`E0477`). Through a function type it is allowed:
`| Later((Later<List<a>>) -> Int)` holds no such value.

**Keys and custom display.** By default a value is compared, ordered, hashed and shown by its
constructor and fields. That is wrong for a type that can represent one thing in two ways. The
module that declares a sum can say how its values should be read instead:

```ply
pub type Deque<a> = | Deque(List<a>, List<a>)   // a front, and a reversed back

key for Deque by to_list       // used by `==`, `!=`, `compare`, `min`, `max`, `digest`, `Map` keys
show for Date by written       // used by `show`, `display`, interpolation

pub fn to_list<a>(d: Deque<a>) -> List<a> = ..
fn written(d: Date) -> String = ..
```

`key for T by f` names a function of the same module, `f: (T<a, ..>) -> K`.

- Two values of `T` are equal exactly when `f` returns equal keys, they are ordered as their keys
  are, and a value's digest is its key's. So two deques with the same elements split differently
  are `==`, have the same `digest` and are one `Map` key.
- The key is used wherever a `T` appears: in a list, record, tuple, another sum, a map's key or
  value, the key of another type, or behind a type parameter. `assert_eq` uses it too.
- `derivable(eq, T)`, `derivable(ord, T)` and `derivable(hash, T)` hold exactly when they hold for
  `K`, whatever `T`'s fields are. `K` must be ordered and hashable (`E0206`).
- A `match` still sees the value as it was built, as `reflect` does, and a test over cases tells
  its cases apart that way (§8.1).
- `f` runs on every comparison, so keep it cheap.

`show for T by g` names `g: (T<a, ..>) -> String`. `show`, `display` and interpolation use `g`
wherever a `T` is shown, and `derivable(show, T)` always holds. It does not affect comparison.
`std.value.reflect` and diagnostics still show the value as built.

**Invariants.** `invariant for T by v` names `v: (T<a, ..>) -> Bool`. It declares that every value
of `T` satisfies `v`. `T` must be `opaque` or not `pub` (`E0480` otherwise), so only its module
can build values. Each place that module applies a constructor of `T` becomes a claim that `ply
prove` must prove (§10), and code elsewhere may assume every `T` satisfies `v`:

```ply
pub opaque type Balance = | Balance(Int)
invariant for Balance by solvent
fn solvent(b: Balance) -> Bool = match b { Balance(n) -> n >= 0 }

pub fn amount(b: Balance) -> Int = match b { Balance(n) -> n }
law "a balance is never negative" forall (b: Balance) { amount(b) >= 0 }
```

The program never calls `v`; it changes nothing at run time. A parameter default that uses a
constructor of `T` is also `E0480`, since each call that leaves out the argument would build that
value itself.

**Rules for `key`, `show` and `invariant`:**

- Only the module that declares `T` may write them (`E0208`), at most once each (`E0105`).
- `T` must be a sum. An alias is the type it names, and a `new` record has no constructor at run
  time to find them by (`E0217`).
- The function must be declared in the same module (`E0101`). It takes one `T` at the type's own
  type and row parameters and nothing narrower (`f: (Held<a | e>) -> K` for `type Held<a | e>`),
  returns a type built only from those parameters, and needs no constraint `T` does not carry.
- Its row must be empty: no raise and no `diverges`. `==` and `show` never perform effects, never
  raise and always return, and a key that raised inside a `Map` insert would have nowhere to go
  (`E0218`).
- It may not take a resource label, and a type that takes a label cannot declare these (`E0218`).
- A key that is or holds a value of its own type would need its own key to compare, so it is
  refused (`E0219`), however indirectly.
- A type's hash covers these functions, so changing one rechecks and reruns everything that can
  hold a `T`.

**Custom arithmetic.** A number type declares its arithmetic the same way:

```ply
pub type BigInt = | BigInt(Bool, List<Int>)

numeric for BigInt by { add: add, sub: sub, mul: mul, neg: neg, of_int: of_int }
```

`a + b`, `a - b`, `a * b` and `-a` on a `BigInt` then call `add`, `sub`, `mul` and `neg`, and
`numeric_of_int(n)` calls `of_int(n)`.

- The record names exactly these five operations, each once (`E0215`), and each function is
  declared in the module (`E0101`).
- `add`, `sub` and `mul` are `(T, T) -> T`, `neg` is `(T) -> T` and `of_int` is `(Int) -> T`. None
  takes type parameters, and each has an empty row, so `a + b` adds no effects (`E0218`).
- The type must be a sum the module declares (`E0208`, `E0217`), stated once (`E0105`), with no
  parameters (`E0216`).
- `/` and `%` are not available, since they have no answer for every pair of values: the module
  provides its own functions (`std.bigint.div` returns an `Option`). Bit operators, wrapping,
  saturating and checked arithmetic, and rotations stay with the built-in integers (`E0201`,
  `E0223`).
- A type with both `numeric` and a `key` is a numeric type: `<`, `<=`, `>` and `>=` compare keys,
  and it satisfies `numeric(a)` (§4.5), so `std.math.sum(xs)` works on a `List<BigInt>`. Without
  a `key`, the operators work but it does not satisfy `numeric(a)`. `==`, `compare` and `digest`
  always use the key.

### 4.5 Generics

```ply
fn apply<a, b | e>(x: a, f: (a) -> b / e) -> b / e = f(x)
```

Type parameters are lowercase names in `<...>`. Row parameters (for effects, §6.2) come after `|`;
write `<| e>` when there are no type parameters. A row variable among the type parameters is
`E0301`. Aliases and `new` records may take parameters too: `pub type Route<a> = { ... endpoint: a
}`, `type Pair<a> = new { first: a, second: a }`.

- A type parameter marked `keep`, as in `<keep a>`, may be filled with a resource (§7.1). Filling
  an ordinary type parameter with a resource is `E0317`.

**Label parameters.** A name in brackets is a resource label parameter (§6.2): `fn relay<[l]>(b:
Bytes) -> Unit / {net.send[l]} = net.send[l](b)`.

- Label parameters sit among the type parameters, before the `|`: `fn serve<a, [l], [k] | e>(..)`.
  `<[l, k]>` is short for `<[l], [k]>`.
- A call fills them left to right, either explicitly, `relay[conn](b)`, or from an argument whose
  row or type names a label (§6.2).
- A label that names a region (§7.2), in a `Hold`'s type or in a `hold` atom, is filled by the
  argument that carries the region, never written. So `fn log_first<[l], [r]>(h: Hold<Bytes, [r]>)
  -> Unit / {log.write[l], hold.read[r]}` is called as `log_first[audit](h)`.
- A printed signature shows them: `<[l]>(Bytes) -> Unit / {net.send[l]}`. Label variables print as
  `l`, `m`, `n`, then `l1`, skipping any name already used by a label in the same signature.
- Filling the wrong number of labels, leaving one unfilled, or using a label-generic definition as
  a value without calling it is `E0306`.

**Recursion and generics.** A call inside a recursive group (a definition calling itself, or
definitions calling each other) fills type, label and row parameters like any other call. So
generic `even<a>` and `odd<a>` can call each other. A recursive call may pass a different label or
a callback with a different row; what it then performs must fit the caller's row (`E0302`). A
constraint on a type parameter (`where numeric(a)`) must be met by the caller.

What a recursive call may not do is fill a type parameter with a larger type built from it, as
`f<a>` calling itself at `List<a>`. Each round would need a copy of the function at a bigger type,
and no finite set of copies would do. This *polymorphic recursion* is `E0308`. Passing a parameter
on unchanged, or using a fixed type like `Int`, is fine.

**Parameters of types.** A `type` (alias, sum or `new` record) takes type, label and row
parameters just like a `fn`:

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

- A row argument is written as a row (§6.2): `{fs.read_at[src]}`, `{log.write | e}`, a bare `e`,
  or `{}` for a function that performs nothing. A label argument is a label or a label parameter in
  scope.
- A use must fill every parameter the type declares. The wrong number is `E0202`; a type with a
  row parameter has no short form that leaves it out.
- A declared type is closed: a row variable in it must be one of its own row parameters (`E0301`),
  and a label in it is one of its own label parameters or a global label, never a parameter of the
  function using the type.
- An alias's arguments are substituted into its expansion, so `Step<Int | {}>` is just the function
  type. A sum's or `new` record's arguments are part of its identity, so `Seq<Int | {}>` is not
  `Seq<Int | {fs.read_at[src]}>` (§6.2). A function stored in a `Stepper<Int | e>` may perform
  less than `e`, as any function may perform less than its type allows.
- A printed type shows its arguments as written: `m.Sink<Int, [conn] | {}>`.

**Numeric constraints.** `where numeric(a)` lets a type parameter use arithmetic (`+`, `-`, `*`,
unary `-`) and ordering (`<` and the rest), and `numeric_of_int(n)` makes a constant of type `a`.
`where integer(a)` also allows `/`, `%` and the bit operators; using those under `numeric(a)` alone
is `E0209`. A call must fill `a` with a numeric type (`Int`, the fixed widths, `Float`, `Decimal`,
or a type with `numeric` and a `key`, §4.4; for `integer`, only the first two) or with a type
parameter that has the same constraint. Anything else is `E0201`. Each operator fails as it does at
that type: `/` or `%` by zero raises (§6.8), and fixed-width overflow ends the program.

```ply
fn sum<a>(xs: List<a>) -> a where numeric(a) = xs.fold(numeric_of_int(0), |s: a, x: a| s + x)
```

The type that fills a constrained parameter is passed as a hidden argument, so such a definition
must be called directly. Using it as a value is `E0310`; wrap it in a lambda instead. A constrained
parameter must appear in the signature's parameters or result, and inside a recursive group only
the definition itself can call it at a constrained parameter; otherwise `E0310`. Spec clauses may
assume their definition's constraints.

**Constraints on types.** A `type` may take a `where` after its parameter list, with `derivable`
constraints on its own type parameters (anything else is `E0102`). Wherever the type is written or
built, the argument for that parameter must satisfy the constraint (`E0206`):

```ply
pub type Game<s, a, c | e> where derivable(eq, a) = {
  status: (s, c) -> Status,
  choices: (s, c) -> List<a>,
}

pub fn find<a>(g: Game<Int, a, Int | {}>, x: a, y: a) -> Bool = x == y
```

`find` needs no `where`: a signature that uses the type at its own parameter gets the constraint
automatically, as with a `Map`'s key (§4.3). A `Game` whose choices cannot be compared, such as
`Game<S, (Int) -> Int, C>`, is refused where it is built, not later where a search over it runs. A
sum or `new` record whose fields use the type at one of its own parameters carries the constraint
the same way. An alias is checked at each use.

### 4.6 Built-in types

These are in scope everywhere, and redeclaring one is `E0105`:

```ply
Option<a>     = None | Some(a)
Result<a, e>  = Ok(a) | Err(e)
Ordering      = Less | Equal | Greater
Rounding      = HalfEven | HalfUp | Down | Up | Ceiling | Floor
Iter<s, r>    = Continue(s) | Stop(r)
Instant       = Instant(Int)
Duration      = Duration(Int)
```

`Instant` is a clock reading and `Duration` the time between two, both in nanoseconds. They are
separate types so a deadline cannot be added to a byte count. `std.time` builds and reads them.

A module that declares or imports (by name) its own `Ok`, `Err`, `Some` or `None` cannot use `?`.
One that defines its own `Ok` or `Err` cannot use `try` (§6.8) or tagged literals (§2.3); each is
`E0118`. One that declares its own `Stop` cannot use `iterate`.

**`Secret<a>`** holds a credential, a `String` or `Bytes`, that no Ply code can read.

- `Secret::of_string` and `Secret::of_bytes` seal a value the program already has. `config.secret`
  (`std.config`) returns a secret the program never sees in plain form. `std.random`'s
  `entropy.secret(n)` draws `n` random bytes straight into a secret.
- These make a new secret from secrets without revealing them: `Secret::bytes` (a string's UTF-8),
  `Secret::concat`, `Secret::decode` (from hex, base64 or base32 text), `Secret::encode` (to
  those), `Secret::private_key` (the key in a DER document), `Secret::rsa_jwk` (from an RSA JSON
  Web Key) and `std.hash`'s key derivations `hkdf` and `pbkdf2`. None returns part of a secret,
  since comparing a one-byte slice against 256 guesses would reveal it a byte at a time. To get two
  keys from one secret, call `hkdf` twice with two different `info` values.
- A secret's bytes are kept in memory no other value shares, and overwritten with zeros when the
  last reference is dropped.

A secret can be observed in only four ways: `==` and `!=` between two secrets, `Secret::verify`
against a string, `Secret::is_empty` and `Secret::len`. Comparison takes time that depends only on
the two lengths, never on where they differ, and reveals only equal or not, so a secret can only be
read by guessing all of it. Its length is not secret.

The runtime uses secrets in constant time: for a MAC (`std.hash`), and for signing, deriving a
public key, sealing or opening a message and key agreement (`std.crypto`). The results (a tag, a
signature, a public key) are not secret, since each is one-way in the key.

A secret cannot be shown, encoded, ordered, hashed, matched on or drawn by a `forall`. It can reach
a native function only through a parameter declared `Secret` (§12). The functions that accept one are:

- `std.password`'s hashing natives, which return a one-way hash;
- `std.fs`'s `write_secret`, which writes to a file only its owner can read;
- `std.net`'s `send_secret`, which writes to a connection.

The two writers send one frame, all or nothing: plain bytes, then the secret (raw, or as `hex`,
`base64`, `base64url` or `base32`), then plain bytes. That covers formats like a key file (a word,
the key in hex, a newline) and SASL PLAIN (`AUTH PLAIN `, a credential in base64, a line end). This
frame is the only way a secret leaves a program. `std.fs`'s `read_secret` reads one back from a file
in the same frame format, and returns a secret, never a plain value. This is how `ply keygen` writes
its key and `ply build --sign` reads it without the key ever being a plain value.

**`Hold<a, [r]>`** (§7.2), **`Task<a>`** and **`Chan<a>`** (§9) belong to a *region*, a scope
that releases them when it ends, and cannot outlive it.

- A `Hold`'s type names its region. `Hold` can be written only as the type of a parameter (of a
  definition, a lambda or a function type). In a `let` annotation or as a type argument it is
  `E0446`.
- A `Hold` is not data: it cannot be compared or ordered (`E0201`), hashed, derived or used as a
  `Map` key (`E0206`), and `reflect` never shows what it holds.
- A declaration exists outside every region. So a variant field, a `new` record field, or an
  operation's parameter or result that mentions a `Hold`, `Task` or `Chan` at any depth is
  `E0446`. Use a type parameter instead, `type Held<t> = Held(t)`, so the region travels with the
  type argument where the escape checks can see it.
- For the same reason, a field holding a function whose row names `hold`, or joins a task, or uses a
  channel, is `E0446`. Take the row as a row parameter instead, `type Held<| e> = Held(() -> Int /
  e)`, and the checker will see a closure that reaches a region leave it through that row. The
  same applies to an operation's parameters and result, since a handler could keep them past the
  region: give the operation a row parameter, `write put<| e>(f: () -> Int / e) -> Unit`.
- A `new` record's fields are visible wherever it goes, so it may write such a row directly.
  Still, rows in a declaration cannot name a region (§7.2), so a closure over a hold must go
  through a row parameter there too.

### 4.7 Function types, and what must be written

`(A, B) -> C` is a pure function that cannot raise. `(A) -> B / {abort.raise}`, `(A) -> B /
{db.read[users]}` and `(A) -> B / e` have effect rows. With no `/` the row is empty, both in
signatures and in declared types. A function value may perform less than its type allows (§6.2).
Functions cannot be compared, encoded, ordered or used as map keys. `once (A) -> B` is a function
that may be called at most once and never copied; it is a resource (§7.1). A function that may be
called again may be passed where a `once` one is expected, but a `once` one goes only where its
type is written `once` (`E0201`). `keep` before a function type's parameter means the function may
keep what it is passed, as on a `fn` (§7.1).

**Written:** every parameter type and return type of a top-level `fn` (`E0126`), every `forall`
binder type, and every top-level `fn`'s effect row. Callers, in this package or another, are
checked against the signature, never the body.

- The written row is an upper bound: what the body performs must fit inside it (`E0302`, which
  shows the row to write, using an `effect set` where the module has one). It may be wider than the
  body needs.
- Each written atom covers what it names. The mode atom `net.write[conn]` covers every `write`
  operation of `net` on `conn`; the operation atom `net.send[conn]` covers only `send` (§6.2).

**Borrowed function parameters.** A parameter of a top-level `fn` or `extern fn` whose type is
written as a function type is *lent* to the call: the function borrows it for the duration of the
call. The body may call it, pass it to another borrowed parameter, capture it in a lambda, or use it
inside a `handle` (whose lambda or continuation is then borrowed too, §5.3). Anything that could
keep it after the call returns is `E0475`: returning it, storing it in a record, list, constructor
or handler variable, performing an operation with it, or passing it to a parameter of another type
(including a type parameter) or to a function value.

- Write `keep` to let the function keep it: `f: keep (Int) -> Int`. `keep` is part of the signature
  and of the function's hash. A caller is checked against `keep`, never against the body, so a
  function the caller passes to a parameter without `keep` never outlives the call.
- `keep` also goes before a parameter whose type is a resource, which the call then takes (§7.1).
  Before any other type it is `E0323`.
- A parameter whose type is written as an alias or a type parameter is not borrowed, even if it is
  a function type: it may keep what it receives, so a borrowed function cannot be passed to it.

**Inferred inside bodies:** lambda parameter types and rows, `let` types, and everything else. A
local `let` is not generic, so `let f = |x| x;` used at two types is `E0201`.

## 5. Expressions

Everything is an expression, including `if`, `match`, `handle` and blocks. Expressions nest at most
128 levels deep. Each operand, parenthesis and block counts as a level, and the parser refuses
deeper nesting ("input is nested too deeply to parse"). An `else if` chain and a run of operators
of one precedence count as one level, however long.

### 5.1 Blocks and `let`

A block `{ statements... tail }` has the value of its last expression (its *tail*), or `Unit` if
it has none. `let <pattern> [: Type] = <expr>;` binds a name once; the `;` is required, and a later
`let` may shadow an earlier one. An expression statement needs `;` unless it is the tail or is
block-like (`if`, `match`, `handle`, a block, `with_hold`, `simulate`).

A `let` pattern can take a record apart, which is how a function returns several values:

```ply
fn sum_two(input: Bytes) -> Int = {
  let {value, next} = advance(input, 0);
  let {value: second, ..} = advance(input, next);
  value + second
}
```

A record pattern names every field or ends with `..` (`E0201`). A `let` whose pattern does not
match raises (§6.8). `let <pattern> = <expr> else { .. };` says what happens instead: when the
pattern does not match, the `else` block becomes the value of the enclosing block, and the
statements after it do not run. The `else` block has the enclosing block's type and cannot see the
pattern's names. A `?` inside it exits as it would from the block's tail.

```ply
fn first_word(line: String) -> String = {
  let [word, ..] = line.split(" ") else { "" };
  word
}
```

`if a { x } else if b { y } else { z }` requires braces, and every branch has the same type. An
`if` without `else` has type `Unit`.

### 5.2 `match` and patterns

```ply
fn area(s: Shape) -> Int =
  match s {
    Circle(r) -> 3 * r * r,
    Rect(w, h) -> w * h,
    Point -> 0,
  }
```

Arms are separated by commas (optional after a block-like arm) and may have a guard: `[x, y, ..rest]
if x > y -> x + rest.len(),`. A `match` must cover every case (`E0205`, which names a missing
case).

| pattern | matches |
| --- | --- |
| `_` / `name` | anything; `name` binds it |
| `Ctor`, `Ctor(p, q)`, `mod::Ctor(p)` | a constructor |
| `42`, `-1`, `1.5`, `1.50m`, `"s"`, `b"s"`, `b'{'`, `true`, `()` | a literal |
| `0..=9`, `b'a'..=b'z'`, `1u8..=9u8` | an integer from the first bound to the second, inclusive |
| `[]`, `[a, b]`, `[a, ..]`, `[a, ..rest]` | a list of exact length, or a prefix |
| `{a, b}`, `{a: p, b: q}`, `{a, ..}` | a record; `..` allows other fields |
| `(p, q)` | a tuple |
| `p \| q` | either alternative, tried left to right; use parentheses to nest |

Every alternative of `p | q` must bind the same names at the same types (`E0212`). An arm is tried
once per alternative, so if an earlier alternative matched and the guard rejected it, the guard runs
again for a later one. A plain `let` may use an or-pattern only if it cannot fail, as in `let Ok(v)
| Err(v) = r;`; one that can fail needs an `else` (`E0213`).

A range's bounds are two integer literals of one type, the first no greater than the second, and
not in parentheses (`E0222`). A range binds nothing:

```ply
fn value(src: Bytes, i: Int) -> Result<Json, ParseError> / {abort.raise} =
  match src.at(i) {
    b'"' -> string_value(src, i),
    b'-' | b'0'..=b'9' -> number(src, i),
    b'{' -> object_value(src, i),
    _ -> Err(error_at(i, "expected a value")),
  }
```

For `U8`, `U16` and `U32`, literals and ranges that together cover every value make the match
complete, and a missing range is named: `match b { 0u8..=9u8 -> .., 20u8..=255u8 -> .. }` is
`E0205`, not covered: `10u8..=19u8`. For other integer types, a match needs a final `_` arm.

### 5.3 Lambdas

```ply
|acc, a: Account| acc + a.balance
|| do_something()
|r: Result<Int, E>| -> Result<Int, E> { Ok(r? + 1) }
```

Type annotations are optional. A parameter without one takes the type the called function expects
there, once the earlier arguments settle it: in `accounts.map(|a| a.balance)`, `a` is an
`Account`. If nothing settles it, its type is inferred from the body, which then cannot read a
field of it or call a function on it (`E0201`). A lambda captures variables by value, and its
effects become part of the enclosing function's effects. A written return type requires a block
body and allows `?` inside it.

A borrowed function parameter (§4.7) stays borrowed wherever it goes in the body: a `let` bound to
it, an `if` or `match` that may return it, a lambda that captures it, and the continuation of a
`handle` that uses it (handlers are deep, so each clause runs in the body's place). Each of these
may be called, passed to a borrowed parameter, or used in a `parallel` branch, which finishes
before the block ends. Keeping it after the call returns is `E0475`, whose suggested fix adds `keep`
to the parameter:

```ply
fn twice(f: (Int) -> Int, x: Int) -> Int = f(f(x))
fn each_twice(xs: List<Int>, f: (Int) -> Int) -> List<Int> = xs.map(|x: Int| twice(f, x))
fn later(f: keep (Int) -> Int) -> () -> Int = || f(1)
```

`each_twice` passes `f` on through a lambda that `List::map` only calls. `later` returns a lambda
that holds `f`, which only `keep` allows. A lambda's own parameters are not borrowed, and neither
is a call's result, except a continuation's: a continuation returns what its `handle` may return,
so a continuation whose `handle` may return a borrowed function cannot be passed to any parameter.

### 5.4 Calls

`value.name(args)` is a call on a value.

- If the value is a record with a field `name`, it calls the function stored in that field:
  `codec.decode(json)`.
- Otherwise it calls `T::name(value, args)`, the function `name` of the value's type `T` (§3.1):
  `s.push(1).size()` is `Stack::size(Stack::push(s, 1))`. The type alone decides which function,
  with no import needed, so the value's type must be known where the call is written. A type
  parameter or a function type has no functions to call (`E0201`), and a type with no function of
  that name is `E0101`.
- A label is written as on any call: `s.note[audit]()`.
- A built-in type's functions are its builtins (§12) and what its home module declares (§13).

`value.m::f(args)` is `m::f(value, args)`, whatever the value's type. This lets a function that is
not one of the type's functions sit in a chain: `text.trim().parse::number()`. `m` is a module or
type in scope. A function of a type declared in another module is reached through that module:
`shapes::Shape::make(3)` and `s.shapes::Shape::area()`.

A bare name before the dot is read by what it names: if it is a variable, `x.name(..)` is a call
on its value; otherwise it is an operation of the effect `x` (§6.3). A variable and an effect with
the same name, both in scope at such a call, is `E0105`.

Positional arguments fill parameters left to right. The rest must be named (`greet("ada",
greeting: "hey")`) or have defaults. Errors:

- a positional argument after a named one: `E0124`;
- an unknown or repeated name: `E0123`;
- too few arguments: `E0202`;
- a parameter left unfilled in a call that used names: `E0125`.

A call on a value fills the parameters after the value the same way: `s.scaled(by: 3)`. A function
value (a lambda or a function stored in a field) has no parameter names to use (`E0123`), and
neither does an effect operation. There is no partial application.

### 5.5 Record update

`{..base, deep: {..base.deep, a: 7}}` copies `base` with the listed fields replaced, and returns a
record with the base's fields. The base is a variable, a field path, or one call (which runs once).
The base is evaluated first, then the fields left to right, as arguments are. The base's fields
come from its inferred type, wherever that type was declared, including another module.

- A base whose type is unknown, or that is not a record, is `E0116`.
- A field the base does not have is `E0117`.
- Updating a `new` record (§4.2) returns that record type, so each field keeps its declared type
  (`E0201`). Since it builds a value, updating an `opaque` record outside its module is `E0220`
  (§3.2).

### 5.6 Lists and updates in place

`xs.at(i)` returns `None` for an index past the end **or negative**: `xs.at(-1)` is `None`, not
the last element (that is `xs.at(xs.len() - 1)`). `xs.set(i, v)` with an index out of range ends
the program (`E0502`).

Values are immutable, but an update reuses memory when nothing else can see the old value.
`xs.push(x)` appends in place when the caller holds the only reference to `xs`, and otherwise
copies one path of the list's internal tree. `List::set` works the same way. `Array::set` copies the
whole array when it cannot update in place. The same holds for `Bytes` and `++`.

A copy happens when something else still holds the value:

- a variable that is used again after the update;
- a captured variable in a closure that is called again;
- a value read out with `Map::get`, `List::at` or `Array::get` (use `Map::update` instead);
- a caller that keeps using what it passed.

"Used again" means along the path the program actually takes. An `if` or `match` branch releases a
variable that only another branch uses, and a closure called by its only holder (as a sequence's
step function is) hands what it captured to that call. A variable passed to a call is read after
the call's other arguments, so `c.set(i, c.get(i) + 1)` and `f(buf, buf.len())` update in place, as
does the left side of `++`.

A record does not count as a second holder once it is done with a value:

- Reading a field that nothing reads again (neither the field nor the whole record) moves the field
  out of a record no one else holds.
- A branch next to one that reads the whole record releases the fields it does not read.
- A `let` or `match` pattern releases its subject once its names hold the fields (an arm with a
  guard keeps it for the arms after it). So `{ ..s, out: s.out.push(x) }` and `let { out, n } = s`
  grow `out` in place.
- A released subject's memory is reused for the next record or constructor of the same size the
  body builds. So `match s { Set(m) -> Set(m.insert(x, ())) }` updates a set no one else holds
  without allocating a new wrapper.

`ply check --costs` reports every `List::push`, `List::set` and `Array::set` in your modules that
copies, with its cause and a fix. In a `reuse fn`, a copying update is an error (`E0127`). A
dependency's `reuse fn` promises, including the standard library's, are checked when that package
is checked:

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n == 0 { xs } else { collect(xs.push(n), n - 1) }   // fine: xs is a parameter at its last use

reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {
  let ys = xs.push(n);
  if xs.len() < 0 { xs } else { ys }                       // E0127: xs is used again after the update
}
```

A call to another package's function is judged by its `returns` clause, never its body. `returns
fresh` promises a value nothing else holds; `returns xs` says the result may share the parameter
`xs`. Without a `returns` clause, appending to the result cannot be shown to update in place. The
clause goes between the signature and its specifications, and is checked against the body when that
package is checked (`E0148`):

```ply
pub fn digits(n: Int) -> List<Int>
  returns fresh
= range(0, n)
```

A list literal of at most 32 elements that its function only reads, or passes to a call that keeps
none of it, lives in the function's stack frame and allocates nothing: for example `sum([a, b,
c])`, where `sum` walks the list with `at`. A list the function returns, stores, carries through a
loop, or passes to a call that might keep it or an element of it (such as `List::fold`, which hands
each element to a function value) is allocated. A frame holds at most 128 words of such lists
(each list takes its elements' words plus three); further lists are allocated. Every list in a
function that may stop at a perform for a handler to resume later (§6.6) is allocated.

### 5.7 Iteration

There is no `for`, `while` or `break`. A function calling itself in tail position runs as a loop:
it does not grow the stack, so it may run any number of times. Tail position is the body's own
value, the tail of a block, an `if` or `match` arm, or the right operand of `&&` or `||`:

```ply
fn sum_to(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { sum_to(n - 1, acc + n) }
```

A tail call to another function in the same module is also a loop when the two are in one
recursive group (functions whose tail calls lead back to each other), so mutual recursion like
`even`/`odd` does not grow the stack either. The exception is a group containing a `handle` whose
clause resumes somewhere other than its tail (§6.6): it runs the body on a separate stack, so its
calls nest. A group whose `handle`s return directly, resume in tail position, or handle a raise (as
`try` does) keeps its loop.

Every other call nests, up to 10,000 deep (`E0502` beyond that): `1 + f(n - 1)`, a call inside
`handle`, `with_hold` or a lambda, and a call of another function. Each task has its own depth
(§9).

A loop that never ends runs until a clock stops it. A test, and each entry `ply prove` makes to
evaluate a claim, has 60 seconds by default (§8.4, §10). `ply run` has no clock by default, since a
server is meant to run forever; `--timeout MS` sets one.

`range`, the byte scanners, `List::map`, `List::filter` and `List::fold` do not nest calls:
`std.prelude` writes the list functions in Ply as tail calls over the list's internal tree.
`iterate` is a loop with an early exit and a budget of passes:

```ply
fn first_gap(xs: List<Int>) -> Int =
  iterate({i: 0, want: 0}, 1000, |s: {i: Int, want: Int}|
    if s.i >= xs.len() { Stop(s.want) }
    else { Continue({i: s.i + 1, want: s.want + 1}) })
```

The step function returns `Continue(next)` or `Stop(result)`. Using up the budget ends the program
(`E0502`).

### 5.8 `?`

```ply
fn int_value(text: String) -> Option<Int> = {
  let d = Decimal::of_string(text)?;
  Int::of_decimal(d, Down)
}
```

`e?` means `match e { Err(er) -> Err(er), Ok(x) -> rest }` (or the `Option` version), taking the
constructors from the enclosing function's **written** return type. It may appear wherever nothing
conditional sits between it and the function's result and everything evaluated before it is pure:
`let x = e?;`, or `parse_or_more(parse_and(ts)?)`.

- It converts no errors: `Result<_, E1>` inside a function returning `Result<_, E2>` is `E0201`.
- Inside a `handle` or `try` (its body, a clause or its `return` clause), `?` exits the whole
  function, as a raise would: every `handle` in between is left, none of their `return` clauses
  runs, and no clause catches it, a `try`'s included. The outermost such `handle` must stand where
  a `?` could (`E0119`):

  ```ply
  fn priced(sku: String) -> Result<Int, String> = {
    let base = handle { catalog::price(sku)? + rate.get() } with { rate.get() -> 3 };
    Ok(base * 2)
  }
  ```
- `E0118`: `?` inside a `with_hold`, a `simulate` or a `parallel` branch; inside a lambda without
  a written return type; or where `Ok`, `Err`, `Some` or `None` are redefined. Inside a lambda with
  a written return type, `?` exits the lambda.
- `E0119`: `?` in an `if` branch, `match` arm or right of `&&` that is not in return position; after
  an argument with effects (`g(h(x), k(x)?)`); or in a nested block. Bind the value first.

### 5.9 `parallel`

```ply
fn both(xs: List<Int>) -> (Int, Int) =
  parallel { xs.fold(0, |a: Int, x: Int| a + x), xs.filter(|x: Int| x > 0).len() }
```

`parallel { a, b, .. }` returns the tuple `(a, b, ..)` and means exactly that: the branches
evaluated left to right. The runtime runs them at the same time, on its own threads, whenever that
cannot change the result. The checker allows a block only when no two branches' effects conflict
(§6.2), so no branch reads what another writes. A block needs two or more branches; `parallel` is
an ordinary name when no `{` follows it.

- The block's effects are the union of its branches'. It is as deterministic as they are, so a
  deterministic test may contain one and its result can be cached.
- If several branches fail, the leftmost failure is the block's, whichever happened first. A branch
  to its right may already have performed effects.
- A block whose branches could reach a `handle` around it, or a `simulate` region (§9), runs its
  branches in turn, with the same result. A handler clause that resumes in tail position, captures
  nothing and has no handler variables does not force this: each branch gets its own copy on its
  own thread. A block written directly in a definition's body is judged against that definition's
  row, so a `handle` that handles none of the branches' effects does not force it. A block inside a
  lambda or a `handle` body runs in turn under any `handle` around it. So does a block whose
  branches hold a task or a continuation, and every block of a program started from another
  program's branch, as each test in a `ply test` run is.
- `E0309`: two branches perform operations of one effect on one resource and one of them writes;
  or one branch calls a function value with an open row (a row variable) while another performs
  anything; or a branch opens a `simulate` region or performs a `task` operation, since a task
  belongs to the scheduler of the thread that spawned it. `?` inside a branch is `E0118`.

`std.parallel` splits a list across nested blocks.

### 5.10 Termination

Ply checks that functions return. A function that might not must say so.

```ply
fn count(xs: List<Int>) -> Int = match xs { [] -> 0, [_, ..rest] -> 1 + count(rest) }

fn down(n: Int) -> Int = if n <= 0 { 0 } else { down(n - 1) }

fn climb(a: Int, b: Int) -> Int
  decreases a + b
  = if a + b <= 0 { 0 } else { climb(a + 1, b - 2) }
```

A *recursive group* is the definitions of a module that call each other in a cycle. A call counts
whether it is written by name, made by a tagged literal (§2.3), or made implicitly through a type's
`key` or `numeric` (§4.4): `a + b` at a type with `numeric` calls its `add`, and `xs == ys` on a
list of a keyed type calls the key.

The checker looks for a measure that every cycle of calls in the group makes smaller
(size-change termination). A measure can be:

- a part of an argument: a constructor's field, a record's field, a list's element or tail, a map's
  key, value or entry, at any depth;
- an integer moving toward a bound that a condition on the way to the call keeps it beyond, as in
  `down`, or `if i >= xs.len() { .. } else { walk(xs, i + 1) }`.

A value with no parts (a nullary constructor, an empty list, a literal) is no larger than any
argument, and a constructor with one field is no larger than what its field is part of. Dividing by
a literal greater than one, shifting right by one, and taking a remainder by the measure itself all
lower an integer that the condition keeps at one or more. Different calls may lower different
measures, as in Ackermann's function, and a condition in one member of a group bounds cycles
through the others. A condition of `n == 0` bounds nothing: from `-1`, `n - 1` never reaches it.

A condition is something checked on the way to the call: an `if`, a `match` arm's guard, the failed
guard of an earlier arm whose pattern always matches, a condition matched against `true` or
`false`, or `List::at` or `Array::at` returning `Some` (which keeps the index below the length).
`List::set` and `Array::set` keep the length.

Some list operations keep track of parts. A list built from parts, a list they are pushed onto, a
list `List::filter` keeps, a list `List::map` makes from a part of each element, and the result of
`List::fold` when each step returns the accumulator or a part are all made of parts. Each element is
a part, even though the list itself need not be smaller. A value an `if` or `match` chooses is
whatever each branch is. What a group member returns is computed to a fixed point over the group,
so a member that returns a part of its argument hands its caller a part.

Function values are followed too. A function given to `List::map`, `List::filter` or `List::fold`
is called with each element, and one given to `Map::fold` with each key and value. A function
given to a definition outside the group is called as that definition calls it. A function an `if`
chooses is followed through each branch. A lambda bound by `let` is followed where it is called, up
to eight lambdas deep. A group member passed anywhere else, or called from a lambda the checker
cannot follow, is called with nothing known.

A definition whose group is shown to shrink ends, and so does one that calls only definitions that
end. Any other may not return, and its row must say so with the atom `diverges` (§6.2):

- If its own recursion is not shown to shrink, or goes beyond what the check follows (a lambda
  deeper than it reads, or calls that combine into more than 10,000 size-change graphs), it must
  write `diverges`. Without it, the definition is `E0302` with no fix offered, since the usual fix
  is a recursion that shrinks, a budget with `iterate`, or a `decreases` clause.
- `diverges` belongs only on a loop that waits on something outside, such as a stream or a peer.
- A caller inherits `diverges` like any other atom, and there `E0302` offers to add it.
- A test, law or clause that may not return needs no `diverges`: its timeout bounds it.

`ply check --types` says under each such definition why it may not return: its own recursion is
not shown to shrink, or goes beyond what the check follows, or it calls a named definition that may
not return. `ply check --json` gives each definition's `ending`: its `kind` (`ends`, `stated` for
one that ends thanks to a `decreases` measure, or `diverges`) and, for one that diverges, `through`
and `at`.

`decreases <measure>`, after the other clauses, states an `Int` over the parameters for a descent no
single argument shows, as in `climb`. At every call the group makes back into itself, the callee's
measure at the call's arguments must be below the caller's, and the caller's must not be negative.
The checker proves this, as `ply prove` proves a claim (§10), using the body and the conditions on
the way to each call. A group that shrinks once its proved measures count is treated as ending. A
measure the checker cannot prove is `E0467`, which shows where the proof stopped; so is one on a
definition the checker sees ends without it, which can be deleted. A call into the group from a
lambda, a region or a guard leaves nothing to prove. A measure must be pure, like a clause
(`E0417`), and is part of the definition's hash.

### 5.11 Holes

```ply
pub fn reward_of(r: Rewards, p: Player) -> Float = ..

pub fn search(g: Game, s: State) -> Move =
  if over(s) { last(g) } else { ??? }
```

`???`, or `..` in place of a body you have not written yet, is a *hole*. A hole checks as any type
and performs nothing, so it never widens a written row, and a signature that promises purity stays
pure. The program checks, builds and runs. Each hole is a warning, never an error (`W0613`), and
the warning reports:

- the type the hole must have, as far as the body determines it, or that nothing constrains it;
- the effects it may perform: the definition's written row and what the rest of the body already
  performs, or that no row is written and the effects will be inferred;
- the names in scope whose type fits, local names first, at most eight;
- that reaching the hole at run time raises `abort.raise`.

A run that reaches a hole raises (`E0502`, `panic: unfilled hole`, naming the spot), so `ply test`
fails each test that reaches one and passes the others. A handler may catch the raise like any
other (§6.8). `..` and `???` are the same thing: each prints as written, and switching between them
changes no hash. `??` is not a hole: `x??` is `?` applied twice (§5.8), and `x???` is `x` followed by
a hole, which is an error. A skeleton of signatures with `..` bodies is a design the checker can
judge: `ply check` lists every hole with what it must be and may do. Warnings do not count toward
the cap on reported errors, so no hole is hidden.

## 6. Effects and handlers

An *effect* is a named set of *operations*, such as reading a database or the clock. Code
*performs* an operation the way it calls a function, and a *handler* decides what the operation
does. A function's type lists the operations it may perform in its *row* (§6.2), so the type says
everything the function can touch.

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

Each operation is a `read`, a `write` or a `raise`.

- A `read` or `write` returns to where it was performed.
- A `raise` never returns, so it declares a name and parameters but no result (§6.8).
- `[r]` makes a `read` or `write` take a *resource label*: every perform must name one, as in
  `db.get[users](3)` (`E0304` otherwise). Labels say which resource an operation touches, so the
  checker can tell that writing `[orders]` does not conflict with reading `[users]`.
- `nondet` marks an effect whose results are not determined by the program's state, such as a
  clock or the network (§8.3).
- A parameter marked `keep` takes ownership of a resource the perform passes, instead of borrowing
  it (§7.1).

Effects are nominal: two identically declared effects are different. The names `task`, `clock`,
`random`, `sim`, `abort`, `diverges` and `hold` are taken (`E0105`).

An operation may have its own type parameters, written just before its parameters: `read
take[r]<a>(key: Int) -> a`. These are its only type variables, so any other lowercase name, or a
type not in scope, is `E0102`. Each perform picks its own `a`, so a handler clause for the
operation must work for every type (`E0201`). The standard library's own effects with such type
parameters (such as `task`'s, §9) can only be handled by standard library modules (`E0165`). An
operation has no `where`. If an operation's types mention a `Map`, or a type that carries an order
(§4.2), whose ordering depends on one of its type parameters, each perform must pick an ordered type
for that parameter (`E0206`), and each clause may assume it.

### 6.2 Atoms and rows

An *atom* is one entry of an effect row: `effect.mode[resource]`, or `effect.mode` for an effect
without labels. A *row* is a set of atoms with an optional *row variable* at the end standing for
"and whatever else": `/ {db.read[users], clock.read}`, `/ {net.write[conn] | e}`, `/ e`, `/ {}`.
Atoms from another module are qualified with `::`: `/ {store::db.read[users]}`.

- A *mode atom* like `net.write[conn]` covers every `write` operation of `net` on `conn`.
- An *operation atom* like `net.send[conn]` names one operation, and is covered by its mode atom.
  Naming an operation the effect does not declare is `E0104`.
- A `raise` has no mode atom: a row names each raise operation, such as `toml.syntax`, so a row
  lists every way a call can fail (§6.8).
- `diverges`, written alone, is the atom of a call that may not return (§5.10). It names no
  operation, and nothing handles it.
- An effect with no operations is named by a mode atom, such as `native.read`, which likewise names
  no operation and is never handled.

**Inferred rows** name operations. Performing `net.send[conn](..)` adds `net.send[conn]`. A call
adds the callee's row as written (a mode atom stays a mode atom), and a lambda's type carries the
operations its body performs. A definition with a written row publishes that row: a caller of a
function written `/ {net.write[conn]}` gets the mode atom, meaning the callee may perform any
`write` of `net` on `conn`.

A written row must cover the inferred one, atom by atom. Under `/ {net.send[conn]}`, a body may
perform `net.send[conn](..)` but not `net.recv[conn](..)`, and may call a function written `/
{net.send[conn]}` but not one written `/ {net.write[conn]}`. Each is `E0302`, which names the
operation or callee and suggests the atom to add. `ply check --types --explain` prints the inferred
row as `body performs`.

**Function values.** A function value may perform less than its type says, but not more
(`E0302`). This applies when it is passed as an argument, returned against a written return type,
bound with a `let` annotation, or used as one of the elements of a list or branches of an `if`,
`match` or `let ... else`. It also applies to a function held in a record, tuple, `List`, `Array`,
`Option`, `Result`, `Map`, `Iter` or `Hold` (§7.2), and to a function returned by a function. So:

- `run(|| a.x())` fits `fn run(k: () -> Unit / {a.x, a.z})`;
- a callback whose row names `net.send[conn]` fits a row written `/ {net.write[conn]}`;
- two callbacks fill one row variable with both their rows: given `fn both<| e>(f: () -> Unit / e,
  g: () -> Unit / e) -> Unit / e`, `both(|| a.x(), || b.y())` performs exactly `{a.x, b.y}`, and
  `[|| a.x(), || b.y()]` is a `List<() -> Unit / {a.x, b.y}>`.

Inside a function's parameter, a `Task`, a `Chan` or a declared sum type, rows must match exactly,
because a function stored there could be given something its own type does not allow. A parameter
typed `() -> Unit / {net.send[conn]}` is not one typed `() -> Unit / {net.write[conn]}`, in either
direction.

The row given to a sum (§4.5) is an argument, not a bound, and must also match exactly: a
`Seq<Int | {net.send[conn]}>` is not a `Seq<Int | {net.write[conn]}>`, and a `Seq<Int | {}>` does
not perform. A constructor gives the sum exactly the row of the function it receives, so `Seq(||
None)` is a `Seq<a | {}>`. To build a sum with a wider row than its function needs, give the
function the wider type first with a `let` annotation. An alias's row argument is substituted into
its expansion, so `fn both<| e>(f: Thunk<| e>, g: Thunk<| e>)` over `type Thunk<| e> = () -> Unit
/ e` accepts two callbacks that perform different things, as the expanded form does.

**Labels.** Resource labels are global: two modules writing `[users]` mean one resource. A
definition may be generic over a label (§4.5). Inside the body, a label parameter shadows the global
label of the same name: under `fn relay<[l]>`, the `[l]` in a row, in a perform `net.send[l](b)`,
in a handler clause `net.send[l](x) -> ..`, and in a call `inner[l](..)` is the parameter, while a
label with no parameter of that name is the global one. A call fills a label parameter explicitly,
`relay[conn](b)`, or from an argument: a parameter typed `() -> Unit / {net.send[l]}` given an
argument whose row is `{net.send[conn]}` sets `l` to `conn`, and so does a parameter typed
`Sink<[l]>` given a `Sink<[conn]>`. A label left unfilled is `E0306`.

The standard library is generic over its labels: `std.net` and `std.http` use no fixed labels of
their own, so a program can accept connections under one label and call upstream servers under
another, sharing one serve loop and one writer.

**Conflicts.** Two atoms *conflict* when they name the same resource of the same effect and at
least one is a `write`. A `raise` conflicts with nothing. A label parameter or `[*]` may be any
label, so it conflicts with every label of its effect. `parallel` (§5.9) and the test scheduler
(§8.4) both use this rule.

**Counts.** A row also records how often each atom is performed: a `bounded` number of times that
no input decides, or a `scaling` number that grows with the input. An atom scales when it is
performed:

- inside a callback that `List::map`, `List::filter`, `List::fold`, `Map::fold`,
  `Bytes::position` or `iterate` calls once per element (unless the list, `range` or budget is a
  literal);
- inside a definition that calls back into its own recursive group;
- by a callee that performs it a scaling number of times.

Higher-order definitions count their callbacks the same way: `map(ids, lookup)` scales `lookup`'s
query, a definition that calls its callback inside a `List::fold` scales whatever it is given, and
one that calls it once does not. `ply check --types` prints each atom and row variable with its
count:

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

A definition's row may promise that an atom or its row variable is `bounded`: `/ {db.query[conn]
bounded}`, `/ {log.write | e bounded}`, `/ e bounded`. A body that performs it a scaling number of
times is `E0465`, which names the operation and the loop that repeats it. The usual fix is one
operation over the whole input, or moving the operation out of the loop. `bounded` is only allowed
in a definition's own row; in a function type or an effect set it is `E0466`. A definition from
another package is judged by its written row: an atom it does not promise `bounded` may scale, and
so may each callback it takes unless its row variable is promised.

### 6.3 Performing

`db.get[users](3)`, `clock.now()` and `store::db.put[orders](id, row)` perform operations and add
their atoms to the enclosing definition's row. A written row such as `fn stale(s: Session) -> Bool
/ {clock.read}` is an upper bound (§4.7).

### 6.4 Effect sets

```ply
pub effect set Persist = {store.read[db], store.write[db]}
effect set Full        = {Persist, log.write[app]}
```

An effect set is a name for a group of atoms, usable wherever a row is written. `pub` exports it:
another module imports it by name (`import storage (Persist)`) or names it through the module (`/
{storage::Persist}`). Each atom means what it means in the declaring module, so the importer does
not need to import the effects. Sets may include other sets, across modules too (a cycle is
`E0115`), but not a row variable. An unknown set is `E0114`, and a private one used elsewhere is
`E0107`.

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

`handle { body } with { clauses }` runs the body, and each operation the body performs is handled
by the matching clause. A clause is `effect.op[resource](params) -> value`. An optional `return x ->
value` clause transforms the body's result. A handler may also hold variables its clauses update
(§6.9).

**Wildcard labels.** A clause's label may be `[*]`, which handles the operation for **every**
label: `log.note[*](line) -> …` handles `log.note[users]` and `log.note[orders]` alike. This lets a
library serve an effect whose operations are per resource (a database driver over tables, a trace
sink over channels) when the label is chosen at the call site, not by the handler. A clause naming a
specific label handles only that label. Clauses are tried in order, so put a specific clause before
a `[*]` clause that would also match. `[*]` on an operation declared without a label is `E0304`.

`[*t]` matches every label like `[*]`, and **binds** the label the call site used to `t`, a
`String` in scope in the clause. This lets a handler check what it was asked, not just answer it.
A database driver's table check works this way: it handles `db.query` on every table and rejects a
statement that touches a table the call site did not name.

```ply
handle { load() } with {
  db.query[*table](sql, params) -> match labelled(table, sql) {
    Some(answer) -> answer,
    None -> panic("this statement reaches a table the call site did not label"),
  },
}
```

The bound label is the one the *call site* used, not the name of a label parameter: a clause
handling `relay[users]("k")` binds `"users"` even when the call is inside `fn relay<[l]>(…)`. A
clause may name a label or bind one, never both.

A **row** may also use `[*]`, which lets a library handle an effect for code it is given: `fn
sink<a | e>(body: () -> a / {log.note[*] | e}) -> a / {wire.put[c] | e}` says the body may perform
`log.note` on any label, and `sink` handles it. A row with `[*]` is a permission rather than a
requirement, so a body that performs nothing satisfies it; the body still may not perform an
operation the row does not name. `[*]` in a row on an operation declared without a label is
`E0304`.

**What a handler removes.** A clause for `net.send[conn]` removes that atom from the body's row. A
set of clauses covering every operation of a mode (`send`, `recv` and `close` for `net.write[conn]`)
removes the mode atom. The `handle`'s row is the body's row, minus what the clauses remove, plus
every clause's own row.

If a body performs operations under a mode atom the handler only partly handles, the rest would
escape the `handle`. That is an error (`E0305`, naming the operation, where it is performed or which
callee reaches it, and the clause to add), unless the enclosing function's written row covers it.
Then it passes on to the caller's handler. For example, `std.db`'s `transaction`, written `/
{db.begin, db.commit, db.abort | e}`, passes `begin`, `commit` and `abort` on while handling
`rollback` itself. A body that calls a function written with a mode atom, or a function value whose
type has one, may perform any operation of that mode: `E0305` then names the mode atom, says which
callee or value reaches it, and offers a clause for each operation not yet handled. A `handle` is
judged by rows alone, so a lambda or a function-typed parameter is judged by its type.

**What a handler sees.** A `handle` handles only what its body's row names: the operations the body
performs, and those of the definitions and function values it calls whose types name them. An
operation that reaches the body only through a row variable or a label parameter of the enclosing
definition belongs to the caller, whatever it turns out to be at run time. It goes to the handlers
around the call. So a definition generic over a row or label never sees or handles what the row or
label stands for:

```ply
fn quiet<| e>(f: () -> Int / e) -> Int / e =
  handle { f() } with { log.note(s) -> () }
```

In `quiet(|| { log.note("x"); 1 })`, the `log.note` goes to the handlers around the call of
`quiet`, never to `quiet`'s clause, which handles only a `log.note` that `quiet`'s own body names. A
library that wants to handle its callback's operations names them in the parameter's type, as `sink`
names `log.note[*]` above, and as `std.db`'s `transaction` takes `body: () -> a / {db.rollback | e}`
to handle the body's rollback. A clause on a label parameter handles that parameter, and a clause on
a named label never handles a perform on the parameter, whatever label fills it. A function value
gets its handlers from where it is called, according to its row there; this includes continuations
(§6.6).

### 6.6 `resume`

In `amb.flip[coin]() resume k -> k(true) * 2`, `resume k` binds the *continuation*: the rest of
the body after the perform, as a function. The clause then has the `handle`'s type.

- `k` is a `once` function (§4.7): it may be called at most once (`E0316`), and a value that holds
  it is a resource (§7.1), so wherever `k` goes it is called at most once.
- Without `resume`, a clause's value is returned to the perform site. A clause for a `raise` is
  different (§6.8).
- `k` is a function value. It may be kept after the `handle` ends and called later. Whatever the
  resumed body performs that its own `handle`s do not handle goes to the handlers around that later
  call, not those around the original capture.

A clause that binds `resume` and never calls it abandons the body where it stopped. To make sure a
body releases what it holds even then, use `bracket(acquire, release, body)`. `release` runs on
what `acquire` returned when `body` returns, when a clause abandons it this way or a raise unwinds
it (§6.8), and when its task is cancelled (§9). It runs where the bracket stands, with the handlers
around it.

- A `release` that fails replaces whatever was unwinding.
- A runtime failure ends the program, so nothing more runs, `release` included.
- Nested brackets release innermost first.
- A cancel does not interrupt an `acquire` or a `release`: it takes effect when that one returns
  (§9), so whatever was acquired is fully released.
- The body receives what `acquire` returned as is, and may keep it past the release. `with_hold`
  (§7.2) is the bracket whose body cannot keep what it holds.

### 6.7 Unhandled effects

- `E0302`: the body performs an atom or operation its written row does not cover.
- `E0303`: an effect escaped inference (a compiler bug).
- `E0305`: a `handle` lacks a clause for an operation its body performs on an atom it handles.
- `E0424`: an operation reached the host at run time with nothing set up to serve it, such as a
  test's process streams. Handle it (§14).

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

Ply has no exceptions; failures are `raise` operations, listed in rows like any effect. A `raise`
never returns to where it was performed. Its perform has whatever type is needed and adds the
operation to the row, so a row names every way a call can fail. Inference unions them, and they
pass through row variables, so `lines.map(parse_line)` raises what `parse_line` raises. A raise
declaration is a name and parameters, with no result, no resource label and no type parameters
(`E0001`).

A clause for a raise has the `handle`'s type: its value becomes the `handle`'s value, `return` is
not applied to it, and it cannot bind `resume` (`E0201`). Its parameters are what the raise
carried. The clause runs after the body is abandoned and the regions the body opened are closed,
so a raise inside a clause goes to a `handle` further out. A clause handles only the raise it
names and lets the effect's other raises pass. A raise nothing handles ends the program with
`E0502`, naming the operation and what it carried. In a test, it is the test's failure, which is why
a raise is never part of a test's footprint (§8.4). One raise is handled by the host under `ply
run`: `std.process`'s `pipe.broken`, if unhandled, ends the program as a closed pipe would, with
nothing written and status 141 (§16).

`try { body }` returns `Ok` of the body's value, or `Err` of what a raise in it carried: the value
for a raise with one parameter, `()` for none, and a tuple for several.

- It handles the one `raise` operation the body's row names (other than `abort.raise`) and removes
  it from the row.
- A body that raises nothing is `E0311`. One that raises several is `E0312`; write
  `try[toml.syntax] { body }` to choose one and let the others pass. Naming an operation that is not
  a `raise` is `E0313`.
- A named `try` whose raise can only reach the body through a row variable of the enclosing
  definition is also `E0311`, since that raise belongs to the caller. A function that handles a
  raise from its callback names it in the callback's type: `f: () -> a / {toml.syntax | e}`.
- `try` is a `handle` with that one clause and `return v -> Ok(v)`, so everything true of `handle`
  holds: a recursive group with a `try` keeps its loop (§5.7), a `?` inside exits the function
  through it (§5.8), and a module that defines its own `Ok` or `Err` cannot use it (`E0118`).

To turn an `Err` back into a raise, use a function:
`r.unwrap_or_else(|e: SyntaxError| toml.syntax(e))` (`ply doc std.result`).

**`abort.raise`.** The prelude declares `effect abort { raise raise(message: String) }`.
`abort.raise(m)` is that raise, and so is every other failure on a bad value: `panic`, `assert` and
`assert_eq`; a builtin used outside its domain (§12); `/` or `%` by zero (§2.4); a `let` whose
pattern does not match (§5.1); `iterate` past its budget; `task.join` of a cancelled task;
`task.channel` with a negative capacity; and `random.below` with a bound below one (§9).

- A signature that leaves `abort.raise` out of its row when its body can raise is `E0302`, which
  names the operation and offers the row to write.
- Overflow and the call depth limit end the program regardless of the row.
- A clause for `abort.raise` receives the message: `panic`'s argument, or what would otherwise be
  reported.
- Unhandled, it is `E0501` for a failed assertion and `E0502` for anything else.
- `try` handles it only when named: `try[abort.raise] { body }`.

**Calls that cannot fail.** A call that cannot fail where it is written adds no `abort.raise`:

```ply
fn peek(b: Bytes, i: Int) -> Int = if i < 0 || i >= b.len() { -1 } else { b.at(i) }

fn hex(b: Bytes) -> String =
  range(0, b.len()).fold("", |acc: String, i: Int| {
    let n = b.at(i);
    acc ++ "0123456789abcdef".slice(n / 16, n / 16 + 1)
      ++ "0123456789abcdef".slice(n % 16, n % 16 + 1)
  })
```

Neither function needs a row. A builtin that can raise states when it cannot, as a `requires` over
its parameters (`Bytes::at`: `0 <= i && i < Bytes::len(b)`). A call adds no `abort.raise` when the
conditions on the way to it prove every clause. The same goes for `/` or `%` whose `Int` divisor is
shown nonzero, and `task.channel` or `random.below` whose argument is shown in range.

These conditions count:

- an `if` condition; the left side of `&&` (which held) or of `||` (which did not);
- a `match` arm's guard; a literal pattern that matched or that an earlier arm failed to match; a
  range pattern (`b'0'..=b'9'` keeps the value between the two, and alternatives keep it between
  the least and greatest of them); a list pattern of a known length;
- `List::at` or `Array::at` returning `Some` in a `match` or `let ... else`;
- inside a lambda given to `List::map`, `List::filter` or `List::fold` over `range(lo, hi)`, that
  its last parameter lies in the range.

A `let` of one name is its value, and a field read through records is the same value wherever it
is read.

The checker reasons with linear `Int` arithmetic: literals, variables, fields, `+`, `-`,
multiplication by a literal, and what a builtin returns according to its `ensures` (`Bytes::at`
returns `0..=255`, a length is not negative, `min` returns no more than either argument,
`Bytes::scan` returns between `from` and the end). Division or remainder by a literal, masking with
`&`, `|` or `^` of two values in `0..2^n`, a right shift by a literal, and a remainder by a divisor
known to be positive are each bounded accordingly. So `n & 0xFF` and a byte's `n / 16` are in range
for `byte_of_int` and for indexing sixteen digits. A string or bytes literal has its written length.
Nothing else is known: a call of a definition is a value about which nothing is known, whatever its
body says, and a width other than `Int` is understood only through `Int::of_u8` and similar. What a
call that can raise ensures holds only after the call.

This check looks only at the body it is in, so a row never depends on another function's body. It
decides by elimination over the integers, within a limit. Where it cannot show a clause, the call
keeps its `abort.raise` and nothing is refused. So an index loop that recurses on `i + 1` with only
`i >= b.len()` as its guard still raises, at a negative `i`: guard both ends, or fold over a
`range`. A definition's own `requires` is not used (§10): it is not checked at callers, so the body
cannot rely on it.

`ply check --types --explain` lists, under each definition, every place its body can raise
`abort.raise` and why: a builtin's `requires` clause nothing proves, a divisor not shown nonzero, a
`let` that can fail, a builtin with no `requires` (`panic`, `String::of_bytes`), a definition whose
row names the raise (where it is first called), or a function value whose type does. A row that
names a raise the body never makes prints `declared, not performed`, as does one whose only raise
is the definition's own recursion. Under `--json`, each definition has both, as `raises` (each with
`at`, `kind`, `why` and, for a builtin, `callee` and `requires`) and `declared_not_performed`.

A raise in a `parallel` branch is handled around the block. A task may raise: its raise goes to the
task's own `handle`s and then to those around its `simulate` region, never to the copies of
handlers it inherited when spawned (§9). Outside `simulate`, it ends the program.

### 6.9 Handler variables

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

A handler can keep state in variables: `var name = init` or `var name: T = init`, at the start of
its `with` block, before any clause.

- The initializers run in order when the `handle` starts, before its body and outside the handler:
  an operation an initializer performs goes to the handlers around the `handle`, and adds to its row.
- Each initializer sees the variables above it. Two with the same name are `E0105`.
- The variables are visible in every clause and in `return`, never in the body, which reaches them
  only through the operations its row names.

A clause assigns a variable with `name = value;`, anywhere in its blocks and in the branches of its
`if`s and `match`es.

- An assignment inside a lambda, or inside a nested `handle`, `simulate`, `parallel`, `try` or
  `with_hold`, is `E0315`, since that code may run after the clause has returned. So is one in
  `return`, after which the variables are gone. A lambda sees the value a variable had when the
  lambda was created.
- Assigning a name that is not a variable of this handler (including a clause parameter, or a `let`
  that shadows a variable) is `E0314`. In a clause of a nested `handle`, only that handler's
  variables can be assigned.

A clause without `resume` gives the body back the values it ended with. A clause with `resume k`
continues the body, when it calls `k`, with the values it holds then, and changes made during that
resumption stay in it unless what it returns carries them out (as `return` can).

Tasks spawned under a handler share its variables (§9). A clause runs without other tasks'
clauses interleaving until it performs something that may wait, such as a `task` or `clock`
operation or a host operation. So a clause that reads a variable, waits, and then assigns it may
overwrite what another task assigned in the meantime. Keep a read and the assignment that depends on
it on the same side of every wait. `simulate` tries the different orders in which tasks reach a
handler's variables, so a test of such a handler finds the lost update.

### 6.10 Real handlers

An effect's own module may name a default handler for when the program does not handle the effect
itself. This is called the effect's **real handler**:

```ply
pub nondet effect os {
  read pid() -> Int
}

host for os by served

fn served<a | e>(go: () -> a / {os.read | e}) -> a / e =
  handle { go() } with { os.pid() -> machine_pid() }
```

Every root the runtime starts (`main`, an entry `ply run` names, a test) runs under the real handler
of each effect its row names, after the root's own handlers. The runtime calls the real handler
with a closure over the root, so an `os.pid()` that no `handle` in the program handles reaches `served`,
and one inside `os::on` (`std.os`) never does. A root whose row names no such effect runs as it is.
The wrapping happens at the entry point, after checking: a test's row, and so `E0412`, is what the
test performs, never what a real handler does with it.

Only a standard library module may declare a real handler (`E0161`), for an effect it declares
itself (`E0162`), once (`E0163`). The function must handle the whole effect on every label, judged
by its signature alone (`E0164`): it takes one function with no parameters, whose row names every
`read` and `write` operation of the effect (on `[*]` where the operation takes a label) plus a row
variable. It returns what that function returns, and its row is the same variable plus anything the
handler adds of its own: raises, and effects that have no real handler. So its body, checked like
any function, handles every operation (§6.5). The effect's raises are still the program's to
handle. A real handler takes no label, has no `where` and is not a `const fn`.

## 7. Resources, holds and regions

### 7.1 Resources

A *resource* is a value that must be released exactly once, such as a connection. Ply tracks
ownership of resources, so it can never be used after release or released twice.

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

`release for T by f` makes `T` a resource: each value has one owner, is never copied, and is
released once, by `f`. `T` must be an opaque sum type of the module (`E0225`, `E0217`, `E0208`),
and `f` takes one parameter written `keep T` and returns `Unit` (`E0218`). A type whose constructors
hold a resource is a resource too, as is a record or tuple holding one, and a `once` function
(below). Releasing one releases every resource it holds. All other values are copied freely.

**Owning, taking and borrowing.**

- A `let`, a name bound by a pattern, and a parameter written `keep` *own* their resource.
- Passing it to a `keep` parameter, returning it, putting it in a record, list or constructor, or
  consuming it inside a lambda *takes* it. Using the name afterwards is `E0316`.
- Passing it to a parameter without `keep` *lends* it: the callee borrows it, may use it and lend
  it on, but cannot take it (`E0318`), and the owner still owns it.
- Otherwise a resource is not data: comparing it, showing it, or filling a type parameter with it
  is `E0317`, unless the type parameter is written `keep` (`fn hold<keep a>(x: keep a) -> ..`).
- An operation's parameters lend or take in the same way, and a clause's binding borrows or owns
  accordingly.

**Automatic release.** A resource its owner still holds when its block ends is released there,
last bound first. `f`'s row joins the row of the code that owns it (a definition, test or lambda),
unless a `handle` around the owner handles it (`E0302`). In `greet`, `close(c)` runs after `send`
returns.

- If the block's last expression does not use the resource, it is released before that
  expression, so a tail call stays a loop.
- If some branches take it and others do not, it is released at the start of each branch that does
  not.
- It is also released when a raise, a clause that does not resume, a `?`, or a `let` whose pattern
  does not match leaves its block, and when a release that runs before it fails.
- A resource created where nothing owns it is `E0322`: as a statement, as a lent argument, bound to
  `_`, left out by `..`, or stored in a handler variable. So is a statement that names an owner and
  does nothing with it, or a whole block lent whose value is something it binds.

**Taking a resource apart.** A `match` on a resource takes it apart: its parts go to the arm's
bindings and its own release does not run. Only the declaring module can see an opaque type's
constructors, so only it can do this, which is how `close` releases a `Conn` without releasing it
again.

- A guard only borrows (`E0316`), since a guard that fails passes the value to the next arm. So does
  the right side of `&&` or `||`, which may not run.
- A resource field is moved out of a record once (`E0316`), one field at a time, never through a
  path `r.a.b`. One field cannot be moved out while another resource stays behind (`E0316`): take
  the record apart instead, `let { conn, .. } = r;`. After that, the record can only be used through
  an update that writes every moved field again. An update that overwrites a resource the record
  still holds is `E0322`.
- A field read through a borrowed record is borrowed.

**Closures and continuations.**

- A lambda that consumes a resource it captures takes it. It must be bound by `let` and only called,
  at most once (`E0316`). If its block ends before it is called, what it took is released with it.
- A `once` function may be called at most once and never copied. Calling it consumes it, so only
  its owner (a parameter written `keep once`) may call it (`E0318`).
- A lambda that only borrows what it captures (or calls one that does) may be called or lent while
  the owner holds the resource, but may not be returned, kept or stored (`E0318`).
- A clause, a `return` clause, a `simulate` body and a `parallel` branch may run any number of
  times or none, so each may borrow a resource from outside but not consume it (`E0316`).
- An operation's continuation (§6.6) is a `once` function, so it is a resource too: `resume k`
  binds a `k` the clause may call at most once (`E0316`).

**Errors.**

- `E0225`: `release for` a type that is not opaque.
- `E0316`: a resource used after it was consumed, or consumed by code that may run more than once;
  a function that consumed one passed on; a continuation resumed twice.
- `E0317`: a resource would be copied: compared, shown, or used as an ordinary type parameter.
- `E0318`: a borrowed resource consumed, or a function borrowing one that might outlive it.
- `E0322`: a resource created or named where nothing owns it.
- `E0323`: `keep` before a type that is neither a resource nor a function type.

### 7.2 Holds and regions

`with_hold` is the general way to acquire something, use it within a scope, and release it when the
scope ends. The scope is called a *region*, and the value is accessed through a *hold*:

```ply
fn first_byte(path: String) -> Bytes / {fs.open[repo], fs.read_chunk[repo], fs.close[repo], io.refused, io.closed} =
  with_hold[file](io::reading[repo](path), io::shut) { source -> io::read(source, 1) }
```

`with_hold[r](acquire, release) { h -> body }` runs `body` with `h` bound to a hold on what
`acquire` returned, a `Hold<a, [r]>`. At the region's closing `}`, `release` is called on it.
`release` is any function of the value, `(a) -> Unit / e`, so one form works for every kind of
resource: a library exports the pair (`std.io`'s `reading` and `shut`). The region's row is
`acquire`'s, `release`'s and the body's.

`release` runs exactly once, where the region stands and with the handlers around it, however the
body ends: it returned; a raise or a clause that did not resume unwound it (before the clause that
handles the raise runs, §6.8); or its task was cancelled (§9). A cancel does not interrupt
`acquire` or `release` (§6.6), so what was acquired is released. If `acquire` raised, nothing is
held and nothing is released. A `release` that raises replaces what was unwinding. Nested holds are
released innermost first. A runtime failure ends the program and releases nothing.

**Reading a hold.** `h.get()` returns the held value and adds `hold.read[r]` to the caller's row,
where `r` is the hold's region.

- A function that takes a hold has the region as a label parameter (§4.5): `fn first<[r]>(h:
  Hold<Bytes, [r]>) -> Int / {hold.read[r]}`. A call fills it from the hold it passes, never
  explicitly.
- A closure that reads a hold carries the atom in its type.
- `with_hold[r]` handles `hold.read[r]` for its own hold only. `hold` is a built-in effect
  (`E0105`, `E0103`).

**A hold cannot escape.** `Hold` can be written only as a parameter type (of a definition, a lambda
or a function type), and anywhere else is `E0446` (§4.6). So holds can be passed to functions and
into the closures of `List::map`, `List::fold` and `iterate`, and the checker still sees every way
out of the region: the hold itself by its type, and a closure that reads it by its row, both of
which name the region. Code that reads one hold may leave another hold's region. A hold is released
at the `}` of its own region, so each region has its own name: a `with_hold[r]` inside a region
already named `r` is `E0330`.

Inside the region, `r` in a `hold` atom means the region. Everywhere else, and under every other
effect inside the region, `r` is the global label of that name (§6.2). So a row written outside the
region cannot name it: a parameter typed `() -> Int / {hold.read[r]}` does not accept a closure over
the hold (`E0302`), and a `with_hold[r]` does not handle the `hold.read[r]` of a function it calls.
Pass such a closure through a row parameter instead: `fn twice<| e>(f: () -> Int / e) -> Int / e`.
A definition's own row says `hold.read[r]` only when a clause that binds `resume` (§6.6) ran the
rest of a region's body after the region ended. No region handles that atom, so it appears in every
caller's row.

**Tasks and regions.** A task's scheduler must be younger than any region whose hold the task can
reach. Open the scheduler inside the region, `with_hold[r](..) { h -> simulate { .. } }`, not
around it, `simulate { with_hold[r](..) { h -> .. } }`. An older scheduler (an enclosing `simulate`
region (§9), or the program's own top-level scheduler (§14)) runs unjoined tasks after the region's
`}`, and a `task.join` inside the region does not help, because no type records the join. A task
reaches a hold through the closure it is given, and through the handlers around its `task.spawn`: a
clause answering an operation the task performs runs on the task's behalf, so under an older
scheduler it may not read the region's hold either. This check covers spawns a body performs
itself, not those of functions it calls.

**What a hold protects.** The value `Hold::get` returns is an ordinary value, and a copy of it may
leave the region even though the hold cannot. So a library makes the held value useless without its
hold: its functions take the `Hold`, and refuse to work on something already released, as a
`std.io` source read after its close raises `io.closed`. A lazy `Seq` (`std.seq`) whose steps read a
hold carries `hold.read[r]` in its row, so it must be built and consumed inside the region; letting
it leave is `E0446`.

A continuation captured inside a `with_hold` body and resumed after the hold was released finds
the hold gone. Its first `Hold::get`, or else the region's own `}`, ends the program with `E0331`,
and `release` does not run again.

**Errors.**

- `E0330`: a `with_hold` names a region that is already open.
- `E0331`: a hold read or released after its region released it (at run time).
- `E0446`: `Hold` written where a hold would be kept; a hold, or a closure that reads one, escaping
  its region; a region's value outliving it (passed to an operation, or reached by a task whose
  scheduler is older than the region); a declared type's field or an operation's signature
  mentioning `Hold`, `Task` or `Chan`; or a variant field, operation parameter or result holding a
  function whose written row reaches one (§4.6).
- `E0449`: a task, channel or continuation reaching a host operation, a host result, or an entry
  point's argument or result (at run time). A continuation's type is a function type, so this is
  the only check that catches it.

## 8. Tests

### 8.1 Writing tests

`test "label" { ... }` is an item with a block body (§6.5 shows one). A test cannot be `pub`,
referenced or given arguments.

- A test is identified as `<module>.<name>`, and its name is its label as written, holes and all.
  Two tests (or two laws) in one module cannot share a label (`E0105`), with one exception: two
  tests over cases may share a label that has holes, as long as their case types or tables are
  written differently. Each of their cases is named by the label it produces (below). A test whose
  label an earlier test has is named by the label followed by the first of ` (2)`, ` (3)`, … that
  no test has as its label or name.
- The body must have type `Unit`: a test passes when it finishes, so one that ends with a value
  (such as a comparison missing its `assert`) is `E0201`.
- `assert(cond)`, `assert(cond, Some("why"))` and `assert_eq(actual, expected)` fail with `E0501`.
  `assert_eq` reports both values and their first difference. Any other failure is `E0502`.
- A test that passes but leaves one of its own objects alive once it ends fails with `E0505`. This
  is checked after its tasks finish and its bodies release what they held. Every value is released
  once nothing holds it, so an object left alive is a leak, and a defect in Ply. What the build
  stored, a cached constant's stored copy, and what a scheduler holds for itself are not the test's.


**Tests over cases.** A test can run once for each element of a table. Each run is a *case*:

```ply
type Resolution = { reference: String, target: String }

test "resolves {c.reference}" for c: Resolution in resolutions() {
  assert_eq(url_join(base(), c.reference), url_parse(c.target))
}
```

`for <name>: <Type> in <table>` goes between the label and the body (on a `test/nondet` too).

- The table is an expression of type `List<Type>` (`E0201`), usually a call of a function that
  builds it, often from `embed_dir` (§3.4). A function with no parameters is computed once however
  many cases read it. A record literal in it is a `new` record when the case type is one (§4.2).
- The name is bound in the body and in the label. The label is read like an interpolated string
  (§2.3): each `{expr}` is a hole, and `{{` and `}}` are braces.
- Each case is identified as `<module>.<label>`, using the label its holes produce, so every case
  and test in a module needs its own label. If a case has the same label as another case in its
  table, another test's name or a case of another test, its test fails as a whole with `E0105`,
  naming the label, and none of its cases runs. Two identical cases produce the same label, and a label with no holes
  fits only a table of one case.
- Cases are told apart by their value as built, which is finer than `==`: `1.5m` and `1.50m` are
  two cases, as are two values of a keyed type (§4.4) whose keys are equal. So one case's pass
  never stands for a case its body could tell apart.
- The case type must be hashable without a `key` (`derivable(hash, ·)`, `E0206`): no `Float`,
  function, `Task`, `Chan` or `Secret`, including inside a keyed type.
- Cases are listed before any test runs, so the table and the label may raise (any `raise`
  operation, §6.8) but do nothing else (`E0469`). A table that raises fails as a whole, under the
  test's name. An empty table is no test.
- A tagged literal (§2.3) in a table, a label hole or a body is checked when the program loads, like
  any other.
- A module with a test over cases imports `std.cases` automatically, so `std.cases` itself cannot
  have one. A doc comment above such a test documents nothing, as above any test (`E0003`, §2.1).

**Snapshots.** A test can compare a rendering to a stored file, a *snapshot*, read with `embed_dir`
(§3.4). The test then performs nothing, is cached, and runs again when a stored file changes:

```ply
import std.snapshot
import std.snapshot (Stored)

fn stored() -> Stored = { dir: "snapshots", files: embed_dir("snapshots") }

test "an order renders as stored" {
  snapshot::check(stored(), "order.txt", snapshot::render(order()))
}
```

`check` fails with a unified diff from the stored text to the rendering, or with the diff that
would create a missing file. It never writes. To store snapshots, an entry point calls
`snapshot::accept`, which writes through `std.fs`, and you run it with the directory granted: `ply run
. --fs snapshots=snapshots` (§14). See `ply doc std.snapshot`.

### 8.2 Which tests run

A definition's hash covers a normalized form of it: names, comments, formatting, imports, `pub`,
specs and test labels are removed, and references are replaced by the hash of what they refer to.
A call on a value (§5.4) refers to every function of that name the module can see (its own and
those of each module it imports, directly or indirectly), since the hash is computed before types
are known.

A test runs only when there is no recorded pass for its hash, or for the compiled code it would
run, that still holds. So renames and comment edits run nothing, and neither does an edit (or a new
`ply`) that compiles a test to the same code. The summary counts those as `by code` (`by_code` in
`--json`, reason `same code`). `ply hash` prints the hashes, and `ply check --json` gives each
definition's and test's `hash`.

A pass also records what the test read from the outside world, and stays valid only while all of
that is unchanged; otherwise the test runs again (reason `changed`):

- each file and directory a host handler read for it under a root (recorded by the root's name, so
  a pass holds across checkouts);
- each standard library module it used, recorded as the functions, types and effects the test
  actually reached, so an edit to a function no test reached runs nothing;
- what every `ply` process it started read, including that `ply`'s own program;
- for a test that reached a host handler, the permissions it ran with (what `--fs`, `--exec` and
  `--allow` lend).

These are not inputs: a read handled by the test's own handler, a read of something the test wrote
first, the clock, the network, programs other than `ply`, and caches kept between runs
(`PLY_C_CACHE`, `PLY_C_STAGE`). A `ply` process that ended before reporting what it read records no
pass.

Flags:

- `--explain` says why each test was selected. For a `changed` test, it names the input that changed
  (`moved` in `--json`). It also shows what a pass is recorded under (the test's hash, the runtime,
  and the compiled code; `filed_under` in `--json`), and where the run's time went, phase by phase
  (`phases` in `--json`).
- `--filter SUBSTRING` runs tests whose `<module>.<label>` contains the text. A filter that names a
  module, one holding a `.` with a whole dotted run that is a module's name (`std.ascii85.`,
  `std.ascii85`), matches only from within a test's `<module>.`, never inside a label. Repeat it to
  run tests matching any of them.
- `--shard K/N` runs part K of N of the selected tests. Tests are split by a hash of their name, so
  a test is always in the same part, and parts `1/N` to `N/N` together run each test once.
- `--no-cache` ignores recorded results and cached checks.

Each case of a test over cases (§8.1) is selected as a test of its own. Its hash is the test's hash
(covering the body and the case type, but not the table or the label) combined with the case's
value. A case added to the table runs alone, wherever it is added. Reordering the table or
rewording the label runs nothing. Editing the body runs every case, and editing what the table is
built from runs only the cases whose values changed. A case's label is the one its holes produce,
which is what `--filter` and `--shard` match. Listing the cases runs the table, so a run that
reports on a test over cases builds the program even when every case has a pass. A run lists only
the tables of the modules its path names that a `--filter` could match a case of: a filter that
names a module lists that module's tables and those beneath it.

### 8.3 Determinism

A test whose row, after its own handlers, still has an atom of a `nondet` effect is an error
(`E0412`). Handle the effect, or write `test/nondet "label" { ... }`. A `test/nondet` is cached like
any test (§8.2): what it reads from the clock or the network does not count as an input.

### 8.4 Scheduling and failures

Tests whose footprints do not conflict run at the same time, in `parallel` blocks (§5.9) on one
thread per core. A test's *footprint* is the set of effects it performs on the outside world; a test
whose effects are all handled inside it conflicts with nothing. Only atoms of `nondet` effects
conflict between tests, since anything else is determined by the test's own state. A raise is never
part of a footprint, since the run handles it as the test's failure. `--jobs N` (`-j`) deals tests
into `N` lanes, each running its tests in turn (default: one lane per test). `ply prove --jobs N`
deals out its claims' points the same way.

The report is printed as the run goes:

1. what was selected and how it will run: the groups and workers, isolation, the host permissions,
   and what `--explain` says about selection;
2. each group's results as that group finishes, in the order the groups ran, so a long run shows
   progress and its first failure early;
3. the run's own figures (handshakes, the backend, simulation, and with `--explain` where the time
   went), the summary, and each failure's diagnosis.

`--json` prints one object when the run ends. `--workspace` prints each package's heading before
it runs.

`--timeout MS` is how much wall-clock time each test may take (default 60000; `0` is no limit). It
is not a verdict: a test over it is *abandoned* (`W0612`), reported separately from failures,
recorded nowhere, and run again next time. A run with an abandoned test is not a success, since it
decided nothing about that test.

**Suspects.** A failure lists its suspects: the definitions its test reaches that changed since the
test last passed, and the test itself when its own text changed. A passing run stores the text of
each test that passed and of each definition it reached. A definition whose hash changed is edited
when its text differs from the stored one, and changed only because something it uses changed when
its text is the same. The test comes first, then each definition that was edited, added or
removed, then the rest. After the failures, `ply test` prints each change once as a diff of its
text then and now, with where it is now. A diff longer than 40 lines is cut short. A test that never
passed, or a run under `--no-cache`, has no earlier version to compare: its suspects are the
definitions it reaches that no passing run has seen.

`--json` prints one object with each failure's diagnostic, declared footprint, suspects and replay
command, and `diffs`: each change once, with its name, kind, location and unified diff
(`schema_version` 8). Each result lists, as `performs`, the operations its test performed,
including handled ones. `--watch` reruns on every `.ply` change and on every change to a file the
last run embedded, keeping caches in memory.

Each case of a test over cases (§8.1) passes, fails and is reported under its own label, so a
failing table names every failing case. In `--json`, a case's entry under `selection.tests` has
`case`: the test it belongs to (`of`), its position in the table (`at`), and its `identity`, the
BLAKE3 hash of its value. A failing case's suspects are what it runs, which excludes the table.

### 8.5 Coverage and mutation testing

`--coverage` reports which tests reach each definition, and which definitions no test reaches,
from the hashes alone.

`--mutate [DEF]` runs after a passing run. Each definition (or only `DEF`, a full name or a unique
simple name) is changed one operator or literal at a time: `+`/`-`, `<`/`<=`, `>`/`>=`, `==`/`!=`,
`&&`/`||`, `true`/`false`, a `!` removed, an integer increased by one. The program is checked
again, and the tests that reach the definition run against the mutant, using a scratch store that
never touches the cache. A mutant that every one of those tests passes is a *survivor*: it is
reported with its location and the tests that let it through, and fails the run. A mutant that does
not type-check is skipped. `--mutate-budget N` (default 64) limits how many mutants are tried. A
case (§8.1) reaches what its test does, including its table, and a mutant that changes a case's
value is killed by that case, which the table no longer contains.

### 8.6 Compiled backend

Every command compiles the program to C and runs the compiled code; there is no interpreter.
`ply test` and `ply run` take `--profile development` (the default: `tcc` on x86-64 Linux, else
`cc -O0`) or `--profile release` (`cc -O2`), which picks the C toolchain settings. Other commands
compile under `development`.

Every program that checks compiles. A definition Ply fails to compile is a defect in Ply, reported
as `E0505` when the program is built.

| variable | effect |
| --- | --- |
| `PLY_CC=cmd` | the C compiler to run in place of `cc`; while it is set, `tcc` is not used |
| `PLY_C_CACHE=DIR` | where compiled code and other build results are cached, keyed by content (default `ply/cache` under the user's cache directory: `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere) |
| `PLY_C_CACHE_MAX=BYTES` | a size limit for the cache, enforced by deleting the least recently used entries first, but never one used in the last hour; `0` is no limit |

These variables are for working on `ply` itself:

| variable | effect |
| --- | --- |
| `PLY_C_STAGE=DIR` | where the compiler's own build stages are kept, separate from the cache (default `ply/stage` next to it) |
| `PLY_C_PHASES=1` | print what each build phase did and how long it took |
| `PLY_HEAP_POISON=1` | fill freed memory with a pattern and fail on any read of it |
| `PLY_HEAP_DELAY=N` | reuse freed memory only after `N` more frees |

## 9. Simulation

Concurrent code is tested by running it under a scheduler that controls the order tasks run in and
can try every order:

```ply
simulate {
  let a = task.spawn(|| settle("alice", "bob", 60));
  let b = task.spawn(|| settle("alice", "carol", 60));
  task.join(a);
  task.join(b);
  assert_eq(overdrawn(ledger.balances()), 0)
}
```

`simulate { ... }` handles the language's concurrency effects with a seeded scheduler:

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

- The region's row gains `sim.read`, which a deterministic test may have.
- Time is virtual: it advances only when no task can run, so `clock.sleep` takes no real time.
  Outside a `simulate` region, `task` is the program's own scheduler and `clock` is the real clock
  (§14).
- A task performs operations against the handlers around its `task.spawn`, so a clause it reaches
  can read only the holds the task itself may (§7.2). A clause that returns, or resumes in tail
  position, handles the task's operation as it would the spawner's. One that resumes elsewhere, or does not
  resume, would capture or abandon the spawner's body rather than the task's, and is refused when
  the task reaches it (`E0502`). This does not apply to clauses for raises (§6.8).
- If a spawned task raises and no clause the task installed handles it, or a raise in the region's
  body is not handled there, the region first cancels every other task, so each releases its
  brackets (§6.6). Once they have ended, the raise arrives where the `simulate` is written, as if
  the region's body raised it. A second raise from a task during this goes nowhere.
- A `parallel` block (§5.9) inside a region runs its branches in turn, so the scheduler sees none of
  it. A branch may not open a region (`E0309`), since a region's schedule is drawn from its entry's
  seed in the order regions open.

Errors:

- `E0413`: a `Task` or `Chan` escapes in the region's result, directly, inside a value, or inside a
  closure that captured it. A closure's type shows a captured task only as `task.join`,
  `task.await` or `task.cancel` in its row, a captured channel as `task.send`, `task.recv`,
  `task.close` or `task.select`, and either as the `sim.read` of a `simulate` it opens to use one;
  so a function whose row has any of these may not leave the region. Nor may a task or channel
  from outside come in: the region may not use a variable from outside whose value holds one. Each
  region numbers its own tasks and channels, so a handle that sneaks into another region (in a
  closure or through a type parameter) fails there at run time with `E0413`.
- `E0414`: no task can make progress (deadlock), or an interleaving took more scheduling steps than
  `--sim-steps` allows.
- `E0416`: a `simulate` inside another. A `simulate` inside a task of the program's own scheduler
  (§14) is allowed, its schedule drawn from the entry's seed.
- `E0425`: a host operation inside the region, refused before the handler runs, whether or not it
  is set up. The region handles `task`, `clock`, `random` and `sim.seed` itself.

**Cancellation.** `task.cancel(t)` stops `t` where it is. A sleep, join, channel operation or host
operation it is waiting on is abandoned; an operation that completed but whose result it has not
used yet loses its result; and it performs nothing more except the `release` of each `bracket` it is
in (§6.6). When it next runs, it only unwinds, releasing what it holds.

- A bracket is where a result is kept. Inside an `acquire`, a cancel stops waiting but takes no
  result: the `acquire` continues (an operation that completes at once still completes, and one that
  would wait ends it), and the cancel takes effect when it returns, with what it returned released.
  Inside a `release`, a cancel interrupts nothing and takes effect when the `release` returns.
- `cancel` returns `false` for a task that had already finished (whose result it leaves alone) and
  for one already cancelled.
- `task.await(t)` is a join that returns `Some` of the result, or `None` if `t` was cancelled.
  `task.join` of a cancelled task has nothing to return and raises (§6.8).
- A task cannot cancel itself or the region's body.
- A deadline combines the two: one task sleeps and then cancels another, which a third awaits.
- The search tries each cancel one step earlier and one step later, and from each of those the next,
  until it has tried every point it could land.

**Channels.** `task.channel(n)` makes a channel holding up to `n` values no receiver has taken yet.
`0` makes a rendezvous channel, where a send waits for the receive that takes it.

- A send waits while the channel is full, and returns `true` once its value is queued or handed
  over. A receive waits while it is empty and returns the oldest value sent.
- `task.close(c)` ends sending: a waiting receiver gets `None` and a waiting sender `false`; later
  sends return `false` without sending; receives take what was queued before the close, then
  `None`. Closing twice changes nothing. A negative capacity raises (§6.8).
- Cancelling a task waiting on a channel stops its wait, and a value it was sending is dropped. A
  value a receive returned is lost with a receiver cancelled before it used it, unless the receive
  was a bracket's `acquire`.
- All operations on one channel are ordered against each other, so the search tries every order two
  senders or two receivers could take.
- A race is one channel: each worker sends its result, the first receive wins, and cancelling the
  others stops them.

`task.select(arms, wait)` performs one of several channel operations. An arm is a channel with
`None`, to receive from it, or with `Some(x)`, to send `x` to it. The first arm (in the order given)
that can proceed without waiting does, and `select` returns `Some((i, got))`: the arm's index, and
`Some` of the value received or sent, or `None` if the channel was closed (a receive found it closed
and empty, or a send was refused). When no arm can proceed, `wait: false` returns `None` at once,
and `wait: true` waits on every arm, queued in each channel as a plain send or receive would be, and
proceeds with the first that becomes ready, as if it had never waited on the others. Arm order is
the only priority, so a caller that wants fairness rotates the arms, as `std.chan` does. All arms
carry one type, so to select over channels of different types, use channels of one sum type. A
select is ordered against every operation on each of its channels.

**Interleavings.** Tasks switch only at `task`, `clock` and `random` operations. Any two
allocations, and any two accesses to one handler variable where one writes, are ordered. A read and
then a write with no scheduler operation in between run as one step, so put a `task.yield()`
between them to let the search try an interleaving there. When the default search covers every
possibility, the result holds for **every** interleaving and is reported as `exhaustive`.

| flag | meaning |
| --- | --- |
| `--sim dpor` | search guided by which steps can affect each other (dynamic partial-order reduction); the default |
| `--sim random` | one random interleaving per seed |
| `--sim once` | exactly one interleaving |
| `--seeds N` | seeds per test, from 0 (default 1 under `dpor`, 64 under `random`) |
| `--sim-roots FROM..TO` | the seeds from `FROM` up to, not including, `TO`: `5..6` is seed 5 alone |
| `--sim-budget N` | interleavings per seed (`dpor` only) |
| `--sim-steps N` | steps per interleaving before `E0414` |
| `--seed 7`, `--seed 7:3.0.2` | replay one interleaving; implies `--sim once` |
| `--measure-reduction` | also run the search twice more without the reduction (`naive`) and without ordering from spawns and joins (`blind`), and report each count; a failure only they find fails the test |

Results are cached per search plan. A search that runs out of budget passes but is not cached. A
failure prints the racing steps (each one's task, and the definition and position where it first
touched shared state) and a replay command such as `ply test --seed 0:0.1.0.2 --filter 'no account
is ever overdrawn'`.

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

- `requires` and `ensures` go between the signature and the body, as many as you like, along with
  `cost` and `space` (below). `result` names the return value in `ensures`. `requires` limits the
  inputs for which the `ensures` must hold. It is not checked at call sites, and laws do not inherit
  it. A call that relies on a contract must prove it (below).
- A law has a label, optional `forall` binders (each with a type; a type that cannot be quantified
  over is `E0418`, such as a sum whose function performs the type's row parameter), an optional
  `where` guard, and a block body.
- A law's or schema's `where` may start with `identity(a)` constraints on its type variables, then
  a comma and the guard: `forall (xs: List<a>, n: Int) where identity(a), n > 0`. Under
  `identity(a)`, `==` at `a` is identity, so the law only claims types whose `==` is identity: not
  `Float`, `Decimal`, a type with a `key`, an `extern type`, `Hold`, `Task`, `Chan`, `Secret`, a
  function, or anything holding them. Without the constraint, `==` at a type variable means only
  what holds at every type (below). Using a constrained schema at a type that breaks the constraint
  is `E0479`, and a law over such a variable is not a lemma at other types. A leading `identity(x)`
  where `x` is not a type variable of the binders (or a schema type parameter) is read as the guard.
- Specs, guards and law bodies must be pure (`E0417`), except that they may raise (§6.8) or not
  return (§5.10). A law body may be a `simulate` region.
- Specs do not change a definition's hash. An `ensures` implies that every resource outside the
  definition's footprint is unchanged; there is no `old()`.
- `Int` arithmetic is checked, so bound inputs with guards, as above.

**Every claim is proved, or the run fails.** `ply prove` exits 0 only when every claim it reports
on is `proved`.

| outcome | meaning |
| --- | --- |
| `proved` | holds for every input the guard allows, or a `cost` or `space` bound read from the body holds |
| `refuted` (`E0419`) | false at the input shown |
| `vacuous` (`E0420`) | the guard allows no input |
| `unproved` (`E0474`) | nothing proves it and nothing refutes it |
| `defect` | Ply failed, not the program; nothing is claimed |

A property that cannot be proved is not a claim a program may rely on. Check it in a test (§8.1)
instead, over values `std.gen` generates. Then it says what a test says: that it held where it was
run.

**What the prover can do.** It proves claims using: evaluation of ground terms; enumeration of
finite domains up to 4096 points; linear `Int` arithmetic; case splits; congruence; constructor
injectivity; unfolding non-recursive definitions; taking a `match` arm when the value's shape is
known; applying a lambda; `List::map`, `List::filter` and `List::fold` over the known part of a
list; running a terminating (§5.10) recursive definition on known arguments (each one known, a
function, or a value of a built-in type); the contracts of called definitions (below); exhaustive
interleaving; and induction.

Induction works on a definition that calls only itself and terminates (§5.10). It is unrolled over
known values as deep as the unfolding goes. On an `Int` binder, the claim is proved for `n <= 0`
and then for `n > 0` assuming it at `n - 1`. On a `List` binder (structural induction), it is proved
for `[]` and then for `[h, ..t]` assuming it for `t`. `List::len` and `List::push` are reduced over
a known list spine, and `List::len` is known to be below `i64::MAX`. A small trusted kernel checks
every induction step from the claim alone: the binder it names, a hypothesis strictly smaller than
the case being proved, and only terminating definitions unrolled.

**Equality in proofs.** Congruence and injectivity treat two terms as one value. The program's `==`
is read according to the operand type:

- At `Int`, `Bool`, `Char`, `String`, `Bytes`, `Unit`, the fixed widths, and lists, maps, records
  and sums of them without a `key`, `==` is identity.
- At `Float`, `==` is IEEE's: neither side is `NaN`, and they are at the same place in the order
  (`-0.0` equals `0.0`). `<`, `<=`, `>` and `>=` are the order, false when either side is `NaN`.
  `Float` arithmetic is evaluated only when both operands are literals.
- At `Decimal`, `==` compares values, so `1.5m == 1.50m` even though they print differently.
- At a type with a `key` (§4.4), `==` compares the keys. Since the prover does not read the key
  function, such a comparison (and `min`, `max` and `Map` operations on keyed values) is treated as
  a call that may end the program.
- At a value holding any of these, `==` is read part by part where the shapes are known, and
  otherwise as a symmetric relation, true of a value and itself only when no `Float`, keyed value or
  type variable could be inside.
- At a type variable, `==` is identity only under an `identity` constraint.

An operator at a type with `numeric` is the function it names. An operator on a type the prover
does not know (a type variable, or a lambda's result) is a function of its operands that may raise.
A `perform`, `handle` or `simulate` is a value the prover does not read, and a `let` that can fail
raises where it does not match. In a law, and in any claim's own clause and guards, each of these
must be shown unreachable. An `ensures`, by contrast, speaks only about what its definition returns:
a raise in the body, or a call the prover cannot show returns, is a run that returns nothing the
`ensures` talks about. Calling a function value returns a known result where the prover can read
its body, or in a law where a binder's type performs nothing (the law is then about such
functions). An `ensures` may be given functions by callers it cannot see, so calling a function
parameter there gives nothing to rely on.

An `unproved` claim names each place the prover stopped, and says `unclosed` where it read the whole
claim and found a case it could not close: the claim may be false there, or may need a lemma or a
guard.

**Invariant claims.** Each definition in the module of a type with an `invariant` (§4.4) must prove
that every value of the type it builds satisfies the invariant, under the conditions on the way to
the build. These claims are named `<owner> builds <type>`.

- The definition's `requires` are not assumed, since no call is checked against them. A body that
  raises or never returns builds nothing.
- What a lambda builds must hold for every argument. A constructor passed on as a function, or
  applied where the prover cannot read, leaves the claim `unproved` as `unseen_build`.
- A value of the type that a claim receives (a law's binder, or a call's result) is assumed to
  satisfy the invariant. A value the claim itself builds is not.
- No run builds values of the type to test a claim with. Only the prover checks these claims.

**Contracts.** The prover reads some calls by their contract instead of unfolding the body: calls
of a recursive definition or another package's definition, when the callee's row names no raise and
the checker sees it terminate. At such a call, each `ensures` holds at the call's arguments and
result wherever its `requires` hold.

- Another package's contract is that package's to prove. The call must still be shown to return.
- A contract in this package is used once a proof has shown every `ensures` of its definition
  without relying on any call in its own body. Then the call returns wherever the `requires` hold.
  The clause being proved also holds at each call its own definition's body makes back into it, on
  the way to that call. So `fn quadrupled(n: Int) -> Int ensures result == n * 4 =
  doubled(doubled(n))` is proved from `doubled`'s `ensures result == n * 2` without reading
  `doubled`'s body.
- A contract or law proved over a type variable does not show that a call returns when it fills the
  variable with a type whose `==` may call a key.

**Proving by running.** A run proves a claim only when it covers every input.

- A domain of at most 4096 points is checked whole (for a law, after a short static search).
- A law over a `simulate` region is searched at every interleaving of each point the guard allows
  (§9). A search that stops early, or binders with an infinite domain, leave such a law `unproved`.
- An `Int` binder is finite when the guards bound it between two literals: each bound a comparison
  of the binder with a literal (`n > 0`, `n <= 100`, `3 <= n`, `n == 7`) that is a guard or one side
  of its `&&`s. So `where n > 0 && n <= 100` is a hundred points.
- Over any other domain, a claim the prover left open is tried at a few small points: zero and its
  neighbors; each literal in the claim and its guards, and their neighbors; two strings; `NaN` among
  floats; the empty list and lists of one and two elements; an empty map and a map of one entry;
  each variant of a sum; a record with each field in turn at its second value; and an `Int` for a
  type variable. Holding at these points proves nothing. A point where the claim is false refutes
  it, a point where it raises is reported, and a proof of the claim under its guard counts once one
  of these points satisfies the guard.
- No run creates a value of a type another module declares `opaque` (§3.2), or a function, so a
  claim over either is proved by the prover or not at all.

**Lemmas.** A law with no guard, once proved, is a lemma for every claim below it in its module and
every claim in a module that imports its module, directly or indirectly. A guard-free law of
another package, which that package proves, is a lemma for the claims of modules that import its
module. A lemma is applied through its *trigger*: the first call its body always makes whose
arguments mention every binder. Where a claim makes a matching call (each argument of its binder's
type), the law at that call becomes a known fact. A trigger that takes an argument apart, as
`reverse(ys.push(x))` does, stands in for the call in place of its body:

```ply
fn reverse(xs: List<Int>) -> List<Int> =
  match xs { [] -> [], [x, ..rest] -> reverse(rest).push(x) }

law "a push reversed leads" forall (ys: List<Int>, x: Int) {
  match reverse(ys.push(x)) { [h, ..t] -> h == x && t == reverse(ys), [] -> false }
}

law "reverse twice is identity" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }
```

Both are `proved`, the second by structural induction on `xs` using the first. Under `--json`, a
proof certificate's `rules` name each lemma as `lemma` with its `law` and `label`; each induction as
`induction` with its `binder`, `def`, `over` (`int` or `list`) and `step`; and each `cost` or
`space` bound read from a body as `cost_bound` or `space_bound` with its `def`. `ran` says whether
running the code helped make the proof.

**Law schemas.** A `law schema` states a law once, over the definitions it is about, and a law
instantiates it:

```ply
pub law schema round_trip<a, b | e>(encode: (a) -> b / e, decode: (b) -> Option<a> / e)
  forall (x: a) { decode(encode(x)) == Some(x) }

law "a URL round-trips" = round_trip(url_encode, url_decode)
```

- A schema has a name, type parameters, at most one row parameter, and parameters with written
  types (function-typed parameters are treated as `keep`, §4.7). Then it has what a law has: binders, a guard and a
  body, pure like a law's (`E0417`). Calling a function parameter whose row performs anything other
  than a raise or `diverges` is refused in the body.
- The row parameter is what the given definitions may raise: `abort.raise`, their own raises
  (§6.8), or nothing. Filling it with anything else is `E0417`. A schema has no label parameters. It
  may be `pub`, and is imported and qualified like a definition.
- An instantiation gives one argument per parameter, in order (`E0202`), each of the parameter's
  type (`E0201`, or `E0302` for a function that performs more than the row allows): a definition's
  name, a lambda or a value. It writes no binders: the schema's binders are quantified at the types
  the arguments determine, which a `forall` must be able to range over (`E0418`, at the
  instantiation, naming the binder). A type parameter no argument determines stays a variable of the
  claim. Instantiating something that is not a schema is `E0472`.
- An instantiation is the law obtained by replacing each parameter in the schema's guard and body
  with its argument. It has its own label, key, outcome, certificate and cache entry, and is proved
  as that law written out would be. Proved with no guard, it is a lemma with that body's trigger.
  Its hash covers the schema's guard and body, wherever declared, and its arguments. Under `--json`
  its claim has `schema`, the schema's full name, which `--explain` and a refutation print.
- A schema also declares a function for its body, taking its parameters and then its binders, so
  `round_trip(url_encode, url_decode, "a b")` checks the claim at one point. Its guard is declared
  as `<name>_where`. Both are `transparent` (below), share the module's namespace (`E0105`), and do
  not count as definitions with or without claims. What a schema's body calls in its own package is
  read from another package like any definition there: by its contract, unless it is `transparent`
  too. `std.laws` holds the schemas the standard library states its laws with (`ply doc std.laws`).

**Other packages.** A definition from another package (a dependency, or the standard library when
you are not working inside the library itself) is known only by its `requires` and `ensures`. A
proof may use what it promises but never unfolds its body; that package proves it. A `transparent
fn` is known by its body too: a proof anywhere may unfold it, so its body and everything it reaches
is part of what dependents are checked and proved against.

**`ply prove` output.** It reports the definitions that carry no claim, then each claim's outcome.

- An `unproved` claim says what stands between it and a proof. Under `--json` this is `gap` (a
  sentence) and `gap_kind`: `open` (with each place the prover stopped as `stuck`);
  `unhandled_effect` (an `ensures` whose definition performs effects, which only the prover can
  close); `raised` (with the point as `raised_at`); `unwitnessed` (proved under a guard no value
  was found to satisfy); `endless` (a law over a `simulate` region whose binders cannot be covered
  by running); or `unsearched` (a law with more interleavings than the search ran).
- A failure of Ply's own, such as a proposition its unit holds no root of, is a `defect`,
  reported under `E0505` as `ply test` reports one, with `defect`: its `code`, `message`, the
  `bindings` Ply failed at, and a `summary`.
- `summary` counts `proved`, `refuted`, `vacuous`, `unproved` and `defect`.
- A claim's type variables are named by where they first appear among its binders (`forall (x: a,
  y: List<b>)`), and a proof treats each as an unknown type (`uninterpreted a, b`).

Flags:

- `--filter SUBSTRING`: the claims whose owner's name contains the text, as `ply test` matches a
  key (§8.2); repeat to match any.
- `--prove-budget N`: static reasoning steps per claim; a claim that runs out is `unproved`.
- `--timeout MS`: wall-clock time each entry a claim's evaluation makes may take (default 60000;
  `0` is no limit), as `ply test --timeout` bounds a test. An evaluation over it is abandoned
  (`W0612`) and proves nothing: the claim is `unproved` as one whose evaluation raised, with the
  runtime's message as its `gap`, and it is not cached.
- `--reach`: ask the static prover alone about every claim, cached or not. Under `--json`, each
  claim then has `reach`: what it decided (`proved`, `guard_unsatisfiable`, `open` or
  `budget_spent`), the steps it took, and each place it left the fragment it can decide, as `{kind,
  about}`; `null` for a law over interleavings, which the static prover never sees.

A proof made from the claim alone is cached under the claim's hash, which reads other packages'
definitions by their contracts, so it survives edits to a dependency's body. A proof that running
helped make is cached under the hash of every implementation the claim runs. Only proofs are
cached: any other outcome is recomputed on every run.

**Cost bounds.** A `cost` clause bounds how many steps a call takes:

```ply
fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost xs.len() * (ys.len() + 1)
= xs.fold(0, |a: Int, x: Int| ys.fold(a, |b: Int, y: Int| b + x * y))
```

The bound is an `Int` expression over the parameters, and the steps must be at most a constant times
the bound plus a constant, at every size including zero. With `ys` empty, these steps still grow with
`xs`, so `xs.len() * ys.len()` would not hold. A `cost` clause on a definition whose row has
`diverges` is `E0468`, and on one that performs anything other than a raise is `E0463`, since a
handler could change the steps.

A function parameter may give a name to the steps one call of it takes, after its type. Only the
definition's `cost` clauses can use that name, as an `Int` of at least 1 (`E0105` if it clashes with
a parameter or another cost name):

```ply
fn each_of<| e>(xs: List<Int>, f: (Int) -> Int / e cost k) -> Int / e
  cost xs.len() * k
= xs.fold(0, |a: Int, x: Int| a + f(x))

fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost xs.len() * (ys.len() + 1)
= each_of(xs, |_x: Int| total(ys))
```

At a call, the name stands for the steps of the function passed in: a lambda's body, a named
definition's body, or the caller's own parameter's cost name. So `pairs` combines `each_of`'s bound
with its lambda's. A function passed in whose steps grow with its own input leaves the call
unreadable.

A `cost` bound is a claim, `<owner> cost #<n>`, that `ply prove` reads from the body and never
proves by running:

- A native function costs at most one step. A function `std.prelude` writes in Ply (§12) costs
  what its body takes: `List::at`, `List::push` and `List::set` take at most a constant number,
  since a list's internal tree is never deeper than an `Int` has bits, and a `Map` function on one
  key takes the steps of one walk down the map's tree.
- `Map::fold`, `iterate`, `Map::update` and `Bytes::position` call a literal lambda or named
  definition as many times as an argument's size allows. `List::map`, `List::filter` and
  `List::fold` cost what they state, `cost xs.len() * k`: one step per element plus each call of
  the function. What `map` and `filter` return is no longer than their input. A raise ends the call.
- A call of a definition costs its steps at its arguments' sizes; a call of another package's
  definition costs its first `cost` bound.
- A recursion is understood when every call back either passes a part of one list parameter that a
  list pattern took the head off, or moves an `Int` parameter by a positive literal toward a limit
  the path to the call keeps it short of. The limit's parameter must stay unchanged, and the limit
  can be: a literal, from below or above; a parameter's length, from below (`if i >= xs.len() { 0 }
  else { walk(xs, i + 1) }`, also through `!`, an `&&` that held, and an `||` that did not); or the
  length of a list where `List::at` found an element (`match xs.at(i) { Some(x) -> walk(xs, i + 1),
  None -> 0 }`). No path may call back twice, and every other parameter the steps grow with must be
  passed on unchanged or as a part of itself. Such a recursion takes as many calls as the list's
  length, or as lie between the index and its limit. That grows with the `Int` itself unless
  `List::at` found an element there, since it might start below zero. An `Int` literal's size is its
  magnitude, so a call starting at `0` grows with the limit alone.
- The bound may use: lengths of parameters (`List::len`, `Map::len`, `String::len`, `Bytes::len`,
  `Array::len`, `Columns::len`), cost names, positive literals, `+`, `*`, and `ilog2`, alone or next
  to sizes of the one length it applies to.

Steps within the bound are `proved`. Anything else leaves the bound `unproved` (`E0474`), stopped at
`unbounded` with the reason. That includes another operation, a call of a function value with no cost
name, definitions calling each other, another package's definition with no `cost` bound, an `Int`
parameter the steps grow with, and a definition that takes, returns or builds a value of a type with
a `key` or `numeric` (§4.4), whose comparisons and operators call functions the reading cannot see.
If a bound cannot be shown this way, do not write it.

**Space bounds.** A `space` clause bounds the memory a call holds beyond what it is given:

```ply
type Chain = | Link(Int, Chain) | End

fn reversed(xs: Chain, onto: Chain) -> Chain
  space 0
= match xs { End -> onto, Link(x, rest) -> reversed(rest, Link(x, onto)) }
```

The bound is read like a `cost` bound, over the parameters, and `0` is allowed: `space 0` says a
call runs in place, holding no more than a constant amount whatever it is given.

- A call's space is every object it creates (even ones it frees before returning) plus a frame for
  each non-tail call back into itself. A number is not an object.
- The bound holds when each argument has no other holder, all the way down, as an update in place
  needs (§5.6). A `match` or `let` pattern without a guard releases a subject it was the last to read
  and held alone, and the next constructor of the same name the body builds reuses its memory. So
  `reversed` builds no `Link` it did not take apart.
- `List::push`, `List::set`, `Array::set`, `Columns::set`, `Map::insert` and `++` update a value
  held once in place and copy one held anywhere else. Only a fold's first call copies a seed held
  elsewhere.
- A record, a record update, a list literal, a closure and a list pattern's rest each create an
  object, as does a constructor with no released subject to reuse.

A function parameter may give a name to the memory one call of it holds, after its type and its
cost name if any. Only the definition's `space` clauses can use it, as an `Int` that may be `0`
(`E0105` if it clashes):

```ply
fn wrapped<| e>(xs: List<Int>, f: (Int) -> List<Int> / e space s) -> List<List<Int>> / e
  space xs.len() + xs.len() * s
= xs.map(f)
```

At a call, the name stands for what one call of the function passed in holds, as a cost name stands
for its steps. A function may hold nothing, so `space xs.len() * s` would not hold here: `map` builds
the list it returns whatever `f` holds.

A `space` bound is a claim, `<owner> space #<n>`, that `ply prove` reads from the body using the same
rules for calls and recursion as `cost`, with two differences. A recursion whose calls back create
nothing and are last holds only what its last call holds, whatever it descends on. A parameter to
which a call back passes a value held elsewhere is treated as held elsewhere on every call. A call of
another package's definition holds its first `space` bound, but only when each argument that is not
a number or function is held alone. `List::map`, `List::filter` and `List::fold`, which state no
bound, hold what their function holds once per element, as `Map::fold`, `iterate`, `Map::update`
and `Bytes::position` do per call, and `map` and `filter` also hold a list as long as their
input. A `space` clause on a definition that performs anything other than a raise is `E0463`.
Anything else leaves the bound `unproved` (`E0474`), stopped at `unbounded` with the reason.

**Review.** `ply review` reports, for each definition changed since the last `ply review --accept`,
whether its implementation, its spec and its claims changed. The baseline is keyed by name.

## 11. Derivation

`derive d for T` calls `d::derive(declaration)` of the module the file imports as `d`, before the
rest of the module is read. Its result is source code for the functions it derives, read into the
module as if written there:

```ply
import std.json

pub type Line = { sku: String, qty: Int, unit_price: Decimal }
derive json for Line
// line_json : () -> json::JsonCodec<Line>
```

The toolchain ships these derivers, each documented in its module:

| deriver | module | generates | type |
| --- | --- | --- | --- |
| `json` | `std.json` | `<snake_case(T)>_json` | `std.json.JsonCodec<T>` |
| `bin` | `std.bin` | `<snake_case(T)>_bin` | `std.bin.BinCodec<T>` |
| `sql` | `std.sql` | `<snake_case(T)>_row` | `std.sql.RowCodec<T>` |
| `map` | `std.derive.map` | `T::map` | the value with each type parameter's values mapped |

`snake_case` is `std.text`'s, so two types that give the same name (`HTTPRequest` and `HttpRequest`
both give `http_request`) are `E0105`. A `derive` must be in the module that declares its type
(`E0208`).

A codec of a parameterized type takes one codec per type parameter, named as the parameter is:

```ply
pub type Box<a> = { label: String, inner: a }
derive json for Box
// box_json : <a>(JsonCodec<a>) -> JsonCodec<Box<a>>

fn encode<a>(b: Box<a>, c: json::JsonCodec<a>) -> String =
  json::encode_string(b, box_json(c))
```

Codecs are plain values: `json::decode_bytes(body, order_json())`. A `new` record (§4.2) derives like
a plain record with its fields: the same documents, bytes and shape. A codec for a type that
contains itself, directly or through the types its module derives, takes a nesting limit and passes
one less to each level, so the recursion visibly ends (§5.10). Beyond the limit, decoding is an
error and encoding panics (`ply doc std.derive.codec`).

**What cannot be derived.** A refusal is `E0206` at the field or variant that blocks the derivation:

- a function, a `Task`, a `Chan` or a `Secret`, which no codec writes;
- an `Option<Unit>` or an `Option<Option<a>>`, whose `None` and `Some` would be one JSON document;
- a type that takes a label (§4.5).

A field whose type's module derives no codec is `E0206` at the name the generated body cannot see. A
type with a `key` or `show` (§4.4) is encoded by its fields as they are.

`==`, `compare`, `digest` and `show` need no derivation: they work on any value its type allows.
`where derivable(eq, a)`, `derivable(ord, a)`, `derivable(hash, a)` and `derivable(show, a)` require
a type parameter to support them. `where derivable(D, p)` goes after the row and before any
`requires`.

**SQL rows.** `sql` maps a record to a row of a SQL table, one column per field named after the
field, read and written in field declaration order:

```ply
import std.sql
import std.json

pub type Line = { sku: String, qty: Int }
pub type Order = { id: Int, customer: String, note: Option<String>, lines: List<Line> }
derive json for Line
derive sql for Order
// order_row : () -> sql::RowCodec<Order>
```

- A `String`, `Int`, `Bool`, `Float`, `Decimal` or `Bytes` field is a column of that type. An
  `Option` is a nullable column, a `List` of those is an array, and a `json::Json` is the document
  it holds.
- Any other field is a `jsonb` document read and written with its type's own `json` codec, which the
  module must import `std.json` for and derive.
- A sum, a type with parameters, and an empty record have no row (`E0206`).
- The codec's `fields` are what the SQL check compares a statement's columns against, when `std.db`'s
  `fetch` reads rows with it (§16).

**Writing a deriver.**

- A deriver is any module's `pub fn derive(declaration: derive::Declaration) -> Result<String,
  derive::Refused>`, performing nothing but raises. `std.derive` describes a declaration.
- The checker compiles a deriver once and runs it for each declaration in isolation, with a limit
  of 60 seconds of wall clock. Each result is cached under the declaration and the source of the
  modules the deriver reaches, so it reruns only after an edit to one of them. A run that raised or
  ran out of time is not cached.
- A deriver is a standard library module or one of the program's own, and a deriver's own modules
  derive nothing through a deriver.
- A name no import binds is `E0207`. A module whose `derive` is not a deriver is `E0160`. A deriver
  that raises, runs out of time, returns anything other than functions, or writes a definition
  that does not check is `E0155`.

`std.derive.map` writes `T::map` for a sum or a `new` record: the value rebuilt with each type
parameter's values replaced by what its function returns, calling the functions in the order the
value holds what they read (`ply doc std.derive.map`).

## 12. Builtins

Builtins are the functions every module can call without an import. They are all declared in
`std.prelude`, the standard library module every module reads automatically, each with a doc
comment.

- An `extern fn` is a signature, with a row and a `where` like any other, whose body the runtime
  implements.
- A `pub fn` in the prelude is written in Ply: `List::at`, `List::push`, `List::set`, `List::map`,
  `List::filter`, `List::fold`, and `Map`'s functions except `Map::new` and `Map::len`. A call reads
  these by their signatures and clauses, as it reads an `extern fn`, so `List::set`'s `requires` is
  checked where it is called (§6.8).
- In a `cost` bound (§10), a call of a native function counts as one step, and a function written
  in Ply takes the steps its body takes.
- Comparing (`==`, `!=`, `compare`, `compare_values`), hashing (`digest`) or reflecting (`reflect`,
  `shown`) a list or map uses `List::at`, `List::push` and `Map::entries`, so a definition that does
  so depends on them and its hash covers them.

Only standard library modules may declare an `extern fn` or an `extern type` (`E0151`). An `extern
type`, such as `extern type ListNode<a>`, is a type the runtime lays out, with parameters but no
constructors or fields. Its values come only from its module's `extern fn`s. `==` on a value that
holds one is `E0201`, as on a function, and no deriver reads one, so `compare`, `digest`, `show`, a
`Map` key and a `derive` over it are `E0206`. Modules under `std.internal` are shared within the
standard library, and only standard library modules may import them (`E0159`). A program is compiled
only against a runtime that implements every native it calls in the way the compiler expects
(`E0476`, `E0481`).

Most builtins are functions of a built-in type, declared under it (`extern fn List::len`) and called
on a value or through the type like any function of a type (§5.4): `xs.len()`, `Map::new()`. No
module may declare one of those again, so nothing can hide one. The rest have plain names (`range`,
`min`, `panic`), which a module may shadow, except `compare_values` and the six functions the
wrapping and saturating operators call (§2.4), which no binding around such an operator may hide
(`E0105`). `to_int` reads any integer type as an `Int`, returning `None` if it does not fit, so with
`numeric_of_int` it converts between any two integer types.

`ply doc std.prelude` lists every builtin, natives first, each with its summary. `ply doc List::len`
shows a native: its signature with parameter names, its doc and its clauses. `ply doc List::map`
shows one written in Ply like any definition, with its location and hash. `ply doc List` lists the
functions of a built-in type: the natives, then those `std.prelude` and the type's home module
declare (§13).

A builtin whose row names `abort.raise` may state a `requires`: a condition on its arguments under
which a call cannot raise. For most builtins it describes exactly when a call cannot raise.
`Decimal::round`'s describes the part arithmetic can express; beyond it, a call raises only when the
digits it adds do not fit. A call that satisfies the `requires` adds no `abort.raise` (§6.8). An
`ensures` describes the result, as `result`, which is what the checker knows about the value when
it reads a later clause. A builtin with no `requires` raises for reasons its arguments' values
cannot express in arithmetic: `panic`, a failed `assert`, `String::of_bytes` of invalid UTF-8,
`String::find` of an absent substring, `iterate` past its budget, `Int::of_u64` of a value too large
for an `Int`.

```
   Bytes::at(b: Bytes, i: Int) -> Int / {abort.raise}
      `0..=255`; raises out of range.
      requires 0 <= i && i < Bytes::len(b)
      ensures 0 <= result && result <= 255
```

## 13. The standard library

The standard library is the built-in package `std`, carried inside `ply` and available to every
program without declaring a dependency: `import std.<name>`, with more parts for nested modules
(`std.hash.legacy` next to `std.hash`).

Its tests and claims belong to the library, not to your project: `ply test`, `ply prove`, `ply
review`, `ply check` and `ply defs` on a project leave them out. In the repository that builds
`ply`, the library's directory is a project of its own (§3.3): `ply test packages/std` runs every
module's tests, and `ply check packages/std` reports each module's errors, warnings and definitions.

**Home modules.** A module named after a built-in type is that type's *home*: only it and
`std.prelude` (§12) may declare functions of the type (`fn List::name` in `std.list`, §3.1). Any
module can call them without an import, on a value (`xs.sort()`) or through the type
(`List::sort(xs)`). The homes are `std.list`, `std.string`, `std.map`, `std.option`, `std.result`,
`std.char`, `std.bytes` and `std.float`. A program loads a home module only if it calls a name the
module declares.

A few other names need no import, wherever a module does not declare or import its own: `print`,
`eprint`, `read_line` and `Console` from `std.console`, `Set` from `std.set`, and `abs` from
`std.math`.

**Documentation.** Each module is documented in its source.

- `ply std` lists the modules, each with its summary.
- `ply doc std.json` documents a module and everything it exports; `ply doc std.json.parse` one
  definition; `ply doc fs.read_at` one operation of an effect; and `ply doc` of a type what it is
  made of and its functions (§3.1), each of which, like `ply doc Stack::size`, has its own page.
- `ply std --show std.json` prints one module's source, and `ply std --show` alone prints all of
  them.
- A module may embed (§3.4) data files that `ply` carries next to it, each named by its path inside
  the library (`std/oid/names.txt`). `ply std` lists them with the module that embeds each, and
  `ply std --show std/oid/names.txt` prints one. The digest `ply std` prints covers them.

A program loads only the standard library modules it imports, what those import, and what they
embed, so a change to any other module does not affect it. From those, it checks and hashes only
the functions the program reaches and the names it imports, plus every type and effect, the
functions a type's `key`, `show`, `invariant` or `numeric` names (§4.4), and each effect's real
handler (§6.10). It never reads the library's tests or laws. A function the program reaches comes
with what its specifications name (§10), which a dependent's proof may need.

## 14. The host boundary

The *host* is the machine a program runs on. Effects that reach outside the program (files,
network, processes, the console) are served by host handlers. Every run sets up the host: `ply
run`, `ply test` and `ply hosts` alike. Flags grant only things that name a place or a power: file
system roots (`--fs`), programs (`--exec`) and privileged families of operations (`--allow`).
Everything else is open by default.

**Real handlers.** An effect whose module declares a real handler (§6.10) never reaches the host
boundary directly: every root whose row names it runs under that handler, whatever the flags. The
operations of `std.os`, `std.env`, `std.certgen`, `std.config`, `std.password`'s `kdf`, `std.dns`,
`std.net`, `std.udp`, `std.sqlite`, `std.term`, `std.signal`, `std.trace`, `std.time`,
`std.random`'s `entropy`, `std.process` and `std.fs` work this way. A native function
such a handler calls refuses to run inside a `simulate` region, and in a test the search reruns, as a
host operation does (`E0425`). A native that would block the program's thread, such as hashing, hands
the work to the runtime's own threads (as many at once as the machine has cores, the rest queued),
while the task that asked waits and the region's other tasks run. A test that only computes a
function of its inputs on the machine, like a hash, is cached like any other. A test that read the
machine is cached with the permissions it ran with (§8.2).

**Tasks.** `task` is handled by the program's own top-level scheduler: every root whose row names it
runs as the first task, and the root finishes once every task it spawned has ended. Ready tasks run
first in, first out. A task that sleeps or waits on the host waits while the others run; code outside
a task, such as a `parallel` branch, blocks its thread. A raise no clause in a task handles fails the
run. A `simulate` inside a task runs as anywhere else (§9).

Outside a `simulate` region, `clock` is handled by its real handler,
`std.internal.schedule.clocked`, which every root whose row names it runs under. Tasks run in turn on the thread that
spawned them, and one that waits (on a join, a channel, a host operation or a sleep) is set aside
while the others run. `clock.now` reads a monotonic clock, in nanoseconds since the run began, and
`clock.sleep` waits until a deadline on it. A cancel stops a sleep, as in a region (§9), so a
deadline one task sets for another fires while the rest of the program keeps working.
`std.time`'s `time.sleep_ms` blocks the whole thread instead, and every task waits with it.

**What is open by default.**

- The environment: `std.env` reads the process's variables and working directory, which a test
  sets with `env::on`.
- Configuration: `config.get` and `config.secret` read from `--set`, `--config`, the environment
  and the schema's defaults, under both `ply run` and `ply test`. `config` is `nondet`, so a test that
  reads it must be a `test/nondet` (§8.3).
- The network: a program can look up names, connect and listen. Each socket belongs to the run, under
  the label its operation names. A TLS listener or upgrade uses the credentials `--tls` gives; a run
  given none has none (`E0429`).
- The console: `std.term` uses the terminal the process's streams are connected to, and the run
  restores the terminal mode it found.

**Signals.** `ply run` listens for `SIGINT` and `SIGTERM` when its root uses `std.signal` or
accepts connections (`net.accept`), and drains as the flags below say. When its root starts child
processes (`process.start` or `process.spawn`) and drains nothing, a stop signal ends the children
and exits at once, with status 130 or 143, so no child outlives it. Ctrl-C ends any other program at
once. A run that listens for no stop signal, such as a test, installs no signal handler.

**Other services.** `std.trace` writes to the sink `--trace` names, and checks that each span is
closed (`E0445`). A database is opened under a root given with `--fs`, so a run with no roots opens none
(`E0451`).

**Tests and the host.** A test that can reach a nondeterministic handler is cached with what it read
and the permissions it ran with (§8.2), so its pass never stands for a run given other roots,
programs or families. A test that reaches only deterministic handlers, whose results depend only on
their inputs, is cached like any other. An operation inside a `simulate` region reaches no host
handler at all (`E0425`, §9), since the region runs once per interleaving. `std.process` (and
`std.console`, built on it) is the run's own process under `ply run`. `ply test` withholds it
(`E0424`, naming the handler that serves it under `ply run`), since a test is not a process. The
exception: a test may use `process.bound`, `process.spawn`, `process.start` and the operations on a
started child, which reach only the programs `--exec` names.

**`ply hosts`** lists every operation the program can perform on the host (`effect.op[resource]`:
one row per operation and label some row names, where a written mode atom names every operation of
its mode). For each it shows the handler, whether it is deterministic, whether it is `at-most-once`
or `repeatable`, whether it blocks, and whether it may receive a `Secret`. It also shows the run's
TLS, file system, database, configuration, tracing and shutdown settings. `--digest` prints one `b3:`
line. An effect with a real handler (§6.10) is listed under that handler's name: it is deterministic
unless the effect is `nondet`; an operation is `at-most-once` when it is a `write`; it blocks when
the handler's clause waits on the runtime's threads; and it may receive a `Secret` when an argument's
type holds one.

| flag | meaning |
| --- | --- |
| `--tls NAME=CERT,KEY` | a TLS credential (PEM, leaf first; key in PKCS#8, PKCS#1 or SEC1); repeatable. Used as `net.listen_tls[l](port, "NAME")` and by `net.serve_tls`, which `std.http`'s `serve_pooled_tls` uses to offer `h2` and `http/1.1`. A name given several times holds several certificates, and each handshake gets the one matching the name the client asks for (SNI), or the first. Changed certificate files are reloaded within a second without a restart, keeping the old one if the new one fails to load. Sessions resume from tickets whose key rotates. `E0430` if it does not load, `E0429` if a program names one not given. |
| `--trust CERT.pem` | an extra trusted certificate; repeatable. `net.connect_tls` and `net.start_tls` accept servers it signs, in addition to the built-in roots. With any `--trust`, every TLS listener asks clients for a certificate, still serving a client without one unless `--mtls` names the credential. `net.peer_certificate` returns the certificate presented (on a client, the server's, for channel binding); `std.http` puts it on each request as `client_certificate`, and `std.x509` reads it. A client presents one when its connection was opened or set with `net::Presenting("NAME")` (a `--tls` credential), as `std.http`'s `fetch` does for `Fetching.credential`. `net.listen_tls` and `net.connect_tls` offer only `http/1.1`; to offer other protocols (`h2`), use `net.serve_tls` or `net.start_tls`, and `net.protocol` returns what the handshake agreed. `E0430` if it does not parse. The `ply` command's own connections use `PLY_TRUST` instead (§16). |
| `--mtls NAME` | a `--tls` credential whose listeners require a client certificate; repeatable. A client without one that `--trust` verifies is refused at the handshake. `E0429` if no `--tls` names it, `E0430` without a `--trust`. |
| `--fs NAME=PATH` | a file system root; repeatable. `std.fs` files and `std.sqlite` databases under that label live below it, as does a Unix socket `net.listen_unix` or `net.connect_unix` names. `E0454` if it is not a directory. |
| `--exec NAME=PATH` | a program that a `process.spawn` or `process.start` with that label may start; repeatable (`ply run`, `ply test`). `E0457` if it cannot be executed. |
| `--allow NAME` | a privileged family of operations granted to the program, which must declare the family's effect; repeatable (`ply run`, `ply test`). `machine` (`std.machine`'s `machine`) opens a compiled program, gives it host access from what this run was granted, and runs one of its entry points. `shipped` (declared in `compiler.unit`) reads the standard library `ply` carries. `toolchain` (`compilers`, `toolchain` and the nondeterministic `memory`, declared in `compiler.cc`) compiles and links C with the host's compiler and reads the program's own heap. `E0459` if the program does not declare it, or if a run that was not granted `machine` performs one of its operations that uses the host, measures, or times an entry. `machine`'s deterministic half, `hermetic_machine`, uses only what it is given (no host, no clock) and needs no grant, so a test that drives a program through it stays cached. |
| `--set KEY=VALUE` | a configuration value; repeatable, highest precedence |
| `--config PATH` | a file of `KEY=VALUE` lines; repeatable, above the environment |
| `--config-schema MODULE.FN` | a `ConfigSpec`: a missing key is `E0441`, a bad value `E0442`, an undeclared key `W0607` |
| `--trace json\|text\|off` | the trace sink: JSON lines on stderr (default), text, or nothing |
| `--trace-level debug\|info\|warn\|error` | the lowest level written (default `info`) |
| `--drain-lead-ms MS` | after `SIGINT`/`SIGTERM`, keep accepting connections this long (default 0), in a run that drains |
| `--drain-ms MS` | then let in-flight requests finish for this long (default 30000); running out is `W0608`, exit 3 |

When the stop signal arrives, listeners are closed, so a waiting `net.accept` returns `0`. That is
how `std.http`'s `serve_pooled` drains: it accepts nothing more, answers every connection it had
already accepted (queued ones included) with `Connection: close` on the last response, and closes
idle connections at once. An HTTP/2 connection (detected by its preface, or by the `h2` its TLS
handshake agreed under `serve_pooled_tls`) is drained with `GOAWAY`: the streams the client opened
before seeing it are answered, then the connection closes. Before a stop, the server sheds load,
answering `503` with `Retry-After` to a connection that finds its queue full or that waited in it
too long.

An unreadable configuration source is `E0440`. `std.db` refuses statements the database server
rejects, and statements that touch a table outside what the call site labelled.

## 15. Building and shipping

```
$ ply build . --entry app.serve -o app.plyx
$ ply build . --digest              # print `b3:...`; writes no file
$ ply build . --diff old.plyx       # added, changed, removed, unchanged
$ ply run app.plyx
```

**Programs.** `ply build` writes everything one entry point (default `main`) needs as a `.plyx` file
(default `<entry module>.plyx`). It contains:

- the program's definitions, by hash;
- those definitions printed back to source, without tests, laws, comments or anything unreachable;
- what `ply run` loads: that source, checked again, and its compiled code. So running a `.plyx`
  checks nothing.

What is included is what each definition refers to. An effect or type with the same name as a
function is a separate definition, so a program that names only the effect includes nothing the
function calls.

The BLAKE3 digest covers all of this and the entry point, so an edit nothing reaches leaves it
unchanged. A failure in a built program has no line numbers. A `.plyx` whose parts disagree (a body
under a hash that does not match, a name with no body, a closure or compiled program that is not the
one these definitions make) is `E0443`, as is a build containing two identical declarations it
cannot tell apart (two effects, or two members of one recursive group). An artifact built by another
compiler, or for another runtime, is `E0444`: rebuild it with this `ply`. A `.plyx` carries the
standard library definitions it uses, so a `ply` with a different standard library runs it as it was
built. `--diff` shows what a rebuild would change. `--config-schema` includes that function, resolved
as a run resolves it; a name that is not a pure function with no parameters returning a
`ConfigSpec` is `E0440`.

**Libraries.** A package whose manifest names no entry, and whose modules declare no `main`, is a
library. `ply build` writes it as a `.plyz` file (default `<name>.plyz`; `-o FILE` to choose). It
holds every module's source, the `ply.pkg`, and compiled code for every definition (a library has no
entry to prune from, so nothing is left out). A consumer compiles the sources. A library is not a
program: `ply run lib.plyz` refuses it (`E0443`).

### 15.1 The registry

Libraries are published to a registry and depended on from it (§3.3). A registry is a directory of
files behind an HTTP server:

```
GET  /<name>/index.json                        every version of <name>, newest last
GET  /<name>/<version>/package.plyz            the library's `.plyz`, as `ply build` writes it
GET  /<name>/<version>/package.plyz.b3         its digest, one `b3:<hex>` line
GET  /<name>/<version>/interface/<semantics>   the interface its publisher produced under <semantics>
GET  /<name>/<version>/attestation/<semantics> what the registry's attester found
PUT  /<name>/<version>                         publish: the `.plyz` as the body
PUT  /<name>/<version>/interface/<semantics>   the interface
PUT  /<name>/<version>/attestation/<semantics> an attestation signed by the attester's key
POST /<name>/<version>/yank                    mark the version yanked
```

`index.json` is `{"name": .., "versions": [{"version": "0.2.0", "digest": "b3:..", "yanked":
false, "runtime": {"major": .., "minor": .., "patch": ..}}]}`, in publication order, where `runtime`
is the toolchain the version's manifest declares. The index is only advisory: `ply resolve` checks
every archive against it and against the lock, so a stale or hostile index can cause a refusal but
never a wrong build. A version's identity is the digest `ply build` gives its `.plyz`: BLAKE3, over
the same parts a `.plyx` digest covers, written out in full.

**Publishing.** `ply publish [path]` builds the library's `.plyz` exactly as `ply build` does and
sends it to the registry `PLY_REGISTRY` names, with the token in `PLY_REGISTRY_TOKEN` as
`Authorization: Bearer <token>` and the digest as `X-Ply-Digest: b3:<hex>`.

- Only a library can be published, and only one whose dependencies all come from the registry, since
  its users resolve them from the registry alone. A program, the anonymous package, or a path or git
  dependency is `E0145`, as is a `ply yank` of a name or version the registry does not have.
- The registry recomputes the digest from the body and refuses a mismatch. It refuses different bytes
  under a version it already lists (a published version never changes; publish a new version), but
  accepts the same bytes again. It refuses an archive whose manifest is not the package and version
  it was sent as, names an entry point, or depends on anything but the registry. Each refusal is
  `E0144` with the registry's reason.

**Semantic versioning is checked.** `ply publish` compares the contract of every public definition
(its signature and specs, and its body if it is `transparent`; a type's or effect's whole
declaration) with the highest non-yanked version published below it. A patch release may change no
contract, a minor release may only add definitions, and a major release may change or remove
anything. A version that bumps less than its changes need is `E0150`, naming what changed and the
smallest version that would be correct, before anything is sent. `0.x` versions follow the same
rules. `ply contracts NAME FROM TO` lists the contracts that changed between two published versions.

**Interfaces.** After the archive, `ply publish` sends the package's *interface*: what each of its
modules shows to a module that imports it (each name's signature, effects, counts and hashes),
together with the *semantics version* of the `ply` that produced it and the archive's digest. The
registry keeps one interface per version and semantics version, and refuses one that names another
archive or semantics version; the same bytes again are accepted, so a publish whose interface was
refused can be rerun whole. The semantics version identifies what a definition's hashes, checked
rows and claim outcomes mean, and changes only when one of them does; so a `ply` that changes none
of them can use an interface another produced. `ply publish --json` reports it. `ply resolve`
downloads the interface with each archive. A load checks a dependency's modules from their source,
and `--verify-deps` (on `check`, `test` and `prove`) refuses a dependency whose interface does not
match its source (`E0149`).

**Attestations.** A registry with an *attester* vouches for what it serves. `ply attest NAME
VERSION` sets the published version up as a project of its own, downloaded and checked as `ply
resolve` would, and runs `ply test` and `ply prove` on it. It reports whether it type-checks,
whether its `reuse fn` and `returns` promises hold, its test results, and whether each claim is
proved, under this `ply`'s semantics version.

- With `--sign KEY`, the result (a `std.pkg.Attestation`) is signed as §15.2 describes and sent to
  the registry, which keeps it only if its configured attester's public key signed it, for that
  version's archive and semantics version. Without `--sign` nothing is sent, so anyone can rerun an
  attestation.
- A registry run with `--set attester=<public key hex>`, `--set attest.key=<secret key file>`,
  `--set url=<the registry's URL as the attester reaches it>` and `--exec attest=<a ply>` attests
  every published version, one at a time between connections, with that `ply`.
- A version whose tests fail or whose claims are not proved is still published, and its attestation
  says so.
- `ply resolve` downloads each dependency's attestation and trusts it only if a key listed in
  `PLY_ATTESTERS` (public key files, separated as `PATH` separates directories) signed it for the
  archive the lock pins. `--json` reports each as `attestation` (`attested`, `trusted`, `signer`, and
  what was run).

**Yanking.** `ply yank NAME VERSION` marks a version yanked, with the same token. New resolutions
skip it, a lock that pins it keeps it, and its archive is still served. Nothing is ever deleted.

**Connecting.** `PLY_REGISTRY` is one base URL, `https://host[:port][/prefix]`. The client verifies
the server against the built-in roots and the certificates `PLY_TRUST` names (§16), so for a
registry signed by a private CA, point `PLY_TRUST` at the CA's certificate. A failed handshake is
`E0141`. `http://` is accepted only for a registry on the same machine (`localhost`, `127.x.x.x`,
`[::1]`), because a publish sends a token.

**Running a registry.** The registry is a Ply program, `packages/registry`:

```
$ ply run packages/registry --fs store=/srv/ply \
    --tls registry=cert.pem,key.pem --set tls=registry --set port=8443 \
    --config tokens.conf
```

- `store` is the directory tree above.
- `port` is where it listens (default `8080`).
- `tls` names the `--tls` credential to serve with. Without it, the registry serves plain HTTP, for
  a proxy that terminates TLS in front of it or a registry on the same machine.
- Each package's publishing token is the configuration key `token.<name>`, one exact name per key,
  so a `--config` file of `token.orders=...` lines fully defines who may publish what.
- It serves one connection at a time, and locks a package around every write, so two registries
  over one store never interleave writes.
- Like every Ply listener, it binds only `127.0.0.1`. Other machines reach it through a proxy that
  either passes TLS through to a `--tls` registry or terminates TLS for a plain one.

### 15.2 Signing

```
$ ply keygen release.key                       # release.key and release.key.pub
$ ply build . -o app.plyx --sign release.key   # app.plyx and app.plyx.sig
$ ply run app.plyx --require-signer release.key.pub
$ ply build . -o app.plyx --verify             # compare, write nothing
```

`ply keygen PATH` writes an Ed25519 key pair (`std.ed25519`): the secret key at `PATH`, readable
only by its owner, and the public key at `PATH.pub`, each one line naming what it holds followed by
64 hex digits. It never overwrites a file. A key file that cannot be read, decoded or written is
`E0462`. The secret key is a `Secret` (§4.6) from the moment it is generated, and is read back as
one.

`ply build --sign KEY` signs what it writes, program or library, in `<artifact>.sig` next to it.
Signatures are detached, so the artifact's digest is the same whoever signs it, and signing again
with another key adds a second signature. What is signed is the artifact's provenance: its kind and
name, its full digest, the `ply` that built it, the semantics version (§15.1), and the git commit of
its sources, if any.

`ply run ARTIFACT --require-signer KEY` (repeatable) runs a built artifact only if one of the named
public keys signed its provenance for the artifact's own digest. No signatures, signatures for other
bytes, or none by a trusted key is `E0460`, before anything is loaded.

`ply build --verify` builds in memory and checks the file `-o` names against what these sources
build, and every signature next to it against that build. It writes nothing and reports which keys
signed. A file that is not that build, or a signature that does not verify, is `E0461`.

## 16. The `ply` command

`ply [--color auto|always|never] <command> [path] [options]`

**Color.** `--color` applies to every command and overrides the environment. `auto` decides as
`std.term`'s `decide` does (`ply doc std.term`), counting a variable as set when it is non-empty:
color if `FORCE_COLOR` is set; otherwise none if `NO_COLOR` is set; otherwise color if
`CLICOLOR_FORCE` is set; otherwise color only on a terminal whose `TERM` is not `dumb`.

**JSON.** Every command takes `--json`, and then prints exactly one compact JSON object on stdout,
with sorted keys.

**Environment.** The command reads `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`, `TERM`, `COLORTERM`,
`PLY_CACHE_UPSTREAM` (§1), `PLY_REGISTRY` and `PLY_REGISTRY_TOKEN` (§15.1), the `PLY_C_*` variables
(§8.6), and `PLY_TRUST`: PEM files, separated by colons, whose certificates the command's own HTTPS
connections (to a registry, for `ply publish`, `ply yank` and `ply resolve`) accept in addition to
the built-in roots, as `--trust` does for a program. A file in `PLY_TRUST` that does not load is
`E0430`, before the command runs.

**Exit status.**

| exit | meaning |
| --- | --- |
| 0 | success |
| 1 | a test failed, or `main` raised |
| 2 | the program did not run: bad path, syntax error or type error |
| 3 | the drain deadline passed with requests still in flight |
| *n* | `process.exit[p](n)` under `ply run`: the program's own status, `0` to `125` |
| 130, 143 | `SIGINT` or `SIGTERM` ended a `ply run`: immediately if the run ends its children at once, or after draining (§14) |
| 141 | the reader of stdout or stderr went away, as when output is piped into `head`: nothing more is written, by `ply` or by a program under `ply run` that left `pipe.broken` unhandled (`ply doc std.process`) |

**Paths.** A relative path is resolved against the working directory. In the table below, `[path]`
is a `.ply` file or a directory; left out, it is the package that contains the working directory.
`ply fmt` rewrites exactly the paths it is given, and `ply new`, `ply bootstrap` and `ply work` take
their paths as written. `ply doc` works outside any package for builtins and the standard library.

**Flag groups.** *simulation* (§9); *host* (the flags in §14 other than trace and drain); *prove*
(`--prove-budget`, `--timeout`, §10); *trace* (`--trace`, `--trace-level`); *drain* (`--drain-ms`,
`--drain-lead-ms`).

| command | flags |
| --- | --- |
| `ply new PATH` | `--name NAME` (default: the last part of the path), `--lib` (a library: one `pub` definition, no `main`). Refuses an invalid package name and an existing directory. |
| `ply check [path]` | `--types`, `--costs`, `--explain` (the checking phases and how much was reused from the cache; with `--types`, also effect sets, where each type came from, and each place a body can raise, §6.8), `--workspace`, `--verify-deps` |
| `ply test [path]` | `--filter`, `--shard`, `--jobs`/`-j`, `--timeout`, `--no-cache`, `--explain`, `--watch`, `--coverage`, `--mutate [DEF]`, `--mutate-budget`, `--profile`, `--workspace`, `--verify-deps`, host, simulation |
| `ply run [path] [-- ARGS]` | `--seed` (always one interleaving), `--timeout` (no limit by default, since a server runs forever), `--profile`, `--explain` (on standard error: whether checking was reused from an earlier run, the load's phases, the host operations the program may perform, and how it handles a stop signal), `--require-signer KEY` (repeatable; §15.2), host, trace, drain. `ARGS` is what `process.args` returns. A `.plyx` path runs the built artifact. |
| `ply prove [path]` | `--filter`, `--jobs`, `--no-cache`, `--no-incremental`, `--explain`, `--reach`, `--workspace`, `--verify-deps`, prove, simulation |
| `ply review [path]` | `--changed` (default), `--accept`, `--no-cache`, `--no-incremental`, prove, simulation |
| `ply build [path]` | `--entry NAME`, `-o FILE` (default `<entry module>.plyx` for a program, `<package>.plyz` for a library), `--config-schema`, `--digest`, `--diff OLD.plyx`, `--sign KEY` (signatures in `<FILE>.sig`), `--verify` (compare, write nothing; §15.2) |
| `ply hosts [path]` | host, trace, drain, `--digest` |
| `ply std` | lists the standard library's modules. `--show [NAME]` (a module's source, or a data file a module embeds), `--digest`. Used by `ply bootstrap`: `--answers DIR` (precompute every standard library module's check results into `DIR/pack`), `--answering` (print one line identifying how this `ply` checks the standard library; two `ply`s that print the same line produce the same results). No path. |
| `ply explain CODE` | one line on what a diagnostic code means; `--all` lists every code. No path. |
| `ply doc NAME [path]` | documentation for a full or unique simple name (§2.1). For a definition: its signature with parameter names, its doc, `returns` and spec clauses, location, hash, footprint, and the laws that mention it. For a law schema: the schema and the laws that use it. For a type: its fields or variants (not an opaque sum's) and its functions. For an effect: its operations (one operation is `effect.op`). Also effect sets, and modules with what they export. A name under `std` documents the standard library, the same everywhere. Any other name the program does not define is looked up among the builtins, then the standard library. Pages are read from cached check results, so no module is parsed to print one. |
| `ply fmt [paths]` | rewrites every `.ply` file under the paths (default: the package containing the working directory) in the standard layout. `--check` writes nothing and exits 1, naming the files that would change; `--json` reports the same and requires `--check`. |
| `ply rename --map FILE [paths]` | renames each old name in the map file to its new name, in the `.ply` files under the paths (default: the package containing the working directory). `--words` renames words inside names instead. `--check` writes nothing and exits 1, listing what would change; `--json` reports the same and requires `--check`. Exits 2 without writing anything if the renamed program would not check, or a name in it would refer to something else. See **Renaming** below. |
| `ply show NAME [path]` | prints one `fn` or `type` as its file has it: its doc and the comment lines above it, `pub`, the body, and a comment ending its last line. `--json` adds the byte range. |
| `ply replace NAME [path]` | rewrites one `fn` or `type` from `--with FILE` or stdin, formatted, leaving every other byte of the file alone. Refused with `E0128` (exit 2, nothing written) unless the program still checks and no other definition's name or hash changes. `--check` writes nothing. |
| `ply resolve [path]` | writes `ply.lock` from the manifests: each dependency's name, version, and the digest of each one not in this checkout (`in the checkout` for the rest; `"digest": null` in `--json`). Parses no modules. The only command that downloads registry dependencies, from `PLY_REGISTRY`, with their interface and attestation where published (`interface` and `attestation` in `--json`; attestations trusted as `PLY_ATTESTERS` says, §15.1). |
| `ply vendor [path]` | copies every dependency into `vendor/`, one directory per package plus an index, so the project builds with no cache and no network |
| `ply why NAME [path]` | why a package is a dependency: the chain from the root package to it, then the version and digest pinned |
| `ply publish [path]` | builds this library's `.plyz` and uploads it, then its interface, to `PLY_REGISTRY` with `PLY_REGISTRY_TOKEN`, once its version is the bump its changes need (§15.1) |
| `ply yank NAME VERSION` | marks a published version yanked, with `PLY_REGISTRY_TOKEN`. No path. |
| `ply attest NAME VERSION` | runs a published version's tests, claims and promises and reports the result; `--sign KEY` signs it and sends it to the registry (§15.1). No path. |
| `ply keygen PATH` | writes an Ed25519 key pair: the secret key at `PATH`, the public key at `PATH.pub` (§15.2). No path. |
| `ply contracts NAME FROM TO` | the public definitions whose contracts were added, changed or removed between two published versions, the bump that requires, and whether `TO` makes it (`needs`, `kept`). No path. |
| `ply hash [path]` | `--deps` (references and everything reachable from each) |
| `ply defs [path]` | every definition: location, hash, signature, footprint and references; `--filter SUBSTRING` |
| `ply callers DEF [path]` | what refers to a definition directly, and every definition, test and law of your own modules that reaches it |
| `ply bootstrap [<checkout>]` | builds a native `ply` from a checkout (default `.`), using the parts its `ply.work` lists (§3.3). `--profile development\|release` (default `development`; both compile the C with `cc -O2`, and `development`'s checks its reference counts), `--runtime LIBRARY` (default: the runtime's static library in `target/debug`, or `target/release` under `--profile release`), `--out PATH` (default `ply` in that same directory). Reuses work from earlier bootstraps whenever what it depends on is unchanged. |
| `ply work [<path>]` | where the `ply.work` at or above a directory (default `.`) places each package and each part of `ply`: one role and path per line, or `--json` (§3.3) |
| `ply cache clear\|stats\|compact [path]` | empty the cache / report what it holds and how much space could be reclaimed / reclaim that space |
| `ply cache inspect <DEF> [path]` | what the cache holds for a definition, declaration or test, by full name, simple name or a hash prefix of 4 or more hex digits: its location, hash, type or form, footprint, the effects it names, its body's size, and whether a run proved a test at its hash |

**The first run.** The first run after `ply` or the program changes compiles the program, which
needs a C toolchain and takes a few seconds. Later runs reuse the compiled code.

**Reusing earlier answers.** `ply check`, `ply test`, `ply run` and `ply defs` remember their last
answer for the same command line. When nothing they read has changed since, they return it without
loading the program; when something changed but every module still checks the same, they also
reuse it.

- `ply run --explain` says `reused` or `built` on stderr before the program starts, with timings
  (`front_end` in `--json`). A reused run still sets up permissions (`--allow`, `--exec`, `--fs`) and
  picks its entry point fresh.
- `ply check --explain` always checks again, since what it reports is about this run.
- `ply test` reuses an answer only while every pass it relied on is still recorded. `--watch`,
  `--explain`, `--coverage`, `--mutate`, `--no-cache`, and a configuration file or schema always
  load the program.
- Under `--json`, `ply check`, `ply test` and `ply prove` report under `front_end` whether the answer
  was reused and, when the program was loaded, how many cached results each kind of check took and
  how many it recomputed.
- `ply build`, `ply hosts`, `ply test --no-cache` and `--no-incremental` check your modules from
  scratch and cache nothing. A cache that cannot be read means checking from scratch, never a
  failure.

**What runs before the program.** `ply check`, `ply run`, `ply test`, `ply build` and `ply prove` do
two things after checking and before anything else:

1. **Tagged literals** (§2.3). Each literal in the root package's modules is parsed by calling its
   parser, in isolation with no permissions, with a limit of 60 seconds of wall clock. A refusal
   is `E0153` or `E0154`. The verdict depends only on the parser and the text, so it is cached under
   the hash of everything the parser reaches and the text, and a parser runs again only for a new
   text or after an edit to something it reaches. A parser that runs out of time is not cached, since
   the clock says nothing about the parser. A dependency's literals are that package's to check. In
   a program loaded without this step, a literal its parser rejects ends the run where it is read
   (`E0502`).
2. **SQL statements.** The statements the root package's modules run are checked against the schema
   (§3.5) their module reaches: each `db.query`, `db.batch`, `db.execute`, `db.returning` and
   `db.copy_out`; `std.db`'s `fetch` and `fetch_returning`; and `std.sqlite`'s `query`, `execute`,
   `returning`, `run` and `fold_rows`. A statement is checked when it can be computed before the
   program runs: text (joined by `++` or not), a call of a definition on such values,
   `sqlite::bound` of one, or a tagged literal. Each definition a statement is computed by runs in
   isolation with the literals' time limit; one that raises or runs out of time leaves the statement
   to be checked at run time. Errors are reported at the call, with the statement, where it came from, and the schema:
   - `E0431`: text the SQL parser rejects.
   - `E0435`: the statement's tables do not include the table the call's label names.
   - `E0436`: the operation does not match the statement: a write run as a query, a read (or a
     statement with `returning`) run as `execute`, or one without `returning` run as `returning`.
   - `E0438`: the number of parameters, written as a list, differs from the placeholders; or a
     parameter written as a `Param` constructor is not the type of the column it is compared with,
     stored in or bounds.
   - `E0433`: a table or column the schema does not declare.
   - `E0447`: a `*`.
   - `E0470`: a construct the check does not follow (`with`, a set operation, a subquery in `from`,
     a window function, `on conflict`), reported where the check stops.
   - `E0437`: the columns a `fetch` returns do not match the fields of the row type its codec reads,
     when `derive sql` wrote it (§11): by name, type and nullability, where a `left join` makes the
     joined table's columns nullable.

   Results are cached under the hashes of everything the statement and schema reach. At run time,
   `std.db` applies the same rules to a statement the check could not compute, and raises.

**Formatting.** `ply fmt` keeps comments, the spelling of every literal, and the order of imports,
items and statements, and keeps each doc comment directly above what it documents. It prints
`formatted PATH` for each file it changed. A file that does not parse is left alone, and the command
exits 2 with the diagnostic. A line string starts on its own line, with each line at the first's
indentation, and takes no trailing comma as the last item of a list. A file it cannot read or write
is also an error (exit 2), so `--check` never passes a file it did not read. Directories whose names
start with `.`, and directories named `target`, are skipped. A symlink found while walking is
skipped; one named on the command line is an error.

**Renaming.** `ply rename` replaces names in many files at once. The map file has one rename per
line: the old name, a space, and the new name.

```
held_count cached_count
Expr Expression
```

`ply rename` reads the program the way the compiler does. Each definition, local variable, effect
operation, record field and module named by an old name is renamed where it is declared and
wherever it is used, in every `.ply` file under the paths. A record field is renamed in every
record type that has it and at every use: records are structural, so a field belongs to no one
type. Then `ply rename` checks the renamed program, and refuses unless every name in it still
refers to what it did.

When several modules declare the same name, put the file that declares the one you mean first:

```
packages/cli/build.ply Made BuildOutput
```

This renames `Made` in that file, in files that import it by name (`import build (Made)`), and
where a file writes it after the module's name (`build::Made`). Other definitions named `Made`, and
locals and fields spelled `Made`, stay as they are. A function of a type, `Stack::size`, is renamed
by its own name (`size`), and follows its type when the type is renamed.

To rename a module, give its file, or its directory ending in `/`, and the new name:

```
packages/compiler/emit/answer.ply output
packages/compiler/emit/lowered/ tests
```

The file or directory moves, and every `import` of it changes to match, along with uses written
`answer::name`. Other names spelled `answer` stay as they are. A string that contains the module's
full name, such as `"compiler.emit.answer"`, is reported as a warning.

To rename an operation of an effect, write the effect and the operation joined by a dot, after the
file that declares the effect:

```
packages/std/fs.ply fs.read_file load_file
```

The operation is renamed inside the effect's declaration and wherever a file that imports from that
module writes `fs.read_file`: calls, effect lists in signatures, and handlers.

Types and values have separate names, so renaming a type leaves a variant or function spelled the
same alone. If a module's name changes, its file moves: renaming `held` to `cached` moves
`held/store.ply` to `cached/store.ply`.

A name inside backticks in a comment is renamed too, where the file renames every use of that name
the same way, so doc links stay correct. Text in strings is never changed. When a string contains a
renamed name, `ply rename` prints a warning with the line, so you can decide whether to change it by
hand.

With `--words`, each line renames a word inside names. Words are separated by `_` or by a capital
letter. The line `ty type` changes `ty_vars` to `type_vars`, `TyCon` to `TypeCon`, and `MAX_TY` to
`MAX_TYPE`.

`ply rename` changes nothing, and exits 2, if:

- a path's program does not check before the renames;
- a line names a file the paths do not hold, or a name that file does not declare;
- a new name is a keyword or not a valid name;
- a renamed name is declared with `extern`, so code outside Ply uses that name;
- a module's file would be moved onto a file that already exists, or two files onto one;
- the renamed program would not check, or a name in it would refer to something other than what it
  did: two definitions with one name, or a local that another local of its new name hides.

Only files under the paths are changed. A file of the same package outside the paths that uses a
renamed name stops the rename, because the renamed program would not check. Another package that
uses a renamed name is not checked; `ply check` reports it.

**Editing one definition.** `ply show NAME` and `ply replace NAME` read and rewrite one definition
without touching anything else in the file. The replacement is one item of the same kind and name,
with its own doc above it. `replace` formats it with `ply fmt` and puts it in the byte range `show`
reports.

**Runtime flags for measuring allocation.** These flags are read by the runtime of any native Ply
program, `ply` included, and removed from the command line before the program sees it:

- `--count-allocs=PATH` writes how many objects (`allocations`) and how many `bytes` the entry
  point allocated, across all threads.
- `--count-alloc-sites=PATH` also writes `sites`: the nearest few `ply_*` C functions each allocation
  came from, most frequent first. It samples one allocation in `sampled_every` and scales the counts
  back up, so rows are close to the totals but need not add up to them exactly. Allocations with no
  `ply_*` function on the stack are counted under `<no ply frame>`. Walking the stack is slow, so
  this is for measurement, not production.
- `--count-alloc-sites-exact=PATH` walks every allocation instead: exact, and slower.

## 17. Diagnostics

`E` codes are errors, and so is `X0001`, a character that cannot appear in source (§2.1). `W`
codes are warnings, which never mean your program is wrong. `ply explain CODE` prints a one-line
description of an `E` or `W` code, and `ply explain --all` lists them all.

Every command prints diagnostics in one format:

```
Error[E0201]: type mismatch: function body type
  --> app.ply:1:17
   | fn f() -> Int = "x"
   |                 ^^^ expected `Int`, found `String`
  = the body must answer the written type
```

The heading is the severity, the code and the message. Then comes one block per label: where it
points, the source line, and carets under the span (one caret for an empty span, never past the end
of the line the span starts on). A label whose span runs past the end of its file, or names no file,
is dropped, as it is under `--json`; one that splits a character is still placed on the line it
starts on. A diagnostic with no labels left is its heading and notes alone. Each note is a `  = `
line. Color never changes this layout: the heading and carets take the severity's color, and a pipe,
`NO_COLOR` or `--color never` gives exactly these bytes.

A diagnostic that knows its fix carries `fixes` under `--json`. Each fix has a `title` and `edits`;
an edit replaces the text between `start` and `end` of `file` (an empty range inserts) with `text`.
Applying the edits as given leaves a program where the diagnostic no longer applies. On a terminal,
a fix is shown as a `= fix:` line under the message.

A diagnostic the runtime raises, such as a failed assertion, writes each value it names in its text.
A long value is cut there, and `… N more` says how many characters, bytes or items were left out.
Under `--json` each value it names also comes whole, as `values` beside the text that names it.

## 18. What Ply does not have

These describe the language as it is today.

- No loops, `break` or `return`: use recursion (§5.7). `?` is the only early exit, and a raise the
  only one that passes the caller (§6.8).
- No mutable variables, except handler variables (§6.9).
- No exceptions outside the row: every failure is a `raise` operation in a row (§6.8).
- No typeclasses or implicits, and no dispatch at run time: a call on a value (§5.4) is chosen by
  its type where it is written.
- No modules as values, and no first-class effects.
- No `unsafe` and no foreign function interface: everything a program does outside itself is an
  effect a handler serves, and the builtins are the only functions the runtime implements (§12).
- Specs cannot refer to mutable state.
- Reference cycles are not collected.
- A task never moves between OS threads; only a `parallel` block's branches run on the runtime's own
  threads.
- No file handles: `fs` reads a range and appends by path, with nothing held open between calls.
- No backpressure.
- No database migrations or live schema checks.
- No HTTP/2 over a `listen_tls` listener, whose handshake offers only `http/1.1` (`std.http`'s
  `serve_pooled_tls` offers `h2` through `serve_tls`); no `Upgrade: h2c`; and no server push.
- No authentication framework.

Sharp edges:

- `x.f(y)` where `x` is not a variable is an effect operation, not a function call (§5.4).
- An operation nothing handles is found only when it reaches the host at run time (`E0424`), unless
  its effect is `nondet` and it is in a deterministic test (`E0412`).
- A record update needs the base's type to be known where it is written (§5.5).
- Two tasks that allocate are always ordered against each other (§9).
- `Bytes::at`, `Bytes::u32_le`, `String::slice`, `String::find`, `List::set`, `Array::get` and
  `Array::set` raise when out of range, whereas `List::at` and `Array::at` return `None`.

## 19. Examples

In `examples/`:

- `clock.ply`: a `nondet` effect, a handler, and `test/nondet`.
- `ledger.ply` and `report.ply`: modules, an `opaque` type, specs and laws.
- `pipeline.ply`, `bank.ply` and `timeout.ply`: simulation, a race and its fix, and a virtual clock.
- `echo.ply` and `hello.ply`: sockets and an HTTP endpoint.
- `orders.ply`: `derive json`.
- `relay.ply`: one forwarder that is generic over the label it writes under.
- `shout.ply`: a command line declared with `std.cli` and read into the program's own type.
- `store.ply`: a handler used to grant a capability.
- `agreement.ply` and `twin_divergence_audit.ply`: `std.db`'s in-memory twin checked against
  recorded PostgreSQL answers.
- `service.ply`: a service over PostgreSQL or its in-memory twin, whose store, TLS and accept loop
  are configuration, with tracing and graceful shutdown.
