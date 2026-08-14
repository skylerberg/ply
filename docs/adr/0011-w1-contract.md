# ADR 0011 — The W1 implementation contract

Status: accepted

ADR 0008 settled *what* the host boundary is. This one settles *how*, to the
precision six concurrent implementations need in order to compose. Where 0008
and this disagree, this wins — it was written after it, against the code.

## The rule everything else follows from

> Every guarantee Ply has rests on the runtime knowing what a computation can
> do. A host handler is the one place that knowledge can be wrong. So the
> boundary is built so that a **wrong declaration is loud** and a **missing
> declaration is fatal**, and never the other way round.

Three corollaries, each of which decides a design question below:

1. **A host binding may not change what the front end computes.** If binding a
   Rust handler moved a row, an E0412 verdict or a definition hash, then `ply
   check` would answer differently with and without `--host` and the cache would
   split on a flag. So determinism propagation is checked *against* the source
   declaration at bind time rather than injected into inference (§4).
2. **A host binding may not change what a green result means.** A run that
   reached the host is never written to the result cache, and a pass earned
   hermetically never satisfies a host run (§5).
3. **When the static picture and the dynamic one disagree, the dynamic one wins
   and says so.** The declared footprint is a trusted claim; the runtime checks
   the claim at every host answer it gives and refuses loudly on a mismatch
   (§7). This is the one defence against the failure mode 0008 names as
   irreducible.

## 1. Registration

A host handler is a Rust value registered against an `(effect, operation,
resource)` triple drawn from an ordinary Ply `effect` declaration. There is no
FFI expression, no `extern`, no attribute macro and no inventory-style global
constructor: a `HostRegistry` is *built* by a function in `ply-host`, so the
trusted computing base is a list you can read top to bottom in one file.

A registration carries, at registration:

| field | why it is at registration and not later |
| --- | --- |
| `effect`, `op`, `resource` | the footprint claim; nothing else can supply it |
| `determinism` | must be checkable against the declaration before anything runs |
| `linearity` | decides whether a second resumption is a replay or a no-op |
| `blocking` | decides whether the operation may run on the scheduler's thread |
| `path` | the Rust path, printed by `ply hosts`; the reviewable identity |

**Resources.** A registration names either one resource or `Any`. `Any` is what
a postgres driver needs — it serves `db.get[users]` and `db.get[orders]` alike —
and it is also exactly the widening 0008 §2 warns about, so it is never printed
as `*`. `HostRegistry::bind` resolves `Any` against the atoms the *program*
actually contains, which are enumerable because a resource label is a ground
identifier in the source. `ply hosts` therefore prints one line per real atom,
and a new table appearing in the program appears in the TCB listing on the next
run. That is the difference between "this handler claims everything" and "this
handler claims these four tables", and it costs nothing to have the honest one.

**Binding fails loudly.** `HostRegistry::bind(&CheckOutput)` returns
`Result<HostBinding, Vec<Diagnostic>>` and refuses:

- a triple whose effect, operation or resource the program does not declare
  (`E0421`) — the diagnostic names the nearest declared operation, because the
  common case is a rename on the Ply side that the Rust side did not follow;
- two registrations claiming one triple (`E0422`) — ambiguity here is a coin
  flip over which real resource gets touched;
- a `nondet` handler for an effect the program did not declare `nondet`
  (`E0423`) — see §4;
- an `Any` registration that resolves to no atom at all is **not** an error. A
  driver linked into a program that never queries is idle, not wrong; it
  contributes no line to `ply hosts` and no atom to the binding's footprint.

An empty binding is the hermetic one and is always legal.

## 2. The handler signature

```rust
fn call(&self, rt: &dyn HostRuntime, req: &HostRequest<'_>)
    -> Result<HostAnswer, Diagnostic>
```

The handler receives the resolved atom, the argument values, and the span of the
`perform` — so every diagnostic it raises points at Ply source rather than at
Rust. It returns:

- `HostAnswer::Value(Value)` — the operation completed; the machine returns the
  value into the perform site along the ordinary tail-resumptive path. This is
  the shape for anything that cannot block: a clock read, a byte-slice
  operation, a non-blocking socket write that took the whole buffer.
