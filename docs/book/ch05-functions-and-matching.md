# 5. Functions, blocks and pattern matching

A Ply program is a set of definitions, and almost all of them are functions. This
chapter is about writing their bodies: the block, the `let`, `if` and `match`,
and how a call is spelled.

## Defining a function

```ply
fn credit(a: Account, amount: Int, note: Option<String> = None) -> Account {
  let moved = a.balance + amount;
  { name: a.name, balance: moved }
}
```

- Every parameter and return type of a top-level `fn` is written (`E0126`).
- The body is either `= expression` or a block with no `=`.
- There is no `return`; a body is an expression whose value is the result.
- A parameter the body never uses may be written `_`, and may repeat.
- `pub` makes it visible to other modules (chapter 16).

## Blocks and `let`

A block is `{ statements... tail }`. Its value is the tail's value, or `Unit`
with no tail. A statement ends with `;` unless it is the tail or is block-like
(`if`, `match`, `handle`, another block):

```ply
fn total_of(xs: List<Int>) -> Int = {
  let doubled = map(xs, |n: Int| n * 2);
  fold(doubled, 0, |sum: Int, n: Int| sum + n)
}
```

`let` binds once and may shadow:

```ply
fn shadow() -> Int = { let x = 1; let x = x + 1; x }
```

A `let` whose pattern can fail needs an `else`, and the `else` block becomes the
value of the enclosing block when the pattern does not match:

```ply
fn first_word(line: String) -> String = {
  let [word, ..] = string_split(line, " ") else { "" };
  word
}
```

> **Trap.** A `let` is a statement, not an expression, so it cannot follow `=`:
> `fn f() -> Int = let x = 1; x` does not parse. Write the block.
>
> ```text
> Error[E0001]: expected an expression, found keyword `let`
>   --> a.ply:1:17
>    | fn f() -> Int = let x = 1; x
>    |                 ^^^ expected an expression
> ```

## `if`

`if` is an expression. It requires braces, and every branch must have one type:

```ply
fn classify(n: Int) -> String =
  if n < 0 { "negative" } else if n == 0 { "zero" } else { "positive" }
```

Without `else`, an `if` has type `Unit`, so it is only useful for its effect.

## `match` and patterns

`match` is how you take a value apart. It must cover every case, and the
diagnostic names the one you missed:

```text
Error[E0205]: match does not cover every case
  --> c.ply:3:31
   | fn name(c: Color) -> String = match c { Red -> "red", Green -> "green" }
   |                               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ not covered: `Blue`
   = add the missing arms, or a `_` arm
```

This is the full set of patterns:

| pattern | matches |
| --- | --- |
| `_` / `name` | anything; `name` binds it |
| `Ctor`, `Ctor(p, q)`, `mod::Ctor(p)` | a constructor |
| `42`, `-1`, `1.5`, `1.50m`, `"s"`, `b"s"`, `b'{'`, `true`, `()` | a literal |
| `0..=9`, `b'a'..=b'z'`, `1u8..=9u8` | an integer from the first bound through the second |
| `[]`, `[a, b]`, `[a, ..]`, `[a, ..rest]` | a list of exact length, or a prefix |
| `{a, b}`, `{a: p, b: q}`, `{a, ..}` | a record; `..` allows other fields |
| `(p, q)` | a tuple |
| `p \| q` | either alternative, the leftmost first |

A pattern in a `let` binds without matching arms:

```ply
fn swap(p: (Int, String)) -> (String, Int) = match p { (n, s) -> (s, n) }
```

An arm may carry a guard, and an arm is tried once per alternative, so a guard
runs again for a later alternative when an earlier one matched and the guard
refused it:

```ply
fn describe(v: Option<Int>) -> String = match v {
  Some(n) if n > 100 -> "big",
  Some(n) -> int_to_string(n),
  None -> "nothing",
}
```

Alternatives join with `|` and must bind the same names at one type (`E0212`):

```ply
fn vertical(d: Dir) -> Bool = match d { North | South -> true, East | West -> false }
```

A range's bounds are two integer literals of one type with the first no greater
than the second. Literals and ranges that between them hold every value of a
`U8`, `U16` or `U32` exhaust it; no other integer type's ends are both literals,
so a match over one ends in `_`.

## Lambdas

A lambda is written `|args| body`, with optional annotations, and captures by
value:

```ply
|acc, a: Account| acc + a.balance
|| do_something()
|r: Result<Int, E>| -> Result<Int, E> { Ok(r? + 1) }
```

Annotations are often needed on the element of a `fold` or `map` over a record,
because the compiler has nothing else to infer the element's type from. A written
return type requires a block body, and it is what lets `?` inside the lambda exit
the lambda rather than the enclosing function (chapter 10).

A lambda's row flows into the enclosing function's: a lambda that performs an
effect adds that atom to the row of whatever contains it.

## Calls

Arguments fill parameters left to right, and the rest must be named or defaulted:

```ply
fn greet(name: String, greeting: String = "hello") -> String =
  greeting ++ ", " ++ name

test "named and defaulted arguments" {
  assert_eq(greet("ada"), "hello, ada");
  assert_eq(greet("ada", greeting: "hey"), "hey, ada")
}
```

A positional argument after a named one is `E0124`, an unknown or repeated name
`E0123`, and a parameter left unfilled by a call that used names `E0125`. Only a
callee reached by name takes named arguments, and there is neither partial
application nor method syntax.

A parameter default must be a value: a literal, a constructor over literals, a
record or a list. It may not name another parameter (`E0121`).

> **Trap.** `x.f(y)` where `x` is a bare variable is an **effect perform**, not a
> call of a function held in a field. To call the function in a record field,
> parenthesize the field access:
>
> ```ply
> fn use(h: Handlers) -> Int = (h.decode)("7")
> ```
>
> Writing `h.decode(s)` by mistake gets a diagnostic that explains itself:
>
> ```text
> Error[E0103]: unknown effect `h`
>   --> b.ply:2:34
>    | fn use(h: H, s: String) -> Int = h.decode(s)
>    |                                  ^ not declared
>    = `h` is a value in scope, so this looks like a field call rather than a perform: write `(h.<field>)(..)` to call a function held in a record
>    = `a.b(c)` is an effect operation; parentheses make it a field access
> ```

A call that is not in tail position nests, and there is a limit; loops and the
`decreases` measure that extends recursion beyond it are in
[chapter 6](ch06-collections.md).

## Summary

- A body is `= expression` or a block. Blocks have a tail value; `let` is a
  statement and may shadow; a failing pattern needs `let ... else`.
- `if` and `match` are expressions. `match` must be exhaustive.
- Patterns cover constructors, literals, ranges, lists, records, tuples and
  alternatives, with optional guards.
- A lambda captures by value and carries its row into the enclosing function.
- Arguments are positional or named; defaults must be pure closed values. A
  field call needs parentheses.

Next: the collections the standard library gives you, and the shapes of recursion
that replace loops.
