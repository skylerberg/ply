# 4. Machine-shaped failure

Status: accepted (interface landed, implementation outstanding)
Date: 2026-07-25
Depends on: `docs/adr/0003` — the store must hold definition **bodies**, not
only hashes and interfaces. Section 3 below states exactly what M5 needs from
it, and what M5 degrades to if 0003 lands in a weaker form.

## Context

DESIGN.md §5 says a failure is a structured artifact carrying a diff, a
footprint, and a suspect set. That is what M4 shipped and it is a good start
rather than the goal.

The premise of the language is that the primary consumer of a test failure is an
agent, not a person. An agent reading today's artifact still has to do the
diagnosis itself: it gets `suspects: [apply_debit, settle, post, Ledger.balance]`
and has to work out which of them it broke, usually by reading four definitions
and re-deriving what the system already knows. Two things make that
unnecessary, and both are consequences of decisions already made:

- **The system knows what changed.** The store records which definition hashes
  it has seen, and a test's closure is on hand. Their intersection is the
  suspect set — but the suspect set over-approximates in three separate ways.
  A reference contributes the referent's hash, so editing one definition moves
  the hash of every transitive dependent, and every one of them shows up as a
  suspect with no edit behind it. The closure is static, so it names
  definitions that never ran. And nothing distinguishes a definition that is
  merely *implicated* from one that is *responsible*.
- **The system can run the question.** Because compilation is content-addressed
  and cached, a *hybrid* program — some definitions at their old hashes, the
  rest at their new ones — is a legitimate program, its test hash is a
  legitimate cache key, and most of a search over such programs is answered
  without evaluating anything. This is the whole reason M5 comes after the
  incremental front end rather than before it: on a system that recompiles a
  module per touch, running a test twenty times in twenty configurations is
  absurd, and here it is cheaper than reading the diff.

So M5's job is not to add fields. It is to move work from the agent to the
system: *which change broke this, out of the ones I made*, and *what actually
ran on the way to the assertion*.

## Decision

A failure artifact answers four questions, in this order of value:

1. **Which change caused this?** — bisection over the definition graph.
2. **What actually ran?** — the causal slice.
3. **What else could have?** — the suspect set, now ranked and annotated.
4. **What was asserted?** — the structured expected/actual, which is where the
   old artifact started.

The terminal output follows the same order, because the culprit is the answer
and the diff is the evidence.

### 1. The baseline: what "before" means

Bisection needs a configuration the test passed at. The result cache records
`DefHash -> Pass`, which is not enough: a test's hash covers its whole closure,
so a regression has a *different* test hash and there is nothing to look up.

The store therefore gains a third map, the **pass record**, keyed by the test's
`<module>.<label>` key:

```rust
pub struct PassRecord {
    pub test_hash: DefHash,
    /// Program-wide name -> hash, for every definition in the test's closure at
    /// the moment it passed.
    pub closure: BTreeMap<Symbol, DefHash>,
}

impl Store {
    pub fn pass_record(&self, key: &Symbol) -> Option<&PassRecord>;
    pub fn put_pass_record(&mut self, key: Symbol, record: PassRecord);
}
```

One record per test key, overwritten on each pass, so it grows with the number
of tests rather than with history. It is written on the same path that writes
`Outcome::Pass` and under the same rule: **never for a failing or nondet test**.

Keying it by name rather than by hash is the one place in Ply where a *name* is
load-bearing for a cache, and it is deliberate. The whole point is to survive an
edit that moves the hash, so the key has to be the thing that does not move.
Renaming a test's label therefore loses its baseline, and the cost is one
missing bisection, never a wrong one. Renaming a *definition* still costs
nothing: names appear in the record's keys, which are metadata, and the hashes
they point at are unchanged.

### 2. The delta: an edit is not the same as a hash that moved

Let `B` be the baseline closure and `C` the current one. Every name where they
disagree is a **change**, but only some changes are candidates:

| kind | when | candidate |
| --- | --- | --- |
| `Edited` | its own normalized body differs | yes |
| `Derived` | its body is byte-identical; its hash moved because a dependency's did | **no** |
| `Added` | absent from `B` | yes |
| `Removed` | absent from `C` | yes |

