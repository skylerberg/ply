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
rest printed, two hundred and seventy-five bodies where the port lacks a fast
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

**Built, 2026-09-17: the driver asks the port.** A load reads the files,
parses and expands them with the Rust parser, resolves them, and asks the
port once for everything else: the diagnostics it reports, the checker's
output, the hashes, the load order, the ordinals, and the bodies the
store writes. The Rust checker and the Rust hasher no longer run on a
user's program. Both gates went with them, and with the gates the refusal
report, the restored interfaces, the trees a warm process resumed,
`Loaded`'s second program, and the reloads that existed because a gate
might have skipped a file. `ply check --explain` loses the per-file
skipped-and-parsed block and the per-definition cached-and-rechecked
list; it keeps its time breakdown and, with `--types`, the effect sets
and the provenance. The backend builds over the driver's answer rather
than deriving one of its own, so an invocation runs one front end. One
order had to be put back: the port answers definitions in the checker's
own order, which is dependency-first, where a reader of `ply check
--json` is promised the run's files and each file's items as written, so
the driver publishes that order from the ordinals it was given. What
still runs the Rust chain is an artifact opened without its sources,
which rebuilds a program from stored bodies and has no text to hand over;
§2 decides it. `CONTRACTS.md` describes the gates still, and is pinned by
its own header, so that description is historical now.

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
of `ply-syntax` first is not a deletion that can be carried out. The same
holds for the reference emitter, for the same reason and one level down:
`Source::from_front` hands `synthesized`'s `&FnDef`s — the parser's own
function definitions — to `opt.rs` and `c/emit.rs`, and outside those two the
backend reads the parsed program and the resolver's output in exactly one
place of its own, `region_kind::infer`. So the emitter and the optimiser go
*before* `ply-syntax` as well, and the order this record first wrote, which
put them last of all, would have left the parser's trees with a live reader at
the moment it deleted them. Before each
deletion its differential retires into what survives: `golden::check`
drops its reference arm and holds the port to the golden alone; the
digested goldens are re-blessed as text first, since a digest mismatch
reports nothing once there is no reference to diff against; `fields.rs`
goes with the parser. The lowering and emit differentials retire into the
bootstrap fixpoint and the behavioural gates. Before `c/emit.rs` goes,
this record names the bodies the port still emits differently and why each
is a missing fast path rather than a wrong rule, because a wrong rule the
port applies to itself is a fixpoint.

**Read, 2026-09-17: the bodies, and why none of them is a wrong rule.** Over the
compiler's own sources the port emits all 2,687 bodies the reference emits, and
2,412 of them resolve to the reference's C exactly. The 275 that differ are all
in the port's own modules: `infer` 77, `derive` 33, `emit` 33, `exprs` 24,
`hash` 24, `front` 20, `resolve` 17, `rewrite` 17, `items` 12, `patterns` 5,
`types` 5, `diag` 4, `code` 3, `tycore` 1, five of them test bodies. The
differential prints the list by name on every run, which is where it is named
rather than transcribed into this record.

Why each is a missing fast path and not a wrong rule is not an argument made
body by body, and it would be dishonest to present it as one. It is the
argument §2 already states, and these are the sources it applies to: the port
compiles itself. A wrong rule in any of these bodies would be a wrong rule
applied to the compiler that emitted them, so the bootstrap fixpoint would not
converge and the compiler's own tests would not pass on the tier. Both hold on
every run. So a difference here is a difference in how, not in what — the
reference reaching a fast path the port has not got — and the named levers ADR
0051 took are exactly of that shape.

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

