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
checker. `ply-hash`, `ply-store`, `ply-host` and `ply-test` do not depend on
`ply-core` at all, and the prelude's tables have since joined `ply-ty`, which
takes the runtime and the prover off it too. No production crate calls
`ply-core` any more: what still calls the checker is the test crates, and
nothing else.

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

**Built, 2026-09-17: every code the port can raise, counted, because §1 had only
asserted it.** This section says every front-end diagnostic code reaches the user
from the port, and that codes it cannot raise are deleted. Neither half had been
checked here.

`ply-span`'s registry declares ninety-one codes, each a named constant. The port's
own sources raise forty-seven of them, spelled as byte-string literals at `diag1`,
`diag2` and `report`; `diag.ply` passes a code through and synthesises none, so
that is the whole of what the port can raise. The other forty-four are raised by
the runtime and the tools -- `ply-eval`, `ply-host`, `ply-cli`, `ply-test`,
`ply-codegen`, `ply-ty`, `ply-std`, `ply-prove`, `ply-store` -- and **none** by
`ply-core`, `ply-syntax` or `ply-derive`. So there is no front-end code the port
cannot raise, and the clause about deleting such codes has nothing to act on.
`docs/GUIDE.md` §18 is total over the registry in both directions, ninety-one
against ninety-one: no code missing from the guide, none documented that the
registry no longer declares.

**The method earns a line, because three attempts at it were confidently wrong.**
Searching for the literal `"E0111"` finds only the registry, since a raiser names
the constant and never the string. Searching `crates/*/src/` and taking the first
match answers `ply-span` for every code, since the declaration is a match. And an
earlier sweep in this record used `--include=*.rs` unquoted, which lets the shell
expand the glob before grep sees it and silently stops the filter. What exposed the
second of those was not care but an armed test: `every_registered_code_is_
constructed_in_production` sits in `ci-shards.sh`'s `TREE_CHECKS` and was green
throughout, while the sweep claimed forty-four codes were constructed nowhere. A
cheap grep that contradicts a green armed test is the thing that is wrong. That is
the cheapest instrument in this repository and it is worth reaching for before
trusting a result that arrives without one.

## 2. Delete the Rust front end, its test crates and the differentials

In the order the goldens and the crates together allow: `ply-core`'s
checker; `ply-hash`'s hasher; `ply-derive`; `ply-syntax`'s rewrites,
resolver, parser, lexer and printer; then `ply-eval`'s lowering with `opt.rs` and `c/emit.rs`
last; with `ply-derive-tests`, `ply-syntax-tests`, `ply-core-tests`,
`ply-hash-tests` and `ply-compiler-diff`; the `tools/mine-*.py` scripts stay,
because the corpora they mine are read by crates that survive.
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
the moment it deleted them.

**Read, 2026-09-17: and the emitter is not a reference implementation.** This
record's survey calls `ply-codegen` "a runtime beside a reference emitter
(`c/emit.rs`, `opt.rs`)", which describes a separation the code does not have at
the seam that matters. `c/build.rs` is the tier's own unit builder — it decides
which bodies the tier takes, emits their C, and builds the tables the runtime
reads them against — and it imports `super::emit` directly. So the emitter is not something only the
differential reached; it is what the runtime's own builder calls.

**Corrected, same day: the optimiser is narrower than the sentence above said.**
`crate::opt::optimize` has exactly one call site, in `emit_one`, and it sits
*after* the early return that hands back the port's answer -- so a body the port
answers is never optimised by `opt.rs`. What is on every path is `emit.rs` itself:
`Tables` is the type the port's answer carries, `CTy` types the declared
parameters, `PROLOGUE` goes into every unit's head, and `resolve` reads the tables
back. The conclusion stands; its reason is the types and the unit structure, not
the optimiser.

That makes this deletion a split rather than a removal, and it is the third time
today that planning a step from this record's prose has mis-sized it. The order
above stands: the emitter still goes before `ply-syntax`, because it is what
holds the parser's trees open. What changes is the shape of the work, and a
paragraph written an hour before this one said that shape was deciding which of
the emitter the tier needs and which only the reference needed. Reading
`producer.rs` says otherwise, and this supersedes it.

**There is no reference half.** The path that makes the emitter "the reference"
is a closure in `build_from`, reached when no bundle is present or when a
bundle's helper table is not a prefix of the runtime's. It calls `front_end(src)`
and then the same `super::build` the tier calls. So it is not a second emitter:
it is *this* emitter, reached with the port's own sources instead of a prebuilt
bundle. What made it the reference was that the front end running over those
sources was the Rust chain; since the handover it is the committed emitter, and
only the Rust *emitter* half is left in the path.

So `c/emit.rs` and `opt.rs` are the tier's emitter, and this record's own
exclusion keeps them: the runtime half of `ply-codegen` stays until a C runtime
exists. §2's list cannot delete them and that exclusion cannot hold at once, and
the code decides which survives — they stay. What §2 can retire here is the seed
path, and it retires *with* the Rust front end rather than before it, because
the front end is what it calls.

**§2's remaining work is the four crates, not five items.** The last entry of
its order was describing something that does not exist apart from what the four
deletions already take. Before each
deletion its differential retires into what survives: `golden::check`
drops its reference arm and holds the port to the golden alone; the
digested goldens are re-blessed as text first, since a digest mismatch
reports nothing once there is no reference to diff against; `fields.rs`
goes with the parser. The lowering and emit differentials retire into the
bootstrap fixpoint and the behavioural gates. Before `c/emit.rs` goes,
this record names the bodies the port still emits differently and why each
is a missing fast path rather than a wrong rule, because a wrong rule the
port applies to itself is a fixpoint.

**Built, 2026-09-17: the emit and lowering differentials are retired.** Both are
gone, with the two oracle files that tested the reference dumpers rather than
the port, the solo row that ran the heavy one alone, the nextest override that
kept it off a shared process after it killed a hosted runner twice at 4.2 GB
resident, and the blessing run's two skips that named modules which no longer
exist.

What witnesses the emitter now is what §2 said would: the bootstrap fixpoint,
and six behavioural gates that are each real and green — the compiler's own
tests on the tier and the standard library and examples, all in
`ply-cli-tests`'s `corpus` suite, which also drives `ply test tests/lang` under
each engine; the raising fixtures in `lang_fixtures`; and `examples/same-tests.sh`
with its own job.

**And the census goes with them, which is a real loss and not a tidy one.** The
test that produced the count of differing bodies took the reference's emission,
the reference's lowering and the port's own, and classified the difference by
lowered node tag. Every input to it is a reference dumper this record deletes,
so the report cannot outlive the reference. The 275 named above, on 2,687
bodies with 2,412 identical, is its final reading. After this the fixpoint says
the port agrees with itself and the gates say it agrees with the language, and
nothing says any more which bodies differ from a reference that is no longer
there to differ from.

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

**Built, 2026-09-17: an artifact carries its sources, and three things follow.**
`--sources` was opt-in and off, so the rebuild was what nearly every artifact
took rather than an exceptional branch; removing it takes `ply_syntax::resolve`
and `ply_core::check_program` out of the command crate. Opening now parses the
shipped text, pulls the standard library to a fixed point and expands derives,
which reconstructs the whole program. So: the digest no longer follows the
reachable closure, because an edit to a definition nothing reaches ships its
text and moves it; the disclosure is total, tests, laws and unreached code
travelling as readable source; and the closure no longer decides what a deployed
artifact can *name*, so a schema not given at build time is reachable anyway and
refuses on its missing key rather than on an unreadable source. The last of
those is what a test caught, and it is recorded here rather than absorbed as an
updated expectation.

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
`ply_core::` is `ply-ty` wearing the older name. One real holding remains
there, `check_program` itself. The other was `ply_core::prelude`, which six
surviving crates read for constructor arities and the prelude effects, and it
has since moved to `ply-ty`. The reason given here for why it could was wrong
while its conclusion was right: the prelude did import from the parser —
`ply_syntax::ast::{Mode, ModuleName}` — but both of those are `ply-ty`'s own
types that `ply-syntax` re-exports, so the move rewrote one `use` line and
needed no cycle broken. `ply-hash`'s consumers almost all want `DefHash`, another `ply-ty`
re-export; `ply-store`'s body reconstruction and `ply test`'s bisection
renormalizer are the only readers of hasher internals. `ply-syntax` is the
hard one, and not for its parser: `resolve::Resolved` is threaded through
the public signatures of six crates that survive, which makes it a
front-end product rather than vocabulary or value model, and it is the
reason that crate cannot leave in one move. Nothing in the workspace
depends on `ply-compiler-diff`, so it leaves with what still names it:
`bless.yml` and `profile.yml`. Its nextest override and both of its solo rows
are already gone, with the differentials they protected.

**The `tools/mine-*.py` scripts do not leave with it.** One half of the sentence
above is right and the other is not, and a draft of this paragraph got the wrong
half. **Three** of the four do write into a `tools/fixtures/` that does not
exist — `mine-checks`, `mine-fixtures` and `mine-programs`, each building the
path from `__file__`'s own directory — while `mine-hashes` joins `..` on the way
and lands in the crate's `fixtures/`, which exists. The original count was
correct; the draft that raised it to four was not, and the difference is one
`".."`.

What is wrong is "run by nothing in the tree". They are named as the way to
regenerate those corpora in `crates/ply-compiler`'s `README.md`, `GAPS.md` and
`GAPS-harness.md`, and in the expect messages of four surviving test files —
`ply-codegen-tests`'s `infer.rs` and `resolve.rs`, `ply-compiler-diff`'s
`agreement.rs`, and `ply-syntax-tests`'s `parser.rs`. Nor is it only the mine
scripts: `arm-*.sh` is cited from `BOOTSTRAP-PATH.md` and `GAPS.md`, `arm.sh`
from six places including `ci.yml`, `CONTRIBUTING.md` and source in
`ply-eval-tests` and `ply-prove`, and `measure-multiplier.sh` from three. Only
`diff-items.py` is cited nowhere at all. So the directory outlives the
differential, and the broken paths are a thing to fix rather than to delete.

**Read, 2026-09-17: the parser corpus is stale, and re-mining it is a larger
change than it looks.** Its header still says it came from
`spikes/ply-parser/mine-fixtures.py` reading `crates/ply-syntax/src/tests.rs`,
and neither path has existed for some time — a corpus frozen because nothing
could regenerate it, now that the generator's own path is fixed. Re-mining was
tried here and reverted.

The reason is what it drags. `README.md` and `GAPS-harness.md` each carry a table
of the corpora with six columns — inputs, bytes, dump records, nodes,
diagnostics, and totals — and the mined row reads 716 inputs where the script now
yields 1,028. Editing one cell would leave the other five wrong and the totals
wronger, and taking them honestly means a harness run whose instrument is the
differential this record is retiring. A known-stale artifact is better than a
silently wrong table.

