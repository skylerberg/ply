# 11. Cells, regions and holds

Ply has no mutable variables. When a computation genuinely needs state — an
accumulator that several branches of a `match` update, a pool shared with tasks —
it asks for it explicitly, in a **region**. The region is a scope with a name,
and nothing allocated in it can leave.

## Cells

`with_cell[r](init) { c -> body }` allocates a cell for the duration of `body`, in
a region named `r`:

```ply
fn counted(n: Int) -> Int =
  with_cell[work](0) { c -> {
    cell_set(c, cell_get(c) + n);
    cell_get(c)
  } }
```

`cell_get`, `cell_set` and `cell_update` are builtins; their atoms, `cell.read[r]`
and `cell.write[r]`, never leave the region. In `counted` they are discharged at
the region's `}`, so `counted`'s own row is empty.

Nest regions for several cells, or reuse a name to allocate into the region that
is already open:

```ply
fn two() -> Int =
  with_cell[r](1) { a -> with_cell[r](2) { b -> cell_get(a) + cell_get(b) } }
```

A `cell_update(c, f)` applies `f` to the value and stores the result; if `f`
raises, the cell is left as it was. `cell_update` is the one to reach for when you
hold a value briefly — `cell_get` followed by `cell_set` reads and writes, and
chapter 6's copy rules make the difference visible in allocations.

## A cell cannot escape

A cell's type is branded by its region — `Cell[work]<Int>` — and the region
outlives nothing but itself. Returning the cell, storing it outside, or reading it
from a closure that outlives the region is `E0201`:

```text
Error[E0201]: the cell escapes its `with_cell[work]` region
  --> i.ply:2:40
   |   let c = with_cell[work](0) { cell -> cell };
   |                                        ^^^^ this has type `Cell[work]<Int>`
   = read the cell inside the region and return the value instead
```

The check is what makes a cell safe without a garbage collector: the region's `}`
frees the cell, so a reference outliving it would dangle. The same reasoning
brands `Task` and `Chan` by their region (chapter 12) and `Hold` by its region
(below), and the same diagnostic family `E0446` covers a value that outlives its
region through a type, a closure or an operation.

## Holds: the bracket

`with_hold[r](acquire, release) { h -> body }` is the bracket of a resource.
`body` runs with `h` a **hold** on what `acquire` answered, and at the region's
`}` `release` is called on what was acquired:

```ply
effect door {
  write open[r]() -> Int
  write close[r](h: Int) -> Unit
}

fn use_door() -> Int / {door.open[room], door.close[room]} =
  with_hold[room](door.open[room](), |h: Int| door.close[room](h)) { h ->
    hold_get(h) + 1
  }
```

`release` is any function of the acquired value, `(a) -> Unit / e`, so one form
serves every resource a library can open and close: the library exports the pair,
and no resource has a form of its own. `std.io`'s `reading` and `shut` are such a
pair.

`hold_get(h)` reads what the hold holds and puts `hold.read` in the row of
whatever calls it; the `with_hold` answers `hold.read` for its body, so
`use_door`'s row above holds only the door's own atoms. A function handed a hold
says `/ {hold.read}`, and a closure that reads one carries it in its type, which
is how the checker sees a way out of the region.

`release` runs **once**, where the region stands and with the handlers around it,
however the body ends:

- the body returned;
- a raise, or a clause that did not resume, unwound through it — before the
  clause that answers the raise runs;
- the task was cancelled.

`acquire` and `release` are a bracket's: a cancel takes no answer from the one and
no wait from the other, so what was acquired is released. An `acquire` that
raised holds nothing, so nothing is released. A `release` that raises replaces
whatever was unwinding. Holds nested in one another are let go innermost first. A
runtime failure ends the run and releases nothing.

```ply
test "a hold is released however the body ends" {
  handle {
    assert_eq(use_door(), 42)
  } with {
    door.open[room]() -> 41,
    door.close[room](h) -> (),
  }
}
```

`with_hold` is the bracket whose body cannot keep what it holds and which
releases once. `bracket(acquire, release, body)` is the general form whose body
is handed a plain value and may keep it past the release; it is in the prelude
and the reference covers it (§6.6).

## The naming rules

A hold is let go at the `}` of its own region, so that region is the hold's
alone. A `with_hold[r]` inside a region already named `r`, or a `with_cell[r]`
inside a `with_hold[r]`, is `E0330`:

```text
Error[E0330]: `with_cell[r]` opens a region named `r` inside one of that name
  --> d.ply:2:49
   |   with_hold[r](1, |h: Int| ()) { h -> with_cell[r](0) { c -> cell_get(c) } }
   |                                                 ^ this name is already a region's
   --> d.ply:2:13
   |   with_hold[r](1, |h: Int| ()) { h -> with_cell[r](0) { c -> cell_get(c) } }
   |             ^ `r` opens here and is still open
   = a hold is let go at the `}` of the `with_hold` that made it, so the region that brands it has to be that one and no other
   = give this region a name no region around it has
```

A continuation captured inside a `with_hold` body and resumed after the hold was
let go finds the hold gone: its first `hold_get`, or else the region's own `}`,
ends the run with `E0331`. Where what was acquired is a host's, an at-most-once
operation on it refuses the second resumption first (`E0426`).

A `Hold` is not data: it is not compared, ordered, hashed or derived over, and
`reflect` answers the cell it is. A closure whose row reads a hold may not leave
the region (`E0446`), which is why a lazy `Seq` built inside a region must be
consumed there.

## Why regions exist

A region is where Ply puts the trade-off that other languages leave to a garbage
collector or to the programmer. There is no tracing collector: an allocation is
freed when nothing can reach it. Cells and holds make that reachability visible
in the type, so a use-after-free is a diagnostic rather than a running bug, and a
program that wants shared mutable state has to say where that state lives and who
may reach it. The cost is the naming and the escape rules; the benefit is that
`parallel` and `simulate` can reason about what a computation touches (chapter
12).

A reference **cycle** is never freed — the runtime does not trace — and the
checker warns `W0610` when it can see one.

> **Try it.** Write a small counter using `with_cell` and a loop over `range`,
> then move the `cell_get` outside the region and read the escape diagnostic.
> Then rewrite it to return the count instead.

## Summary

- `with_cell[r](init) { c -> body }` gives a region-scoped cell;
  `cell_get`/`cell_set`/`cell_update` are the operations, and their atoms cannot
  leave the region.
- A cell's type is branded by its region and cannot escape (`E0201`, `E0446`).
- `with_hold[r](acquire, release) { h -> body }` brackets a resource; `hold_get`
  reads it and `release` runs however the body ends, once, innermost first.
- `hold.read` is discharged by the `with_hold`; a closure that carries it cannot
  leave the region.
- `E0330` is a region named inside one of the same name; `E0331` is a hold read
  after release. Cycles are never freed (`W0610`).

Next: many tasks at once, and a scheduler you control.
