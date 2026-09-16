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
`rt_field`, `call_value` and the count traffic, and that profile is flat:
no lever is in it, so the levers are aimed by what the compiler allocates
rather than by where its time goes.

**Read, 2026-09-15: what the emitter is made of.** `PLY_C_PHASES` says,
at an entry's end, how many objects of each kind it allocated and, one
level down, which constructors, which record shapes and which sizes of
bytes. Over the emitter's own sources, of two hundred million objects:
the constructor `Some` alone is three tenths; bytes with room for one
byte or none, the empty literal a fold starts from and the one character
a lexer step keeps, a sixth; closures an eighth; records a tenth, the two
largest shapes inference's `{name, ty}` and `{args, name}` at under three
per cent each; list headers a tenth and the trie's nodes one in a hundred;
`Stop` a twentieth; `tycore.TyCon` three per cent. The hasher reads the
same shape: `Some` three tenths, the small bytes a quarter, records a fifth
with its own `{a, b, c, d}` state a tenth of the whole, lists an eighth.
`None` and every other nullary constructor is a singleton already.

The three levers this record first named were the profile's, and the
census re-aims two of them. Records with unboxed scalar fields change no
allocation: a scalar is an immediate or an immortal already, every count
helper returns on one before touching memory, and a record is one object
whatever its fields are. A growable array beside the trie addresses the
trie's nodes, one object in a hundred. Both stay below, measured and
deferred, behind what the census weighs. The levers, in its order:

1. **The accumulator appended in place.** §1 found the compiler's byte
   accumulators copied whole each step. The heap already appends in place
   to a value held once; the lever is the join the dumps go through, so
   that the dumps every phase hands to the next are built by one buffer
   growing. The census is its measure: the chunk bytes under a delayed
   reuse of a thousand blocks, and the objects allocated.

   **Built, 2026-09-15.** The second count was never held: the emitted C
   compiles `bytes_concat_all([acc, ..])` to `rt_bytes_join` over the
   pieces alone, and that join allocated a fresh buffer for the whole every
   time, whoever held the pieces. It now hands a first piece held once to
   `Heap::append`, which writes after it with room doubling, and copies
   the other pieces once each; the interpreter's list arm does the same
   when the list alone holds its first piece. The heap gained the other
   diagnostic mode the measure needs, `PLY_HEAP_DELAY=<n>`: a dead block
   waits `n` releases before an allocation may take it.

   **Measured, 2026-09-15.** Under a delay of a thousand, the emitter over
   its own sources held 16.5 GB of chunks and 15.4 GB resident before, and
   334 MB of chunks and 0.75 GB resident after: the same as immediate
   reuse, which is what a value built by one buffer growing looks like
   under any reuse policy. The objects allocated moved by a tenth of a per
   cent, on the emitter and on the hasher alike, because the join was a
   few hundred thousand of two hundred million allocations. The own-sources
   solo's test ran in 35–43 s against 52 s on main in the same hour, and
   the hasher's first part in 11 s against 16 s, both from single runs in
   a band that is wide. The lever is kept for the delayed-reuse figure,
   which was the one that named it.

2. **A bytes value of one byte or none is an immortal singleton.** The
   census named it: a sixth of the emitter's objects and a quarter of the
   hasher's were bytes with room for one byte or none.

   **Built, 2026-09-15.** The heap holds a static empty bytes object and
   one per byte value, immortal by their count, and answers every such
   value from them: at `Heap::bytes`, at every join, and at
   `bytes_concat_all` over pieces that total one byte or none. An append
   to one copies into a fresh buffer, as an append to any shared value
   does; a fold that starts from `b""` therefore allocates once more than
   it did and copies nothing more.

   **Measured, 2026-09-15.** The emitter over its own sources allocated
   167.6 million objects against 199.4 million, a sixth fewer, and the
   hasher's first part 46.5 million against 54.6 million; the chunk bytes
   and the resident memory did not move, because the blocks were recycled
   before. The census file carries the new counts.

3. **`Option` answered without its `Some`.** Nine builtins answer
   `Option`, and the compiler's sources unwrap `list_at` and `map_get` at
   once in some six hundred places: seven copies of one helper, `at(xs,
   i)`, that matches `list_at` and panics on `None`, the direct calls, and
   the map reads. Each builds a constructor that the next line takes apart
   and nobody releases. The runtime has `rt_list_lookup` and
   `rt_map_lookup`, which answer the element held once more, or zero for
   none, with no constructor between; neither is in the helper table, so
   no emitted C reaches them. The lever is a fused `match` on a `list_at`
   or `map_get` call whose arms are `Some(x)` and `None`, in both
   emitters, over those two helpers; the sentinel is zero, which is also
   what a failed call answers, so the fused site checks `ctx->failed`
   before it reads zero as `None`. The measure is the `Some` line of the
   census and the objects allocated.

   **Built, 2026-09-16.** Both emitters compile the match to the helper,
   the `Some` arm binding the answer itself with no increment and the arms
   keeping the general path's shape; the two helpers joined the table at
   its end; the emit differential's corpus arms either arm order, a name
   or a wildcard, and a module's own `list_at` that must not take the
   path. Two traps on the way are now closed: the differential entered the
   port through the checked-in bundle, which served the old emitter as
   long as the new helper table extended the old, so it asserts first
   that the bundle was emitted from the tree's sources; and the bundle
   refresh converged in one round, which a change to what the emitter
   emits cannot do, so it emits from each emission until two agree.

   **Measured, 2026-09-16.** The emitter over its own sources allocated
   107.9 million objects against 167.6 million, a third fewer; `Some` fell
   from 62 million to 2 million, and the constructors from two fifths of
   the whole to a fifth. The hasher's first part allocated 32.1 million
   against 46.5 million. The chunk bytes and the resident memory did not
   move. The census file carries the new counts.

4. **The loop step's `Stop` and `Continue` unwrapped through `match`.**
   `fused_iterate` writes a step's payload straight into the loop's
   variables when every exit is a written `Stop(..)` or `Continue(..)`,
   and `fusable_step` sees through `if` and a block but not through
   `match`; the parser's loops are `match`-tailed, so they build the
   constructor and peel it. The lever is `fusable_step` descending through
   `match` in both emitters; the measure is the `Stop` line.

5. **A named function passed to `map` or `filter` without a closure.**
   The emitter boxes a named function into a closure over nothing at every
   `map` and `filter` call so that `rt_map` can enter it, one object per
   call; `fold` and `iterate` are fused and pay nothing. The recorded
   attempt to fuse `map` and `filter` (the note above `fused_fold`) freed
   a list a name still read, and the next needs the release keyed on the
   object; the census weighs this lever an eighth and the record leaves it
   named, not scheduled.

6. **Records with unboxed scalar fields**, measured and deferred: no
   allocation to move, and the count helpers already return on an
   immediate. What remains of it is a helper call per `Bool` field read
   and a walk of every field at release for a record that mixes scalars
   with boxed fields, which is nearly every record the compiler has.

7. **A growable array beside the trie**, measured and deferred: the trie's
   nodes are one object in a hundred. The list headers, a tenth, are one
   per list value however it is stored.

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
in the order the census weighs the levers, one at a time, so each reading
is one lever's; the profile could not order them, being flat, and the
census by name could.
