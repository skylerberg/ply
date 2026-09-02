# ADR 0036 — After the gate: what ADR 0035's rule named, revisited

**Decided and landed; the series says how far it went.** ADR 0035's re-take
put both kernels over its bar and its own rule named what to revisit:
Decisions 2 and 5 for the integer kernel, Decisions 1 and 4 for the record
kernel. This record is the first pass over those four, taken as what the
profiles pointed at rather than as a redesign, and it changes no
representation ADR 0035 decided: the words, the layouts, the counts, the
strings, the list and the seam all stand. `benches/value-model/after-tokens.txt`
is the series after it and `benches/front-end-whole/observation-5.txt` the
front-end row, both under their own pre-registrations. What they say: the
record kernel sits at the bar's edge, within its resolution of it; the
integer kernel is outside it by a smaller factor than at the re-take — its
round is straight-line arithmetic over one record rebuilt in place since
Decision 8, and what remains is that arithmetic, every add checked and every
word masked as the source writes it, and the records the source keeps alive by
shape; and the whole front end under the backend is under a second of wall
time where it was several before ADR 0035 and tens interpreted, every phase
entered whole, and every test entered whole since Decision 7.

> **What this decides.** That a builtin the checker can type is a load in a
> register rather than a call; that a callback over a range or a list is a loop
> in the body that calls its step directly rather than a closure the runtime
> calls back; that a record update copies by offset, never by name, when it
> cannot write in place; that an entry's dead memory is reused within the
> entry, so an entry's memory is bounded by what it holds; and that the seam
> remembers a pure root's answer as compiled code does and takes it back in
> as the word, so a phase's tree crosses the seam once rather than at every
> root it is handed to; that the map is an ordered tree, so no map
> operation's cost grows with the map's size on a property the source does
> not show, which ADR 0034 asks of every core operation; that a test is a
> root the backend enters whole; and that a record dying in a body that
> builds another of its width is that record's memory.
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

## Decision 4 — an entry's dead memory is reused within the entry

ADR 0035's allocator bumped a pointer and gave memory back only at the entry's
end, so an entry's memory was bounded by what it ever allocated rather than by
what it held: a long entry — a whole compile, a loop of a few million steps
building a record each — grew without bound, and the list gate's array
refusal (ADR 0035, step 6) was memory before it was time. A dead object now
goes back to its heap's free list for its size class, from the header its
allocation site wrote, and an allocation of that class takes it before the
bump pointer moves; a bridged object does not, because its slot is on the
heap's drop log and a second bridged value in the same slot would be dropped
twice at the entry's end. A release build reuses; a debug build does not, so
every suite still reads a stale word as a dead header rather than as someone
else's object — the net the counts are checked under — while the examples
under the audit, the kernels and the front-end row run the reuse. A
five-million-step churn of one record holds its memory flat where it grew by
the step before.

## Decision 5 — the seam remembers what compiled code remembers

A pure nullary root entered from the interpreter was run and its answer
converted every time, while the same function called from compiled code was
answered from the memo; and the tree a phase answers was converted out and, at
the next phase's entry, converted back in. The seam now memoizes a pure nullary
root as compiled code does — the answer copied into the tables' own heap, the
value it is converted to kept beside the word — and takes that value back in
as its word when a body hands it to the next root; a call whose arguments are
all such words is a pure function of remembered inputs and is remembered in
turn, up to a bound, and the parts of a remembered answer one level down —
the fields a body pulls out of a record or a constructor, the elements of a
short list — carry the words they came from. The interpreter memoizes the same
functions, so the two arms of the front-end row do the same work now, and the
census says what still crosses: the answers of roots that take arguments the
memo does not hold, which is what a driver written in Ply, entered once, would
never convert at all. What does cross is walked once: the conversion notes on
the way whether it read a handle, which the seam refuses to carry out, rather
than walking the answer again to ask.

## Decision 6 — the map is an ordered tree

