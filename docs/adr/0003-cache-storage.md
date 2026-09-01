# ADR 0003 — Cache storage

**Accepted, format implemented.**

## Context

JSON on disk was justified as "not the bottleneck, and being able to read the
cache by hand is worth more than speed." That was written when the cache held
pass/fail bits keyed by hash — a file a person could actually read.

It became two caches in one directory with opposite characters. The result cache
is small, read whole once, written a few entries per run, and is the one people
actually `cat` when they ask *why didn't this test re-run*. The front-end cache
at ten thousand definitions is a multi-megabyte JSON document read whole at open
and rewritten whole whenever any one definition changes — the largest single
cost in a run whose entire premise is that almost nothing needs doing. Nobody
has ever debugged anything by reading a megabyte of serialized type schemes, so
the thing the format was chosen for is not being bought.

**The result cache stays JSON.** Nothing below changes it. **The front-end cache
becomes a binary content-addressed store**: a small index, rewritten whole and
atomically, over an append-only data file that is mmap'd and decoded on demand.

## Immutability is the whole argument

A hash-to-entry mapping can never change; that is what content addressing means.
Entries are therefore only ever *added*, so the data file is append-only and is
never rewritten, and a whole-document rewrite becomes an append of a few
kilobytes. Removal is not a mutation but a garbage collection, and it happens
when a person asks for it, not on the edit-test path.

Append-only buys a second property worth as much: **a reader needs no lock.**
Appending never moves a byte another process has already mapped, so a reader's
`(offset, len)` stays valid while a writer works. Only writers serialize — and a
writer that *cannot* take the lock now writes nothing and warns, where it
previously proceeded unlocked. Two processes appending to one file interleave
frames; that is corruption, not a lost update, and it is not worth a lock-free
heroic. Losing an entry costs a recheck; interleaving frames costs the file.

Frames carry a length prefix and a checksum, which is what makes a torn append
*detectable* rather than misparsed: a read that disagrees with the frame header
yields "not cached" and a warning, never a value. A nonce in both file headers
pairs the index with the data file it was written against, for the case where
someone deletes or restores one of the two.

Fingerprints stay in the data file rather than the index. They are most of the
bytes, and keeping them out is what holds the index small enough to checksum
unconditionally on every open.

## Writing order

Under the lock: re-read the index from disk (it may be newer than the one mapped
at open, and its length and nonce are authoritative); truncate the data file to
that length, which is how a torn tail from a killed writer is recovered, since
no indexed entry ever lies above it; append and fsync; union the directories;
then write the new index to a temp file, fsync, rename, fsync the directory.

**The data file is made durable before the index that names it.** That is the
whole ordering requirement: an index entry may never point at bytes that are not
there. The reverse — data present that no index names — is garbage, and garbage
is what compaction is for.

## The store holds definition bodies

The design promises a codebase is `hash → (definition, type, footprint)`. Only
the type and the footprint were ever stored, and bisection has to *evaluate* a
historical set of definitions. So bodies land too, as opaque versioned canonical
bytes. Four rules make that a stable serialization keyed by the hash:

1. **The body is a function of the hash.** Two definitions with the same hash
   encode to the same bytes, by construction: the body stream *is* the
   normalizer's stream — de Bruijn levels for locals, the referent's hash for a
   free reference, names and spans erased.
2. **It is self-checking.** For a definition that is its own component,
   `blake3(bytes)` *is* the key; for a member of a mutually recursive component
   the bytes are the component's and the key is derived from it and the index.
   Either way a stored body can be verified against the key it is filed under,
   so a body store cannot corrupt silently.
3. **Decoding yields an evaluable definition, not the user's source.** Names are
   synthesized, spans come back empty, item order and module membership are
   gone. A reconstituted program is resolved from its own synthesized namespace,
   which is exactly what makes it independent of what anything is called *now* —
   the property bisection needs, since the names moved while the hashes did not.
4. **Normalization's rewrites are baked in.** A decoded definition equals the
   original up to commutable-`let` reordering, sorted operations and sorted
   record type fields, which are semantics-preserving by the same argument that
   lets them into a hash at all.

## Versioning is the risk, and gets three interlocking gates

