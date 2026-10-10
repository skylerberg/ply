# 6. Collections and iteration

Ply has three collection shapes that come with the language — `List`, `Array`
and `Map` — plus a set in the standard library. All of them are immutable values.
This chapter covers them, the higher-order functions you work with them through,
and the shape of recursion that replaces a loop.

## Lists

`List<a>` is an immutable homogeneous sequence, written `[a, b, c]`:

```ply
test "list basics" {
  let xs = [1, 2, 3];
  assert_eq(len(xs), 3);
  assert_eq(push(xs, 4), [1, 2, 3, 4]);
  assert_eq(list_at(xs, 0), Some(1));
  assert_eq(list_at(xs, -1), None);
  assert_eq(list_set(xs, 0, 9), [9, 2, 3])
}
```

- `len(xs)` is the count.
- `push(xs, x)` answers a new list with `x` appended.
- `list_at(xs, i)` answers `Option`: `None` for an index past the end **or
  negative**. `list_at(xs, -1)` is `None`, not the last element.
- `list_set(xs, i, v)` replaces one element and **raises** out of range.
- List patterns `[]`, `[a, b]`, `[a, ..]` and `[a, ..rest]` take a list apart.

`push` never changes `xs`, but nothing is copied when it does not have to be: if
the caller holds the last reference to the list, `push` appends in place. Whether
it can is a property of the whole body, and the checker will tell you when a
`push` copies:

```console
$ ply check --costs
```

`--costs` reports every copying `push`, `list_set` and `array_set`, with the
cause and the fix. When a definition promises its updates never copy, write
`reuse fn`:

```ply
reuse fn collect(xs: List<Int>, n: Int) -> List<Int> =
  if n <= 0 { xs } else { collect(push(xs, n), n - 1) }
```

A copying update in a `reuse fn` is `E0127`:

```text
Error[E0127]: `grow` is a `reuse fn`, and this `push` copies its list: `xs` is read again after this point, so this use clones it
  --> r.ply:2:12
   |   let ys = push(xs, n);
   |            ^^^^^^^^^^^ this update
   --> r.ply:1:1
   | reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {
   | ^^^^^ the promise
   = fix: make the update the binding's last use: bind whatever you still need out of it first
```

The promise is checked where the definition's package is checked. A caller in
another package reads the `returns` clause instead: `returns fresh` says the
answer shares nothing with the arguments, `returns xs` says it may share the
parameter `xs`. Without a clause, a caller cannot know an append onto the answer
reuses. You do not need any of this to write correct code — only to make an
allocation-free path a checked fact rather than a hope.

## Higher-order functions

`map`, `filter` and `fold` are in the prelude and take a lambda or a named
function:

```ply
test "map, filter and fold" {
  let xs = [1, 2, 3];
  assert_eq(map(xs, |n: Int| n * 2), [2, 4, 6]);
  assert_eq(filter(xs, |n: Int| n > 1), [2, 3]);
  assert_eq(fold(xs, 0, |a: Int, n: Int| a + n), 6);
  assert_eq(range(0, 4), [0, 1, 2, 3])
}
```

`fold` walks left to right, calling `f(accumulator, element)`. `range(lo, hi)` is
`[lo, hi)`. `map_fold` is the map version. Any effectful callback carries its row
into the row of the caller, so `map(ids, lookup)` performs whatever `lookup`
performs — and does it a number of times that grows with the list, which the row
records as `scaling` (chapter 8).

The ordinary arithmetic builtins (`list_at`, `map_get`, `array_get`, and so on)
are listed by `ply doc prelude`, and the standard library adds the rest. If you
find yourself writing a helper over a list, check `ply doc std.list` first.

## Arrays

`Array<a>` is a fixed number of elements laid out one after another, so reading
or replacing one by index is a load or a store. It is a value like a list —
compared, ordered and derived element by element — and it has no literal:

```ply
test "arrays have a fixed length" {
  let a = array_new(3, 0);
  assert_eq(array_len(a), 3);
  assert_eq(array_get(a, 1), 0);
  assert_eq(array_get(array_set(a, 1, 5), 1), 5)
}
```

`array_new(n, x)` makes `n` copies of `x`; `array_of_list` and `array_to_list`
convert. `array_get` raises out of range; `array_at` answers `Option`.
`array_set` copies the whole array unless the caller holds the last reference.

## Maps

`Map<k, v>` is an immutable sorted map, written `#{k: v}` or built with
`map_new`, `map_insert` or `map_of_entries`. It iterates in `compare` order,
which is why the key type must be ordered:

```ply
test "maps are ordered by key" {
  let m = #{"b": 2, "a": 1};
  assert_eq(map_keys(m), ["a", "b"]);
  assert_eq(map_values(m), [1, 2]);
  assert_eq(map_get(m, "a"), Some(1));
  assert_eq(map_get(m, "z"), None);
  assert_eq(map_keys(map_insert(m, "c", 3)), ["a", "b", "c"]);
  assert_eq(map_len(m), 2)
}
```

