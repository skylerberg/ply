# ADR 0016 — W6: where the time goes, and whether M9 comes forward

Status: accepted — **decided: keep deferring M9** (§10), on three of the four
criteria.

**The evidence is one milestone stale and the verdict is not.** ADR 0017 landed
after this document and states in its own Consequences that codegen's ceiling
should be re-measured, because this ADR's projection was a verdict on the old
representation and ADR 0017 changed exactly what made that ceiling low. That
re-measurement has not happened, so everything in §8 onward is a reading taken
against a forkable world and a `Machine` that no longer exist. **None of the four
criteria moved in M9's favour, so the verdict is unchanged; §10's table is not
current.**

**§10.2's reopen sentence no longer decides M9, and ADR 0026 §4.2 says why.**
This instrument answers a question about a served HTTP request and cannot be
pointed at anything else.

W6 answers one question — *where does a request's time go now, and is native
codegen the right next lever* — and it closes the web track. ADRs 0011 through
0015 each settled what a milestone builds. This one settles what a milestone
**decides**, which is a different kind of document: its deliverable is a number
and a verdict, and the only way a verdict of that shape is worth anything is if
the criteria are fixed before the numbers exist.

Sections 0 through 7 were written before any W6 measurement was taken. **The
result is §8 onward**, at the end, in that order deliberately: a reader who wants
to check that the bar was not moved reads the bar first.

**§8 onward is a second take.** The first was published against a tree that no
longer existed: the constant memo landed in `ply-eval` afterwards — one of §4's
own cheaper levers *built* rather than priced — and it removed most of two rungs.
§12 is what two audits found and what changed. The thresholds in §2 are not among
the things that changed.

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
   run.** Not a timer inside the machine, not a profiler's attribution, and not a
   number quoted from a previous milestone. A rung that cannot be expressed as
   one substitution is not a rung (§1.1).
3. **What the ladder did not separate is printed as the residue and credited to
   nobody.** In particular it is *not* credited to the interpreter, which makes
   the share M9's case rests on a lower bound (§1.4).

## 0. What is already known, and what it does not say

This table is assembled from five milestones' machines, which is why W6 re-takes
all of it on one machine in one run.

| taken by | what it says |
| --- | --- |
| M9's deferral | in a *test* run the front end and the cache are still the larger cost, but a faster evaluator moves more than nothing |
| W1 | serving inverts M9's argument: there is no front end on the request path, so the interpreter *is* the request path |
| W1 | the host boundary ADR 0008 was most worried about is not a cost; it is a correctness surface |
| W2 | the per-byte cost was five O(n) folds boxing an `Int` per byte with no early exit — an algorithm, not a constant factor |
| ADR 0005 | the control-stack machine's heap allocations per frame push are a lever that is neither codegen nor an algorithm |

**Do not argue the decision from this table.** §0's header used to claim every
number in it was taken by the milestone named beside it, and that provenance
claim does not hold. An audit found the M9-deferral row's share wrong by roughly
a factor of seven against two independent re-takes; several of the W1 and W2
figures occur exactly once in this repository — here — so the milestone that
supposedly took them recorded nothing; and where the same measurement *is*
recorded elsewhere, `CONTRACTS.md`'s W2 re-measurement, it disagrees with this
row and the two are not reconcilable by calling one of them newer.

**The re-taken share does not overturn the deferral** — the front end plus hash
is still most of a warm run — but it means **the decision should be argued from
the served profile in §8.1**, measured by this milestone on this machine, rather
than from any row here.

What none of it says: **what the whole W5 stack costs.** W3 added framing,
routing, keep-alive and TLS; W4 added a database; W5 added a sink, a
configuration and a drain. Each was priced against the thing it replaced and none
against the total, so the accumulated bill has never appeared in one place. §1 is
how it appears, and §5 is what must be said about it.

**And the reason the W2 result is the governing precedent rather than a
footnote.** W1 predicted codegen would be the second lever and was right for the
wrong reason: what fixed the request was not a faster interpreter but *fewer
passes over the buffer*. **A milestone that reaches for a code generator before
it has priced the algorithmic levers is repeating the mistake W2 corrected.** §4
is that list, and an unpriced entry on it is on its own sufficient to keep
deferring (§2.5).

---

## 1. The workload ladder

### 1.1 Measurement is by substitution, not by instrumentation

> Every rung runs **the same program**. Two measurements are taken that differ in
> exactly **one** thing underneath it, in the **same arena**, in the **same
> run**. The layer is their difference. Nothing is timed from inside the machine,
> because a timer inside the machine is a claim about where a boundary is, and
> the boundary is what is being measured.

Three consequences, and all three are load-bearing:

- **A rung needs two numbers, not one.** `w6::Point` carries `with_micros` and
  `without_micros` and refuses to be built from one of them. A ladder of
  cumulative totals silently assumes each rung's baseline is the rung below,
  which is true in one arena and false across the seam (§1.3).
- **A negative layer is a result.** It means the substitution did not isolate
  what it claimed to. `w6::Ladder` reports it, `Report::audit` names it, and
  `decide` returns `Undecided` rather than reading a share off a ladder carrying
  a large one. **A clamp to zero would turn a broken measurement into a plausible
  one.**
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
| 4 | `routing` | `table()` and `route_of()` above the framed call | rung 3's `with` | building the route table from its pattern strings, and one match |
| 5 | `machine` | the whole `serve_one` over `SimNet` | rung 4's `with` | the loop around the pure pieces: `recv`, the handler-stack walk, the perform, the body, `send`, keep-alive, teardown |
| 6 | `socket` | the same loop over the real `ply_host` TCP handler, in process | rung 5's `with` | the socket, the reactor, the blocking pool, the `Pending` token |
| 7 | `tls` | the served binary with `--tls`, steady state | the same, plaintext | the TLS record layer, handshake excluded |
| 8 | `database` | `run` — postgres behind the `db` atoms | `run_memory` — `std.db`'s twin | the postgres boundary, the wire, and the server |
| 9 | `tracing` | `--trace json` to `/dev/null` | `--trace off` | the sink: encoding a record and writing it |

And the denominator every one of them is read against: `rust-floor`, the same
accept/recv/send/close in Rust answering the same bytes — what the syscalls cost
with no interpreter under them.

Four things about this table are decisions rather than descriptions.

**`call` exists as its own rung** because every rung above it pays it, and
because it is what the spike's comparison pays on both sides (§3.3). Without it,
`endpoint` would be reported as the cost of a route body when part of it is the
cost of having an interpreter at all.

**`tracing` is the sink and not the perform.** ADR 0015 §1.4 is explicit that
there is no configuration in which a trace operation is not performed, so
`--trace off` still builds the `Fields` map, still performs, and still walks to
`ply_host::trace::discard`. That cost is inside rung 5; rung 9 is only what the
JSON encoder and the write add.

**`tls` is read off the steady state and not the handshake.** W3's README states
the trap: `desk.ply` serves one connection at a time and the handshake completes
on the server's first `recv`, **so a client that connected while the server was
busy times the queue.** The rung is taken with keep-alive on, over enough
requests per connection that the handshake is a rounding error.

**`database` is taken on a route that has one.** Its `without` is the same route
against `std.db`'s in-memory twin — the substitution W4 built the twin for — so
the difference is postgres and not a different endpoint.

