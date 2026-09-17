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
derive, infer, hash — and `golden::check` asserts two things: the golden
exists, and the port agrees with it. The reference arm is gone, and with
it the last reader of the Rust front end in those seven; blessing writes
the port's answer now, which makes blessing a deliberate act of moving the
specification rather than of taking the reference's. Every golden is text.
The resolve and hash goldens were digests and were re-blessed before the
arm went, because a digest reports only *that* something differs and the
difference itself used to be read off the reference's dump. The lowering and emit differentials
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

**Built, 2026-09-16: the message channel.** The port's diagnostics carried
a code, spans, a label count and a note count; they now carry the
reference's message, label texts, notes and severity at every site, and
one entry, `diag.diag_dump`, answers every diagnostic a program raises in
the driver's order as length-framed text: `diag <i> <n>` frames of
`<key> <n>` fields, so that no payload byte is a delimiter. `ply-span`
reads that text back into its own `Diagnostic`, naming each of the
forty-seven codes the port raises with a literal constructor so the
armed-code check holds them after the reference goes, and writes one
from a `Diagnostic` so the two forms cannot drift. The differential
holds the port's dump to the reference chain's over the mined corpora,
the standard library, the examples, the compiler's own sources and the
fixtures, the first comparison of messages this tree has had; the
existing dumps and their goldens did not move. One code stays the
driver's: `E0111`, a module name derived from a file path the port never
sees. What the reference prints from data the port lacks is recorded in
`crates/ply-compiler/GAPS.md` §11.

**Built, 2026-09-16: the tables.** `front.front_dump` answers, after the
diagnostics, the module order, each module's items and imports, every
definition's scheme, footprints, constraints, row aliases, specs and
span, the tests and laws with their names and spans, the effects with
their operations, the constructors with their fields, every hash the
hasher publishes, the ordinals the cache keys are numbered by, and the
normalized bytes the store files. `ply-ty` reads them back into the
`CheckOutput` and `HashOutput` the tools already hold, through a parser
for the printed type forms, so a scheme crosses as the text the printer
and the checker already agree on rather than as a second encoding; the
hash vocabulary moved there with it. Two shapes the frames take from the
data rather than from the draft: a test is headed by its index, because
a label may hold a space or a newline and a header may not, and a body
travels as hex, because the dump is text and the bytes are not. The
differential holds the port's answer to the reference chain's over the
same corpora as the diagnostics, after the reference's own answer
round-trips through the reader.

**Found on the way, by the tier gate and by nothing else.** The
compiler's own tests, run by the compiled compiler, aborted on one new
function, `bytes_at(b, bytes_len(b) - 1 - i)`: the port's release mode
(ADR 0046) spends a value at its last read, and the inline `bytes_at`
path evaluated its index, which held that last read, before it bound its
buffer, so the buffer read handed zero to the runtime. The Rust reference
has no release mode and the differential compares the port with release
off, so no byte comparison could see it, and the fixpoint was green over
it. The behavioural gate caught it, which is the witness §2 says survives
the reference; the shape is in the language corpus on both tiers now, in
four forms.

**The switch, in stages.** A survey of every reader of the syntax tree
and the resolver after loading found the driver is not the only one.
The backend's tables (`Source::ctors`, `functions`, `emit_keys`, the
producer's `bodies_of`) are the module order, the ordinals and the hashes
the port answers, and take them first while the Rust chain still runs,
held green by the emitted C not moving a byte. Five readers need a frame
the port does not answer yet, each one small: item visibility with the
`reuse` marker and a type's arity for the store's fingerprint; parameter
names and spans for the prover's obligations; `effect set` expansions
for `ply check --explain`; the literals a law's guard mentions, the
prover's witness seed; and the normalized bodies. Then the driver enters
the port once and its two gates, `Resume`, the restored interfaces and
`check_program_with` go. Two readers remain the interpreter's: `Pure`
evaluating a configuration or database schema constant, and the prover's
claims, whose bodies the e-graph lowers from the tree; those become
frames the port answers, or stay fed by one, in the record's last step
of §1. One prerequisite is not on §2's list and is done first: the
operator, literal and visibility vocabulary leaves `ply-syntax`'s tree
for `ply-ty`, so the runtime and the tools stop naming the parser's
crate.

**Built, 2026-09-16: the backend's tables.** A unit takes a `Front` and
derives from it what it read from the tree: the constructor table, the
root list, the cache keys, each root's arity, the module count, and the
arguments the producer hands the port. `ply bootstrap`'s source digest
and the artifact's stored bodies come from it too. The Rust chain answers it
while it still runs, because asking the port is a second front end over
the program and the standard library for every unit; `PLY_FRONT=port`
asks the port instead, one gate runs with it set, and the port becomes
the only answer when the chain goes. The producer is also built before it
can be asked, and `ply bootstrap` installs none. The tree is still read
for the reference emitter's bodies, which §2 deletes. Two things this
found. The checker fills its constructor table in load order where the
tree walk filled it in program order, and a unit names its tags by
position in that table, so taking the checker's order would have made a
unit's C a function of the import graph; the table is rebuilt in program
order from the ordinals. And the narrow register offer reads the checked
scheme now rather than the written type, so an alias for `Int` counts as
scalar where the written form did not, which changes what that offer
holds and not what a body computes. The request path's shipped allocation
figures moved with the stage and were re-taken from the command that
writes them: a `/health` request allocates 328 objects where it allocated
343. The count is reset after one warm request, but a fixed cost outlives
that: read at twenty requests and at two hundred, a request's marginal
cost is about 178 allocations on both sides, and what differs is some
three thousand allocations the window counts once. The offered set is the
same on both, 1066 definitions and every one answered by the port, so
what the service computes per request did not change.