The `Edited`/`Derived` split is the single biggest reduction and it is only
available in a content-addressed system. Editing one leaf moves the hash of
every transitive dependent; on a realistic graph that turns one edit into a
dozen suspects, eleven of which nobody wrote. There is no change to attribute to
a definition whose text nobody touched, and flipping one is a no-op because its
body is the same on both sides.

**How `Derived` is decided.** Re-normalize the definition's *current* body
against the *baseline* hash table. If the result equals its baseline hash, its
own structure is unchanged and only its references moved. This is one
normalization per changed definition and it is exact.

Two cheaper tests were considered and both are unsound, which is worth recording
because both look right:

- *"the set of names it mentions is unchanged"* — `f(x)` becoming `f(f(x))`
  mentions the same names and is a different definition.
- *"its interface is unchanged"* — an interface says nothing about a body.

A false `Derived` drops a real candidate from the search and yields a confidently
wrong culprit, which is worse than no bisection at all. The exact test is
affordable; use it.

### 3. Hybrid programs, and the ones that do not typecheck

A hybrid `H(S)` takes the post-edit body for every definition in `S` and the
baseline body for everything else in the closure, and runs **the test as it is
written now**. The test is pinned to its current body in every hybrid, because
the failure being explained is this test's failure; the old test asserting
something else is not evidence about it.

`H(∅)` is therefore the baseline definitions under the current test, and its
outcome is itself an answer:

- `H(∅)` **fails** and the test was edited → the edit to the test is the
  culprit. Verdict `test_changed`.
- `H(∅)` **fails** and the test was not edited → nothing in the definition graph
  explains this failure. Verdict `not_in_the_graph`: look at a `nondet` effect,
  something outside the program, or a defect in Ply. This is a genuinely useful
  answer and today's artifact cannot produce it.
- `H(∅)` **passes** → bisect.

#### The typecheck problem

A changed signature means old and new callers disagree, so `H(S)` for many `S`
is not a well-typed program. This is common, not exotic: it is what every
rename-a-parameter or add-an-argument edit looks like. "Give up" is not an
answer, and neither is "treat it as a failure" — a program that does not compile
is not evidence that the test broke.

The answer has two parts.

**Part one: fuse what cannot be separated, before searching.** For each
candidate, compare its *published interface* — canonicalized scheme and
footprint — on the two sides. The baseline side comes from the front-end cache's
`CachedDef`; the current side from `CheckOutput::defs`. Then:

> A candidate whose interface is unchanged stands alone. A candidate whose
> interface changed is fused with every candidate that mentions it.

That rule is exactly the typecheck condition. A caller only notices its callee
being swapped when the callee's *interface* moved — and a caller that had to be
edited because of that is itself a candidate, so it is in the graph to be fused
with. A caller that did *not* have to be edited is `Derived`, its body is
identical on both sides, and it compiles against either. `Added` and `Removed`
are never independent: nothing that mentions a definition can be flipped without
the definition existing.

The dependency edges must be unioned over **both** eras. The current graph alone
misses a baseline body that references a definition since deleted; the baseline
graph alone misses a caller written against a definition since added. Unioning
over-approximates, which merges two clusters that could have been searched
apart — a slower search, never a wrong flip. When the baseline interface is
unavailable (a pruned cache), the candidate is treated as *not* independent,
which is the same conservative direction.

**Part two: when a hybrid still does not check, it is not evidence.** The search
is three-valued — `Fails`, `Passes`, `Unresolved` — and `Unresolved` covers "does
not typecheck", "a body is missing from the store", and "it failed, but not with
the failure being explained". Delta debugging refines its partition past an
unresolved configuration rather than concluding from it.

The honest cost is stated in the artifact rather than hidden: **any unresolved
trial disqualifies the minimality claim**, even one off the path to the answer,
because the search walked around a question it could not ask. Confidence drops
to `partial`, and `search.unresolved` says how often it happened. A consumer that
sees a `partial` verdict over two definitions knows to read both.

