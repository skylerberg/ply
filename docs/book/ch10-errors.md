# 10. Errors: raising, `try` and `?`

Ply has no exceptions. Failure is an effect like any other: an operation declared
`raise` does not come back, a row names every way a call can fail, and a handler
answers what the caller does about it. This chapter covers the three forms you
will use: `raise`, `try` and `?`.

## `raise` operations

An operation declared `raise` writes a name and parameters and no result:

```ply
type SyntaxError = { line: Int, why: String }

effect toml {
  raise syntax(e: SyntaxError)
}

fn digit(b: Int, line: Int) -> Int / {toml.syntax} =
  if b >= 48 && b <= 57 { b - 48 } else { toml.syntax({ line: line, why: "not a digit" }) }
```

Performing a raise puts the operation in the row, exactly as any perform does.
The perform has whatever type is asked of it, because it never comes back. A row
therefore names each way a call can fail: inference unions them, and they pass
through a row variable, so `map(lines, parse_line)` raises whatever `parse_line`
raises.

A raise takes no resource label and no type parameters; its declaration is just
the name and its parameters.

## Answering a raise

A clause for a raise, with or without `resume` (it cannot bind one — `E0201`),
has the `handle`'s type rather than the perform's:

```ply
fn digit_or(b: Int, fallback: Int) -> Int / {} =
  handle { digit(b, 1) } with { toml.syntax(e) -> fallback }
```

Because `digit_or`'s row is empty, it answers the raise completely, and a caller
sees no failure at all. The clause runs outside its `handle`, once the body is
abandoned and any regions it opened are closed, so a raise inside another clause
goes to a `handle` further out than the one whose clause raised.

A raise that no clause answers ends the run: a test's failure is a raise, which
is why a raise is in no test footprint (chapter 13).

## `try`

`try { body }` is the shorthand for "answer the one raise operation this body can
perform, and give me a `Result`":

```ply
fn checked(b: Int) -> Result<Int, SyntaxError> = try { digit(b, 1) }
```

It answers `Ok` of what the body answers, or `Err` of what a raise carried: the
value for a raise of one parameter, `()` for one of none, and the tuple for one
of several. It answers the one `raise` operation the body's row names, taking
that atom out of the row. A body that names none is `E0311`; one that names
several needs `try[toml.syntax] { body }` to say which, and lets the others pass
(`E0312`); naming an operation that is not a `raise` is `E0313`.

A `try` is exactly `handle` with that one clause and `return v -> Ok(v)`, so
everything that holds of a `handle` holds of it. `std.result` has
`result_unwrap_or_else(r, |e: SyntaxError| toml.syntax(e))` to send an `Err` back
to its raise.

```ply
test "raise and try" {
  assert_eq(digit_or(53, -1), 5);
  assert_eq(digit_or(120, -1), -1);
  assert_eq(checked(53), Ok(5));
  assert_eq(checked(120), Err({ line: 1, why: "not a digit" }))
}
```

## `?`

`e?` is sugar for `match e { Err(er) -> Err(er), Ok(x) -> rest }`, taking its
constructors from the enclosing function's **written** return type:

```ply
fn parse(s: String) -> Result<Int, String> =
  if string_len(s) == 0 { Err("empty") } else { Ok(string_len(s)) }

fn double(s: String) -> Result<Int, String> = {
  let n = parse(s)?;
  Ok(n * 2)
}
```

The `Option` form works the same way, for a function returning `Option`:

```ply
fn inc(d: Option<Int>) -> Option<Int> = { let n = d?; Some(n + 1) }
```

Two rules decide where a `?` may appear, and each has its own code.

**`E0118`** — there is no `Result` or `Option` for the `?` to expand into. This
is a `?` inside a `handle`, `try`, `with_cell`, `with_hold` or `simulate`; inside
a lambda with no written return type; or where `Ok`/`Err`/`Some`/`None` are
rebound.

```text
Error[E0118]: this file gives `?` no meaning here
  --> q.ply:9:14
   |   let f = || parse(s)?;
   |              ^^^^^^^^^ no `Result` or `Option` to expand this into
   = this `?` is inside a lambda, which has no written return type of its own. `?` exits the expression it is written in and never a lambda, a handler or a region: name a function with a written `->` and call it
```

