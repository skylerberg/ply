# 2. Effects and tests

Chapter 1 ended with a program whose `main` wrote to standard output, and the row
`{process.out[proc]}` is what said so. This chapter takes that idea seriously: you
will declare an effect of your own, put it in a function's type, and then answer
it in a test. By the end you will have a function that reads a spending store and
a test that hands it one — with no mocking library, no test doubles, and no global
state.

## The program so far

A spending tracker starts with a record and a sum to describe one expense. Put
this in `spend.ply`:

```ply
// spend.ply
type Category = | Food | Transport | Rent | Other

type Expense = { what: String, cents: Int, category: Category }
```

`Category` is a sum: one of four constructors, no payload. `Expense` is a record
with a `String`, an `Int` and a `Category`. Both forms are covered properly in
[chapter 4](ch04-records-and-sums.md); for now, a record literal is written
`{ what: "lunch", cents: 1200, category: Food }` and a field is read with
`.cents`.

## Declaring an effect

Suppose you want to total the expenses in a category. The expenses live
somewhere — a file, a database, a network service — and where they live is not
this function's business. So the function declares what it needs:

```ply
// spend.ply
effect store {
  read load[r]() -> List<Expense>
}
```

An `effect` is a named group of **operations**. `load` is a `read` operation: it
answers a value and comes back. The `[r]` makes it **resource-parameterized**:
a caller performing it says which store it means, so `store.load[spend]()` and
`store.load[archive]()` are different atoms that a handler can answer separately.
That is how one program can talk to two databases without either function knowing
about the other.

## Performing it

A perform is written like a call with a resource and a label:

```ply
fn total(category: Category) -> Int / {store.read[spend]} =
  fold(store.load[spend](), 0, |sum: Int, e: Expense|
    if e.category == category { sum + e.cents } else { sum })
```

`store.load[spend]()` is a perform. It has type `List<Expense>` here — the
compiler infers it from what `fold` needs — and it adds `store.read[spend]` to
the row of whatever contains it. That is why `total`'s type ends in
`/ {store.read[spend]}`: the row is not decoration, it is the only way a caller
learns that calling `total` touches a store.

`fold` is one of the list functions from the prelude; it walks the list left to
right, accumulating. [Chapter 6](ch06-collections.md) covers it, but the shape
above is worth reading now: start at `0`, and for each `e`, add its cents if its
category matches.

Ask the compiler what it inferred (it also warns that nothing uses the
new declarations yet, because there is no `main` and no test):

```console
$ ply check --types
   checked 1 module, 1 definition, 0 tests
   warning: type `spend.Category` is never used
   warning: type `spend.Expense` is never used
   warning: effect `spend.store` is never used
   warning: fn `spend.total` is never used

   spend spend.ply
     effect store
       read load[r]() -> List<{category: spend.Category, cents: Int, what: String}>
     total : (spend.Category) -> Int
             / {spend.store.read[spend] bounded}
```

The `bounded` beside the atom is part of the row's meaning: it says a call to
`total` performs `store.load[spend]` a number of times that no input makes grow.
A row can promise `bounded`; a body that performs the atom once per element of a
list would be refused. [Chapter 8](ch08-effects.md) is about rows and counting.

## A test hands the function its world

Here is the part that makes the row worth writing. A test can answer the effect:

```ply
test "a total counts only the category asked for" {
  handle {
    assert_eq(total(Food), 1200)
  } with {
    store.load[spend]() -> [
      { what: "lunch", cents: 1200, category: Food },
      { what: "bus", cents: 300, category: Transport },
    ],
  }
}
```

`handle { body } with { clauses }` runs `body` with a set of answers in place. The
clause `store.load[spend]() -> [...]` answers every perform of that atom inside
the body, so the body's row loses `store.read[spend]` and the test's own row is
empty — that is what lets the test run with no host at all. The list in the clause
is the world this test's `total` sees.

Run it:

```console
$ ply test
   selected 2 of 2 (0 cached)
   1 group · 10 workers
   isolated 2 of 2

   ok      a total counts only the category asked for      0.0ms
   ok      an empty store totals zero                      0.0ms

   backend c · 2 of 2 offers entered · 0 declined · 3 in the fragment
   compiled 1 unit(s) in 0.0ms, after 0.0ms deciding what to compile
   0 failed, 2 passed, 0 cached (0.00s)
```

The second test in that run is the same function against a different world:

```ply
test "an empty store totals zero" {
  handle {
    assert_eq(total(Food), 0)
  } with {
    store.load[spend]() -> [],
  }
}
```

That is the whole testing strategy Ply is built around. There is no library to
mock the store, no injection framework and no setup or teardown: a function says
which operations it needs, and every caller — including a test — supplies them.
A test that answers every operation in the body's row is hermetic, so it can be
cached and run in parallel with other tests safely.

> **Try it.** Delete the `handle` and the `with` from one test, leaving
> `assert_eq(total(Food), 1200)`. Run `ply test`. The test still checks, and fails
> at run time with a message naming the operation and a `handle` as its fix. There
> is no host handler for `store.load`, so a hermetic run cannot invent one.

A test that leaves an operation unanswered fails like this:

```text
   FAIL    no handler at all             0.1ms

   spend.no handler at all
     no handler for `spend.store.load[spend]`
       at spend.ply:10:8
     = wrap this in a `handle ... with { ... }` that names the operation
     suspects: spend.total
```

The `suspects` line names the definitions the failure reached, which is Ply
pointing at where the operation came from.

## Failing loudly beats failing quietly

A row is a promise in both directions. A function that writes `{store.read[spend]}`
may perform that operation and no other. If a body performs an operation its
written row does not name, the check refuses it, and if a row names an operation
the body never performs, that is allowed — a row is an upper bound, which lets you
widen one deliberately.

This is why the compiler can do so much with so little: a signature is a
complete, checked account of a function's reach. Chapter 8 makes the accounting
precise, and chapter 9 shows what a handler can do with `resume`, but the pattern
in this chapter — declare, perform, handle in a test — is the one you will use
constantly.

## Summary

- `effect name { ... }` declares operations. `read` and `write` operations come
  back; `raise` operations do not (chapter 10).
- `[r]` on an operation is a resource label. A perform says which resource it
  means: `store.load[spend]()`.
- A perform adds an atom to the enclosing definition's row, and the row is
  inferred automatically and printed by `ply check --types`.
- `handle { body } with { clauses }` answers operations. A body whose operations
  are all answered has an empty row and runs with no host.
- A test is an item, written `test "label" { ... }`, keyed `<module>.<label>`.

Next: the values you can put in a record — numbers, text, bytes, lists, maps, and
what is checked about each.