### 1.3 The two arenas, and the seam between them

Rungs 1–6 are **in process**: one thread, the machine engine, no CLI, no child.
Rungs 7–9 are **served**: the real `ply` binary over loopback, because `--tls`,
`--db` and `--trace` are flags and a flag substitution needs a process to pass it
to.

That seam is real and the ladder does not pretend otherwise. `w6::Arena` is a
printed column. The in-process rungs are not "the first six-ninths of the served
total" — they are what those layers cost with a syscall's variance kept out of
the middle of them, which is the only way the parse, the route and the encode can
be measured at all.

What ties the two together is that **rung 6 and the served baseline measure the
same configuration**: same route, same store, plaintext, `--trace off`. Their
difference is the process boundary, the CLI's reactor placement and the client's
own cost, and that difference is part of the residue.

### 1.4 The residue is printed

`total_micros` is **measured**, not summed: it is what the fully-configured
served stack delivered per request, end to end, at the concurrency that maximizes
throughput. The rungs are then checked against it, `residue = total − Σ layers`,
and it is printed on its own line. **Every profile-shaped table in the industry
either omits this or folds it into the nearest plausible layer; both are ways of
claiming an attribution the measurement did not earn.**

**The residue is not credited to the interpreter.** `interpreter_micros` is the
sum of rungs 1–5 and nothing else, so `interpreter_share` is a **lower bound** —
deliberately conservative in the direction M9's case rests on.

**That was written before the numbers and it holds only for a *positive*
residue.** A negative one is the layers summing to more than the request they
were read against, and leaving it uncredited inflates the numerator instead — so
it is charged back, and `decide` reads `Ladder::conservative_share`. §12.3.

### 1.5 The engine substitution

**The same request under the tree-walking evaluator and under the control-stack
machine.** The cheapest empirical handle on how much of a request is *dispatch*
as opposed to native work sitting under it. `bytes_scan_until` is `memchr`;
`Value::Bytes` is an `Arc<[u8]>` allocation; a derived JSON encode is largely
`String` building. **None of those get faster because the code around them was
compiled. If swapping one entire interpreter for another entire interpreter
barely moves a request, a third one will not either.**

It also prices ADR 0005's allocations-per-frame-push as a **lever** rather than
as a fact, which is why it is in §4 as well as here. `--engine both` already
exists to police the two for agreement, so this is the one table W6 takes that is
free of new machinery.

### 1.6 What the ladder is measured on

- `examples/desk.ply`, read from the repository, rewritten only in the ways
  `w3::Service` and `w5::project` already rewrite it. **The service under
  measurement is the one W5 shipped, not a copy that can drift.**
- The task-per-connection accept loop for every served row. A sequential server
  answers one connection at a time, so a tail latency at concurrency 8 is a queue
  and not a service. The sequential loop is reported once, separately, and
  labelled. (§8.4 records that the run departed from this, and §12.3 why.)
- Release profile. A debug measurement of an interpreter is a measurement of
  `debug_assertions`.
- Best of N repeats, N reported. Best-of rather than mean because the quantity of
  interest is the cost of the work, and everything a run adds to it is additive
  noise.
- **The request head length is printed on every table.** W1's endpoint's cost was
  a function of head length and W2's very nearly is not; a load number quoted
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

**A dependency.** `cranelift-jit` and `cranelift-codegen` are large. `ply-eval`'s
existing dependencies are crates whose whole content is something it would be
foolish to write here. Cranelift is a different order of thing.

**A second execution path, permanently.** `--engine both` exists precisely to
police one pair of engines for divergence, and `E0503` is what it raises. A third
engine makes that three pairs. `crates/ply-eval/src/differential.rs` and every
corpus it runs would have to cover the new one, **or the guarantee weakens
silently — which is the failure mode every audited milestone has produced and
none has produced as a crash.**

**A cache key.** `RUNTIME_VERSION` keys `(runtime_version, test_hash) → Pass`. A
cached pass is a claim about what the evaluator did, so an engine that evaluates
differently must move it — and if the engine is a *flag*, the cache splits on a
flag. ADR 0011 §4 refused exactly that shape for `--host` and gave the reason:
`ply check` must not disagree with itself. **A JIT that is opt-in is a cache that
is opt-in.**

**Determinism.** A seeded simulation replays exactly because evaluation order is
fixed. `proved` is a truth claim discharged over a fixed evaluation. A backend
that reassociates arithmetic, reorders argument evaluation, or evaluates a
short-circuit differently breaks replay and tier honesty **silently**, and a
silent break in this system is worse than a loud one by the whole margin the
project is built on.

**Maintenance, forever.** Every new builtin, every `Value` variant, every
handler-stack change lands twice. W2 added twenty-odd builtins and two `Value`
variants in one milestone; that is the rate.

Against all of that, a marginal speedup is not a reason. That is why C2 has a
floor and why C3 asks for double the best alternative's *gain*.

### 2.3 The thresholds, and why each is where it is

**`S ≥ 0.50`, and defer below `S < 0.35`.** Amdahl's ceiling is `1/(1−S)`
whatever the backend does. Fifty percent is the point at which an *infinitely*
fast execution strategy is worth 2× end to end, and 2× is the least that buys a
permanent second execution path (§2.2). Below 35% no spike result can change the
ceiling, so **the correct response is to report the ceiling and stop — not to
measure harder.**

**`k ≥ 3.0`, and defer below `k < 2.0`.** A first Cranelift backend for this
language will not deliver an order of magnitude, and it is important to say why
rather than to hope: values stay boxed, the scan primitives are already `memchr`,
effects still walk the handler stack, and a `perform` still has to reach a host
binding. What codegen removes is node dispatch, the frame push and the `Env`
walk — real, but a constant factor on part of the work. **Under 2× it is inside
the range a single algorithmic change has already delivered once.**

**`E ≥ 1.50`.** The projection is what actually arrives. Everything in §2.2 is
paid whatever the win is.

**`(E − 1) ≥ 2 × (A − 1)`.** On the **gains**, not the ratios. Two times, because
M9 is a permanent surface and an alternative is one change; a near tie goes to
the alternative, always.

**A ceiling on the negative layer.** Above it the ladder did not separate and no
share read off it is trustworthy, so nothing is decided.

### 2.4 The grey band

`0.35 ≤ S < 0.50` is the honest middle, and pretending otherwise would be the
place this document cheats. In that band the share alone cannot carry M9, but a
very good backend might.

> In the grey band, M9 is `Conditional` if `k` clears the higher bar **and** C2's
> `E` still holds **and** C3 and C4 hold. Otherwise defer.

`Conditional` is a distinct verdict and it means something specific: **M9's scope
is the fragment the spike proved, not a whole backend.** A conditional advance
that quietly became "write a Cranelift backend for Ply" would be this document
being used as cover.

### 2.5 What would make the honest answer "keep deferring"

Any one of these, and each is reported with the number that produced it:

- The share is under its defer threshold. Report the ceiling, name the layers
  that own the request instead, and stop.
- The spike found no headroom on a function chosen to have the most.
- The projection does not clear what §2.2 costs.
- A cheaper lever is close enough that it wins on cost.
- **Any alternative in §4 is unpriced.** Independent of every number above, and
  it is the W2 precedent stated as a rule: a cheaper lever that has not been
  priced is on its own a reason to keep deferring, because W1 predicted a codegen
  win and W2 beat the prediction by attacking the algorithm instead.