**Built, 2026-09-17: a merge takes the pull request's build.** Reuse looks
the build up by the tree it is standing on, and a merge stands on a tree
no run has built whenever main moved under the pull request between its
last run and its merge, which is every second merge of a pair. A
records-only merge paid for exactly that: 370 s wall, 266 s of it
archiving the tests and 165 s linking the binary, for a change no
compiler reads. The lookup now falls back to the second parent's tree
when every file the comparison names is a record, a bench or a workflow,
and refuses a comparison of three hundred files or more, where GitHub
stops listing them and the file that reaches the compiler would be the
one left off the end. The run that merged the fallback read 145 s: both
build legs took their artifacts back, in 21 s and 18 s, and the longest
jobs are four test partitions at 75–80 s. That run hit the lookup
directly, main not having moved under it, so the fallback is built here
and not yet exercised. The merges after it read 126 s, 130 s, 142 s, 168 s, 163 s and 129 s, each reusing by tree the same way and for the same reason. The last is twelve seconds under the bound, which reads as a drift and is not one: over the last nine green runs the longest partition has been 88, 91, 97, 101, 101, 102, 110, 113 and 120 s, a band with no step where §2's 38 MB of text goldens landed. A draft of this sentence called it a trend, from four partitions in one run's longest-five rather than from the series. Seven running have hit the lookup directly, because none of them moved main while
another pull request sat behind it, so the fallback this paragraph describes
is still unproven in the case it was written for.

**Read, 2026-09-17: the switch put the clock over, and the tables are spent.**
The merge that made the port the only front end read 224 s. Both build legs
still reused by tree, so the cost was tests, and the per-test figures named it:
two archive tests at 135 s and 90 s, two incremental sessions at 86 s and 78 s,
out of 1,118 s of `ply-cli-tests` spread over eight partitions. Each of them
drives the real command many times in one session, and since the switch every
invocation pays a whole front-end pass where the gates cost milliseconds.

Moving the two archive tests into the solo table took the next run to 184 s,
still over, and moving the two corpus sessions out as well took it to 149 s,
under, with the longest partition at 114 s and no outlier.

A draft of this paragraph read the first of those runs as proof that the lever
was nearly spent, and predicted a 136 s pole and some 176 s of wall. Both were
wrong, and how they were wrong is the part worth keeping. Those 128 s and 136 s
archive jobs were measuring a **cold per-slot object cache**: a solo row creates
a job identifier that has never run, so its cache is empty and every fixture
program is compiled from scratch once. The same test reads 113.9 s on that first
run and **0.104 s** on the next, its job 15 s. Both jobs ran one test in both
runs, so nothing was lost — the first run was paying for the cache the second
reused. **Read a new solo job's first run as its setup, never as its cost.**

So isolating a heavy test costs one expensive run and is nearly free after it,
and `ci-shards.sh` has a great deal left in it. What remains true from that
draft is the mechanism it started from and not the conclusion: a partition runs
its tests in parallel, the heaviest holding 347 s of tests and finishing in
141 s of wall, where a solo job holds one test and cannot.

That said, the clock is still set by what one `ply` invocation costs, because
every test in this list drives the real command repeatedly. That is §1's front
end, and the two parts do converge on it — by the size of the work rather than
by the shard tables running out.

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

**Read, 2026-09-17: what it cost after the switch, before the front end's three scans were removed.** Two runners agree to within one percent, both at a load under
the gate. A warm `ply test` with nothing changed now takes 0.93 s, 5.2 s
and 39.6 s at the three sizes, against 0.01 s, 0.09 s and 0.41 s before
it; the front end is 923 ms, 5,094 ms and 39,713 ms against 2.7 ms, 22.5
ms and 92.6 ms. Reading, parsing, resolving and writing back are
unchanged and account for 211 ms of the largest figure. The port's own
answer is the rest: 39,502 ms.

Two different things are inside that number and reading them as one would
misplace the work. The gates are gone, so where the reference rechecked
nothing the port checks and hashes every definition on every run. The
in-process rows price that full work for the reference at the same size:
141.8 ms to typecheck and 100.8 ms to hash. So the port is charging about
a hundred and sixty times what the reference charges for the same
checking and hashing, and the deleted gates account for about three of
the four hundred and twenty-seven-fold rise in the warm row. The port's
own speed is the term that matters.