`GAPS.md` reached this before: nothing checks the corpus against
`parser.rs`, `agreement.rs` asserts only `fixtures.len() > 700`, and re-mining
then gave 889 where it now gives 1,028. The diagnosis there is the one to keep —
"a checked-in artifact of a generator with no freshness gate" — and the gate is
what the change wants, alongside the tables, rather than a quiet re-mine.

**Read, 2026-09-17: the blessing path works, and a dispatch is what says so.**
Moving the golden suites broke `bless.yml` — it cleared and re-blessed only the
differential crate's tree, so the five moved phases could not be blessed at all,
and blessing is how a golden moves when the port's answer legitimately changes.
Nothing in CI catches that: the workflow runs by hand. The fix was dispatched
against `main` and its answer is both trees whole —
`ply-compiler-diff/fixtures/goldens` at 6.3 MB over 93 files, `lexer` and
`parser`; `ply-codegen-tests/fixtures/goldens` at 45 MB over 138 files, `derive`,
`infer.check_dump_known`, `infer.check_dump`, `rewrite`, `hash` and `resolve`.
An instrument that only the author ever runs is one to run after moving anything
it reads.

**Built, 2026-09-17: each corpus is written to both crates.** Fixing the three
was the smaller half. `mine-hashes` was the one that resolved correctly, and it
wrote to `ply-compiler-diff/fixtures/` — the crate that no longer reads
`reference-hashes.corpus`, since the `hash` suite moved. So the working script
was wrong in a subtler way than the broken three, and pointing it at the new
reader alone would only move the problem. Each of the four now writes both
copies, derived from the repo root each already computed, because both crates
hold these corpora and a pair kept in step by hand is a pair that drifts.

**Read, 2026-09-17: the checker and the hasher are already off the load path.**
The driver says so itself. It parses and resolves with the Rust chain because
`Program` and `Resolved` are still what the prover, `ply check --costs`, the
artifact path, `Pure` and the interpreter read, and then asks the port for
everything else, so `ply_core::check_program` and `ply_hash::hash_program` are
not called on a user's program from there at all. What the four names hold is
therefore not four deletions but three splits and a crate that cannot leave yet:
`ply-core` keeps nothing of its own — `ty`, `DefInfo` and `Front` were always
`ply-ty`'s and the prelude has joined them, leaving the checker alone;
`ply-hash` keeps `DefHash` and the body envelope while its hash-from-tree half
goes; `ply-derive` expands *inside* `parse_module`, so it leaves with the parser
at the end rather than first, and the order above is corrected to match; and
`ply-syntax` cannot leave while those five consumers read the tree, which this
record's own exclusion defers to the C runtime. This is the fifth time sizing a
step from this record's prose has been wrong, and each time the correction has
been the same shape: a name that reads as a component is a layer two things
share.

What is deletable now is the checker and the hasher themselves, and their callers
are fewer than the survey above suggests. Of twenty-one non-test calls to
`check_program` and thirteen to `hash_program*`, six die with
`ply-compiler-diff`, two are the seed path that retires with the front end, one
is the arm `ply-codegen` takes by default, and six are `ply-corpus`'s cost
harnesses, which check a program because
they need a runnable one to measure and so move to the port's door rather than
being deleted. Three are genuine: opening an artifact, `ply test`'s fresh bodies,
and the hybrid's trial. `ply-store`'s `Bodies::reconstruct` has no non-test
caller at all.

**And the bisection does not print.** A paragraph above says both paths that
rebuild a program from stored bodies print it as source and hand the text to the
port. Opening an artifact did; the bisection does not. `hybrid.rs` reconstructs
the tree, then resolves, hashes and checks it with the Rust chain, which is why
it holds all three crates open at once. Printing is the bridge available to it —
`ply_syntax::print` exists — not what it does today.

**Built, 2026-09-17: opening an artifact asks the port.** `open_sources` took its
`CheckOutput` from `ply_core::check_program` and its hashes and bodies from
`ply_hash::hash_program_with_bodies`, over a tree it had just parsed. It takes
all three from the port's one answer over the same texts instead, and the
name-to-hash body conversion that `build` already carried inline becomes the
helper both use. Sources that are not the artifact's are still refused, and now
it is the port that checked them. Opening a `.plyx` pays a whole front end where
it paid the checker and the hasher before, which is the trade every other command
took at the switch.

**Read, 2026-09-17: parts 2 and 3 meet at the per-unit front end, and that is
the last hold.** A draft of the paragraph above had `ply-codegen`'s
`hash_program_with_bodies` as a `"ref"`-mode fallback beneath a port call, which
is the opposite of what it is, and the shape of that error is worth keeping
because it is the same one four earlier sizings made: a name that reads as an
exceptional branch is the ordinary one. `front_for` asks
the port only when `PLY_FRONT=port` is set, and otherwise answers from the Rust
chain, so that arm is what every caller holding no `Front` of its own takes. The
driver holds one and hands it to `Unit::over_front`; the six callers of
`over_with_texts` do not.

The reason is in `front_for`'s own doc and it is a measurement this record already
carries from the other side: asking the port for every unit's tables took a quiet
main run from 144 s to 254 s, because it is a second front end *per unit*. So the
chain answers while it exists, and exactly one solo CI row —
`compiler-on-the-tier`, running the compiler's own tests as the only engine —
sets `PLY_FRONT=port` and proves the port can answer that path at all.

This is the real gate on deleting the hasher, and it is not a caller that can be
ported away one file at a time. Either the callers come to hold a `Front` the way
the driver does, or the port answers per unit at a cost §3's bound can absorb.
The change below takes the first of those for the one caller that could already
have had it.

**Built, 2026-09-17: opening a `.plyx` asks one front end, not two.** `Opened`
kept the `CheckOutput` out of the port's answer and dropped the rest, so a run
that wanted a backend derived a second front end through `over_with_texts` —
Rust-chain by default — over the program printed back to source. It carries the
whole `Front` now and hands it to `build_backend_over`. The unit's emit cache
keys are this program's rather than absent, which a comment there had conceded,
and the cost the paragraph above added to opening an artifact is paid back in the
same place it was spent. `build_backend` survives for `ply-corpus`'s bench, which
calls it while timing the compile phase and holds only its own chain-derived
check: another of this record's "deletions" that is a split.

**Built, 2026-09-17: `ply test`'s fresh bodies come from the port.** The
bisection re-hashed the whole program to get this run's normalized bytes, which
is half of what the load had already asked the port for and thrown away.
`ply_hash::body::of_front` is the inverse of `ply_codegen::source`'s
`fill_bodies`: that writes each body as `StoredBody::as_bytes`, and
`from_bytes` reads the same envelope back, so `key()` re-derives the hash a body
is filed under rather than being told it. `diagnose_failures` takes the whole
`Front` in place of the `CheckOutput` and `HashOutput` it carries both of, and
that is `ply-test`'s last call to `hash_program_with_bodies`.

The fixtures that drive it keep the bodies their hasher already produced. A
`Front` without them is not a smaller answer but a wrong one: `bodies_available`
reads false, the bisection reports that no mixture could be built, and every
hybrid stops running while the suite stays green. The shared fixture hashed with
`hash_program` and dropped them, so it hashes with `hash_program_with_bodies`
now and answers with a `Front` of its own.

What is left of the checker is the hybrid's trial, which is a hoist rather than
a port: it already prints its reconstructed program back to source and hands the
text to the emitter forty lines below where it runs the Rust chain over the same
tree.

**Built, 2026-09-17: the hybrid's trial asks the port, and the checker has no
genuine caller left.** It was a hoist, not a port: the trial already printed its
reconstructed mixture back to source and handed the text to the emitter, forty
lines below where it ran `resolve`, `hash_program_ast` and `check_program` over
the same tree. The print moves above them, the check and the hashes come from
that one answer, and the answer itself goes to `Unit::over_front` rather than
`over_with_texts` — so the trial stops deriving a second front end as well.
`resolve` stays, because the machine and the unit both read `Resolved` and
`ply-syntax` outlives this record. `ply-test` drops `ply-core` from its manifest
in the same change, which is what "no genuine caller" looks like when it is true
rather than asserted: nothing warns about a dependency a crate has stopped
using, so the claim is only worth as much as the line removed to back it.

Two details decide whether it is correct. The mixture is handed **fresh
sequential** source ids rather than the tree's: a reconstructed module carries
`Span::DUMMY.source`, so the tree's ids are all one id, and the protocol writes a
span's module as its position in the very list handed over, which would fold
every module onto that one. And a mixture reconstructs with a non-empty relink
map, which is exactly the case `reconstruct_with` skips `Reconstruction::verify`
for — so the trial's re-hash was never that self-check, and removing it removes
the last external caller of `hash_program_ast` rather than a verification.

What would have made this silent is worth naming, because it is the shape of
failure this whole section keeps meeting: a port that refused mixtures would send
every trial to `Unresolved::DoesNotCheck`, the bisection would report that
nothing could be mixed, and the suite would stay green. It does not, because the
hybrid suite asserts outcomes rather than the absence of errors — a verdict, the
named culprits, `search.evaluated > 0`, and a logarithmic budget.

**Read, 2026-09-17: what is left of the hasher, counted.** `hash_ast`,
`hash_ast_with_bodies` and `hash_module` have no non-test caller at all and
twenty-five test ones, so three of `ply-hash`'s seven public entries exist for
`ply-hash-tests`, which leaves with the crate. `hash_program` keeps two — the
seed path and `ply-corpus`'s pipeline — and
`hash_program_with_bodies` keeps `ply-codegen`'s default arm and the corpus. So
after this the hasher is held open by the seed path and the corpus harnesses,
both of which §2 already retires, plus the two
structural holds named above: the bisection renormalizer, which re-normalizes per
node against empty tables for an identity no `Front` carries, and `front_for`'s
per-unit default.

**Read, 2026-09-17: `diag` and `front` had no stated retirement, and now they
do.** §2's clause above covers three cases — `golden::check` drops its reference
arm, `fields.rs` goes with the parser, and the lowering and emit differentials
retire into the fixpoint and the behavioural gates. The two widest front-end
differentials are in none of them. `diag` compares every diagnostic a program
raises, `front` the whole front-end answer, and both run over the same seven
corpora: the standard library, every example, the compiler's own sources, the
language fixtures, the parser's fixtures, the mined single modules and the
program bundles. Neither has a golden.

They retire into the behavioural gates, not into goldens, and the cost is why.
Blessing them would add two new phases over those seven corpora, where the front
dump is the widest frame the protocol has — every diagnostic, the load order, the
check output, the hashes, the bodies, the ordinals — against a `resolve` phase
that is already 27 MB across sixteen files. That weight lands in every checkout
of every job, and §3's bound is held by eight partitions running 82–111 s with no
pole left to absorb it. Part 1 says what to do instead: the differentials are the
instrument *one last time*. They run over the corpus, std, examples and the
compiler's own sources, this record says they agreed, and then they go.