- `HostAnswer::Pending(Pending)` — the operation did not complete; the token is
  polled by the scheduler. This is the shape for anything that waits.

The two shapes exist because §8 of ADR 0008 requires it, and the split is
exactly the one `ply_eval::sim::Answer` already draws between `Value` and
`Sleeping`: a `Pending` token puts its task out of the enabled set until the
token resolves, and a virtual-clock deadline does the same thing for a simulated
one. The machine treats them identically, which is why the production scheduler
is not a second scheduler (§9).

**When there is no scheduler**, a `Pending` has nowhere to park. The machine
then drives the host runtime until the token resolves — `HostRuntime::block_on`
— which is correct for `ply run` and for a single-task test, and is the only
place in the language where a Ply computation blocks a real thread.

**A handler declared `blocking: true` dispatches its own work to a dedicated
pool and answers `Pending` immediately**, so a handler that calls a blocking C
library cannot stall the tasks it is sharing a thread with. The dispatch is the
handler's, not the machine's: `call` is always entered on the machine's thread,
because a `Value` is not `Send` and nothing in `ply-eval` could hand the work
anywhere else.

The machine checks the half of that which is observable — a `blocking: true`
handler that answers `HostAnswer::Value` did the work on this thread, and that is
`E0428`. A handler declared `blocking: false` that blocks anyway is not
detectable at all: there is no budget on `call`, no timeout, and no cancellation
in W1, so the run hangs. ADR 0008 §8 states that residual rather than implying a
defence for it.

**A handler does not choose the code its failure is reported under.** Several
codes decide classification — `E0505` and the two divergence codes mean "the run
watched its own machinery break", and `ply test` turns them into
`Status::Panicked` and a note telling the reader to file a bug against Ply — and
`E0421`–`E0428` and `E0504` are verdicts about the machine's own state. A
handler minting one has redirected the reader away from itself. So the boundary
rewrites a reserved code to `E0502`, keeping the handler's message, labels and
notes and adding two: what was claimed, and who claimed it. Every handler
refusal, reserved or not, is attributed with the handler path and the operation.
`ply_eval::host::RESERVED_CODES` is the list.

## 3. Linearity

The hazard is precise: M6 gives a Ply handler a reified `resume` usable many
times, and if a captured continuation's control performed real I/O, resuming it
a second time performs that I/O a second time.

```ply
handle {
  log.write[audit](entry);      // a host handler; the packet goes out
  net.write[socket](payload);   // and so does this one
  done()
} with { retry.ask() resume k -> { k(1); k(2) } }   // both sent twice
```

Note what is *not* the hazard: performing a host operation twice by ordinary
control flow is a retry, and legal. The hazard is one `perform` executing twice
because the control containing it was reinstated.

**The mechanism.** The machine holds `host_ops: u64`, incremented once for every
host operation answered with `Linearity::AtMostOnce` in the current entry point.
Every `Continuation` records `born` — the value of `host_ops` at capture — and
carries a resumption counter shared by its clones. A resumption is refused with
`E0426` when

```
resumes > 1  &&  host_ops > born
```

A first resumption is always allowed. A second is allowed whenever no
irreversible host operation has happened since the capture, which is *always* in
hermetic mode, where `host_ops` is zero for the life of the run. So multi-shot
continuations are entirely unaffected by W1 everywhere they are currently used.

**This over-approximates, deliberately.** The rule refuses a second resumption
when an at-most-once host operation happened anywhere after the capture — in
another task, or in the handler clause rather than inside the continuation —
even though replaying that particular continuation would repeat nothing. The
precise rule needs a per-resumption liveness scope on the control stack, which
means a new frame kind interacting with capture, splice, `Next::Leave` and task
start, in the one part of the system where a defect is silent and sends a packet
twice. The conservative rule is four lines and one counter, and its false
positive is a diagnostic on a program that is unusual to begin with. That trade
is the whole of the "when in doubt, refuse" posture this milestone is built on.