That is why this record holds the switch rather than merging it green.
The commonest act in the loop is asking a project that has not changed
whether it is still good, and the switch makes that act a hundred times
slower at the smallest size and a hundred times slower at the largest.
§4's subject is precisely that loop. The port needs either the gates'
answer — an incremental front end, which is what §4 says falls out of
content addressing — or its own speed, before the reference stops
running. A 39-second no-op is not a front end a person can develop
against, and no CI reading catches it, because CI's programs are the
compiler's own sources and the corpora, not a four-thousand-definition
project asked the same question twice.

**Read, 2026-09-17: the shape of it, at five sizes.** `ply check` over
generated projects of 250, 500, 1,000, 2,000 and 4,000 definitions, two
passes agreeing within half a percent, puts the port's answer at 855 ms,
1,761 ms, 4,804 ms, 12,163 ms and 38,213 ms. Reading, parsing, resolving
and writing back stay linear and stay small: 98 ms of parsing at the
largest size against 38 seconds of answering. The standard library
checked alone as a project is 4,653 ms.

Per definition that is 3.4 ms, 3.5 ms, 4.8 ms, 6.1 ms and 9.6 ms, so the
cost of one definition nearly triples across the range, and doubling the
project multiplies the answer by 2.1, 2.7, 2.5 and 3.1 where linear would
be 2. Two separate faults are in that, and a fix for either alone leaves
the other. There is a constant factor: at the smallest size, where the
superlinear term has barely started, the port already charges about
fifty-five times the reference per definition. And there is a term that
grows with the project, which is what turns fifty-five into a hundred and
sixty by 4,000 definitions. The candidate for the second is the one ADR
0049's profile already named in the emitter, a linear scan standing where
a lookup belongs, and the next reading is a flat profile of the port's
own front end rather than another point on this curve.

**Read, 2026-09-17: the profile is flat, which is the answer.** A CPU
profile of the port's front end over the compiler's own sources finds no
hotspot to remove. Reference counting and allocation lead it — `heap::inc`
at 5.7%, `dismantle` at 4.6%, `raw_alloc` at 3.4%, `dec` at 1.8%,
`offset_by_index` at 1.7% — so about a sixth of the time is spent keeping
objects rather than computing anything. `list::get`, the linear scan the
paragraph above nominated, is 2.8%: present, and not the reason. Nothing
the port itself compiles reaches 2%; its largest body is `infer`'s
`position_bytes` at 1.8%, then `tycore.at` at 1.3% and `resolve.find_sig`
at 1.3%.

This is the shape ADR 0049 found for the emitter, now found again for the
front end, and for the constant factor it says what the fix is: the object
model, by ADR 0051's levers, fewer allocations for the same work, rather
than one hot rule.

**It does not say that about the term that grows, and reading the source
does.** `infer.position_bytes` folds over a whole `List<Bytes>` to return
the index of one name, carrying the answer forward instead of stopping at
it, and the checker's call-graph seeding calls it once for every function
definition and again for every edge out of one, against `names`, which
holds every qualified function name in the program. The list it scans is
itself built by pushing each name after a `contains_bytes` over what is
already there, so it is quadratic before anything reads it. That is the
term the curve shows and the profile hides, because its cost lands in
`list::get`, `heap::inc` and `raw_alloc` rather than in a body of its own.

`hash.ply` holds a fold of the same shape and it is **not** the same
finding: it scans the deduplicated encodings of one cyclic component,
which is small, not a list the size of the program. Saying the two
together, as an earlier draft of this paragraph did, overstates what was
found. The checker's is the quadratic; the hasher's is a scan.

A name index built once is the first thing to try, and it is a small
change rather than a new mechanism: the port already keys maps by `Bytes`
throughout `front.ply`, and `map_new`, `map_insert` and `map_get` are used
in the hundreds across its sources. The same accumulate-then-test idiom
appears elsewhere in `infer.ply`, which is a candidate and not yet a
finding, since nothing has measured those. The bench goes either side.

