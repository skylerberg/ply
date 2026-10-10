# 7. Generics, labels and constraints

Writing one function for many types is not a special feature in Ply; it is the
default. This chapter covers type parameters, the two other kinds of parameter a
definition can take — row parameters and resource labels — and the constraints
that say what a type parameter must be able to do.

## Type parameters

A type parameter is a lowercase name in `<...>`:

```ply
fn apply<a, b>(x: a, f: (a) -> b) -> b = f(x)

fn pair<a>(x: a) -> (a, a) = (x, x)
```

A call settles them from the arguments, so you write neither:

```ply
test "type parameters are inferred" {
  assert_eq(apply(1, |n: Int| n + 1), 2);
  assert_eq(pair("x"), ("x", "x"))
}
```

A `type` takes parameters the same way, on an alias, a sum and a `new` record:

```ply
type Box<a> = { label: String, inner: a }
type Pair<a> = new { first: a, second: a }
type Tree<a> = | Leaf | Node(Tree<a>, a, Tree<a>)
```

An alias's arguments are written into its expansion, so `Box<Int>` is the record
`{ label: String, inner: Int }`. A sum's and a `new` record's arguments are part
of which type it is: `Tree<Int>` is not `Tree<String>`, and filling the
parameters differently makes another type.

> **Polymorphic recursion is not inferred.** Each member of a recursive group
> keeps its own type parameters, and a call inside the group must use the
> callee's. A call that would ask for a different type is `E0308`:
>
> ```text
> Error[E0308]: a call to `depth` inside its own recursive group cannot give `a` another type
>   --> p.ply:1:48
>    | fn depth<a>(x: a) -> Int = if true { 1 + depth(["s"]) } else { 0 }
>    |                                                ^^^^^ `a` would have to be `List<String>` here
>    = the members of a recursive group are checked with one set of type parameters each, so every call inside it uses the callee's own; calling one at another type is polymorphic recursion, which type inference cannot decide
>    = break the cycle so `depth` is checked on its own before this call, or monomorphise it: write the type this call needs
> ```
>
> The fix is to break the cycle or write the type the call needs.

## Row parameters

A row parameter says "this function does whatever you hand it does". It is
written after `|` in the parameter list, and it flows into the caller's row:

```ply
fn twice<| e>(f: () -> Unit / e) -> Unit / e = { f(); f() }
```

Given a pure argument, `e` is `{}` and `twice` performs nothing. Given a lambda
that writes a log, the log's atom fills `e`, and `twice` carries it:

```ply
test "a row parameter flows through" {
  handle {
    twice(|| log.note[app]("x"))
  } with {
    log.note[app](s) -> (),
  }
}
```

That is how `map`, `fold` and the rest of the prelude are written: they perform
exactly what their callbacks perform, and a caller's row says so.

A row argument is a row, written as chapter 8 writes one — `{}`, `{fs.read_at[src]}`,
`{log.write | e}`, or a bare `e` for the callers that have no row of their own:

```ply
fn run_record<a | e>(f: () -> a / e) -> a / e = f()
```

A type may carry a row parameter too, and then a use fills it:

```ply
type Thunk<| e> = () -> Unit / e
type Step<a | e> = () -> Option<a> / e
```

A sum's row argument is held exactly, not as a bound (chapter 8), so
`Seq<Int | {a.x}>` is not `Seq<Int | {}>`.

## Resource labels

A bracketed name binds a **resource label**, which lets a definition speak about
any resource rather than one named at its definition:

```ply
fn relay<[l]>(b: Bytes) -> Unit / {net.send[l]} = net.send[l](b)
```

The `[l]` in the body is the parameter, and a call fills it explicitly, or from
an argument's row:

```ply
test "a label parameter is filled from the call" {
  handle {
    relay[conn](b"x")
  } with {
    net.send[conn](b) -> (),
  }
}
```

Binders sit among the type parameters and before the `|`, and one bracket may
hold several: `<a, [l], [k] | e>` is `<a, [l, k] | e>`. The standard library is
written this way: `std.net` and `std.http` name no resource of their own, so a
program can answer its connections under one label and talk upstream under
another through one serve loop and one writer. The reference covers the rules
for recursive groups and printed signatures (§4.5, §6.2).

## Constraints

A constraint says what a type parameter must be able to do. Three families exist.

`where numeric(a)` lets `a` take arithmetic and the ordered comparisons, so the
operators and `numeric_of_int` work at it:

```ply
fn sum<a>(xs: List<a>) -> a where numeric(a) =
  fold(xs, numeric_of_int(0), |s: a, x: a| s + x)
```

`where integer(a)` adds `/`, `%` and the bit operators. A call fills the
parameter with one of the numeric types — `Int`, a fixed width, `Float`,
`Decimal`, or a type that states `numeric` and a `key` (chapter 4) — or with a
parameter of its own under the same constraint. Anything else is `E0201`.

A constrained parameter is passed as a **hidden argument**, because the operators
at `a` need the type's functions to call. So such a definition must be called
directly, and used as a value it is `E0310`:

```text
Error[E0310]: `sum` must be called directly
  --> q.ply:4:34
   | fn bad() -> (List<Int>) -> Int = sum
   |                                  ^^^ used as a value
   = `sum` takes the type its `numeric` constraint is about as a hidden argument each call fills
   = wrap it in a lambda that calls it
```

`where derivable(D, a)` says a value of `a` can be handled by the deriver `D`.
The ones you will use are `eq`, `ord`, `hash`, `show`, `json` and `bin`
(chapter 15):

```ply
fn render<a>(x: a) -> String where derivable(show, a) = f"{x}"

fn fingerprint<a>(x: a) -> Bytes where derivable(hash, a) = digest(x)
```

```ply
test "a derivable constraint" {
  assert_eq(render(42), "42");
  assert_eq(render("hi"), "hi");
  assert_eq(bytes_len(fingerprint([1, 2])), 32)
}
```

A `Map` key needs `derivable(ord, k)`, which is why a function or a `Float`
cannot be one. `derivable` is also the promise a `derive` (chapter 15) and a
`forall` binder (chapter 14) are held to.

A constraint goes after the row and before any `requires`:

```ply
fn render_all<a | e>(xs: List<a>, f: (a) -> String / e) -> String / e
  where derivable(show, a)
  requires len(xs) < 1000000
= join(map(xs, f), ", ")
```

> **A note on `display`.** Interpolation writes a `String` or a `Char` as itself
> and any other value as `show` writes it, so `f"{x}"` of a string is `hi` while
> `show(x)` of one is `"hi"`. A type that states `show for T by g` writes what
> `g` answers.

## Summary

- Type parameters are inferred at calls. Aliases expand; a sum's or `new`
  record's arguments are part of the type's identity.
- Polymorphic recursion is not inferred (`E0308`); break the cycle or
  monomorphise.
- A row parameter `<| e>` carries whatever a callback performs into the
  caller's row.
- A label parameter `<[l]>` speaks about any resource; a call fills it, often
  from an argument's row.
- `where numeric(a)` (arithmetic), `where integer(a)` (adds `/`, `%`, bits) and
  `where derivable(D, a)` constrain a type parameter. A constrained parameter is
  a hidden argument, so the definition must be called directly (`E0310`).

Next: the part of Ply that makes the rest cohere — effects in the type.
