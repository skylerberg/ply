# ADR 0002 — The incremental front end

Status: accepted — **implemented**. Its *storage* is superseded by ADR 0003 —
the gates, the witness and the invalidation table below stand unchanged; "one
file written atomically" does not.

Both gates are `crates/ply-cli/src/driver.rs`, whose module doc states them in
this ADR's own terms; `--no-incremental` is a flag on four subcommands; and the
equivalence test is `crates/ply-cli/tests/suite/incremental.rs`, **which compares
`DefHash`, `Scheme` and `Footprint` by equality rather than alpha-equivalence
and asserts that gate 1 actually fired before it credits an agreement.**

## Context

DESIGN.md's first row promises "content-addressed definitions; a definition
compiles once, ever". That is not what M0–M4 built. The store caches *test
outcomes* only. Every `ply test` invocation re-reads every file, re-parses it,
re-checks the whole module and re-hashes every definition, because
`check_module` and `hash_module` both take an entire module by construction.

At a few dozen definitions this is invisible. At ten thousand it becomes the new
floor: the result cache would be perfect, selection would be exact, **and the
loop would still be slow — which is precisely the failure this language exists
to prevent. A perfect cache behind a linear front end is a linear system.**

## Decision

The store gains a second, independently versioned cache — the **front-end
cache** — and the loader gains two gates that consult it.

### The maps

```
sources : path    -> SourceFingerprint   { content_hash, deps, imports, defs, tests }
defs    : DefHash -> CachedDef           { scheme, footprint, names }
decls   : DefHash -> CachedDecl          { body, names }
```

`sources` is the namespace half: what names a file declares, where they are, and
what its free names resolved to. `defs` / `decls` are the interface half: what a
definition *means*, keyed by what it *is*.

### Gate 1 — file level, keyed on raw content

A file is skipped without being parsed when both hold:

1. `blake3(raw bytes on disk) == fingerprint.content_hash`; and
2. every entry of `fingerprint.deps` still resolves to the same `DefHash`, and
   is still exported by the module that declares it.

Under modules, (2) has a cheap conservative pre-check: every `ImportEdge`'s
`exports` digest still matches the digest of that module's exports this run. The
digest is over the sorted `(name, DefHash)` pairs, so reordering items in a file
— or reordering the files within a module — does not invalidate an importer.

Its definitions' types and footprints are then loaded from `defs` / `decls`, and
its tests' hashes and footprints from the fingerprint.

### Gate 2 — definition level, keyed on DefHash

Inside a file that *did* change, only those definitions are re-checked whose own
`DefHash` is absent from `defs`. Because a reference contributes the referent's
hash rather than its name, a definition whose dependency changed already has a
different hash, so this single condition covers "its own normalized form
changed" and "a dependency's hash changed" at once. Mutually recursive
definitions share a component hash and therefore invalidate as a unit, which is
also exactly the granularity inference needs.

The decision is per definition and so is the consequence: an accepted definition
is handed to inference as a finished interface (`ply_core::Known`) rather than
re-inferred, so a module is never dragged into a recheck by a module it imports.
`type` and `effect` declarations are the one exception, and they are re-derived
rather than restored: a declaration's signature comes from its own text and
reaches no body, so deriving it costs less than looking one up and checking a
witness.

### The ordering constraint

The two gates use different keys, and the reason is temporal rather than
aesthetic. **You cannot compute a `DefHash` without parsing.** Gate 1 has to
decide whether to parse *before* it holds anything a parse would produce, so its
only available key is the file's raw bytes. Gate 2 runs after a parse it could
not avoid, so it can afford the exact key.

That asymmetry is why `ContentHash` and `DefHash` are separate types. They are
both 32 bytes of BLAKE3 and are trivially confusable; keeping them distinct is
what stops someone asking the definition-keyed maps a question only raw content
can answer. Gate 1 is therefore conservative — reformatting a file, or editing a
comment, costs a parse — while gate 2 is exact and costs nothing for either.

### Where a bug would be silent

Typechecking a definition needs the *types* of its dependencies, which on the
incremental path come from `defs` rather than from re-checking them. This is the
compile-once-ever mechanism and it is the only place where a defect produces a
wrong answer rather than a slow one. Two things make it work, and both were
discovered to be non-trivial:

**A `DefHash` does not by itself determine a `Scheme`.** A hash erases names —
that is what makes renaming free — but a `Scheme` is *written in names*:
`Type::Con(Symbol, ..)` and effect atom labels. So `type A = | X(Int)` and
`type B = | Y(Int)` hash identically (hashing property 5), and therefore so do
`fn f(a: A) -> Int` and `fn g(b: B) -> Int` — while their schemes differ. A map
`DefHash -> Scheme` is ambiguous as written.

