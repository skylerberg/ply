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

**Built.** `PLY_HEAP_POISON=1`, read once, puts every heap made after it
into the mode: a dead block's payload words are overwritten at release
with a word that points at a static object whose header is `DEAD`, and
every object read through `heap::obj` fails, naming the body's stored site
as module and byte range, when it meets that word or a dead header. The
mode as first built also kept every dead block; over the emitter's own
sources that is two hundred million blocks, the runner sent its shutdown
signal before anything was read, and the mode now lets a poisoned block
be reused, so the net is the window before the block is taken again.
`profile.yml` took an `env` input to run it, and prints a failing run's
last lines whole, since twice the reading was on the line its filter
dropped. Under the mode the hasher and the emitter over its own sources
both pass, in the memory immediate reuse holds; the mode names nothing,
and it stays, its cost being one cached flag test per object read.

**Found, 2026-09-15: there is no stale read.** The census settled it. A
plain queue that holds each dead block back behind a thousand others
reproduces the reading exactly: the emitter over its own sources ends with
16.5 GB of chunks against 334 MB under immediate reuse, and recycles the
same number of objects, 195.7 million either way. Every delayed block
comes back; what grows is the size of the allocations that miss. That is
the shape of a value built by appending, where each step frees a buffer of
size S and allocates one a little larger: under immediate reuse the freed
buffer is the next allocation's, and under a delay of N there are N copies
of it live at once, and for the emitted unit's own text S is sixteen
megabytes. `Heap::append` already writes in place, doubling its room, when
the value is held by nobody else; so somewhere the compiler's accumulators
are held twice at the append and copied whole each step, which is a
quadratic copy hidden by reuse, not a defect of the heap. Where they are
held twice is §3's first lever to find, and the question opened in ADR
0050 §1b is closed with that.

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
`rt_field`, `call_value` and the count traffic, and §1's finding puts one
lever ahead of the others. Three levers, in the order the readings say
they pay:

1. **The accumulator appended in place.** §1 found the compiler's byte
   accumulators held twice at the append and copied whole each step. The
   heap already appends in place to a value held once; the lever is
   whatever holds the second count, in the emitted C's ownership or in the
   sources' shape, so that the dumps every phase hands to the next are
   built by one buffer growing. The census is its measure: the chunk bytes
   under a delayed reuse of a thousand blocks, which today read fifty times
   immediate reuse's, and the objects allocated.
2. **Records with unboxed scalar fields**, so a record of `Int`s and
   `Bool`s is built without a count on each field and read without a
   helper per field; the emitter already knows a `FLAT` record and the
   shape table already says which fields are scalar.
3. **A growable array beside the trie**, for the build-then-read shape most
   of the compiler's lists have: pushed to once, indexed many times, never
   shared while being built. The trie stays for what is shared.

**Built when.** Each lever lands on its own branch, in two pull requests
where it adds a builtin or a helper (the bundle before the sources that
use it), and is read by `profile.yml` and the part-2 census before it
merges; a lever that does not move the census is not merged, and the
record says what each one moved.

## The order, and why

1 before everything, because a stale read would have been a defect in the
heap, and the value model is a change to the heap; it was not one, and the
finding ordered §3. 2 before 3, because a lever without a figure is a
feeling, and ADR 0050 §3 says a change made for speed cites a reading. 3
in the order the readings weigh the levers, one at a time, so each reading
is one lever's.
