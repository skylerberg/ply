# ADR 0008 — The host effect boundary

Status: proposed

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