`Linearity::Repeatable` is what keeps the approximation tight: it marks an
operation whose replay changes nothing outside the program — a clock read, a
read of an immutable resource — and such operations do not touch the counter. A
handler author choosing `Repeatable` for a socket write is making a false claim
of exactly the kind `ply hosts` exists to put in front of a reviewer, which is
why the flag is printed per handler and folded into the TCB digest.

## 4. Determinism propagation

The naive design has the binding mark an effect `nondet`, which then flows into
E0412. That design is wrong here, and the reason is corollary 1: E0412 is
computed by inference, inference feeds `FRONTEND_VERSION`-keyed caches and the
`det`/`nondet` verdict decides the *result* cache key. A verdict that depends on
whether `--host` was passed is a verdict that splits every cache in the system on
a command-line flag, and makes `ply check` disagree with itself.

**So the arrow is reversed.** The declaration is the authority and the handler is
checked against it:

> A handler registered `Determinism::Nondeterministic` requires its effect to be
> declared `nondet` in Ply source. Otherwise `E0423`, at bind time, naming both
> the handler and the declaration.

E0412 then needs no change whatsoever. `effect net { write send[s](..) }` without
`nondet` simply cannot have a real socket behind it; adding `nondet` to the
declaration is a source edit, it moves the hashes it should move, and every `det`
test that reaches `net.send` fails to compile — which is the guarantee 0008 §3
asks for, obtained from the machinery that already exists rather than from a
second path into it.

The flag is therefore consulted in exactly two places: `HostRegistry::bind`, and
the `ply hosts` listing. Nowhere in inference, nowhere in the cache key, nowhere
in the evaluator.

A `Determinism::Deterministic` handler is still not cacheable (§5). The flag's
content is precisely "this handler may serve an effect the program did not
declare `nondet`", which is a real and reviewable claim, and nothing more.

## 5. Hermetic by default

`ply test` binds `HostBinding::hermetic()`. The registry is still present — it is
compiled in — so the *diagnostic* for reaching the boundary can name the handler
that would have served the operation. What is absent is the binding.

**Reaching the boundary with nothing bound is `E0424`**, and it is deliberately
not `E0303 UNHANDLED_EFFECT`. E0303 means inference should have prevented this
and did not; it is a bug-catcher. E0424 means inference was right, the row was
legal, and the run was configured hermetically. The two call for opposite
responses — file a bug, versus pass `--host` or write a test double — and a
consumer that cannot tell them apart will do the wrong one.

```
error[E0424]: `net.write[socket]` reached the host boundary in a hermetic run
  ┌─ src/serve.ply:31:5
31│     net.send[socket](payload)
  │     ^^^^^^^^^^^^^^^^^^^^^^^^ no handler here, and no host handler is bound
  = `ply test` is hermetic: it binds simulated handlers and refuses real ones
  = handle `net.send[socket]` in the test, or run `ply test --host`
  = `ply_host::tcp::send` would serve this under `--host`
```

**Selection under `--host` stays exact.** The binding's footprint is known before
anything runs, and a test's footprint is an upper bound on what it performs, so
the tests that can reach the host are exactly those whose footprint intersects
the binding's. Those get `Reason::Host`: they always run and are never written to
the cache, in either direction. Every other test is selected exactly as it would
be hermetically — reads its cache entry, writes its pass. `--host` is therefore
not a `--no-cache`, which matters because a service's test suite is mostly tests
that never touch a socket, and making them all re-run would be the tax that
teaches people not to run `--host` at all.

**The runtime is authoritative for the cache decision.** `Record::Host` is
written from what actually happened, not from the prediction. If the two ever
disagree — a test not predicted to reach the host reached it — the run has
observed a footprint that under-reports, which is the failure mode this whole
document is built around, so it is `E0427` and Ply's fault rather than a quiet
uncached pass.

**Bisection and hybrids are skipped for a host-backed failure** (`Skipped::Host`).
M5 re-runs a failing test many times over mixed definition sets; doing that to a
test that sends packets sends the packets that many times. The suspect set is
still computed — it comes from hashes, not from running — so the failure artifact
degrades to the static half rather than to nothing.

Two things make that real rather than aspirational, and both are needed:

- **The fact is dynamic.** `Failure::host` is set from what the runtime did, not
  from the footprint prediction, and a refusal a handler *returned* counts —
  a handler that failed may have acted before it failed, and nothing above the
  boundary can know which.
