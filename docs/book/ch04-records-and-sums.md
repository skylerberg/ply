# 4. Records, tuples and sums

Chapter 3 covered the values that come with the language. This chapter covers the
values you build: records that group named fields, tuples that group positional
ones, and sums whose constructors are the cases of a value. It ends with the two
declarations that tell the language how to compare and print one of your types.

## Records

A record literal names its fields. Access is `.field`:

```ply
// spend.ply
type Expense = { what: String, cents: Int, category: Category }

fn amount(e: Expense) -> Int = e.cents
```

`type Expense = { ... }` is an **alias**: it names the record type `{ what: String,
cents: Int, category: Category }`, and every place that expects one accepts the
other. When a field's name is also a binding in scope, the record literal can
pun it:

```ply
fn spend(what: String, cents: Int) -> Expense =
  { what, cents, category: Other }    // `what` is `what: what`
```

Records are structural and field order does not matter, so an alias and a
literal record type with the same fields are one type:

```ply
type Account = { name: String, balance: Int }

fn g(r: { name: String, balance: Int }) -> Int = r.balance
fn f(a: Account) -> Int = g(a)           // Account is that record type
```

An update copies a record with some fields replaced. `..base` is the base, which
must be a variable, a field path, or one call:

```ply
fn cheaper(e: Expense, by: Int) -> Expense = { ..e, cents: e.cents - by }
```

A pattern can take a record apart. A record pattern names every field or ends
with `..`:

```ply
fn label(e: Expense) -> String = {
  let { what, cents, .. } = e;
  what ++ " " ++ int_to_string(cents)
}
```

[Chapter 5](ch05-functions-and-matching.md) covers patterns properly.

## Tuples

A tuple is a record with positional fields. `(A, B)` means `{_0: A, _1: B}` in
types, values and patterns, and the fields are read as `._0`, `._1`:

```ply
fn divmod(a: Int, b: Int) -> (Int, Int) / {abort.raise} = (a / b, a % b)

fn first(t: (Int, String)) -> Int = t._0
```

Parentheses around a single type only group: `(A)` is `A`. `()` is `Unit`.

## Sums

A sum is a type whose values are one of several named constructors, each with
zero or more payload values:

```ply
type Category = | Food | Transport | Rent | Other

type Shape =
  | Circle(Int)
  | Rect(Int, Int)
  | Point
```

The leading `|` is optional. A nullary constructor like `Point` is a value; a
constructor with a payload like `Circle(3)` is a call. You take a sum apart with
`match`, which must be exhaustive:

```ply
fn area(s: Shape) -> Int = match s {
  Circle(r) -> 3 * r * r,
  Rect(w, h) -> w * h,
  Point -> 0,
}
```

A sum is **nominal**: two modules that declare the same constructors declare two
different types. That is the opposite of a record alias, which is only ever a
name for a shape. It is why a sum is the right way to say "these are the cases"
and a record is the right way to say "this is the data".

`Option` and `Result` are sums the language declares, in scope everywhere:

```ply
Option<a>     = None | Some(a)
Result<a, e>  = Ok(a) | Err(e)
```

You do not declare them and you cannot redeclare them, which is what lets `?` and
`try` (chapter 10) know what `None` and `Err` mean.

## `new` records: a record with a name of its own

An alias shares its shape with every other record of that shape. Sometimes you
want a distinct type — a `Date` that is not a `{year, month, day}` that came from
somewhere else. Write `new`:

```ply
pub type Date = new { year: Int, month: Int, day: Int }

fn epoch() -> Date = { year: 1970, month: 1, day: 1 }
```

A `new` record is read, updated and matched exactly as a plain record is:

```ply
fn next(d: Date) -> Date = { ..d, year: d.year + 1 }
```

But it is no other type, whatever its fields:

```ply
fn plain() -> { year: Int, month: Int, day: Int } = { year: 1970, month: 1, day: 1 }
fn bad() -> Date = plain()
```