**And the goldens outlive the crate that held them.** They were 50 MB under
`crates/ply-compiler-diff/fixtures/goldens/`, beside 69 KB of mined corpora, with
`golden::check` and `port::dump*` modules of that same crate. This record lists
those goldens under what holds the port without the reference, so deleting the
crate around them would have deleted the gate rather than retiring it. They are
split now, as this tree measures them: 44 MB in `ply-codegen-tests` — resolve
27M, hash 11M, rewrite 3.4M, `infer.check_dump` 2.7M and `infer.check_dump_known`
224K, derive 108K — with the five suites that hold the port to a stored answer,
and 6.3 MB left in `ply-compiler-diff` — parser 3.8M, lexer 2.5M — with
`agreement`, `lexer_agreement` and `fields`, which still read the Rust chain and
retire with the parser.

A draft of this change kept `diag.rs` for its corpus helpers, on the reasoning
that the golden suites read the same corpora and would want them. The compiler
said otherwise: with `front.rs` gone all six were dead. Seven of the surviving
suites define their own `first_difference` and eight their own `repo_root`, so
`diag.rs`'s copies had exactly one consumer — the differential that left with it.
The duplication is why this looked shared and was not. So what moves to a
surviving crate is the goldens, `golden::check` and `port`, and nothing else.

**Built, 2026-09-17: the goldens outlive the crate, and are copied rather than
moved.** The five golden suites that read the port alone — `derive`, `hash`,
`infer`, `resolve`, `rewrite` — and 44 MB of their fixtures now live in
`ply-codegen-tests`, which already enters the port in three of its own suites and
carries every dependency they need. `agreement`, `lexer_agreement` and `fields`
stay in `ply-compiler-diff` with the `lexer` and `parser` goldens, 6.3 MB,
because all three read the Rust chain and retire with `ply-syntax`. So that crate
is not deleted by this step; what this step buys is that deleting it later no
longer deletes the gate.

They sit in a binary of their own, `tests/goldens/`, not in that crate's `suite`.
Put beside `suite`'s tests they broke three of them: everything in `suite` shares
one process and runs in parallel, and these enter the port for every program of
every corpus. `suite/main.rs` already names that hazard for `ply_eval::census`
and the allocator; this is the same one, and a *run* found it where a compile
could not — twenty failures, then three once the suites moved out, then the one
`parser_census` failure that main has too.

The harness is **copied**, not moved, and that is a correctness requirement
rather than untidiness. `golden::dir()` resolves against `CARGO_MANIFEST_DIR`, so
a single shared copy could only ever point at one crate's fixtures; each side
needs its own, pointing at its own. The duplicate is seven small items —
`golden`, `port`, `part`, `programs`, `records`, `bundle`, `census`, plus
`strip_one_newline` — and it ends when `ply-compiler-diff` does.

Three things the survey missed, each found by a different instrument, and all
three the same mistake: asking what a name *is* rather than what reaches it.
`records` is called inside `lib.rs` itself, so the helpers are shared between what
moves and what stays rather than belonging to the movers — the compiler found
that. "Self-contained" was checked against `crate::`, the reference dumpers and
the front-end crates, but not against *private siblings in the same file*, so
`bundle`'s call to `strip_one_newline` went unseen, as did three suites reaching
`bundle` and `census::hold` fully qualified rather than through an import — the
compiler found those too. The third only a run could find: the suites read their
**inputs** through a manifest-relative `here()` exactly as the goldens resolve
through a manifest-relative `dir()`, so moving the suites moved what `here()`
means and the corpora stayed behind. One targeted test caught it; no compile
could have.

**And a golden's identity is its absolute path, which is why this moved 32 file
names.** `golden::place` sanitises the whole input path into the file name, so a
golden blessed in CI is named `_home_runner_work_ply_ply_crates_…`. The 32
`rewrite` goldens that come from `here()/fixtures` therefore had to be renamed to
the new crate's segment; the 23 that come from `repo_root()` did not, because
that path is the same from either crate, and the corpus-named goldens of
`derive`, `hash`, `infer` and `resolve` are stable by construction.

The same naming makes these suites **CI-only**: locally the paths are a
developer's checkout, so the goldens are never found and the tests fail wherever
they live. That is true on main today — the three `rewrite` tests fail identically
in `ply-compiler-diff` before this change and in `ply-codegen-tests` after it — so
it is a property of the goldens, not of the move. Worth a note beside the claim
because the first reaction to a red `rewrite` run is to look for a regression that
is not there.

**Read, 2026-09-17: §1 and §2 end where §4 does, and for the same reason.** The
driver enters the port once and takes everything back, and every command goes
through it. What is left of the Rust chain on a live path is one thing:
`front_for`'s default. `ply_codegen::source` answers a unit's tables from
`ply_hash::hash_program_with_bodies` unless `PLY_FRONT=port` is set, and one solo
row sets it. Every remaining caller of the checker and the hasher sits behind
that door — three in `ply-codegen`, five in `ply-compiler-diff` and nine in
`ply-corpus` — so retiring the default is what would take them, and retiring it
means asking the port for a front end per unit.

A draft of that sentence said `ply-corpus` runs in no CI job. It does, and the
correction sharpens the point rather than weakening it. `w3`'s service loader
parses, expands, resolves and checks, and `ply-corpus-tests` reaches it through
`w3::Loaded` and through `constant_memo_service`, which `.config/nextest.toml`
names in its long-test filter and which runs in a partition on every push,
selected by package rather than by name. Those harnesses hand their own tests a
`CheckOutput`, so porting them is the per-unit cost again: every remaining caller
really is behind the one door.

§4 has priced that from the other side, with the instruments this record trusts
rather than a differential's clock. Three quadratics are out, the front end is
proportional, and what stands between 13.14 s and 0.41 s over four thousand
definitions is a constant factor of about a hundred and forty. A constant factor
is the object model, and this record says in its opening that it does not decide
the runtime.

So the parts do not finish independently, and it is worth saying where they
stop rather than leaving it to be rediscovered. §2's deletion of `ply-core`'s
checker and `ply-hash`'s hasher waits on §1's last callers; those wait on the
per-unit cost; and that is ADR 0051's levers and the C runtime — BOOTSTRAP-PATH
step 4, the record after this. What §2 could finish without them is finished:
every differential that read the Rust front end is retired, the goldens that
outlive them have moved to a crate that survives, and what remains —
`agreement`, `lexer_agreement`, `fields` — holds the *parser*, which this record
defers for the same reason it defers the checker's last mile.

**Built, 2026-09-17: the emit oracles were dead, and the bound was not where
this record put it.** Retiring the emit differential left four entries in
`ply-compiler-diff` with no caller — `reference_ctors`, `reference_builtins`,
`reference_emit_encoded`, `reference_emit_dump` — and with them the whole
lowered-form dump machinery they fed: `dump_code` and its eleven helpers, 535
lines, along with the `INLINING` constant the emit differential pinned. None of
it warned, because a `pub` item in a library has no dead-code gate — the same
asymmetry that hid four unused `reference_*` functions from this record once
before. A sweep for callers is what finds them, and a sweep is what this record
should have run when the differential went.

What it costs the chain is the point. `ply-compiler-diff` held five calls to
`check_program` and three to `hash_program`; it now holds neither. The last of
each was `stage.rs`'s, the binary that bootstraps a working copy, which the entry
below takes. Tree-wide the checker had nine non-test callers at that point: the
seed path in `ply-codegen`, `stage.rs`, six in `ply-corpus`, and the value-model
benchmark's arm, which sits outside `crates/` and which every survey behind these
counts was rooted too narrowly to see.

**So the paragraph above overstated the bound, and this corrects it.** It says
every remaining caller sits behind `front_for`'s per-unit default and that the
default cannot move because asking the port for every unit's tables took a quiet
main run from 144 s to 254 s. That measurement was taken when *every* unit build
asked — including the driver's, on every invocation in every CLI test. It no
longer describes this tree. The artifact path went to `Unit::over_front` and the
hybrid's trial with it, so nothing on a user's path enters `front_for` at all:
its Rust arm is reached from five places, and all five are `ply-corpus`. What is
left is therefore not one immovable cost but three ordinary pieces of work — the
corpus harnesses onto the port's door, and the two bootstrap paths that §2
already means to retire when the bundle becomes the only way to build the
language. The object model bounds how *fast* the port answers, not whether these
callers can stop asking the chain.

**Built, 2026-09-17: five of the corpus's six harnesses take their check from
the port.** `w3`, `w4`, `w5`, `serve` and `measure` each needed a checked program
in order to measure what it *does* at run time, and none of them cared who
checked it. They ask the port now, over the same texts and in the program's
module order — which matters, because the protocol writes a span's module as its
position in the list handed over, and `w3`'s existing `texts` is a map for
`Unit::over_with_texts` rather than an ordered list.

`pipeline.rs` is the sixth and keeps the Rust chain. Its call sits inside
`Phase::Typecheck`: what it reports *is* the chain's own per-phase breakdown, so
it retires with its subject rather than moving to the port. That is the rule the
differentials retired under, and it applies here unchanged.

So `check_program` falls from nine non-test callers to four, and none of the
four is a program a user runs: the seed path in `ply-codegen`, `stage.rs` in
`ply-compiler-diff`, `pipeline.rs`, and the value-model benchmark's arm. Two of them are the bootstrap paths §2
already means to retire when the bundle becomes the only way to build the
language, and the third retires with the chain it times.

**What this does not do** is touch `front_for`'s Rust arm. That is reached
through `over_with_texts`, whose five remaining callers are all in this same
crate: those harnesses build a *unit*, and to stop deriving a front end they
would have to hold a `Front` the way the driver and the artifact path now do.
That is the change after this one, and it is what stands between the hasher and
its deletion.

**Built, 2026-09-17: the corpus holds the port's answer rather than asking
twice.** The change before this had five harnesses ask the port for a check and
then let `over_with_texts` derive a *second* front end for the unit. They keep the
whole `Front` now — `w3`, `w4`, `w5`, `serve`, `measure` and `pipeline` — and
build the tier from it through `Unit::over_front`, so one ask serves both. Six
sites in all, `bench`'s default tier included; `ply-corpus` no longer calls
`over_with_texts` anywhere.

`pipeline` holds **both** answers on purpose. Its `check` and `hashes` stay the
Rust chain's, wrapped in `Phase::Typecheck` and `Phase::Hash`, because timing
those phases is the whole of what that harness reports; the port's answer sits
beside them, asked outside every timed region, and is only what the tier is built
from. Two answers to two questions, and only one of them is a measurement.

