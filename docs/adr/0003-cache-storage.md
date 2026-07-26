# 3. Cache storage

Status: accepted (interface landed, implementation outstanding)

## Context

CONTRACTS.md justified JSON on disk: *"this is not the bottleneck and being able
to read the cache by hand is worth more than speed."* That was written when the
cache held pass/fail bits keyed by hash — a file a person could actually read.

It is now two caches in one directory, and the second one is not that file. At
10,000 definitions the front-end cache is a single ~15 MB JSON document that is
read whole at `Store::open` and rewritten whole whenever any one definition
changes. On the generated corpus that profiles at ~70 ms of a 205 ms warm
`ply test`, plus ~50 ms more on any run that changed something — the largest
single cost in a run whose entire premise is that almost nothing needs doing.
(The prototype below measures 18 ms and 16 ms for the same two operations on a
faster machine; the ratio is what matters, and it is the same.) Nobody has ever
debugged anything by reading 15 MB of serialized type schemes, so the thing the
format was chosen for is not being bought.

The two caches also have opposite characters, which one format cannot serve:

| | result cache | front-end cache |
| --- | --- | --- |
| size at 10k defs | tens of KB | 15 MB |
| read | whole, once | a few hundred entries of tens of thousands |
| written | a few entries per run | rewritten whole per run |
| read by a human | *yes* — "why didn't this test re-run" | no |
| key | test `DefHash` | path, and `DefHash` |

## Decision

**The result cache stays JSON.** It is small, `cat .ply-cache/results.json`
answers the one question people actually ask a cache, and its parse cost does
not appear in a profile. Nothing below changes it.

**The front-end cache becomes a binary content-addressed store**: a small index,
rewritten whole and atomically, over an append-only data file that is mmap'd and
whose entries are decoded on demand.

### Immutability is the whole argument

A `DefHash → entry` mapping can never change; that is what content addressing
means. Entries are therefore only ever *added*. So the data file is append-only
and is never rewritten, and the ~50 ms whole-document rewrite becomes an append
of a few kilobytes. Removal is not a mutation but a garbage collection, and it
happens when a person asks for it (`ply cache compact`), not on the edit-test
path.

Append-only buys a second property worth as much as the first: **a reader needs
no lock.** Appending never moves a byte another process has already mapped, so a
reader's `(offset, len)` stays valid while a writer works. Only writers
serialize.

### Files

Under `<root>/.ply-cache/`:

```
results.json    outcomes and the definitions seen. JSON, rewritten whole, read at open.
passes.json     the pass records. JSON, rewritten whole, read only on the first question.
frontend.idx    the index: header + sorted directories. Rewritten whole, atomically.
frontend.dat    append-only entry data. mmap'd. Only `ply cache compact` rewrites it.
lock            as today, and now mandatory for writers
```

(`passes.json` is the addendum's doing, not this section's original design; see
the end of this document for why the result cache had to be split in two.)

### `frontend.dat`

A 56-byte header, then frames appended forever:

```
header  0  magic     8   b"PLYFEDAT"
        8  format    u32 FRONTEND_FORMAT
       12  flags     u32 0
       16  schema    32  the schema fingerprint (below)
       48  nonce     u64 random, generated when the file is created

frame   0  kind      u8  1 def · 2 decl · 3 body · 4 source fingerprint
        1  len       u32 payload length
        5  checksum  u64 first 8 bytes of blake3(kind ‖ len_le ‖ payload)
       13  payload   len bytes
```

The length prefix and the checksum are what make a torn append *detectable*
rather than misparsed. A frame is only ever read through an index entry that
already claims its `(offset, len, kind)`; a read that disagrees with the frame
header, or whose payload does not match the checksum, yields "not cached" and a
warning. It never yields a value.

`nonce` exists so an index cannot be paired with a data file it was not written
against — the case where someone deletes one of the two files, or restores one
from a backup. Both files carry it; disagreement discards the front-end cache.

### `frontend.idx`

A fixed 132-byte header, a section table, and the sections:

```
  0  magic         8   b"PLYFEIDX"
  8  format        u32 FRONTEND_FORMAT
 12  flags         u32 0
 16  schema        32  the schema fingerprint
 48  nonce         u64 must equal the data file's
 56  data_len      u64 every entry lies wholly below this
 64  version_hash  32  blake3(FRONTEND_VERSION)
 96  sections      u32 number of section descriptors
100  checksum      32  blake3 of every byte from 132 to EOF
132  descriptors   sections × { kind u32, count u32, offset u64, bytes u64 }
     payloads
```

