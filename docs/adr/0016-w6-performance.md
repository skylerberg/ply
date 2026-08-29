# ADR 0016 — W6: where the time goes, and whether M9 comes forward

Status: accepted — **decided: keep deferring M9** (§10). The verdict stands and
has **not** been re-taken since ADR 0017 changed the representation it was
measured against; see the note below.

> **Added by the W6 documentation audit.** ADR 0017 landed after this document
> and states in its own Consequences that "codegen's ceiling should be
> re-measured after this lands, because ADR 0016's 1.05× was a verdict on the
> old representation and this ADR changes exactly what made that ceiling low."
> That re-measurement has not happened, so every number in §8 onward — `S`,
> `k`, `E`, the ceiling — is a reading taken against the forkable world and a
> `Machine` that no longer exist. The verdict is unchanged because none of the
> four criteria moved in M9's favour; the *evidence* for it is one milestone
> stale, and a reader should not treat §10's table as current.
>
> One thing ADR 0017 did do to §4's roster: it attacked "boxing on hot paths"
> and reported the result. It is **still `priced: false`** in
> `benches/w6-ladder.json`, correctly — ADR 0017 measured allocations per
> request (1,035 → 1,082 → 1,122, i.e. *up*), and C3 requires an end-to-end
> speedup on §1.6's served workload, which is a different measurement. So C3
> still reads 1 of 7 priced, and `Decision::reopens_at` still composes the same
> sentence. A future contributor pricing that lever should know the allocation
> reading already exists and points the wrong way.

W6 answers one question — *where does a request's time go now, and is native
codegen the right next lever* — and it closes the web track. ADRs 0011 through
0015 each settled what a milestone builds. This one settles what a milestone
**decides**, which is a different kind of document: its deliverable is a number
and a verdict, and the only way a verdict of that shape is worth anything is if
the criteria are fixed before the numbers exist.

Sections 0 through 7 were written before any W6 measurement was taken and are
unchanged, except that `examples/desk.ply`'s route table is **eleven** routes and
not ten — a description W3 got wrong and every document since repeated, corrected
throughout rather than left standing beside a table of measured numbers; that
§2.6 said `Criteria` holds "nine" numbers where it holds and always held
**eight**; and two block quotes that point at §12 where a claim written before
the numbers turned out to be half of one. **The result is §8 onward**, at the
end, in that order deliberately: a reader who wants to check that the bar was not
moved reads the bar first.

**§8 onward is a second take.** The first was published against a tree that no
longer exists: the constant memo landed in `ply-eval` afterwards, which is one of
§4's own cheaper levers *built* rather than priced, and it removed most of two
rungs. Two audits found it, along with a path from a measurement file to C3 — the
criterion the whole verdict turns on. §12 is what they found and what changed;
the thresholds in §2 are not among the things that changed.

> **The verdict, for a reader who wants it now: keep deferring M9**, on three of
> the four criteria. The interpreter is **35%** of a request, so an *infinitely*
> fast execution strategy is worth **1.55x** end to end; the spike hit **11.67x**
> on its function, which projects **1.48x** — under the 1.50x §2.3 fixed as what
> a permanent second execution path has to buy; and **one** of the seven cheaper
> levers in §4 has been priced, because it landed, at **1.15x** on the ladder's
> own workload. §2.5 makes each of those independently sufficient.
>
> The share was **67%** in this document's first take. It fell because a cheaper
> lever took interpreter time out of the request, which §10.2 named in advance as
> the most likely way it would move: *"that is success, and it lowers M9's ceiling
> at the same time."*

## The rule everything else follows from

> A decision made after the numbers arrive is a decision fitted to them. So the
> **criteria are pinned first**, the **measurement is by substitution** so that
> no in-machine timer has to be trusted, and the **honest ceiling is stated
> plainly** — because a flattering number that a reader later discovers is
> flattering costs more than the truth would have.

Three corollaries, each of which decides something below:

1. **The thresholds live in code, not in prose.** `ply_corpus::w6::Criteria`
   holds them, `w6::decide` applies them, and `w6::Report` — the file the
   measuring runs produce — carries *no* criteria field and *no* verdict field.
   There is no path from a measurement to the bar it is about to clear (§2.6).
2. **A layer is a difference between two absolutes taken in one arena in one
   run.** Not a timer inside the machine, not a profiler's attribution, and not
   a number quoted from a previous milestone. A rung that cannot be expressed
   as one substitution is not a rung (§1.1).
3. **What the ladder did not separate is printed as the residue and credited to
   nobody.** In particular it is *not* credited to the interpreter, which makes
   the share M9's case rests on a lower bound (§1.4).

## 0. What is already known, and what it does not say

Every number in this section was taken by the milestone named beside it. W6
re-takes all of them on one machine in one run, because a table assembled from
five milestones' machines is a table about five machines.

| taken by | number | what it says |
| --- | --- | --- |
| M9's deferral | execution was reported as **4.2%** of a warm 10,000-definition `ply test` run; re-measured, it is **~29%** (see the note below) | in a *test* run the front end and the cache are still the larger cost, but a faster evaluator moves more than nothing |
| W1 | **72%** of a request was above the socket; **5.41µs per byte** of request head; **175–350 req/s** on a browser-sized request | serving inverts M9's argument: there is no front end on the request path, so the interpreter *is* the request path |
| W1 | the host boundary cost **0.5µs** of a **601µs** request | the boundary ADR 0008 was most worried about is not a cost; it is a correctness surface |
| W2 | a request went **527.7µs → 109.8µs**, **1,895 → 9,109 req/s**; over a head sweep **84x** the bytes went from costing **29.29x** the time to costing **1.00x** | the 5.41µs/byte was five O(n) folds boxing an `Int` per byte with no early exit — an algorithm, not a constant factor |
| ADR 0005 | the control-stack machine costs **four heap allocations per frame push** | a lever that is neither codegen nor an algorithm |

> **Audit note on the M9-deferral row: 4.2% is unsourced *and* contradicted by
> measurement.** An earlier pass of this note enumerated every other row of the
> table as sourced or unsourced and left this one out, which let §0's header
> ("every number in this section was taken by the milestone named beside it")
> read as re-verified for it. It was not. `grep -rn '4\.2%' --include='*.md' .`
> finds the string exactly twice in the tree: here, and at ROADMAP.md:286
> pointing back here. The audit re-took the warm-run profile on the documented
> corpus — `ply-corpus gen --out <dir> --seed 1 --modules 200 --defs-per-module
> 50 --tests 5000 --depth 6`, then `ply-corpus bench <dir> --repeats 3` — and
> the `warm` scenario prints `execute 125.47 ms 29.2%` of a `total 429.04 ms`.
> README.md publishes an independent take of the same scenario at **125.1ms of
> 437.2ms, 28.6%**, and withdraws the sibling `10.4ms / 3.3%` it used to carry;
> ROADMAP.md:270 records that withdrawal. Two takes agreeing at 28.6% and 29.2%
> against a published 4.2% is a wrong row, not machine noise.
>
> One caveat in the interpreter's favour, from `benches/README.md`: `bench`
> builds a worker per pool thread per concurrency group, so its `execute` phase
> carries per-group setup and **over**-states interpreter time. The true share
> is bounded above by ~29% and below by whatever `ply-corpus measure` separates
> out. What it is not is 4.2%.
>
> **What this does and does not do to the decision.** It does not overturn it:
> at ~29% the front end plus hash is still ~68% of a warm run, so the test-run
> argument for deferring M9 survives in direction. It does mean §0's opening row
> understates the evaluator's share in a test run by about 7x, and that the
> decision should be argued from the *served* profile in §8.1 — measured by this
> milestone, on this machine — rather than from this row.

