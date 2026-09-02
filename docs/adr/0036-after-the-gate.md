# ADR 0036 — After the gate: what ADR 0035's rule named, revisited

**Decided and landed in part; the series is what says how far it went.**
ADR 0035's re-take put both kernels over its bar and its own rule named what
to revisit: Decisions 2 and 5 for the integer kernel, Decisions 1 and 4 for
the record kernel. This record is the first pass over those four, taken as
what the profiles pointed at rather than as a redesign, and it changes no
representation ADR 0035 decided: the words, the layouts, the counts, the
strings, the list and the seam all stand. `benches/value-model/after-loops.txt`
is the series after it and `benches/front-end-whole/observation-4.txt` the
front-end row, both under their own pre-registrations.

> **What this decides.** That a builtin the checker can type is a load in a
> register rather than a call; that a callback over a range or a list is a loop
> in the body that calls its step directly rather than a closure the runtime
> calls back; and that a record update copies by offset, never by name, when it
> cannot write in place.
>
> **What it does not decide.** A new bar. ADR 0035's stands, and the series
> here is read against it.

## Where the time was

Three profiles, on the binary ADR 0035's re-take measured, each read by the
runtime symbols under the compiled frames rather than by the compiled frames
themselves, which the sampler cannot name.

- **The integer kernel.** Half of the record updates in the hash's chunk loop
  took the update helper's copy path with a base of another shape, because the
  lowering guesses an update's base from the first projected variable in the
  written fields and, once callees are inlined, that variable is a record of
  another type more often than not; the copy path resolved every field by name
  for it. Beside that: a builtin call per byte read, and the chunk loop's
  callback through the runtime.
- **The record kernel.** A key built per step from a two-element list literal
  and a one-byte value, a probe and an insert into the map, and the loop itself
  a fold over a range the runtime built as a list and walked calling the step
  back through its loop.
- **The front end's check row.** The callbacks — `fold`, `map`, `filter`,
  `iterate` — calling their steps back through the runtime's loop, and behind
  them the seam's conversion at the three entries each row makes, which the
  census (ADR 0035, Decision 6) put at about a sixth of that row: each phase's
  root receives the previous phase's tree as an argument and answers a larger
  one. That share is the row's shape, three roots entered from the interpreter,
  and not the model's; a driver written in Ply enters once.

## Decision 1 — a record update copies by offset

The fresh record takes the written fields at their offsets, copies the rest out
of a base of the same shape by offset, and resolves by name only for a base of
another shape that left a field unwritten — which a fully written literal never
does. The written offsets are a word of bits. The in-place path is unchanged.

## Decision 2 — a builtin the checker can type is a load

`bytes_at`, `bytes_len` and `len` answer inline: the argument's kind is checked,
the length or the byte loaded, and the answer is an `Int` register that joins
predict. Another kind, or an index out of range, enters the runtime's own path,
so the diagnostic stays the interpreter's. The one-byte values `byte_of_int`
answers are immortal singletons the unit makes once, and `bytes_concat_all`
over a list literal joins the pieces without the list.

## Decision 3 — a callback is a loop in the body

`fold`, `map` and `filter` over a `range` or a list, and `iterate`, are emitted
as loops in the calling body when their step is a compiled function or a lambda
literal: a range is walked as the integers themselves and a list by index; the
step is called through its typed body, or through its own entry with the
captured values as leading arguments, held for the loop and handed over once
per call as a closure would hand them. No list is built for a range, no closure
is allocated, and no call goes back through the runtime's loop. The
interpreter's limits hold — a range past its limit and an `iterate` that runs
out raise, and so decline — and the constructors `iterate` unwraps are checked
inline. Whether the step can be called is decided from its denotation before
anything is evaluated, so a step that is a constructor or a builtin takes the
runtime's path whole.

## Priced and rejected

A wider inlining budget. ADR 0035's integer kernel keeps its state in records
the inliner leaves because the round is over its budget, so the budget was
widened enough to admit the round. The integer kernel got *slower* — a body
seven rounds deep is more than the register allocator does well by — and code
generation over the examples took several times longer. The budget stays.

## What would make this wrong

- **A step that is neither a compiled function nor a literal.** A callback
  passed a closure held in a variable still goes through the runtime's loop;
  the front end's own code takes that shape rarely, and the census of steps is
  what would say if that changed.
- **The seam's share of the front-end row.** It is the probe's shape, not the
  model's, and the only thing that removes it is a driver entered once — which
  is `docs/BOOTSTRAP-PATH.md` step 10's, not this record's.

## Provenance

The profiles are the ones ADR 0035's re-take was read by, over the same probes;
the record-update count was taken with a counter on the helper's two paths for
one run and removed. The budget's pricing is in ADR 0035's sequence step 5.
