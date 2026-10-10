# 3. Values and types

Ply is statically typed. Every expression has a type the compiler knows before the
program runs, and a mistake in a type is a diagnostic, never a surprise at run
time. This chapter walks through the values you can write down: numbers, text,
bytes, and the operators over them.

## Inference, and written signatures

Two things are true at once, and it is worth separating them:

- **Inside a body, types are inferred.** You rarely write a type for a local
  binding: `let n = 40 + 2` infers `Int`.
- **A top-level `fn` writes every parameter and return type.** This is checked
  (a missing one is `E0126`), and it is what lets a caller be checked against a
  signature without reading a body.

Ply is deliberately not a language where you sprinkle annotations to make things
compile. `ply check --types` prints what was inferred, and the diagnostics name
the types involved:

```ply
// main.ply
fn f() -> Int = 1.0
```

```text
Error[E0201]: type mismatch: function body type
  --> main.ply:1:17
   | fn f() -> Int = 1.0
   |                 ^^^ expected `Int`, found `Float`
   compilation failed (1 error)
```

That failure is the point: `Int` and `Float` are different types and Ply never
converts between them for you.

## Integers

`Int` is a 64-bit signed integer, and it is the type to count and index with.
Underscores group digits, and `0x` writes hex:

```ply
1_000_000     // Int
0xFF          // Int, 255
```

Fixed-width integers exist for data defined in a width and for exact answers
past `Int`. A suffix on the literal picks the type:

```ply
255u8   0x6A09_E667u32   -1i8
```

The widths are `u8 u16 u32 u64 u128 i8 i16 i32 i64 i128`. A decimal literal is
bounded by its range (`256u8` is `E0211`), while a hex literal is bounded by its
width, so `0xFFu8` is `255` and `0xFFFF_FFFF_FFFF_FFFF` is `-1`.

There is no numeric tower. Both operands of an arithmetic operator have one type,
and there is no implicit widening:

```ply
fn h(a: U32) -> U32 = a + 1u32     // checks
fn g(a: U32) -> U32 = a + 1        // E0201: cannot mix U32 and Int
```

When the operand type is not settled from the rest of the definition, you get
`E0210` rather than a default:

```ply
let f = |a, b| a + b;    // E0210: nothing says whether this is Int, Float, ...
```

Conversions are explicit builtins. `u32_of_int(n)` and its siblings raise if the
value does not fit; to truncate instead, mask first:

```ply
let low = u8_of_int(n & 0xFF);
```

`to_int(x)` reads any integer type as an `Int`, answering `Option<Int>` — `None`
rather than raising when the value is too large. `numeric_of_int(n)` writes an
integer at any numeric type.

### Arithmetic is checked

`/` and `%` raise on a zero divisor, which puts `abort.raise` in the enclosing
row unless the checker can see the divisor is not zero. Overflow and a shift
count that is negative or as wide as the type are not rows at all: they end the
run, because they are the machine's limit rather than a value your program chose.

Three operators and a family of builtins make the choice explicit:

- `+%`, `-%`, `*%` **wrap** at the operand's width: `255u8 +% 1u8` is `0u8`.
- `+|`, `-|`, `*|` **saturate**: `250u8 +| 10u8` is `255u8`, `3u8 -| 10u8` is `0u8`.
- `checked_add`, `checked_sub`, `checked_mul` and `checked_neg` answer `Option`,
  `None` where the answer would leave the type.

```ply
test "arithmetic edge cases are explicit" {
  assert_eq(255u8 +% 1u8, 0u8);
  assert_eq(250u8 +| 10u8, 255u8);
  assert_eq(checked_add(255u8, 1u8), None);
  assert_eq(checked_add(1u8, 1u8), Some(2u8))
}
```

The bit operators act at the type's own width: `~0u8` is `255u8`, `&`, `|`, `^`
are integer-only, `<<` discards shifted-out bits, `>>` is arithmetic and `>>>` is
logical. `-8 >> 1` is `-4`; `-8 >>> 1` is a large positive number.

## Floats and decimals

`Float` is IEEE-754 binary64. Equality is IEEE, so `NaN != NaN` and `0.0 / 0.0`
is a `NaN` rather than an error. Float functions answer the same bits on every
machine, which is what lets their results be cached and proved.

`Decimal` is exact base ten: a 96-bit mantissa and a scale from 0 to 28. It keeps
the scale it was written with, so `1.50m` and `1.5m` are equal and print
differently:

```ply
test "decimals are exact and keep their scale" {
  assert_eq(0.1 + 0.2 == 0.3, false);        // Float
  assert_eq(1.50m == 1.5m, true);            // Decimal
  assert_eq(decimal_scale(1.50m), 2);
  assert_eq(decimal_to_string(1.50m), "1.50")
}
```

`/` on a `Decimal` is `E0209`; use `decimal_div`, which takes the scale of its
answer along with a `Rounding`. That is deliberate: division has no exact answer
at every scale, so Ply makes the choice of scale and rounding visible in the
call.

## Booleans and `Unit`

