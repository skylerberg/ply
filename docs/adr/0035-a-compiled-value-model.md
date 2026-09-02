# ADR 0035 — A compiled value model: layouts from types, counts without atomics, reuse

**Decided; sequence steps 1 to 3 are landed, and the gate is not yet met.**
`benches/value-model/PRE-REGISTERED.md` is the gate's protocol and the bar is
in `benches/value-model/analyze.py`, where a number cannot set it after the
fact; `baseline.txt` there is the series before anything was built,
`after-words.txt` the series after the words landed and `after-layouts.txt`
the series after the layouts did. What landed: calls between compiled
functions are direct and typed; a compiled value is one word, with records,
constructors, lists, maps and native closures laid out as counted objects
allocated by bumping a pointer over memory the entry recycles, and everything
else bridged; a field of a record whose type the checker fixed is read at its
offset, and one read by name finds its offset in a table after the first
read; a remembered constant is copied into memory that outlives the entry.
What the third series says: both kernels are still over the bar, the integer
one at a small fraction of its baseline distance and the record one within an
order of magnitude of Rust, and the whole front end under the backend takes
roughly a quarter of what it did before this record. Where the integer
kernel's remaining distance is: not in any one helper any more but in the
record built and torn down per mixing step, which only inlining the step and
keeping the record's fields in registers removes — a compiler pass over the
lowered code, sequence step 5 below. What is next before it: step 4's drops,
so that a value's holder count says what it should and an update or an append
writes in place. `docs/BOOTSTRAP-PATH.md` step 9 carries this record's place
in the path and step 10 what is built on it if the gate clears.

> **What this decides.** That the representation compiled Ply code runs on today
> is an interpreter's, that no exception carved out for a hot function changes
> that, and that the fix is a second value model designed once for compiled
> code — layouts fixed from the checker's types, scalars unboxed wherever the
> type is known, reference counts without atomics, and reuse where a value is
> unique — with the interpreter's model kept whole as the oracle and the seam
> converting between the two at an entry's root.
>
> **What it does not decide.** Whether the model competes. That is the gate, and
> it is stated against Rust on two kernels before either has been built.

## What compiled code runs on today

The code generator keeps only `Int` and `Bool` unboxed inside a body
(`Kind` in `crates/ply-codegen/src/jit.rs`). Everything else is a handle into an
arena of the interpreter's values. A record is an atomically reference-counted
vector of name–value pairs searched by binary search on every field read; a
field read is a call into the runtime that clones the field's symbol, searches,
and pushes the answer into a fresh slot (`rt_field`); a list is a radix trie of
atomically counted nodes; a callback step hands its state through the arena on
the way in and takes it back out on the way out (`rt_fold`, `call_value`); and
every argument of every call, compiled callee included, is a handle. That
representation is the right one for the machine that walks a tree, which is what
it was designed for (ADR 0019). Driven from native code, it is what the
whole-front-end row and the profile beside it show: the compiled frames are the
minority of the checker's time and the runtime's helpers, drops, drains and
allocations are the rest (`benches/front-end-whole/profile-check-wide.txt`,
`docs/BOOTSTRAP-PATH.md` step 7).

**The cost is not in arithmetic.** It is in records, lists and calls, which is
why a builtin for any one hot function — the hash was the candidate — changes
the row without changing the fact. ADR 0033 named that builtin as its
throughput follow-up; this record retires it (Decision 7), because the hash is
the cleanest integer kernel the tree has and hiding it would hide the signal the
checker still carries.

## What the surveyed languages do, and the line they hold

Immutable values with reference counting is a model that competes when three
things hold, and each of the languages that competes holds all three. **Koka**
and **Lean 4** keep a uniform boxed word for polymorphic positions and unbox
scalars wherever a type is known, count references without atomics, and reuse a
dying cell for a new one of the same shape (Perceus, which ADR 0034 adopted for
the machine). **Roc** follows the same line. **OCaml** before them fixed every
block's layout from its type and stored an immediate integer unboxed. None of
them searches a record by name at run time, none of them pays an atomic
increment to share a value within a thread, and none of them routes a call
through an arena. **Go**, the language the bar is meant to stand for, has a
collector and mutable structs; where an immutable language matches it is exactly
the case a compiler's front end is made of — one state record threaded through a
loop and updated in place because nobody else holds it — and where it loses is a
heavily shared pointer graph, which a front end's data is not.

Nothing in Ply's semantics stands against that line. Values are immutable,
effects are explicit, and record types are structural but fully known to the
checker at nearly every read. ADR 0034's last-use ownership is already the first
half of reuse; what it asked for and did not get was "a flat record
representation … statically known wherever the type is", and this record is
where that arrives, for compiled code.

## Why the interpreter's value is not narrowed instead

