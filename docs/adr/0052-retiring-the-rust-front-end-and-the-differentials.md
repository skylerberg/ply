# ADR 0052 — Retiring the Rust front end and the differentials

**Accepted as an ordering.** ADR 0042 set six steps toward one implementation
of the language, and ADR 0048 finished the fifth: the tier is the only
evaluator. The sixth, deleting the Rust front end, was deferred by ADR 0049
§4 behind the differentials that compare the port against it, and nothing
since has moved it. The three records between were about speed, and they
made the cost of the deferral visible: every change to what the emitter
emits is now made twice, in `c/emit.rs` and in `emit.ply`, byte for byte,
and then the bootstrap bundle is refreshed from CI, because the Rust
reference is still the oracle the port is held to. This record takes step
six in the order the goldens allow, retires each differential into what
survives it before its oracle goes, and holds CI under three minutes while
the tree shrinks; the deletions are the lever for that, not a cost to it.

## What the tree is, read before anything was moved

**Two front ends run on every program.** `ply run`, `ply test`, `ply prove`
and `ply build` enter the driver, which runs the Rust chain whole — parse,
derive, resolve, hash, check — for the diagnostics, the definition hashes,
the constructor table and the load order, and only then builds a backend.
The backend hands the port the module names and the module source bytes,
and the port parses, resolves, checks and lowers them again to emit
(`backend.rs`: *it is a front end, not an AST consumer*). `ply check`,
`ply hash` and `ply std` never enter the port. The port's own diagnostics
reach a user only as one string behind `BACKEND_UNAVAILABLE`. `ply
bootstrap` never installs the producer and so still emits with the
reference fragment, which refuses `perform`, `handle`, `with cell` and
`simulate`; the archive it writes is not the language.

**What Rust remains, and who reads it.** The front end is five crates:
`ply-span` (the codes and the diagnostics), `ply-syntax`, `ply-derive`,
`ply-core`, `ply-hash`; some twenty-four thousand lines. `ply-core`'s type
vocabulary — `Footprint`, `Scheme`, `EffectAtom`, `Resource`, `Type`,
`IntTy` — is read by `ply-eval`, `ply-codegen`, `ply-hash`, `ply-store`,
`ply-prove` and `ply-test`, all of which survive the front end; that
coupling, not `infer.rs`, is the cost of retiring the checker. `ply-eval`
still holds the values, the builtins, the host bindings, the regions and
the simulation the runtime calls through, and the lowering (`code.rs`) the
emit differential's oracle lowers with. `ply-codegen` is a runtime
(`heap.rs`, `rt.rs`, `list.rs`, `map.rs`, `detached.rs`, the prelude and
the helper table) beside a reference emitter (`c/emit.rs`, `opt.rs`) that
no user program reaches any more: with the bundle serving, the port's
answer is the unit's and the reference is not run. The reference's three
remaining uses are the differentials' oracles, `ply bootstrap`, and the
fallback `build_from` takes when the bundle does not serve — which under
tier-only already cannot emit the compiler whole.

**The differentials.** `crates/ply-compiler-diff` compares the port
against the Rust reference at every phase. Seven of them are held to
goldens under `fixtures/goldens/` — lexer, parser, rewrite, resolve,
derive, infer, hash — and `golden::check` asserts three things: the golden
exists, the reference still agrees with it, the port agrees with it. The
last is what survives the reference. The resolve and hash goldens are
digests, and a digest that mismatches is explained by diffing against the
reference's dump *while there is one*. The lowering and emit differentials
have no goldens: ADR 0050 said the bundle is theirs, and the own-sources
emit differential holds the port to a floor of agreeing bodies with the
rest printed, some two hundred and forty bodies where the port lacks a fast
path the reference has. `fields.rs` and the corpus-mining scripts read Rust
source text.

**What holds the port without the reference.** The compiler's own tests
on the tier, the standard library's, the examples', the language corpus,
the raising fixtures, `same-tests`, the bootstrap fixpoint, and the
hazard, producer and fragment suites: six hundred and some `test` items in
Ply and one fixpoint, all of which run against the port only, plus the
goldens once their reference arm is gone.

**CI.** A quiet main run is over three minutes: 282 s on the run that
merged ADR 0051's last lever, with the archive build at 164 s (the
`cargo test --no-run` at 124 s, a serial chain `ply-codegen` →
`ply-test` → `ply-cli` → `ply-corpus` → its binaries, recompiled whenever
`ply-codegen` changes, and main cannot read the cache a pull request's run
filled), the longest partition at 105 s (51 s of tests, the rest the
archive restore, the object-cache restore and save, and tcc), and the
`emit-diff-own-sources` solo at 72 s. Pull-request runs of the same trees
built in 83–91 s.

## 1. The port is the only front end

Enter the port once from the driver, source bytes in, and take everything
back from it: the diagnostics with the user's spans, rendered by `ply-span`
so the terminal and `--json` read as they do now; the definition hashes the
store, `ply test`'s selection, `ply prove` and `ply build` key on; the load
order, the constructor table and what the backend needs. Every command goes
through it, `ply bootstrap` included. First, `ply_core::ty` moves into a
crate the runtime and the tools keep. Every diagnostic code a program can
raise in the front end reaches the user from the port; the codes the front
end can no longer raise are deleted, and `docs/GUIDE.md` §18 stays total
over what remains. The differentials are the instrument one last time:
before the Rust side stops running on a program, the port's answers over
the shipped corpus, the standard library, the examples and its own sources
are the reference's.