At the limit, where the search narrows *nothing* and had unresolved trials, the
verdict is `inconclusive` rather than `bisected`. Returning the whole change set
under a `bisected` label would have a consumer act as though the search had
endorsed it, when in fact it never got an answer to a single question.

**A failure signature** decides whether a hybrid reproduced *this* failure: the
diagnostic's code, its primary span, and its message. A different assertion
failing is `Unresolved`, not `Fails`. Requiring the message to match is
deliberate — an `assert_eq` that now reports different numbers is a different
failure and attributing this one to it would be a false positive.

#### What ADR 0003 has to provide

Bisection needs, for a `DefHash`, the definition it names, in a form that can be
assembled with others, checked, and evaluated. **It must be name-erased and
hash-linked** — a body whose free references are `DefHash`es rather than names —
for a reason specific to the hybrid case: a hybrid mixes definitions from two
namespaces, and a removed definition has no module to live in any more. Storing
source text and re-resolving would require inventing a module layout for the
mixture, which reintroduces exactly the naming dependence content addressing
removes, and would make a hybrid's hashes disagree with the ones the same
definitions have in the real program.

Concretely M5 needs:

- `Store::body(DefHash) -> Option<Definition>` where a `Definition`'s references
  are hashes;
- `prune` to retain bodies reachable from any surviving `PassRecord::closure` —
  otherwise the first prune deletes the baselines;
- `ply-eval` to evaluate a hash-linked graph, and `ply-core` to check one.

If 0003 lands storing source text instead, bisection is still possible but must
reconstruct a module layout, and this ADR's claim that a hybrid's test hash is a
valid cache key no longer holds. That would be a material downgrade and should
be re-decided rather than worked around.

### 4. The search

Delta debugging (ddmin) over clusters, not a plain binary search. "Flip half and
see" assumes a single cause; two edits that only break the test together are
ordinary — a caller and its callee, a constant and the assertion about it — and a
binary search silently returns whichever half it happened to try first. ddmin
returns a 1-minimal set: dropping any group makes the failure go away.

The single-cause case is the fast path *through the same algorithm* and costs
`2·log₂(n)` trials, which is the O(log n) the roadmap promises. The quadratic
worst case is real and is what the budget is for.

Three things make it cheap in practice, in descending order of importance:

1. **One cluster needs no trials at all.** The overwhelmingly common case — one
   edit — is answered for free, before anything is built.
2. **A hybrid's test hash is a cache key.** `H(∅)` under an unedited test *is*
   the baseline test hash, which the store holds as `Pass`, so the lower bound
   costs nothing. Repeated bisections over overlapping subsets hit the same
   entries.
3. **Subsets are memoized within a search**, so ddmin's complement passes never
   re-ask what a chunk pass already answered.

**Writing to the cache.** A hybrid that passes may be recorded as
`Outcome::Pass` under its own test hash: the hash covers the entire hybrid
closure, so the claim is true of exactly that configuration and of nothing else.
Bisection must **never** call `observe_definitions`, though. Recording a
definition retires it as a suspect, and a definition that was fine *in a hybrid*
has not been vindicated in the real program; marking post-edit bodies as seen
would empty the next run's suspect set. This is the one silent-wrongness path in
M5 and it needs a test of its own.

### 5. The causal slice

The closure is what a test *could* reach. The slice is what it *did* reach. That
is the difference between "these 40 definitions could be involved" and "these 3
ran", and neither is derivable from the other.

Three things are recorded, and they are three because they answer different
questions:

- **`stack`** — the frames at the moment of failure, outermost first. This is
  the path, and it is what a person reads.
- **`entered`** — every definition entered, in first-entry order, with a call
  count. Everything on the stack ran, but not everything that ran is on the
  stack: a definition that returned before the assertion is still implicated and
  is *not* where the failure happened. A call count in the thousands next to an
  assertion about a list length is itself a finding.
- **`observed`** — the atoms actually performed, which is a subset of the
  declared footprint. A declared atom that never fired means a branch was not
  taken, which is often the whole explanation.