Sections, each sorted so it can be binary-searched in place:

| kind | record | sorted by |
| --- | --- | --- |
| 1 `DEFS` | `{ hash [32], offset u64, len u32, flags u32 }` | hash, then offset |
| 2 `DECLS` | as `DEFS` | hash, then offset |
| 3 `BODIES` | as `DEFS` | hash |
| 4 `SOURCES` | `{ path_off u32, path_len u32, offset u64, len u32, flags u32 }` | path bytes |
| 5 `PATHS` | the path string blob | — |

`DEFS` and `DECLS` admit repeated hashes: two structurally identical definitions
in different modules share a `DefHash` and their interfaces still differ, so a
hash owns a *run* of slots. A binary search lands anywhere in the run and walks
back to its start. `BODIES` admits exactly one record per hash, because a body
is name-free and is therefore a function of the hash alone.

`Store::open` mmaps both files, checks both headers agree on format, schema,
version and nonce, and verifies the index checksum. It builds no map and decodes
no entry. A lookup binary-searches the mmap'd section, bounds-checks the
`(offset, len)` against `data_len`, validates the frame, decodes the payload,
and memoizes the result for the rest of the run.

Fingerprints are deliberately *not* in the index: they are five of the seven
megabytes, and keeping them in the data file is what holds the index to half a
megabyte at 10,000 definitions.

### Measured, not guessed

Apple M4, macOS, release build, warm page cache. A throwaway prototype built
both layouts over a synthetic cache with the real shapes — 10,000 definitions
across 400 files, 12.75 MB as JSON — and timed them:

| operation | JSON today | binary store |
| --- | --- | --- |
| `Store::open` | 1.4 ms read + 17–19 ms parse | **0.015 ms** header only, **0.21 ms** with the index checksum |
| index size | — | 0.50 MB |
| data size | 12.75 MB | 7.70 MB |
| one interface | 0, all resident | 0.002 ms |
| 200 interfaces | 0 | 0.23 ms |
| all 10,000 interfaces | 0 | 10.7 ms |
| all 400 fingerprints | 0 | ≤ 5.2 ms |
| persist a one-definition change | 13–16 ms serialize + 12.75 MB write | 0.015 ms append + 0.3–0.5 ms index write |
| `fsync` | 4–8 ms, and paid by both | 4–8 ms |

`Store::open` at 0.21 ms is 25× inside the 5 ms budget, and the budget is what
was measured against: the checksum is 0.2 ms per half-megabyte of index and
scales linearly, so it is verified unconditionally rather than behind a size
threshold.

Two of these numbers deserve to be stated rather than buried:

- **The fingerprint number is an upper bound.** The prototype encodes
  fingerprint payloads as JSON, because what mattered there was their size. A
  tagged binary payload is strictly cheaper.
- **A fully warm run that materializes every interface saves almost nothing.**
  10.7 ms of decode against 18.4 ms of parse. The unconditional wins are open
  (18.4 → 0.21 ms) and persistence (16 → 0.5 ms); the *decode* win only arrives
  when the loader stops materializing interfaces it will not consult. Today's
  driver builds a `DefInfo` for every definition in the program on every run,
  so it materializes all of them. Making that lazy is follow-on work in the same
  milestone — the store API makes it possible by handing entries out one hash at
  a time, and nothing in this ADR requires it. Even without it the binary store
  is never slower than what it replaces.

### The store now holds definition bodies

DESIGN.md §3 promises a codebase is `Hash → (Definition, Type, Footprint)`. Only
the type and the footprint were ever stored. M5's bisection has to *evaluate* a
historical set of definitions, which is impossible without their bodies, so the
third element lands now:

```
bodies : DefHash -> DefBody          # opaque, versioned, canonical bytes
```

The encoding lives in `ply-hash`, beside the normalizer it mirrors, and
`ply-store` treats it as opaque bytes with an encoding version. The rules that
make it a *stable canonical serialization keyed by `DefHash`*:

1. **The body is a function of the hash.** Two definitions with the same
   `DefHash` encode to the same bytes. This holds by construction: the body
   stream is the normalizer's stream — de Bruijn levels for locals, the
   referent's hash for a free reference, names and spans erased.
2. **It is self-checking.** For a definition that is its own component,
   `blake3(bytes)` *is* the key. For a member of a mutually recursive component
   the bytes are the component's and the key is
   `blake3(component ‖ index_le_u32)`. Either way a stored body can be verified
   against the key it is filed under, which is why `put_body` refuses one that
   does not — a body store cannot corrupt silently.
3. **Decoding yields an evaluable definition, not the user's source.** Names are
   synthesized (`_l0` for a local level, `d_<hash12>` for a reference); spans
   come back as `Span::DUMMY`; item order, `pub` and module membership are gone.
   A reconstituted program is resolved from its own synthesized namespace, which
   is exactly what makes it independent of what anything is called *now* —
   the property bisection needs, since the names moved while the hashes did not.
4. **Normalization's rewrites are baked in.** Commutable `let` runs are
   reordered, an effect's operations sorted, record *type* fields sorted, a
   `{ e }` wrapper dropped. A decoded definition is therefore equal to the
   original up to those rewrites, which are semantics-preserving by the same
   argument that lets them into a hash in the first place.

### Versioning is the risk, and gets three interlocking gates

Today a schema drift that `FRONTEND_VERSION` misses produces a loud JSON parse
error and degrades to an empty cache. With a non-self-describing binary encoding
the same drift deserializes into plausible garbage — a wrong type, a wrong
footprint — and footprints drive test scheduling, so garbage there silently
corrupts which tests are allowed to run concurrently. That is a class of bug
this project cannot ship, so drift is caught three times over:

**At compile time.** `ply_store::schema` names every variant of every stored
type through exhaustive `match`es with no wildcard arm. Adding a variant to
`Type`, `Resource`, `Mode`, `DeclBody`, `DefKind` or `Outcome` fails to compile
until it is named, and a coverage test then fails until an exemplar value
mentions it. A contributor cannot reach the next gate without having noticed.

**At test time.** `schema_fingerprint()` is BLAKE3 over `FRONTEND_FORMAT`,
`BODY_ENCODING` and the encoding of that exemplar set. A pin test compares it to
a constant and, when it differs, prints the new digest and says to bump
`FRONTEND_VERSION`. This is the mechanism the doc comments used to ask for
politely: the digest is computed *from the encoder*, so it cannot go stale the
way a hand-maintained list does.

**At run time.** Both file headers carry the fingerprint, the format number and
`blake3(FRONTEND_VERSION)`. Any mismatch discards the front-end cache with a
warning and rebuilds. Below that, every frame carries a length and a checksum,
so a torn or overwritten entry is detected per entry rather than at the file
level.

The rule for a contributor is unchanged in spirit and now enforced: a change to
any *stored shape* is caught by the pin; a change to inference or normalization
that leaves the shapes alone is still a judgement call and still requires bumping
`FRONTEND_VERSION` by hand.

### Writing

A writer takes the cache lock, and — this is a change — a writer that **cannot**
take it writes nothing and warns, where today it proceeds unlocked. Two
processes appending to one file interleave frames; that is corruption, not a
lost update, and it is not worth a lock-free heroic. Readers still take nothing.

Under the lock:

1. re-read the index from disk; it may be newer than the one mapped at open.
   Its `data_len` and `nonce` are authoritative.
2. truncate `frontend.dat` to that `data_len`. This is how a torn tail left by a
   killed writer is recovered: no indexed entry ever lies above `data_len`, so
   nothing indexed is lost. A data file *shorter* than `data_len` is an
   inconsistent pair — discard both files and start fresh, with a warning.
3. append this run's new frames; `fsync` the data file.
4. union the directories. `DEFS`, `DECLS` and `BODIES` are content-keyed, so a
   foreign entry is as good as a local one and the union is always sound.
   `SOURCES` is last-writer-wins, which costs at worst a parse because a
   fingerprint is never believed until its `content_hash` is checked against the
   bytes on disk.
5. write the new index to a temp file, `fsync`, rename, `fsync` the directory.

The data file is made durable before the index that names it, which is the whole
ordering requirement: an index entry may never point at bytes that are not
there. The reverse — data present that no index names — is garbage, and garbage
is what compaction is for.

