# ADR 0008 — The host effect boundary

Status: accepted — **implemented in W1**, and settled to implementation
precision by ADR 0011, which wins where the two disagree because it was written
after this one and against the code. §6 is **amended by ADR 0017**; see the note
in §6.

**This status line read "proposed" through six shipped milestones**, while later
ADRs opened by citing it as settled and built on it. Every mechanism named here
is in the tree. **A status of "proposed" on a document that is the trusted
computing base's specification is exactly the shape of claim this project's
audits keep finding, in the harmless direction: it invites a reader to skip it.**

## Context

Ply cannot reach the outside world. Every handler written for it so far is
in-memory or simulated; there is no mechanism by which an effect operation
becomes a syscall. Nothing on the web track is possible until there is one.

The difficulty is not plumbing. Every guarantee this language has —
footprint-directed scheduling, world isolation, deterministic replay, an exact
test cache, `exhaustive` proofs over interleavings — rests on the runtime
knowing what a computation can do. A host escape hatch is by construction a hole
in that knowledge. The whole design problem is making the hole small, declared,
and enumerable rather than ambient.

## Decisions

### 1. A Ply program never calls the host

There is no FFI expression, no `extern`, no foreign call syntax. A program
performs an ordinary effect operation; the runtime's handler stack may resolve
it to a handler implemented in Rust rather than in Ply.

This is the decision everything else depends on. It means a host-backed effect
and an in-memory one are indistinguishable at the type level, which is precisely
what makes substitution work — the same `db.get[users]` call site is served by
postgres in production and by a map in a test, and neither the caller's type nor
its footprint changes.

### 2. A host handler declares its footprint, and that declaration is trusted

A host handler registers against a specific `(effect, operation, resource)`
triple drawn from an ordinary Ply `effect` declaration. It may not widen that
footprint at runtime.

The compiler cannot check this. A Rust handler registered for `db.read[users]`
that also writes a file is lying, and nothing in the type system will catch it.
That is the irreducible trust boundary, and this ADR's position is that the
correct response is to make it **enumerable rather than invisible**:

```
$ ply hosts
  db.read[users]     postgres::read_rows        deterministic: no   linear: yes
  db.write[orders]   postgres::write_rows       deterministic: no   linear: yes
  net.write[socket]  tcp::send                  deterministic: no   linear: yes
  clock.read         host_clock::now            deterministic: no   linear: yes
```

That listing is the trusted computing base of a Ply program. It should be short,
reviewable, and diffable in CI. A change to it is the change most worth a human's
attention in the entire system, because it is the only place a guarantee can be
lost silently.

**What is and is not enforced, exactly.** ADR 0011 §7 adds a runtime check
(`E0427`) and it is easy to over-read, so this ADR states its edges rather than
leaving them implied:

- **Enforced.** The atom a `perform` reached must be in the declared footprint of
  the entry point that reached it. This catches a *program* footprint that
  under-reports — a `handle` whose clause set covers some but not all operations
  of an atom discharges the atom out of the row and leaves the rest to fall
  through to the binding — and it catches a binding that resolved an atom the
  program's own footprints never enumerated.
- **Not enforced, and not enforceable.** A handler that does more than its
  registration declared. The atom the runtime records is the one the *registry*
  computed; a handler is handed that atom and has no way to report a different
  one, so a `db.read[users]` handler that writes, or that opens a file, is
  invisible to everything above the boundary. It is recorded as a read, reported
  as a read, and **scheduled** as a read — which means two tests whose footprints
  say `db.read[users]` are placed in one concurrency group and run beside each
  other against a resource one of them is mutating.

That second bullet is not a gap to be closed later; it is the trust this ADR
buys the boundary with. Since §6 makes footprint conflict grouping the *only*
isolation a host-backed test has, the honest statement of the residual risk is:
**the isolation of a host-backed test is exactly as good as the registration's
mode and resource, and nothing checks either.** `ply hosts` and review are the
whole of the defence, which is why the listing prints the atom rather than
leaving a reviewer to derive it.

### 3. Every host handler declares determinism, and non-determinism propagates

A host handler registers as deterministic or not. A non-deterministic one makes
its effect `nondet`, which flows into the existing E0412 check and the test
cache exactly as `clock` and `random` already do.

This is how the flakiness guarantee survives contact with I/O rather than being
quietly suspended for it. A test that reaches a real socket is not cacheable, and
the compiler says so.

### 4. Tests are hermetic unless told otherwise

`ply test` binds simulated handlers by default. Reaching a real host handler
requires `--host`, and a run that used one is reported and never cached.

The default matters more than the flag. A test suite that silently acquires a
dependency on a live database is the failure mode this language exists to
prevent, and the only reliable defence is that the hermetic path is the one you
get by not thinking about it.

### 5. Every host handler should have a simulated twin, and a law relating them

