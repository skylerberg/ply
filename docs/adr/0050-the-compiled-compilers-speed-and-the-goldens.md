# ADR 0050 — The compiled compiler's speed, the goldens before the reference goes, and the instruments that hold both

**Accepted as an ordering.** ADR 0049 left the suite at the shape where a
quiet run's pole is the emitter over its own sources: the compiled
compiler's own running time, in one solo job. It also found that the
differentials are the only statement that the two front ends are one
language, and that they retire with the Rust one and not before. This
record takes the three pieces of work those findings name, in the order
they have to go, and fixes the measure each is held to.

> **What this decides.** That a compiled unit carries the runtime helper
> table it was emitted against, and serves while the runtime's table starts
> with it, so a helper can be appended without taking the bootstrap bundle
> out of service. That the language gains `list_set`, an update of one
> element of a list at the cost of the trie path to it, in every
> implementation at once, and that the compiler's own sources use it where
> they rebuilt a list to change one element. That each mined corpus bundle
> the differentials read takes the reference's dump beside every input
> while the reference can still produce it, and that the harness can
> compare the port against the file, so nothing is lost on the day the
> reference retires. And that no performance claim lands from here on
> without a reading from `.github/workflows/profile.yml` or the build's
> timings artifact, named in the record that makes the claim.
>
> **What it does not decide.** The value model, which the profile says is
> where the counts and the field reads come from; that is a record of its
> own once `list_set` has moved what it moves. When the Rust front end
> retires; ADR 0042 orders that, and the goldens here are what make it
> possible without loss. Whether a larger runner is worth paying for.

## 1. The compiled compiler's own speed

`profile.yml`, taken 2026-09-15 on the hash differential and on the
emitter over its own sources, was flat: the heap's `dismantle` and
`raw_alloc`, then the compiler's own `at` and `set_at` over lists, then
`rt_field`, `call_value`, `list::get` and the count traffic, none above six
per cent. Of those, `set_at` is the one lever a small change moves. It is
written in the compiler's sources as a `map` over `range` that rebuilds
the whole list to replace one element, with a closure call per element,
and it is called from forty-four places across `infer`, `rewrite`,
`resolve`, `hash` and `tycore`. A builtin that copies the trie path to the
element instead is logarithmic where that is linear, and allocates a
handful of nodes where that allocates the list.

### 1a. The unit carries its helper table

A builtin the compiled tier answers needs a runtime helper, and the helper
table is a contract between the emitted C and the runtime: the C declares
one pointer per helper and `ply_bind` fills them by position. Today a
bundle records a digest of the whole table beside its C and does not serve
against any other table, so appending a helper takes the bootstrap bundle
out of service, and under tier-only the reference cannot rebuild it whole.
That is the wrong rule. A unit emitted against the first `n` helpers binds
the first `n` positions and reads no other; it serves against any table
that starts with its own.

**Built.** `Exports` carries the helper table the unit was emitted
against, one line per helper with its name, its argument count and whether
it answers, first in the table so it is the first thing read. `finish`
refuses a unit whose table the runtime's does not start with, naming the
first helper that differs, as an `Unserved` the callers tell from a unit
that is broken: the producer and the fixpoint test build the emitter with
the reference on it, and `ply run` leaves an artifact's unit aside with the
warning it gave before. `RUNTIME.digest` and `runtime_digest` are gone,
from the bundle, from the artifact's unit section and from the fixpoint
test's message; the body cache's key keeps a digest of the whole table,
since a cache may be conservative. The bundle in the tree was given its
table by transformation, and the fixpoint compares the C with it in. The
measure is the next item: `list_set`'s helper is appended and this bundle
still serves, so the fixpoint's refresh runs from it rather than from the
reference.

### 1b. `list_set`

`list_set(xs: List<a>, i: Int, v: a) -> List<a>` answers `xs` with its
element at `i` replaced by `v`, and raises `E0502` for an index the list
does not hold, as `bytes_at` does: a write past the end is a defect, not a
lookup that may miss. It is a builtin rather than a library function
because the trie's path copy is not expressible from outside the trie.

**Built when.** It exists in every place a builtin lives: the evaluator's
`Builtin` and its arity and purity tables, the pure interpreter, the
persistent list in `ply-eval`, the trie in `ply-codegen` with `rt_list_set`
appended to the helper table, the Rust emitter's builtin-to-helper
mapping, the Ply emitter's, the Ply checker's builtin type table, the
prelude's published signature, `docs/GUIDE.md` §13, a `tests/lang/lists`
program that states it on both tiers, and the builtin unit tests that walk
every builtin. Then every `set_at` in `crates/ply-compiler/ply` calls it,
and the bundle is refreshed from the tree by the fixpoint test. The
measures are the emit-diff own-sources solo's time and the hash
differential's, read from the run before and the run after, and
`profile.yml` on both tests after; the record says what moved. If the solo
does not move, the record says so and the builtin stays, since a
logarithmic update is right whatever it measures.

## 2. The goldens the differentials need before the reference goes

Each differential compares a Ply phase's dump of an input against the Rust
reference's dump of it, over the mined `.corpus` bundles in
`crates/ply-compiler-diff/fixtures`, the lexer fixtures, `examples/` and
the standard library. The Rust side is the oracle, and the day it retires
the oracle is gone. The dumps it produces today are the specification the
Ply compiler is held to; they should be in the tree as files before that
day, with the harness able to read them.

**Built when.** Beside each mined bundle sits a golden file with one dump
per record, in the record order, written by the harness from the
reference. For the whole-file corpora the goldens live under the harness's
`fixtures/goldens/`, one per input per phase. `PLY_DIFF_BLESS=1` rewrites
the goldens from the reference; without it, each differential asserts the
port against the golden **and** the reference against the golden, so a
golden that drifts from either is red, and the port-against-golden half is
what survives the reference. `first_difference` reports where, as now. The
arming scripts run the port-against-golden tests. The size of the goldens
is measured before they are committed and the record says what it was;
a phase whose goldens are too large to review is stored as the digest of
each dump beside the dump's first line, and the record says which phases.

## 3. The instruments stay honest

Two levers in ADR 0049 were guessed right and measured wrong: the prelude's
helpers as macros moved nothing, and mold made the build slower. Both were
caught only because a reading was taken on a runner before the change was
believed. `CONTRIBUTING.md` §"Performance" names the two instruments,
`.github/workflows/profile.yml` and the build job's timings artifact, and
the rule: a change made for speed cites a reading from one of them in the
record that lands it, or it does not claim the speed.

## The order, and why

1a before 1b, because 1b appends a helper and would otherwise brick the
bundle. 1 before 2, because 2 changes the harness's inputs and 1b changes
what the harness measures; each is easier to read alone. 3 throughout.