### Compaction

Append-only grows without bound, so `ply cache compact` exists, and never runs
by itself.

An entry is **unreachable** when nothing in the index names it after pruning to
the files that exist now:

- a `SOURCES` record whose path is not among the `.ply` files discovered by a run
  that saw the whole root — the same precondition `prune` already carries;
- a `DEFS`, `DECLS` or `BODIES` record whose `DefHash` is declared by no
  surviving fingerprint;
- every *superseded* record: an older fingerprint for a path that was written
  again, an older slot for the same `(hash, name)`. These are unreachable by
  construction, since the index only ever points at the newest.

Compaction copies the reachable records into a fresh data file in directory
order, generates a new nonce, writes a matching index, and renames both under the
lock. `ply cache stats` reports live bytes, garbage bytes and the ratio, and
suggests compacting past 50%; it does not act. The reason to keep it manual is
that dropping an interface costs a recheck, and the definitions most likely to
be garbage are the ones most likely to come back — a commented-out function, the
other side of a branch.

### `ply cache inspect <def>`

This is what recovers the hand-readability that JSON was being kept for, and it
is strictly better than `cat`, because it can print a resolved type rather than a
serialization of one:

```
$ ply cache inspect active_users
user.active_users  9f2c1a4b7e03  fn  src/user.ply
  type       (Int) -> List<User> / {db.read[users]}
  footprint  {db.read[users]}
  witness    user.User → 4a1f0b2c9d55, store.db → 77bd3e10c4a2
  body       412 bytes (encoding 1)
  result     —  (not a test)
```

The query is a program-wide name, a simple name, or a hash prefix of at least
four hex characters. Several matches print several entries rather than erroring:
for a prefix that is what was asked for, and for a simple name declared in two
modules it is the answer. No match is `E0101` with a note that a definition
appears here only after a run that checked it. `--json` emits the same as an
array.

### Migration

There is no reader for the old format and there will not be one. On the first
run after this lands, `Store::open` finds a `frontend.json` and no
`frontend.idx`, and:

- warns (`W0603`) that the front-end cache format changed, that this run
  recomputes types and hashes for the whole project, and that **the result cache
  is untouched, so no test re-runs**;
- proceeds with an empty front-end cache and rebuilds it;
- deletes `frontend.json` at the next successful flush.

Degrading to empty is the answer, deliberately: the cost is one full check of
the project — the `cold` column of the benchmark, seconds, not minutes — the
alternative is a JSON reader that must be kept correct forever to save that one
run, and the user is told exactly what happened and that their test results
survived. `FRONTEND_VERSION` bumps to `0.4.0` in the same change, so a build
that somehow did read the old file would reject it anyway.

## Consequences

**A borrowed interface cannot survive.** `cached_def` and friends hand out
`&CachedDef` into a map that holds every entry. A store that materializes an
entry from a mapped byte range on demand has nothing to lend, so the accessors
become `Arc`-returning: `def`, `def_of`, `decl`, `decl_of`, `fingerprint`. The
alternative — a frozen arena behind interior mutability, to keep faking a
reference — trades a clear ownership story for `unsafe` and an unbounded memo,
and buys only the absence of a mechanical call-site change. `Arc` also lets a
materialized entry be handed to a worker thread, which the borrowed form never
could. The borrowed accessors stay for now as thin wrappers and are removed by
the change that lands the mmap, which is the same change that must touch their
call sites anyway.

**Two megabytes of the win are conditional.** Restated because it is the number
most likely to be quoted without its condition: open and persistence get faster
unconditionally and by an order of magnitude; the decode-on-demand saving needs
a loader that asks for fewer interfaces than it does today.

**Concurrent writers now serialize or skip.** A run that cannot take the lock
within the bounded wait keeps its work in memory and warns, where today it wrote
anyway and risked losing another process's entries. Losing an entry costs a
recheck; interleaving frames costs the file.

**The front-end cache is no longer readable by hand, and `jq` no longer helps.**
`ply cache inspect` and `ply cache stats` are the interface. This is the trade
being made deliberately, and it is only defensible because the *result* cache —
the one whose contents answer a question a person asks — stays exactly as it is.

## Alternatives rejected