**What this does not do, against what an earlier paragraph implied.** That
paragraph said `front_for`'s Rust arm is reached from five places and all five
are `ply-corpus`, so the corpus moving would leave it callerless. That was true
only of the non-test callers. `over_with_texts` has nine more in test crates —
four in `ply-test-tests`, three in `ply-codegen-tests`, one each in
`ply-hash-tests` and `ply-eval-tests` — and `ply-cli`'s `build_backend` keeps a
caller in `ply-cli-tests` beside the corpus bench's. So the Rust arm keeps a
constituency, the hasher is not unblocked by this, and what this buys is one
fewer front end derived per harness rather than a door closed.

**Read, 2026-09-17: the three things §2 promised about the bundle, said.** The
paragraph above says this record *says* how a fresh clone builds the language,
how a broken or unserved bundle is recovered with no Rust emitter, and how a
helper-table or layout change is carried across. It had not said any of them.

**Two artifacts, not one, and conflating them is the easy mistake.**
`crates/ply-compiler/bootstrap/` holds `unit.c.gz` and `SOURCES.digest` — the
emitter's own unit, 2.3 MB compressed — and that is what builds the *emitter*
without a Rust emitter. `ply bootstrap --out <dir>` writes `frontend.c` and
`manifest.json` — the *front end* emitted as standalone C — and that is what
answers "what does a fresh clone read the language with" for someone who does not
have this repository's Rust driver. Its own module doc argues the case: keeping
the Rust front end is not dropping it, and committing a built object is a binary
per platform per version in the history, while C is text, it diffs, it needs only
a C compiler, and git already archives every version of it.

**The emitter, from the bundle.** `bundle::build` unpacks the gzip and calls
`load_unit(text, None, "bootstrap")`, which compiles and loads the unit against
the tables it carries rather than against any program's: the runtime helper
table, the constructor table its tags are positions in, its functions and their
arities all travel inside the C, so **no source is parsed** and the modules are
numbered from zero. Underneath, `compile_and_load` writes `unit.c`, runs the C
compiler with `-fPIC -shared -fno-strict-aliasing`, and moves the object into a
cache keyed on the source by a rename — which is what makes two workers
compiling the same unit at once safe, since both write the same bytes.

**The archive is gated, not aspirational.** Two solo rows run it on every push:
one writes an archive and checks `--verify` accepts it, the other adds a
definition and checks `--verify` refuses. Two digests, because they answer
different questions — the source digest is over every definition's content hash
and names *which* front end this is; the artifact digest is over the C and says
whether this file is the one that version produces.

**Recovery, and where it stops.** A unit binds helpers by position and reads no
further, so it serves any runtime whose table *starts with* its own: an appended
helper leaves every older bundle serving, and history plus the fixpoint's refresh
is a real recovery. ADR 0050 §1a records the precedent — `list_set`'s refresh was
the first to start from a bundle emitted against an older table. A helper
*changed or removed* is the other case, and `Exports::unserved` names the first
position that differs. There the recovery is a textual migration of the bundle,
and that migration does not exist yet: today `build_from` falls back to
`from_committed()`, which stands where the Rust chain stood — ADR 0050 §1a chose
that fallback deliberately, "the producer and the fixpoint test build the emitter
with the reference on it". So the seed path is load-bearing for exactly this
case, and §2 cannot delete it before the migration is built. So the order §2
gives — the bundle becomes the only way, then the Rust emitter goes — has a step
inside it that this record had not named: the migration comes first, or the seed
path stays.

**Read, 2026-09-17: three comments in the port name oracles that are gone, and
fixing them costs more than they are worth today.** `emit.ply:4` and `code.ply:8`
each say "the oracle is `reference_…_dump` in `harness/src/lib.rs`", and
`code.ply:101` names `reference_lower_dump` without the path. Both functions were
deleted by this record — the lowering dumper with the effect census, the emit
dumper with the dead oracles — and `harness/src/lib.rs` has not existed for longer
than either. What holds those files now is what §2 retired their differentials
into: the goldens for `code.ply`'s dump, the bootstrap fixpoint and the
behavioural gates for `emit.ply`.

The fix is one line each and it is not being made here, which is a judgement
rather than an oversight. Editing any byte of `crates/ply-compiler/ply/*.ply`
moves `SOURCES.digest`, and the bundle is then the old emitter's: the first run
fails its bootstrap job, uploads a `bootstrap-bundle` artifact, and the landing
takes a second commit. That is two CI rounds and a new object in history, and the
object is measured rather than guessed — the bundle has been refreshed eighteen
times, every refresh a distinct blob because a gzip does not dedupe, 38.2 MB in
total at 2.12 MB each. Three comment lines do not buy that. They go with the next
emitter change that refreshes the bundle anyway, and until then this paragraph is
where a reader learns the comments are wrong.

**Built, 2026-09-17: the staging binary asks the port, and the checker is down
to three callers.** `stage.rs` bootstraps a working copy of the emitter, and it was
never blocked — only unexamined. It already installs the emitter at the top with
`producer::ensure_default()`, already holds its modules as `(name, text)` pairs in
exactly the shape `producer::front` takes, and its `SourceMap` hands out ids
`0..n` in that same order, which is the order the protocol reads a span's module
as a position in. What it did with all that was `check_program` → `hash_program`
→ `front_of`: a second front end assembled from the chain, for an answer the port
gives in one call.

It asks the port now. The parse, the expansion and the resolve stay, because
`Source::from_front` still takes the syntax tree and the resolution beside the
`Front` — that is the object model §4 prices, not a front end running twice for
its answers. Nothing else in the binary read the `CheckOutput`, so the three calls
came out whole.

That leaves `check_program` with three non-test callers and `hash_program` with
two. The seed path in `ply-codegen` and `pipeline.rs` in `ply-corpus` call both;
`benches/value-model/ply-arm`, the value-model benchmark's Ply arm, calls the
checker alone. None of the three is a program a user runs. The seed path is
the one §2 cannot delete before the bundle migration exists, for the reason given
above — `build_from` falls back to `from_reference()` when a unit is unserved, and
ADR 0050 §1a chose that deliberately.

`pipeline.rs` was deliberately not moved onto the port with the others, and the
reason is worth stating because it would otherwise read as an oversight. Its `check_program` sits in
`Phase::Typecheck` and its `hash_program` in `Phase::Hash`, and those rows are
what the marginal-change bench sums into the front-end figures §4 cites. Pointing
them at the port would move numbers this record depends on without saying so: it
would report the port's cost under the chain's name. It retires with the chain it
times, which is the rule the differentials retired under.

**Built, 2026-09-17: the prelude moves to `ply-ty`, and the runtime halves come
off the checker's crate.** §2 deletes `ply-core`, and the crate was held for two
unrelated reasons: two calls to `check_program`, and the prelude tables — the ADT
list, the effect names, the constructor arities — which have nothing to do with
checking. The second was the larger hold. `ply-eval` and `ply-prove` read
`ply_core` for the prelude and for nothing else, so a table of constructor
arities was what kept the runtime and the prover depending on the crate this
record means to delete.

The move cost one `use` line. `prelude.rs`'s only `crate::` reaches were
`crate::ty` and `crate::{CtorInfo, EffectInfo, OpInfo}`, every one of them
`ply-ty`'s own, so inside `ply-ty` those paths resolve unchanged. Its single
outward reach was `ply_syntax::ast::{Mode, ModuleName}` — and `ModuleName` is
defined in `ply-ty`'s `decl.rs` and `Mode` in its `ty.rs`, so the parser crate was
re-exporting `ply-ty`'s own types back at it. There was no cycle to break, only an
import pointing the long way round. `prelude_arity` stays with the checker,
because it delegates to `infer::prelude_arity`, which is the checker.

**What came off, stated as two different things.** `ply-eval` and `ply-prove` no
longer declare `ply-core`; `ply-hash`, `ply-store`, `ply-host` and `ply-test`
never did. Their *test* crates still do — `ply-eval-tests` and `ply-prove-tests`
call `check_program` and `check_module` at ten sites between them to check their
fixtures — so the edge is gone from the runtime halves and alive as a
dev-dependency. Those are different claims and this says both, because "`ply-eval`
no longer depends on `ply-core`" on its own would be false of the package.

The readers in the test crates were deliberately left pointing at
`ply_core::prelude`: the crate's `pub use ply_ty::*` re-exports the moved module,
so they keep resolving, and re-pointing them would churn more crates to prove a
point §2 will settle by deleting the name outright. `ply-codegen` is no longer
among them -- the handover below took its last reach off the crate -- and
`ply-cli` now names `ply_core` only in a comment. That the
glob carries a `pub mod`, and that `crate::prelude` still resolves from a sibling
module through a root-level `use`, were both things this record believed; a
throwaway two-crate workspace settled them before the change was written, and the
nine-package check confirmed it against the tree.

**Built, 2026-09-17: the prover comes off the hasher's crate.** `ply-hash`
re-exports `DefHash` from `ply-ty` by name, and the prover reads nothing else in
it: three sites in the library and five in its test crate, every one of them
`use ply_hash::DefHash;`. Both packages held a dependency on the hasher for a
type that lives somewhere else, and both drop it.

Only the prover was in that shape, which is why this goes no further. `ply-store`
reads `body::BODY_ENCODING`, `ply test`'s bisection reads `graph::`, `normalize::`
and `component_hashes`, and `ply-codegen` and `ply-corpus` call `hash_program*`.
Sixty-eight type-only sites remain across those crates, and re-pointing them would
churn five crates to drop no edge at all; they keep resolving through the same
named re-export. That is the judgement this record already made for
`ply_core::prelude`, applied again: the older name goes when the crate does, not
before.

Stated as the two claims it is, because either alone would mislead: the prover's
**library** now depends on neither the checker's crate nor the hasher's, and
`ply-prove-tests` still declares `ply-core` -- it calls `check_program` at two
sites to check its fixtures -- while no longer declaring `ply-hash`.

**And a note on part 3's instrument, after the third platform-side failure.**
#348's first run went red with `cargo build --release -p ply-cli` failing, and the
aggregate job reported `build-ply = failure` and nothing else, which reads exactly
like a compile error in the change under review. The failing job's own log showed
`mozilla-actions/sccache-action` returning HTTP 500 during setup, before cargo ran
at all; a re-run of the same head was green with no edit to the tree. With the
`EHOSTUNREACH` cache download and the `getaddrinfo EAI_AGAIN` retries recorded
above, that is three, which is enough to stop treating each as a surprise. The
procedure is cheap and worth naming: read the failing job's log before the
aggregate's summary, and before touching the tree at all. The aggregate names
which leg failed, never why, and "why" is the whole question when the answer might
be that nothing is wrong.