**Built, 2026-09-17: a float literal's text.** The port carries a float
literal as its text, and had no way to the double the reference's lexer
produced: it went through `Decimal`, which cannot hold `1.0e-30` or
`1.0e300`, so the hasher gave up on exactly the literals a shipped test
writes, and it gave up the moment the backend started asking the port for
the whole front end's answer. `float_of_string` is the builtin that reads
one, published as `(String) -> Option<Float>` beside `decimal_of_string`,
answering nothing for any text a literal cannot spell and saturating to
infinity where the lexer saturates. It lands before anything written in
Ply calls it, because the bundle must know a builtin before the
compiler's own sources may use it, and the hasher calls it now.

**Built, 2026-09-17: the frames the switch still needed.** The dump carries
what the driver reads from the tree and the tables did not hold: each
item's visibility and a `fn`'s reuse marker, a definition's parameter
names and their spans, a type's own frame with its arity and span, a
test's name span, each module's effect sets with the atoms they expand
to, and the literals a law's guard mentions, in the order and with the
negation folded as the prover's own walk collects them. A type needed a
frame of its own, because no table of the checker's output holds one: a
sum type reaches it through its constructors and an alias reaches it not
at all. Nothing is rewired onto them yet, and the writer refuses a front
whose syntax tables are missing, so the Rust chain's assembler cannot
answer empty ones where the port answers real ones.

**What the driver loses.** Its gates decided per file not to parse and
per definition not to re-infer, keyed on the store's fingerprints; a run
that enters the port whole parses and checks every module every time.
`ply test`'s result cache, keyed on the hashes the port answers, still
runs only what an edit touched, but `ply check`, `ply hash`, `ply prove`
and `ply build` have no cache and paid whole-program cost only through
the gates. The switch carries the marginal-change bench's reading in its
description, and a regression there is the item after it, as §3 treats
the clock.

## 2. Delete the Rust front end, its test crates and the differentials

In the order the goldens and the crates together allow: `ply-derive`;
`ply-core`; `ply-hash`; `ply-syntax`'s rewrites, resolver, parser, lexer
and printer; then `ply-eval`'s lowering with `opt.rs` and `c/emit.rs`
last; with `ply-derive-tests`, `ply-syntax-tests`, `ply-core-tests`,
`ply-hash-tests`, `ply-compiler-diff` and the `tools/mine-*.py` scripts.
`ply-hash` goes ahead of `ply-syntax`, not after it: its normalizer, its
body store and its graph all read the parser's syntax tree, so a deletion
of `ply-syntax` first is not a deletion that can be carried out. Before each
deletion its differential retires into what survives: `golden::check`
drops its reference arm and holds the port to the golden alone; the
digested goldens are re-blessed as text first, since a digest mismatch
reports nothing once there is no reference to diff against; `fields.rs`
goes with the parser. The lowering and emit differentials retire into the
bootstrap fixpoint and the behavioural gates. Before `c/emit.rs` goes,
this record names the bodies the port still emits differently and why each
is a missing fast path rather than a wrong rule, because a wrong rule the
port applies to itself is a fixpoint.

Two things the survey found do not go with the parser. The tree's
types are the interpreter's value model: a closure holds an expression,
the prover synthesizes expressions at run time for its higher-order
properties, and `Pure` walks them for a schema constant. They move to a
kept crate when the parser goes; the parser, the printer and the
resolver are what is deleted. And two paths rebuild a program from the
store's normalized bodies, print it as source and hand the text to the
port: opening an artifact built without its sources, and `ply test`'s
failure bisection. This record decides the first by removing the case,
an artifact carries its sources, and the second retires with `ply-hash`
into a frame the port answers, the port already holding the bytes it
hashed.

The bundle becomes the only way to build the language. This record says
how a fresh clone builds it — the bundle's C, `cc`, `dlopen`, nothing
parsed — how a broken or unserved bundle is recovered with no Rust emitter
(the last serving bundle from history, and the refresh that iterates to its
fixpoint), and how a helper-table or object-layout change is carried across
it: ADR 0050 §1a's prefix rule where a helper is appended, a bundle
migration by textual transformation where it is not. ADR 0042 step 6 stands
corrected in place by what this finds.

