# ADR 0038 — The invocation is the cost: what a resumable front end has to do

**Registered, started, and re-aimed by its own measurement.** It began by
claiming an invocation costs the project; a warm arm taken at three sizes over
two corpora says the cost is what the tests that must run *reach*, and that a
project whose tests are all deterministic is already flat in project size. The
question is therefore narrower than the one this record opened with, and it is
stated in §"The question". `crates/ply-cli/tests/suite/incremental.rs` holds the
gate, `driver::Resume` holds the state, and the parse phase no longer runs for a
file that still says what it said. The seam §"The question" names is not done.
ADR 0037 ordered the loop's work and its row answered the question it was
registered for. This record starts where that answer left off.

## What the row found, and the correction the warm arm made to it

`benches/marginal-change/` prices one edit at three project sizes. Two of its
readings are settled: a rename costs nothing, and a leaf edit is flat under the
interpreter across a project sixteen times larger. Selection works.

The third reading is what this record was written for, and **it was read wrongly
when it was written.** A warm `ply test` that rechecks nothing still paid a front
end proportional to the project, and this record took that to mean an invocation
costs the project. `benches/marginal-change/warm-loop.sh` takes the same
question inside a warm process, at three sizes, over two corpora that differ in
one property, and its `observation-warm.txt` says otherwise:

- **With every test deterministic, the cost of an edit does not grow with the
  project at all.** Across a project sixteen times larger it does not rise; the
  smallest of the three is the slowest, because what an iteration costs is what
  the edit *reaches*, and reach is a property of the program's shape rather than
  its size.
- **With the corpus generator's default fraction of nondeterministic tests it
  grows steeply** — and it grows with exactly the number of modules those tests
  pull in, which is what the same report's parsed count says.

The same corpus, warm, settles it in one comparison: running its
always-nondeterministic tests, it parses a hundred modules and pays a front end
of tens of milliseconds; with a filter that selects nothing to run, it parses
none and pays a sixth of that. Nothing about the project changed between those
two runs.

**So the invocation is not the cost. Re-deriving the modules a test needs in
order to run is the cost**, and it is paid for modules that did not change, to
produce facts the store already holds. A nondeterministic test must run every
time — that is what it is for — but running it needs the *bodies* of the
modules it reaches, and the driver gives it those by marking them
`Refusal::NeededToEvaluate`, which makes them parsed files: hashed, walked by
the checker, and written back, all to arrive at what their fingerprints already
said.

This record's original claim — that no phase dominates, so the whole front end
must be held — was true of the corpus it was read on and false as a general
statement. It is corrected here rather than beside itself: git holds what it
said.

## What `--watch` already does, and what it does not

ADR 0037 built the warm process: `ply test --watch` holds the store, the checked
front end and the compiled unit across iterations. An iteration where nothing
moved costs a stat per file.

An iteration where something moved pays the front end in full, because
`crates/ply-cli/src/driver.rs` is one-shot. It takes a set of files and produces
a `Loaded`, and every part of it is written to work over the whole program: gate
1 runs to a fixed point, and each round copies every parsed module and hashes
the result; gate 2 widens and repeats; the merge restores every interface and
writes every fingerprint. None of that is wrong. It is what a process that
starts knowing nothing has to do.

## Where the time actually is, one level down

On a project whose tests must run, the modules they reach are re-derived, and
that re-derivation divides like this. Sub-phase instrumentation, taken and then
removed, puts it here:

- **Hashing is three things**: a normalization pass over every definition that
  yields the reference graph, a second pass that hashes each component against
  its referents' hashes, and the tests, laws and specs — which are hashed the
  same way and are, on a project with more tests than definitions, the largest
  of the three.
- **Writing back is two things**: building the tables a fingerprint is cut from,
  and re-encoding every definition's interface to compare it against the one the
  store already holds. The second is the larger.
- **Restoring** merges each file's contribution into one output, every time.

Every one of those runs over the modules the tests reach, and none of it is
about the edit.

## The question

**What does a module needed only to *run* owe the front end?**

Nothing, is the answer this record proposes, and the driver currently says
everything. A test that must run needs bodies. Its modules' hashes, schemes and
footprints are in the store, unchanged, and the run recomputes them anyway
because the only way the driver has to obtain an AST is to mark the file parsed —
and a parsed file is one the gates re-derive.

That is one seam, not eight phases. It is a much smaller question than the one
this record asked when it was written, and it is the one the measurement
supports.

## What makes it harder than it looks

The obvious route — hash only the modules that changed and take the rest from
their fingerprints — is sound about cycles and unsound about effects.

- **Cycles are safe to split on modules.** Import cycles are a diagnostic
  (`crates/ply-syntax/src/resolve.rs`), so the module graph is a directed acyclic
  one and a definition cycle is confined to a module. Freezing whole modules
  therefore never cuts a cycle in half.
- **Effect enumeration is not.** A definition's hash is written against an
  ordering that splices in the orderings of the components it references. A
  frozen module contributes no ordering, so a dependent hashed beside it would be
  written against a different enumeration and would hash differently from a
  from-scratch run. Freezing needs each frozen component's ordering carried
  forward, not only its hash.
- **The bytes are the identity.** `CONTRIBUTING.md` §"Where a change is likely to
  bite" puts `crates/ply-hash/src/normalize.rs` first: a change there is a
  cache-format change, and every cached result everywhere depends on it. A hasher
  that is subtly wrong does not fail — it answers a different hash, and the cache
  believes it.