**Built, 2026-09-17: the CLI comes off the checker's crate, and a caller every
survey here had missed.** `ply-cli` never checks a program. The only
`check_program` string in the crate is `driver.rs`'s own doc line, which exists to
record that the checker is *not* called on a command path. Everything else written
`ply_core::` there was a name that lives in `ply-ty` -- `CheckOutput`, `DefInfo`,
`Front`, `ModuleInfo`, `TestInfo`, `CtorInfo`, `LawBinder`, `Footprint`, `Type`,
`Ordinal`, `print::Printer`, `print_scheme`, `print_type`, and since #348
`prelude`. Forty-eight uses are re-pointed and the manifest trades `ply-core` for
`ply-ty`. Every distinct head was checked against `ply-ty`'s exports first; the one
that came back missing was `check_program`, the doc line, which is kept exactly as
it was, because a sentence recording what is not called must keep naming the thing
that is not called.

Two claims, as before, because either alone would mislead: the CLI's **library** no
longer links the checker's crate, and `ply-cli-tests` still declares it, calling
`check_module` at two sites to typecheck its fixtures.

**And the caller none of this record's surveys could see.**
`benches/value-model/ply-arm` is a workspace member outside `crates/`, and it has
called `ply_core::check_program` since `6aca8591` (2026-09-06). Every survey behind
the caller counts in this record was rooted at `crates/`, so every one of those
counts was short by one from the day it was written -- not stale, wrong when
published. The sentences above are corrected in place rather than annotated, and
a repo-rooted sweep is what every figure here now rests on. Where they ended is
the terminal position below: **no** non-test caller of the checker, the handover
having taken the last one, and **two** of the hasher, in `ply-codegen` and
`ply-corpus`.

The defect was the method, and its general form is bounded: `cargo metadata`
reports thirty-two members and exactly one outside `crates/`, so this is a single
hole and not a systemic one. It is recorded because the shape recurs -- a search
root chosen once, then trusted by everything downstream, is invisible precisely
where it is wrong.

**What the arm is, and why it is not what blocks the deletion.** It loads a
project, parses, resolves and checks in order to build a `Source`, and then
compiles that through `ply_codegen::c::build` -- the tier's own emitter -- and
times the kernel entry. The front end is setup; the gate reads the call. So it
retires with the chain, exactly as `pipeline.rs` does, and moving it to the port
would remove a caller without unblocking anything: what stands between §2 and the
checker's deletion is the seed path, and behind it the bundle migration that does
not exist yet.

**One thing worth arming, stated as unarmed.** `.github/ci-shards.sh`'s `members()`
reads the workspace list with a `"crates/NAME"` pattern, so it extracts thirty-one
of the thirty-two members and has never seen `ply-arm`. That file's own comment
says a crate in neither `KNOWN_OUTSIDE` nor `members` is an accident, "nothing
builds it and no job tests it" -- and the second half does not follow here, because
`cargo clippy --locked --workspace --all-targets` compiles the arm on every push.
So the coverage is real and the accounting is narrower than it reads.

**Corrected after reading `cmd_verify`, because the sentence above framed this as
an oversight with a one-line fix.** The `"crates/NAME"` pattern is deliberate and
load-bearing: four checks downstream of `members()` are all `crates/`-shaped -- the
`-tests` loop reading `crates/$member/src` and its `[profile.dev.package]`
override, the accident sweep walking `crates/*/`, the `KNOWN_OUTSIDE` validation,
and `check_test_exists`. Widening the pattern would feed `ply-arm` into loops that
then demand a `crates/ply-arm/src` and a profile override that must not exist,
turning a narrow gap into a failing guard. The actual defect is two smaller things:
the summary line reports its count as "workspace members" when it means members
under `crates/`, and nothing asserts that a member outside `crates/` is covered by
any job. The fix is a separate assertion and honest wording, not a wider regex. Per
§"Do not state a guarantee you have not armed": **not enforced.**

**Built, 2026-09-17: the differential crate's dead edges, and the arm scripts
that lost their tests.** Two unrelated pieces of staleness in one crate, both left
by earlier merges in this record.

`ply-compiler-diff` declared `ply-core` and `ply-hash` and used neither. The
reference dumpers went with the dead-oracle sweep and `stage.rs` stopped calling
`check_program` and `hash_program` when it moved onto the port. Both edges are
dropped. The check behind that claim is worth naming, because the first attempt at
it lied: `grep --include=*.rs` written unquoted lets the shell expand the glob
before grep sees it, and the filter silently stops filtering -- the output arrived
full of `.sh` files and would have read as proof the crate was clean. Quoted, over
the whole crate including its tests and the `refdump` bin, the count is zero.

The same promotion left a third thing behind. `ca0a0ccf` moved this crate into the
workspace, which made its own `Cargo.lock` inert -- cargo resolves a member from
the root lockfile -- and the file stayed, still naming the two dependencies dropped
here. It is deleted. The two other nested lockfiles, under `benches/value-model/`,
are deliberately left: those crates sit outside the workspace, so theirs are the
lockfiles cargo actually reads.

The five arm scripts are the second. `arm-derive`, `arm-resolve`, `arm-rewrite`,
`arm-hash` and `arm-infer` are mutation harnesses: each applies a semantic mutation
to a `.ply` source, rebuilds through `stage`, and asserts the suite goes red. They
are the instrument that makes §2's retirement sound, because a golden that cannot
go red retires nothing -- "the signature defect here is a green result over
unexplored space", as `arm-harness.sh`'s own header puts it. Their tests moved to
`ply-codegen-tests/tests/goldens/` in `d761a80c`, two days after `eab318a7` last
touched them, and every one still ran `-p ply-compiler-diff --test suite` and still
named a header path under `crates/ply-compiler-diff/tests/suite/`. They have been
broken since, loudly rather than silently -- the "green before any mutation" guard
exits 1 -- and no CI job runs them, so nobody met the failure. They now point at
the package that holds the goldens. `arm-harness.sh` is deliberately left alone: it
filters `agreement::`, and `agreement.rs` is still in this crate.

**Read, 2026-09-17: where the 166 s went, and the explanation that did not survive
checking.** That run is the highest reading since the 173 s the paragraph above
anchors on, against a recent band of 138 to 153. It was not the tree. The longest
partition was 112 s against the previous run's 110 s, inside the 88-to-120 band
recorded above; the partitions simply started 18 s later, at 47 s rather than 29 s,
and the wall grew 19 s. All of it sat in one step: the archive reuse, 6 s in the
run before and 27 s in this one, with the partitions waiting on it.

The obvious explanation was that both merges edited `Cargo.toml` files, changing
the dependency graph and invalidating the cache the archive restores from. That is
wrong, and the step's own log says so. Both runs took the identical successful
path -- `artifact-for-this-tree.sh` found the pull request's archive for the same
tree, `gh run download` fetched it, both printed "came back from run" -- and
neither rebuilt or missed a key. The whole difference is one download of
`nextest.tar.zst` taking 27 s instead of 6 s. It is the artifact transfer, which is
platform-side, and it is the fourth such observation here. The alternative is kept
because it is the one a reader would reach for next, and the reason it fails is the
evidence: the reuse succeeded in both runs.

**Read, 2026-09-17: where §2 ends, and why it is not the bundle migration.**
This record has said, more than once, that the seed path cannot go before a
textual migration of an unserved bundle exists. That is true and it is not the
binding constraint, which matters because the two imply different work.

`build_from` falls back to `from_committed()` in two cases: no bundle for these
sources, and a bundle whose helper table this runtime's does not start with --
`Exports::unserved` walks the unit's helpers by position and names the first one
that differs in name, arity or whether it answers. And
the seed path was never one thing. It was `front_end(src)`
*followed by* `super::build`, which is `c/build.rs`, which imports
`super::emit::{Emit, Unit, mangle}` and calls `crate::opt::optimize` -- and only
the first half was `ply-core`'s. The second needs the Rust **emitter**.

`crates/ply-codegen/src/c/emit.rs` and `crates/ply-codegen/src/opt.rs` are what
this record's closing exclusion keeps until a C runtime exists. This record
argued from that to "`ply-core` cannot leave while the seed path stands", and the
inference was wrong because it read the seed path as one thing. The handover
below replaced `from_reference` with `from_committed`, which takes the front end
from the committed emitter and leaves `super::build` where it was: so `ply-core`
left while the seed path still stands, built by the very emitter the exclusion
keeps. Building the bundle migration would not change that either: the migrated
bundle would still be built by that emitter. The migration is worth building for
what §2 promised a reader -- how a broken bundle is recovered -- and not as a way
to reach a deletion.

**So the terminal position, stated once.** The checker has no non-test callers
left. The seed path was the last and the handover took it; `pipeline.rs` and
`benches/value-model/ply-arm` read `ply-ty` directly now. Both still time the
chain that §4's front-end figures are summed from, so they retire *with* their
subject rather than standing before it -- pointing them at the port would report
the port's cost under the chain's name -- but neither holds the checker's crate
any more. What holds `ply-core` is the test crates that check their fixtures
through it: every one calls `check_program` or `check_module`, and nothing else
in the workspace does. They are being moved onto the port one crate at a time,
which the entry below takes.

`ply-syntax` and `ply-derive` are held by a different thing again: `Program` and
`Resolved` stand in the public signatures of the crates that survive, which is the
object model §4 prices at a residual constant factor and which this record
explicitly does not decide. And `ply-eval` splits where the goal says it does --
the lowering is `code.rs`, while the value, builtin, host, region and simulation
layer around it is what `ply-codegen` reads as `Value`, `Compiled`, `Builtin`,
`HostRuntime`, `HostBinding`, `region::Record` and `sim::TASK_OPS`, and stays.

What §2 therefore finished is the part that was reachable, stated without
flattering it: every package that held a front-end crate *only* for vocabulary has
dropped the dependency. `ply-eval`, `ply-prove` and `ply-cli` read `ply-ty`
directly; `ply-compiler-diff` declared two crates it had stopped using at all.
`ply-codegen` and `ply-corpus` kept theirs for work -- the seed path and the
marginal-change bench -- and both have since dropped it: the handover took
`ply-codegen`'s last reach off the crate, and `ply-corpus` reads `ply-ty`
directly. What still spells vocabulary through an older name is the test crates,
left deliberately: re-pointing them would drop no edge while they call the
checker regardless, and this record has declined that same trade twice already --
for `ply_core::prelude`'s other readers, and for the sixty-eight type-only
`ply_hash` sites across five crates. The older name goes when the crate does.

**What §2 moved, measured.** `cargo metadata` over the workspace: the checker's
crate fell from seven packages declaring it as a dependency to none, with its
eleven dev-dependents unchanged, because the test crates still check their
fixtures and that is not vocabulary. The hasher's fell from seven to five, and its
dev-dependents from six to five. `ply-syntax` and `ply-derive` did not move at
all, at twelve and six, which is the paragraph above restated as a number: nothing
here touched the object model.