A literal is the call `map_of_entries([{key: k, value: v}, ...])`, so the two
spellings are one definition. A later entry for a key replaces an earlier one.
`Float`, `Secret`, functions and the region types cannot be keys (`E0206`);
everything else can, or states a `key` (chapter 4).

## Sets

A set is written `#[a, b]` and is `std.set`'s `Set<a>`: each element once, in
`compare` order. `show` writes it back as the literal:

```ply
import std.set

test "sets hold each element once, ordered" {
  assert_eq(#[3, 1, 3, 2] == #[1, 2, 3], true);
  assert_eq(f"{#[3, 1, 3, 2]}", "#[1, 2, 3]");
  assert_eq(set::of_list([3, 1, 3, 2]) == #[1, 2, 3], true)
}
```

A module that writes a set literal imports `std.set` implicitly, under a name no
source can write; `set::of_list` above needs the ordinary import.

## Iteration: recursion, and where it stops

There are no `for` or `while` loops, and no `break`. A call of the enclosing
function in **tail position** runs as a loop: it does not nest, so it may run any
number of times. Tail position is the body's own value, the tail of a block, an
`if` or `match` arm, or the right operand of `&&`/`||`:

```ply
fn sum_to(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { sum_to(n - 1, acc + n) }
```

`sum_to(1000000, 0)` runs in constant stack. A tail call to another member of the
same **recursive group** is a loop too, so mutual recursion works:

```ply
fn even(n: Int) -> Bool = if n <= 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n <= 0 { false } else { even(n - 1) }
```

Every other call nests, at most 10,000 deep, and then fails with `E0502`. Depth
and work are two separate bounds: one entry also gets a budget of a billion calls
by default, which `--steps N` changes.

### Proving a loop ends

Because the checker reads each recursive group for a measure that every loop
lowers, a recursion that descends needs nothing written. A part of an argument (a
constructor's field, a list's tail, a map's entry) counts, and so does an integer
moving toward a bound a guard on the way holds it beyond. `even`/`odd` descends
because the `n <= 0` guard means the recursive call has `n > 0`, so `n - 1` is
closer to the bound. A guard of `n == 0` would not have counted: from `-1`, `n - 1`
never meets it.

When no single argument descends, only a combination does. `decreases` states an
`Int` over the parameters:

```ply
fn climb(a: Int, b: Int) -> Int
  decreases a + b
  = if a + b <= 0 { 0 } else { climb(a + 1, b - 2) }
```

`a + b` falls by one at every call, so the group ends and the row is empty. A
measure no proof shows is `E0467`. A definition whose recursion is not seen to
descend must say so:

```ply
fn forever(n: Int) -> Int / {diverges} = forever(n + 1)
```

```text
     forever : (Int) -> Int
               / {diverges scaling}
               diverges: its recursion at d.ply:1:42 is not seen to descend
```

`diverges` belongs in the row only of a loop that waits on something outside,
such as a stream or a peer, and a caller inherits it. A budget spent finishes
with `E0503` only in a test; a `run` has no bound, because an entry point that
serves forever is a program.

### `iterate`: a loop with a budget

When you want the bound in the program rather than in an argument, `iterate`
takes a seed, a budget and a step function answering `Continue(next)` or
`Stop(result)`:

```ply
fn first_gap(xs: List<Int>) -> Int / {abort.raise} =
  iterate({i: 0, want: 0}, 1000, |s: {i: Int, want: Int}|
    if s.i >= len(xs) { Stop(s.want) }
    else { Continue({i: s.i + 1, want: s.want + 1}) })
```

Spending the budget raises. The `abort.raise` is in the row because the budget
can run out — which is exactly the distinction between a recursion that always
ends and a loop with a bound.

> **Try it.** Write `first_gap` as a plain tail recursion instead of `iterate`
> and check that its row loses `abort.raise`. Then write it with a bound you pass
> in and see which version `ply check --types` describes best.

## Summary

- `List` is immutable and homogeneous; `push`/`list_set` copy only when the value
  is shared, and `--costs` and `reuse fn` make that a checked fact.
- `map`, `filter`, `fold`, `range` and `iterate` are in the prelude; a callback's
  row joins the caller's.
- `Array` is fixed-size and laid out; `Map` is sorted by key; a set (`#[...]`) is
  ordered and deduplicated.
- Tail calls loop. The checker proves a recursion ends from the arguments and the
  guards, `decreases` states a measure when no single argument descends, and
  `diverges` is written when a loop is meant never to return.

Next: writing a function once for many types, and the constraints that go with it.
