# ADR 0038 — The invocation is the cost: what a resumable front end has to do

**Registered, not decided. It states a problem the measurement found, fixes the
criteria a solution is judged by before one exists, and builds nothing.**
ADR 0037 ordered the loop's work and its row answered the question it was
registered for. This record starts where that answer left off.

## What the row found

`benches/marginal-change/` prices one edit at three project sizes under both
engines. Two of its readings are settled and are not what this record is about:
a rename costs nothing, and a leaf edit is flat under the interpreter across a
project sixteen times larger. Selection works.

The third is:

> A warm `ply test` that rechecks **nothing** — the front-end report says
> `rechecked 0` — still pays a front end proportional to the project.

That is not selection failing. It is the cost of a process establishing what the
previous process already knew: reading every file, parsing what it must, hashing
every definition to find that none moved, restoring every interface from the
store, and writing every fingerprint back to compare against what is there.

**And no single phase dominates it.** On a four-thousand-definition project,
warm, hashing is about a quarter and writing back about a fifth; parsing,
restoring, resolving, checking, assembling the modules and reading the files
divide the rest. Every one is proportional to the project.

That shape is the whole of this record's motivation, and it is the opposite of
what an optimisation would want to find. There is no lever. A perfect
incremental hasher leaves three quarters of the cost where it was.

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

## The question

**What does a front end have to hold, and what does it have to redo, for an
iteration to cost the edit rather than the project?**

Not "how is hashing made incremental". The row says that question is a fifth of
an answer.

## What makes this harder than it looks

- **A definition's hash is over its closure**, so an edit to one definition moves
  the hash of everything that reaches it. The set to recompute is the changed
  definitions' *dependents*, and that set is what `ply_store::PassRecord` and
  `DepEdges` already know how to walk — but hashing computes the graph as its
  first pass, from a normalization of every definition. Reusing the graph and
  reusing the hashes are two different problems.
- **Gate 1 and gate 2 are fixed points.** Each round can pull in files the
  previous round had skipped. A held state has to be correct after a round that
  widens, not only after one that does not.
- **The bytes are the identity.** `CONTRIBUTING.md` §"Where a change is likely to
  bite" puts `crates/ply-hash/src/normalize.rs` first: a change there is a
  cache-format change, and every cached result everywhere depends on it. A
  resumable hasher that is subtly wrong does not fail — it answers a different
  hash, and the cache believes it.

The third is why this record exists rather than a branch.

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

**Then the row.** `benches/marginal-change/` re-taken with a warm arm, at the
same three sizes:

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

## What would make this wrong

- **If holding the front end costs more than rebuilding it.** A resumable driver
  keeps every module's AST, interfaces and hashes alive between iterations. On a
  large project that is memory a cold process never pays, and the suite's heaviest
  test already peaks near four gibibytes. If the held state does not fit, the
  answer is a smaller held state and a different design, not a bigger machine.
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
finishing it means and how anyone would know they had, **before** a branch
exists to judge, because the measurement that motivates it is exactly the kind
that invites optimising the phase that is easiest to see rather than the cost
that is actually there.