ADR 0034's representation gate, asked of the compiled map as it was asked of
the list, refused the sorted array: a shared insert copied the whole map, so
its cost per operation grew with the size, and at a few thousand keys the
compiled map was slower than the interpreter's tree. The front end's maps are
exactly that case — in the check row most inserts are into a shared map of
hundreds to thousands of entries, and the hash row's index build moves
kilobytes per insert. The map is now an ordered B-tree over words
(`crates/ply-codegen/src/map.rs`): leaves of sorted key–value pairs, branches
of up to thirty-two children beside each child's greatest key, an insert or a
removal walking one path — in place along what is held once, one node copied
per level where it is not — a probe walking the same path with a binary
search at each node, and iteration in key order, which is the interpreter's.
A small map is a map object and one leaf; a map built from sorted entries at
the seam fills its leaves left to right with no path walked.

## Decision 7 — a test is a root

The front-end row's shape — three roots entered from an interpreted test body
per row, each phase's tree converted out and back in — was the seam's share
of every row, and Decision 5 removed only the part the memo could hold. A
test body is now a root: the seam synthesizes a nullary definition of it,
named by the test's place among its module's tests, the fragment compiles it
like any other function, and the test runner offers that root to the backend
before interpreting, taking the backend's answer as the pass. A backend that
declines — a body the fragment refused — leaves the test to the machine, which
raises the diagnostic as before. One that ran the body and raised, an
assertion included, is not folded into a decline: the machine runs the body
too, and where it passes the test fails as a disagreement, because a body
entered whole is the one place a wrong compiled answer never crosses the seam
and the rerun would otherwise hide it. A corruption wrapper leaves every test
to the machine for the same reason from the other side: it corrupts answers at
the seam, and a test entered whole hands it none. This is the shape a
program's entry point takes: the whole test runs compiled and nothing crosses
the seam but a unit.

## Decision 8 — a dying record is the next one's memory

The integer kernel's profile after Decision 7 was nine tenths runtime helpers,
and the largest was the release of a sixteen-word record that the next round
allocated again at once: every round's state died at its last field read,
was walked and recycled, and was re-allocated for the round's answer. Two
things stood behind that. The inliner had opened an inlined call's parameter
block into the caller but not the callee's own block under it, so the callee's
answer stayed a record bound to a block and the scalar replacement never saw
it; and a record literal written entirely from a dying record of another
shape was lowered as an update of it, which the runtime's copy path served.
Now: the flattening opens every block the inliner made, so a small callee's
answer is scalars in the caller; a callee too small to be anything but a
mask or a shift is inlined at any depth, since the budget's depth ran out
exactly on those; and the compiled body keeps Perceus's reuse tokens — one
slot per width of record literal the body builds, filled by a record of that
width at its last use when nothing else holds it, taken by the next literal
of that width, which rewrites the header and the fields in place, and
released at the exit when nothing took it. A record whose fields are all
scalars is kept with no call at all; one holding words lets them go first.
A fully written literal no longer reads its base: a base at its last use is
released into its width's token and one still held is left alone, and the
update helper serves only a literal that copies fields. The integer kernel
moved by more than half on this, and what remains of it is the compiled
arithmetic itself and the records the source keeps alive by shape — a
chaining value copied out of a state that stays for its last block.

## Priced and rejected

A wider inlining budget. ADR 0035's integer kernel keeps its state in records
the inliner leaves because the round is over its budget, so the budget was
widened enough to admit the round. The integer kernel got *slower* — a body
seven rounds deep is more than the register allocator does well by — and code
generation over the examples took several times longer. The budget stays.
Widened by half with one more level of depth, after Decision 8, it bought the
integer kernel a fifth for a code generation nearly twice as long over the
examples and the suites; the register allocator's time over the larger bodies
is the whole of that cost. The budget stays at that reading too, and the
tiny-leaf rule takes the part of the depth that mattered.

## What would make this wrong

- **A step that is neither a compiled function nor a literal.** A callback
  passed a closure held in a variable still goes through the runtime's loop;
  the front end's own code takes that shape rarely, and the census of steps is
  what would say if that changed.
- **The seam's share of the front-end row.** It is the probe's shape, not the
  model's: three roots entered from the interpreter per row. Decision 5 takes
  most of it — what a remembered root answered goes back in as the word — and
  what remains is the answers of roots over arguments the memo does not hold,
  which only a driver entered once removes (`docs/BOOTSTRAP-PATH.md` step 10).

## Provenance

The profiles are the ones ADR 0035's re-take was read by, over the same probes;
the record-update count was taken with a counter on the helper's two paths for
one run and removed. The budget's pricing is in ADR 0035's sequence step 5.