**Read, 2026-09-17: what the index bought, and what it did not.** The same
five-size curve over the fix, two passes agreeing within a percent. The
port's answer falls at every size and falls by more the larger the project:
807 ms, 1,586 ms, 4,113 ms, 9,459 ms and 27,239 ms, against 855, 1,762,
4,804, 12,163 and 38,213. That is 6% at 250 definitions and 29% at 4,000,
with the standard library alone down a tenth. A saving that grows with the
input is a growth term removed rather than a constant shaved, which is what
the change was for.

The doubling ratios fall at every step — 2.1, 2.7, 2.5 and 3.1 become 2.0,
2.6, 2.3 and 2.9 — and they do not fall to two. Across the range the
exponent moves from about 1.37 to about 1.27. One quadratic is gone and the
curve is still superlinear.

The two candidates this record named are **not** it, and reading them costs
less than measuring them. One folds over a single effect's operations; the
other over a single module's items, so it is quadratic in functions per
module rather than per program. Both are the shape without the size, which
is the same test that kept the hasher's fold out of this. The record says so
rather than leaving a reader to rediscover it.

**The switch stays held on this.** A warm run over a four-thousand-definition
project with nothing changed now spends 27 seconds in the port rather than
38, against 93 ms for the chain it replaces. That is real progress and it is
not the two orders of magnitude this needs.

**And an instrument is retired here.** The differential's warm wall clock
cannot answer a question like this and no reading of it belongs in this
record: four runs gave 32.4 s and 36.5 s for one tree and 37.0 s and 21.5 s
for the other, a 72% spread between two runs of the *same* tree, because a
whole-unit `tcc` compile sits inside the timed region on a contended runner.
Read it for the profile's shares and never for its clock. The five-size
curve is the fit instrument: two passes inside one run agree to half a
percent, and two runs on different runners agreed to one.

**And the port cannot time itself.** The obvious next instrument is the curve
with the port's five stages resolved, since `front` is one number covering
parse, resolve, index, check, and hash with the tables. It cannot be built
inside the port. The language does declare a clock, as a prelude effect whose
`now` is nondeterministic, so reading it inside `front` would put that effect
in the front end's row and carry it up every signature above — contaminating
the purity the differentials and the bootstrap fixpoint rest on, to measure
them. The stage breakdown therefore comes from outside, timing the separate
entries the differentials already call, which needs no change to the port and
no bundle refresh.

**Read, 2026-09-17: there is no guilty stage.** The port's phases timed over
corpora at five sizes, two readings each agreeing within a percent, in
milliseconds: parse and resolve 1,355 / 1,838 / 3,145 / 6,187 / 15,688; with
checking 2,240 / 2,829 / 4,573 / 7,979 / 18,650; the hasher 2,583 / 3,495 /
5,952 / 11,802 / 30,137; the whole front end 3,949 / 5,054 / 8,198 / 14,828 /
35,285. The last doubling multiplies them by 2.54, 2.34, 2.55 and 2.38.

Every stage grows at the same rate, and parsing and resolving alone are 15.7 s
of the front end's 35.3 s at the largest size. A growth term that is equally
present in parsing, in resolving, in checking and in hashing is not a rule
inside any of them, which is the same thing the flat profile said and the
reason the name index bought a fifth rather than the whole gap. The remaining
cost is the object model the port runs on, not an algorithm in the front end,
so ADR 0051's levers are the ones that bear on it.

What separates those two readings is whether the *allocations* grow
superlinearly or only the clock does: the first would put it back in the
port's own code, the second in the runtime beneath it. That is the next
measurement, and the census already exists to take it.

**Read, 2026-09-17: it is the runtime, not the compiler's allocation count.**
The census beside the clock, over parse and resolve at 250, 1,000 and 4,000
definitions: 4.38 M, 8.29 M and 22.16 M objects allocated; 66 MB, 133 MB and
267 MB of chunks; 1,355 ms, 3,145 ms and 15,688 ms. Over the last step the
project quadruples, allocations grow 2.67-fold, chunk bytes grow 2.01-fold,
and the clock grows 4.99-fold. Time per allocation is 0.31 µs, 0.38 µs and
0.71 µs.