What remains is held by three things this record names and defers: the emitter, the
object model, and the harnesses that time the chain.

**Built, 2026-09-17: the test crates begin asking the port, and one cannot.** A
migrated crate replaces `check_program` or `check_module` with `producer::front`
over the fixture's own source, and asserts the answer carries no diagnostics --
the job the old `.expect("... typechecks")` did, since `front` answers `Ok` with
diagnostics rather than erroring. `ply-cli-tests` and `ply-host-tests` went
first.

Two rules travel with it, and the first is the one that matters: **hand `front`
the module name the site already gave the parser.** `ply_syntax::parse` names a
module `ModuleName::anonymous()`, which is `Symbol::new("")`, and an anonymous
module qualifies nothing -- `ModuleName::qualify` returns the bare name, and
`resolve.ply`'s `qualify` returns it for a zero-length module, the two
implementations agreeing independently. Pass that name through and no key moves;
invent one and every key gains it. `ply-cli-tests` was migrated with an invented
`"m"`, so `config` became `m.config` and `db.get[orders]` became
`m.db.get[orders]` in the host listing: churn that was avoidable rather than
inherent. `ply-prove-tests` then failed across four files for the same reason,
and passed with every assertion untouched once the anonymous name was preserved.

The second rule is that where a site names its module deliberately, the name is
load-bearing: `tcp.rs` must pass `ply_host::tcp::MODULE`, because `std.net` is
what qualifies its `EFFECT` as `std.net.net`. An effect a fixture does not
declare keeps its bare name either way -- `clock` comes from the prelude and
`task` from the simulation, not from the source.

The cost is a dependency rather than a deletion for the crates that lacked
`ply-codegen`: they trade the checker for the code generator, which survives
this record. Where `ply-ty` was already present nothing re-points, because it
owns `CheckOutput`, `DefInfo` and `ty` outright and `ply-core` only re-exported
them.

**`ply-store-tests` cannot go, and it is one of two.** Its second
site typechecks a program *reconstructed from the store*: `reconstruct` hands
back a syntax tree and there is no source text anywhere, which is the crate's
whole thesis -- what a run writes comes back out as a program that checks,
without the source. The port's interface is source bytes in, so `front` cannot
answer it, and migrating the crate's other site alone would leave the dependency
in place, which is why the attempt was withdrawn rather than halved. Every other
remaining site parses from source first. The corpus harnesses in
`ply-eval-tests` looked like a second case and are not: they reach their
programs through a `load` that reads files, so the text exists and the loader
need only hand it back. This one has no text to hand.

**`ply-codegen-tests` cannot go either, for an unrelated reason.** Seven of its
nine `check_program` sites migrate like any other. The two that do not are
`bootstrap.rs`'s `emitter_source` and the loader in `tests/producer.rs`, whose
tests install their own recipe through `producer::install` -- a `OnceLock`, where
the first installation wins and a second is ignored -- and whose check runs
*before* that install. Adding `ensure_default` there would claim the lock first
and leave the fixpoint test exercising an emitter it did not choose; omitting it
leaves `front` with no emitter to ask. Either way the test stops testing what it
says, which is why seven of nine is not a migration: the dependency stays
standing.

This one has a path, unlike the store's. `with_producer` is scoped by `Drop`
rather than by a `OnceLock` -- that is what let the handover stand one emitter up
inside another -- and the same mechanism could replace `install` in those two
harnesses. That is a restructure of the fixpoint test rather than a migration, so
it is named here instead of attempted alongside the others.

**Built, 2026-09-17: `verify` sees the member it could not see, and the count
says what it counts.** This record described the gap twice and fixed it neither
time, which is the ritual `CLAUDE.md` warns about standing in for the rigour.

`members()` reads the workspace list with a `"crates/NAME"` pattern and returns
thirty-one of the thirty-two members; the one it never saw is
`benches/value-model/ply-arm`. The pattern stays, because four checks below it are
`crates/`-shaped -- the `-tests` loop reading `crates/$member/src` and its
`[profile.dev.package]` override, the accident sweep walking `crates/*/`, the
`KNOWN_OUTSIDE` validation, and `check_test_exists`. Widening it would feed a
member from elsewhere into loops that then demand a `src/` and an override which
must not exist, turning a narrow gap into a failing guard. So this adds a second
reader over the same block and leaves the first alone.

**What it asserts is the coverage that actually exists.** No partition names a
member outside `crates/`, and nothing below `members()` is shaped for one. What
keeps the arm compiling is the workspace-wide lint leg -- which the root manifest
already argues for in its own comment: the arm is a member rather than a workspace
of its own so that `cargo clippy --workspace --all-targets` and `cargo fmt --all`
keep it building when `ply-codegen`'s surface moves, and "outside, it would rot
until somebody ran the gate by hand". The workflow header then records that `fmt`
and `clippy` deliberately do *not* gate the expensive jobs and that the `ci`
aggregate is where lint is required. So that one `needs` entry is the whole of the
coverage for a member no partition mentions, and nothing checked it was still
there.

**The guard was watched to go red**, which is what this file asks of its own
checks -- the probe check records being watched to fail on 2026-08-30 by naming a
wrong directory. Against the workflow as it stands the predicate finds
`--workspace --all-targets` and passes; against a copy with that string removed it
fails. A check that cannot fail proves nothing, and the version of this fix that
merely counted members would have been exactly that.

And the summary line now reports members under `crates/` plus however many sit
outside, rather than calling the first number "workspace members". That was the
smaller half of the defect and the one a reader would have believed.

**Built, 2026-09-17: the census §2 asks for, and what it turns out to be.** §2
says that before `c/emit.rs` goes, this record names the bodies the port still
emits differently and why each is a missing fast path rather than a wrong rule.
That clause was written while the emit differential ran both emitters over the
same bodies and compared their C. The differential is retired, and its premise
with it: on a build the port serves, `emit_one` returns the port's answer and
`c/build.rs` says so in its own comment -- "the chain entered whole: the port's
answer is the unit's, and the reference is not run". **There are no bodies both
emit.** So the census is not a diff. It is the port's refusals.

**The vocabulary, counted.** `emit.ply` raises a refusal at seventeen call sites
with fifteen distinct reasons; `code.ply` raises none. They are: `iterate` with no
`Stop` and `Continue` in the program; a capture the port has not bound; a local it
has not bound; a global, a name, or an effect it cannot resolve; a constructor not
in the table; a fixed-width literal of no known width; a literal it does not pool;
a scalar parameter read outside its window; an operator it does not emit; a
callback or inline builtin outside its fused shape; an effect construct or another
node it does not emit; a `handle` this tier cannot carry; and a shift over a value
whose type the fragment does not fix.

**What a refusal does is not a fallback.** `emit_one` turns
`Answer::Refused(why, _)` into the tier's own `Refused { function, construct: why }`,
which joins the fixpoint's refusal list and drops the function from the compiled
unit -- it stays interpreted. A construct the port does not emit costs a function
its place in the tier; it does not produce a different emission.

**Why each is a missing fast path and not a wrong rule -- armed, not argued.**
`the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds` installs the
producer, builds the emitter from the bundle, has it emit its own sources, and
asserts `p1.refused.is_empty()`; on a refresh it asserts the same of the
generation built from that emission. It is a `SOLO` row, so every push asserts
that the port emits **all of itself, refusing nothing**. That is the goal's own
argument made checkable: a wrong rule would not appear as a refusal, it would
appear as a fixpoint that does not close, and that is the property this test
watches.

**What is not armed, and is the real precondition for deleting the emitter --
corrected, because the first statement of it named the wrong gate and the wrong
corpus.** Nothing asserts the port answers every body it is asked for. The
producer's counts are read in one place, `ply-codegen-tests/tests/producer.rs`,
and asserted only `answered > 0` -- "the Ply emitter answered nothing of {asked}
asked, so nothing below is about it", which is a liveness check.

But `answered == asked` is not the gate to arm, and this record said it was. The
counts are `Cell<u64>` accumulated over a thread's life and never reset, and
`body()` increments `asked` before returning `None` for an out-of-range module
index -- so the pair counts calls, not bodies the port was expected to emit, and
an equality over them would be armed in name only. `producer.rs`'s own header says
what the instrument is: "the count of what it answered is the ratchet."

And the corpus was wrong too. `fragment.rs`'s census is named for the standard
library and does load it -- `load()` collects `ply_std::sources()` and appends the
fixture as module `m` -- but it builds through `unit()`, which wraps
`producer::reference_only(...)`. That forces `mode() == "ref"`, so the **reference**
emitter produces every body and the port is never consulted. The refusals it
counts are the tier's under the Rust emitter, ADR 0030's ranked roots, not the
port's. `parser_census.rs` is the same shape. `whole()` is the helper that builds
under the port, and it has one caller, `number_types.rs`, which does not census
anything.

**So no test censuses the port's refusals over std, and the gap is one test wide.**
Over the port's own sources the question is already settled and armed -- the
bootstrap fixpoint asserts no refusals at all. Over std and the corpus nothing
measures it. What that test looks like is not open: build through `whole()` so the
port produces, census `unit.refusals()` by construct, and hold the count in the
idiom `benches/compiled-compiler.json` already uses -- **not** through
`census::hold` itself, which holds `entries`, `allocated`, `recycled` and
`chunk_bytes` from `producer::census()` and cannot carry a refusal count. What
transfers is the shape: a `benches/*.json` entry the test compares against, whose
missing entry fails with the reading printed for pasting, so the figure is blessed
from a run rather than guessed. That is why this is reachable without the heavy
local load this machine is not to be given: CI computes it and its failure hands
over the line. **Writing that
ratchet is
what would let §2 delete `c/emit.rs`**; until that exists the deletion would rest
on prose, and this record has spent the day learning what prose-sized estimates
are worth.

**Built, 2026-09-17: `whole()` builds under the port it documents.**
`fragment.rs`'s `whole()` is documented as "the same program under the whole Ply
emitter", and `number_types.rs`'s `the_two_emitters_answer_the_same_at_each_width`
compares its answers against `unit()`'s. Neither claim held.

Whether the port emits depends on `producer::mode()`, which answers `"ref"` unless
a producer is installed, and nothing on `whole()`'s path installs one:
`Unit::over_with_texts` calls `front_for` and then `over_front`, and
`ensure_default()` has no call site anywhere in `ply-codegen` outside its own
definition. `.config/nextest.toml` makes that decisive rather than probable --
"one process per test" -- so the single `ensure_default()` in this binary, in
`unit/c.rs`, cannot reach another test's process.

So both units were emitted by the reference, and what that test compared was two
ways of obtaining a `Front`, not two emitters. Its constant checks were always
real: they hold the fragment emitter to a hand-written answer at each width. The
cross-check was not, and the second loop -- `round_trip` and `mixed`, which carry
no constants and are "checked against the whole emitter alone" -- proved nothing
at all.