The fix is a **resolution witness**: every cached interface records the
top-level names it mentions and the `DefHash` each denoted when it was written,
and is usable only while all of them still hold. The consequence is worth
stating plainly: *renaming a type costs a recheck of the definitions that
mention it.* It still changes no `DefHash`, so it still selects no test and
rebuilds nothing downstream — the headline invariant survives intact — but it is
not free the way DESIGN.md §3 implies. The principled fix is to store schemes in
a name-erased form (types referred to by hash, names reattached from the
namespace on load); that is deferred, and recorded below.

**Visibility is erased, and gate 1 has to notice anyway.** `pub` decides
whether a reference is *legal*, never which definition it denotes, so
normalization drops it and adding or removing it moves no hash. Removing it from
a name another module imports is therefore invisible to both of gate 1's
conditions: the importer's bytes are unchanged and its `deps` still resolve to
the same hashes. Every entry of a fingerprint's `deps` crossed a module boundary
to get there, so every one of them had to be exported for that file to have
compiled; a run refuses any skip candidate naming a definition a parsed module
now declares without `pub`, and re-resolves the body so that `E0107` is reported
against the reference rather than swallowed.

**Schemes must be canonicalized before they are stored or compared.**
`ply_core::env::generalize` quantifies over whatever `TyVar` / `RowVar` numbers
the run's global counter happened to hand out. Check a different subset of the
program and the same definition generalizes to an alpha-equivalent scheme with
different numbers. Byte-identical schemes are a stated requirement of the
equivalence test below, so quantified variables must be renumbered from zero in
a deterministic traversal order at the point of storage. Without this the
equivalence test fails on every polymorphic definition, and — worse — an
implementer's most likely reaction is to weaken the comparison to
alpha-equivalence, which would also hide a real defect.

### Evaluation still needs an AST

The front-end cache accelerates checking, not running. `Interp::new` takes a
`Module`. A test that is a *result*-cache miss must therefore have its file
parsed even if gate 1 skipped it — which happens whenever the result cache was
cleared but the front-end cache survived.

Files are therefore parsed on demand for evaluation. When that happens the
parse's `DefHash`es are compared against the fingerprint that authorized
skipping it; a mismatch means the cache lied, and the run falls back to the full
path with a warning rather than evaluating against a stale interface.

### `--no-incremental` and the equivalence test

`ply check` and `ply test` take `--no-incremental`, which ignores and does not
write the front-end cache. `--no-cache` continues to mean "prove everything
again" and disables *both* caches.

The mandatory test, which is the whole safety argument: **for every corpus, the
incremental path must produce byte-identical `DefHash`es, `Scheme`s and
`Footprint`s to a full from-scratch check.** The equivalence test runs both over
each example corpus and compares the three maps in full. It runs the mutation
sequences in the table below rather than only the clean case, because every
interesting failure is an *invalidation* failure and a cold cache never
exercises one.

Getting this wrong is strictly worse than a stale result cache. A stale result
cache skips a test that would have passed anyway or, at worst, misses a
regression. A stale *type* corrupts the hashes everything else is keyed on: the
wrong scheme yields the wrong normalization for every dependent, which yields
wrong `DefHash`es, which yields wrong cache keys — and the result cache then
happily answers questions about definitions that no longer exist.

### Reporting

`ply check --explain` reports, per definition, whether it was loaded from cache
or rechecked, and per file whether it was parsed or skipped, with the reason a
skip was refused (`content changed`, `dependency <name> changed`,
`import <module> changed`, `no fingerprint`, `` `<name>` is no longer public ``).
The per-definition half is honoured at definition granularity: a module holding
one changed definition reports the rest of its definitions as cached, and a
module that merely *imports* a changed one is not rechecked at all — its
definitions' interfaces come from the store. `ply test --explain` prints the
same block for its front-end phase before the existing selection and scheduling
blocks.

## Invalidation edge cases