And two outcomes that are `Undecided` rather than `Defer`, because they call for
the opposite response — take the measurement, rather than accept the answer: a
missing rung, so the total is not the stack's; and a spike that is not evidence
(§3.4), so `k` does not exist.

### 2.6 The criteria are in code

`ply_corpus::w6::Criteria` holds the eight numbers above, and `w6::decide`
applies them in the order §2.5 lists. `w6::Report` — the JSON the measuring runs
produce — has **no** criteria field and **no** verdict field, and a test asserts
that a serialized report contains neither. The verdict is recomputed from
`Criteria::default()` every time the report is rendered.

**This protected the *thresholds* and it did. It did not protect C3**, which was
checked against a list the measurement file supplied, so an empty list cleared it
vacuously. §4's roster is now in `w6::LEVERS`, beside the criteria. §12.2.

That is not ceremony. It is the same structural argument ADR 0007 makes about
tiers: there is no `tier` field anywhere, because **a label that can be asserted
will eventually be asserted wrongly. A verdict that can be written into a file
will eventually be written into a file.**

---

## 3. The spike

Small enough to throw away, large enough to price the ceiling. It exists to
produce `k` and nothing else.

### 3.1 Which function

**The selection rule, applied to the W6 stage table rather than chosen in
advance:** among the pure Ply functions on the request path, take the one with
the highest per-request cost whose **entire body** is inside the fragment §3.2
compiles. The whole body, because **a spike that compiles half a function and
calls back into the interpreter for the rest is measuring a trampoline.**

The report names the function, its node count, and what share of the `framing` +
`endpoint` rungs it accounts for, so the choice is reviewable and so a reader can
see whether the ceiling was priced on something that matters.

`std.http::read_line` is the expected answer and is stated here as a prediction
rather than a decision. If the stage table names something else, the rule wins.

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
- Calls to builtins through **one** runtime trampoline.
- Calls to other Ply functions through a trampoline back into the machine.
- **Nothing else.** No `perform`, no `handle`, no closures, no `with_cell`, no
  `simulate`. A function containing any of them is refused, loudly, and is not
  the spike's function.

Values stay boxed and effects stay in the interpreter **on purpose**. A spike
that unboxed everything and inlined the builtins would price a language Ply is
not, and would produce a `k` that no real backend could deliver. **The point of a
ceiling is that it is reachable.**

### 3.3 How it is measured

- One process. The same `Machine`, the same `Value` inputs, the same function.
- At least three inputs (`w6::SPIKE_MIN_INPUTS`), spanning the range the real
  request path produces.
- Four times per input: the interpreter's best and worst, and the spike's best
  and worst, over the same repeat count.
- **Compile time is reported separately and never amortized into the
  comparison.** A service compiles at first call, so `compile_micros` is a
  column. It is also the one place Ply has a genuine structural advantage worth
  noting: a definition is content-addressed, so a compiled body is cacheable on
  its hash, permanently, across processes. **That is an argument for M9's
  *design* and it is not evidence for M9's *value*; it does not enter the
  criteria.**

### 3.4 What is evidence, and what is noise

`w6::Spike::judge` implements this and returns `evidence: false` with every
failed rule named:

1. **At least three inputs.** A ratio over fewer is one input's constant.
2. **Agreement on every input**, by `Value`'s own ordering. A faster wrong answer
   is an `E0503`, not a speedup — this is C4.
3. **Separated samples on every input**: the spike's *worst* is below the
   interpreter's *best*. Overlapping bands mean the measurement found no
   difference, whatever the midpoints say.

And the reported `k` is the **minimum** conservative ratio — interpreter best over
spike worst — across every input. **A speedup that holds on one input and not
another is that input's, so the weakest one is the claim.**

### 3.5 What the spike may not do

- It may **not** be a synthetic benchmark. No `fib`, no `nbody`, no loop written
  for it. The function comes from the stage table or the spike has not been run.
- It may **not** be kept because it works. It is thrown away whatever the
  verdict; an `Advance` schedules M9, and M9 is a milestone with an ADR, not a
  promotion of a spike.
- It may **not** report a ratio whose two sides did different work — for instance
  an interpreter column that includes `Machine::call` entry where the spike
  column does not. Rung 1 is measured precisely so that cost is known and present
  on both sides.
- **`crates/ply-codegen-spike` may not be wired into any command, and it is
  not.** It is not a workspace member, carries its own `[workspace]`, is depended
  on by nothing, and `cargo check --workspace` does not build it. The deletion
  was verified rather than argued: a reviewer copied the tree, ran
  `rm -r crates/ply-codegen-spike`, and got the whole workspace green.

**The prohibition on a compiled backend *in general* is withdrawn, and cheap
deletion is over.** This bullet used to add that the spike lived behind a cargo
feature with optional cranelift dependencies, "so that deferring M9 deletes one
feature block and one dependency line, and nothing else in the workspace knows it
existed." Three things ended that, in order:

1. **What shipped was never the arrangement described.** The spike is a separate
   crate with its own workspace and non-optional cranelift dependencies, not a
   feature of `ply-corpus`. §6 carries the same correction.
2. **R5 left something behind that survives the `rm -r`.** `crates/ply-eval`
   carries `compiled.rs` — a public `Compiled` trait, `Machine::set_compiled`,
   counters, and a branch in `Machine::enter_code` taken on every interpreted
   call — none of it with a shipping implementor. That was a deliberate change
   under ADR 0018 §0, and no ADR recorded the amendment at the time.
3. **A cranelift code generator is now wired into `ply test`**, as
   `--backend cranelift`, in a **new** crate `crates/ply-codegen` that
   `crates/ply-cli` depends on unconditionally. It ports the spike's lowering and
   runtime as *source* — it does not depend on the spike — and implements
   `ply_eval::Compiled` and `ply_eval::Policed`. **ADR 0026 §4.7 is where the
   ADR-level authorisation lives.** The toolchain premise behind "optional" is
   also gone: the cranelift the spike now pins builds on the workspace toolchain,
   so CI no longer installs a second one.

**Deferring M9 is now a real revert rather than a line, and that cost is accepted
rather than argued away.** What replaces cheap deletion as the protection is the
thing this section could not have asked for in W6, because nothing implemented
`Compiled` then: **ADR 0026 §4.5's condition that a backend be *policeable*
before it is fast**, discharged in `crates/ply-cli/tests/suite/backend.rs`.

---

## 4. The alternatives to price alongside it

W2's byte builtins beat W1's predicted codegen win by attacking the algorithm
instead of the constant. This is the list of everything in that class, and §2.5
makes an **unpriced** entry sufficient on its own to defer.

Each is priced the same way: as a **measured end-to-end speedup on the same
served workload the ladder's total came from**. Not a microbenchmark — **a
microbenchmark is how a lever that moves nothing gets adopted.**