One line in the helper repairs every caller, and the ratchet §2 prescribes would
have needed it too: written without it, that gate would have censused the
reference emitter exactly as this test compared it with itself.

**This is also the first time a gate makes the port emit these bodies**, which is
why the reading matters beyond the repair. `emit.ply` has the widths --
`wrap_add` and its family, `u8_of_int` and the seven beside it, `int_of_u8` and
its seven, shift precedence and constant folding -- and its two width-shaped
refusals are narrow: a fixed-width literal whose name has no known width, and a
shift over a value the fragment leaves `TyOpaque`. If this run is green the two
emitters agree at every width in the test. If it is red, the failure names the
width construct the port refuses, and that is the empirical census §2 asked for
and that this record has said no test measures.

**Built, 2026-09-17: the port refuses no more of the standard library than the
reference.** §2 asks this record to name the bodies the port still emits differently
before `c/emit.rs` goes, and the entries above say twice that nothing measures it.
Something does now.

The gate is a *comparison*, not a blessed ceiling, and that is the point. The
ceilings a few paragraphs up are the reference's, and they ratchet down as
constructs land. What matters before the reference can be deleted is narrower: that
the emitter replacing it does not cost the tier a definition the one it replaces
kept. So the test builds the same loaded standard library twice -- `unit` under
`reference_only`, `whole` under the producer -- and asserts the port refuses nothing
the reference emits. A construct only the port refuses is named by the failure, with
the reason the port itself gave, which is the census §2 asks the record to carry.

It needs no number blessed from a run, which is why it can be written under the
constraint this record works within. And it could not have been written a day ago:
`whole` did not install a producer, so both units were the reference, and a
comparison like this one could not have failed.

**Read, 2026-09-17: the seed path cannot go, and attempting it is what showed
why.** This record has twice named the seed path as the last thing holding the
Rust front end into `ply-codegen`, and twice implied it was a matter of writing
the recovery down. It is not. Deleting `from_reference` takes `ply test` with it
for any working copy, and the reason is structural rather than an oversight.

`PLY_C_EMITTER=ply:<dir>` is the documented way to run an emitter change before it
is bootstrapped, so that directory has **no bundle by construction** --
`bundle::of` returns `None` for it, and `from_reference` is what built an emitter
from those sources. Remove it and `ply test` produces no report at all;
`lang_fixtures` caught it in four partitions.

The obvious repair does not work either. To build an emitter *from source* you
need a `Source`, which needs a front end, and the port cannot be that front end
here: `mode()` answers `"ref"` whenever `BUILDING` is set, because this runs while
the producer's own unit is being built. The port cannot answer for the program it
is compiled from. Building it instead from the *committed* bundle and using that
emitter to emit the working copy is the way out, and it is a two-stage producer --
a restructure, not a fallback removed.

Falling back to the embedded bundle would make the fixture pass while silently
emitting with the committed emitter rather than the working copy's, which is a
green that tests the wrong thing. That is the trap worth recording: the failing
test was the honest answer.

What this pull request keeps is the part that was genuinely removable:
`PLY_C_BOOTSTRAP=off`, which nothing set -- no test, no workflow, no script, only
four documents advertising it, which move here. `from_reference` stays, reached
now by the two cases that are real: sources with no bundle, and a bundle whose
helper table this runtime's does not start with.

**Read, 2026-09-17: what taking the seed path out would actually require.** The
entry above says it cannot go and why the obvious repairs fail. This says what
would work, so the next attempt starts from a specification rather than from the
attempt.

The producer holds **one emitter per run**, and the code says so three times.
`RECIPE` is a `OnceLock`: "the first installation wins; a second is ignored,
because a run has one emitter". `MINE` is a single `RefCell<Option<_>>` per
thread, so a second producer evicts the first rather than standing beside it. And
`BUILDING` is a thread-local `bool` rather than a per-recipe one -- `with_current`
sets it around `recipe()` and returns `None` outright while it is set, so any ask
nested inside a recipe degrades silently to the reference.

A two-stage build -- the committed bundle's emitter emitting a working copy's
sources -- is a nested ask by construction, so all three have to move. The front
end is only half of it: `build.rs`'s `emit_one` consults `with_current` for every
body, so the emitting half needs a producer *handed* to it rather than one it looks
up.

So the change is: key `MINE` and `BUILDING` by recipe identity, or thread a
producer explicitly instead of through thread-locals, and give `emit_one` a
producer to use. That reaches `reference_only`, `reset_thread` and every
`with_current` caller. It is the one change that ends §2's last production
dependency on `ply-core`, and it is an architecture decision rather than a
deletion -- which is why this record names it instead of attempting it twice.

**Built, 2026-09-17: the handover, and what three red runs were hiding.** The
entry above names what taking the seed path out would require, and it was built:
`with_producer` hands an emitter over, consulted before the `BUILDING` and
`REFERENCE_ONLY` guards, carrying the identity its bodies key under, so the
committed emitter stands up a working copy. It took `ply-core` to no production
dependent, and it went red three times on one test -- `lang_fixtures`, in four
partitions each time.

What the log ruled out is worth keeping, because both were the obvious guesses.
**Not thread crossing**: the handover is a `thread_local!`, but `build.rs` spawns
nothing and uses no rayon -- the only mentions of workers are comments on why
`ply_eval::Value` holding `Rc` keeps emission off it. **Not a missing bundle**:
`bundle::of(&Sources::Embedded)` returns an ungated `include_str!` constant, so
there is always a committed bundle to start from.

It was neither, and it was not a refusal either. `Source::from_front` leaves
`texts` empty, and `bodies_of` answers `Ok` with an empty map -- not an error --
when a module has no text. So the handed-over emitter served every request and
answered none: every root refused as "which the Ply emitter did not answer", with
nothing recorded in `failed` for `failure()` to report. The shape that hides a
missing input is the shape that reports its absence as unanimous.

The reference path never reached it. There `BUILDING` makes `mode()` "ref", so
`produced` is false and the port is never asked for a body; handing an emitter
over makes `mode()` "ply" and asks it for every one, which is the point of the
change and the reason this seam had never run. `with_texts` attaches them.

Two attempts were spent before the canary printed anything -- `cmd.output()` read
`stdout` and dropped `stderr`, and the `expect` threw away the report it did get.
The third printed the root count, whether the entry was among the roots, and the
refusals, and named the cause on one run.

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
and not yet exercised. The merges after it read 126 s, 130 s, 142 s, 168 s, 163 s, 129 s, 224 s, 157 s, 184 s, 149 s, 148 s, 158 s, 149 s, 156 s, 173 s, 140 s, 147 s, 140 s, 153 s, 164 s, 324 s, 130 s, 139 s, 223 s, 140 s, 136 s, 140 s, 127 s, 145 s, 142 s, 138 s, 149 s, 153 s, 150 s, 147 s, 166 s, 133 s, 134 s, 143 s, 145 s, 140 s, 140 s, 203 s, 153 s, 199 s, 381 s, 139 s, 168 s, 167 s, 211 s, 219 s, 193 s and 167 s, each reusing by tree the same way and for the same reason. Ten of those went over. Two were the merge that made the port the only front end and the first attempt to answer it, which the paragraph below takes; two more are 324 s and 223 s, and the paragraphs after it take them; the fifth is 203 s, the sixth 199 s, the seventh 381 s and the eighth 211 s, which the entries closing this section take. None of those six was caused by the tree. The ninth and tenth, 219 s and 193 s, were: they are the handover's own cost, and the two entries closing this section take them together with the reading that placed it. The highest of them, 173 s, is seven seconds under the bound, which reads as a drift and is not one: over the last nine green runs the longest partition has been 88, 91, 97, 101, 101, 102, 110, 113 and 120 s, a band with no step where §2's text goldens landed, 50 MB of them at the time. A draft of this sentence called it a trend, from four partitions in one run's longest-five rather than from the series. Eleven running have hit the lookup directly, because none of them moved main while
another pull request sat behind it, so the fallback this paragraph describes
is still unproven in the case it was written for.

**Read, 2026-09-17: a second run over the bound, and the same kind of cause.**
The merge recording where §1 and §2 stop read 223 s, on a records-only change.
Not the tree, and not the reuse: both build legs took their artifacts back and
do not appear among the twelve slowest jobs, the run was created and started in
the same second, and every test job finished by +142 s against +129 s on the
merge before it — thirteen seconds of ordinary variance. The whole difference is
the `CI` aggregate job, which computed nothing: its log spans **0.3 seconds**,
set-up to "Every job succeeded" to complete, and the API bills it at 72 s, ending
at +218 s. The same job took 3 s on the run before.

So both of today's over-bound readings are the runner platform rather than this
repository — that one and the 324 s below it. Worth one note rather than two
mysteries. And worth saying how it was found: three causes were guessed first —
the tree-reuse fallback, the runners' arrival staircase, and the aggregate job
doing real work — and the data refuted all three before the per-job timestamps
settled it. The staircase was the closest and still wrong: jobs started at +38
to +58 s in both runs.

**Read, 2026-09-17: a run over the bound that the tree did not cause.** The
merge retiring the `diag` and `front` differentials read 324 s. §3's rule is that
such a run is the next item before any other, so it was taken before anything
else, and the cause is not this repository's. One partition of eight took 295 s
where the other seven took 68–105 s and where the same job had taken 80 s on the
previous merge. Inside it, nextest reports 45.7 s of tests: roughly 250 s of that
job was not testing.

The job's log names what it was. The per-slot object cache failed to restore
(`GetCacheEntryDownloadURL: EHOSTUNREACH`), so tcc rebuilt every fixture from
scratch — which is why its tests read 45.7 s against the control's 32.4 s. Four
retries against `results-receiver.actions.githubusercontent.com` failed with
`getaddrinfo EAI_AGAIN`, about 65 s. The cache then failed to save, about 21 s
more. A disk-cleanup step took 36 s. The same infrastructure had already left an
earlier run unregistered for an hour, with no run existing for a pushed head
until a poller had nearly spent its budget.

Re-running the identical SHA is the controlled experiment, and it read 125 s.
So the tree is where it was and the reading stands as an outlier with a named
cause. Recorded rather than dropped, because a bound that only keeps its
favourable readings is not a bound — and because the next unexplained run over it
should not have to rediscover that this one was the network.

**Read, 2026-09-17: the switch put the clock over, and what brought it back.**
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

**Read, 2026-09-17: the run that went over, and where the 203 s went.** §3 says a
run over the bound is the next item before any other. This is that item, and the
tree is not in it: #356 changed one markdown file.

Against the run before it, which read 140 s over the same shape of tree:

| | the 140 s run | the 203 s run |
| --- | --- | --- |
| partitions start | 36 s | 35 s |
| longest partition | 97 s, ending 133 s | 123 s, ending 159 s |
| every job finished by | 133 s | 159 s |
| the `CI` aggregate starts | 136 s | 198 s |

