# 8. Effects in the signature

Chapter 2 used an effect before defining one. This chapter makes the machinery
precise: how an effect is declared, how a row is written and inferred, what a
written row promises, and what the counts beside an atom mean.

## Declaring an effect

An `effect` is a named group of operations. Each operation is `read`, `write` or
`raise`:

```ply
type Row = { id: Int }

effect db {
  read  get[r](key: Int) -> Row
  write put[r](key: Int, value: Row) -> Unit
}
```

- `read` and `write` answer where they were performed. The distinction matters
  for concurrency: two `read`s do not conflict, a `write` conflicts with
  anything on the same resource.
- `raise` does not come back, so it writes a name and parameters and no result
  (chapter 10).
- `[r]` makes an operation resource-parameterized. A perform supplies a label,
  and a handler can answer different labels differently.

`nondet` before `effect` marks results that are not a function of program state;
[chapter 13](ch13-tests.md) covers what that means for tests.

```ply
nondet effect clock {
  read now() -> Int
}
```

Effects are nominal, and the names `task`, `clock`, `random`, `sim`, `abort`,
`diverges` and `cell` are taken by the language.

## Atoms and rows

An **atom** names one operation or one mode against one resource:
`effect.mode[resource]`, or `effect.mode` for a singleton. A **row** is a set of
atoms with an optional tail variable:

```ply
/ {}
/ {db.read[users]}
/ {db.read[users], clock.read}
/ {net.write[conn] | e}
/ e
```

A qualified atom uses `::`: `/ {store::db.read[users]}`. `/ e` is a row variable:
"whatever the caller performs". `diverges`, written bare, is the atom of a call
that may not return (chapter 6); nothing handles it.

The checker **infers** a row for every definition from what its body performs,
and a written row is an **upper bound** on it. This is why a body may perform
less than its signature says:

```ply
fn wider() -> Int / {db.read[users]} = 0    // checks: the row may be wider
```

but never more:

```text
Error[E0302]: `bad` writes no `/`, so its row is empty, and its body performs effects
  --> e.ply:3:26
   | fn bad(id: Int) -> Row = db.get[users](id)
   |                          ^^^^^^^^^^^^^^^^^ performs `e.db.get[users]`
   --> e.ply:3:4
   | fn bad(id: Int) -> Row = db.get[users](id)
   |    ^^^ no `/` written: the row is empty
   = a function performs only what its signature writes: write `/ {db.get[users]}` after the return type, or handle the effects inside `bad`
   = fix: write the row `/ {db.get[users]}`
```

The `fix:` line is literal: `ply` will tell you the exact row to write. When a
written signature is wrong, that is the fastest way to make it right.

## Mode atoms and operation atoms

A mode atom covers every operation of its mode on the resource; an operation atom
covers that one operation:

| written | covers |
| --- | --- |
| `db.write[users]` | `db.put[users]`, and any other `write` of `db` on `users` |
| `db.put[users]` | `db.put[users]` alone |

So a body under `/ {db.put[users]}` may perform `db.put[users]` and not
`db.recv`-like siblings, and may call a callee written `/ {db.put[users]}` but
not one written `/ {db.write[users]}`:

```text
Error[E0302]: effect not permitted by the signature of `caller`: `e.db.write[users]`
  --> e.ply:4:41
   | fn caller() -> Unit / {db.put[users]} = use_write()
   |                                         ^^^^^^^^^^^ reaches `e.db.write[users]` through `use_write`, which may perform any of its operations; this row names only `e.db.put[users]`
   = add `e.db.write[users]` to the `/ {..}` annotation, or handle it inside `caller`
   = a row that names operations permits a call only when the callee's row names them too: widen this row to the mode atom, or narrow the callee's row
```

The rule in one line: **a caller's row must cover what the callee's row can
perform, atom by atom.** A written row on a published function is therefore a
real interface — you can narrow it later without breaking a caller, and widening
it is a breaking change.

## The counts beside an atom

A row also says how many times. Each atom is either `bounded`, performed a number
of times no input decides, or `scaling`, growing with an input:

```ply
fn lookup(id: Int) -> Row / {db.read[conn]} = db.get[conn](id)

fn lookup_all(ids: List<Int>) -> List<Row> / {db.read[conn]} = map(ids, lookup)
```

```text
     lookup      : (Int) -> Row
                   / {db.read[conn] bounded}
     lookup_all  : (List<Int>) -> List<Row>
                   / {db.read[conn] scaling}
```

`map` calls `lookup` once per element, so `lookup_all`'s atom scales; the checker
knows this because the callback is called inside a `map` over a list whose length
is not written out. You never write `bounded` or `scaling` in the row of an
ordinary function — it is inferred. A definition may **promise** `bounded` in its
own written row:

```ply
fn cheap(id: Int) -> Row / {db.read[conn] bounded} = db.get[conn](id)
```

A body that performs a promised-bounded atom a scaling number of times is `E0465`,
which names the operation and the iteration that repeats it — the fix is to batch
the operation over the whole input, or move it out of the iteration. `bounded`
belongs to a definition's own row; in a function type or an effect set it is
`E0466`. A function in another package is read by what its row writes: an atom it
does not promise `bounded` may scale, and so may each callback it takes unless its
row variable is promised.

This is what lets a caller see a cost property in a signature. "This handler
performs exactly one query per call" is a checked fact, not a comment.

## `nondet`

A `nondet` effect's results are not a function of program state. A deterministic
test that retains a `nondet` atom after handling is `E0412`, because its verdict
would depend on the clock or the network. Declare the effect `nondet` when that
is true of it, and read chapter 13 for `test/nondet`.

## Effect sets

An effect set names a group of atoms so a signature need not list them:

```ply
effect set Persist = {db.read[users], db.write[users]}
effect set Full    = {Persist, log.write[app]}

fn through_set(id: Int) -> Unit / {Persist} = copy(id)
```

The printed row expands the set into its atoms, so `through_set` above reports
exactly the two `db` atoms. Sets may nest, across modules too, and may not hold a
row variable. `pub` exports a set: another module imports it by name and each
atom means what it means in the module that declared the set, which is how the
standard library groups a capability in one place.

## Why this matters

A signature that names every effect a function can have is a complete answer to
"what does calling this do?" — no body to read, no call graph to trace, no
hidden global. It is what lets:

- a test answer a function's needs and be hermetic by construction (chapter 9);
- a build know what a program can touch, and refuse a host operation nothing
  bound (`E0424`, chapter 19);
- `parallel` prove that two computations cannot interfere (chapter 12);
- a cached test pass mean something, because what the test read is known.

The cost is that rows spread: a helper that performs an effect makes its callers
perform it too. Narrowing a helper's row is therefore a real design act, and
`ply check --types` is how you watch it happen.

> **Try it.** Run `ply check --types` on the `db` example and read `through_set`'s
> row. Add an operation to `db`, then try to write a handler for `copy` before
> reading chapter 9.

## Summary

- `effect name { read/write/raise op[r](...) -> T }`. `[r]` is a resource label.
  `nondet` marks results that are not a function of program state.
- A row is a set of atoms with an optional tail: `/ {a, b | e}`.
- Inferred rows are exact; a written row is an upper bound and is checked
  (`E0302`).
- A mode atom covers every operation of its mode; an operation atom covers one,
  and a call must fit inside the caller's row atom by atom.
- `bounded` and `scaling` are inferred counts; a definition may promise
  `bounded`.
- An `effect set` groups atoms and expands into them.

Next: what turns a row back into an ordinary function — the handler.