| lever | what it is, concretely | why it might be big |
| --- | --- | --- |
| **more native builtins** | fold `read_line`, `is_token`, `trim_ows` and `trim_end` into one native head scan; `string_lower`; `add_field` | this is W2's lever reapplied to the layer W3 added |
| **the frame push** | ADR 0005's heap allocations per frame push; measured by the §1.5 engine substitution and by `crates/ply-corpus/tests/allocation/frame_cost.rs` | it is paid per node the machine suspends inside, which is most of them |
| **`Env::lookup`** | a linear walk down an `Rc` chain, so a variable reference costs O(scope depth); priced by a depth sweep and by an indexed alternative | this is an *algorithm* on the hottest path in the interpreter, which is exactly W2's class of finding |
| **boxing on hot paths** | where a `Value::Int` per element survives; counted per request rather than guessed at | `Value::Int` per byte was W1's whole disease |
| **caching derived work** | `table()` rebuilds the route table from its pattern strings on **every request**; a derived codec dictionary is a record built per call | a per-request rebuild of a constant is free to remove |
| **connection and statement reuse** | W4's pool and prepared-statement cache: hit rate, and what a miss costs | a miss is a round trip, which is worth more than any interpreter change |
| **response buffering** | writes per response, and the copies `bytes_concat` and `bytes_slice` make (`Value::Bytes` is `Arc<[u8]>` with no slicing — ADR 0011 §8 deferred it to W3 and W3 did not reopen it) | a syscall per fragment is a syscall per fragment |

**A priced lever that bought nothing is a result**, recorded as `end_to_end:
1.00`, not omitted. Half the value of this list is knowing which of these
plausible things does not matter.

**The cost column matters as much as the ratio.** A native builtin grows the
trusted computing base `ply hosts` invites a reader to check; caching `table()`
changes when a route table can be edited; an indexed `Env` touches capture,
splice and every frame kind. `w6::Alternative` carries `cost` for that reason.
None of them is remotely M9's cost, which is the point.

**`boxing on hot paths` is still `priced: false`, correctly, and a contributor
should know why.** ADR 0017 attacked it and reported the result — but it measured
*allocations per request*, and they went **up**. C3 requires an end-to-end
speedup on §1.6's served workload, which is a different measurement. The
allocation reading already exists and points the wrong way.

---

## 5. The honest account

W6 closes the web track, so the report is the last word on what this is. It owes
six things, and `Report::audit` names any that are missing **above the tables**
rather than leaving a shorter document:

1. **The accumulated stack in one table.** §1's nine rungs, the floor, the
   residue, the measured total, and `over_floor`.
2. **What a reader gets today.** Throughput and p50/p95/p99 for a no-database
   route and a one-select route, on plaintext and on TLS, at the concurrency that
   maximizes throughput, with the head length printed — and the Rust floor on the
   same machine beside it, so the multiple is visible rather than inferable.
3. **Where this is genuinely not competitive**, named and, where a number exists,
   with the number. The candidates are known before the run and **none of them is
   allowed to go unmentioned because it is unflattering**: one machine is one
   core, because a `Value` holds `Rc` and a task cannot move between OS threads
   (ADR 0011 §9); no cancellation, unresolved through W5, so a request live at
   the drain deadline loses its connection and the run exits `3`; no backpressure
   and no load shedding, which W4 promised and W5's "not in" list breaks
   explicitly; `bytes_slice` and `bytes_split` copy; `--engine both` costs two
   runs; `std.trace`'s `Sink` is O(n²) in the records it holds; and whatever the
   ladder itself says.

   **The `Sink` row is worth its own paragraph, because the cost is right and the
   cause is not what it was written as.** It was blamed on `push`, and the only
   remedy that implies is *avoid `push`* — **which no one can act on**, since
   `push` is the language's sole list primitive. `push` grows a `List` **in
   place** when the caller is its last owner and copies only when something else
   can still see it, and what decides that is **position**: `ply_eval::rc::carry`
   hands a pending frame a live clone of the scope whenever any sub-expression of
   the enclosing node remains, and never asks what those sub-expressions read.
   `append` writes the growing field first of three, so the list is at two owners
   and is copied once per record. **The real fix is one line of field order** —
   zero copies at every size measured, **on the machine engine only.** The
   tree-walker runs no reference counting at all, so under `--engine treewalk`
   the sink is quadratic whatever order is written, **which makes this a limit of
   one engine rather than of the library.** `spikes/ply-lexer/GAPS.md` §1 states
   the rule and ADR 0020 §5.2 measures that it composes across call boundaries.
4. **The M9 verdict**, with the §2 criteria restated, the measured numbers
   plugged in, and — if deferred — the number that would reopen it.
   `Decision::reopens_at` is that sentence, computed rather than written.
5. **Provenance.** Machine, profile, date, repeats, request head length, postgres
   version. **A table without it is a rumour.**
6. **What was not measured, and why.** An empty `not_measured` is itself an audit
   finding, because every run leaves something out and which things is the
   reader's to judge.

**And the tone, since it is a decision rather than a description.** The right
sentence to be able to write is of the form *"a Ply service serves N requests per
second on one core, which is M times what the same thing costs in Rust, and here
is where the M goes"*. **The wrong one is any sentence whose numerator was chosen
after the numbers arrived.**

---

## 6. Workspace

`crates/ply-corpus/src/w6.rs` — the ladder types, the criteria, the decision, the
audit and the renderer. It carries no measurement of its own: the ladder is
assembled from `Point`s, and `w6::decide` is a pure function of a `Ladder`, a
`Spike` and the alternatives.

`ply-corpus w6 <report.json>...` merges the measurement files field by field — so
the ladder run and the spike run produce their halves independently — applies
`Criteria::default()`, and prints the tables, the verdict and the audit.
`--strict` exits non-zero on an incomplete report; `--json` emits the report, the
assembled ladder, the spike's verdict, the decision and the audit together.

**This section planned the spike as a cargo feature of `ply-corpus` and that is
not what shipped.** `ply-corpus` has no such feature and no cranelift dependency.
What shipped is `crates/ply-codegen-spike`, a separate crate with its own
`[workspace]` table and non-optional cranelift dependencies. The "no other crate
learns the spike exists" half survives and is stronger than written: the shipping
workspace's `members` list does not name it at all, which is what §3.5's
`rm -r` deletion depends on. **The cost of that isolation is §7.**

`benches/README.md` carries the `w6` section describing the ladder, the criteria
and what the report owes.

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
8. A share and spike at their bars with a weak alternative advances; a low share
   with a huge `k` defers and names the ceiling; an alternative within half the
   projected gain defers and says so.
9. The grey band advances conditionally only above the higher spike bar.
10. A serialized `Report` contains no `verdict` and no criteria, and a
    round-tripped report decides identically.

**The spike**

11. Fewer than three inputs, a disagreement on any input, and overlapping bands
    each make the spike not-evidence, and the failure names which.
12. The reported speedup is the minimum conservative ratio over the inputs.
13. The spike refuses a function outside the §3.2 fragment, loudly, naming the
    construct — **a spike that silently fell back to the interpreter for part of
    a body would report a ratio for a program it did not compile.**
14. On its chosen function, over its whole input set, the spike's output equals
    the machine's — and equals the tree-walker's, so the comparison is against an
    evaluator that is already policed for divergence.

**The report**

15. `audit` names a missing engine substitution, a missing spike, an unpriced
    alternative, a missing offering, a missing limits section and an empty
    `not_measured`; a complete report audits clean.