Two things, and neither is the tree. The fan-out started on time. The longest
partition grew from 97 s to 123 s -- against 108, 107, 107 and 103 over the four
main runs before that, so 123 is a new high by fifteen seconds, and one reading.
And after all twenty-three jobs had finished at 159 s, the aggregate did not start
until 198 s: **thirty-nine seconds with nothing running at all**. Every one of the
twenty-three was listed and checked before that sentence was written, because a
truncated list would have hidden a slow job and read as scheduling -- the same
mistake in a smaller place.

So 203 s is 140 s plus twenty-six seconds of slower work and thirty-six of a job
waiting to be scheduled. The larger half is entirely the platform's. It is the
fifth platform-side reading this record has taken, after `EHOSTUNREACH` on a cache
download, `getaddrinfo EAI_AGAIN` retries, `sccache-action` answering HTTP 500,
and an artifact download that took 27 s where the run before it took 6.

**What this does not license.** The band above cites nine longest partitions from
88 to 120 s, taken when the question was whether 173 s was a drift. The five
before this run are 97, 108, 107, 107 and 103, so the band has crept up and 123
sits above it. One reading is not a trend -- that paragraph's own draft was
corrected for exactly that error, calling one from four partitions in a single
run -- and the next merge's longest partition is the reading that would say
otherwise. What §3 asks for here is that the run be read and recorded, not that a
number be moved.

**And the next merge answered it: 153 s, longest partition 117 s.** So the
203 s was variance and not a step, which is what the paragraph above said the next
reading would settle. The bound holds without anything being done to the tree,
which is the outcome a diagnosis pointing entirely at the platform predicts.

**Read, 2026-09-17: main went red, and the job that caused it reported success.**
Not a reading over the bound but a failure, recorded because the shape is worth
knowing. Sixteen test jobs failed inside eighty seconds, every one with the same
line: `Failed to restore cache entry. Exiting as fail-on-cache-miss is set. Input
key: nextest-archive-<run id>`. No test ran.

The cause was in none of the sixteen. `cargo nextest archive` **succeeded** -- it
reused the archive an earlier run built for this tree, the mechanism §3's series
describes -- and then hit `You've hit a rate limit, your rate limit will reset in
13 seconds` and never saved the entry under this run's key. So the job that broke
the run is green in the run's own summary, and the sixteen that are red name a
cache key rather than a cause. The aggregate says which leg failed and never why,
which is the procedure this record wrote down after an `sccache` HTTP 500: read
the failing job's log, then read the log of the job it was waiting on.

**The remedy follows from the shape, and getting it wrong costs a cycle.**
Re-running only the failed jobs would have failed again: the key still would not
exist, because nothing among those sixteen writes it. The whole run had to be
re-run so the archive job's save step executed again. It did, and logged `Cache
saved with key: nextest-archive-<run id>`.

**And `fail-on-cache-miss` is vindicated by its own failure.** Without it each
consumer would have rebuilt the archive quietly, turning a lost cache entry into
sixteen slow green jobs and a wall clock this record would have spent an afternoon
attributing to the tree. It failed loudly instead, and the loud failure took ten
minutes to read.

**Read, 2026-09-17: and that re-run went over, at 199 s, on one partition
scheduled forty seconds late.** The figure is not an artifact of re-running:
`run_started_at` is reset to the second attempt's start, and the job span reads
191 s independently, the eight seconds between them being the run object's own
bookkeeping. Where it went is narrow. Every partition started at 29 or 30 seconds
except `test 4/8`, which started at **70** and ran 114 s to end at 184 -- thirty-
nine seconds after the next-latest job, with nothing waiting on it. It was
scheduled late.

So the sixth over-bound reading is the same shape as the fifth: a job that did not
*start*, not a job that ran long. The fifth was the aggregate waiting thirty-nine
seconds after every job had finished; this one is a partition waiting forty before
beginning. Both are the platform, and the tree is in neither -- the merge under
this one changed one line of a test helper and one document.

**Read, 2026-09-17: a quiet main run reads 139 s, and every excursion was a job that
did not start.** §3 asks for a quiet main run under 180 s. This one was taken with
nothing else of this session's in flight, and *measured* rather than assumed: the
reading listed every run overlapping its window and found none. It reads **139 s**,
its partitions beginning at 27 s and the longest running 105 s.

Against that baseline the three over-bound runs are one shape. At 203 s every job
had finished by 159 s and the aggregate did not begin until 198. At 199 s every
partition began at 29 or 30 except one, which began at 70. At 381 s no partition
began until 245. None of the three is work taking longer; all three are a job that
did not start.

**And contention explains less than it first appeared -- the first measurement of it
was wrong.** An overlap table built from each run's `createdAt` suggested two of the
three had a competitor in flight. Taken again from `run_started_at` -- `createdAt` on
a re-run spans the attempt that failed -- the 199 s run's second attempt began
fifty-one seconds *after* its supposed competitor ended, and the 203 s run had no
concurrent run at all. Only 381 s overlaps one, and there the competitor started
139 seconds into a 245-second wait, so it cannot account for the first 139. The
honest statement is narrower than the tidy one: these runners usually schedule a
fan-out within half a minute and sometimes do not, and the tree is in none of it --
the merges under those three runs changed one line of a test helper, one document
and one ADR.

**Read, 2026-09-17: the seventh excursion of one shape, and by now that is the
finding.** Main read 211 s after a merge that changed one test helper and one
document. The partitions did not begin until **97 s**, against 27 s in the quiet
run this section measured; once begun, the longest ran 107 s, inside the band
recorded above; the aggregate followed three seconds after the last job. So again
nothing ran long -- the fan-out started seventy seconds late.

It was **uncontended**: no other run overlapped its window, measured rather than
assumed. That matters because contention was the tidy explanation this record
reached for once and had to withdraw: re-measured from `run_started_at` rather
than `createdAt`, two of the three excursions then in hand had no competitor at
all.

Seven excursions shared one shape -- a job that does not *start* -- and four were
measured quiet. The bound holds on a quiet runner: 139 s, partitions at 27 s.
What varied across those seven is the scheduling around them, and the tree was in
none of it. The entry below is the first to break that shape, so "the tree is in
none of it" is a statement about those seven and not a standing property of this
section.

**Read, 2026-09-17: an excursion of a different shape, and this one is the tree.**
Main read 219 s after the handover merged. The partitions began at **+39 s** --
against 27 s in the quiet run and 24 s and 38 s on the two merges before it, not
the 97 s of a late fan-out -- so nothing was waiting to be scheduled. The pole was
`test 5/8` at **171 s**, which ran 108 s and 102 s on those same two merges, and
the aggregate followed three seconds behind it. The wall is over because the
longest job grew by about a minute, not because the start slipped.

This is the first excursion here the tree accounts for, and the mechanism is the
handover §2 records. `from_reference` left `produced` false, so the *reference*
emitted every body when an emitter was stood up from a working copy;
`from_committed` hands an emitter over, which makes `mode()` "ply" and `produced`
true, so the port emits them instead.

One test names the whole of it.
`lang_fixtures::the_shift_and_overflow_raises_are_the_same_on_both_engines` ran
**8.468 s** on the merge before and **99.104 s** here, crossing nextest's slow
threshold it had never approached. It is one of four that pass
`PLY_C_EMITTER=ply:<dir>`, which has no bundle by construction, so each stands an
emitter up from the committed bundle and emits the port's own sources through it;
there is one in each of partitions 1, 3, 4 and 5, and three of the four longest
jobs are among them. The shape of the cost is eager: `bodies_of` asks
`emit_unit_all` for the whole unit, where the reference emitted one body at a
time as `emit_one` asked for it. `--no-cache` is not the lever -- it points `ply
test`'s store at a scratch directory and never reaches the C unit cache.

The work did not appear -- it moved to the implementation this record exists to
reach. So this is a cost to place, not a regression to revert, and by §3's own
rule placing it is the next item before any other.

**Read, 2026-09-17: the cost is placed, and the bound is back.** A working copy
holding the sources the bundle was emitted from *is* that bundle, which the
fixpoint test already asserts by comparing `sources_digest()` against
`digest_of` over those modules. So `from_committed` hands the carried native
back rather than emitting the same unit again. The four fixtures read 0.169 s,
0.199 s, 0.220 s and 0.254 s, where two of them had read 86.339 s and 99.104 s.
That is faster than the 6.628 s and 8.468 s they cost *before* the handover,
and for a reason worth keeping: the early return is taken ahead of
`front_end`, so it skips the emission and the front end both, where the old
path still ran a Rust front end over every definition to emit on demand. Main
read 219 s, then 193 s on the record-only merge that changed no code, then
**167 s**, the longest job falling from 171 s to 131 s.

What it gives up is written down because it would not be rediscovered
otherwise. `from_committed`'s own plumbing -- `with_producer`, `HANDED`, and the
`texts` whose absence took three red runs to find -- now runs only when a
working copy differs from the bundle, which is the first pull request of an
emitter change and not every one. `test bootstrap` does not stand in for it: it
emits the same unit through a `build_from` of its own and never enters this one.
That is the right place for the exercise, and it is less of it.

**Read, 2026-09-17: a cancelled run costs the next one, and has no wall clock of
its own.** Main read 288 s after a merge that changed one document and no code.
That is the largest excursion this section holds and the first of a third shape:
not a job that failed to start, and not the tree.

The time is in the build rather than the fan-out. `cargo nextest archive` ran
**156 s** against 32 s on the run measured above, and `cargo build --release -p
ply-cli` **124 s** against 17 s. The partitions began at +164 s, which reads like
the late fan-out of the seven excursions above and is not one: the archive ended
at +161 s and the partitions wait on it. Once begun they ran 88 s to 116 s, which
is the ordinary band.

The cache was cold because the run before it never finished. Landing two merges
back to back cancels the first merge's run, and a cancelled run writes no cache,
so the next one rebuilds from nothing. One event, two symptoms: that cancelled
run also left this section no reading to take.

**A cancelled run still has a duration, and it is not a wall clock.** The figure
first taken for that merge was "134 s, under the bound" -- the length of a run
killed part-way, not the length of a run that did the work. A reading counts only
when its conclusion is `success`, and a watcher that prints a bound against
anything else is reporting a pass it did not measure.

**So the cadence is part of the bound.** A merge lands after the previous merge's
run has finished, or the next run pays the build from cold. No deletion in §2
moves that, and the largest excursion in this section came from spacing two
merges too closely rather than from anything in the tree.

**Why each of these got written at all.** The scripts that append a reading assert
`wall < 173`; an over-bound one fails them and takes a different entry point that
asserts the opposite. The guard is the reason eight excursions were each written
consciously instead of accumulating into a series nobody read.

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