- **`host` outranks `nondet` in the gate.** Nearly every host-backed test is also
  `test/nondet`, and the two say different things: `nondet` says a hybrid's
  answer would be evidence about nothing, `host` says asking the question is
  itself an action on the world. A reader deciding whether to re-run needs the
  second.

A trial's machine is built by `BodyHybrid::trial` with no binding and there is no
path by which one reaches it. That is deliberate and it is the second lock rather
than the first: `diagnose_failures` builds no hybrid at all for a host-backed
failure, so threading a binding into the trials to "make bisection work under
`--host`" cannot turn the accident into a blocker.

## 6. `ply hosts`

The listing is one line per resolved **triple**, not per registration, sorted by
`(effect, operation, resource)`, so an `Any` handler can never hide a resource
behind a `*`. It prints the atom beside the operation, because the atom is what
scheduling and isolation speak in and deriving it from a mode annotation in
another file is not work a reviewer should do. It ends with a digest over the
canonical form, which is what CI diffs:

```
$ ply hosts --host
   4 host handlers · 6 operations · trusted computing base

   OPERATION           ATOM                HANDLER                    DET  LINEAR         BLOCKING
   clock.now           clock.read          ply_host::clock::now       no   repeatable     no
   db.get[orders]      db.read[orders]     ply_host::postgres::read   no   at-most-once   yes
   db.get[users]       db.read[users]      ply_host::postgres::read   no   at-most-once   yes
   db.put[orders]      db.write[orders]    ply_host::postgres::write  no   at-most-once   yes
   net.send[socket]    net.write[socket]   ply_host::tcp::send        no   at-most-once   no
   task.spawn          task.write          ply_host::task::scheduler  no   repeatable     no

   digest: b3:4f19c0a8e2d3
```

The row key is the triple and not the atom, which matters more than it looks: an
`EffectAtom` carries no operation, so `db.get[users]` and `db.peek[users]` are
*one atom*. A registry keyed on the atom reports those two handlers as a
conflict and refuses a program that is merely ordinary.

Hermetic is not an empty listing — an empty listing is indistinguishable from a
registry that failed to load:

```
$ ply hosts
   hermetic — no host handler is bound

   6 atoms would be bound under `--host`; run `ply hosts --host` to list them
```

`--json` carries the same content with the digest, and `--digest` prints the
digest alone, which is the one-line form a CI check pins. A change to the trusted
computing base is then a one-line diff in a review, which is the whole ambition
of 0008 §2.

## 7. World interaction, and the footprint check

A host-backed computation is **not forkable**, and W1 spends that in four places:

- `Isolation` gains a `Host` variant. A host-backed test is never counted in the
  `isolated: n of m` number, and `--explain` says `host` rather than `shared`, so
  the trivially-parallel count stays honest and the reason is legible.
- Grouping is unchanged: a host atom is an ordinary contending atom, so
  readers-writers over `db.read[users]` still decides what may run beside what.
  0008 §6 asks for footprint conflict grouping as the only isolation mechanism,
  and that is what falls out with no special case.
- A host operation performed **in a test the search re-runs** is `E0425`. The
  premise is that DPOR re-runs a test whole per interleaving — and it re-runs the
  *test*, not the region, so the refusal covers the prefix and the suffix around
  a region exactly as it covers the inside of one. A program whose source sends
  once otherwise sends once per schedule explored, and the run then reports the
  total as a proof over all of them.

  `Machine::innermost_simulation` cannot see the outer shape: it is empty before
  the region is entered and after it closes. So the fact comes from the runner,
  which is the only party that knows how many times it is about to run this test
  — `Machine::set_re_executed`, set from `Plan::re_executes()` and from
  `--simulation measure-reduction`, which runs the whole search a second time
  unpruned. The refusal precedes the handler call, so the count is zero rather
  than one.

  `--simulation once` explores one interleaving, so it does not re-execute and a
  host-backed test may run under it. That is the "run it once without searching"
  answer, reached by a flag rather than by silently dropping the search.

  Opening a **production region** is exempt: `task.*` reaching the binding
  performs nothing outside the program, and §9's three locks already refuse a
  seeded and a production scheduler in one entry point with `E0416`, which is the
  more specific answer.