**Tracing runs on a re-run, not on the first execution.** The green path is the
one that has to be fast, and a push/pop per call sits on the interpreter's
hottest path. A `det` test replays identically by construction — that is what the
determinism guarantee buys — so re-running one failing test costs one test and
changes nothing. For `test/nondet` the replay may not reproduce; the slice then
carries `reproduced: false` and is reported as evidence about a different
execution rather than silently mixed in.

The traced re-run doubles as the **reproduction check**: a `det` test that fails
and then passes on replay is a defect in Ply or a leak in a handler, and the
artifact says so instead of bisecting a phantom.

`ply-eval` gains the recorder; the hook is `Interp::apply` for named closures and
the perform site for atoms, both of which already have the qualified name in
hand. The trace has a size cap; hitting it sets `truncated`, and the `stack` stays
exact because it is bounded by recursion depth rather than by call count.

### 6. Ranked suspects — what ships before 0003

Bisection is gated on a body store. Everything else in this ADR is not, and the
suspect set improves substantially without it:

- `Derived` changes are marked as such, so an agent stops reading the eleven
  definitions it did not write. (This needs a baseline, not bodies.)
- Suspects that never ran are marked, and sorted last.
- Suspects on the failing stack are sorted first, innermost first.

The ranking is: bisected culprit; then on the failing stack, innermost first;
then ran but had returned; then untraced; then did not run — breaking ties
toward an edit over an inherited hash, then by name. It is total and
deterministic, because two runs over the same failure must produce byte-identical
artifacts or the artifact cannot be diffed against yesterday's.

### 7. The failure artifact

`ply test --json` emits one object with `"schema_version": 2` at the top level.
The version exists because a machine consumer needs to know what it is parsing
before it parses it; it is bumped whenever a field changes meaning or leaves.

Each entry of `failures` is:

```json
{
  "key": "ledger.balance never goes negative",
  "name": "balance never goes negative",
  "module": "ledger",
  "test_hash": "9f1c…",
  "nondet": false,
  "status": "failed",
  "location": { "file": "src/ledger.ply", "line": 88, "column": 5,
                "end_line": 88, "end_column": 34 },

  "culprit": {
    "verdict": "bisected",
    "skipped": null,
    "confidence": "minimal",
    "definitions": ["ledger.apply_debit"],
    "groups": [["ledger.apply_debit"]],
    "reason": "narrowed 5 changed definitions to ledger.apply_debit in 3 runs (2 answered from the cache)",
    "search": { "candidates": 5, "clusters": 4, "evaluated": 3, "cached": 2,
                "memoized": 4, "unresolved": 0, "exhausted": false }
  },

  "assertion": {
    "kind": "eq",
    "expected": "0",
    "actual": "-5",
    "message": null,
    "first_difference": { "path": ".entries[2].amount", "expected": "3", "actual": "4" }
  },

  "causal_slice": {
    "traced": true,
    "reproduced": true,
    "truncated": false,
    "stack": [
      { "name": "ledger.post", "hash": "3b2a…", "span": { "source": 1, "start": 1840, "end": 1863 } },
      { "name": "ledger.apply_debit", "hash": "7d40…", "span": { "source": 1, "start": 902, "end": 930 } }
    ],
    "entered": [
      { "name": "ledger.post", "hash": "3b2a…", "calls": 1 },
      { "name": "ledger.apply_debit", "hash": "7d40…", "calls": 3 }
    ],
    "observed_footprint": ["ledger.db.read[accounts]"]
  },

  "suspects": [
    { "name": "ledger.apply_debit", "hash": "7d40…", "before": "1c88…",
      "change": "edited",  "ran": true,  "depth": 0,    "culprit": true },
    { "name": "ledger.post",        "hash": "3b2a…", "before": "aa02…",
      "change": "derived", "ran": true,  "depth": 1,    "culprit": false },
    { "name": "ledger.format_row",  "hash": "51ff…", "before": null,
      "change": "added",   "ran": false, "depth": null, "culprit": false }
  ],

  "footprint": {
    "declared": ["ledger.db.read[accounts]", "ledger.db.write[accounts]"],
    "observed": ["ledger.db.read[accounts]"]
  },

  "diagnostic": { "code": "E0501", "severity": "error", "message": "…",
                  "labels": [ … ], "notes": [ … ] }
}
```