ADR 0019 §4 refused to narrow the interpreter's value type, on a measurement,
and that refusal stands. The machine, the memo, the host boundary, the seam's
checks and every audit in the tree read `ply_eval::Value`; changing it changes
every engine at once, and the interpreter is the oracle the differential corpus
holds compiled code to. The seam was built to convert at an entry's root
(ADR 0030), which is precisely what lets the two models differ: compiled code
gets its own representation, the interpreter keeps its own, and the conversion is
paid once per root entry rather than once per value.

## Decision 1 — layouts from types

A record type and a constructor get a fixed layout decided at compile time from
the checker's published types: fields at known offsets, scalar fields stored
inline, pointer fields as words. A field read at a site whose type is known is a
load at an offset. A site whose type is a row variable — a function polymorphic
in its record — reads through a shape descriptor passed with the value, which is
Lean's rule for polymorphic positions and is rare in a front end whose state
records are named types. The code generator's `shapes` table, today keyed on the
field names an update writes, becomes the layout table.

## Decision 2 — scalars unboxed wherever the type is known

`Int`, `Bool`, `Float` and `Unit` live in registers, in record and constructor
fields and in list leaves without a box or a tag when their type is known. A
polymorphic position carries a uniform word: an immediate for a value that fits
the immediate range and a boxed cell otherwise, Lean's scheme, so that a
polymorphic `map` over `List<Int>` is not a list of allocations. `Kind` grows
from three cases to the typed set.

## Decision 3 — reference counts, per object and without atomics

A compiled value is counted per object with a plain increment. A compiled value
never crosses a thread: the simulation scheduler is single-threaded by
construction (ADR 0006), a test worker owns its machine, and the only path out of
compiled code is the seam, which converts. A constant — a literal, a remembered
nullary definition — is immortal and never counted. Strings and bytes stay
reference-counted buffers with slicing, under the same counts.

## Decision 4 — reuse, given the layout it needed

ADR 0034's Decision 3 adopted drop-reuse and said the flat layout was its
prerequisite. With Decision 1 in place: an update of a record whose count is one
writes in place; a constructor applied where a cell of the same layout dies at
the same point takes that cell; an append to a list whose count is one writes in
place. Uniqueness is the count at run time, as ADR 0034 chose, so a multi-shot
resumption degrades to a copy rather than breaking.

**The list's structure is a candidate, not a decision.** ADR 0034 refused a
representation whose worst case is unbounded, and an array whose shared append
copies the whole list is that representation. What landed is that array — an
append writes in place when nothing else holds the list and copies otherwise —
because it is the simplest thing the words could carry, and ADR 0034's
bounded-worst-case gate has not been run against compiled code. Until it is,
the array is the candidate that record priced and not a decision, and the trie
with typed leaves is the fallback if the gate refuses it.

## Decision 5 — calls are direct and typed

A call from compiled code to compiled code passes scalars in registers and other
values as words, under a signature the code generator derives from the type;
no handle, no arena slot, no argument vector. A callback is a direct call through
the closure's code pointer with its captures as leading arguments, which is the
closure shape ADR 0030's lambda lowering already built. The arena and its
handles remain at the seam, where the interpreter hands a value in and takes one
out.

## Decision 6 — the seam converts at the root, and the census counts it

Entry converts the arguments from the interpreter's values to the compiled
layout; exit converts the answer back. The conversion is deep and paid once per
root entry, which is what the front end's entry pattern is now — every phase's
root is entered whole and nothing under it declines (`benches/front-end-whole`).
The seam census counts the words converted, so an outcome where conversion
dominates is visible rather than inferred.

## Decision 7 — no builtin stands in for a slow kernel

ADR 0033's throughput follow-up, a hash builtin, is retired. Measured under the
gate the interpreted hash fell short of the bar that record set, which was
"not worse than the interpreted lexer" (`benches/hash-throughput`); measured under
the backend it clears the same bar several times over. The bar was set when the
interpreter was the only engine. What that measurement says is not "add a
builtin" but "the compiled model is slow on integer kernels", and BLAKE3 in Ply
is kept as exactly that kernel (K1 below). A builtin remains available as a
measured reversal if, with this model landed, hashing is still what keeps the
front end out of the loop — and it would then be taken for a reason this record
would have to state.

## What this does not do

**No linear or uniqueness types**, for ADR 0034's reason: they conflict with
multi-shot handlers and Koka is the existence proof that the performance does not
require them. **No change to the interpreter's value**, for ADR 0019's. **No
tracing collector**: deterministic destruction is what regions, cells and the
dismantler's bounds are built on, and reference counting with reuse is the route
every surveyed immutable language took. **No effects in compiled bodies**: a body
that performs is still refused by the fragment, and the continuation question
belongs to the C target's own record (`docs/BOOTSTRAP-PATH.md` step 10). **No C
emission here**: emitting C onto today's representation would move a slow model
to a different host, which is why this record comes first.

