# ADR 0002 — The incremental front end

**Accepted, implemented.** Its *storage* is superseded by ADR 0003 — the gates,
the witness and the invalidation rules stand; "one file written atomically" does
not.

## Context

The promise is "a definition compiles once, ever". What shipped cached *test
outcomes* only: every run re-read every file, re-parsed it, re-checked the whole
module and re-hashed every definition. At a few dozen definitions that is
invisible. At ten thousand it becomes the new floor — the result cache would be
perfect, selection exact, **and the loop would still be linear, which is
precisely the failure this language exists to prevent. A perfect cache behind a
linear front end is a linear system.**

## The two gates

The store gains a second, independently versioned cache: `sources`
(path → fingerprint: content hash, deps, imports, defs, tests) and
`defs`/`decls` (hash → interface). The first is the namespace half — what names
a file declares and what its free names resolved to. The second is the interface
half — what a definition *means*, keyed by what it *is*.

**Gate 1, file level, keyed on raw content.** A file is skipped unparsed when
its bytes hash equal and every dependency still resolves to the same hash and is
still exported. There is a cheap conservative pre-check per imported module: a
digest over the sorted `(name, hash)` export pairs, so reordering items within a
file does not invalidate an importer.

**Gate 2, definition level, keyed on hash.** Inside a file that did change, only
definitions whose own hash is absent are re-checked. Because a reference
contributes the referent's hash, one condition covers both "its own form
changed" and "a dependency changed". Mutually recursive definitions share a
component hash and invalidate as a unit, which is exactly the granularity
inference needs. An accepted definition is handed to inference as a finished
interface rather than re-inferred, so a module is never dragged into a recheck
by a module it imports.

**The ordering constraint is temporal, not aesthetic.** You cannot compute a
definition hash without parsing. Gate 1 has to decide whether to parse *before*
it holds anything a parse would produce, so its only available key is raw bytes.
Gate 2 runs after a parse it could not avoid, so it can afford the exact key.
That asymmetry is why `ContentHash` and `DefHash` are separate types: both are
32 bytes of BLAKE3 and trivially confusable, and keeping them distinct is what
stops someone asking the definition-keyed maps a question only raw content can
answer. Gate 1 is conservative — reformatting costs a parse — and gate 2 is
exact, so reformatting costs nothing beyond it.

## Early cutoff: a definition hash is the wrong key for a recheck

A definition hash is transitive by construction, so editing one body moves every
transitive caller's hash and gate 2 rechecks the whole caller cone. That was not
waste while signatures were inferred — a caller's type could depend on a
callee's body, so the cone was the honest answer. Written signatures removed the
dependency, and the transitive key now answers a question nobody asked at that
site: it asks *"has anything in this closure changed?"*, which is right for the
test cache and too strong for a recheck.

So gate 2 keys on two narrower things and the transitive hash keeps the test
cache unchanged:

- **own hash** — the definition normalized with references *by name*. It moves
  when the definition's own text moves and not when something it calls is
  re-implemented. A gate key and never an identity: nothing is selected, cached
  or referenced by it, and its name-dependence errs toward more rechecking.
- **interface hash** — over the published scheme, footprint and constraints,
  which are everything a caller can observe. The footprint is in there because
  effect rows are still inferred: a body edit that adds a `perform` does change
  what callers must be checked against.

Two readings of that are wrong and both were tried on paper first. Accepting a
definition because its *own* hash is unchanged is unsound — every caller of an
edited definition has unchanged own text. Accepting it because its own hash is
unchanged *and* every callee is stable, bottom-up in one pass, is sound and buys
nothing: a rechecked callee is not stable until its fresh interface is known, so
the condition propagates exactly like the transitive hash.

The cutoff therefore runs as waves. The first checks definitions whose own hash
moved; each later one compares a rechecked interface against the stored one and
admits the callers of only those that differ. It terminates because the recheck
set only grows. Only the final wave's output escapes — an intermediate wave can
hand a caller a stale interface and report a diagnostic a from-scratch check
would not, so a wave that restored any interface and then failed is thrown away
and re-run restoring none. A type error therefore costs two checks.

**The invariant that decides every ambiguous case: a missing entry, an absent
fingerprint or a wave that cannot tell means recheck.**

## Where a bug would be silent

Typechecking a definition needs the *types* of its dependencies, which on the
incremental path come from the store rather than from re-checking. This is the
compile-once mechanism and the only place a defect produces a wrong answer
rather than a slow one. Three things make it work, and all three were discovered
to be non-trivial.

**A hash does not by itself determine a scheme.** A hash erases names — that is
what makes renaming free — but a scheme is *written in names*. `type A = | X(Int)`
and `type B = | Y(Int)` hash identically, so `fn f(a: A) -> Int` and
`fn g(b: B) -> Int` do too, while their schemes differ. A map from hash to
scheme is ambiguous as written.