What each field is *for*. A field an agent cannot act on is noise, so this is
the justification, and a field that cannot answer it should be deleted rather
than defended:

| field | the action it enables |
| --- | --- |
| `culprit.verdict` | the field to branch on. `bisected`/`sole` → go fix a definition. `test_changed` → the edit to the test is the bug. `not_in_the_graph` → stop reading definitions; look at nondeterminism or the environment. `not_reproduced` → re-run before acting. `not_attempted` → read `skipped`. |
| `culprit.skipped` | *why* nothing was attempted, so a consumer can fix the cause (`never_passed` → nothing to do; `no_bodies` → the cache was pruned). |
| `culprit.confidence` | how many definitions to open. `minimal` → exactly these. `fused` → one of these, they could not be separated. `partial` → these and possibly more. |
| `culprit.definitions` | the flat list to act on. |
| `culprit.groups` | which culprits are fused with which, so `fused` says *which* group is ambiguous rather than leaving a flat list. |
| `culprit.reason` | the sentence to print when handing the failure back to a person. |
| `culprit.search` | whether to trust the above, and whether to raise the budget. `exhausted: true` with a wide `definitions` means "re-run with `--bisect-budget`". |
| `assertion.expected/actual` | the diff, structured, so it is not parsed out of a rendered message. |
| `assertion.first_difference.path` | where to look inside a large value — the field that turns a 400-element list diff into a line number. |
| `causal_slice.stack` | the path to the failure. The innermost frame is where to start reading. |
| `causal_slice.entered[].calls` | a loop that ran the wrong number of times, visible without instrumenting anything. |
| `causal_slice.observed_footprint` | which branch was taken, and which handler fired. |
| `causal_slice.reproduced` | whether any of the above is evidence about *this* failure. |
| `suspects[].change` | `derived` means nobody edited it — skip it. This is the field that shrinks the reading list. |
| `suspects[].ran` | `false` means it cannot have caused this, whatever its hash did. |
| `suspects[].depth` | how close to the failure it sits; the ranking is already applied, this is why. |
| `footprint.declared` vs `observed` | a declared atom that never fired is a branch not taken. |
| `test_hash` | the cache key, so a consumer can correlate this failure with the same test across runs. |
| `location` | where to open the editor. |
| `diagnostic` | the rendered form, unchanged, for anything that already consumes it. |

`suspects` becoming an array of objects is a **breaking change** to the v1
shape, which is why the schema is versioned. The array is ranked, so a consumer
that reads only `suspects[0]` gets the best guess.

### 8. Human output

```
   ✗ balance never goes negative                             2.1ms

   1 failed, 2 passed, 44 cached (0.08s)

   balance never goes negative
     culprit: ledger.apply_debit                       src/ledger.ply:31
       narrowed 5 changed definitions to ledger.apply_debit in 3 runs (2 cached)
     assertion failed: expected 0, found -5
       at src/ledger.ply:88:5
     ran: ledger.post → ledger.apply_debit
     suspects: ledger.format_row (added, did not run)
```

The culprit line comes before the diff because the culprit is the answer. A
reader who already knows which definition broke does not need to work backwards
from an expected/actual pair to find out.

When there is no culprit the block degrades rather than apologizing — no culprit
line at all, and the causal slice carries the weight:

```
   balance never goes negative
     assertion failed: expected 0, found -5
       at src/ledger.ply:88:5
     ran: ledger.post → ledger.apply_debit
     no culprit: this test has never passed, so there is no earlier definition
                 set to compare against
```

A first-ever red test and a regression are genuinely different situations and
this is where the difference shows: a regression leads with a name, a new test
leads with a path. Neither leads with an apology.

### 9. Cost control

Bisection runs **automatically** when all of these hold:

1. the test failed (not panicked — a panic is a defect in Ply, not a change);
2. it is not `test/nondet`;
3. a `PassRecord` exists for its key;
4. the delta is non-empty;
5. bodies for the baseline closure are in the store.