## The gate, registered before the measurement

Two kernels, each a Ply program and the Rust a competent engineer would write
for the same algorithm, timed the same way from outside the process under
ADR 0030's protocol — counterbalanced arms, a null control, minimum user CPU
over blocks, the load gate before and after, the binary checked current on both
sides. The Ply arm is the program under `--backend cranelift` through
`ply test`; the Rust arm is a release binary in the tree that runs the same
kernel over the same input and prints its time, so the two are compared on one
machine in one sitting and never against a figure written down elsewhere.

- **K1, integers: BLAKE3.** `std.hash.blake3` over a fixed input against a
  scalar Rust transliteration of the same module — the same rounds, the same
  masks, no SIMD — because the question is what the compiled model costs per
  operation, not what a vectorised library achieves.
- **K2, records: a threaded state.** A fold that carries one record of several
  fields — counters, a list, an ordered map, bytes — through every element and
  updates some of them each step, which is the shape the checker's state takes,
  against the same loop in Rust over a struct updated in place.

**The bar is a factor of three over Rust, on both kernels, and it is registered
here and in `benches/value-model/analyze.py`.** Three is where "competes with
Go" is credible for a language with a collector or counts and without Rust's
control over layout; ten is where it is not. **The decision rule:** both within
the factor and the model stands, and step 10 is planned on it; K1 outside and
Decisions 2 and 5 are what to revisit before anything else, because an integer
kernel exercises nothing else; K2 outside and Decisions 1 and 4 are, because a
state loop is layouts and reuse; both outside and the model is refuted as
designed and this record says so in its opening.

**The outcome measure is the front-end row**, re-taken on the model under its
own pre-registration (`benches/front-end-whole`), and its decision rule is
unchanged. The kernels decide whether the model is right; the row decides
whether it was enough.

**Taken first on today's fragment**, before anything is built, so the series has
a baseline the change is read against and the kernels are known to run.

## Sequence

1. **Landed.** The kernels and their Rust bars, under `benches/value-model`, and
   the baseline series on today's fragment.
2. **Landed, the calling half.** Direct and typed calls (Decision 5), with an
   `Int` or a `Bool` in a register and everything else as its word; a callback
   is still a call through the runtime's loop.
3. **Landed.** Words, with records, constructors, lists, maps and native
   closures as counted objects (Decisions 1, 3 and 4 as far as the layout
   goes) allocated by a bump pointer the entry recycles, and a record's fields
   at the offsets its shape fixes — read at a known offset where the checker
   fixed the type, through a per-shape table otherwise.
4. Drops at a scope's end and reuse of a dying cell, so that uniqueness is the
   common case a state loop sees rather than the exception.
5. Inlining a small pure callee into its caller over the lowered code, and
   keeping a record that never escapes in registers, which is what removes the
   record built per mixing step from the integer kernel.
6. Native strings and bytes; the list's array candidate priced under ADR 0034's
   gates, or the trie with typed leaves if it is refused.
7. The seam's conversion and its census (Decision 6).
8. Both kernels and the front-end row re-taken, and the decision rule applied.

Each step is behind the differential corpus and the fragment's own cases, as
every backend change has been.

## What would make this wrong

- **Layouts from types are unavailable at enough sites.** If the front end's
  reads go through row variables more often than through named types, Decision
  1's fast path is the rare path and the shape descriptor is the common one; K2
  would say so, and the answer is monomorphisation, not a return to name-keyed
  records.
- **Values cross threads more than Decision 3 assumes.** Then either a
  conversion at that boundary or an atomic count on the values that cross, and
  the census is what would show it.
- **Reuse rarely applies because the state is shared at its update sites.** ADR
  0030 already found the spike passing a record and reading a field of it in a
  later argument; if that shape dominates, the fix is the spike's code or a
  borrow analysis in the code generator, and K2 taken over the spike's own
  functions would locate it.
- **The kernels clear and the row does not move.** Then the cost is at the seam
  or in the callbacks' dispatch rather than in the values, and Decision 6's
  census is the instrument that says which.
- **The Rust bars are written to be slow.** The bars are in the tree, read by
  anyone, and the transliteration rule for K1 is stated above so a reviewer can
  hold it to the same algorithm.

## Provenance

The whole-front-end row and the profile it led to are this record's evidence
(`benches/front-end-whole`, both observations and the profile file, with
`docs/BOOTSTRAP-PATH.md` step 7 carrying the decision the row forced). The
survey above is the same one ADR 0034 made for the machine, read again for
compiled code. The hash throughput series that retired ADR 0033's follow-up is
`benches/hash-throughput/observation-1.txt`.
