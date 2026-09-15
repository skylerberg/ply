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

**Built.** It exists in every place a builtin lives: the evaluator's
`Builtin` and its arity table, the persistent list in `ply-eval`, the trie
in `ply-codegen` with `rt_list_set` appended to the helper table, the Rust
emitter's builtin-to-helper mapping, the Ply emitter's, the Ply checker's
builtin type table, the prelude's published signature, `docs/GUIDE.md` §6,
§13 and §19, a `tests/lang/lists` program on both tiers with its raising
fixture, and the tests that walk every builtin. It took two pull requests,
because the bootstrap bundle has to know a builtin before the compiler's
own sources may use it: the first added it everywhere and left `set_at`
alone, and its refresh was the first to start from a bundle emitted against
an older helper table, which is §1a's measure met; the second made each
module's `set_at` answer `list_set`.

**Measured.** The emit-diff own-sources solo's test step read 66 s on the
run that landed it against 71 to 106 s over the four runs of `main` before,
a band too wide for one reading to settle, and the record leans on the
profile instead. `profile.yml` on the hash differential read 18.2 s against
19.9 s before, with the hasher's `set_at` gone from the profile and its
`at` halved. On the emitter over its own sources it read 77 s, with the
heap's `dismantle` at a quarter of the time where it had been a twentieth:
ADR 0049 item 2's quarantine, a queue threaded through the dead blocks and
bounded by tens of megabytes, was touching its oldest block cold on every
eviction. Two forms were then read on their own branches: a ring of a
thousand slots kept outside the blocks ran the same test in 64 s but held
fifteen gigabytes where the queue had held two, and no quarantine at all,
immediate reuse in every build, ran it in 62 s in 730 MB, the memory the
test had before any quarantine existed. Why a shorter quarantine holds more
memory the readings did not say. The quarantine is gone; every build reuses
at once, and ADR 0049 item 2 says so in place.

## 2. The goldens the differentials need before the reference goes

Each differential compares a Ply phase's dump of an input against the Rust
reference's dump of it, over the mined `.corpus` bundles in
`crates/ply-compiler-diff/fixtures`, the lexer fixtures, `examples/` and
the standard library. The Rust side is the oracle, and the day it retires
the oracle is gone. The dumps it produces today are the specification the
Ply compiler is held to; they should be in the tree as files before that
day, with the harness able to read them.

**Built.** `ply_compiler_diff::golden::check` holds every comparison to a
file under the harness's `fixtures/goldens/<phase>/`: a bundle's records
share one file, one record per `%%% <i>` line, and every whole-file input
has a file of its own. `PLY_DIFF_BLESS=1` writes the goldens from the
reference, and `.github/workflows/bless.yml` does that on a runner for a
named branch and hands them back as an artifact, since the reference is
what CI runs. Without it, each differential asserts the reference against
the golden **and** the port against the golden, so a golden that drifts
from either is red, and the port-against-golden half is what survives the
reference; each phase's own first-difference report says where. The
arming scripts run the same tests, so a mutant is caught against the
golden. The emit and lower differentials are not held to goldens: the
bootstrap bundle is theirs.

Measured on the first bless, 2026-09-15: the goldens as text weighed 50 MB,
of which the resolver's were 27 MB and the hasher's 11 MB, because each of
those dumps the standard library again for every example. Those two phases
keep the digest of each dump over its first record instead
(`golden::DIGESTED`), and a mismatch there is read against the reference's
dump while there is one. The rest is text, 13 MB in all: the parser's,
the rewrites', the checker's and the lexer's between two and four
megabytes each, the deriver's and the published-interface round trip's
under a quarter of one.

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