It does **not** run for a test that has never passed. There is no "before", and
inventing one — bisecting against an empty program, or against `main` — would
produce a confident answer to a question nobody asked. Verdict
`not_attempted` / `never_passed`, and the causal slice is what that failure
gets instead.

When there is exactly one cluster it "runs" in the sense that it produces a
verdict, but evaluates nothing. This is the common case and it is why the
default is on.

**Flags** (`ply test`):

| flag | meaning |
| --- | --- |
| `--bisect <auto\|always\|never>` | `auto` is the above. `never` suppresses it — for CI that only wants pass/fail, and for a bisect that is itself misbehaving. `always` ignores the budget and still respects preconditions 1–5, because none of them can be waived without inventing evidence. |
| `--bisect-budget <n>` | hybrid *evaluations*, default 64. |
| `--trace <auto\|always\|never>` | `auto` traces a failing test's re-run. `always` traces the first execution too, which is what a `test/nondet` that will not reproduce needs. `never` for a profiling run. |

**The budget is in evaluations, not seconds.** A budget in wall-clock time makes
the artifact vary with machine load, and an artifact that differs between two
runs over the same code cannot be diffed against yesterday's. A cached trial
costs nothing and is not charged.

**Scheduling.** Bisection happens after the main run, and its hybrids perform
the same effects the real test does, so two bisections whose tests conflict must
not overlap. They are scheduled through the existing conflict colouring
(`group_by_conflict`) over the failing tests' footprints, which is the machinery
already there for exactly this.

## What is NOT in M5

**Shrinking input values is not M5.** Delta-debugging a *counterexample* — the
list of 400 elements that ought to be a list of 2 — is property-test territory,
it needs parameterized tests, and Ply's `test` takes no parameters. It belongs
to **M8**, alongside specs and generated properties. Nobody should build a value
shrinker speculatively; there is nothing to shrink.

Also out of scope, each with a milestone:

- **Bisecting across git history.** M5 bisects between the last recorded pass and
  now. Walking further back needs a VCS, and the whole cheapness argument rests
  on the store, not on `git`.
- **Forking a fixture per hybrid** (M6). Each hybrid builds its own state today.
- **A seed as the repro artifact** (M7). M5's repro is a definition set.
- **Suggesting a fix.** M5 names the definition. What to write there is not a
  question the definition graph can answer.

## Consequences

- Every headline invariant is untouched. Bisection reads the store and writes
  pass records and hybrid `Pass` entries; it moves no `DefHash`, so renaming a
  function still selects zero tests and moving a definition still changes no
  hash.
- The result cache gains entries for configurations that never existed on disk.
  They are content-keyed and true, so this is sound, but `ply cache stats` will
  report more entries than there are tests and should say why.
- `prune` acquires a second retention root. Getting this wrong deletes baselines
  and silently downgrades every future bisection to `no_bodies` — visible in the
  artifact, which is the point of having the field.
- The front-end cache's `CachedDef` becomes load-bearing for a second reason:
  the independence test reads its `scheme` and `footprint`. A pruned entry costs
  a fused cluster, not a wrong answer.

## Required tests

The first four are the design; without them this is a claim.

1. Edit one definition in a passing test's closure; bisection names exactly it,
   with `confidence: minimal`, having evaluated **zero** hybrids (one cluster).
2. Edit five definitions, one of which breaks the test; bisection names exactly
   that one, in at most `2·log₂(5)` evaluations.
3. Edit two definitions that only break the test together; bisection names both
   and neither alone.
4. Change a function's signature and its caller; the two are fused into one
   cluster, no hybrid that splits them is ever built, and the reported
   confidence is `fused` rather than `minimal`.
5. A definition whose hash moved only because a dependency's did is classified
   `derived`, is never a bisection candidate, and is sorted below every edited
   suspect.
6. A test that has never passed produces `not_attempted` / `never_passed`, runs
   zero hybrids, and still carries a causal slice.
7. A `test/nondet` failure produces `not_attempted` / `nondet`.
8. A panicking test produces `not_attempted` / `panicked`.
9. Editing only the test body produces `test_changed` naming the test.
10. A failure the baseline also reproduces, with the test unedited, produces
    `not_in_the_graph`.
