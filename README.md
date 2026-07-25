# Ply

A programming language built around one bet: that generating code is becoming
free, and that what stays expensive is knowing whether it is correct.

So Ply is designed backwards from the verification loop. Not "how do we make
tests run faster" as a tooling concern, but "what would the language have to look
like for the loop to be near-instant and the signal to be trustworthy."

```
$ ply test
47 passed (2.1s)

$ # edit one function body
$ ply test
selected 3 of 47 (44 cached) — 3 passed (0.08s)

$ # rename a top-level function
$ ply test
selected 0 of 47 (47 cached) — rename changed no definition hash
```

That last case is the point. A rename is not "probably safe to skip." It changes
no definition's hash, so there is provably nothing to re-run.

## Three ideas

**Effects are in the type, at resource granularity.** Not `IO`, and not even
`db` — `db.read[users]` is distinct from `db.write[orders]`.

```ply
fn active_users() -> List<User> / {db.read[users]} = ...
```

That precision is what lets the scheduler decide, statically, which tests can run
at the same time: two footprints contend only if they share a resource and one of
them writes.

**Definitions are content-addressed.** The unit of compilation is the definition,
not the file. A definition's hash is computed over its normalized structure, with
references to other definitions replaced by *their* hashes and local names
replaced by de Bruijn levels. A definition compiles once, ever. A test result is
keyed by the test's hash, so it stays valid until something it actually depends on
changes.

**Flakiness is a compile error.** Effects can be declared `nondet`. A test is
deterministic by default, so if a nondeterministic atom survives in its footprint,
the program does not compile — rather than the test failing on its 400th CI run.

```
error[E0412]: nondeterministic effect in a deterministic test
  ┌─ src/user.ply:42:13
42│     let now = clock.now()
  │               ^^^^^^^^^^^ performs `clock`, declared `nondet`
  = handle `clock`, or declare the test `test/nondet`
```

Handlers are what make that practical: swapping a real resource for an in-memory
one is a language construct, not a mocking library, so the double and the real
thing are checked against the same declared signature and cannot drift.

## Status

Early. The M0–M4 vertical slice — parse, typecheck with effect inference,
content-address, evaluate, and run tests incrementally — is what exists. It is
enough to demonstrate the claim above and not much more.

Read [DESIGN.md](DESIGN.md) for the language and the reasoning, [ROADMAP.md](ROADMAP.md)
for what is built and what is planned, and [CONTRACTS.md](CONTRACTS.md) for the
internal crate APIs.

Deliberately not done yet: native codegen, multi-shot continuations, VM-level
snapshot/fork of world state, deterministic scheduling simulation, and
spec-derived property tests. Each has a milestone; none needs a rewrite.

## Building

```
cargo build --workspace
cargo test --workspace
./target/debug/ply test examples/
```

## License

MIT OR Apache-2.0