- Bisection is skipped (§5).

**And the footprint check.** When the machine answers a perform from the host
binding, it checks the performed atom against the declared footprint of the entry
point being run. A mismatch is `E0427`, `Status::Panicked`, not bisected — the
same class as `E0503` and `E0415`, because the run knows two of its own answers
disagree and nothing in the definition graph decides which was meant. This is a
`BTreeSet` lookup per host operation, against a syscall.

**It is armed by every command that runs an entry point**, `ply test` included —
`InterpExecutor` restates the claim before each test on all three machine paths,
because one `Machine` serves many tests per worker and a claim that outlived its
entry point would judge the next test by the last one's row. A check nothing
installs is not a defence; it was unarmed in `ply test` once, and a `det`,
world-isolated test opened a real TCP listener and was reported green.

**What it defends is narrower than "a footprint that under-reports", and the
difference matters.** It compares the atom the *registry* resolved against the
row of the entry point, so it catches:

- a program footprint that under-reports what the program itself performs — a
  `handle` whose clause set covers some but not all operations of an atom
  discharges the atom out of the row and leaves the rest to reach the binding;
- a binding that resolved an atom the program's own footprints never enumerated.

It does **not** catch a handler that does more than its registration declared,
and it cannot: a handler is handed the atom and has no way to report a different
one, so `E0427` can never fire on a `db.read[users]` handler that writes. ADR
0008 §2 now states that residual explicitly, including its consequence for
grouping, rather than leaving a reader to infer a backstop that is not there.

## 8. `Bytes`

`Value::Bytes(Arc<[u8]>)`, mirroring `Value::Str(Arc<str>)` exactly. Not
`bytes::Bytes`: what that crate buys is cheap slicing of a shared buffer, which
W3's streaming bodies want and W1 does not, and it would put a type with its own
refcount semantics into the one enum the hygiene rules are written against.
Slicing copies at W1. W3 revisits it with a reason.

`Lit::Bytes(Vec<u8>)`, written `b"..."`, ASCII plus `\xNN`. A distinct
normalization tag from `LIT_STR`, so `b"ab"` and `"ab"` are different
definitions — they have different types and must not share a hash.

**The UTF-8 boundary** is three builtins rather than one, because Ply has no
`Result` in its prelude and a single partial conversion is a landmine:

- `bytes_of_string(String) -> Bytes` — total, UTF-8 encoding.
- `bytes_is_utf8(Bytes) -> Bool` — the check, so the partial path is avoidable.
- `string_of_bytes(Bytes) -> String` — `RUNTIME_ERROR` naming the byte offset of
  the first invalid sequence.
- `string_of_bytes_lossy(Bytes) -> String` — total, `U+FFFD` for each invalid
  sequence.

`++` stays `String`-only. Overloading it needs type-directed dispatch, which W2
explicitly declines to settle; `bytes_concat` is the honest form until then.

Pattern matching is exact-literal only — `b"GET"` as a pattern works because
`PatternKind::Lit` already carries a `Lit`. Byte-slice patterns are not in W1.

`Bytes` is quantifiable in a `forall`: a generator drawing length `0..=32` and
uniform bytes, shrinking length first and then bytes toward zero, with `b""` as
the minimal value. The alternative — `E0418` on the first `Bytes`-typed law — is
an M8 guarantee quietly regressing on contact with a new primitive, which is
precisely the class of thing this project audits for.

## 9. The scheduler seam

M7 defined `task.spawn` / `task.join` / `task.yield` and built only the seeded
handler. W1 adds the production one, and the decision that makes it small is
this: **a Ply task cannot move between OS threads.** `Value` holds `Rc`; a
continuation is `Rc<Vec<Segment>>`; a `Machine` is single-threaded by
construction and always has been. So the production scheduler is *not* one task
per thread. It is the same cooperative scheduler over the same machine, choosing
by real readiness instead of by a seed, with real threads confined to the host
runtime's reactor and blocking pool where no Ply value ever goes.