- **And there is a second seam, not only hashing.** A restored definition is
  recorded as internally effectful, on the stated ground that a skipped module
  contributes no AST. A module kept for its bodies does contribute one, so that
  answer would become conservative where it used to be exact — which costs the
  compiled backend entries rather than correctness, and has to be decided rather
  than inherited.

The third is why this is a record and not yet a branch. The second is what the
branch would have to solve first.

## Criteria, registered before anything is built

**Correctness comes first and is not a benchmark.** A resumable front end is
correct iff, for every edit in the corpus, what it produces equals what a
from-scratch load produces — the same `HashOutput`, the same `CheckOutput`
interfaces, the same fingerprints, bit for bit. That is a differential of the
kind this tree already runs over every ported phase, and it is the gate. A
resumable path that is faster and not equal is refused, whatever it measures.

The differential must be armed the way the others are: a mutation that makes the
resumable path wrong has to turn it red. A comparison that cannot fail is the
defect class `CONTRIBUTING.md` §"The one rule" names.

**The gate exists.** `agree_resumed` compares a resumed load against a
from-scratch load over a sequence of edits, using the snapshot the cold-versus-
warm equivalence already uses, and each step asserts reuse happened. Both halves
of the reuse key are armed: dropping the file's content makes it red, and
dropping the source id makes `a_new_file_is_not_resumed_over` red.

**The warm arm exists.** `benches/marginal-change/warm-loop.sh` prices an edit
inside a warm process, at three sizes, over a deterministic corpus and one with
the generator's default fraction of nondeterministic tests. It is what a change
here is read against, and what said this record's first premise was wrong.

**The row.** `benches/marginal-change/`, at the same three sizes:

| arm | what it measures |
| --- | --- |
| cold process, edit | today's cost, the control |
| warm process, nothing moved | already near zero; it stays the floor |
| warm process, one leaf edit | the quantity this record is about |
| warm process, one hub edit | the same where the dependent set is large |

**Decision rule.** The marginal cost of a leaf edit in a warm process must be
flat in project size over a ratio of sixteen, by the fit
`benches/marginal-change/analyze.py` already applies. Flat, not smaller: a
constant-factor win on a cost that still follows the project is not the property
ADR 0021 claims, and taking it would be the milestone reporting success for the
wrong reason.

A hub edit is read for where its knee falls, not against a bar — its dependent
set really is proportional to the project, and a loop that recomputes it is
correct.

## What is done, and what is left

Done, and armed:

- **The gate.** `agree_resumed` compares a resumed load against a from-scratch
  load over a sequence of edits. Both halves of its reuse key were seen to fail.
- **The parse phase.** A file that still says what it said keeps the tree this
  process parsed, which takes parsing out of a warm iteration.
- **The measurement that re-aimed this record.** `warm-loop.sh` and its
  `observation-warm.txt`.

Left, and now a single seam rather than eight phases: **a module needed only to
run should not be re-derived.** What it needs is its bodies; what the driver
gives it is `Refusal::NeededToEvaluate`, which is the same treatment a changed
file gets.

Not attempted here, and the reason is in §"What makes it harder than it looks":
freezing a module's hashes needs its effect ordering carried with them, and
getting that wrong does not fail — it answers a different hash. The gate above
compares answers and would catch it on the fixtures; that is necessary and not
sufficient for a change to the bytes that are the cache's identity.

**A note on the shape of the mistake this invites**, because it has now happened
twice in this record's own work. Both times the answers stayed correct and the
*work* changed: a seed placed where a parsed tree goes made skipped files look
parsed and grew an iteration by half, and this record's first premise read a
whole-project cost off a corpus whose tests reach the whole project. Neither was
a wrong answer, so neither could have been caught by comparing answers. The row
caught both.

## What would make this wrong

- **If holding the front end costs more than rebuilding it.** A warm process
  keeps every module's tree alive between iterations. Measured on a
  4,000-definition project, a warm process settles at somewhat more than a cold
  invocation's peak and drifts upward slowly across edits — the store keeps every
  superseded definition, since content addressing is what makes an undone edit
  free. That is affordable for a session and unbounded for a long one, so a warm
  store owes an eviction rule that a cold one does not.
- **If the fixed points make a held state unrepresentable.** Gate 1 can widen and
  gate 2 can restore; if what a round gives up cannot be expressed as an update
  to the previous round's state, then the driver has to be rewritten rather than
  resumed, and that is a different record.
- **If a warm loop is fast enough without it.** The row's warm arm is what says.
  If a leaf edit in a warm process is already small and flat because the phases
  that follow selection dominate, this record's premise is gone.
- **If the editor is the loop rather than the command.** This record assumes the
  loop is `ply test` re-run. A language server holding the same state would want
  the same resumable front end for different reasons, and would set different
  latency budgets; nothing here has asked what those are.

## What this is not

Not a decision to build it, and not a design. ADR 0037 named a warm process and
its row said the warm process alone does not finish the job. This records what
finishing it means and how anyone would know they had.

And it is no longer a record about eight phases. It began as one, on a reading
its own warm arm later corrected, which is the reason the correction is written
into §"What the row found" rather than beside it: a reader who arrives at the
old claim would otherwise go looking for a lever in every phase, and there is
one seam.