11. A hybrid that does not typecheck is `unresolved`; the search completes,
    reports `unresolved > 0`, and downgrades confidence to `partial`.
12. Bisection never calls `observe_definitions`: after a bisected failure, the
    next run's suspect set is unchanged.
13. A hybrid whose test hash is already in the result cache is answered without
    evaluating, and is not charged against the budget.
14. A spent budget yields `exhausted: true`, `confidence: partial`, and a
    culprit set that still contains the true cause.
15. The causal slice of a failure names only definitions that ran, and its stack
    is a suffix-consistent path ending at the failing frame.
16. A definition that ran but had returned before the assertion has `ran: true`
    and `depth: null`.
17. Two runs over the same failure produce byte-identical artifacts, including
    the order of `suspects` and of `culprit.groups`.
18. `--bisect never` produces `not_attempted` / `not_requested` and evaluates
    nothing.
19. The human summary prints the culprit line above the assertion line whenever
    a culprit exists, and prints no culprit line when none does.
20. A search that narrows nothing while hitting unresolved trials reports
    `inconclusive`, not `bisected` over the whole change set.

## Alternatives considered

**Bisect over files with `git`.** The obvious implementation, and wrong here. It
needs a VCS, it cannot see uncommitted edits — which is the state an agent is
always in — and every step re-runs the whole front end over the whole project.
The definition graph is finer-grained, works on unsaved work, and is nearly free
precisely because of the two ADRs before this one.

**Binary search instead of delta debugging.** Cheaper and silently wrong when two
edits interact. ddmin degenerates to the same cost in the single-cause case, so
there is nothing to buy.

**Treat "does not typecheck" as a failure.** It would make the search terminate
faster and attribute failures to definitions that merely could not be mixed. A
non-compiling program is not evidence about a test.

**Skip the fusion pre-pass and let ddmin discover the constraints.** ddmin does
degrade gracefully — see required test 11 — but it pays several unresolved trials
to learn what one interface comparison knows for free, and it ends up reporting
a pair where the fused answer is exact. Interface comparison is cheap and the
data is already in the front-end cache.

**Rank suspects and stop there, no bisection.** This is real value and it ships
first (§6), but it is a heuristic ordering of a list. The difference between
"probably this one" and "this one, and here is the program that proves it" is
the difference between the agent checking and the agent acting.

**Trace every execution rather than re-running the failure.** Simpler, and it
taxes the green path — the one the entire language exists to make fast — to
serve the red one. `det` replay makes the re-run free of risk.

## Not done here

This ADR lands the search, the classification and fusion rules, the artifact
types, and the report projection in `ply-test`. `Store::pass_record` /
`put_pass_record` and the body store have since landed with `ADR 0003`. Not
implemented:

- The `Hybrid` implementation: assembling, checking, hashing and evaluating a
  mixed definition graph. This is the bulk of M5's remaining work.

  Measured on a 10,000-definition corpus, this is not a partial capability but a
  binary one. `NoHybrid` answers every mixture `Unresolved`, so the only verdicts
  reachable today are `sole` — exactly one candidate, which needs no search — and
  `not_attempted(no_hybrids)`. Break one leaf definition and the culprit is named
  with **zero** evaluations. Break one and make twelve value-preserving edits
  beside it and the artifact has no culprit at all: `candidates: 0`,
  `evaluated: 0`, five suspects handed back to the reader. The "which of my
  twelve edits broke this" case the ADR opens with is precisely the case that
  does not work yet, and `--bisect-budget` is inert until it does.
- `Delta` *construction* from a baseline: the re-normalization that decides
  `Derived`, and the interface comparison that decides `independent`. The
  algorithm is specified above; the code needs the body store.
- The tracer in `ply-eval`, and the structured `Assertion` payload — today
  `expected`/`actual` are rendered into the diagnostic's notes and the artifact's
  `assertion` field is `null`.
- `ply-cli`: the flags, the culprit-first terminal block, and the v2 JSON
  emitter. `ply-test`'s own projection is the reference for the shape.