```text
Error[E0201]: type mismatch: function body type
  --> n.ply:4:20
   | fn bad() -> Date = plain()
   |                    ^^^^^^^ expected `n.Date`, found `{day: Int, month: Int, year: Int}`
   = `n.Date` is declared with `new`, so no other record is one whatever its fields; a record literal is one where a `n.Date` is expected
   compilation failed (1 error)
```

A record literal is not ambiguous, because a literal becomes a `new` record
exactly when the place it is written says so. The places are listed in full in
the reference (§4.2); the short version is: a function body, a written `let`, a
parameter default, an argument whose parameter's type is known, and the branch of
an `if`, arm of a `match` or element of a list whose expected type is one.

At run time a `new` record is the record it is written as, so two of them with
the same fields print, digest and compare alike. What is distinct is the type,
and therefore what the checker lets you write.

> **Try it.** Declare `pub type Cents = new { n: Int }` and change `Expense`'s
> `cents` field to it. The compiler will point at every place that treats it as a
> bare `Int`, including `e.cents + 1`. Then decide whether the extra type was
> worth it — often it is, for a quantity that must not be confused with another.

`opaque` keeps a type's values to the module that declares it, which is a
visibility rule rather than a typing one; it is covered in
[chapter 16](ch16-packages.md).

## Custom equality and display

A value is compared, ordered, digested and shown as its constructors and fields
say. That is wrong for a type that can hold one thing two ways — a queue split
differently, an amount kept as a numerator and a denominator. The module that
declares a sum may state what its values are read through instead:

```ply
type Interval = | Interval(Int, Int)   // a start and a length

key for Interval by start              // ==, !=, compare, digest and map keys
show for Interval by written           // show, display and interpolation

fn start(i: Interval) -> Int = match i { Interval(s, _) -> s }
fn written(i: Interval) -> String = match i {
  Interval(s, n) -> int_to_string(s) ++ "+" ++ int_to_string(n),
}
```

`key for T by f` names a function `f: (T) -> K`. Two `T`s are equal exactly when
`f` answers equal keys, they order as their keys do, and a value's digest is its
key's. The key is read wherever a `T` sits — in a list, a record, a map, a
parameter — and by `assert_eq`. So:

```ply
test "a key decides equality and order" {
  assert_eq(Interval(1, 5) == Interval(1, 9), true);
  assert_eq(compare(Interval(1, 5), Interval(2, 0)), Less);
  assert_eq(f"{Interval(1, 5)}", "1+5")
}
```

`show for T by g` names `g: (T) -> String`, and `show`, `display` and a hole in an
interpolated string write what it answers. It decides nothing about comparison.

Three rules are worth knowing up front, because each is a diagnostic you will
meet:

- Only the module that declares `T` may state either (`E0208`), and it states
  each at most once (`E0105`). There is no search for them.
- `T` must be a sum. An alias is the type it names, and a `new` record runs as its
  plain record with no constructor to find the function by (`E0217`).
- The function is one the declaring module declares, takes one `T` and nothing
  narrower, has no `where`, and answers with an empty row: it performs nothing,
  raises nothing and always returns (`E0218`). Equality and display that could
  fail would have nowhere to put the failure — a `Map` insert raising inside a
  comparison is not a thing Ply allows.

Two more declarations sit beside these, over a type: `numeric for T by { ... }`
says what `+`, `-`, `*` and unary `-` mean at `T` (chapter 7), and `gen for T by
f` says how a proof draws values of `T` (chapter 14). The reference gives both in
full (§4.4).

## Summary

- `type N = { ... }` is an alias for a structural record. Field order does not
  matter; `n.field` reads a field; `{ ..base, f: v }` updates one.
- A tuple is `{_0, _1, ...}`, read as `t._0`.
- A sum's constructors are its cases. It is nominal, and `match` takes it apart.
  `Option` and `Result` are the language's own sums.
- `new { ... }` declares a record with a name of its own, read and updated like a
  record but no other type.
- `key for T by f` and `show for T by g` state a type's equality, order, digest
  and display. They are declared in `T`'s own module, over a sum, by a pure
  function.

Next: functions and the expression forms that make up a body.
