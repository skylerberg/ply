# ADR 0026 — A reachable backend, and the instrument that could not decide it

Status: accepted — **decided, not built**. Answers the roadmap's open question:
whether a backend is ever reachable from a shipping command. Amends ADR 0016
§2.1, §2.4, §2.5, §3.5 and §10.2, and ADR 0018 §0.5's list of what it owes.
Supersedes nothing.

**A backend now ships.** §4.7 authorised taking cranelift into the workspace and
`ply test --backend cranelift` exercises it; `crates/ply-codegen` is a workspace
member that `crates/ply-cli` depends on unconditionally. §4.9 is what the code
generator reaches and costs, and its first sentence is the one to read: **on the
front-end-shaped workload it is a net loss.** `crates/ply-codegen-spike` is still
neither promoted nor deleted, and no workspace toolchain moved.

**Read this line first: nothing in this document *decided* to ship a backend.**
This project's most expensive defect class is a mechanism named everywhere a
reader would look for it and constructed nowhere (`CONTRIBUTING.md` §"The one
rule"), and an ADR that decided a backend was reachable would be the largest
possible instance of it. What is decided here is the *question*: whether the
answer is ever yes, on what evidence, and against which instrument.

Three findings from the arc that built the first backend behind this decision,
each recorded where it bites:

1. **A comparison guarded on prior agreement cannot see a backend that turns a
   red test green.** Comparing the backed machine against the plain one only
   where the two *engines* had already agreed on a **pass** makes exactly that
   the one thing the comparison cannot see. Found by the budget mutant over a
   recursion past the machine's bound: the guarded form reported nothing at all.
   §4.3's C4 is written against this shape and it still nearly happened.
2. **Seven of eight, not eight.** §4.5's second bullet predicted two mutants
   would not move unchanged and named the right two; one moved further than
   predicted and one did not move at all.
3. **The seam is O(n²) on a body it declines.** A `Reference` offer re-runs the
   body to exhaustion, so a recursion that outruns the budget costs one full
   attempt per level — three orders of magnitude on a deep ladder. **This is a
   property of the seam's `budget` contract rather than of any one backend** — a
   cranelift fragment burns fuel per offer too — and nothing in §1.4's cost
   accounting priced it.

## Context

Two facts made the question urgent rather than tidy.

**The first is that R5 measured a large speedup and no user of Ply could reach
any of it.** ADR 0018 §0.5 is blunt about it under a heading of its own —
"Nothing here ships, and that is the load-bearing sentence" — and the consequence
is that `ply test --engine both` could not install a backend, so the shipping CLI
caught **zero** of the eight deliberately wrong backends the spike's mutation
harness runs, and the rule that a backend run must not populate the result cache
was **unenforced because it was unreachable**.

**The second is that the deferral was ordered by an instrument that cannot be
pointed at any workload but one.** `ply_corpus::w6` decides M9 from the
interpreter's share of a served HTTP request over TLS over postgres, and
`Ladder::missing()` refuses a ladder without all nine of its rungs — so a compute
kernel, or a lexer over a file, cannot be fed to it at all. §2 is that finding,
measured rather than argued, and §3.1 is careful about which recorded goal §4
rests on, because the tree does not say what the framing around this ADR says it
does.

Everything below that is a number was re-derived under a pre-registration written
before any binary ran and kept outside the repository. Where a figure is quoted
from an earlier milestone rather than re-taken, it says so. **The instrument was
checked before it was believed**, with `.github/binary-is-current.sh`.

---

## 1. What is true today, checked rather than quoted

### 1.1 The seam is unreachable, and the record's inventory of it is stale in one place

At the time this section was written, `crates/ply-cli` held no mention of
`set_compiled`, `Compiled`, cranelift or a backend; `EngineArg` had exactly three
variants; `Cargo.lock` held no cranelift; and no flag in `ply --help` or
`ply test --help` installed a backend. Run rather than reasoned:
`ply test examples --engine both --no-cache` is the tree-walker against the
control-stack machine and nothing else.

**One figure in the record was wrong and is corrected in place.** `compiled.rs`
and `CONTRIBUTING.md` both said "all five `set_compiled` call sites in the
workspace are tests or the spike's own harness". The count is an order of
magnitude higher, over six files, and CONTRIBUTING's own parenthetical list
summed to a third number while omitting a file entirely.

**The load-bearing half is unaffected and was re-checked one file at a time:
every call site is a test or the spike's harness.** The count was decoration on a
claim about *reachability*, **which is why nobody noticed it was wrong for four
days — and that is the point of correcting it. A number carried beside a true
sentence is still a number a reader will re-quote.**

### 1.2 The seam is not inert on the shipping corpus

`crates/ply-eval/tests/suite/differential_corpus.rs` attaches two hand-built `Compiled`
implementations — one that declines everything, one that answers by tree-walking
— to the machine over `examples/` and `tests/fixtures/`.

**About a sixth of offered calls clear all seven gates on real Ply source.** So
the seam is not a hypothetical surface waiting for a workload that never arrives:
on the repository's own corpus a backend that could answer anything `Int` or
`Bool` would be entered tens of thousands of times. A cranelift fragment accepts
fewer, and the number is an upper bound rather than a forecast — **but it is not
small, and it is what `--engine both` would be auditing if it could.**

### 1.3 The spike builds, passes, and its published figures are stale

**`CONTRIBUTING.md` §"Things known to be broken" item 1's block was stale on both
of its figures, and the second was stale in the direction that matters.** It gave
a test count that had moved and omitted a whole test file. It also said
`cargo clippy --all-targets` in the spike reports a specific class of errors, all
in `src/rt.rs`, and gave that as the reason `.github/workflows/ci.yml`'s `spike`
job builds and tests but does not lint. **Re-taken, the crate is
clippy-error-clean**, so the job that already builds it could lint it. Two
documents disagreed about the same crate, and `compiled.rs` was the one that was
right — **which is what item 1's own closing paragraph predicts happens when
figures are not re-taken where they are published.**

### 1.4 What the seam costs, and what it is

`crates/ply-eval/src/compiled.rs` is the `Compiled` trait, `crossable`, seven
`Gate` variants and `admit`, plus a large test module. `Machine::set_compiled`
installs one, three counters hang off `Machine`, and one branch in
`compiled_answer` is reached on every interpreted call. **No implementor existed
in the shipping workspace.** ADR 0018 §0.5 prices the branch at **zero
allocations** per `/health` request and hundreds of predictable branch tests, and
says in its own voice that the wall clock of those tests was never taken.

One property of the seam decides how far §2's evidence reaches, and it is easy to
miss because it is stated as a safety rule rather than as a limit — `crossable`
admits `Value::Int` and `Value::Bool` and nothing else. Its doc comment calls
this "a capability cut as much as a safety one: nothing taking or returning a
`List`, `Map`, `Record`, `Str` or `Float` can be entered at all."

**So the seam as it exists is a scalar seam.** That is the right cut for the
backend behind it — ADR 0019 §5 item 4 records that the fragment lowers `a + b`
as `Int` arithmetic whatever the operands are and fails at run time, so a `Float`
crossing would be a working program that starts raising at a call site nobody
opted into — **and it is the reason the kernel result is a fact about integer
compute and cannot be extrapolated to an HTTP request, independently of any
share.**

### 1.5 The function ADR 0016's whole projection rests on cannot cross the seam

Read from both halves of the source rather than inferred, and it is the sharpest
thing this arc found that nobody had written down.

ADR 0016's `k` is measured on `std.http::read_line`, chosen by that ADR's §3.1
rule as the request-path function with the highest per-request cost whose entire
body is inside the fragment. Its signature is
`fn read_line(buf: Bytes, from: Int, budget: Int) -> Line`. **`admit` refuses on
the first argument, before any gate that has anything to do with effects or
budgets**: `Bytes` is not crossable and neither is the `Line` record it returns,
so `read_line` is `Gate::ArgumentShape` on every call the machine could ever
offer.

The spike's ratios are real, and the spike's own source says which path they are
taken on: `measure::compare` times the spike side through
`Harness::compiled_call`, whose doc comment reads *"A direct native call,
**outside any machine**: ADR 0016's original path, and the only one that can
report the fragment's own failure."* `Harness` holds a `hybrid` machine with
`set_compiled` attached, **and it is not what the `read_line` ratio is taken
on.** ADR 0016 was entitled to do that — the spike existed to price a *ceiling*,
and entry did not exist when its §3 was written.

The consequence is not a defect in either document and it is load-bearing for
§4.3: **ADR 0016's projection is Amdahl applying, to the whole interpreter share,
the speedup of a function that the seam as built cannot enter.** It is not that
the number is unreachable pending a wiring change; the argument shape is refused
by the boundary's first line. **So the served-HTTP arm of C2 is not "never
measured" — it is not measurable through this seam at all**, and would stay
unmeasurable after any amount of CLI work, until either `crossable` widens or the
fragment learns the constructs ADR 0016 §9.2 says endpoints and codecs are made
of.

This is also the cleanest illustration of §2's whole point. **The ladder's `k`
and the seam's reachability were built by different milestones against different
questions, and nothing in the tree compares them, because no instrument takes
both.**

---

## 2. The instrument cannot answer the question it is being asked

### 2.1 Nine rungs, and a refusal

`ply_corpus::w6::Layer::ORDER` is nine fixed rungs — call, endpoint, framing,
routing, machine, socket, tls, database, tracing — and `Ladder::missing()`
refuses a ladder that lacks any of them. Demonstrated rather than read, by
deleting one rung from a copy of the current ladder kept outside the tree: `w6`
answers `undecided — the ladder carries no `database` rung, so its total is not
the stack's and no share can be read off it`.

**The M9 decision procedure structurally requires a served HTTP request with a
socket, a TLS record layer and a database round trip on the path. A compute
kernel cannot be fed to it. Not "has not been" — cannot be, by the type.**

That refusal is correct for what the ladder is: a share taken over a partial
stack is a share of the wrong denominator. **The defect is not in the refusal. It
is in reading the output of a nine-rung HTTP instrument as an answer about a
language.**

### 2.2 The verdict is decided at C3, and the share tests are never reached

Re-derived by running `ply-corpus w6` over both published pairs rather than by
quoting `ROADMAP.md`. `w6` renders and judges files and takes no measurement of
its own, so load is irrelevant to these numbers.

**`w6::decide` returns inside the `c3_gaps` block** — six of ADR 0016 §4's seven
levers unpriced — **before the defer-share test, before the band-straddle check,
before every share test there is.** The share and the projection appear in the
output only because the reasons are printed above the verdict and because
`reopens_at` is composed from them. **Nothing about the interpreter's share has
decided anything in either take.**

### 2.3 The share has already crossed the ladder's own categorical floor

The measured share is **below `Criteria::defer_share`**, and the whole band is
below it, so it does not even straddle. What that means is that **if C3 were
satisfied tomorrow, the ladder would not weigh anything** — it would take ADR
0016 §2.3's "report the ceiling and stop, not measure harder" branch.
Demonstrated by marking the six unpriced levers priced in a copy outside the
tree: the verdict becomes *"below the floor: even an infinitely fast backend is
worth less than the bar"*. **That branch is already live. The unpriced levers are
the only thing standing in front of it.**

### 2.4 The ratchet is real, is stronger than the roadmap states, and is not the criterion the roadmap names

The roadmap composes the reopen sentence out of the unmet criteria and states the
problem in its own words: *every cheaper lever that lands makes M9's case
weaker*, and three have now landed where a code generator was predicted to be the
answer.

**The direction holds and is sharper than that.** Between the two ladders, with
no regression recorded anywhere, the share fell, the projection fell, the ceiling
fell, and **the `k` a backend needs to clear C2 rose by more than half again.
The bar moved away from the backend while the backend stood still.**

**But the mechanism is not the one the roadmap names, and the difference
matters.** The interpreter did not get faster in absolute terms between the two
ladders — it got *slower*, and so did the whole request, by more. **The share
fell because the rest of the request — TLS, postgres, framing, the box — grew
faster than the interpreter did.**

So `S` is a ratio whose denominator contains a TLS record layer and a database
round trip, and **it moves with facts that have nothing to do with Ply. A slower
disk under postgres lowers M9's case. A faster TLS library raises it.** ADR 0017's
consequences block says the portable readings are the ratios and not the
microseconds, and it is right about the ladder's *rungs*; **it does not follow
that a ratio between an interpreter and a network stack is a portable statement
about an interpreter.**

**C2 is not the ratchet. C1 is.** At the measured share a backend clearing C2 is
arithmetically possible — the fragment already measures far above what would be
needed, on the MCTS kernel. Run as a counterfactual with every lever priced and
the spike's times divided until C2 clears, **the reopen sentence collapses to a
single clause: "M9 reopens when the interpreter share reaches 50%."** C1 asks the
interpreter to become half of a request, and the only thing that makes the
interpreter half of a request is the interpreter getting slower or the database
getting faster. **That is a criterion satisfiable only by a regression or by
somebody else's release notes, and a criterion satisfiable only by a regression
is not a criterion.**

### 2.5 There is barely a point left before the criteria become unsatisfiable at `k = ∞`

`ceiling(S) = 1/(1−S)` clears C2's bar only above `S = 1/3`. The measured `S` is
just above it. **Below that, C2 is arithmetically impossible at an infinite
`k`**, while the criteria still read as "computed rather than argued". One more
lever of the size of the constant memo — which halved the share on its own —
**puts the ladder in a state where it prints a bar that no backend in any
universe can clear, in the same sentence and the same tone as a bar that a good
backend could.**

### 2.6 What is *not* wrong with the ladder

Stated because §4 keeps most of it, and because a document that only listed the
defects would be dishonest about a measurement this project paid for twice.

- **The thresholds are right for the question the ladder asks.** Fifty percent is
  the share at which an infinitely fast strategy is worth 2×, and 2× is a
  defensible price for a permanent second execution path. ADR 0016 §2.3's
  reasoning survives.
- **C3 has never been a ratchet.** It compares *gains* against *gains*, and both
  sides are measured on the same workload, so the instrument's denominator
  cancels. It is the criterion that has fired every time, and it fired for a good
  reason each time: W1 predicted codegen and W2 delivered by attacking an
  algorithm.
- **Keeping the verdict out of `Report` worked.** ADR 0016 §2.6's structural
  argument — a verdict that can be written into a file will eventually be written
  into a file — is why every number in §2.2 could be re-derived today by running
  a binary over a shipped file rather than by trusting a sentence. **That
  property is what made this section possible and it must not be lost.**

---

## 3. What an interpreter share can settle, and what it cannot

This section exists because §4.1 is the one decision below that does **not** rest
on a measurement in this tree, and a decision resting on a goal instead of a
number must say so in its own words rather than stand next to a number that
appears to support it.

**What `S` settles.** On the workload the shipped ladder was taken on —
`examples/desk.ply` served over a socket, TLS and postgres — the interpreter is
about a third of a request, so no execution-strategy change is worth much end to
end on *that* workload, and the best-priced cheaper lever is already worth a
meaningful fraction of it. That is a fact, it is re-derivable by one command over
a shipped file, and **it is a sufficient reason not to put a JIT in front of
Ply's HTTP stack. Nothing in §4 disturbs it.**

**What `S` does not settle.** Whether Ply should have a compiled backend is a
question about what Ply is for. A number that requires a `database` rung in its
denominator cannot be evaluated on a workload with no database, and §2.1 shows
the instrument answering `Undecided` rather than extrapolating — correctly.

On the one non-HTTP workload anyone has measured, ADR 0019 §5 puts most of the
executed work inside the fragment and ADR 0018 §0.5 measures a large end-to-end
speedup with thousands of native entries against a control at zero entries. **On
those figures C1, C2 and C4 would all pass and only C3 would fail. There is no
code path in this tree that computes that**, because `w6` demands the nine rungs.

### 3.1 The basis for §4, stated so a reviewer can reject it directly

The framing this ADR was commissioned under describes Ply as "a compiled language
carrying the workloads Go, Java and Swift carry". **That sentence, or anything
equivalent to it, is in no file in this repository**, and this document is not
entitled to decide a milestone from it. Checked with a grep across `README.md`,
`DESIGN.md`, `ROADMAP.md` and ADR 0021, which returns nothing.

`README.md` opens "**It is a research language**", and its bet is that
*generating code is becoming free, and that what stays expensive is knowing
whether it is correct*. `DESIGN.md`'s thesis is the verification loop. The only
document that asks whether Ply should be fast at compute is ADR 0018, whose
status line is "**proposed. No decision here is accepted.**"

**Deciding M9 on that framing would reproduce, one document later, the exact
defect ADR 0021 was written to fix** — a rationale that existed only in a
conversation, leaving the next reader a rejection with no goal behind it.

So the target is **not** the basis. What is, and it is recorded and accepted and
has nothing to do with an HTTP request's interpreter share: **ADR 0021 §4 item 3
puts compiled entry on the critical path of an ADR accepted as a statement of
intent** — "the fragment, entered at token granularity", on a profile where
dispatch dominates builtin bodies by more than an order of magnitude, **so
compilation removes the right half.** That is a codegen item, on a critical path,
in a document whose claim is that Ply's verification loop is O(the change) while
every toolchain it competes with is O(the project). **The reason to want a
backend that is written down in this tree is the bootstrap track, not throughput
on a served request** — and the ladder cannot see it at all: a lexer over a file
has no socket, no TLS and no database, so `w6` answers `Undecided` for it.

Two things follow, and both are limits rather than support.

- **The kernel result is corroboration and is not the reason.** One kernel, one
  program, one box, one pre-registered run whose own pre-registration forbade
  re-running it, through a seam that passes only `Int` and `Bool`. **Had it come
  back small, §4.1 would read the same and §4.3's C2 would be what stopped it.**
- **ADR 0021 §4 item 3's own unmeasured half is unmeasured still.** "The cost at
  one entry per token rather than one per file" is a question about *entry*, and
  the nearest thing in the tree to an answer is
  `crates/ply-codegen-spike/tests/entry_cost.rs` — three tests, `#[ignore]`d on
  purpose ("a measurement, not a gate"), which established that **an entry once
  cost O(the previous entry's peak arena)** by two orders of magnitude, and that
  `Ctx::end` fixed it. That is the failure mode that would make per-token entry
  unaffordable, it is closed, and **the evidence lives in three ignored tests
  inside the crate ADR 0016 §3.5 wants deleted.** §4.7 is written with that in
  front of it.

**If a reviewer rejects ADR 0021 §4 item 3 as a basis** — because the bootstrap
track is itself speculative, or because a token-granularity entry is not the same
thing as a backend a user reaches — then §4.1's answer weakens to "not decidable
here", and §4.2 through §4.7 stand unchanged, because each rests on a measurement
in §1 or §2 rather than on a goal. **That is the intended failure mode of this
section: it is separable.**

---

## 4. Decisions

### 4.1 Yes. A compiled backend is reachable from a shipping command

Answered **yes**, on the basis §3.1 sets out and no other: compiled entry is item
3 of ADR 0021 §4's critical path, that ADR is accepted as a statement of intent,
and the thing it is a critical path *to* — a front end hosted in Ply, verified in
time proportional to an edit — has no socket, no TLS and no database on it, **so
the instrument that has been ordering this deferral cannot see it.**

The question the record has actually been answering is a narrower one — *should
this HTTP service have a JIT* — and its answer is no, is well measured, and is
untouched by anything here.

Three things this "yes" is not, said now because each is a way it will be
misread:

- **It is not "advance M9".** §4.4's verdict is defer, on C3 and C4.
- **It is not a claim that a backend is close.** §1.5 records a function whose
  compiled speedup the entire HTTP projection rests on and which the seam refuses
  on its first line.
- **It is not permission to promote the spike.** §4.7.

**What changes is the standing of the question.** M9 has been deferred by an
instrument that structurally could not consider the case for it; from here it is
deferred by two obligations that name work. What stands between here and a
reachable backend is §4.3's C3 and C4, **and neither is a number that must rise.**

### 4.2 The W6 ladder is withdrawn from the role of deciding M9

The ladder is **not** withdrawn as a measurement. It remains the best account
this project has of where a served request's time goes, it stays in `benches/`,
and §3's first paragraph stands unamended. What is withdrawn is its authority
over M9, on the three grounds §2 measures: **it refuses every workload but one
(§2.1), its share moves with the network (§2.4), and its reopen sentence names a
criterion that only a regression can satisfy (§2.4) and is barely a point from
naming one that nothing can satisfy (§2.5).**

**A ratchet may not be left standing while a decision is taken around it**, so
this is a change to code and not only to a document:

- `Verdict::label` and `Decision::reopens_at` name **"a code generator for the
  served HTTP workload"**, not M9. The strings change; the thresholds do not.
- `Decision` grows a field naming the workload its share was taken on, and the
  rendered sentence carries it, **so that "a third of a request" cannot be
  re-read as "a third of Ply".**
- ADR 0016 §10.2's reopen sentence is annotated in place with a pointer here.

After that change the ladder's answer is the same and is true: **do not put a JIT
in front of this HTTP stack; price the six unpriced levers instead.** It simply
stops claiming to have decided a question about the language.

### 4.3 The criteria that replace it, and each of them can fire

Written before they are implemented, in ADR 0016 §2.1's order, and each stated so
that a contributor can make it true by doing work rather than by waiting for a
number to drift.

The single structural change is that **a verdict names a workload class.** There
is no global answer to "should Ply have a backend"; there is an answer for
MCTS-shaped compute and a different answer for a served HTTP request, and **ADR
0016's mistake was not its thresholds but its assumption that one workload could
stand for the language.**

> **M9 advances for a workload class when all four hold, each taken on that
> class.**
>
> **C1 — Coverage, measured as work rather than as time.** The fragment accepts
> **≥ 50%** of the workload's *executed work*, counted in executed lowered nodes.
> Not the interpreter's share of a wall clock.
>
> *Why this is not the old C1.* A work share does not move when the interpreter
> gets faster at the same work, and does not move at all when TLS or postgres
> gets slower — which is what actually moved `S` between the two ladders (§2.4).
> **It moves when a lever *deletes* work, which is honest: a backend genuinely
> has less to do.** It is satisfiable by widening the fragment, which is backend
> work.
>
> *And the threshold is inherited rather than re-derived, which is the weakest
> line in this section.* ADR 0016 §2.3 justified 50% by an argument about a
> **time** share. A work share is a different quantity and that derivation does
> not transfer to it unchanged. **50% is therefore provisional**, and what would
> settle it is §7's first bullet: both shares taken on one workload, and the
> difference between them read rather than assumed. Until that exists, C1 is a
> bar whose number is borrowed and whose *shape* — work, not time; named
> workload, not a fixed one — is what this ADR is actually changing.
>
> **C2 — A delivered speedup, not a projection.** **≥ 3.0×** end to end on that
> workload, **measured with a backend attached and native entries counted**, with
> a control arm at zero entries.
>
> *Why this is not the old C2.* `E` is Amdahl over a share, and ADR 0016 §10.3
> lists three measured reasons to doubt its own projection — a direct measurement
> of the same function's end-to-end value that is nearly nothing, a fragment
> reaching under half the functions, and a coverage cliff that collapses the
> ratio the moment two callees stay interpreted. **A projection that its own
> author calls "an upper bound with three measured reasons to doubt it" may not
> clear a gate.** R5's kernel result against its zero-entry control **is** such a
> number; ADR 0016's projection is not, and ADR 0018's withdrawn ceiling is the
> standing proof that a projection through this seam can be wrong in either
> direction. On HTTP it has never been taken and is **not takeable through the
> present seam at all** (§1.5).
>
> **C3 — Nothing cheaper. Kept, with one word changed.** The list is the
> **workload's**, not ADR 0016 §4's, because those seven levers are levers on a
> served request and half of them do not exist on a compute kernel. Everything
> else — the pricing method, the margin, the gains-not-ratios reading, and the
> rule that a single unpriced lever defers on its own — is unchanged.
>
> *Why this one is kept as it was.* **It is the only one of the four that was
> never a ratchet:** both sides are gains on the same workload, so the
> instrument's denominator cancels, and it can be satisfied by measuring rather
> than by waiting. It has also been right every time.
>
> **C4 — Correctness, with measured sensitivity, through a shipping command.**
> Agreement on every input, **and** the corpus that produced the agreement must
> have been seen to fail, **and** the eight wrong backends must be caught by a
> command a user can run.
>
> *Why this one is strengthened.* **"0 disagreements" is the exact shape of
> result `CONTRIBUTING.md` §"The one rule" names as this project's most expensive
> defect class**, and `wrong.rs`'s own module header says so first. C4 as ADR
> 0016 wrote it is satisfied by a corpus that compares nothing. The third clause
> is new and is §4.5.

### 4.4 Where those criteria stand today, on both workloads

| | compute kernel (`benches/kernel`) | served HTTP (`examples/desk.ply`) |
| --- | --- | --- |
| **C1 — coverage ≥ 50% of executed work** | **pass** — every function and every node inside the fragment | **fail** — a few percent |
| **C2 — measured ≥ 3.0× with entries counted** | **pass** — against a zero-entry control at parity | **not measurable** — `read_line` takes `Bytes`, so `admit` refuses it on its first line (§1.5) |
| **C3 — nothing cheaper, all priced** | **fail** — `sqrt`/`ln` as prelude builtins is inferred by Amdahl over three fields of one file and **not priced end to end**; ADR 0018 §4's `Map`/record/list machinery is a fifth of executed work and outside the fragment whatever compiles | **fail** — six of seven levers unpriced |
| **C4 — agreement, sensitivity, and a shipping oracle** | **fail on the third clause** | **fail** — same |

**Verdict: defer, for both classes, on C3 and C4.** And the difference from every
previous deferral is the shape of what is owed: **C3 is a measurement somebody
can take next week, and C4 is the subject of §4.5. Neither asks a ratio to move
on its own, and neither can be satisfied by a regression.**

**A third workload class is named and not judged, deliberately: the bootstrap
front end.** §3.1 makes it the recorded basis for §4.1, and no row is offered
here, because ADR 0021 §4 item 3's own sentence means C1 and C2 have never been
taken on it. **Writing a row for it would be the vacuous green this project keeps
producing.** Taking that measurement is the highest-value item in §6 that this
ADR does not itself require, because it is the one workload where a positive
answer would order M9 against something already accepted rather than against a
preference.

The sensitivity C4's second clause demands does exist, at corpus scale, and was
re-taken rather than quoted: `mcts --only agreement` reports zero disagreements
over thousands of cases, and `mcts --mutate off-by-one` reports well over a
thousand disagreements and exits non-zero. **The corpus has measured
sensitivity, which is the thing C4's second clause is for.**

One blind spot in it is real, is the spike's own finding, and is carried forward
into §7 rather than smoothed over: **a whole-kernel search is a weak oracle.**
Most searches notice a corrupted move selector, but **every compiled function
except a search's entry points is offered zero times during a search**, because
the hook sees nothing under an entered root. The per-function generated cases are
what cover them, **and a future harness that ran only whole-kernel searches would
report the same green over a much smaller explored space.**

### 4.5 The contract for reachability: a backend must be policeable before it is fast

This is the clause that answers "under what contract", and it inverts the order
the record has been assuming.

**No backend may ship until `ply test --engine both` can attach one and catch the
eight.** Not because policing is more valuable than speed, but because it is
*upstream* of it: **a backend whose wrong answers no shipping command can detect
is not a backend that can be shipped at any ratio**, and every argument about `k`
presumes a correctness story that did not exist outside a crate ADR 0016 §3.5
requires be deletable.

Three things follow, and each is a checkable obligation rather than a sentiment.

1. **`--engine both` becomes three pairs, and ADR 0016 §2.2 priced that correctly
   as a permanent cost.** What §2.2 could not price, because the number did not
   exist, is what it buys: over `examples/` and `tests/fixtures/`, a thousand
   tests offer the seam six figures' worth of calls and a sixth of them clear
   every gate (§1.2). **A third pair over that corpus is a real oracle on real
   source, not a ceremony.**

2. **The eight wrong backends must be catchable from the workspace, without
   cranelift.** `Mutation` has seven wrong variants plus a control, and eight
   *configurations* are tested because the budget mutant is exercised bounded and
   unbounded. Every one is a wrapper over `Compiled::enter`, and
   `differential_corpus.rs` already holds two `Compiled` implementations and a
   large corpus in the shipping workspace. **The mutations do not need a code
   generator; they need something that answers**, and the tree-walking double
   answers. (The declining double is the control: it answers nothing, so it can
   host no corruption — **which is exactly why a harness must assert the offer
   count before it asserts the catch.**)

   Two of the eight will not move unchanged, and saying which is the point of
   naming them:
   - The published-row mutant **cannot be produced by a backend at all.**
     `Gate::PublishedRow` and `Gate::InternalEffects` mean the mutant is never
     asked, and what stands is an offer count of zero. Pricing that gate requires
     deleting a machine line, and `tests/fixtures/self_handled_effect.ply`
     already does this on the workspace corpus.
   - The unbounded budget mutant is a native stack overflow, **caught only from
     outside the process.** Any workspace harness that runs it must run it as a
     child, **or it will report the most catastrophic failure available as its
     quietest.**

3. **`wrong-type` is not caught where a reader expects.** `compiled_refusals`
   stays at zero, because `Bool` and `Int` both cross. It is caught downstream,
   on the value axis and by a type error in the caller. **A future harness that
   watched `compiled_refusals()` for it would watch a counter that never moves.**

**Built, and the clause is now met for two backends rather than one.**
`ply test --backend <spec>` attaches a backend on `--engine machine` as well as
on `--engine both`, and under `--engine both` it is a **third** engine compared
against the plain machine rather than against the tree-walker — **so a divergence
reported is the backend's and nothing else's.**

**The clause is discharged per backend, not per seam, and that is why the table
is run twice.** The first implementation was reached only through a concrete
type, so the eight policed one implementation of `Compiled` and there was no
route by which a second was offered to them — **and two of the eight need
operations the `Compiled` trait does not have**: one asks the registry whether a
body exists, which is the distinction between "declined" and "never had one", and
one re-runs the body with fuel that is *not* the machine's budget. Those are now
`ply_eval::Policed::holds` and `::run_with_fuel`, `Mutant` holds an
`Rc<dyn Policed>`, and the executor takes a `&'static dyn Provider` — a
run-scoped, `Send + Sync` source of per-worker backends — instead of a concrete
type.

What a corruption can bite depends on which definitions the backend has a body
for, **and the two fragments are very different** — over `examples/` the
tree-walking backend holds definitions by the hundred where cranelift holds
dozens. So `crates/ply-cli/tests/suite/backend.rs` runs every configuration against
each installed backend and reports the counts separately.

**`reference` catches seven of eight; `cranelift` accounts for eight of eight.**
The eighth is the unbounded runaway over a non-terminating body, and it is not
caught by a disagreement — it cannot be, the process dies — **it is caught by the
process dying**, in hundredths of a second with a signal, where the tree-walker
produces no output and no exit for a minute. **The test asserts the child died
*by signal* rather than merely failing**, because the weaker assertion was
watched passing under a deliberately emptied fragment while nine other tests went
red.

### 4.6 The result-cache rule is armed before a backend exists, not after

`Machine::set_compiled`'s doc comment states the rule and, unusually for this
project, states its own unenforceability in the same breath: a run with a backend
attached is a third execution strategy, and a cached `Pass` is a claim about the
authoritative engine — **but `cache_bypassed` takes a `&TestArgs` with no
`Machine` in scope, and no shipping command could install one.**

Verified by reading every candidate: `EngineChoice::bypasses_cache` reads a
three-variant enum and knows nothing about a backend; `TestArgs` has no field
that could carry a `Machine`; and `Machine::compiled_counts()` — the fact that
would answer the question — had **no caller outside `ply-eval`'s own tests and
the spike's harness**. **The rule was unenforced twice over: the flag that would
set it did not exist, and the fact that would detect it was never read.**

Note the interlock, because it is a trap: **`--engine both` already implies no
cache**, so a backend installed on the `--engine both` path would be cache-safe
**by accident**, while a backend on the default `--engine machine` path would
not. **Enforcement may not be that accident.**

**Decision: the rule is armed in two stages, and the first stage lands before any
backend does.**

- **Stage one — the tripwire, buildable with no backend in existence.** A test in
  the `crates/ply-span/tests/armed.rs` tradition — a check over production source
  rather than over behaviour —
  `a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache`. It
  fails when `set_compiled` acquires a caller in production source unless
  `cache_bypassed`'s inputs have grown a way to see it. **This converts a rule
  stated on a doc comment into a test that fires on the exact change that would
  break it.**
- **Stage two — the diagnostic, owed by M9 and specified here so that M9 cannot
  choose a weaker shape.** The precedent is `cache_escapes`, the same class of
  rule for host binding: it walks the finished `RunReport`, finds results written
  to the cache whose test could reach the host, and turns each into an
  `INTERNAL_ERROR`. The backend rule is that shape one field over — `TestResult`
  carries the native entry count from the `Machine` that ran it, and a written
  `Pass` with a non-zero count is the diagnostic. Named `backend_escapes`, beside
  its precedent.

  The cost is stated rather than waved at: it plumbs `compiled_counts()` out of
  `ply_test::Worker` — which is also the exact place a backend would be installed
  — into `TestResult`, a signature change across `ply-test` and its report and
  JSON schema. **The one-line version, adding a clause to `cache_bypassed`, is
  stage one's *flag* half and covers only a backend that arrives by that flag.
  Stage two is the version that survives a backend arriving by any route**, and
  the reason to specify it now is that stage one alone would let M9 ship the
  cheap version and call the rule enforced.

**Both stages hold against a code generator — but "it must still hold" is the
sentence §"The one rule" is about, so each was broken again rather than reasoned
about.** The arming is backend-agnostic by construction and that is what was
tested: `cache_bypassed` reads a string, and `backend_escapes` reads the entry
count the **machine** recorded, which is a number no backend supplies. Deleting
the bypass condition reddens the read test; making the backend record arm
unreachable reddens the write test with `E0505`; and adding a `set_compiled` call
to the new crate trips the stage-one scan — **which is the thing worth checking,
that `armed.rs`'s production-source scan reaches a crate that did not exist when
it was written.**

**One defect was found by doing this rather than by reading.** The read half and
the write half were first written as **one** test over one project directory: the
read half warms the cache, so the write half's closing assertion found the passes
left by the warming and reported a failure on a tree where the rule holds
perfectly. They are two tests over two directories now.

ADR 0016 §2.2's own objection lands squarely on the flag half and is not answered
here: `RUNTIME_VERSION` keys `(RUNTIME_VERSION, DefHash) -> Outcome`, so **an
opt-in JIT is an opt-in cache**, and ADR 0011 §4 already refused that shape for
`--host` on the grounds that `ply check` must not disagree with itself. **This
ADR does not resolve it, and M9 may not treat it as resolved.** §5.

### 4.7 ADR 0016 §3.5 — one clause honoured, one amended, with both quoted

§3.5's last bullet reads:

> It may **not** be kept because it works. It is thrown away whatever the
> verdict; an `Advance` schedules M9, and M9 is a milestone with an ADR, not a
> promotion of a spike.

**That clause is honoured literally and remains so.** The spike is 6,909 lines of
source built to price a ceiling, its `rt.rs` is a JIT calling convention, and it
has bit-rotted twice while nothing noticed. **It is not a shipping component and
the path to one does not run through it.**

**The maintainer authorisation to take cranelift into the shipping workspace was
exercised, and this section is the record of exercising it.** `ply-cli` **gains a
dependency**: `ply-codegen`, unconditional, no feature flag. `Cargo.lock`
**gains cranelift**. `ply test --backend cranelift` installs a code generator.

**What still holds, and is the half worth naming.**

- **No workspace toolchain moves.** The pinned cranelift declares a
  `rust-version` at or below the one every CI job runs, so a default
  `cargo build` and a default `cargo test` need no second toolchain. **The next
  cranelift minor requires a newer rustc and is deliberately not the route** — a
  version bump past this line is a toolchain decision, and
  `crates/ply-codegen/Cargo.toml` says so where the pins are.
- **The spike is not promoted.** It is still not a workspace member, still
  depended on by nothing, still unreachable from any command, and
  `crates/ply-codegen` does not import from it. **What moved is source**, copied
  and adapted, with the provenance in each file's header. **A new crate built
  from a spike's source is not the spike being kept because it works; it is the
  spike being read.**
- **§4.5's condition is met before speed is argued.** The code generator was made
  policeable in the same change that made it installable — the mutations were
  lifted onto `ply_eval::Policed` first. **Nothing in this change quotes a ratio
  that was not taken against a backend the eight can corrupt.**

**What is not authorised by this and was not done.** Nothing here permits a
backend on `ply run`, on `ply serve` or on the default `ply test`: a backend is
installed only when `--backend` names one, and a run that installs one neither
reads nor writes the result cache. §4.6's interlock is unchanged and ADR 0016
§2.2's cache-key objection is still open (§5).

§3.5's *other* clause is the deletion requirement, whose stated reason was "so
that deferring M9 deletes one feature block and one dependency line, and nothing
else in the workspace knows it existed."

**That reason was already false before this change, and R5 is what made it
false.** After R5, `crates/ply-eval` carries `compiled.rs` — a public trait,
`set_compiled`, three counters and a branch on every interpreted call — **all of
which survive `rm -r crates/ply-codegen-spike`.** Performing the deletion removes
the only implementation of `Compiled` and leaves the declaration standing, which
makes the declared-nowhere-constructed shape *worse*, not better — **and
`armed.rs` does not reach traits and does not reach `ply-eval`, so nothing in the
tree would say so.**

> **Amended (this ADR).** `crates/ply-codegen-spike` is deleted when — and only
> when — the seam's **measured sensitivity** exists inside the shipping
> workspace: the eight wrong backends reproduced over the `Compiled` doubles in
> `crates/ply-eval/tests/`, running under `cargo test --workspace`, with a corpus
> that has been seen to fail. Until then the spike is the only thing in this
> repository that has ever demonstrated that a wrong backend would be noticed.
>
> Everything §3.5 says about *promotion* is unchanged.

**The condition failed once, for a structural reason, and §7's fifth bullet
predicted it.** That bullet warned the moved harness might have less sensitivity
than what it replaced, and that the mitigation was naming *measured sensitivity*
rather than a test count. Seven configurations moved and were **stronger** on the
axes that moved — real tests rather than generated cases, under
`cargo test --workspace` rather than a crate on another toolchain. The eighth,
the unbounded budget mutant, did not: §4.5 called it "a native stack overflow,
caught only from outside the process", **which is true of native code on a fixed
stack and false of a tree-walker whose frames grow on the heap.** The same
corruption does not crash there — **it hangs.** A harness can run it as a child;
what it cannot do is tell a hang from work, **and a wall clock is not a
disagreement.**

**The condition is met now, and the spike is still not deleted. Both halves of
that sentence are decisions.** A code generator supplies the missing half,
because native frames sit on a fixed stack — and the earlier diagnosis was half
wrong about which half was missing: **the reporter was never absent**, since
every test in `crates/ply-cli/tests/suite/backend.rs` runs `ply` as a child. It is not
acted on for two narrow reasons that are meant to expire: the spike is **the only
home of a measurement that is currently red and unexplained**
(`CONTRIBUTING.md` item 18, plausibly a defect in the harness's own refused-kind
check rather than in the backend — **deleting the crate deletes the evidence for
an open finding before anybody has decided what it means**), and ADR 0018 §0.5's
kernel ratio **has no other instrument.**

**This is a permission, not a schedule, and it has expired once before** — ADR
0016 §11's deletion requirement sat undone for two milestones because nothing
named a condition. **This is the third document to touch this obligation, and
that is a reason for suspicion rather than for confidence.** The difference
claimed, and it should be held to it: **this one names a condition, and the
condition is a test somebody writes.**

### 4.8 The seam stays

Stated as a decision because "delete the spike" is routinely read as implying it,
and R5's correction to §3.5 shows the two are different acts.

`compiled.rs` stays: the trait, `crossable`, the seven gates, `admit`,
`set_compiled`, the three counters, and the `enter_code` branch. Three reasons,
in order of weight.

1. **It is the answer to an architectural question, and the answer was
   measured.** ADR 0018 §0 said "make the interpreter able to enter compiled
   code, or the ceiling holds however much of the fragment you accept". Entry
   turned parity into a large win, and §0.5 withdrew its own ceiling as an
   artifact of a body-only attribution. **That finding is the most valuable thing
   R5 produced and the seam is its standing form.**
2. **Its contract cost four review rounds to get right and every round is
   recorded in it.** The frame bound that let a backend answer where the machine
   raised; `Gate::InternalEffects` and the transitivity it needs, which a one-hop
   bit does not give; the arena reset that cost O(the previous entry's peak);
   `crossable`'s refusal of `Float` in front of a fragment that lowers `a + b` as
   `Int`. **Deleting the seam deletes the tests that hold those, and the next
   backend rediscovers them by review or does not.**
3. **It costs zero allocations per request**, measured against a binary with the
   call site deleted, at three window sizes, with the arms alternated.

And the honest cost of keeping it, which §7 turns into a way this can be wrong:
**the wall clock of the per-request branch tests has never been taken**, on
either binary, although both existed. ADR 0018 §0.5 says so about itself. **Zero
allocations is not zero cost, and this ADR does not take that measurement
either.**

### 4.9 What the code generator costs and what it reaches

**Read this first, and it is the sentence this section exists to make
unavoidable: on the front-end-shaped workload the code generator is a net loss.**
ADR 0030 predicted the front end would be the worst case and it is worse than that
prediction, for a reason that prediction did not price.

#### The pre-registration

Written before any binary ran and kept outside the repository, with one amendment
made before any number existed and marked as such. It fixes five statistics,
their run counts and their decision rules, **including two written to permit an
unwelcome answer**: *"`entered == 0` on all three is a NULL RESULT and is reported
as one in the first sentence of the report"*, and *"A cranelift result AT OR
BELOW `Reference`'s is the PREDICTED outcome and is reported as a confirmation,
not as a failure. No arm is re-run to get a better number."*

#### What it reaches

`ply test <corpus> --engine both --no-cache -j 1 --json`, one run each, on a
binary `.github/binary-is-current.sh` reported current first. **Both corpora are
green on `--engine both` with the backend installed**, which is the correctness
result and the reason the speed rows are worth reading at all.

**The finding is the contrast between two corpora.** On a compute kernel the code
generator reaches exactly what the tree-walking backend reaches — the fixpoint
refuses *nothing* in `benches/kernel`, and every definition compiles as one
closed unit. On a program built out of the standard library it reaches about one
percent of offers, and `crates/ply-codegen/tests/suite/fragment.rs`'s census says why,
ranked: **`++`, `fold`, `map`, a lambda.** **That is ADR 0018 §0's roadmap
re-derived on the shipping standard library, four milestones later, unchanged in
its ordering.**

#### What it costs

Per run, and split because the halves scale differently — **the analysis is
whole-program and paid once, the code generation is paid per worker**, because a
compiled unit owns a constant pool of `Value`s and a `Value` is `Rc` all the way
down. Both are printed by `ply test` itself — `compiled N unit(s) in X ms, after
Y ms deciding what to compile` — rather than living only here.

#### Speed, and the load caveat stated before the numbers

`ply test <corpus> --engine machine --no-cache -j 1`, many windows per arm, arms
interleaved one window at a time in rotation, min reported, **with two arms that
are the same command under different labels as the null control.**

**Every series was taken above this project's load gate. These are observations
and not figures**, and `CONTRIBUTING.md` §"Gate on an idle machine before
measuring, not after" is right that they should have been taken behind it. **What
makes the ordering safe anyway is the null control, which is in the series rather
than argued:** the two identical arms landed within a fraction of a percent of
each other, against differences of hundreds of percent.

Three things follow.

1. **The tree-walking backend on `examples/` replicates ADR 0030's front-end
   figure to three decimal places** — same corpus, same command, when the review
   below re-took it on ADR 0030's own workload. That is a real replication.
2. **cranelift on the kernel is the first speedup from compiled Ply code that a
   user can reproduce with a documented command.** ADR 0018 §0.5's kernel ratio
   is a different measurement — the spike's own harness, body-only, no front end,
   no cache check, no JIT compile in the window — and this does not replace it.
3. **cranelift on `examples/` is several times *slower* than running no
   backend.** That is not noise and it is not a mystery: most of the window is
   the analysis plus code generation, and what it buys is entry on about one
   percent of offers. **Per-run compilation costs more than the ceiling is worth
   on that workload.**

**Item 3's magnitude does not transfer, and the correction is in this ADR's
favour.** An adversarial re-take, five arms **rotated one slot per window** so
that a within-rotation position effect and an arm effect cannot be confused,
replicated both series — the fixed order was hiding nothing — and added the row
this section was missing: **ADR 0030's own workload**, `spikes/ply-parser/`
driven by a generated probe over the examples as byte literals.

- **On the real front end the code generator is a few percent slower, not
  several times slower.** The large loss is sound as a fact about
  `ply test examples/` and is an artefact of corpus length as a fact about
  anything else: that run is short enough that the fixed cost *is* the window,
  while ADR 0030's workload runs long enough that the same fixed cost is a small
  fraction of it. **The sign is right; the magnitude is not transferable.** This
  ADR reached for ADR 0030's ceiling over a corpus that ceiling was not about,
  and ADR 0030 separately calls `examples/` *"the workspace's own test corpus and
  not a program anyone is trying to make fast"*.
- **What actually costs the code generator the front end is the fragment, and it
  is narrower than `Reference`'s.** Over that corpus the tree-walking backend
  enters nearly every offer across dozens of definitions; cranelift enters well
  under half as many across a handful — **exactly the figure ADR 0030 names as
  the pre-`Bytes`-widening `Int | Bool` rung.** So `crates/ply-codegen` reaches
  less than half of what the seam's tree-walker reaches on the workload ADR
  0021's bootstrapping goal turns on. **That is a fourth route out and it is not
  on the list below: widening the code generator's fragment to what `Reference`
  already reaches. It is the only one of the four whose size is known in
  advance.** Nothing measured is above ADR 0030's ceiling and none of this
  challenges it.

#### What would change item 3, and what was deliberately not done

Three routes, none measured, all named so that the next change starts from a list
rather than from the number:

- **Compile fewer definitions.** The unit compiles the whole closed set, of which
  a small fraction is enterable. Only definitions reachable from an enterable one
  can ever run, and pruning to that closure would cut both halves. **It needs a
  call graph the fixpoint currently discards.**
- **Compile once instead of once per worker.** The blocker is named in
  `ply_codegen::Cranelift`'s header: **a `Value` is `Rc`.** Making the constant
  pool shareable is a second representation of every constant, or an `Arc` audit
  of `Value`, and neither was attempted.
- **Compile lazily, on the first offer a worker actually receives.** Cheapest of
  the three and helps least on these corpora, where the workers that exist do get
  offers.

**None of this is a reason to have deferred.** §4.5's whole argument is that
policeability comes before speed, and the seam is now policed by eight
configurations against a real code generator from a command a user runs. **A slow
backend that is checked is the thing this ADR asked for; a fast one that is not
is what it refused.**

---

## 5. What this ADR does not decide

- **Which backend.** Cranelift, LLVM, a bytecode VM with a template JIT, or
  ahead-of-time compilation are all still open. §4.3's criteria are written
  against a `Compiled` implementation and say nothing about how one is produced.
- **The cache-key objection.** ADR 0016 §2.2's "an opt-in JIT is an opt-in cache"
  and ADR 0011 §4's refusal of that shape for `--host` are unanswered here.
  §4.6's stage two enforces that a backend run does not *write* the cache; it
  does not answer what a cache key should be for a world where a backend is the
  normal path.
- **Determinism.** ADR 0016 §2.2 names it as the sharpest cost — a backend that
  reassociates arithmetic or reorders argument evaluation breaks seeded replay
  and `proved` **silently**. The present seam evades it by being off inside a
  `simulate` region and by handing back at most one scalar. **A backend that
  matters will not evade it, and nothing here says how it is checked.**
- **The toolchain.** M9 forces a version *choice*, not a toolchain *move*: the
  cranelift line the workspace pins declares a `rust-version` the repository
  already meets, and the next minor moves it. **The 1.94 floor was a property of
  the version the spike pinned, not of cranelift** — read off the crates.io index
  per release, and then run rather than read, by compiling and *calling* a native
  body through the older cranelift on the pinned toolchain. **The check was seen
  to fail before it was believed**, by flipping one expected answer.

  What M9 still forces is the other half of this ADR's cost, and that half is
  unchanged: **a cranelift dependency puts dozens of packages into the shipping
  `Cargo.lock` even with the feature off and the crate excluded from
  `workspace.members`. A lockfile entry is not conditional on a feature.**
- **Whether the kernel ratio holds anywhere else.** One kernel, one program, one
  box, one pre-registered run whose pre-registration forbade re-running it,
  through a seam that passes only `Int` and `Bool`. ADR 0018 §0.5 lists this among
  what a reader still does not know and this ADR does not move it.

---

## 6. What must be built, and the test that says it happened

In dependency order. Each line names the artifact that makes it checkable,
because a decision recorded without one is what §4.7's amendment is trying not to
repeat.

1. **Stop the ladder claiming M9, in code.** `w6::Verdict::label` and
   `Decision::reopens_at` name the served HTTP workload; `Decision` carries the
   workload its share was taken on. *Checked by* an existing test's expectations
   moving, plus one asserting the rendered verdict names a workload. **The prose
   half is done**: ADR 0016 §10.2's reopen sentence is annotated in place, so a
   reader who never reaches this file still sees that the sentence no longer
   decides M9.
2. **The cache tripwire**, §4.6 stage one. *Non-vacuity demonstrated by* adding a
   production `set_compiled` call site and recording that it goes red, in the
   file's header, the way `armed.rs` records its own.
3. **The eight mutants, in the workspace**, over the `Compiled` doubles in
   `crates/ply-eval/tests/`, **with the offer/answer counts asserted before the
   catch is asserted** — the three-step shape `mutations.rs` already uses, whose
   middle step is the one usually missing. *Checked by* the same self-test that
   file applies to itself.
4. **The spike is deleted** the day 3 lands, per §4.7, and
   `.github/workflows/ci.yml`'s `spike` job with it.
5. **C3 on the compute kernel.** Price ADR 0019 §5 item 5's `sqrt` and `ln` as
   prelude builtins end to end on `benches/kernel` — the current figure is
   inferred by Amdahl over three fields of one file and is not a measurement —
   and re-take ADR 0018 §4's `Map`/record/list share on a hybrid run rather than
   on the pre-R5 attribution it currently rests on.
6. **C4's third clause**, which is M9's own first task and not a prerequisite to
   it: `--engine both` attaching a backend, `backend_escapes` beside
   `cache_escapes`, and the eight caught by a command a user can run.
7. **C1 and C2 on the bootstrap front end**, which is the measurement ADR 0021 §4
   item 3 says is missing and **the one workload where a positive answer would
   order M9 against something already accepted.** It needs no CLI work and no
   backend the seam can enter today: the executed-work share of
   `spikes/ply-lexer` inside the fragment is a census, and the entry-granularity
   cost is the question `entry_cost.rs` was built next to.
8. **If the compiled-workload target is real, write it down.** ADR 0021 exists
   because a rationale that lives only in a conversation leaves the next reader
   with a rejection and no goal behind it, and **§3.1 shows this ADR was nearly
   the same failure one document later.** An ADR-0021-shaped statement of what
   Ply is for at run time — or a `README.md` that keeps saying "research
   language" — is what settles whether §4.1 has a second basis or only the one.

---

## 7. What would make this wrong

In order of how hard it would be to see.

- **The replacement C1 is a work share, and nobody has taken one on an HTTP
  request.** The kernel's share comes from ADR 0019 §5 and is a body-only
  accounting — **the same accounting ADR 0018 §0.5 showed produces a *wrong
  ceiling***, because it charges the call-site machinery to an unattributed
  bucket that entry also deletes. If a work share is systematically wrong in the
  same direction, **C1 is a bar that passes things it should refuse, and it is
  the criterion this ADR moved.** The check is to compute both shares on the same
  workload and see whether they disagree by more than the residue. **This ADR did
  not do it.**
- **§4.1 rests on one item of one ADR, and that ADR decides no implementation.**
  ADR 0021 is "accepted as a statement of intent. It decides no implementation",
  and its §4 item 3 inherits that standing. If the bootstrap track is abandoned,
  or if a token-granularity entry turns out to be a different thing from a
  backend a user reaches — the seam admits whole definitions and refuses anything
  but `Int` and `Bool`, and a lexer's inner loop passes `Bytes` — then §4.1's
  basis is gone and the answer reverts to "not decidable here". **§3.1 is written
  to be separable for exactly this reason.**
- **The framing this ADR was commissioned under is not in the tree, and it would
  have carried the decision if §3.1 had not checked.** A first draft opened §4.1
  with "a language whose stated target is the work Go, Java and Swift carry", and
  no file in this repository says that. **If a later revision quietly restores
  it, that revision is the defect**, and §6 item 8 is what should have happened
  instead.
- **A ratio is not a product decision.** A large speedup on an MCTS kernel,
  against Rust at roughly an order of magnitude, **may leave Ply in the same place
  relative to the language a user would otherwise pick.** §4.3 has no criterion
  for that at all, and adding one is not obviously possible.
- **The mutation harness may not survive the move.** §4.5 asserts the eight are
  wrappers over `Compiled::enter` and so need no code generator. Two already need
  special handling, and a third — `stale` — needs a corpus that *varies
  arguments*, which the workspace corpus does by running real programs rather
  than by generating cases. **If the moved harness turns out to have less
  sensitivity than what it replaced, §4.7's deletion condition will have been
  satisfied on paper by something weaker.** The mitigation is that the condition
  names measured sensitivity and not a test count.
- **The whole-kernel oracle is weak and the replacement C4 may inherit that.**
  Every compiled function except a search's entry points is offered zero times
  during a search. **A future harness built on whole-program runs alone would
  report the same green over a much smaller space.** C4's second clause is
  written against that and does not prevent it.
- **Keeping the seam may cost something nobody has measured.** Hundreds of branch
  tests per `/health` request, wall clock never taken, on either of two binaries
  that both existed. **If that number is not free, §4.8's third reason is wrong
  and the seam is a permanent tax on the engine that ships for the benefit of one
  that does not.**
- **And the ordinary way**: if §6's list is still unbuilt in six months, **this
  ADR will have been a fourth document about an obligation instead of the end of
  one**, and §4.7's amendment will have been the mechanism. The condition was
  chosen to be checkable for exactly that reason, and a reader who finds the
  spike still present and item 3 of §6 still unwritten should treat this document
  as the defect.

---

## Not in this ADR

The backend itself; the choice of code generator; monomorphization, unboxed
mutable arrays, evidence passing and the rest of ADR 0018 §2–§7, none of which
has an end-to-end price on a workload that can enter compiled code; the cache key
a compiled path needs; and the determinism check a reassociating backend would
require. Each is a milestone, and this one decides only which question they are
ordered by.