16. `render` prints every section and recomputes the verdict from
    `Criteria::default()`.

**Tests 12, 13 and 14 are written and `cargo test --workspace` does not run any
of them.** They were listed as one block on §6's assumption that the spike is a
feature of `ply-corpus`. It is not, and the crate it actually lives in declares
its own `[workspace]`, so the shipping test run walks straight past it. 11, 15
and 16 are pure functions of a `Ladder`/`Spike`/`Report`, live in `w6.rs`'s
`tests` module, and do run; 12, 13 and 14 need `cargo test` inside the spike
crate.

**What that means for the verdict.** 12–14 are the ones that make the spike's
ratio a number about a program that was actually compiled rather than partly
interpreted, so the spike half of the evidence in §8–§11 is unverified by any
suite a reader is likely to run. The ladder half is unaffected —
`benches/w6-ladder.json`, every share, the Amdahl projection and `w6::decide` are
exercised inside the workspace by `w6.rs` and by
`crates/ply-corpus/tests/suite/w6_report_integrity.rs`. **And the verdict does not turn
on the spike's magnitude:** C2 fails on the projection, and the unpriced levers
defer it independently. **This gap weakens the evidence for deferring, not the
deferral.**

## Not in W6

- **A codegen backend.** Whatever the verdict. `Advance` schedules M9; it does
  not start it.
- **Keeping the spike.** It is deleted when W6 closes.
- **A profiler-derived attribution.** Corollary 2: if a layer cannot be separated
  by substitution it goes in the residue, where a reader can see it.
- **Optimizing anything.** §4 *prices* the alternatives; choosing and building
  one is the milestone that follows the decision, with the number in hand.
- **A comparison against another language's framework.** The `rust-floor` is the
  same syscalls answering the same bytes with no interpreter, which is a
  denominator. A benchmark against axum or Rails would be a benchmark of two I/O
  strategies, and neither number would say anything about the interpreter.
- **Multi-core throughput.** One machine is one core (§5), and a
  process-per-core measurement would be measuring an operating system.
- **Cancellation, backpressure, load shedding.** Still absent, still stated.

---

# The result

Everything below was written after the measurements and changes nothing above it.

## Provenance

The numbers come from `benches/w6-ladder.json` and `benches/w6-spike.json`, and
**the verdict is in neither of them**:

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
cargo run --release --manifest-path crates/ply-codegen-spike/Cargo.toml \
    -- --half benches/w6-spike.json