**SQLite, redb, sled.** A dependency, a storage engine and, in SQLite's case, a
query language, to serve a map from 32 bytes to a blob whose only write is an
append. The append-only file gets durability and lock-free readers out of the
filesystem for a few hundred lines.

**`bincode` / `postcard` over the existing `serde` impls.** Far less code, and
the exemplar digest would still catch drift. Rejected because the shape of a
serde-derived binary encoding is *implicit in field declaration order*: swapping
two fields of the same type is a silent wire change, and the decoder has no
per-field tag it could reject on — it would read the new bytes as the old shape
happily. A tagged encoding, in the style `ply-hash` already uses for
normalization, can refuse.

**One file per entry.** `open` becomes a directory walk, 20,000 inodes is
hostile to the filesystems this runs on, and per-entry `fsync` costs more than
everything the design saves.

**Keeping JSON and parsing it lazily** (`serde_json::value::RawValue`, or an
index into the text). Still reads and scans 15 MB at open, still rewrites whole
on any change, and adds a second, subtler format. The parse is not the only cost
— the whole-document rewrite is the other half, and laziness does nothing for it.

**Storing definition bodies as source text.** Re-parsing is slower than decoding
and, worse, makes a body's identity depend on formatting — a body would then
differ where its hash does not, which is precisely the property the whole system
is built to avoid.

## Addendum: measured against `ply-store`

The prototype numbers above have now been reproduced against the real store, on
a `ply-corpus` project of 10,000 definitions across 202 files. The front-end half
landed as designed: `Store::open` reads its two file headers and verifies the
index checksum in **0.45 ms**, flat across 50 edit-and-run cycles, and the store
is invisible in a profile of a warm run except where the *loader* asks it for
every interface in the project.

The whole of `Store::open`, however, measured **14.9 ms** — three times the
budget. The offender was not the front end. ADR 0004's pass record is a whole
closure per test, and 4,996 of them made `results.json` 9.5 MB, of which 7.4 MB
was records that only a *failing* test ever reads. The premise this ADR argued
from — "the result cache is small, tens of KB, and its parse cost does not appear
in a profile" — had stopped being true one ADR later, and the budget was being
checked against the front end alone.

So the result cache is now two files, `results.json` and `passes.json`, and the
pass records are read on the first question rather than at open. `results.json`
gains a `format: 2`; format 1 is still read and its inline records are relocated
by the first flush, so no test re-runs.

| operation, 10,000 definitions | before | after |
| --- | --- | --- |
| `Store::open`, whole | 14.91 ms | **2.08 ms** |
| `Store::open`, front-end half | 0.45 ms | 0.45 ms |
| `Store::open`, result half | 14.56 ms | 1.61 ms |
| `results.json` | 9.57 MB | 1.23 MB + 8.34 MB of `passes.json` |
| rewrite the results after an edit | 68.9 ms | 13.7 ms |

Two things this measurement found that the design did not predict:

- **Persisting a change is fsync-bound, not layout-bound.** Appending one entry
  and rewriting the index costs 13–17 ms and is *flat* from 100 to 10,000
  definitions, where the ADR predicted 0.3–0.5 ms of index write over 4–8 ms of
  fsync. The whole-document rewrite is gone; the durability floor is what is
  left, and it is higher than assumed on APFS.
- **The result cache grows without bound and nothing reclaims it.** Fifty
  edit-and-run cycles over a hub definition took it from 1.23 MB to 6.90 MB of
  results and drove `Store::open` back to 11.0 ms, because `compact` deliberately
  leaves the result cache alone. `compact` reclaimed 92% of a 194 MB data file in
  149 ms, so the append-only half is fine; the JSON half has no retention policy
  at all.

## Not done here

The binary format, `ply_hash::body`, `ply cache compact` and `ply cache inspect`
have all landed. What has not:

- **Lazy materialization in the loader.** This is now the largest single cost in
  a warm run, not a theoretical one. At 10,000 definitions with nothing to do,
  `Driver::merge` is 38% of the run rebuilding a `CheckOutput` for every
  definition in the program, and gate 1's witness check is another 19% decoding
  every stored interface — 57% of the run spent materializing what nothing asked
  for. The store hands entries out one hash at a time, as this ADR promised; the
  loader still asks for all of them.
- **A retention policy for the result cache.** See above.
- Removing the borrowed accessors and their call sites.