The port is not allocating quadratically. An earlier draft of this paragraph
read that as each allocation costing more inside the runtime, and that was
wrong. Time outrunning allocations means work that does not allocate at all,
and the profile at two sizes says what it is: between 250 and 4,000
definitions the heap's own share *falls*, `dismantle`, `raw_alloc` and `dec`
together going from 9.4% to 6.3%, while `resolve.find_sig` climbs from 2.6%
to 8.9% and the list primitives under it climb with it, `list::get` from 2.3%
to 5.4% and `rt_list_at` from 1.6% to 3.1%.

**`resolve.find_sig` is the second quadratic, and it explains the rest of the
shape.** It folds over the whole list of the program's signatures to find one
by name, and the defaults pass calls it once for every call expression in the
program. It sits in the resolver, which is a prefix of all four entries, so a
quadratic there makes every stage superlinear at once — which is exactly why
no stage looked guilty, why parse and resolve alone are 15.7 s of the front
end's 35.3 s, and why the flat profile localised nothing. A signature index
built once is the same fix as the name index, in the phase below it.

Two limits on the census reading, both worth keeping. It is the parse and
resolve entry rather than the whole front end, because the run kept only the
first block at each size. And chunk bytes are a high-water mark, not live
bytes, so they bound the heap rather than describe it.

**Read, 2026-09-17: what the signature index bought.** The same corpora, the
same two instruments, and the port's output byte for byte what it was, so only
the clock moved. Its answer under `ply check`, in milliseconds: 424, 801,
1,973, 3,957 and 9,994, against 807, 1,586, 4,113, 9,459 and 27,239. That is
47% off at 250 definitions, rising to 63% off at 4,000.

The test set before the reading was the resolver's *own* growth rather than the
total, since that is what the change claimed. Its entry falls from 1,355,
1,838, 3,145, 6,187 and 15,688 ms to 503, 636, 954, 1,445 and 2,627, and its
last doubling from 2.54 to 1.82 — below linear, which is what a phase looks
like once the quadratic is gone and a constant dominates it. The whole front
end's ratios fall with it, 1.97, 2.59, 2.30 and 2.88 becoming 1.89, 2.46, 2.01
and 2.53, and the exponent over the range from about 1.27 to about 1.14.

The golden-backed differentials passing is the other half of the result. The
fold this replaced answered with the first matching signature where a map keeps
the last, so a program carrying two of a name would resolve differently; the
resolve golden did not move, so first-writer-wins survived the change.

**Hashing is the worst stage now**, at 2.31 for its last doubling against the
resolver's 1.82, and it is where the same method points next. **The switch
stays held.** A warm run over four thousand definitions with nothing changed
spends 10.0 s in the port where the chain it replaces spends 0.093 s. Two
quadratics have been found and removed, and what remains is a hundredfold gap
rather than a fixed one.

**Read, 2026-09-17: the third scan, in the hasher.** A profile taken at two
project sizes is what found it, after a single-size profile localised nothing
and reading the source guessed wrong twice. Between 250 and 4,000 definitions
the allocator's share *falls*, 11.1% to 8.8%, while three of the hasher's own
bodies appear from nowhere and the list primitives climb with them,
`rt_list_lookup` to 4.1% and `list::get` from 1.6% to 3.5%. `record` searched
the accumulated dependency list and the accumulated closure list for the name
it was recording, and `assemble` calls it once per definition, test and law.

The lists stay, because `dump_hashes` maps over both in order and that order is
the specification; what is added is where each name sits in them, carried in
`assemble`'s own accumulator rather than in the public `HashOutput`. The hash
golden not moving is the proof that neither which entry wins nor where it lands
changed.

