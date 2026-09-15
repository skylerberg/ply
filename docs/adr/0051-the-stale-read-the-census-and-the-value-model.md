# ADR 0051 — The stale read, the census, and the value model measured

**Accepted as an ordering.** ADR 0050 took the one algorithmic lever its
profile named and read the compiled compiler's heap twice over: a dead
block held back from reuse for any while, whether the queue was threaded
through the blocks or kept outside them, made the emitter over its own
sources hold twenty times the memory that immediate reuse holds, and the
readings did not say why. It also said what immediate reuse holds is not a
good result: hundreds of megabytes and some two hundred million objects to
compile fewer than thirty thousand lines, where a lean compiler takes tens
of megabytes and single-digit millions. This record takes the three pieces
of work those two findings name, in the order they have to go: the reason
for the first, because whatever it is would be built on; a figure for the
second that a test holds; and then the value model, one lever at a time,
held to that figure.

> **What this decides.** That the heap gains a diagnostic mode, turned on
> by a test or an environment variable and shipped by nothing, in which a
> dead block is never reused, its payload is poisoned at release, and any
> read of it through the runtime fails at the body's own site. That the
> compiled compiler's cost is stated as objects allocated, objects recycled
> and chunk bytes at an entry's end, for the emitter over its own sources
> and for the hasher, read apart from the harness and the reference, kept
> in `benches/` where a test holds them within a band, and compared per
> source line with a lean compiler's. And that the value model moves by
> three levers, each on its own branch, each read by `profile.yml` and the
> census before it merges, each kept only if the figures move.
>
> **What it does not decide.** What a lever looks like beyond its measure;
> that is settled on its branch. Whether the poison mode stays after it has
> found what it was built for; it stays if it is cheap to keep armed and
> goes if it is not, and the record says which.

## 1. Find the stale read

A heap that holds more memory the sooner a dead block leaves its
quarantine is not a healthy heap: it says something read a dead block and
acted on what it found, and that immediate reuse survives that read by
luck. The debug build's old net, keeping every dead block until the entry's
end so a stale read found a `DEAD` header, caught reads through the
runtime's counted paths and nothing else, and it is gone. What is needed is
a net that names the site.

**Built when.** `PLY_HEAP_POISON=1`, read once, puts every heap made after
it into the mode: a dead block goes to no free list, its payload words are
overwritten with a word that is not an immediate and points at a static
poison object whose header is `DEAD`, and every runtime helper that takes
an object checks its header first and fails, in this mode, with a
diagnostic placed at the body's stored site and naming the helper. The
emitted C's own guards fall to the runtime on a `DEAD` kind already. The
mode is run over the own-sources solo and the hash differential through
`profile.yml -f ref=<branch>`, with the variable passed by a new input, and
what it names is fixed; the record carries the site and the mechanism. If
it names nothing on either test, the record says so, says what was tried,
and the question stays open in ADR 0050 §1b where it was raised.

## 2. Measure the compiled compiler alone

`PLY_C_PHASES=1` already prints, at every entry's end, what the entry
allocated, what it recycled and how many bytes of chunks it holds. Those
three are the compiled compiler's cost in the units the value model is
about; resident memory of the harness process, which also runs the
reference and holds both emitted units as text, is not.

**Built.** `ply_codegen::c::producer::census()` accumulates, per thread,
what the compiled compiler's entries allocated and recycled and the most
chunk bytes one held; the own-sources solo and the hash differential's
first part read theirs after the comparison they already make and hold
them to `benches/compiled-compiler.json`, within one per cent on the
counts and a quarter on the bytes. The file was written from a runner's
first reading, and a change to the value model is held to it from here.

**Measured, 2026-09-15.** The emitter over its own sources, one entry over
the compiler's and the standard library's twenty-eight thousand lines:
two hundred million objects allocated, ninety-eight per cent of them
recycled within the entry, and 334 MB of chunks at the end. That is seven
thousand objects and twelve kilobytes of chunk per source line. The hasher
over the standard library and half the examples, seven entries of some
nine and a half thousand lines each: fifty-five million objects, ninety-four
per cent recycled, 32 MB of chunks, about eight hundred objects per line
per entry. For the same lines, as magnitudes, rustc and clang are in the
low hundreds of objects and a few kilobytes per line, and a lean compiler
in the tens of objects and under a kilobyte; the emitter is an order of
magnitude past the heavy ones and two past the lean, in objects, and the
recycling is what keeps the chunks to hundreds of megabytes rather than
gigabytes. The allocation count, not the resident memory, is the figure
the value model is held to.

## 3. The value model, measured

Every profile of the compiled compiler ends at `dismantle`, `raw_alloc`,
`rt_field`, `call_value` and the count traffic. Three levers the profiles
name, in the order the readings suggest they pay:

1. **Records with unboxed scalar fields**, so a record of `Int`s and
   `Bool`s is built without a count on each field and read without a
   helper per field; the emitter already knows a `FLAT` record and the
   shape table already says which fields are scalar.
2. **A growable array beside the trie**, for the build-then-read shape most
   of the compiler's lists have: pushed to once, indexed many times, never
   shared while being built. The trie stays for what is shared.
3. **A byte builder** that appends into one buffer, for the dumps every
   phase of the compiler hands to the next as bytes and today assembles
   through thousands of intermediate concatenations.

**Built when.** Each lever lands on its own branch, in two pull requests
where it adds a builtin or a helper (the bundle before the sources that
use it), and is read by `profile.yml` and the part-2 census before it
merges; a lever that does not move the census is not merged, and the
record says what each one moved.

## The order, and why

1 before everything, because a stale read is a defect in the heap, and the
value model is a change to the heap. 2 before 3, because a lever without a
figure is a feeling, and ADR 0050 §3 says a change made for speed cites a
reading. 3 in the order the profiles weigh the levers, one at a time, so
each reading is one lever's.