For each host handler, an in-memory handler satisfying the same declared
signature. M8 then makes the relationship checkable rather than aspirational:

```ply
law "the in-memory store agrees with postgres"
  forall (ops: List<Op>) {
    handle replay(ops) with db.memory(seed)
      == handle replay(ops) with db.postgres(conn)
  }
```

This cannot reach `proved` — one side touches a socket — so it discharges as
`property`, and the tier label says so. That is the honest form of the claim
every backend team makes about its mocks and none of them check.

### 6. Host effects are not part of the forkable world

A socket cannot be forked. A host-backed effect therefore marks its computation
non-forkable, and the consequences follow:

- world isolation does not apply, so those tests fall back to footprint conflict
  grouping as the only isolation mechanism
- `ply test --explain` must report them as *not* world-isolated, so the
  trivially-parallel count stays honest

M6's isolation guarantee is not weakened; it is declared inapplicable where it
cannot hold.

> **Amended by ADR 0017.** There is no forkable world any more, so the premise
> of this section's title is gone while its conclusion is untouched — the
> conclusion never depended on *which* isolation mechanism the language had,
> only on a socket not having one. Read it as: **region isolation does not apply
> to a host-backed test, and footprint conflict grouping is the only isolation
> it has.** That is what `crates/ply-cli/src/hosts.rs` implements, in the
> comment above `Counts`.
>
> Two mechanical details a reader would otherwise get wrong from this section:
>
> - The correction is **not** an `Isolation` variant. `ply_test::schedule::Isolation`
>   has exactly two constructors, `Region` and `Shared`, and a host atom is an
>   ordinary contending atom under both. The host category is computed at the
>   CLI, by `ply_cli::hosts::Counts::of` asking `Hosts::reaches(footprint)`, so
>   that a host-backed test is subtracted from `isolated` rather than counted in
>   it. ADR 0011 §7's first bullet, which promised an `Isolation::Host`, is
>   corrected there.
> - `--explain` does not print the word `host` in place of `shared`. It prints
>   `N host-backed and never free` as a separate clause beside the group counts
>   (`crates/ply-cli/src/commands/test.rs`), which keeps the denominator of
>   `isolated: n of m` the same as `selected: n of m`.
>
> This section's *reporting* obligation is the one ADR 0017 §6 cites as the trap
> to avoid repeating, and `Parallelism::region_contended` exists because of it.

### 7. A host handler is linear — at most one resumption

This is the sharpest interaction and the easiest to get wrong. M6 gave handlers a
real reified `resume`, usable many times. Resuming a continuation that performed
real I/O would perform that I/O again: charge the card twice, send the packet
twice, insert the row twice.

A host handler's continuation may therefore be resumed **at most once**.
Attempting a second resumption is a runtime diagnostic naming the operation and
the handler, not a silent replay.

Multi-shot remains available for pure and in-memory handlers, which is where it
was wanted. The restriction is on the boundary, not on the feature.

### 8. Host handlers do not block the scheduler

A blocking host handler stalls every task the scheduler owns. Host handlers are
asynchronous at the boundary: an operation returns either a value or a pending
token the scheduler polls, and a handler that must block runs on a dedicated
pool rather than on a scheduler thread.

**How much of that is enforced.** The machine calls every handler's `call` on its
own thread — it cannot do otherwise, because a `Value` is not `Send` — so
"running on a dedicated pool" is work the handler does for itself. What the
boundary checks is the observable consequence of having done it: a handler
registered `blocking: true` must answer `HostAnswer::Pending`. A value returned
from `call` is this thread having done the work, and that is `E0428`.

The opposite direction is not detectable and this ADR does not pretend it is: a
handler registered `blocking: false` that blocks inside `call` stalls every task
in the region and, under `ply test`, the worker thread. There is no step budget
on that path, no timeout, and ADR 0011 defers cancellation, so the run hangs with
nothing to read. The one blocking failure the runtime *can* see is a `Pending`
inside a production region that never resolves, which the fruitless-park count
and the deadlock check turn into a diagnostic.

So `blocking` is half mechanical and half a review obligation, and the column
`ply hosts` prints is the half that is review.

## Consequences

The trusted computing base becomes a short, listable set of Rust functions, each
with a declared footprint, a determinism flag, and a linearity obligation. Every
guarantee above the boundary continues to hold *given* those declarations are
honest, and the declarations are reviewable in one command.

Where this can still fail: a host handler that misreports its footprint corrupts
scheduling and isolation silently rather than loudly. Given that every dangerous
defect found in this project's seven audited rounds was a green result over
unexplored space rather than a crash, this boundary deserves harder adversarial
review than anything built so far.

## Not in this ADR

Which host handlers exist. This defines the mechanism; TCP, TLS, postgres and
the clock are separate milestones with their own decisions.