The criterion, set before the reading, was the hasher's own growth: its last
doubling falls from 2.31 to 1.77, below linear, and the whole front end's from
2.16 to 1.75.

**Ratios, not clocks, because the machine moved.** The resolver's entry is
untouched by this change and reads about 30% higher at every size than in the
run before it, which makes it an accidental control and means absolute figures
from different runs are not comparable. A uniformly slower machine scales
everything, so the doubling ratios survive it and the wall clocks do not. That
is why the criterion was a ratio.

**Read, 2026-09-17: the front end is proportional now, and the rest is not this
record's to fix.** The marginal-change bench, one machine, one process, both
sides of it: a warm `ply test` with nothing changed takes 0.84 s, 3.56 s and
13.14 s at 250, 1,000 and 4,000 definitions, where before the three scans came
out it took 0.93 s, 5.17 s and 40.0 s, and where the Rust chain with its gates
takes 0.01 s, 0.09 s and 0.41 s. The front end is 824 ms, 3,501 ms and 12,963
ms. This reading is comparable to the earlier one because the bench's
in-process rows time the Rust engine, which nothing here touched, and they read
660 ms against 638 ms: the machine did not move.

The shape is what changed. The front end's cost per four times the project was
5.5 and then 7.7; it is now 4.3 and 3.7. It is proportional to the program, and
the superlinearity was those three scans and nothing else. At 250 definitions
the gain is almost nothing, which is the same fact seen from the other end: a
constant dominates there and always did.

**So part 1 cannot finish inside this record.** What is left between 13.14 s and
0.41 s is a constant factor of about a hundred and forty, and a constant factor
is the object model — which this record says in its own opening that it does not
decide, reserving it for the record after this one. Three quadratics were the
whole of what the front end's own algorithms had to give. The driver switch
stays held, and what would unhold it is now a runtime question rather than a
front-end one: either this record widens to take the heap on, or the switch
waits for the record that does. That is a decision about scope, so it is put
here rather than taken here.

**Read, 2026-09-17: what one edit to `emit.ply` costs, before and after.** Two
edits, because one would answer half the question. The leaf is a string in
`emit.covers()`, which nothing in the tree calls, so it propagates nowhere. The
hub is the refusal message in `emit.expr`, which has forty-nine call sites. Both
are message text, so neither can change a byte of emitted C. Over the compiler's
own 187 tests, on a runner, from the tool's own output:

| | warm, nothing changed | the leaf edit | the hub edit |
| --- | --- | --- | --- |
| the Rust chain and its gates | 343 ms | 1,001 ms | 20,675 ms |
| the port as the front end | 17,197 ms | 17,291 ms | 38,377 ms |
| tests re-run | 0 | 0 | 2 |

**Selection is identical on both sides.** An edit that reaches nothing re-runs
nothing; an edit at the emitter's centre re-runs exactly the two tests that
depend on it. ADR 0042's promise that `ply test` re-runs only what an edit
touched holds, and the switch neither improves nor harms it.

**What the switch moves is the floor, not the slope.** Asking an unchanged
project whether it is still good costs fifty times more. The marginal cost of
an edit that propagates nowhere is seven times *less*, 94 ms against 658, for
the reason the floor is high: everything is checked every run, so one more
changed definition is nearly free. And an edit that propagates everywhere costs
the same either way, 21.2 s against 20.3, because that cost is re-emitting what
depends on it and both sides pay it alike.

So §4's loop is not what the switch is waiting on. The loop works. The floor is
the constant factor §1 has run out of front-end ways to lower, which is the same
conclusion the curve reached from the other direction.

**One half of this is missing and is not going to be taken here.** The record
asked for the cost locally as well as in CI. Running the compiler's own suite is
exactly the heavy local load this machine is not to be given, so these are
runner figures only, and the record says which half it has rather than passing
one off as both.

The lesson for the instrument is worth keeping: a flat profile is evidence
about where time goes, not about whether an algorithm is quadratic, and
this one was taken at a single size over a single program, which is the
reading it could least afford to be.

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