Today a schema drift the version constant misses produces a loud JSON parse
error and degrades to an empty cache. With a non-self-describing binary encoding
the same drift deserializes into *plausible garbage* — a wrong type, a wrong
footprint — and footprints drive test scheduling, so garbage there silently
corrupts which tests may run concurrently. That is a class of bug this project
cannot ship, so drift is caught three times over.

**At compile time.** A schema module names every variant of every stored type
through exhaustive matches with no wildcard arm. Adding a variant fails to
compile until it is named, and a coverage test then fails until an exemplar
value mentions it. A contributor cannot reach the next gate without noticing.

**At test time.** A schema fingerprint is BLAKE3 over the format number, the
body encoding and the encoding of that exemplar set, compared against a pinned
constant. This is the mechanism the doc comments used to ask for politely: the
digest is computed *from the encoder*, so it cannot go stale the way a
hand-maintained list does.

**At run time.** Both file headers carry the fingerprint, the format number and
a digest of the version constant; any mismatch discards the cache and rebuilds.
Below that, per-frame checksums catch a torn or overwritten entry.

The rule for a contributor is unchanged in spirit and now enforced: a change to
any *stored shape* is caught by the pin; a change to inference or normalization
that leaves the shapes alone is still a judgement call and still requires
bumping the version by hand.

## Compaction is manual

Append-only grows without bound, so `ply cache compact` exists and never runs by
itself. An entry is unreachable when nothing in the index names it after pruning
to the files that exist now, including every superseded record. The reason to
keep it manual is that dropping an interface costs a recheck, and the
definitions most likely to be garbage are the ones most likely to come back.

`ply cache inspect` recovers the hand-readability JSON was being kept for, and
is strictly better than `cat`, because it can print a resolved type rather than
a serialization of one.

## Consequences

**A borrowed interface cannot survive.** A store that materializes an entry from
a mapped byte range on demand has nothing to lend, so the accessors return
`Arc`. The alternative — a frozen arena behind interior mutability, to keep
faking a reference — trades a clear ownership story for `unsafe` and an
unbounded memo, and buys only the absence of a mechanical call-site change.
`Arc` also lets a materialized entry be handed to a worker thread.

**The front-end cache is no longer readable by hand**, and that trade is only
defensible because the *result* cache — the one whose contents answer a question
a person asks — stays exactly as it is.

**The decode saving is conditional and the other two are not.** Open and
persistence get faster unconditionally and by a large factor. Decode-on-demand
pays only once the loader stops materializing interfaces it will not consult;
today it builds an entry for every definition in the program on every run. The
store hands entries out one hash at a time, as this design promised; the loader
still asks for all of them. That is now the largest single cost in a warm run.

Two things the follow-up measurement found that the design did not predict:
**persisting a change is fsync-bound, not layout-bound** — the whole-document
rewrite is gone and the durability floor is what is left, and it is higher than
assumed. And **the result cache grows without bound and nothing reclaims it**,
because compaction deliberately leaves it alone. The append-only half is fine;
the JSON half has no retention policy at all. Both are open.

## Alternatives rejected

**SQLite, redb, sled.** A dependency, a storage engine and, in one case, a query
language, to serve a map from 32 bytes to a blob whose only write is an append.
The append-only file gets durability and lock-free readers out of the filesystem
for a few hundred lines.

**`bincode`/`postcard` over the existing serde impls.** Far less code, and the
exemplar digest would still catch drift. Rejected because the shape of a
serde-derived binary encoding is *implicit in field declaration order*: swapping
two fields of the same type is a silent wire change, and the decoder has no
per-field tag it could reject on — it would read the new bytes as the old shape
happily. A tagged encoding can refuse.

**One file per entry.** Open becomes a directory walk, twenty thousand inodes is
hostile to the filesystems this runs on, and per-entry fsync costs more than
everything the design saves.

**Keeping JSON and parsing it lazily.** Still reads and scans the whole document
at open, still rewrites whole on any change, and adds a second, subtler format.
The parse is not the only cost; the whole-document rewrite is the other half.

**Storing bodies as source text.** Re-parsing is slower than decoding and, worse,
makes a body's identity depend on formatting — a body would then differ where
its hash does not, which is precisely the property the whole system avoids.