`Bool` is `true` or `false`. `Unit` is the single value `()`; it is what a function
returns when it is called for its effect rather than its answer. `&&` and `||`
short-circuit.

## Characters

`Char` is one Unicode scalar value, written `'a'` or with an escape:

```ply
'\n'   '\''   '\u{1F600}'
```

`int_of_char('A')` is `65`. The reverse is `char_of_int(n) -> Option<Char>`, which
answers `None` for a surrogate or a value past `U+10FFFF`. Ordering on a `Char`
is by scalar value. Arithmetic on one is `E0201`; go through `int_of_char`.

## Strings and text

`String` is UTF-8, indexed and sliced by **character**, not byte. A literal is
double-quoted with escapes `\n \t \r \0 \\ \" \'` and `\u{...}`:

```ply
"hello"
"a tab:\tand a newline:\n"
```

`++` joins two strings. `int_to_string(42)` is `"42"`. `f"..."` interpolates; each
hole is written as the value's `display`:

```ply
test "text is indexed by character" {
  assert_eq(string_len("héllo"), 5);
  assert_eq(bytes_len(bytes_of_string("héllo")), 6);
  assert_eq(f"n = {1 + 1}", "n = 2");
  assert_eq(string_slice("abcdef", 1, 3), "bc")
}
```

`string_len` counts characters and `bytes_len` counts bytes, which is why the two
numbers differ for `héllo`. `string_slice(s, lo, hi)` takes a half-open range of
characters, and arguments out of range raise.

A **line string** is a run of lines each starting with `\\`. Everything after
the `\\` is verbatim text — no escapes, no interpolation — and the lines join
with `\n`:

```ply
fn program() -> String =
  \\fn main() -> Int = 42
  \\
```

`program()` is `"fn main() -> Int = 42\n"`; the second line, holding only `\\`,
is what ends the value with a newline. Line strings are how a program embeds a
snippet of another language without escaping it.

> **Try it.** Write a line string holding a small shell script and print it with
> `ply run --host`. Then change `\\` to `\` and read the diagnostic — the row
> starts with exactly two backslashes.

## Bytes

`Bytes` is immutable bytes, written `b"GET "`. Indexing and slicing are by byte,
and an index answers an `Int`:

```ply
test "a byte is an Int" {
  assert_eq(bytes_at(b"abc", 0), b'a');
  assert_eq(b'{', 123)
}
```

`b'{'` is not a separate kind of value: it is the `Int` 123, written in the form a
byte string uses. That is what lets a scanner be a `match` over bytes.

`bytes_of_string` and `string_of_bytes` convert between the two. `string_of_bytes`
raises on bytes that are not valid UTF-8; `string_of_bytes_lossy` replaces what it
cannot decode. `bytes_len`, `bytes_slice`, `bytes_concat`, `bytes_index_of`,
`bytes_split`, `bytes_scan` and the rest are in the prelude; `ply doc prelude`
lists them.

## How a value prints

`ply run` prints what `main` returned with the same `show` that diagnostics and
interpolated strings use. Records and constructors are written in a canonical
form; a record's fields are sorted by name, so field order never reaches the
output:

```console
$ ply run
   Food
```

```console
$ ply run
   {cents: 1200, what: "lunch"}
```

`show` is not a builtin you call directly. Interpolation uses it; `std.show` has
`show` and `display` for explicit use, and `ply doc std.show` describes them.

## Operators at a glance

From loosest to tightest, with all binary operators left-associative:

| operators | operands |
| --- | --- |
| `\|\|` | `Bool` |
| `&&` | `Bool` |
| `==` `!=` `<` `<=` `>` `>=` | see below |
| `\|` | integer |
| `^` | integer |
| `&` | integer |
| `<<` `>>` `>>>` | integer; the count is `Int` |
| `++` | `String` or `Bytes` |
| `+` `-` | numeric |
| `+%` `-%` `+\|` `-\|` | integer |
| `*` `/` `%` | numeric |
| `*%` `*\|` | integer |
| prefix `-` `!` `~` | numeric / `Bool` / integer |
| postfix `f(x)` `r.field` `e.op[r](x)` `e?` | |

`==` is structural at every type except functions, and a type that states a `key`
is compared through it (chapter 4). `<` and friends work on numeric types, on
`Char`, and on a keyed type; use `compare` for everything else. `min` and `max`
take two values of an ordered type.

## Summary

- `Int` is 64-bit signed. Fixed-width integers carry a suffix; there is no
  implicit conversion, and every conversion is a named builtin.
- `Float` is IEEE; `Decimal` is exact base ten with a written scale.
- `Char` is one scalar value; `String` is UTF-8 indexed by character; `Bytes` is
  indexed by byte and a byte literal is an `Int`.
- `+%`/`+|`/`checked_*` make wrapping, saturating and checked arithmetic explicit.
  Division by zero can raise; overflow ends the run.
- Interpolation, `assert_eq` and `ply run` all print through `show`, which sorts
  a record's fields.

Next: the two shapes you build your own types from — records and sums.