```

The files carry the machine, profile, postgres version, repeat counts, request
head length and accept loop. **This is the second take**, against a tree that had
gained the constant memo; §12 is what that cost and what changed.

## 8. Where a request's time goes

### 8.1 The ladder

Nine rungs, each a difference between two absolutes taken in one arena in one
run. **The total is measured, not summed.** Each rung carries the same difference
at its widest and its narrowest over the repeats behind it, **because a layer
whose band spans zero has not measured its own sign.** `benches/w6-ladder.json`
is the table; five readings, in descending order of how much they change what
anyone should do next:

**The interpreter is about a third of a request, and that is the number the
decision reads.** Rungs 1–5 attribute more than that, and the residue is
**negative** — the layers sum to more than the request they were read against, so
the in-process arena over-counts against the served denominator. Charging that
seam back to the arena that produced it is what `Ladder::conservative_share`
does (§1.4, §12.3). **The ceiling on any execution-strategy change at that share
is well under the bar §2.3 fixed.**

**The database is more than half of it**, the biggest single row by a factor of
three. It is also the rung with the most caveats: it is `/items` against
`/health` rather than `run` against `run_memory` (§8.4), so `/items`' own decode
and derived JSON encode are inside it. **That cuts against the interpreter share
in the honest direction** — some of the database rung is interpreter — which is
why §8.4 lists it rather than a footnote.

**Framing is what is left of the interpreter.** It is now the largest interpreter
layer, and the one §4's "more native builtins" lever points at.

**The endpoint layer is a memo hit.** `/health`'s whole body is a nullary pure
definition, so the constant memo evaluates it once per process and every later
request reads the remembered value. The rung is now at the resolution of the
in-Ply loop scaffold it is read off and **is not a number to read to two
decimals.** What a route body costs when it is *not* a constant is measured
beside the ladder instead, against the twin.

**TLS and tracing are each about 1% of a request, and neither is worth two
decimals.** Both are differences between two served rows and their bands are
wide; **the tracing rung has not resolved its own sign**, and `Report::audit`
says so above the tables. What survives is the qualitative reading: **the two
features that sound expensive are a rounding error between them, and the
cheapest-sounding one — routing — is several times either.**

### 8.2 One interpreter against another

The cheapest empirical handle on dispatch cost, and the only table W6 took that
needed no new machinery: the same pure request path — `parse_head`, `route_of`,
the endpoint and the encode, same inputs, same returned value, asserted equal
before either was timed — under each engine.

**The tree-walker is substantially faster than the control-stack machine on it.**
That is ADR 0005's allocations-per-frame-push priced as a lever rather than
restated as a fact, and it is a bound on what a cheaper frame representation is
worth. **It is also §5's honest cost of `--engine both`: the divergence guarantee
is not two equal runs.**

### 8.3 What a reader gets today

`benches/w6-ladder.json`'s `offerings` array, at the concurrency that maximises
throughput, with the head length printed. The floor column is the Rust floor
**replaying that row's own response**, because **a multiple whose two sides
answer different bytes is not a multiple** (§12.3).

**Concurrency buys nothing and costs latency**, which is the single most
important operational fact in this document, and it is true of both accept loops.
Throughput is flat from one connection to thirty-two while p99 goes up by three
orders of magnitude. One machine is one core (§5), so concurrency is a queue on
either loop; the spawning one merely spreads the waiting evenly.

**Most of that paragraph's supporting numbers are in no file.** The `w6-ladder`
command sweeps the whole concurrency range but the report keeps only the
`offerings` array, and `--detail` is documented to write the raw served rows
beside the report. **Re-take with `--detail` and commit the rows, or the next
reader has the same problem** — and this is the operational claim `README.md` and
`ROADMAP.md` repeat.

**And the two loops are not the same service.** The sequential loop is
substantially faster on `/health`, for a measured reason that is new to this take
and is in §8.5: **a spawning service memoizes nothing.**

### 8.4 Where the run departed from §1

Stated because a ladder that quietly substituted something else would be a
different measurement wearing this document's authority.

- **Rungs 1–6 were taken on `/health`, not on `/items`.** §1.2 built the ladder
  around `/items` and allowed `/health` where a rung could not be taken on it. In
  the event that was every in-process rung: a pure call to the `/items` handler
  needs a store, and the only one available in process is `std.db`'s memory
  engine, whose SQL scanner is on no served request path. **Including it would
  have measured the twin.**
- **The `database` rung is `/items` against `/health`, not `run` against
  `run_memory`.** The specified substitution prices `std.db`'s memory engine
  rather than the database, so the reported rung uses the route difference
  instead, which §1.1's third rule permits and requires be printed. It is
  printed, and `Report::audit` reports it as a route mismatch.
- **The served rows are on the sequential accept loop, not the
  task-per-connection one §1.6 pins.** The first take made this departure
  silently; here it is deliberate and the reason is measured. `task.spawn` opens
  a production region that stays open for the life of the server, and
  `Machine::constant` refuses the constant memo inside **any** open region — so a
  spawning service memoizes nothing (§8.5), while the in-process rungs are one
  connection at a time and do memoize. **A ladder whose lower rungs memoized and
  whose total did not would divide a numerator in one regime by a denominator in
  another.** Both loops are measured and the other one's rows are labelled.
- **The residue came out negative**, and is charged back to the interpreter
  rather than credited to nobody (§8.1, §12.3).
- **One alternative in §4 was priced end to end**, and it is the one that had
  already landed. The other six carry in-process bounds or nothing. **This is the
  deviation that decides the verdict**, and §10.1 is why it is not a technicality.

### 8.5 Where this is genuinely not competitive

§5 named the candidates before the run and forbade dropping any of them for being
unflattering. All of them survived, and the run added one nobody had predicted:

- **One machine is one core.** A `Value` holds `Rc` and a continuation is
  `Rc<Vec<Segment>>`, so a task cannot move between OS threads. Throughput is
  flat across the concurrency sweep and p99 is not.
- **A request costs tens of times the syscalls under it**, and that multiple must
  be read like for like — same route, same bytes, same transport on both sides.
  Read that way it is roughly half the headline.
- **A service whose accept loop spawns memoizes nothing.** `task.spawn` opens a
  production region for the life of the server, and `Machine::constant` refuses
  the memo inside any open region; the rule's stated reason is a `simulate`
  region's allocation trail, which a production region does not keep. Measured by
  source substitution: disabling the memo costs real throughput on the sequential
  loop and **nothing at all** on the task-per-connection loop, **where there is
  nothing left to disable.**
- **The in-memory twin is slower than the database it stands in for.** `std.db`'s
  memory engine parses its SQL in Ply on every call.
- **`--engine both` costs more than two runs**, because the two engines are not
  equally fast. Against the tree-walk run it is much more than double.
- **The request path allocates far more times than it writes bytes.**
- **The ladder's own residue is negative.**
- **No cancellation, no backpressure, no load shedding.** No number; the absence
  is the statement.
- **`std.trace`'s `Sink` grows its record list in a non-final field**, so a
  collecting twin is O(n²). ADR 0015 named it and W5 measured it; §5 carries the
  mechanism and the one-line fix.

### 8.6 What was not measured

§5 makes an empty list its own audit finding. It is not empty, and
`Report::audit` prints it above the tables. The entries that bear on the verdict:

- **Six of the seven §4 levers**, as end-to-end numbers.
- **Anything outside the spike's fragment**: no `perform`, no handler-stack walk,
  no host boundary, no continuation capture, no closure, no derived codec, and no
  reference counting — the spike frees into an arena. §10.3 is what that costs
  the projection.
- **What a partially-covering backend is worth.** The `solo, trampolined` variant
  was one point on that curve.

  **That variant no longer exists and its figure cannot be re-taken.** It
  compiled one function and let its callees return to the interpreter through
  `rt_call_machine`, the escape hatch §3.2 allows. That helper was a whole
  `Machine::call` entry point on a second, privately held machine — survivable
  only while the sole way *into* compiled code was at the top of a pure integer
  kernel. R5 made the interpreter able to enter compiled code, so the same helper
  became **a route out of a live machine's frame into a different machine's
  `reset()`, discarding the caller's handler stack, trail, region generations and
  footprint in silence.** It is deleted: a call to a function outside the
  compiled unit now refuses the enclosing function at compile time, so a compiled
  set is closed under calls. **The figures in `benches/w6-spike.json` and
  `benches/w6-ladder*.json` stand as what was measured, and the variant that
  produced them is gone.** Partial coverage is now priced the other way round —
  the interpreter drives and enters compiled leaves. `benches/adr0018-mcts.json`'s
  trampoline fields are the same kind of record: taken before R5, not re-takeable
  after it.
- **What a route body costs when it is not a constant**, other than through the
  twin's `/items` handler.
- **The in-Ply loop rungs 2–4 are read off.** It cancels between rungs 3 and 4,
  whose two sides are both loops, and not between rungs 1 and 2, **so the
  endpoint layer carries it.**
- **The TLS handshake** — deliberately; the rung is steady state (§1.2).
- **Multi-core throughput** — deliberately (§5, "Not in W6").
- **`Env::lookup`'s depth sweep**, and the writes and copies a response makes.

## 9. The spike

`std.http.read_line`, chosen by §3.1's rule rather than in advance: the innermost
loop of the framing layer, called once for the request line, once per field line
and once for the terminator, and the highest per-request-cost function whose
**whole body and whole call graph** — `line_at`, `line_stops` — is inside §3.2's
fragment.

**Agreement first, before anything was timed**: every input answered against the
machine **and** the tree-walker, spanning every line offset of three real heads
plus sixteen adversarial cases — empty, past-end, bare LF, bare CR, NUL, DEL, a
zero budget, a negative budget, an exact budget, one over. All agreed. **That is
C4.**

`k` is the minimum conservative ratio over five inputs, and the samples separated
on every one — the spike's worst below the interpreter's best everywhere — so the
spike is evidence. `benches/w6-spike.json` is the table.

### 9.1 Four variants, and the one that matters most

Four variants were run: the whole call graph compiled with literals folded; the
same with literals rebuilt per evaluation; **`read_line` alone with its callees
trampolined**; and the whole call graph on a browser-sized head.

**The two statistics in that table are not computed the same way, and a reader
who divides one gets a different number from the other.** `k` is the *minimum
over the five inputs*; the per-request column is a *sum* over the calls a head
costs. The stated `k` is the conservative one in each case, which is the right
choice — but it is a different statistic from its own row, and the table did not
say so.

**Only the first variant is in `benches/w6-spike.json`.** The other three, and
§9.2's census, are in no measurement file, so `ROADMAP.md`'s claim that the
spike's numbers survive in that file is true of the headline `k` and false of
§9.1 and §9.2.

The second variant answers "is this just constant folding?" — it is not;
rebuilding every literal per evaluation costs a couple of percent.

**The third is the finding.** Compile `read_line` and leave its two callees in
the machine, reached through the trampoline §3.2 explicitly allows, and the
speedup collapses **below `Criteria::defer_spike`**. Two trampolines per call is
the whole difference. **A backend earns its speedup by closing over a call graph,
and the moment it cannot, the win is gone.**

### 9.2 What the fragment can actually reach

The census §3 required, so a ceiling measured on one function is readable against
a backend's coverage. Over `std.http`, `std.router` and `std.json` the fragment
accepts well under half the functions.

Refused by, in descending frequency: a constructor pattern in a `match`, a field
access, a lambda, a list literal, a list pattern in a `match`, `++`, unary `-`, a
call through a local binding or an expression, and `perform`. `parse_head` itself
is refused, for a field access.

**That list is not a set of gaps to fill.** A constructor pattern, a field access
and a lambda are what a route handler and a derived codec are *made of* — a
`derive json` encoder is a record of closures — **so the fragment reaches the
innermost scanning loops and stops exactly where the endpoint begins.**

## 10. The verdict

`ply-corpus w6` recomputes `S`, `k`, `E`, the ceiling and `A` from the two
measurement files and `Criteria::default()`.

| criterion | bar | |
| --- | --- | --- |
| **C1 — Share** | `S ≥ 0.50` | **fail** |
| **C2 — Ceiling** | `k ≥ 3.0` and `E ≥ 1.50` | **fail** — `k` clears comfortably, `E` does not |
| **C3 — Nothing cheaper** | every §4 lever priced, `(E−1) ≥ 2(A−1)` | **fail** — one of seven priced |
| **C4 — Correctness** | agreement and separation | **pass** |

> ## Keep deferring M9.
>
> The first take deferred on C3 alone, with the share and the spike clearing
> their bars comfortably. This one defers on **three** criteria. The share fell
> **not because the interpreter got faster in the ways M9 would have made it
> faster, but because a cheaper lever landed and took interpreter time out of the
> request** — which §10.2 predicted as the most likely way the number would move.
> At the re-taken share an *infinitely* fast execution strategy is worth less end
> to end than §2.3 fixed as what a permanent second execution path has to buy.

`decide` returns at C3, because §2.5 makes an unpriced lever independently
sufficient and it reads no share; the share and the projection are reported
beside it and are what the reopen sentence is composed from.

### 10.1 Why C3 is not a technicality, with a worked example

§2.5 makes an unpriced alternative independently sufficient, and §4 explains why:
W1 predicted codegen was the second lever and W2 then beat it by attacking an
algorithm instead.

This take is the third instance, and **it is no longer an argument — it is a
measurement.** Between the two takes, **one of §4's seven levers landed**:
"caching derived work", as `ply-eval`'s constant memo. Priced the way §4 requires
— a source substitution, both variants served alternately by the same binary,
byte-identical responses asserted before anything was timed — **it is worth more
on `/health` than this document's first take projected a whole code generator
would be worth end to end.** One memoized definition: no new execution path, no
cache key that splits on a flag, no third `--engine` pair to police.

**It also took M9's own case apart**: the share it removed was interpreter share,
so `S` and the ceiling fell with it.

The six still unpriced carry bounds large enough to matter: **the frame
representation** (§8.2's engine substitution, on the pure request path);
**allocation** (a request allocates far more times than it writes bytes); and
**framing**, the largest remaining interpreter layer, which is exactly where W2's
lever applied to W3's work.

None of those costs a permanent second execution path, a third `--engine` pair to
police for `E0503`, a cache key that splits on a flag, or a backend that can
silently break replay determinism (§2.2).

### 10.2 What would reopen it — computed, not written

`Decision::reopens_at` is composed from the criteria that are **not** met, and
nothing else: the share reaching its bar, the projection reaching its bar, and
the unpriced levers being priced with the best of them under a computed
threshold.

**That sentence no longer decides M9, and ADR 0026 §4.2 says why — measured, not
argued.** Its first clause is **satisfiable only by a regression**: at the
re-taken share a backend would need a `k` far beyond anything the fragment has
ever measured to clear the projection, so a counterfactual run with every lever
priced and an enormous spike still returns `Defer` and reduces the whole sentence
to *"M9 reopens when the interpreter share reaches its bar"*. **Nothing a backend
does moves that number.** Worse, the share fell between the two ladders because
the *request* grew faster than the interpreter did — so it moves with postgres
and TLS rather than with Ply. And it sits barely above the point below which the
projection is impossible **at `k = ∞`**, while the sentence still reads as
computed.

What survives is the ladder's actual finding, and ADR 0026 §3 keeps it unamended:
**do not put a JIT in front of this HTTP stack.** What is withdrawn is this
sentence's authority over the *language's* backend question, which ADR 0026 §2.1
shows this instrument structurally cannot be handed — **it refuses any ladder
missing one of nine served-HTTP rungs, so a compute kernel or a lexer answers
`Undecided` by construction.** ADR 0026 §4.3 replaces C1 and C2 with criteria
that can fire, keeps C3 and strengthens C4.

Concretely, for a future contributor:

1. **Price the six remaining levers in §4** as measured end-to-end speedups on
   the served workload §1.6 defines. Not microbenchmarks. The one that has been
   priced shows the shape.
2. **The share has to come back up**, which is the hard part and the honest one:
   **every cheaper lever that lands lowers it further. M9's case gets weaker each
   time this project does the cheap thing that works.**
3. If a lever lands above the computed threshold, build it and re-take
   everything. `S`, `E` and the bar all move together.

### 10.3 The case against this document's own projection

`E` applies `read_line`'s `k` to the whole interpreter share, which assumes a
backend that is as fast on everything the interpreter does. Three measured things
say that assumption is generous, and they are recorded because the projection is
what M9's case rests on and a reader deserves its width:

1. **`read_line`'s own end-to-end value is nearly nothing.** Directly measured,
   not projected: it is a small single-digit percentage of a served request, so
   compiling it **at the spike's own ratio** buys almost the same as compiling it
   *perfectly*. It is a fifth of `parse_head`.
2. **The fragment reaches well under half the functions** (§9.2), and what it
   refuses is what endpoints and codecs are built from.
3. **Coverage is not linear — it is a cliff.** The whole speedup collapses the
   moment two callees stay in the interpreter (§9.1).

Against that, the share is a lower bound in one direction — `/items`' own decode
and encode sit inside the `database` rung (§8.4) — and an over-count in another,
which is why the negative residue is charged back rather than ignored. **The two
corrections push in opposite directions and neither was measured to a conclusion,
which is why they are stated rather than netted**, and why the honest reading of
the projection is *an upper bound with three measured reasons to doubt it* rather
than a forecast.

## 11. What §2 did not anticipate, and one fix

**The criteria had no shape for "C1 and C2 pass, C3 fails alone."** §2.5 lists
five deferral causes as though they were alternatives; in the first take four did
not fire and the fifth did. The list is still right — every entry is still
independently sufficient — but the *reopen* sentence computed for that case was
wrong: `decide` named the share and the spike as what would reopen M9 when both
already cleared their bars. `w6.rs` now composes that sentence out of the
conditions that are **not** met and nothing else, and
`an_unpriced_alternative_defers_whatever_the_share_says` asserts it.

**One obligation is outstanding.** §3.5 and "Not in W6" require that the spike be
deleted when W6 closes. It has not been. **Recorded here rather than done quietly
because a document that claimed a deletion that had not happened would be the
exact failure this ADR is built to prevent.**

**What §2 got right, and it is worth saying because it cost something.** Putting
the thresholds in `Criteria::default()` and keeping `verdict` out of `Report`
meant that when the numbers arrived in a shape nobody predicted — a very high `k`
and a very high `S` producing a deferral — **there was no way to quietly read the
bar off the result.** A document that had written "M9 is justified if the numbers
support it" would have advanced M9 on the projection and never mentioned the
trampoline, the census, or the route table rebuilt on every request.

**And what §2 got wrong, which §12 is.** The same argument was made about the
*thresholds* and not about the two things a verdict is actually read from: **a
measurement file, which no code can tell is older than the program it describes,
and a list of alternatives, which was also a measurement file.**

## 12. What two audits found, and what changed

§8 onward is a **second take**. Two audits — one of the published artifacts, one
of the decision machinery — found ten defects. Three were blockers and they were
three faces of one thing: **the numbers had stopped describing the tree they
shipped in, and nothing in the repository could have noticed.**

### 12.1 The tree moved under the file

`crates/ply-eval/src/memo.rs` — the constant memo, with its `RUNTIME_VERSION`
bump — landed after the first take. A nullary pure definition is now evaluated at
most once per `Machine` or `Interp`, which is exactly §4's **"caching derived
work"** lever: not priced, *built*. `examples/desk.ply`'s `health()` takes no
parameters and performs nothing, and `route_of` reaches `table()`, so `/health` —
the route five of the nine rungs are taken on — **stopped doing most of what the
ladder said it did.**

**The proof that this is a tree difference and not a rig difference needs no
clock:** the two takes' allocation counts for one `/health` request differ by
nearly an order of magnitude, counted by a global allocator, on any machine.

Three consequences, all in this document's own claims:

- The ladder is re-taken end to end, on this tree, by the method §1 pins.
- **W6's "Not in W6 — optimizing anything" was broken**, by a change that landed
  in the same tree. The honest record is that one of the seven levers was built
  rather than priced, and this take prices what it was worth.
- **Two guards now stand where nothing did.**
  `crates/ply-corpus/tests/suite/w6_report_integrity.rs` re-takes the cheap half of the
  ladder in release and compares it against the shipped file; the fix for a
  failure is to re-take, and the message says so with the command.
  `w6_report_allocations.rs` does the same in allocations, **which do not move
  with a machine and so cannot be argued away as load.**

**And the command now writes the file.** `ply-corpus w6-ladder` used to emit a
differently shaped document with `alternatives: []`, and the shipped ladder — its
hand-written levers and limits — was assembled around it by hand. Both staleness
guards told a contributor to "re-take the ladder", and **following that
instruction would have deleted the evidence C3 is decided against.** The command
now emits the whole report, and runs `ply` for the served rows and a `w6-alloc`
binary for the allocation count, because a counting `#[global_allocator]` is a
whole-binary decision. `the_shipped_ladder_is_what_the_command_writes` holds it
there: the shipped file must be byte-identical to `Report`'s own serialization,
**so a field typed in by hand fails a test rather than surviving until the next
contributor re-takes the ladder and deletes it.**