> **Audit note on the two W1 rows and the W2 row: most of those figures are not
> recorded anywhere in this repository, and where the same measurement *is*
> recorded elsewhere it disagrees with them.** The header above says "every
> number in this section was taken by the milestone named beside it"; that is a
> provenance claim, and it does not hold for these.
>
> - **Sourced:** `5.41µs per byte` (ADR 0012 §5 and CONTRACTS.md's byte-builtin
>   section both carry it) and ADR 0005's `four heap allocations per frame push`.
> - **Unsourced:** `72%` above the socket, `175–350 req/s`, the host boundary's
>   `0.5µs` of a `601µs` request, and W2's `527.7µs → 109.8µs`,
>   `1,895 → 9,109 req/s` and `29.29x`. Each of these strings occurs exactly
>   once in the tree — here. ADR 0011, W1's own contract, contains **no numeric
>   measurement at all**, so W1's own exit criterion ("a measured per-request
>   interpreter cost") has no recorded value to quote.
> - **Contradicted.** CONTRACTS.md's "The re-measurement — landed" records the
>   same W2 before-and-after, with its rig named (`ply-corpus serve --repo .
>   --baseline`, one thread, machine engine, 63-byte head), as **714µs → 221µs**
>   for a whole request over a real socket, **1,401 → 4,528 req/s**, and a head
>   sweep in which 84x the bytes cost **39x** the time before and **0.95x**
>   after. That is a different pair and a different sweep ratio from this row's.
>   Re-running that rig for the audit gave 617.4µs → 135.6µs, 1,620 → 7,372
>   req/s, and the tool's own printed sweep ratios of **26.27x → 0.93x** — which
>   sits nearer this row's 29.29x than CONTRACTS.md's 39x, so the two documents
>   are not reconcilable by calling one of them the newer take.
>
> **This matters beyond bookkeeping, because `4.8x` is derived from it.** The
> "W2 delivered 4.8x by attacking an algorithm" claim used in §2.3, §4 and §10.1
> — and carried in `benches/w6-ladder.json`'s `alternatives` prose — is
> 527.7 / 109.8. Computed instead from the pairs that *are* recorded, the same
> lever is 3.23x (CONTRACTS.md's whole request) or 7.46x (its `answer` alone);
> re-measured for the audit it is 4.55x and 13.3x respectively. 4.8x is inside
> that spread, so the argument it supports survives — but it is one reading of an
> unrecorded pair, not a milestone's published result, and §10.1 leans on it as
> though it were the latter.

What none of it says: **what the whole W5 stack costs.** W3 added framing,
routing, keep-alive and TLS; W4 added a database; W5 added a sink, a
configuration and a drain. Each was priced against the thing it replaced and
none of them against the total, so the accumulated bill has never appeared in
one place. §1 is how it appears, and §5 is what must be said about it.

**And the reason the W2 result is the governing precedent rather than a
footnote.** W1 predicted codegen would be the second lever and was right for the
wrong reason: what fixed the request was not a faster interpreter but *fewer
passes over the buffer*. A milestone that reaches for a code generator before it
has priced the algorithmic levers is repeating the mistake W2 corrected. §4 is
that list, and an unpriced entry on it is on its own sufficient to keep
deferring (§2.5).

---

## 1. The workload ladder

### 1.1 Measurement is by substitution, not by instrumentation

W1 established the method and W3, W4 and W5 each restated it. It is worth
stating once more as the rule it is:

> Every rung runs **the same program**. Two measurements are taken that differ
> in exactly **one** thing underneath it, in the **same arena**, in the **same
> run**. The layer is their difference. Nothing is timed from inside the
> machine, because a timer inside the machine is a claim about where a boundary
> is, and the boundary is what is being measured.

Three consequences, and all three are load-bearing:

- **A rung needs two numbers, not one.** `w6::Point` carries `with_micros` and
  `without_micros` and refuses to be built from one of them. A ladder of
  cumulative totals silently assumes each rung's baseline is the rung below,
  which is true in one arena and false across the seam (§1.3).
- **A negative layer is a result.** It means the substitution did not isolate
  what it claimed to. `w6::Ladder` reports it, `Report::audit` names it, and
  `decide` returns `Undecided` rather than reading a share off a ladder carrying
  one larger than 5% of the total. A clamp to zero would turn a broken
  measurement into a plausible one.
- **A rung names the route it was taken on.** Two rungs on two routes have a
  difference that is a route change as well as a layer. Sometimes there is no
  alternative — the `database` rung needs a route that reaches a database — so
  the column is printed and the mismatch is an audit finding rather than a
  refusal.

### 1.2 The nine rungs

The ladder is built around **one route**, `/items` on `examples/desk.ply`: one
select, a list of records through a derived JSON encoder, which is the shape a
real read endpoint has. Where a rung cannot be taken on it — because a pure call
would need a store the handler performs for — the rung is taken on `/health` and
the report says so.

| # | rung | `with` | `without` | the layer |
| --- | --- | --- | --- | --- |
| 1 | `call` | `Machine::call` on a function returning a constant | not calling the machine | entering the interpreter at all: environment, frame push, return |
| 2 | `endpoint` | the route's handler over an in-memory store value | rung 1's `with` | the route's own body, through its derived JSON encoder |
| 3 | `framing` | `std.http::parse_head` and `std.http::encode` around the same handler | rung 2's `with` | HTTP/1.1: request line, field block, `Host`, length, response encode |
| 4 | `routing` | `table()` and `route_of()` above the framed call | rung 3's `with` | building the eleven-route table from its pattern strings, and one match |
| 5 | `machine` | the whole `serve_one` over `SimNet` | rung 4's `with` | the loop around the pure pieces: `recv`, the handler-stack walk, the perform, the body, `send`, keep-alive, teardown |
| 6 | `socket` | the same loop over the real `ply_host` TCP handler, in process | rung 5's `with` | the socket, the reactor, the blocking pool, the `Pending` token |
| 7 | `tls` | the served binary with `--tls`, steady state | the same, plaintext | the TLS record layer, handshake excluded |
| 8 | `database` | `run` — postgres behind the `db` atoms | `run_memory` — `std.db`'s twin | the postgres boundary, the wire, and the server |
| 9 | `tracing` | `--trace json` to `/dev/null` | `--trace off` | the sink: encoding a record and writing it |

And the denominator every one of them is read against:

| `rust-floor` | the same accept/recv/send/close in Rust, answering the same bytes | what the syscalls cost with no interpreter under them |

Four things about this table are decisions rather than descriptions.

**`call` exists as its own rung** because every rung above it pays it, and
because it is what the spike's comparison pays on both sides (§3.3). Without it,
`endpoint` would be reported as the cost of a route body when part of it is the
cost of having an interpreter at all.

**`tracing` is the sink and not the perform.** ADR 0015 §1.4 is explicit that
there is no configuration in which a trace operation is not performed — a row
cannot be conditional on a flag — so `--trace off` still builds the `Fields` map,
still performs, and still walks to `ply_host::trace::discard`. That cost is
inside rung 5, where `--trace off` already pays it. Rung 9 is only what the JSON
encoder and the write add. W5's `events` table already separates `bare` from
`discard`; W6 does not re-derive it, it cites it.

**`tls` is read off the steady state and not the handshake.** W3's README states
the trap and it applies unchanged: `desk.ply` serves one connection at a time
and the handshake completes on the server's first `recv`, so a client that
connected while the server was busy times the queue. The rung is taken with
keep-alive on, over enough requests per connection that the handshake is a
rounding error, and the handshake is reported separately.

**`database` is taken on a route that has one.** Its `without` is the same route
against `std.db`'s in-memory twin — the substitution W4 built the twin for — so
the difference is postgres and not a different endpoint.

### 1.3 The two arenas, and the seam between them

Rungs 1–6 are **in process**: one thread, the machine engine, no CLI, no child.
Rungs 7–9 are **served**: the real `ply` binary over loopback, because `--tls`,
`--db` and `--trace` are flags and a flag substitution needs a process to pass
it to.

That seam is real and the ladder does not pretend otherwise. `w6::Arena` is a
printed column. The in-process rungs are not "the first six-ninths of the served
total" — they are what those layers cost with a syscall's variance kept out of
the middle of them, which is the only way the parse, the route and the encode can
be measured at all. `w3::stages` made the same choice for the same reason.

What ties the two together is that **rung 6 and the served baseline measure the
same configuration**: the same route, the same store, plaintext, `--trace off`.
Their difference is the process boundary, the CLI's reactor placement and the
client's own cost, and that difference is part of the residue.

### 1.4 The residue is printed

`total_micros` is **measured**, not summed: it is what the fully-configured
served stack delivered per request, end to end, at the concurrency that
maximizes throughput. The rungs are then checked against it:

```
residue = total − Σ layers
```

and it is printed on its own line. Every profile-shaped table in the industry
either omits this or folds it into the nearest plausible layer; both are ways of
claiming an attribution the measurement did not earn.

**The residue is not credited to the interpreter.** `interpreter_micros` is the
sum of rungs 1–5 and nothing else, so `interpreter_share` is a **lower bound**.
That is deliberately conservative in the direction M9's case rests on: if the
share clears the bar without the residue, it clears it.

> Written before the numbers, and half right. It holds for a *positive* residue.
> A negative one is the layers summing to more than the request they were read
> against, and leaving that uncredited inflates the numerator instead — so it is
> charged back, and `decide` reads `Ladder::conservative_share`. §12.3.

### 1.5 The engine substitution

One more table, and it costs almost nothing to take because both halves already
exist: **the same request under the tree-walking evaluator and under the
control-stack machine.**

This is the cheapest empirical handle on how much of a request is *dispatch* as
opposed to native work sitting under it. `bytes_scan_until` is `memchr`;
`Value::Bytes` is an `Arc<[u8]>` allocation; a derived JSON encode is largely
`String` building. None of those get faster because the code around them was
compiled. If swapping one entire interpreter for another entire interpreter
barely moves a request, a third one will not either.

It is also the measurement that prices ADR 0005's four-allocations-per-frame-push
as a **lever** rather than as a fact, which is why it is in §4 as well as here.
`--engine both` already exists to police the two for agreement, so this is the
one table W6 takes that is free of new machinery.

### 1.6 What the ladder is measured on

Stated once, so no row is ambiguous:

- `examples/desk.ply`, read from the repository, rewritten only in the ways
  `w3::Service` and `w5::project` already rewrite it. The service under
  measurement is the one W5 shipped, not a copy that can drift.
- The task-per-connection accept loop for every served row. A sequential server
  answers one connection at a time, so a tail latency at concurrency 8 is a
  queue and not a service. The sequential loop is reported once, separately, and
  labelled.
- Release profile. A debug measurement of an interpreter is a measurement of
  `debug_assertions`.
- Best of N repeats, N reported. Best-of rather than mean because the quantity
  of interest is the cost of the work, and everything a run adds to it — a
  scheduler preemption, a page fault, a client thread — is additive noise.
- The request head length is printed on every table. W1's endpoint's cost was a
  function of head length and W2's very nearly is not; a load number quoted
  without its head length says nothing under the first regime and almost
  everything under the second.
- Postgres: this harness's own instance, on a non-default port, with
  `examples/desk.sql` loaded. Its version is in the provenance.

---

## 2. What would justify M9

### 2.1 The four criteria

Written here before any W6 number exists. `S` is the measured interpreter share
(§1.4), `k` is the spike's measured speedup (§3), `E = 1/((1−S) + S/k)` is
Amdahl's projection of `k` onto `S`, and `A` is the best **measured** end-to-end
speedup among the alternatives (§4).

> **M9 is justified only if all four hold.**
>
> **C1 — Share.** `S ≥ 0.50`.
> **C2 — Ceiling.** `k ≥ 3.0` and `E ≥ 1.50`.
> **C3 — Nothing cheaper.** Every alternative in §4 is **priced**, and
> `(E − 1) ≥ 2 × (A − 1)`.
> **C4 — Correctness.** The spike produced values equal to the interpreter's on
> every input it was given, and its samples separated from the interpreter's.

### 2.2 What M9 costs

The criteria above are meaningless without this, because "is it worth it" is a
comparison and the right-hand side has never been written down.

**A dependency.** `cranelift-jit` and `cranelift-codegen` are large. W6 measures
the transitive crate count and the clean release-build delta and reports both,
because "large" is a number and this project has a habit of taking the number.
`ply-eval` currently depends on `memchr`, `rpds`, `rust_decimal`, `rustc-hash`,
`stacker` and `blake3` — six crates whose whole content is something it would be
foolish to write here. Cranelift is a different order of thing.

**A second execution path, permanently.** `--engine both` exists precisely to
police one pair of engines for divergence, and `E0503` is what it raises. A
third engine makes that three pairs. `crates/ply-eval/src/differential.rs` and
every corpus it runs would have to cover the new one, or the guarantee weakens
silently — which is the failure mode every audited milestone has produced and
none has produced as a crash.

**A cache key.** `RUNTIME_VERSION` keys `(runtime_version, test_hash) → Pass`.
A cached pass is a claim about what the evaluator did, so an engine that
evaluates differently must move it — and if the engine is a *flag*, the cache
splits on a flag. ADR 0011 §4 refused exactly that shape for `--host` and gave
the reason: `ply check` must not disagree with itself. A JIT that is opt-in is a
cache that is opt-in.

**Determinism.** A seeded simulation replays exactly because evaluation order is
fixed. `proved` is a truth claim discharged over a fixed evaluation. A backend
that reassociates arithmetic, reorders argument evaluation, or evaluates a
short-circuit differently breaks replay and tier honesty **silently**, and a
silent break in this system is worse than a loud one by the whole margin the
project is built on.

**Maintenance, forever.** Every new builtin, every `Value` variant, every
handler-stack change lands twice. W2 added twenty-odd builtins and two `Value`
variants in one milestone; that is the rate.

Against all of that, a 1.2x is not a reason. That is why C2's floor is 1.50x and
why C3 asks for double the best alternative's *gain*.

### 2.3 The thresholds, and why each is where it is

**`S ≥ 0.50`, and defer below `S < 0.35`.** Amdahl's ceiling is `1/(1−S)`
whatever the backend does: 2.00x at 50%, 1.54x at 35%. Fifty percent is the
point at which an *infinitely* fast execution strategy is worth 2x end to end,
and 2x is the least that buys a permanent second execution path (§2.2). Below
35% the ceiling is under 1.6x and no spike result can change that, so the
correct response is to report the ceiling and stop — not to measure harder.

**`k ≥ 3.0`, and defer below `k < 2.0`.** A first Cranelift backend for this
language will not deliver an order of magnitude, and it is important to say why
rather than to hope: values stay boxed (`Value` is an enum holding `Arc` and
`Rc`), the scan primitives are already `memchr`, effects still walk the handler
stack, and a `perform` still has to reach a host binding. What codegen removes
is node dispatch, the frame push, and the `Env` walk — real, but a constant
factor on part of the work. Under 2x it is inside the range a *single*
algorithmic change has already delivered once, and W2 delivered 4.8x that way.

**`E ≥ 1.50`.** The projection is what actually arrives. Everything in §2.2 is
paid whether the win is 1.2x or 2.0x.

**`(E − 1) ≥ 2 × (A − 1)`.** On the **gains**, not the ratios: a 1.50x and a
1.10x are a 50% and a 10% improvement, and comparing 1.50 against 2 × 1.10
compares nothing. Two times, because M9 is a permanent surface and an
alternative is one change; a near tie goes to the alternative, always.

**5% for a negative layer.** Above it the ladder did not separate and no share
read off it is trustworthy, so nothing is decided.

### 2.4 The grey band

`0.35 ≤ S < 0.50` is the honest middle, and pretending otherwise would be the
place this document cheats. In that band the ceiling is between 1.54x and 2.00x:
the share alone cannot carry M9, but a very good backend might.

> In the grey band, M9 is `Conditional` if `k ≥ 5.0` **and** C2's `E ≥ 1.50`
> still holds **and** C3 and C4 hold. Otherwise defer.

`Conditional` is a distinct verdict and it means something specific: **M9's
scope is the fragment the spike proved, not a whole backend.** A conditional
advance that quietly became "write a Cranelift backend for Ply" would be this
document being used as cover.

### 2.5 What would make the honest answer "keep deferring"

Any one of these, and each is reported with the number that produced it:

- `S < 0.35`. Report the ceiling, name the layers that own the request instead,
  and stop.
- `k < 2.0`. The spike found no headroom on a function chosen to have the most.
- `E < 1.50`. The projection does not clear what §2.2 costs.
- `(E − 1) < 2 × (A − 1)`. A cheaper lever is close enough that it wins on cost.
- **Any alternative in §4 is unpriced.** This one is independent of every number
  above and it is the W2 precedent stated as a rule: a cheaper lever that has not
  been priced is on its own a reason to keep deferring, because W1 predicted a
  codegen win and W2 beat the prediction by attacking the algorithm instead.

And two outcomes that are `Undecided` rather than `Defer`, because they call for
the opposite response — take the measurement, rather than accept the answer:

- a missing rung, so the total is not the stack's;
- a spike that is not evidence (§3.4), so `k` does not exist.

### 2.6 The criteria are in code

`ply_corpus::w6::Criteria` holds the eight numbers above with these defaults, and
`w6::decide` applies them in the order §2.5 lists. `w6::Report` — the JSON the
measuring runs produce — has **no** criteria field and **no** verdict field, and
a test asserts that a serialized report contains neither. The verdict is
recomputed from `Criteria::default()` every time the report is rendered.

> This protects the *thresholds* and it did. It did not protect C3, which was
> checked against a list the measurement file supplied, so an empty list cleared
> it vacuously. §4's roster is now in `w6::LEVERS`, beside the criteria. §12.2.

That is not ceremony. It is the same structural argument ADR 0007 makes about
tiers: there is no `tier` field anywhere, because a label that can be asserted
will eventually be asserted wrongly. A verdict that can be written into a file
will eventually be written into a file.

---

## 3. The spike

Small enough to throw away, large enough to price the ceiling. It exists to
produce `k` and nothing else.

### 3.1 Which function

**The selection rule, applied to the W6 stage table rather than chosen in
advance:** among the pure Ply functions on the request path, take the one with
the highest per-request cost whose **entire body** is inside the fragment §3.2
compiles. The whole body, because a spike that compiles half a function and
calls back into the interpreter for the rest is measuring a trampoline.

The report names the function, its node count, and what share of the
`framing` + `endpoint` rungs it accounts for, so the choice is reviewable and so
a reader can see whether the ceiling was priced on something that matters.

`std.http::read_line` is the expected answer and is stated here as a prediction
rather than a decision: it is called once for the request line and once per
header field, its body is `bytes_len`, a `bytes_scan_until`, three integer
comparisons and a call, and it is the innermost loop of the framing layer. If
the stage table names something else, the rule wins.

**Give codegen its best plausible case.** The spike is a *ceiling*, so if the
best candidate on the real request path does not clear the bar, nothing will.
What it may not do is become a case that is not on the request path at all —
§3.5.

### 3.2 The fragment

Pinned, because "compile a function" is not a specification and because the
fragment decides whether the number is a ceiling or a fantasy:

- Parameters and locals of any type, held as `Value` in stack slots.
- `Int` and `Bool` arithmetic, comparison, `&&`/`||` with their short-circuit
  order preserved, and `if`, compiled to native instructions, unboxing and
  reboxing at the boundaries.
- `let`, `block`, and `match` on `Int`, `Bool` and unit literals.
- Calls to builtins through **one** runtime trampoline of the shape
  `fn(builtin, *const Value, usize) -> Result<Value, Diagnostic>`.
- Calls to other Ply functions through a trampoline back into the machine.
- **Nothing else.** No `perform`, no `handle`, no closures, no `with_cell`, no
  `simulate`. A function containing any of them is refused, loudly, and is not
  the spike's function.

Values stay boxed and effects stay in the interpreter **on purpose**. A spike
that unboxed everything and inlined the builtins would price a language Ply is
not, and would produce a `k` that no real backend could deliver. The point of a
ceiling is that it is reachable.

### 3.3 How it is measured

- One process. The same `Machine`, the same `Value` inputs, the same function.
- At least three inputs (`w6::SPIKE_MIN_INPUTS`), spanning the range the real
  request path produces — for a head scan: a 63-byte `curl` head, a
  browser-sized head, and one near `max_header_bytes`.
- Four times per input: the interpreter's best and worst, and the spike's best
  and worst, over the same repeat count.
- **Compile time is reported separately and never amortized into the
  comparison.** A service compiles at first call, so `compile_micros` is a
  column. It is also the one place Ply has a genuine structural advantage worth
  noting: a definition is content-addressed, so a compiled body is cacheable on
  its hash, permanently, across processes. That is an argument for M9's *design*
  and it is not evidence for M9's *value*; it does not enter the criteria.

### 3.4 What is evidence, and what is noise

`w6::Spike::judge` implements this and returns `evidence: false` with every
failed rule named:

1. **At least three inputs.** A ratio over fewer is one input's constant.
2. **Agreement on every input**, by `Value`'s own ordering. A faster wrong
   answer is an `E0503`, not a speedup — this is C4.
3. **Separated samples on every input**: the spike's *worst* is below the
   interpreter's *best*. Overlapping bands mean the measurement found no
   difference, whatever the midpoints say.

And the reported `k` is the **minimum** conservative ratio — interpreter best
over spike worst — across every input. A speedup that holds on one input and not
another is that input's, so the weakest one is the claim.

### 3.5 What the spike may not do

- It may **not** be a synthetic benchmark. No `fib`, no `nbody`, no loop written
  for it. The function comes from the stage table or the spike has not been run.
- It may **not** be wired into `ply run`, `ply test` or any other command. It
  lives behind a `codegen-spike` cargo feature in `ply-corpus`, in one file, with
  `cranelift-jit` and `cranelift-codegen` as **optional** dependencies at
  `0.134.3` — so that deferring M9 deletes one feature block and one dependency
  line, and nothing else in the workspace knows it existed.

  > **Corrected in place (R5 review, 2026-08-22): the last clause is no longer
  > true, and R5 is what made it untrue.** The first half stands and was
  > verified by performing the deletion rather than arguing it — a reviewer
  > copied the tree, ran `rm -r crates/ply-codegen-spike`, and got
  > `cargo build --workspace --all-targets` and
  > `cargo test --workspace --no-fail-fast` green: 155 test binaries, 3,680
  > passed, 0 failed. No cranelift in any shipping manifest;
  > `grep -c cranelift Cargo.lock` is 0.
  >
  > What is false is *"nothing else in the workspace knows it existed"*. After
  > R5, `crates/ply-eval` carries `compiled.rs` — a public `Compiled` trait,
  > `Machine::set_compiled`, three counters on `Machine`, and a branch in
  > `Machine::enter_code` taken on every interpreted call — none of it with a
  > shipping implementor or caller, all of it surviving the `rm -r`. That is a
  > deliberate change made by R5 under ADR 0018 §0's "make the interpreter able
  > to enter compiled code", and no ADR recorded the amendment until this block.
  > It costs 0.0 allocations per `/health` request (`benches/r5-timing/`
  > §1) and 237.87 predictable branch tests, and it buys nothing that ships.
- It may **not** report a ratio whose two sides did different work — for
  instance an interpreter column that includes `Machine::call` entry where the
  spike column does not. Rung 1 is measured precisely so that cost is known and
  present on both sides.
- It may **not** be kept because it works. It is thrown away whatever the
  verdict; an `Advance` schedules M9, and M9 is a milestone with an ADR, not a
  promotion of a spike.

---

## 4. The alternatives to price alongside it

W2's byte builtins beat W1's predicted codegen win by attacking the algorithm
instead of the constant. This is the list of everything in that class, and §2.5
makes an **unpriced** entry sufficient on its own to defer.

Each is priced the same way: as a **measured end-to-end speedup on the same
served workload the ladder's total came from**. Not a microbenchmark — a
microbenchmark is how a lever that moves nothing gets adopted.

| lever | what it is, concretely | why it might be big |
| --- | --- | --- |
| **more native builtins** | fold `read_line`, `is_token`, `trim_ows` and `trim_end` into one native head scan; `string_lower`; `add_field` | this is W2's lever reapplied to the layer W3 added, and W2's was 4.8x |
| **the frame push** | ADR 0005's four heap allocations per frame push; measured by the §1.5 engine substitution and by `crates/ply-corpus/tests/frame_cost.rs`'s allocation count | it is paid per node the machine suspends inside, which is most of them |
| **`Env::lookup`** | a linear walk down an `Rc` chain, so a variable reference costs O(scope depth); priced by a depth sweep and by an indexed alternative | this is an *algorithm* on the hottest path in the interpreter, which is exactly W2's class of finding |
| **boxing on hot paths** | where a `Value::Int` per element survives; counted per request rather than guessed at | `Value::Int` per byte was W1's whole disease |
| **caching derived work** | `table()` rebuilds the eleven-route table from its pattern strings on **every request** (W3's stage table says so); a derived codec dictionary is a record built per call | a per-request rebuild of a constant is free to remove |
| **connection and statement reuse** | W4's pool and prepared-statement cache: hit rate, and what a miss costs | a miss is a round trip, which is worth more than any interpreter change |
| **response buffering** | writes per response, and the copies `bytes_concat` and `bytes_slice` make (`Value::Bytes` is `Arc<[u8]>` with no slicing — ADR 0011 §8 deferred it to W3 and W3 did not reopen it) | a syscall per fragment is a syscall per fragment |

Two notes on how to read that table honestly.

**A priced lever that bought nothing is a result**, recorded as `end_to_end:
1.00`, not omitted. Half the value of this list is knowing which of these
plausible things does not matter.

**The cost column matters as much as the ratio.** A native builtin grows the
trusted computing base `ply hosts` invites a reader to check; caching `table()`
changes when a route table can be edited; an indexed `Env` touches capture,
splice and every frame kind. `w6::Alternative` carries `cost` for that reason.
None of them is remotely M9's cost, which is the point.

---

## 5. The honest account

W6 closes the web track, so the report is the last word on what this is. It owes
six things, and `Report::audit` names any that are missing **above the tables**
rather than leaving a shorter document:

1. **The accumulated stack in one table.** §1's nine rungs, the floor, the
   residue, the measured total, and `over_floor` — how many times a Ply request
   costs the same syscalls with no interpreter under them.
2. **What a reader gets today.** Requests per second and p50/p95/p99 for a
   no-database route and a one-select route, on plaintext and on TLS, at the
   concurrency that maximizes throughput, with the head length printed — and the
   Rust floor on the same machine beside it, so the multiple is visible rather
   than inferable.
3. **Where this is genuinely not competitive**, named and, where a number exists,
   with the number. The candidates are known before the run and none of them is
   allowed to go unmentioned because it is unflattering:
   - **One machine is one core.** A `Value` holds `Rc` and a continuation is
     `Rc<Vec<Segment>>`, so a Ply task cannot move between OS threads (ADR 0011
     §9). Throughput scales by processes, not by threads, and every runtime this
     would be compared against scales by threads.
   - **No cancellation** (ADR 0011, unresolved through W5). A request live at the
     drain deadline loses its connection with no response and the run exits `3`.
   - **No backpressure and no load shedding** — W4 promised them and W5's "not
     in" list breaks the promise explicitly.
   - **`bytes_slice` and `bytes_split` copy**, because `Value::Bytes` is
     `Arc<[u8]>` with no slicing.
   - **`std.trace`'s `Sink` grows its record list in a non-final field** of the
     record `append` returns, so the twin is O(n²) in the records it holds.

     > **Corrected (mechanism sweep, 2026-08-28): the cost is right and the
     > cause was not.** This read *"**`std.trace`'s `Sink` appends with
     > `push`**, so the twin is O(n²) in the records it holds"*. The O(n²) stays
     > on this list — it is exactly the kind of unflattering item this section
     > forbids dropping — but `push` is not what makes it. `push` grows a `List`
     > **in place** when the caller is its last owner (`Arc::get_mut`, in
     > `crates/ply-eval/src/builtins.rs`) and copies the whole array only when
     > something else can still see it, and what decides that is **position**:
     > `rc::carry` (`crates/ply-eval/src/rc.rs:98`) hands a pending frame a live
     > clone of the scope whenever any sub-expression of the enclosing node
     > remains, and never asks what those sub-expressions read. `append` writes
     > `{records: push(s.records, r), open: s.open, next: s.next}` — the growing
     > field first of three — so the list is at two owners and is copied once per
     > record. `spikes/ply-lexer/GAPS.md` §1 states the rule; ADR 0020 §5.2
     > measures that it composes across call boundaries, so a correct callee is
     > made quadratic by its caller.
     >
     > This matters to a limits list specifically. The old wording's only
     > implied remedy is *avoid `push`*, which no one can act on — `push` is the
     > language's sole list primitive and `trace.ply`'s own `cons` is written out
     > of it. The real fix is one line of field order, and it is written on
     > PR #38: 0 copies on that module at 200, 400 and 800 records — **on the
     > machine engine only.** The tree-walker runs no reference counting at all,
     > so under `--engine treewalk` the sink is quadratic whatever order is
     > written, which makes this a limit of one engine rather than of the
     > library.
   - **`--engine both` costs two runs.** The divergence guarantee is not free.
   - Whatever the ladder itself says. If the residue is 30%, that is a limit and
     it goes here.
4. **The M9 verdict**, with the §2 criteria restated, the measured numbers
   plugged in, and — if deferred — the number that would reopen it.
   `Decision::reopens_at` is that sentence, computed rather than written.
5. **Provenance.** Machine, profile, date, repeats, request head length, postgres
   version. A table without it is a rumour.
6. **What was not measured, and why.** An empty `not_measured` is itself an audit
   finding, because every run leaves something out and which things is the
   reader's to judge.

**And the tone, since it is a decision rather than a description.** The right
sentence to be able to write is of the form *"a Ply service serves N requests per
second on one core, which is M times what the same thing costs in Rust, and here
is where the M goes"*. The wrong one is any sentence whose numerator was chosen
after the numbers arrived.

---

## 6. Workspace

`crates/ply-corpus/src/w6.rs` — the ladder types, the criteria, the decision, the
audit and the renderer. It carries no measurement of its own: the ladder is
assembled from `Point`s, and `w6::decide` is a pure function of a `Ladder`, a
`Spike` and the alternatives.

`ply-corpus w6 <report.json>...` merges the measurement files field by field —
so the ladder run and the spike run produce their halves independently — applies
`Criteria::default()`, and prints the tables, the verdict and the audit.
`--strict` exits non-zero on an incomplete report; `--json` emits the report, the
assembled ladder, the spike's verdict, the decision and the audit together.

The spike itself is a `codegen-spike` feature of `ply-corpus` with
`cranelift-jit = { version = "0.134.3", optional = true }` and
`cranelift-codegen = { version = "0.134.3", optional = true }`. No other crate
gains a dependency and no other crate learns the spike exists.

> **Audit note (docs pass, 2026-08-17): this paragraph describes a plan, not
> what shipped.** `ply-corpus` has no `codegen-spike` feature and no cranelift
> dependency, optional or otherwise — checked by grepping
> `crates/ply-corpus/Cargo.toml` for `cranelift`, which matches nothing. What
> shipped is `crates/ply-codegen-spike`: a separate crate carrying its **own**
> `[workspace]` table and five **non**-optional cranelift dependencies
> (`cranelift-jit`, `-codegen`, `-frontend`, `-module`, `-native`, all
> `0.134.3`). ROADMAP.md's M9 entry describes the shipped arrangement
> correctly. The "no other crate learns the spike exists" half survives and is
> in fact stronger than written: the shipped workspace's `members` list in the
> root `Cargo.toml` does not name `ply-codegen-spike` at all, which is what
> §3.5's "closing W6 deletes it with `rm -r`" depends on. The cost of that
> isolation is stated in §7 below.

`benches/README.md` gains a `w6` section describing the ladder, the criteria and
what the report owes.

## 7. Required tests

The ones whose absence would let W6 report a number that is not what it says.

**The ladder**

1. A ladder refuses a duplicated layer, an out-of-order layer, a zero total, a
   zero floor, a rung averaged over zero requests, and a rung naming no route.
2. Layer costs, the residue and the interpreter share are computed from the
   `with`/`without` pairs, and the residue is `total − Σ layers`.
3. A negative layer is reported as negative rather than clamped, and
   `worst_negative_share` finds the largest.
4. A rung taken on a different route from its neighbour is an audit finding.

**The decision**

5. `projected` is Amdahl and `ceiling` is its limit at infinite `k`.
6. A missing rung, a missing spike, a spike that is not evidence, and a ladder
   with a large negative layer are each `Undecided` — never `Defer`.
7. An unpriced alternative defers whatever the share and the spike say.
8. `S = 0.50` with `k = 3.0` and a weak alternative advances; `S = 0.10` with
   `k = 20` defers and names the ceiling; an alternative within half the
   projected gain defers and says so.
9. The grey band advances conditionally only at `k ≥ 5.0`.
10. A serialized `Report` contains no `verdict` and no criteria, and a
    round-tripped report decides identically.

**The spike**

11. Fewer than three inputs, a disagreement on any input, and overlapping bands
    each make the spike not-evidence, and the failure names which.
    *Enforced by `ply_corpus::w6::tests::a_spike_is_evidence_only_when_it_agreed_and_separated_on_enough_inputs`
    (`crates/ply-corpus/src/w6.rs:2005`), which is in the workspace and runs.*
12. The reported speedup is the minimum conservative ratio over the inputs.
13. The spike refuses a function outside the §3.2 fragment, loudly, naming the
    construct — a spike that silently fell back to the interpreter for part of a
    body would report a ratio for a program it did not compile.
14. On its chosen function, over its whole input set, the spike's output equals
    the machine's — and equals the tree-walker's, so the comparison is against
    an evaluator that is already policed for divergence.

> **Audit note (docs pass, 2026-08-17): 12, 13 and 14 are written, and
> `cargo test --workspace` does not run any of them.** Tests 11–16 were listed
> as one block on the assumption of §6's first paragraph — that the spike is a
> feature of `ply-corpus`. It is not; it is `crates/ply-codegen-spike`, and
> that crate declares its own `[workspace]`, so the shipping workspace's test
> run walks straight past it. The split is:
>
> - **11, 15, 16** are pure functions of a `Ladder`/`Spike`/`Report` and live in
>   `crates/ply-corpus/src/w6.rs`'s `tests` module. They run. 15 is
>   `the_audit_names_every_section_the_report_owes` (`w6.rs:2310`); 16 is
>   `a_report_renders_its_tables_and_recomputes_its_verdict` (`w6.rs:2349`).
> - **12** is `the_reported_speedup_is_the_weakest_inputs`
>   (`crates/ply-codegen-spike/tests/spike.rs:177`).
> - **13** is `the_fragment_refuses_what_it_cannot_compile_and_names_it`
>   (`spike.rs:142`), with `a_function_inside_the_fragment_is_compiled_rather_than_refused`
>   (`spike.rs:164`) as its positive half.
> - **14** is `the_compiled_function_answers_what_the_machine_answers`
>   (`spike.rs:70`) plus `and_what_the_tree_walker_answers` (`spike.rs:80`).
>
> Running those three requires `cargo test` inside
> `crates/ply-codegen-spike/`. **On the audit machine that command does not get
> as far as compiling:** cranelift `0.134.3` and its transitive
> `wasmtime-internal-core 47.0.3` declare `rust-version = 1.94.0` and the
> installed toolchain is `rustc 1.93.1`, so cargo refuses resolution before any
> test binary is built. The repository pins no `rust-toolchain.toml`, so
> whichever toolchain a reader has is what decides.
>
> **What this means for the M9 verdict.** Required tests 12–14 are the ones
> that make the spike's `11.67x` a number about a program that was actually
> compiled rather than partly interpreted, so the spike half of the evidence in
> §8–§11 is unverified by any suite a reader is likely to run. The ladder half
> is unaffected: `benches/w6-ladder.json`, every share, the Amdahl projection
> and `w6::decide` are exercised by `crates/ply-corpus/src/w6.rs` and by
> `crates/ply-corpus/tests/w6_report_integrity.rs` inside the workspace. Note
> also that the verdict does not turn on the spike's magnitude — §2's C2 fails
> at 1.48x against 1.50x, and the six unpriced levers defer it independently —
> so this gap weakens the *evidence for deferring*, not the deferral.

**The report**

15. `audit` names a missing engine substitution, a missing spike, an unpriced
    alternative, a missing offering, a missing limits section and an empty
    `not_measured`; a complete report audits clean.
16. `render` prints every section and recomputes the verdict from
    `Criteria::default()`.

## Not in W6

- **A codegen backend.** Whatever the verdict. `Advance` schedules M9; it does
  not start it.
- **Keeping the spike.** It is deleted when W6 closes.
- **A profiler-derived attribution.** Corollary 2: if a layer cannot be
  separated by substitution it goes in the residue, where a reader can see it.
- **Optimizing anything.** §4 *prices* the alternatives; choosing and building
  one is the milestone that follows the decision, with the number in hand.
- **A comparison against another language's framework.** The `rust-floor` is the
  same syscalls answering the same bytes with no interpreter, which is a
  denominator. A benchmark against axum or Rails would be a benchmark of two
  I/O strategies, and neither number would say anything about the interpreter.
- **Multi-core throughput.** One machine is one core (§5), and a
  process-per-core measurement would be measuring an operating system.
- **Cancellation, backpressure, load shedding.** Still absent, still stated.

---

# The result

Everything below was written after the measurements and changes nothing above
it. The numbers come from two files in the repository —
`benches/w6-ladder.json` and `benches/w6-spike.json` — and the verdict is not in
either of them:

```
cargo run --release -p ply-corpus -- w6 benches/w6-ladder.json benches/w6-spike.json
```

recomputes every share, the projection, the audit and the verdict from
`Criteria::default()`. Both files are produced by commands rather than by hand:

```
cargo run --release -p ply-corpus -- w6-ladder --repo . --db <url> \
    --requests 512 --iterations 2000 --repeats 3 \
    --concurrency 1 2 4 8 16 32 --per-conn 32 --requests-per-point 2500 \
    --served-repeats 3 --machine <name> --postgres <version> \
    --out benches/w6-ladder.json
cargo +1.94.0 run --release --manifest-path crates/ply-codegen-spike/Cargo.toml \
    -- --half benches/w6-spike.json
```

**Provenance.** Apple M-series, macOS 24.6.0, release profile, PostgreSQL 18.3
on a non-default port, best of 3 repeats of every in-process rung and 3 repeats
of the whole served sweep, a 41-byte request head, `examples/desk.ply`'s
sequential accept loop (§8.4). The spike ran on the same machine under rustc
1.94.0 and Cranelift 0.134.3, at 2,000 iterations × 7 repeats, with its own
served denominator over the in-memory store.

**This is the second take.** The first described a tree without the constant
memo, which had landed by the time anyone re-ran it; §12 is what that cost and
what changed. Where the two takes are comparable they are noted as such, and
where they are not — the accept loop, the floor's route, the residue's sign —
the reason is in §8.4.

## 8. Where a request's time goes

### 8.1 The ladder

Nine rungs, each a difference between two absolutes taken in one arena in one
run. The total is **measured**, not summed. The `over repeats` column is the
same difference at its widest and its narrowest over the repeats behind it,
because a layer whose band spans zero has not measured its own sign.

| # | layer | with µs | without | layer µs | over repeats | share | arena | taken on |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `call` | 0.09 | 0.00 | **0.09** | 0.09–0.09 | 0.0% | in process | a constant-returning function |
| 2 | `endpoint` | 1.23 | 0.09 | **1.15** | 1.15–1.15 | 0.2% | in process | `/health` |
| 3 | `framing` | 99.06 | 1.23 | **97.83** | 97.82–98.52 | 16.5% | in process | `/health` |
| 4 | `routing` | 150.11 | 99.06 | **51.04** | 50.36–52.46 | 8.6% | in process | `/health` |
| 5 | `machine` | 255.65 | 150.11 | **105.55** | 104.14–106.21 | 17.8% | in process | `/health` |
| 6 | `socket` | 301.84 | 255.65 | **46.19** | 45.53–46.43 | 7.8% | in process | `/health` |
| 7 | `tls` | 586.80 | 583.07 | **3.73** | 2.95–10.98 | 0.6% | served | `/items` |
| 8 | `database` | 583.07 | 255.52 | **327.55** | 326.43–328.32 | 55.3% | served | `/items` against `/health` |
| 9 | `tracing` | 592.64 | 586.80 | **5.84** | −1.41–9.74 | 1.0% | served | `/items` |
| | residue | | | **−46.32** | | −7.8% | | everything no substitution separated |
| | **TOTAL** | | | **592.64** | 592.64–596.54 | 100% | | measured end to end |
| | `rust-floor` | | | **15.68** | | | | the same syscalls answering the same 270 bytes |

Five readings, in descending order of how much they change what anyone should
do next.

**The interpreter is 35% of a request.** Rungs 1–5 attribute 255.65µs of
592.64µs — 43.1% — and the residue is **negative**, which means the layers sum
to more than the request they were read against and the in-process arena
over-counts against the served denominator. Charging that seam back to the arena
that produced it gives **209.33µs, 35.3%**, and that is the number `decide`
reads (§1.4's block quote, §12.3). Over the repeats it runs 35.1%–35.4%. The
ceiling on any execution-strategy change at that share is **1.55x**.

**The database is 55% of it**, and that is the biggest single row in the table
by a factor of three. It is also the rung with the most caveats: it is `/items`
against `/health` rather than `run` against `run_memory` (§8.4), so `/items`'
own decode of three rows and its derived JSON encode are inside it. Which cuts
against the interpreter share in the honest direction — some of that 55% is
interpreter — and is why §8.4 lists it rather than a footnote.

**Framing is what is left of the interpreter.** 97.83µs, 16.5%, on a 41-byte
head with two fields: the request line, the field block, the length and the
response encode. It is now the largest interpreter layer, and the one §4's
"more native builtins" lever points at.

**The endpoint layer is a memo hit.** `/health`'s whole body is `health()`, a
nullary pure definition, so the constant memo evaluates it once per process and
every later request reads the remembered value: 1.15µs, against 0.87µs of
in-Ply loop scaffold the rung is read off, which is the resolution that rung has
and not a number to read to two decimals. The first take measured 127.37µs
here. What a route body costs when it is *not* a constant is measured beside the
ladder instead: the `/items` handler over the twin is 544.61µs a call against
344.92µs for the twin's SQL scan under it.

**TLS is 3.73µs and tracing to JSON is 5.84µs, and neither number is worth two
decimals.** Both are differences of about 1% between two served rows, and their
bands are 2.95–10.98µs and −1.41–9.74µs: the tracing rung has not resolved its
own sign. `Report::audit` says so, above the tables. What survives is the
qualitative reading, which is the same one the first take had: **the two
features that sound expensive are about 1.6% of a request between them**, and
the cheapest-sounding one — routing — is 8.6%.

### 8.2 One interpreter against another

The cheapest empirical handle on dispatch cost, and the only table W6 took that
needed no new machinery:

| engine | µs/request | requests |
| --- | --- | --- |
| tree-walker | 55.46 | 512 |
| control-stack machine | 151.58 | 512 |

**Swapping one whole interpreter for another moves the same pure request path
2.73x** — `parse_head`, `route_of`, the endpoint and the encode, same inputs,
same returned value, asserted equal before either was timed. That is ADR 0005's
four-allocations-per-frame-push priced as a lever rather than restated as a
fact, and it is a bound on what a cheaper frame representation is worth. It is
also §5's honest cost of `--engine both`: the divergence guarantee is not two
equal runs.

### 8.3 What a reader gets today

At the concurrency that maximises throughput, with a 41-byte head. The floor
column is the Rust floor **replaying that row's own response** — 65,795 req/s
for `/health`'s 107 bytes, 63,761 req/s for `/items`' 270 — because a multiple
whose two sides answer different bytes is not a multiple (§12.3).

| workload | stack | conns | req/s | p50 µs | p99 µs | vs floor |
| --- | --- | --- | --- | --- | --- | --- |
| no database, plaintext | twin, HTTP, `--trace off`, sequential | 1 | 2,860 | 344 | 439 | 23.0x |
| one select, plaintext | postgres, HTTP, `--trace off`, sequential | 1 | 1,715 | 576 | 636 | 37.2x |
| one select, TLS | postgres, HTTPS, `--trace off`, sequential | 1 | 1,704 | 576 | 623 | 37.4x |
| one select, TLS, tracing to JSON | postgres, HTTPS, `--trace json`, sequential | 1 | 1,687 | 581 | 650 | 37.8x |
| no database, TLS, tracing to JSON | postgres, HTTPS, `--trace json`, sequential | 1 | 3,778 | 254 | 289 | 17.4x |
| no database, plaintext | twin, HTTP, `--trace off`, task-per-conn | 1 | 1,790 | 555 | 649 | 36.8x |
| one select, plaintext | postgres, HTTP, `--trace off`, task-per-conn | 2 | 1,785 | 1,114 | 1,187 | 35.7x |
| one select, TLS, tracing to JSON | postgres, HTTPS, `--trace json`, task-per-conn | 2 | 1,765 | 1,118 | 1,194 | 36.1x |

**Concurrency buys nothing and costs latency**, which is the single most
important operational fact in this document, and it is true of both accept
loops. On the sequential loop, `/health` over postgres runs 3,914 req/s at c=1
and 3,930 req/s at c=32 — while p99 goes from **287µs to 252ms**, because a
sequential server answers one connection at a time and the rest are a queue. On
the task-per-connection loop the same route runs 2,157 req/s at c=1 and 2,254
req/s at c=32, with p50 from 457µs to 13,736µs and p99 from 509µs to 26,944µs.
One machine is one core (§5), so concurrency is a queue on either loop; the
spawning one merely spreads the waiting evenly.

> **Audit note: none of the ten numbers in the paragraph above are in
> `benches/w6-ladder.json`.** The `w6-ladder` command sweeps
> `--concurrency 1 2 4 8 16 32`, but the file it writes keeps only the
> `offerings` array, and that array carries just eight rows — all at c=1 or c=2,
> and none of them a `/health`-over-postgres row. `--detail` is documented to
> write the raw served rows beside the report; no such file is in `benches/`.
> One of the ten is at least derivable: 3,914 req/s is 1/255.52µs, the
> `database` rung's `without_micros`. The other nine — 3,930, 287µs, 252ms,
> 2,157, 2,254, 457µs, 13,736µs, 509µs, 26,944µs — are unsourced, and were not
> reproduced by this audit, which had no postgres to point the sweep at. They
> are also the operational claim README.md and ROADMAP.md repeat. Re-take them
> with `--detail` and commit the rows, or the next reader has the same problem.

**And the two loops are not the same service.** The sequential loop is 1.81x
faster on `/health` — 3,914 req/s against 2,157 — for a measured reason that is
new to this take and is in §8.5: a spawning service memoizes nothing. (This
sentence read "1.6x" until a documentation audit divided its own two operands:
3,914 / 2,157 is 1.81, and the latency pair beside it in §8.5 — 470.6µs against
263.5µs — is 1.79. Neither supports 1.6. The two throughputs themselves are not
in `benches/w6-ladder.json`; see the note at the end of §8.3.)

### 8.4 Where the run departed from §1

Stated because a ladder that quietly substituted something else would be a
different measurement wearing this document's authority.

- **Rungs 1–6 were taken on `/health`, not on `/items`.** §1.2 built the ladder
  around `/items` and allowed `/health` where a rung could not be taken on it.
  In the event that was every in-process rung: a pure call to the `/items`
  handler needs a store, and the only one available in process is `std.db`'s
  memory engine, whose SQL scanner is on no served request path. Including it
  would have measured the twin.
- **The `database` rung is `/items` against `/health`, not `run` against
  `run_memory`.** §1.2 specified the twin as the `without`. That substitution
  prices `std.db`'s memory engine rather than the database — in process the
  twin's `/items` handler is 544.61µs against 344.92µs for its scan alone — so
  the reported rung uses the route difference instead, which §1.1's third rule
  permits and requires be printed. It is printed, and `Report::audit` reports it
  as a route mismatch.
- **The served rows are on the sequential accept loop, not the
  task-per-connection one §1.6 pins.** This is the departure the first take made
  silently; here it is deliberate and the reason is measured. `task.spawn` opens
  a production region that stays open for the life of the server, and
  `Machine::constant` refuses the constant memo inside **any** open region — so
  a spawning service memoizes nothing (§8.5). The in-process rungs are
  `run_memory` over one connection at a time and do memoize. A ladder whose
  lower rungs memoized and whose total did not would divide a numerator in one
  regime by a denominator in another. Both loops are measured; the other one's
  rows are in §8.3, labelled.
- **The residue came out negative**, at −7.8%, and is charged back to the
  interpreter rather than credited to nobody (§8.1, §12.3).
- **One alternative in §4 was priced end to end**, and it is the one that had
  already landed. The other six carry in-process bounds or nothing. This is the
  deviation that decides the verdict, and §10.1 is why it is not a technicality.

### 8.5 Where this is genuinely not competitive

§5.3 named the candidates before the run and forbade dropping any of them for
being unflattering. All of them survived, and the run added one nobody had
predicted:

| limit | the number |
| --- | --- |
| **One machine is one core.** A `Value` holds `Rc` and a continuation is `Rc<Vec<Segment>>`, so a task cannot move between OS threads. | `/health` over postgres: 3,914 req/s at c=1 against 3,930 at c=32, p99 287µs → 252ms (sequential); 2,157 → 2,254 req/s, p50 457µs → 13.7ms (task-per-conn) |
| **A request is 37.8x the syscalls under it.** | 592.6µs for `/items` over postgres over TLS with tracing, against a 15.68µs floor replaying the same 270-byte response over plaintext with no interpreter, no TLS and no database. Like for like — `/health` over plaintext against a floor replaying its 107 bytes — **16.8x** |
| **A service whose accept loop spawns memoizes nothing.** `task.spawn` opens a production region for the life of the server, and `Machine::constant` refuses the memo inside any open region; the rule's stated reason is a `simulate` region's allocation trail, which a production region does not keep. | disabling the memo by source substitution costs **1.77x** on `/health` and **1.15x** on `/items` on the sequential loop, and **1.00x** and **1.00x** on the task-per-connection loop, where there is nothing left to disable: `/health` is 263.5µs sequential against 470.6µs spawning, on the same service |
| **The in-memory twin is slower than the database it stands in for.** `std.db`'s memory engine parses its SQL in Ply on every call. | in process the twin's `/items` handler is 544.6µs a call and its scan alone is 344.9µs of that |
| **`--engine both` costs more than two runs.** | the tree-walker is 2.73x faster on the request path, so `both` — 55.46µs + 151.58µs = 207.04µs — is **1.37x a machine run and 3.73x a tree-walk run**. (This cell read "about 1.4x a tree-walk run" until a documentation audit recomputed it from §8.2's two rows: 1.4x is the multiple against the *machine* run, which is the dearer of the two and therefore the flattering denominator. Against the tree-walk run named in the sentence it is 3.73x.) |
| **The request path allocates far more times than it writes bytes.** | one `/health` request allocates **1,035 times and 0.124 MB** to produce a 107-byte response |
| **The ladder's own residue is −7.8%.** | 592.6µs measured against 638.96µs attributed |
| **No cancellation, no backpressure, no load shedding.** | no number; the absence is the statement |
| **`std.trace`'s `Sink` grows its record list in a non-final field**, so a collecting twin is O(n²) — ADR 0015 named it and W5 measured it. (This cell read "**`std.trace`'s `Sink` appends with `push`**, so a collecting twin is O(n²)" until the 2026-08-28 mechanism sweep. The O(n²) is unchanged; `push` is not its cause — it grows a `List` in place when the caller is its last owner and copies when anything else can still see it — most often because the scope was carried past it, which is decided by the growing expression's *position* in its enclosing node. §5.3's block is the argument, and it is why the row's remedy is a field order rather than a different primitive.) | not re-taken here |

### 8.6 What was not measured

§5.6 makes an empty list its own audit finding. It is not empty; the report
carries ten entries and `Report::audit` prints them above the tables. The ones
that bear on the verdict:

- **Six of the seven §4 levers**, as end-to-end numbers: more native builtins,
  the frame push, `Env::lookup`, boxing on hot paths, connection and statement
  reuse, response buffering.
- **Anything outside the spike's fragment**: no `perform`, no handler-stack
  walk, no host boundary, no continuation capture, no closure, no derived codec,
  and no reference counting — the spike frees into an arena. §10.3 is what that
  costs the projection.
- **What a partially-covering backend is worth.** The `solo, trampolined`
  variant is one point on that curve (1.71x); nothing measured a whole request
  path compiled at partial coverage.

  > **Audit note (R5, 2026-08-21): the `solo, trampolined` variant no longer
  > exists and 1.71x cannot be re-taken.** It compiled `read_line` alone and let
  > `line_at` and `line_stops` return to the interpreter through
  > `rt_call_machine`, the escape hatch §3.2 allows. That helper was a whole
  > `Machine::call` entry point — `escape::check`, `reset()`, `close_regions`,
  > `end_entry_point` — on a second, privately held machine, which was
  > survivable only while the sole way *into* compiled code was at the top of a
  > pure integer kernel. R5 made the interpreter able to enter compiled code
  > (`ply_eval::Compiled`), so the same helper became a route out of a live
  > machine's frame into a different machine's `reset()`, discarding the
  > caller's handler stack, trail, region generations and footprint in silence.
  > It is deleted: a call to a function outside the compiled unit now refuses
  > the enclosing function at compile time, so a compiled set is closed under
  > calls. The figure in `benches/w6-spike.json` and `benches/w6-ladder*.json`
  > stands as what was measured on 2026-08-20; the variant that produced it is
  > gone, and partial coverage is now priced the other way round — the
  > interpreter drives and enters compiled leaves. `benches/adr0018-mcts.json`'s
  > `crossings_by_target`, `trampoline_tax_micros` and
  > `end_to_end_without_trampoline_tax` are the same kind of record: taken
  > before R5, not re-takeable after it.
- **What a route body costs when it is not a constant**, other than through the
  twin's `/items` handler.
- **The in-Ply loop rungs 2–4 are read off** — 0.87µs an iteration. It cancels
  between rungs 3 and 4, whose two sides are both loops, and not between rungs 1
  and 2, so the endpoint layer carries it.
- **The TLS handshake** — deliberately; the rung is steady state (§1.2).
- **Multi-core throughput** — deliberately (§5, "Not in W6").
- **`Env::lookup`'s depth sweep**, and the writes and copies a response makes.

## 9. The spike

`std.http.read_line`, chosen by §3.1's rule rather than in advance: the
innermost loop of the framing layer, called once for the request line, once per
field line and once for the terminator, and the highest per-request-cost
function whose **whole body and whole call graph** — `line_at`, `line_stops` —
is inside §3.2's fragment. 99 nodes, compiled once in 1,608µs.

**Agreement first, before anything was timed**: every input answered against the
machine **and** the tree-walker, spanning every line offset of three real heads
plus sixteen adversarial cases — empty, past-end, bare LF, bare CR, NUL, DEL, a
zero budget, a negative budget, an exact budget, one over. All agreed. That is
C4.

| input | interpreter best µs | spike worst µs | conservative |
| --- | --- | --- | --- |
| served head, request line | 5.509 | 0.425 | 12.97x |
| browser head, request line | 5.513 | 0.430 | 12.83x |
| browser head, `User-Agent` | 5.580 | 0.478 | **11.67x** |
| 64k head, request line | 5.531 | 0.426 | 12.97x |
| 64k head, last field | 5.563 | 0.452 | 12.31x |

`k` is the minimum: **11.67x**. Samples separated on every input — the spike's
worst is below the interpreter's best everywhere — so the spike is evidence.
Re-taken on this tree: the constant memo reaches `line_stops()`, which
`read_line` calls, and the interpreter column moved by under 1% for it.

### 9.1 Four variants, and the one that matters most

| variant | k | per request |
| --- | --- | --- |
| whole call graph compiled, literals folded | **11.67x** | 21.65µs → 1.69µs over 4 calls |
| the same, literals rebuilt per evaluation | 11.05x | the same, to 2% |
| **`read_line` alone, callees trampolined** | **1.71x** | 21.86µs → 12.60µs |
| whole call graph, browser-sized head | 11.17x | 76.52µs → 5.96µs over 14 calls |

**The two columns are not computed the same way, and a reader who divides the
third gets a different number from the second.** `k` is the *minimum over the
five inputs* (§9's rule, interpreter-best against spike-worst); `per request` is
a *sum* over the 4 or 14 calls a head costs. So row 1's µs divide to 12.81x
against its printed 11.67x, row 3's to 1.73x against 1.71x, and row 4's to
12.84x against 11.17x. The stated `k` is the conservative one in each case,
which is the right choice — but it is a different statistic from its own row's
µs, and this table did not say so. (Audit note.)

**Audit note on sourcing: only the first variant is in
`benches/w6-spike.json`.** That file carries one `spike` object — five inputs,
`nodes`, `compile_micros` — and its five inputs reproduce §9's table exactly
(11.67x to 13.23x conservative, minimum 11.67x, all `agreed`). The other three
variants here, and §9.2's census, are in no measurement file. ROADMAP.md's "its
numbers survive in `benches/w6-spike.json`" is therefore true of the headline
`k` and false of §9.1 and §9.2, including the **1.71x** that §10.3 calls one of
the three measured reasons to doubt `E = 1.48`. The audit could not re-take
them: `crates/ply-codegen-spike` does not build on the toolchain in this tree
(cranelift 0.134.3 requires rustc 1.94.0; rustc 1.93.1 is installed), which is
also worth knowing before §3.5's "closing W6 deletes it" is acted on.

The second variant exists to answer "is this just constant folding?" — it is
not; rebuilding every `Bytes`, `String` and nullary-constructor literal per
evaluation costs 2%.

**The third is the finding.** Compile `read_line` and leave its two callees in
the machine, reached through the trampoline §3.2 explicitly allows, and 11.67x
collapses to **1.71x** — *below `Criteria::defer_spike`*. Two trampolines per
call is the whole difference. A backend earns its speedup by closing over a call
graph, and the moment it cannot, the win is gone.

### 9.2 What the fragment can actually reach

The census §3 required, so a ceiling measured on one function is readable
against a backend's coverage:

| module | accepted | functions |
| --- | --- | --- |
| `std.http` | 59 | 144 |
| `std.router` | 28 | 93 |
| `std.json` | 54 | 129 |
| **total** | **141** | **366** |

Refused by, in descending frequency: a constructor pattern in a `match` (71), a
field access (50), a lambda (38), a list literal (21), a list pattern in a
`match` (21), `++` (14), unary `-` (5), a call through a local binding or an
expression (3), `perform net.accept` / `perform net.listen` (2). `parse_head`
itself is refused, for a field access.

That list is not a set of gaps to fill. A constructor pattern, a field access
and a lambda are what a route handler and a derived codec are *made of* — a
`derive json` encoder is a record of closures — so the fragment reaches the
innermost scanning loops and stops exactly where the endpoint begins.

## 10. The verdict

`S = 0.353` (0.431 attributed, 0.351–0.354 over its repeats), `k = 11.67`,
`E = 1/((1−S) + S/k) = 1.48`, ceiling `1/(1−S) = 1.55`, `A = 1.15`.

| criterion | bar | measured | |
| --- | --- | --- | --- |
| **C1 — Share** | `S ≥ 0.50` | `S = 0.353` | **fail** |
| **C2 — Ceiling** | `k ≥ 3.0` and `E ≥ 1.50` | `k = 11.67`, `E = 1.48` | **fail** |
| **C3 — Nothing cheaper** | every §4 lever priced, `(E−1) ≥ 2(A−1)` | 1 of 7 priced | **fail** |
| **C4 — Correctness** | agreement and separation | every input agreed; all 5 timed inputs separated | **pass** |

> ## Keep deferring M9.
>
> The first take deferred on C3 alone, with the share and the spike clearing
> their bars comfortably. This one defers on **three** criteria. The share is
> now 35% rather than 67% — not because the interpreter got faster in the ways
> M9 would have made it faster, but because a cheaper lever landed and took
> interpreter time out of the request, which §10.2 predicted as the most likely
> way the number would move. At 35% an *infinitely* fast execution strategy is
> worth 1.55x end to end, and the spike's own 11.67x projects 1.48x — below the
> 1.50x §2.3 fixed as what a permanent second execution path has to buy.

`decide` returns at C3, because §2.5 makes an unpriced lever independently
sufficient and it reads no share; the share and the projection are reported
beside it and are what the reopen sentence is composed from.

### 10.1 Why C3 is not a technicality, with a worked example

§2.5 makes an unpriced alternative independently sufficient, and §4 explains
why: W1 predicted codegen was the second lever and W2 then delivered **4.8x** by
attacking an algorithm instead.

This take is the third instance, and it is no longer an argument — it is a
measurement. Between the two takes, **one of §4's seven levers landed**:
"caching derived work", as `ply-eval`'s constant memo. Priced the way §4
requires, as an end-to-end speedup on the served workload the total came from,
it is **1.15x on `/items` and 1.77x on `/health`** — 677.0µs → 589.4µs and
466.6µs → 263.5µs, the shipped service against the same source with every
nullary definition of its own given a dead parameter, served alternately by the
same binary, with byte-identical responses on every route the twin can answer
asserted before anything was timed. One memoized definition: no new execution
path, no cache key that splits on a flag, no third `--engine` pair to police,
and it is worth more on `/health` than this document's first take projected a
whole code generator would be worth end to end.

It also took M9's own case apart: the share it removed was interpreter share, so
`S` fell from 0.671 to 0.353 and the ceiling with it.

The six still unpriced carry bounds large enough to matter:

- **The frame representation.** 2.73x on the pure request path between two
  interpreters that already exist (§8.2).
- **Allocation.** One `/health` request allocates **1,035 times and 0.124 MB**
  to produce a 107-byte response.
- **Framing.** 97.83µs, 16.5% of a request, and W2's lever applied to W3's layer
  was worth 4.8x on the layer it was applied to.

None of those costs a permanent second execution path, a third `--engine` pair
to police for `E0503`, a cache key that splits on a flag, or a backend that can
silently break replay determinism (§2.2).

### 10.2 What would reopen it — computed, not written

`Decision::reopens_at` is composed from the criteria that are **not** met, and
nothing else:

> **M9 reopens when the interpreter share reaches 50% (it is 35%, a 1.55x
> ceiling), and the projection reaches 1.50x (it is 1.48x), and the 6 unpriced
> levers in ADR 0016 §4 are priced and the best of them measures at or below
> 1.24x end to end.**

Concretely, for a future contributor:

1. **Price the six remaining levers in §4** as measured end-to-end speedups on
   the served workload §1.6 defines. Not microbenchmarks. The one that has been
   priced shows the shape: a source substitution, both variants served
   alternately by the same binary, byte-identical responses asserted first.
2. **The share has to come back up**, which is the hard part and the honest one:
   every cheaper lever that lands lowers it further. M9's case gets weaker each
   time this project does the cheap thing that works.
3. If a lever lands **above 1.24x**, build it and re-take everything. `S`, `E`
   and the bar all move together.

### 10.3 The case against this document's own 1.48x

`E` applies `read_line`'s `k` to the whole 35% interpreter share, which assumes
a backend that is 11.67x on everything the interpreter does. Three measured
things say that assumption is generous, and they are recorded here because the
projection is the number M9's case rests on and a reader deserves its width:

1. **`read_line`'s own end-to-end value is 1.02x.** Directly measured, not
   projected: it is 2.2% of a served request at a 63-byte head and 5.2% at a
   browser-sized one, so compiling it **at the spike's own 11.67x** buys
   **1.021x** and **1.050x** respectively. It is 20.7% of `parse_head`. (Audit
   correction: this sentence said "compiling it *perfectly*", which is a
   different and stronger calculation — at `k = ∞` the same two shares give
   1.022x and 1.055x. The printed values are Amdahl at `k = 11.67`, so the
   method and the number now agree. The correction moves the figures by less
   than a thousandth and does not touch the argument.)
2. **The fragment reaches 141 of 366 functions** (§9.2), and what it refuses is
   what endpoints and codecs are built from.
3. **Coverage is not linear — it is a cliff.** 11.67x becomes **1.71x** the
   moment two callees stay in the interpreter (§9.1).

Against that, the share is a lower bound in one direction — `/items`' own decode
and encode sit inside the `database` rung (§8.4) — and an over-count in another,
which is why the negative residue is charged back rather than ignored. The two
corrections push in opposite directions and neither was measured to a
conclusion, which is why they are stated rather than netted, and why the honest
reading of `E = 1.48` is *an upper bound with three measured reasons to doubt
it* rather than a forecast.

## 11. What §2 did not anticipate, and one fix

**The criteria had no shape for "C1 and C2 pass, C3 fails alone."** §2.5 lists
five deferral causes as though they were alternatives; in the first take four of
them did not fire and the fifth did. The list is still right — every entry is
still independently sufficient — but the *reopen* sentence computed for that case
was wrong: `decide` named the share and the spike as what would reopen M9 when
both already cleared their bars.

`crates/ply-corpus/src/w6.rs` now composes that sentence out of the conditions
that are **not** met and nothing else, and adds C3's own: the levers priced, and
the best of them no better than half M9's projected gain.
`an_unpriced_alternative_defers_whatever_the_share_says` asserts it.

**One obligation is outstanding.** §3.5 and "Not in W6" require that the spike be
deleted when W6 closes. It has not been: `crates/ply-codegen-spike` is still
present, in its own workspace, with no crate in `crates/*` depending on it, so
`rm -r crates/ply-codegen-spike` is the whole deletion. Its measurements survive
in `benches/w6-spike.json`, which is what the decision reads. It is recorded here
rather than done quietly because a document that claimed a deletion that had not
happened would be the exact failure this ADR is built to prevent.

**What §2 got right, and it is worth saying because it cost something.** Putting
the thresholds in `Criteria::default()` and keeping `verdict` out of `Report`
meant that when the numbers arrived in a shape nobody predicted — a very high
`k` and a very high `S` producing a deferral — there was no way to quietly read
the bar off the result. A document that had written "M9 is justified if the
numbers support it" would have advanced M9 on the projection and never mentioned
the trampoline, the census, or the route table rebuilt on every request.

**And what §2 got wrong, which §12 is.** The same argument was made about the
*thresholds* and not about the two things a verdict is actually read from: a
measurement file, which no code can tell is older than the program it describes,
and a list of alternatives, which was also a measurement file.

## 12. What two audits found, and what changed

§8 onward is a **second take**. This section says why, and what moved in the
machinery between the two, because a result that quietly changed would be worse
than one that was simply wrong.

Two audits — one of the published artifacts, one of the decision machinery —
found ten defects. Three were blockers, and they were three faces of one thing:
**the numbers had stopped describing the tree they shipped in, and nothing in the
repository could have noticed.**

### 12.1 The tree moved under the file

`crates/ply-eval/src/memo.rs` — "the constant memo", CONTRACTS.md, and the
`RUNTIME_VERSION` bump to `0.11.2` — landed after the first take. A nullary pure
definition is now evaluated at most once per `Machine` or `Interp`, which is
exactly §4's **"caching derived work"** lever: not priced, *built*.
`examples/desk.ply`'s `health()` takes no parameters and performs nothing, and
`route_of` reaches `table()`, so `/health` — the route five of the nine rungs are
taken on — stopped doing most of what the ladder said it did.

The proof that this is a tree difference and not a rig difference needs no clock:
the first take published **9,343 allocations and 1.03 MB** for one `/health`
request, and this tree makes **1,035 and 0.124 MB**. That is 9.0x, counted by a
global allocator, on any machine.

Three consequences, all of them in this document's own claims:

- The ladder is re-taken end to end, on this tree, by the method §1 pins. §8
  onward is that.
- **W6's "Not in W6 — optimizing anything" was broken**, by a change that landed
  in the same tree and is documented in CONTRACTS.md. The honest record is that
  one of the seven levers was built rather than priced, and this take prices what
  it was worth: §8.1 and the `caching derived work` row of §10.
- Two guards now stand where nothing did.
  `crates/ply-corpus/tests/w6_report_integrity.rs` re-takes the cheap half of the
  ladder in release and compares it against the shipped file; the fix for a
  failure is to re-take, and the message says so with the command.
  `crates/ply-corpus/tests/w6_report_allocations.rs` does the same in
  allocations, which do not move with a machine and so cannot be argued away as
  load.

**And the command now writes the file.** `ply-corpus w6-ladder` used to emit a
differently shaped document with `alternatives: []`, and the shipped
`benches/w6-ladder.json` — with its seven hand-written levers, seven limits and
`machine: null` — was assembled around it by hand. Both staleness guards told a
contributor to "re-take the ladder", and following that instruction would have
**deleted the evidence C3 is decided against**. The command now emits the whole
report: the rungs, the engines, the offerings, the limits, the §4 roster with
whatever this run priced of it, and the `not_measured` list. It runs `ply` for
the served rows and a new `w6-alloc` binary for the allocation count, because a
counting `#[global_allocator]` is a whole-binary decision and `ply-corpus` is
where the clocks are.
`the_shipped_ladder_is_what_the_command_writes` holds it there: the shipped file
must be byte-identical to `Report`'s own serialization, so a field typed in by
hand fails a test rather than surviving until the next contributor re-takes the
ladder and deletes it.

### 12.2 C3 was decided against a field of the file it was judging

§2.6 claims "there is no path from a measurement to the bar it is about to
clear". There was one, and it was the criterion the verdict turned on.

`decide` implemented C3's first clause — "*every* alternative in §4 is priced" —
as "no entry in the `alternatives` array I was handed has `priced: false`". That
array is a field of the same file the ladder comes from; an **empty** one
satisfied the clause vacuously; and `Report::audit` said nothing about it. Two
lines of `python3` over `benches/w6-ladder.json` turned *keep deferring M9* into
*advance M9*, cleanly, with no audit finding. The same hole ran through the
values: `priced: true, end_to_end: 1.0` on all seven advanced M9 and audited
clean.

Two changes, both of which move the check out of reach of the run being judged:

- **§4's roster is in code.** `w6::LEVERS` holds the seven levers, `w6::c3_gaps`
  answers which of them a report does not price, and `decide` and
  `Report::audit` both read that. A file that says nothing about a lever prices
  nothing, which is what deleting the field now means.
- **A price needs evidence.** `Alternative::evidence` says what the ratio is
  between, in a sentence a reader can check; a lever claimed as priced without
  one is treated as unpriced. `priced: true` is a boolean somebody can type, and
  ADR 0016 §4 wants a *measured end-to-end speedup* — including `1.00x`, which is
  a result, and which is why the fix is a citation rather than a floor on the
  ratio.

`an_absent_alternatives_list_cannot_advance_m9` and
`a_measurement_file_cannot_price_a_lever_by_asserting_it` are the two tests, and
they are the audit's own cases with their assertions turned around.

### 12.3 Four places the tables claimed more than the measurement had

**A negative residue was printed and ignored.** §1.4 says the residue is
credited to nobody, which makes the share a lower bound — and that reasoning
holds only when the residue is *positive*. A negative one means the layers sum to
more than the request they were read against, which can only be the in-process
arena over-counting against the served denominator, and leaving it uncredited
leaves the numerator inflated in the one direction M9's case rests on. The first
take decided at **−6.0%** and called its share "a lower bound twice over" two
sentences after saying why it was not. `Ladder::conservative_share` now charges a
negative residue back to the interpreter and `decide` reads *that*; a positive
residue is still credited to nobody, so the lower-bound reading survives exactly
where it was true.

**One number per rung, and the share straddling its own bar.** The first take
reported `tls` at **8.18µs** and `tracing` at **1.30µs** — two decimals on
differences of about 1% between two served rows, each of which had been selected
independently from its own concurrency sweep. Re-runs of the same harness
produced `tls` of −0.53µs, +6.90µs and +4.29µs. Worse, three clean re-runs put
the interpreter share at **0.541, 0.504 and 0.452** — C1's bar is 0.50, so the
same harness passed it twice and failed it once, and nothing in `Ladder`,
`decide` or `audit` carried a repeat count or a spread. Now: every rung carries
the worst of its repeats as well as the best, `Rung::layer_low_micros` and
`layer_high_micros` bound the layer, a rung whose band spans zero is an audit
finding rather than two printed decimals, the share is printed as a band, and a
share whose band falls on both sides of a bar is `Undecided` — because that
ladder answers whichever run was taken. The two sides of every served rung are
also taken at the **same** concurrency now, so a layer is one flag moved rather
than one flag moved and two rows selected.

**The floor did different work from the total.** "A Ply request costs 41.6x the
same syscalls with no interpreter under them" divided `/items` over postgres over
TLS by a floor replaying `/health`'s 107-byte response over plaintext. The
database rung alone was 30% of the numerator and none of the denominator. There
are now two floors — one per response the service answers — the `/items` total is
read against the `/items` floor, every offering row is read against the floor for
its own route, and `w6::Denominators` spells out what each side did, in the file,
with `Report::audit` reporting a report that leaves it blank. The like-for-like
number — `/health` over plaintext against a floor replaying the same bytes — is
in §8.5 as its own row.

**The accept loop the run used was the one §1.6 excluded, and nothing said so.**
§1.6 pins the task-per-connection loop for every served row and says the
sequential loop is "reported once, separately, and labelled"; `--concurrent`
defaulted to false, so every served row, the total and the first five offering
rows were sequential, and §8.4 did not list it among the departures it disclosed.

The obvious remedy was to default to the pinned loop, and taking that
measurement is what found the reason the run had been right for without saying
it: `task.spawn` opens a production region for the life of the server and
`Machine::constant` refuses the constant memo inside any open region, so a
spawning service memoizes nothing — while the in-process rungs, which are one
connection at a time, memoize. A ladder read off the pinned loop would divide a
memo-inert denominator into a memo-active numerator. So the fix is disclosure
rather than a different default: the flag is now
`--accept sequential|task-per-conn`, **both** loops are swept on every run, the
ladder is read off the sequential one, and the other becomes labelled offering
rows — which is what §1.6 asks of whichever loop the ladder is not read off. The
choice is §8.4's third bullet, what it costs is §8.5's third row and CONTRACTS'
own note on `Machine::constant`, and the first take had none of the three.

`w6_run::best` also had no tie-break on a throughput curve this document itself
measures as flat, so it selected c=4, c=32 and c=1 across three runs of one
harness, once landing on a row whose p99 was 132.6ms; within 5% of the best it
now takes the **lowest** concurrency, which is the one whose latency is a service
rather than a queue.

### 12.4 What did not change

**The criteria.** `Criteria::default()` holds the same eight numbers it held
before either audit — `0.50`, `0.35`, `3.0`, `2.0`, `1.50`, `5.0`, `2.0`, `0.05`
— and no threshold moved in either direction. The bar was not moved; the numbers
under it were re-taken, and where the machinery changed it changed to make a
measurement harder to overstate rather than easier to clear.

**The verdict.** Keep deferring M9. `decide` still returns at C3 — now with one
of the seven levers priced instead of none, because it landed — and what changed
is that C1 and C2 no longer clear their bars either, so the deferral rests on
three criteria where the first take's rested on one.