| Case | What happens | Why it is safe |
| --- | --- | --- |
| **File deleted** | Its path is absent from the discovered set. Its exports vanish, so every file whose `deps` named one of them fails gate 1 and is re-parsed; the dangling reference surfaces as `UNKNOWN_NAME` from a real parse. `prune` drops the fingerprint. | The killer case for a naive design: the *referencing* files did not change, so a content-only gate would skip them and never report the error. Condition (2) of gate 1 exists for this. |
| **File added** | No fingerprint, so it is parsed. If it redeclares an existing name that is a duplicate-definition error even though the older file was skipped. | Duplicate detection runs over the union of cached and freshly-parsed namespaces, never over parsed items alone. |
| **Definition moved between files** | Both files' content changed, so both are parsed. The definition's `DefHash` is unchanged (source position never enters a hash), so gate 2 reuses its interface, and downstream files see an unchanged export set and skip. | The good case, and a real win: moving code rebuilds nothing. Note that `sources` must be rewritten for both files even though no hash changed — the def-to-file attribution moved. |
| **Import added or removed** | The importing file's content changed, so it is parsed. Removing the last import of a module does not make it dead: its tests still run. | Adding an import can create a module cycle, and cycle detection needs the import graph of *every* file including skipped ones — which is why `imports` is in the fingerprint rather than derived from the parse. |
| **`pub` removed from an imported name** | Visibility never enters a hash, so nothing about the importer moved. Every entry in a fingerprint's `deps` is a name reached across a module boundary, so it had to be `pub`; a skip candidate naming one that a parsed module now declares private is refused, parsed and re-resolved. | The same shape as *File deleted* — the referencing file did not change — and the reason a fingerprint's `deps` are checked against more than their hashes. |
| **A dependency's type changes without its body changing** | Cannot happen through a body edit: a body edit changes the dependency's hash, which changes every dependent's hash. It *can* happen through a rename of a `type` or `effect`, which changes no hash. | Caught by the resolution witness, not by the hash. This is the case the witness was introduced for. |
| **Store older than the source** | Nothing consults mtime, anywhere. Only content hashes decide. | mtime is not a correctness signal: checkouts, containers, `git stash`, and clock skew all move a file's mtime backwards while changing its content, or forwards without. |
| **Two files defining the same module name** | Impossible while a module name is derived from the file's relative path, since paths are unique. If explicit `module` declarations ever land, the fingerprint's key is the path, so the collision is visible across skipped files without parsing them. | Recorded here because it is the case that stops being impossible the moment module naming stops being positional. |
| **Front end changed, evaluator did not** | `FRONTEND_VERSION` bumps; every fingerprint and interface is discarded and the *result* cache survives. | The two caches answer different questions. A change to inference invalidates a type; it does not invalidate a test that was proved by running the code. |
| **Evaluator changed, front end did not** | `RUNTIME_VERSION` bumps; results are discarded and fingerprints survive. | The converse. Conflating them would mean every prelude tweak triggers a full recompile. |
| **A changed file fails to parse or typecheck** | No fingerprint is written for it. Every other file's entries are untouched. | A cache must never record what a failed compile produced; the next run must re-derive it. |
| **Interrupted run** | ~~`sources`, `defs` and `decls` live in one file written atomically, so a crash leaves the previous consistent triple.~~ **Superseded by ADR 0003:** they live in `frontend.idx` (rewritten whole and atomically) over an append-only `frontend.dat`. The pairing is enforced by a `nonce` carried in both file headers rather than by single-file atomicity, and a torn append is *detected* by the frame length prefix and checksum. | Split across two files, a crash between renames could pair a new fingerprint with an interface map that lacks the hashes it names — which is why the nonce, not the file count, is what actually closes this case. |
| **Two `ply` processes at once** | `defs` / `decls` union — they are content-keyed, so a foreign entry is as good as a local one. `sources` is last-writer-wins. | A `sources` entry is never trusted until its `content_hash` is checked against the bytes on disk, so a lost or foreign fingerprint costs a parse, never a wrong answer. |
| **`ply check one.ply` in a large project** | Reads and writes the front-end cache for that one file and does **not** prune. | Pruning is scoped to a run that discovered every `.ply` file under the root. Pruning to a single-file run would delete the rest of the project's work — cheap-looking, and it would silently uncache everything on every partial invocation. |
| **Reformatting, recommenting** | Content hash changes, so the file is parsed; every `DefHash` is unchanged, so gate 2 reuses every interface. | Gate 1 conservative, gate 2 exact — the intended division of labour. |
| **A path that cannot be keyed** (not UTF-8, escaping the root) | `put_source` returns `false`; the file is simply never eligible for the fast path. | The alternative — a lossy key — could make two distinct files share a fingerprint. Slower is always available; wrong is not. |
| **Unbounded growth** | `defs` / `decls` are content-keyed and therefore monotone: every historical version of every definition accumulates. `prune` drops entries no surviving fingerprint refers to. | Deliberately opt-in. Dropping an interface only costs a recheck, but dropping one for a definition that is about to come back (a commented-out function, a branch switch) costs it needlessly. |

## Consequences

Renaming a top-level function must continue to select **zero** tests. Nothing
above changes that: renaming changes no `DefHash`, so no test hash moves. It now
additionally costs a recheck of the definitions that mention the renamed name,
which is a regression against "compiles once, ever" and is the price of storing
schemes in named form.

Gate 1 is worth little until modules land. With a directory as one flat
namespace, condition (2) is evaluated against the whole program's exports, so
almost any edit anywhere invalidates almost every file. The name-granular `deps`
form is what keeps it useful in the flat case, and is why it is specified as
normative with the module-granular digest as a pre-check rather than the other
way round.

## Not done here

The gates, the `--explain` output, the `--no-incremental` flag and the
equivalence test were all deferred when this ADR landed and have all shipped.
What is still deferred, by choice:

- **Name-erased schemes.** The principled fix for the witness mechanism, and the
  thing that would make a rename genuinely free rather than merely cheap.
- **Cross-project sharing.** The `defs` map is content-keyed and would federate,
  but nothing depends on that yet.
- **Parallel gate evaluation.** Gate 1 must run in dependency order over the
  module graph, since a skipped file's exports come from its own fingerprint and
  are only valid if it too passed gate 1. Ordered, not necessarily serial.