The fix is a **resolution witness**: every cached interface records the
top-level names it mentions and the hash each denoted when it was written, and
is usable only while all of them still hold. The consequence stated plainly:
*renaming a type costs a recheck of the definitions that mention it.* It still
changes no hash, so it still selects no test and rebuilds nothing downstream —
but it is not free the way "compiles once, ever" implies. The principled fix is
name-erased schemes, with types referred to by hash and names reattached on
load; that is deferred.

**Visibility is erased, and gate 1 has to notice anyway.** `pub` decides whether
a reference is legal, never which definition it denotes, so normalization drops
it. Removing it from a name another module imports is therefore invisible to
both of gate 1's conditions — the importer's bytes are unchanged and its deps
still resolve to the same hashes. Every dependency crossed a module boundary to
get there, so every one had to be exported; a run refuses any skip candidate
naming a definition a parsed module now declares private.

**Schemes must be canonicalized before they are stored or compared.**
Generalization quantifies over whatever variable numbers the run's counter
handed out, so checking a different subset of the program yields an
alpha-equivalent scheme with different numbers. Quantified variables are
renumbered from zero in a deterministic traversal order at the point of storage.
Without this the equivalence check below fails on every polymorphic definition —
and, worse, an implementer's most likely reaction is to weaken the comparison to
alpha-equivalence, which would also hide a real defect.

## The safety argument

**For every corpus, the incremental path must produce byte-identical hashes,
schemes and footprints to a full from-scratch check.** `--no-incremental`
ignores and does not write the front-end cache; `--no-cache` disables both.

The equivalence check runs mutation sequences rather than only the clean case,
because every interesting failure is an *invalidation* failure and a cold cache
never exercises one. It compares by equality rather than alpha-equivalence, and
asserts that gate 1 actually fired before it credits an agreement.

Getting this wrong is strictly worse than a stale result cache. A stale result
cache skips a test that would have passed anyway. A stale *type* corrupts the
hashes everything else is keyed on: the wrong scheme yields the wrong
normalization for every dependent, which yields wrong hashes, which yields wrong
cache keys — and the result cache then happily answers questions about
definitions that no longer exist.

## The invalidation cases that teach the model

- **A file is deleted.** The killer case for a naive design: the *referencing*
  files did not change, so a content-only gate would skip them and never report
  the error. Gate 1's dependency condition exists for this.
- **A definition moves between files.** Both files' content changed so both are
  parsed; the hash is unchanged so gate 2 reuses the interface; downstream sees
  an unchanged export set and skips. The good case, and a real win.
- **A dependency's type changes without its body changing.** Impossible through
  a body edit — that moves hashes. Reachable through renaming a `type` or
  `effect`, which changes no hash. Caught by the witness, not by the hash; this
  is the case the witness exists for.
- **The store is older than the source.** Nothing consults mtime, anywhere.
  mtime is not a correctness signal: checkouts, containers, stashes and clock
  skew all move it backwards while changing content, or forwards without.
- **Two processes at once.** Interface maps union — they are content-keyed, so a
  foreign entry is as good as a local one. Fingerprints are last-writer-wins,
  and a fingerprint is never trusted until its content hash is checked against
  the bytes on disk, so a lost or foreign one costs a parse, never a wrong
  answer.
- **Checking one file in a large project** reads and writes the cache for that
  file and does **not** prune. Pruning is scoped to a run that discovered every
  file under the root; pruning on a partial invocation would silently uncache
  the rest of the project.
- **A path that cannot be keyed** (not UTF-8, escaping the root) is simply never
  eligible for the fast path. The alternative — a lossy key — could make two
  distinct files share a fingerprint. Slower is always available; wrong is not.
- **The front end changes, the evaluator does not.** Fingerprints and interfaces
  are discarded and the *result* cache survives. The two caches answer different
  questions: a change to inference invalidates a type; it does not invalidate a
  test that was proved by running the code. The converse holds too, and
  conflating them would mean every prelude tweak triggers a full recompile.
- **A changed file fails to compile.** No fingerprint is written for it. A cache
  must never record what a failed compile produced.
- **Unbounded growth.** Interface maps are content-keyed and therefore monotone:
  every historical version accumulates. Pruning is opt-in, because dropping an
  interface costs a recheck and the definitions most likely to be garbage — a
  commented-out function, the other side of a branch — are the ones most likely
  to come back.

## Consequences and what is still deferred

Evaluation still needs an AST, so a test that is a *result*-cache miss has its
file parsed even when gate 1 skipped it. The parse's hashes are compared against
the fingerprint that authorized the skip; a mismatch means the cache lied, and
the run falls back to the full path with a warning rather than evaluating
against a stale interface.

Still deferred: **name-erased schemes**, the principled fix for the witness and
the thing that would make a rename genuinely free rather than merely cheap;
cross-project sharing, which the content-keyed maps would support; and parallel
gate evaluation, which must run in dependency order over the module graph since
a skipped file's exports come from its own fingerprint.