A lambda **with** a written return type does exit the lambda, so that is the fix.

**`E0119`** — the `?` is not in a position it can be expanded from. Nothing
conditional may sit between it and the function's result, and everything
evaluated before it must be pure.

```text
Error[E0119]: this `?` is not in a position it can be expanded from
  --> q.ply:4:21
   |   let n = { let m = parse(s)?; m };
   |                     ^^^^^^^^^ `?` cannot exit from here
   = `?` lifts what it unwraps to the head of the statement, or of the return position, it is written in — so nothing conditional may sit between the two, and everything evaluated before it must be pure. Bind it first: `let a = e?;`
```

The diagnostic states the fix: bind it first. `let x = e?;` and
`parse_or_more(parse_and(ts)?)` are fine; a `?` in an `if` branch that is not the
function's result, or after an impure argument, is not.

`?` converts nothing: a `Result<_, E1>` inside a function returning
`Result<_, E2>` is `E0201`, whatever the two error types are. Convert explicitly.

## `abort.raise`: the failure that is always there

The prelude declares

```ply
effect abort { raise raise(message: String) }
```

`abort.raise(m)` is that raise, and so is everything that can fail on the value it
is given: `panic`, `assert`, `assert_eq`, a builtin used outside what it is
defined for, a `/` or `%` by zero, a `let` whose pattern misses, an `iterate`
past its budget, `task.join` of a cancelled task, and the rest. If a body can
raise, its written row must name `abort.raise` (`E0302`), and the diagnostic
offers the row to write.

A clause for `abort.raise` is handed the message. Unanswered, it is `E0501` for
an assertion and `E0502` for anything else. A `try` answers it only by name:
`try[abort.raise] { body }`.

## What the checker knows

The checker eliminates a call's `abort.raise` where the conditions on the way to
the call show every clause of the builtin's `requires`. The clauses are linear
`Int` arithmetic — literals, bindings, fields, `+`, `-`, a product with a
literal, a quotient or remainder by a literal, a mask, and what a builtin's
`ensures` says — plus the conditions the body passed: an `if`, the left of `&&`
or `||`, a `match` guard, a literal or range pattern, `list_at`/`array_at`
answering `Some`, and the bounds of a `range` a `map`/`filter`/`fold` walks.

So this function's row is empty even though it divides:

```ply
fn safe_div(a: Int, b: Int) -> Result<Int, String> =
  if b == 0 { Err("division by zero") } else { Ok(a / b) }
```

The `else` branch holds `b != 0`, so the divisor is shown not zero and `/` adds
no `abort.raise`. The checker's reasoning is its own and local: it sees only the
body it is in, and where it cannot show a clause it leaves the raise in place
rather than refusing the program. A definition's own `requires` is not read at
its callers, so nothing in its body may lean on it (chapter 14).

`ply check --types --explain` lists, under each definition, every place its body
can raise `abort.raise` and why.

> **Try it.** Write a function that indexes a `Bytes` with `bytes_at(b, i)` and
> call it from a loop that guards `i >= 0 && i < bytes_len(b)`. Then let the
> checker tell you whether the guard was enough, with `ply check --types
> --explain`. Guarding one end is a common mistake: `i < bytes_len(b)` alone
> still leaves a negative `i` raising.

## Summary

- A `raise` operation does not come back; its perform has the type asked of it
  and its operation joins the row.
- A raise clause has the `handle`'s type and cannot bind `resume`. It runs after
  the body's regions close.
- `try { body }` answers `Ok`/`Err` for the one raise operation in the body's row;
  `try[op] { body }` names which when several are possible.
- `?` exits through the enclosing function's written `Result`/`Option`. `E0118`
  is a `?` with nothing to expand into; `E0119` is a `?` not in an expandable
  position. `?` never converts an error.
- `abort.raise` is the universal failure. The checker removes it where a
  `requires` is shown, so a guarded call can be failure-free.

Next: state that lives inside a region, and the discipline that keeps it from
escaping.