### 12.2 C3 was decided against a field of the file it was judging

§2.6 claims "there is no path from a measurement to the bar it is about to
clear". **There was one, and it was the criterion the verdict turned on.**

`decide` implemented C3's first clause — *every* alternative in §4 is priced — as
"no entry in the `alternatives` array I was handed has `priced: false`". That
array is a field of the same file the ladder comes from; an **empty** one
satisfied the clause vacuously; and `Report::audit` said nothing about it. **Two
lines of `python3` over the shipped ladder turned *keep deferring M9* into
*advance M9*, cleanly, with no audit finding.** The same hole ran through the
values: `priced: true, end_to_end: 1.0` on all seven advanced M9 and audited
clean.

Two changes, both of which move the check out of reach of the run being judged:

- **§4's roster is in code.** `w6::LEVERS` holds the seven levers, `w6::c3_gaps`
  answers which of them a report does not price, and `decide` and `Report::audit`
  both read that. **A file that says nothing about a lever prices nothing**,
  which is what deleting the field now means.
- **A price needs evidence.** `Alternative::evidence` says what the ratio is
  between, in a sentence a reader can check; a lever claimed as priced without
  one is treated as unpriced. `priced: true` is a boolean somebody can type, and
  §4 wants a *measured end-to-end speedup* — **including `1.00×`, which is a
  result, and which is why the fix is a citation rather than a floor on the
  ratio.**