**Read, 2026-09-17: what the four crates actually hold.** A survey of every
non-test reference to them says where the cost is, and it is not where the
names suggest. `ply-derive` is clean: every use outside it is
`expand_program`, `expand_module` or the derivability table, all of it work
the port takes over, and `ply-eval` declares the crate without a single
file naming it. `ply-core` is nearly all vocabulary: nothing outside it
reads `infer`, `env`, `unify`, `scc` or `derivable`, so the checker's
internals have no external reader at all, and most of what is written
`ply_core::` is `ply-ty` wearing the older name. Two real holdings remain
there, `check_program` itself and `ply_core::prelude`, which six surviving
crates read for constructor arities and the prelude effects and which
imports nothing from the parser, so it can move to `ply-ty` whenever it
suits. `ply-hash`'s consumers almost all want `DefHash`, another `ply-ty`
re-export; `ply-store`'s body reconstruction and `ply test`'s bisection
renormalizer are the only readers of hasher internals. `ply-syntax` is the
hard one, and not for its parser: `resolve::Resolved` is threaded through
the public signatures of six crates that survive, which makes it a
front-end product rather than vocabulary or value model, and it is the
reason that crate cannot leave in one move. Nothing in the workspace
depends on `ply-compiler-diff`, so it leaves with its wiring — a nextest
override, two of the four solo rows, `bless.yml`, and the profiler's
default target — and the `tools/mine-*.py` scripts leave with it, run by
nothing in the tree, three of the four writing to a directory that does
not exist.

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

**Read, 2026-09-16: where the time went.** With the message and the table
differentials in, a quiet main run reads 153 s. The build is no longer
the pole: the archive takes 14 s and the release binary 27 s, against a
longest partition of 113 s. Inside that partition, one comparison of the
port's diagnostics over the compiler's own sources takes 48 s and one of
its tables over half the examples 37 s; the same comparison of the tables
over the compiler's own sources takes 43 s in the next partition. The two
most expensive tests in the suite are now the two that hold the port to
the reference, which is what §2 retires, so the bound and the deletions
pull the same way.

**Read, 2026-09-17: the bound, and what crossed it.** The stage's first
shape asked the port for every unit's tables, which is a second front end
per unit: a main run read 254 s against 144 s, every partition grown and
one of them 218 s, with the build reused in 12 s and no test slower for
any other reason. The chain answers by default now and a gate keeps the
port's own path exercised, which is the bound and the proof kept together
rather than traded. That left 190 s, still over, and the next reading
said where: the longest job was 44 s of setup, 69 s of tests and 35 s
saving its object cache, and of those 69 s one comparison of the port's
tables over the compiler's own sources took 51. It runs alone now, as
this tree's three other heavy tests do, so no partition waits on it.

## 4. The loop that is O(the change)

Once the compiler's tests are Ply tests, `ply test`'s content addressing
re-runs only what an edit touched, which ADR 0042 said would fall out of
step six rather than be built. That is true of running the tests and
not of checking the program: the port checks every module every run
where the gates checked what an edit touched. The record takes what a
one-line edit to `emit.ply` costs, locally and in CI, before and after,
from the tool's own output, and the marginal-change bench beside it.

**Read, 2026-09-17: what one edit costs before the switch.** The bench's
three sizes are 250, 1,000 and 4,000 definitions. A warm `ply test`, with
nothing changed, takes 0.01 s, 0.09 s and 0.41 s, of which the front end
is 2.7 ms, 22.5 ms and 92.6 ms; at the largest size that is 46.9 ms of
parsing, 33.9 ms restoring published interfaces and 7.2 ms writing back,
and nothing was rechecked at any size. That figure is the gates' whole
purpose priced: what it costs to establish that nothing changed. An
edit's marginal cost over that warm run is proportional to the project
under the interpreter — a rename 635 ms, 3.3 s and 24.1 s, an edit to a
leaf or a hub much the same — and far smaller under the code generator:
0.2 ms, 2.6 ms and 25 ms for a rename, 4.4 ms, 16.3 ms and 62.6 ms for a
leaf, 53 ms, 18 ms and 210 ms for a hub. The switch replaces the restore
with the port checking and hashing every module every run, so the warm
front-end row is the one to read again beside it.

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
  step 3 said no Rust would read a Ply value, and the port's diagnostics,
  hashes and tables cross as length-framed text that `ply-span` and
  `ply-ty` read into the structs the tools already hold. The converse is
  the live risk: a Ply value holds an expression, so the tree's types
  outlive the parser, and a step that deletes them with it stalls on the
  runtime.
- **If the fixpoint hides a wrong rule.** The fixpoint proves the emitter
  agrees with itself, not with the language; the behavioural gates are the
  witness after the reference goes, and a body they do not reach is
  unwitnessed. The named list of differing bodies, taken before `c/emit.rs`
  goes, is what makes that risk legible rather than absent.
- **If the bundle is lost.** One binary blob in git builds the language.
  History holds every serving bundle, and the refresh iterates; if a
  layout change ever leaves no bundle that serves, the recovery is a
  migration, not a Rust emitter.