That is why there is **one** `Scheduler` type, in `ply_eval::sched`, gaining a
`Policy`:

```rust
pub enum Policy { Seeded, Host }
```

`Seeded` draws its choice from the trail and its time from the virtual clock.
`Host` takes the lowest-numbered ready task, waits on the host runtime when none
is ready, records no steps, and reports no `Exploration`. A second `Scheduler`
implementation living in `ply-host` is exactly the drift M7's design exists to
prevent — "the signature is declared once so a production handler and a seeded
one cannot disagree" is worth nothing if the two are separate code.

**Mutual exclusion, in three independent locks.** A test cannot accidentally get
real threads, and no one of these is load-bearing alone:

1. **Type level.** `task` is declared `nondet`. A `det` test that performs
   `task.*` without a handler is `E0412` and never runs at all. So only a
   `test/nondet` — which is never cached and is opted into by hand — can reach a
   production scheduler.
2. **Stack level.** `simulate` pushes `Delimiter::Sim`, and `find_handler` walks
   the stack innermost-first before ever consulting the host binding, which is
   the handler of last resort. A `task.spawn` inside a region reaches the seeded
   scheduler always, with no special case and no ordering to get wrong.
3. **Binding level.** Nothing is bound without `--host`, and a `task.*` that
   reaches the boundary unbound is `E0424` naming both remedies.

And the residue: a host operation reached from *inside* a region is `E0425`
(§7), and a `simulate` entered while a `Policy::Host` region is live is `E0416`
by the existing `holds_sim` check, because the production region carries a
`SimId` like any other. The two schedulers cannot nest in either order.

**The production region is opened lazily**, at the first `task.*` perform that
reaches the host binding, rooted at the stack it was performed on and closed by
the same `close_regions` path a simulated one uses. Opening it eagerly around
every entry point would make every existing `simulate` nested and `E0416` under
`--host`.

## Dependencies

**tokio 1.53.1**, features `rt`, `net`, `time`, `sync`, and deliberately *not*
`rt-multi-thread`: a work-stealing runtime is unusable here, because nothing it
would steal is `Send`. Tokio earns its place as the reactor and the timer wheel,
which is the part it would be foolish to rewrite.

Rejected, each with a reason:

- **mio** — tokio wraps it; taking both means two reactors or a hand-written one.
- **socket2** — buys socket options W1 does not set.
- **bytes** — see §8.
- **httparse** — W1's endpoint returns a fixed response; it reads to the header
  terminator and answers. A parser in the trusted computing base can wait until
  W3 needs one, and then it is W3's decision with W3's evidence.

The blocking pool is `std::thread` owned by `ply-host` rather than
`tokio::task::spawn_blocking`, so its size is a declared, reviewable number
instead of tokio's default of 512 — a number nobody chose and which decides how
many real database connections a runaway test can open.

## What this contract does not settle

- **Which host handlers exist.** TCP, HTTP, the clock and the production task
  handler are each a separate implementation against this document.
- **Cancellation.** A `Pending` token has no cancel path in W1, so a task blocked
  on a host operation blocks until the operation completes or the run ends. This
  is a real gap and W3's timeouts need it.
- **Backpressure and partial writes.** A socket write that takes part of a buffer
  is the TCP handler's problem, not the boundary's.
- **A host handler written in Ply.** There is no such thing and there should not
  be; the boundary is the point at which Ply stops.
- **Making a false footprint detectable.** §7's check catches a perform reaching
  an atom outside the entry point's row. Nothing catches a handler that does
  *more* than its registration declared, and nothing in this design can — see
  ADR 0008 §2 for what that costs, which is the isolation of every host-backed
  test.
- **A handler that blocks while declaring it does not.** §2's `E0428` catches the
  structural half. The stall itself has no budget and no watchdog, and adding one
  would make a diagnostic depend on wall-clock time, which is the one thing every
  other verdict in this system is built to avoid.
- **A `HostRuntime` that mints a reserved code.** §2's rewrite covers
  `HostHandler::call`. `poll`, `park` and `block_on` are not rewritten: their
  failures really are about the reactor's own invariants, and `ply-host` raises
  `E0505` from them on purpose.