`an_absent_alternatives_list_cannot_advance_m9` and
`a_measurement_file_cannot_price_a_lever_by_asserting_it` are the two tests, and
they are the audit's own cases with their assertions turned around.

### 12.3 Four places the tables claimed more than the measurement had

**A negative residue was printed and ignored.** §1.4 says the residue is credited
to nobody, which makes the share a lower bound — **and that reasoning holds only
when the residue is positive.** A negative one means the layers sum to more than
the request they were read against, which can only be the in-process arena
over-counting, and leaving it uncredited leaves the numerator inflated in the one
direction M9's case rests on. **The first take decided on a negative residue and
called its share "a lower bound twice over" two sentences after saying why it was
not.** `Ladder::conservative_share` now charges a negative residue back to the
interpreter and `decide` reads *that*; a positive residue is still credited to
nobody, so the lower-bound reading survives exactly where it was true.

**One number per rung, and the share straddling its own bar.** The first take
reported two decimals on differences of about 1% between two served rows, each
selected independently from its own concurrency sweep; re-runs of the same
harness flipped the sign. Worse, **three clean re-runs put the interpreter share
on both sides of C1's bar** — the same harness passed it twice and failed it once
— and nothing carried a repeat count or a spread. Now: every rung carries the
worst of its repeats as well as the best, a rung whose band spans zero is an
audit finding rather than two printed decimals, the share is printed as a band,
and **a share whose band falls on both sides of a bar is `Undecided`, because
that ladder answers whichever run was taken.** The two sides of every served rung
are also taken at the **same** concurrency now, so a layer is one flag moved
rather than one flag moved and two rows selected.

**The floor did different work from the total.** The headline multiple divided
`/items` over postgres over TLS by a floor replaying `/health`'s response over
plaintext — **the database rung alone was a third of the numerator and none of
the denominator.** There are now two floors, one per response the service
answers; every row is read against the floor for its own route; and
`w6::Denominators` spells out what each side did, in the file, with
`Report::audit` reporting a report that leaves it blank. The like-for-like number
is in §8.5 as its own row.

**The accept loop the run used was the one §1.6 excluded, and nothing said so.**
§1.6 pins the task-per-connection loop for every served row; `--concurrent`
defaulted to false, so every served row, the total and most offering rows were
sequential, and §8.4 did not list it among the departures it disclosed.

The obvious remedy was to default to the pinned loop, **and taking that
measurement is what found the reason the run had been right for without saying
it**: `task.spawn` opens a production region for the life of the server and
`Machine::constant` refuses the memo inside any open region, so a spawning
service memoizes nothing — while the in-process rungs memoize. A ladder read off
the pinned loop would divide a memo-inert denominator into a memo-active
numerator. **So the fix is disclosure rather than a different default:** the flag
is now `--accept sequential|task-per-conn`, **both** loops are swept on every
run, the ladder is read off the sequential one, and the other becomes labelled
offering rows.

`w6_run::best` also had no tie-break on a throughput curve this document itself
measures as flat, so it selected a different concurrency across three runs of one
harness, once landing on a row whose p99 was a sixth of a second. Within 5% of
the best it now takes the **lowest** concurrency, **which is the one whose
latency is a service rather than a queue.**

### 12.4 What did not change

**The criteria.** `Criteria::default()` holds the same eight numbers it held
before either audit, and **no threshold moved in either direction.** The bar was
not moved; the numbers under it were re-taken, and where the machinery changed it
changed to make a measurement harder to overstate rather than easier to clear.

**The verdict.** Keep deferring M9. `decide` still returns at C3 — now with one
of the seven levers priced instead of none, because it landed — and what changed
is that C1 and C2 no longer clear their bars either, **so the deferral rests on
three criteria where the first take's rested on one.**