**Built when.** Each step lands as its own pull request; the differentials
and the Ply-side gates are green on each.

**Built, 2026-09-16: the vocabulary.** `crates/ply-ty` holds the types,
the effect rows and atoms, the footprints and schemes, the printer, the
integer widths and the declaration kinds that were `ply-syntax`'s, and
the checker's output shape (`CheckOutput` and what it names); it depends
on `ply-span` alone. `ply-core` re-exports all of it and keeps the
checker. `ply-hash`, `ply-store` and `ply-host` no longer depend on
`ply-core`; the runtime, the prover and the test runner read `ply-ty` and
call `ply-core` only for the checker itself and the prelude's constructor
table, which move when the port answers them.

## 2. Delete the Rust front end, its test crates and the differentials

In the order the goldens allow: `ply-derive`; `ply-syntax`'s rewrites,
resolver, parser and lexer; `ply-core`; `ply-hash`; then `ply-eval`'s
lowering with `opt.rs` and `c/emit.rs` last; with `ply-derive-tests`,
`ply-syntax-tests`, `ply-core-tests`, `ply-hash-tests`,
`ply-compiler-diff` and the `tools/mine-*.py` scripts. Before each
deletion its differential retires into what survives: `golden::check`
drops its reference arm and holds the port to the golden alone; the
digested goldens are re-blessed as text first, since a digest mismatch
reports nothing once there is no reference to diff against; `fields.rs`
goes with the parser. The lowering and emit differentials retire into the
bootstrap fixpoint and the behavioural gates. Before `c/emit.rs` goes,
this record names the bodies the port still emits differently and why each
is a missing fast path rather than a wrong rule, because a wrong rule the
port applies to itself is a fixpoint.

The bundle becomes the only way to build the language. This record says
how a fresh clone builds it — the bundle's C, `cc`, `dlopen`, nothing
parsed — how a broken or unserved bundle is recovered with no Rust emitter
(the last serving bundle from history, and the refresh that iterates to its
fixpoint), and how a helper-table or object-layout change is carried across
it: ADR 0050 §1a's prefix rule where a helper is appended, a bundle
migration by textual transformation where it is not. ADR 0042 step 6 stands
corrected in place by what this finds.

**Built when.** One deletion per pull request, each with its differential's
retirement in the same change or the one before it.

## 3. CI under three minutes, restored first and then held

A quiet main run under 180 s wall before anything else in this record, and
every merge after it under; a run over it is the next item before any
other. The readings above name the levers in order: main rebuilding a tree
a pull request's run already built, the longest partition's overhead
against its tests, and the pole solo, which retires with the emit
differential. The front-end crates are a quarter of the Rust the archive
builds. `.github/ci-shards.sh`'s tables shrink with the crates, with
`verify` still true. The wall clock of the run that merged each pull
request is the reading, from the run itself, and the record says where the
time went.

**Built, 2026-09-16: main reuses the pull request's build.** A pull
request's run uploads its archive and its release binary by tree hash,
and a push to main whose tree a run already built takes them instead of
compiling; the object cache is keyed by what decides a unit's contents,
so a run that changed none of it skips the save. The run that merged it
read 127 s wall against 282 s: both build jobs 18 s, the longest
partition 94 s and the postgres job 88 s, which are the poles now. A pull
request's run pays the upload, nine seconds, and stays near four minutes
with its own release build as its pole.

## 4. The loop that is O(the change)

Once the compiler's tests are Ply tests, `ply test`'s content addressing
re-runs only what an edit touched, which ADR 0042 said would fall out of
step six rather than be built. The record takes what a one-line edit to
`emit.ply` costs, locally and in CI, before and after, from the tool's own
output.

## The order, and why

3 first, because every pull request of this record is read by its run's
wall clock and a clock over the bound is not a reading of the change. 1
before 2, because a deletion with nothing in its place is a regression, and
the port answering everything a command needs is what puts something in
place. 2 in the goldens' order, cheapest first, because a golden-backed
differential retires into a file and the lowering and emit differentials
retire into a fixpoint and a corpus, which is the weaker witness and should
go last. 4 last, because it is a consequence of 2 rather than work.

## What this record does not decide

The runtime. `ply-codegen`'s runtime half, `ply-eval`'s value, builtin,
host, region and simulation layer, `ply-host` and `ply-span` stay until a
C runtime exists over libc behind the same helper table, which is
`docs/BOOTSTRAP-PATH.md` step 4 and the record after this one.

## What would make this wrong

- **If the port's answers cannot be taken without a value bridge.** ADR 0042
  step 3 said no Rust would read a Ply value; the port's diagnostics,
  hashes and tables have to cross as bytes in a form `ply-span` and the
  store already read. If a value bridge is the only way, step 1 is a design
  record before it is code.
- **If the fixpoint hides a wrong rule.** The fixpoint proves the emitter
  agrees with itself, not with the language; the behavioural gates are the
  witness after the reference goes, and a body they do not reach is
  unwitnessed. The named list of differing bodies, taken before `c/emit.rs`
  goes, is what makes that risk legible rather than absent.
- **If the bundle is lost.** One binary blob in git builds the language.
  History holds every serving bundle, and the refresh iterates; if a
  layout change ever leaves no bundle that serves, the recovery is a
  migration, not a Rust emitter.
