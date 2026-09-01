# ADR 0008 — The host effect boundary

**Accepted, implemented.** Settled to implementation precision by ADR 0011,
which wins where the two disagree, because it was written after this one and
against the code.

*Its status line read "proposed" through six shipped milestones, while later
records opened by citing it as settled and built on it. A status of "proposed"
on a document that is the trusted computing base's specification is exactly the
shape of claim this project's audits keep finding, in the harmless direction: it
invites a reader to skip it.*

## Context

Ply cannot reach the outside world. Every handler written for it is in-memory or
simulated; there is no mechanism by which an effect operation becomes a syscall.

The difficulty is not plumbing. Every guarantee this language has —
footprint-directed scheduling, isolation, deterministic replay, an exact test
cache, exhaustiveness proofs over interleavings — rests on the runtime knowing
what a computation can do. **A host escape hatch is by construction a hole in
that knowledge. The whole design problem is making the hole small, declared, and
enumerable rather than ambient.**

## 1. A Ply program never calls the host

There is no FFI expression, no `extern`, no foreign call syntax. A program
performs an ordinary effect operation; the runtime's handler stack may resolve
it to a handler implemented in Rust rather than in Ply.

This is the decision everything else depends on. It means a host-backed effect
and an in-memory one are indistinguishable at the type level, which is precisely
what makes substitution work: the same call site is served by a database in
production and by a map in a test, and neither the caller's type nor its
footprint changes.

## 2. A host handler declares its footprint, and that declaration is trusted

A host handler registers against a specific `(effect, operation, resource)`
triple drawn from an ordinary Ply `effect` declaration, and may not widen it at
runtime.

**The compiler cannot check this.** A handler registered for a read that also
writes a file is lying, and nothing in the type system will catch it. That is
the irreducible trust boundary, and the correct response is to make it
**enumerable rather than invisible**: one command lists every host operation
with its atom, its handler, and its declaration flags.

**That listing is the trusted computing base of a Ply program.** It should be
short, reviewable, and diffable in CI. A change to it is the change most worth a
human's attention in the entire system, because it is the only place a guarantee
can be lost silently.

**What is and is not enforced, exactly**, because the runtime check ADR 0011
adds is easy to over-read:

- **Enforced.** The atom a `perform` reached must be in the declared footprint
  of the entry point that reached it. This catches a *program* footprint that
  under-reports — a `handle` whose clause set covers some but not all operations
  of an atom discharges the atom out of the row and leaves the rest to fall
  through — and a binding that resolved an atom the program's footprints never
  enumerated.
- **Not enforced, and not enforceable.** A handler that does more than its
  registration declared. The atom the runtime records is the one the *registry*
  computed; a handler is handed that atom and has no way to report a different
  one, so a read handler that writes is invisible to everything above the
  boundary. It is recorded as a read, reported as a read, and **scheduled** as a
  read — which means two tests whose footprints say the same read are placed in
  one concurrency group and run beside each other against a resource one of them
  is mutating.

That second bullet is not a gap to be closed later; **it is the trust this
boundary is bought with.** Since footprint conflict grouping is the *only*
isolation a host-backed test has, the honest statement of the residual risk is:
**the isolation of a host-backed test is exactly as good as the registration's
mode and resource, and nothing checks either.** The listing and review are the
whole of the defence, which is why it prints the atom rather than leaving a
reviewer to derive it.

## 3. Determinism is declared, and non-determinism propagates

A host handler registers as deterministic or not, and a non-deterministic one
makes its effect `nondet`, which flows into the existing determinism check and
the test cache exactly as the clock and the RNG already do.

**This is how the flakiness guarantee survives contact with I/O rather than
being quietly suspended for it.** A test that reaches a real socket is not
cacheable, and the compiler says so.

## 4. Tests are hermetic unless told otherwise

Simulated handlers are bound by default; reaching a real host handler requires a
flag, and a run that used one is reported and never cached.

**The default matters more than the flag.** A test suite that silently acquires
a dependency on a live database is the failure mode this language exists to
prevent, and the only reliable defence is that the hermetic path is the one you
get by not thinking about it.

## 5. Every host handler should have a simulated twin, and a law relating them

An in-memory handler satisfying the same declared signature, with a law asserting
the two agree over generated operation sequences. Such a law cannot reach
`proved` — one side touches a socket — so it discharges as `property`, and the
tier label says so. **That is the honest form of the claim every backend team
makes about its mocks and none of them check.**

## 6. Host effects have no isolation but the conflict graph

A socket cannot be forked or region-scoped. So the language's isolation
mechanism, whatever it is, is **declared inapplicable** where it cannot hold
rather than weakened, and footprint conflict grouping is what a host-backed test
has instead.

The reporting obligation is the load-bearing half: a host-backed test must be
subtracted from the trivially-parallel count rather than counted in it, **or the
count silently over-claims.** Two mechanical details a reader would otherwise
get wrong: this is *not* an isolation variant — a host atom is an ordinary
contending atom, and the host category is computed one level up, at the CLI,
because the test scheduler classifies from a footprint alone and has no binding
to ask. And the explain output does not print `host` in place of `shared`; it
prints a separate clause beside the group counts, which keeps the isolated and
selected counts sharing a denominator a reader can check.

## 7. A host handler is linear — at most one resumption

The sharpest interaction and the easiest to get wrong. Resuming a continuation
that performed real I/O would perform that I/O again: charge the card twice,
send the packet twice, insert the row twice. A host handler's continuation may
therefore be resumed **at most once**, and a second attempt is a runtime
diagnostic naming the operation and the handler, not a silent replay.

Note what is *not* the hazard: performing a host operation twice by ordinary
control flow is a retry, and legal. The hazard is one `perform` executing twice
because the control containing it was reinstated. Multi-shot remains available
for pure and in-memory handlers, which is where it was wanted. **The restriction
is on the boundary, not on the feature.**

## 8. Host handlers do not block the scheduler

A blocking handler stalls every task the scheduler owns, so handlers are
asynchronous at the boundary: an operation returns either a value or a pending
token the scheduler polls, and a handler that must block runs on a dedicated
pool.

**How much of that is enforced.** The machine calls every handler on its own
thread — it cannot do otherwise, because a `Value` is not `Send` — so "running
on a dedicated pool" is work the handler does for itself. What the boundary
checks is the observable consequence: a handler registered as blocking must
answer with a pending token, and a value returned from it is this thread having
done the work.

The opposite direction is not detectable and this does not pretend it is: a
handler registered non-blocking that blocks stalls every task in the region and
the worker thread, with no step budget on that path and no timeout, so the run
hangs with nothing to read. So the blocking flag is half mechanical and half a
review obligation, and the column printed is the half that is review.

## Consequences

The trusted computing base becomes a short, listable set of Rust functions, each
with a declared footprint, a determinism flag and a linearity obligation. Every
guarantee above the boundary continues to hold *given* those declarations are
honest, and the declarations are reviewable in one command.

Where this can still fail: a handler that misreports its footprint corrupts
scheduling and isolation **silently rather than loudly**. Given that every
dangerous defect found in this project's audits was a green result over
unexplored space rather than a crash, this boundary deserves harder adversarial
review than anything built so far.
