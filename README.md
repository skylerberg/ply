# Ply

A programming language built around one bet: that generating code is becoming
free, and that what stays expensive is knowing whether it is correct.

So Ply is designed backwards from the verification loop. Not "how do we make
tests run faster" as a tooling concern, but "what would the language have to look
like for the loop to be near-instant and the signal to be trustworthy."

It is a research language. It serves HTTP over TLS against real postgres, and it
does so at **1,687–3,778 requests per second on one core** — **17–38x** what the
same syscalls cost in Rust with no interpreter under them. If that disqualifies
it for you, that is a reasonable conclusion, and
[Where this is not competitive](#where-this-is-not-competitive) says the rest of
it plainly.

- [The loop](#the-loop) — what the verification story buys, measured
- [Three ideas](#three-ideas) — effects, content addressing, flakiness
- [Serving](#serving) — what an HTTP service costs, layer by layer
- [Where this is not competitive](#where-this-is-not-competitive)
- [What is missing](#what-is-missing)

**Writing Ply?** [`docs/GUIDE.md`](docs/GUIDE.md) is the user-facing manual —
syntax, types, effects, tests, specs, the standard library, the CLI and the
diagnostic codes, end to end.

**About the numbers here.** This file gives *shapes* — which layer dominates,
which way a trade goes — and not figures to be re-taken. Figures live where they
were measured: `benches/w6-ladder.json` and `benches/w6-spike.json`, written by
the commands that took them, and the ADRs that argue from them. That split is
deliberate. A magnitude in prose stays true across a refactor; a decimal invites
somebody to spend an afternoon reproducing it and to conclude the number needs
refining when the paradigm is what needed changing.

Two numbers here are *armed* rather than asserted, and they are the exception
that shows the rule: `w6_report_integrity::the_shipped_ladder_still_describes_the_tree_it_ships_in`
and `w6_report_allocations::the_readme_still_describes_this_request_path` fail
when this file stops describing the tree. A number worth keeping in prose is one
a test will notice going stale.

## The loop

On a generated project of **200 modules, 10,000 definitions and 5,000 tests**,
157 of them nondeterministic and therefore never cacheable:

```
$ ply test                                  # empty cache
   0 failed, 5000 passed, 0 cached                      2.60s wall

$ ply test                                  # nothing changed
   selected 157 of 5000 (4843 cached)
   0 failed, 157 passed, 4843 cached                    0.52s wall
```

The 157 that ran are the nondeterministic tests, which run every time by
construction. Now change something:

| what changed | tests selected of 5,000 |
| --- | --- |
| nothing | 157 (the nondet ones) |
| **a top-level function renamed project-wide** | **157 — the same** |
| one leaf definition's body | 158 |
| a hub definition's body, 898 dependents | 613 |

**The rename row is the point.** It is not "probably safe to skip." A rename
changes no definition's hash, so there is provably nothing to re-run, and the
count is identical to changing nothing at all. Selecting *zero* deterministic
tests after a rename is an invariant the test suite asserts, not a heuristic.

And where a warm run's work goes: **typecheck, hash and parse are about
two-thirds of it between them, and execute is under a third.** So a faster
evaluator buys less here than a faster type checker would. That is the shape of
the argument that has kept native codegen deferred, and
[Serving](#serving) is where it inverts — but the case for deferring M9 does not
rest on this profile, it rests on the *served* one below, and
[ADR 0011](docs/adr/0011-the-web-track.md) is where the decision actually lives.

`ply-corpus bench` prints all nine phases if you want them; they are not copied
here, because a phase table in a README is a thing to re-take rather than a thing
to think with.

> **`bench` over-states interpreter time.** It builds a worker per pool thread
> per concurrency group, so its `execute` phase carries setup charged per group.
> `ply-corpus measure` is the harness that separates the two. See
> `benches/README.md`.

Opening a content-addressed store of ten thousand definitions costs single-digit
milliseconds, which is what lets the cache sit in the inner loop rather than be a
build artifact.

> **This loop is the interpreter's, and the compiled one is not yet its equal.**
> What makes it O(change) is the front-end cache keyed by content and a test
> selected against the definition set it last passed under. Under
> `--backend cranelift` the selection still holds and the compilation does not:
> `crates/ply-codegen` persists nothing between runs, so a backed run compiles
> the whole reachable program again, and every invocation of every command
> starts cold — there is no `watch`, no daemon and no server. ADR 0037 carries
> what that costs, the row that would price it, and why emitted C is refused
> inside the loop.

## Three ideas

**Effects are in the type, at resource granularity.** Not `IO`, and not even
`db` — `db.read[users]` is distinct from `db.write[orders]`.

```ply
fn active_users() -> List<User> / {db.read[users]} = ...
```

That precision is what lets the scheduler decide, statically, which tests can run
at the same time: two footprints contend only if they share a resource and one of
them writes. It is also what makes an endpoint's signature a map of the API to
the tables it touches: `ply check --types` prints the exact atom set for every
route in the example service, and the row is *inferred* — writing one is optional
and checked as an upper bound, never as a widening.

The row is the only half of a signature that works that way, and deliberately: a
row is *derived* from what a body calls, while a type is *chosen*. Every
parameter type and return type on a top-level `fn` is written (`E0126`), because
a published signature a reviewer holds fixed cannot be a summary of the body it
describes. Infer what is mechanical; write what is meant.

**Definitions are content-addressed.** The unit of compilation is the definition,
not the file. A definition's hash is computed over its normalized structure, with
references to other definitions replaced by *their* hashes and local names
replaced by de Bruijn levels. A definition compiles once, ever. A test result is
keyed by the test's hash, so it stays valid until something it actually depends on
changes. Moving a definition between modules changes no hash anywhere.

**Flakiness is a compile error.** Effects can be declared `nondet`. A test is
deterministic by default, so if a nondeterministic atom survives in its footprint,
the program does not compile — rather than the test failing on its 400th CI run.

```
[E0412] Error: nondeterministic effect in a deterministic test
   ╭─[ user.ply:8:13 ]
   │
 8 │   assert_eq(stamp(), stamp())
   │             ───┬───
   │                ╰───── reaches `user.clock.read`, and `user.clock` is declared `nondet`
   │
   ├─[ user.ply:8:13 ]
   │
 7 │ test "it stamps" {
   │      ─────┬─────
   │           ╰─────── test `it stamps` is deterministic
   │
   │ Note 1: `user.clock.read` is performed inside something this expression calls
   │
   │ Note 2: handle it here, e.g. `handle <body> with { clock.now() -> <value> }`
   │
   │ Note 3: or declare this `test/nondet`, which opts out of the cache and re-runs every time
───╯
   compilation failed (1 error)
```

That is transcribed from a run, not sketched — the atom and the effect are
printed module-qualified (`user.clock.read`), which the earlier version of this
block dropped.

Handlers are what make that practical: swapping a real resource for an in-memory
one is a language construct, not a mocking library, so the double and the real
thing are checked against the same declared signature and cannot drift.

Three things built on top of those, each with output you can run today:

**Concurrency is an effect, so races are searched rather than waited for.**
`ply test examples/bank.ply` runs four tests, three of them through a seeded
scheduler, and reports `6 interleavings · exhaustive` for *"the guarded transfer
never overdraws, under every interleaving"*. Exhaustive means every interleaving
the footprints did not prove commutative — a proof, not a sample. A failing
schedule comes back as a seed that replays exactly.

**Specs are the reviewable artifact, and their strength is derived.**
`ply prove examples/desk.ply` reports 7 obligations over 180 definitions: 2
`proved` (one exhaustive over 11 constructors, one by ground evaluation) and 5
`property` with their case and rejection counts. A tier is computed from the
evidence rather than stored, so a `proved` that was really a sample cannot be
asserted: `ply_prove` has no `tier` field and `Evidence::tier()` derives it. (This
line used to say "there is no `tier` field anywhere", which one grep falsifies —
the on-disk cache record `ply_store::obligations::CachedObligation` has a
`pub tier: String`. It is not an authority: the reader recomputes the tier from
the evidence and refuses the entry when the two disagree, so the label exists to
make corruption *detectable*, not to be believed.) Coverage is in the default
output: 167 of 180 definitions carry no obligation, and it says so without a
flag.

**The trusted computing base is one command.** `ply hosts examples/desk.ply`
says `hermetic — no host handler is bound`, because tests do not reach the host
unless you ask. With `--host` it lists 25 host handlers over 47 operations, each
with its footprint, its determinism flag, whether it may resume more than once,
whether it blocks, and whether it can receive a secret.

## Serving

`examples/desk.ply` is an eleven-route service: HTTP/1.1 with keep-alive, a route
table that is ordinary data, TLS, postgres with transactions written as effect
handlers, derived JSON, tracing as an effect, typed secrets, configuration
checked against a schema at start-up, and graceful shutdown.

Here is what one request costs, end to end, with everything switched on. Each
row is a **difference between two measurements that changed exactly one thing** —
not a profiler's attribution — and the total is measured rather than summed:

| layer | share of a request |
| --- | --- |
| postgres: the boundary, the wire and the server | over half |
| the serving loop, and HTTP/1.1 framing | about a third between them |
| routing one path against the eleven-route table | under a tenth |
| the socket, the reactor, the blocking pool | under a tenth |
| the TLS record layer, and tracing to JSON | about 1% each |
| entering the interpreter, and the route's own body | near zero — see below |

[ADR 0011](docs/adr/0011-the-web-track.md) carries the microsecond figures, the
per-workload throughput table and the Rust floor each row is measured against.
They live there because they move whenever the service does, and a reader who
needs them needs the conditions with them.
The picture is unusual. **The two features that sound expensive — TLS and
tracing to JSON — are about 1% each**, and neither difference is bigger than the
spread of its own repeats. There is no I/O to hide behind either: the socket
layer is under a tenth, and the interpreter is roughly a third of a request once
the measurement's own seam is charged against it.

Two rows deserve suspicion. The endpoint's near-zero is a *memo hit*:
`/health`'s whole body is a nullary pure definition, and the constant memo
evaluates one of those once per process. That row used to be two orders of
magnitude larger — the route table was rebuilt from its pattern strings on every
request — and removing it is worth **1.77x on `/health`**. That is the thing this
project keeps finding: the cheap algorithmic fix beats the expensive
execution-strategy fix, three times now. The database's share is large partly
because `/items`' own JSON encode sits inside that rung, which the ADR discloses
rather than nets out.

Which is why there is still no code generator on the request path. A Cranelift
spike hit better than 10x on the innermost scanning loop of the HTTP parser,
agreeing with both interpreters on every input first — and applied to a
one-third interpreter share it still projects **under the 1.50x bar fixed before
any of these numbers existed**, and under the ceiling an infinitely fast backend
would have. Three of the four criteria fail.
[ADR 0011](docs/adr/0011-the-web-track.md) is the argument, including three
measured reasons to think even that projection is optimistic. **The lesson is
Amdahl's, not Cranelift's** — a faster backend cannot reach what it is not a
large enough share of, so read a loss here as a fact about where the time is.

## Where this is not competitive

The honest ceiling, stated here rather than discovered later. Every item has a
measurement or an explicit "not measured".

**One machine is one core.** A `Value` holds `Rc` and a continuation is
`Rc<Vec<Segment>>`, so a Ply task cannot move between OS threads. Throughput
scales by processes; every runtime you would compare this against scales by
threads. Measured on `/health` over postgres: the sequential accept loop runs
**3,914 req/s at c=1 and 3,930 at c=32** while its p99 goes from **287µs to
252ms**, and the task-per-connection loop runs **2,157 → 2,254 req/s** with p50
from **457µs to 13.7ms**. Extra concurrency is a queue on either loop.

**37.8x the Rust floor** on `/items` over postgres over TLS with tracing,
against a floor replaying the same 270-byte response. Like for like — `/health`
over plaintext, against a floor replaying its own 107 bytes — it is **16.8x**.

**A service whose accept loop spawns memoizes nothing.** `task.spawn` opens a
region that stays open for the life of the server, and the constant memo is
refused inside any open region, so every nullary pure definition is
re-evaluated per call. Disabling the memo by source substitution costs **1.77x**
on `/health` on the sequential loop and **1.00x** on the task-per-connection one,
where there is nothing left to disable — spawning is the more expensive of the
two before the memo is considered at all.

**A real browser's request head costs more than a `curl`-sized one**, and the
throughput figures are taken with a small head. The shape is what matters and
`ply-corpus serve --load-headers` shows it: cost is proportional to **fields
parsed** rather than to bytes received, so the µs-per-byte column *falls* as the
head grows. That is the good regime; a browser is dearer simply because it sends
more fields. A req/s number quoted without its head length is worth less than it
looks.

**No cancellation, no backpressure, no load shedding.** A request still live at
the drain deadline loses its connection with no response and the process exits
`3`. An overloaded service queues until something times out. Backpressure was
promised by one milestone and explicitly withdrawn by the next; it is not
subtle, it is absent.

**The in-memory test double is slower than the database it replaces.** Most of a
twin call is the memory engine parsing its SQL in Ply, and every twin clause
writes its whole state back through a persistent map. Tests that use the double
for speed will be disappointed; they use it for isolation and determinism, which
it does deliver.

**The tracing sink is linear.** `std.trace`'s `Sink` appends with `push`; a
collecting twin holding N records costs N pushes and no copies, wherever the
caller threads the sink.

> **The mechanism, because "appends with `push`" alone teaches the wrong
> lesson.** `push` grows a `List` **in place** when the caller is its last owner
> (`crates/ply-eval/src/list.rs`) and, when something else can still see it,
> copies one leaf and the path above it rather than the whole array: a `List`
> is a radix trie whose newest leaf is held apart, so the copy is bounded
> whatever the length (ADR 0034). The machine moves a binding's value out of
> its slot at its last use, so sole ownership arrives at the append wherever
> the caller wrote it — a copy means a genuine second owner, such as a binding
> read again after the append or a cell still holding the list.
> `crates/ply-eval-tests/tests/suite/stdlib_accumulator_cost.rs` asserts the standard
> library's accumulators stay linear, and
> `crates/ply-eval-tests/tests/allocation/accumulator_shape.rs` that an accumulator
> with a second owner does too. A `reuse fn` makes the same claim about one
> function an obligation `ply check` enforces (`E0127`), and the standard
> library's lexer, parser and encoder loops are marked.
**`bytes_slice` and `bytes_split` copy.** `Value::Bytes` is `Arc<[u8]>` with no
slicing, so taking a sub-slice allocates. Response write counts and copies were
**not measured**.

**`--audit-backend` is not free.** `ply test --backend <spec>` attaches a
compiled backend; `--audit-backend` runs each test without it as well, so that a
divergence reported is the backend's and nothing else's. That costs two runs, so
it is off by default. A run with a backend attached neither reads nor writes the
result cache either way.

Two backends ship. `reference` evaluates the body on a machine of its own over
the scalar-signature fragment — not a code generator, and slower than entering
none — and it exists so that a *wrong* backend can be caught before a fast one
is argued about:
`--backend wrong:<mutation>` installs one of eight deliberately wrong backends.
`cranelift` is a real JIT, compiled into the binary with no feature flag.

**It wins narrowly on the front end and loses on the request path, and that is
the finding.** On a compute loop, which is almost entirely inside the fragment,
cranelift is several times faster and enters nearly every call it is offered.
The parser spike parsing the examples now runs inside one native entry per file
and beats the interpreter by a fifth (`benches/front-end`), which says the cost
left is what compiled code does with values rather than what it lowers. On the
request path most of the work is a host and a database, and compiling costs
more per run than entering saves. [ADR 0026](docs/adr/0026-a-reachable-backend.md)
and [ADR 0030](docs/adr/0030-compiled-code-on-the-front-end.md) carry the series
and the conditions; do not re-derive the ratios here, and do not read a loss on
one workload as a bound on the idea.
**The request-path allocation count is large.** One `/health` request makes
**540 allocations and 47,349 bytes** to produce a 107-byte response.

**The profile has a −7.8% residue.** The layer table above sums to 638.96µs
against a measured total of 592.64µs, because six of the rungs are taken in
process against the test double and the total is the real binary against
postgres. It is printed rather than folded into a neighbour — and, being
negative, it is charged *against* the interpreter's share rather than credited
to nobody, because it can only mean the in-process side over-counts.

## What is missing

Each of these is absent by decision rather than by oversight, which does not make
it present.

**Authentication and authorization.** There is a typed-secret API-key comparison
in the example service and nothing else. No sessions, no cookies, no password
hashing, no OAuth, no authorization model. Shipping an auth framework before
there was a database or a secret type would have been shipping a shape nothing
could implement correctly.

**Migrations — and the start-up schema check does not exist.** This paragraph
used to read: "`--db-schema` materialises it, reads `information_schema` and
`pg_constraint`, and refuses to start on any difference — which is most of what a
migration tool is bought for." **That check was never built.** `--db-schema
<module>.<fn>` is a real flag: it resolves the name against the program, checks
the function is nullary and returns a `Schema`, evaluates it, and reads its table
and column counts. It never opens a connection to compare. `information_schema`
appears in this tree only in two test queries; `pg_constraint` appears in prose
only. The error the refusal was named after, `E0435 DB_SCHEMA_MISMATCH`, is
**raised nowhere** — the constant has three occurrences in the whole repository:
its definition, its registry row, and its membership in a reserved-codes list.
The code is more honest than this file was: `ply-cli`'s schema state is a
two-valued `Declared | Verified` whose only `Verified` constructions are in unit
tests, and whose comment reads *"`Declared` is the honest word for 'the program
describes this and nothing compared it to a server'."*

What you actually get is the fallback, and it is real: a statement whose shape
the database disagrees with fails at **prepare** time with `E0433
DB_PREPARE_FAILED`, raised from the driver's prepare path. That is per statement
and on first execution — later and narrower than start-up, and it catches only
the tables and columns your statements actually name. On top of that missing
check there is also no versioning, no up and down, no ordering across deploys and
no diffing a live database into a change script. There is no migrations story
here at all.

**HTTP/2 and HTTP/3.** ALPN advertises `http/1.1` and only `http/1.1`, which is
the honest form of not having them. Every browser offers `h2` first. Also
missing: compression in either direction, WebSockets, `Upgrade`, `CONNECT`,
mTLS, SNI-based certificate selection, and session resumption.

**Any dispatch mechanism** — no typeclasses, no implicits, no instance
resolution. A derived codec is a plain function and you pass it: `respond(order,
order_json())`. [ADR 0010](docs/adr/0010-generic-derivation.md) deferred the
resolution layer and named the risk it was accepting, which is **coherence**:
nothing stops module A calling `order_json` and module B calling
`order_json_v2`, both type-checking, one type serializing two ways, and nobody
noticing until a client breaks.

That risk is not hypothetical, and the divergence found was reachable through a
**type alias**. `type Key = String` made `Map<Key, Int>` and `Map<String, Int>`
one type with two wire formats — two codecs of the same type, interchangeable at
every call site, disagreeing about the protocol, with nothing at the `derive`
line to read. The deriver now resolves a key through the declaring module's own
aliases first, which closes it locally. **It is still open across modules**: an
alias to `String` declared elsewhere gets the other wire format, because
expansion has to be a function of the file or the incremental cache goes stale.
An orphan rule (a `derive` may only name a type its own module declares) is the
coherence there is; it is Rust's rule and it is a local property, not a global
guarantee.

**Also not built:** a query builder or ORM, `LISTEN`/`NOTIFY`, cursors, a time
type, a database per test, a template language, metrics backends, log shipping,
distributed tracing propagation, trace sampling, live config reload, artifact
signing, and secret zeroization.

## Status

M0–M8 and W1–W6 are complete: parse, typecheck with effect inference, content
addressing, evaluation on two engines, incremental testing, delta-debugged
failures, multi-shot continuations, deterministic simulation, specs — and on top
of that a web track that ends in a service you can deploy, observe and stop.

Since then the **memory model** changed, which this section did not say. ADR 0017
replaced ADR 0005's persistent forkable `World` with region-scoped bump arenas:
`World` no longer exists as a type, cells are arena slots, and `with_region[r]`
brands the values allocated in a scope so one escaping it is `E0446` at the
escape site and one reaching a runtime boundary is `E0449`. That is why
[DESIGN.md](DESIGN.md) §2 talks about regions and brands at all.

`cargo nextest run --workspace` passes on an unloaded machine;
`docs/ONBOARDING.md` §2 has the command's provenance and what it does not
prove.

**It is deliberately not described here by a test count.** This paragraph used
to carry one, re-taken through a chain of nested blocks each recording what the
last one read. Every one of those re-takes found the number stale, none of them
was found by anything failing, and the last found it fifteen binaries and 182
tests behind. A figure that only ever moves, that nothing checks, and that has
to be re-taken by hand after any change that adds a test is maintenance, not
evidence. `docs/ONBOARDING.md` §Provenance keeps the counts that have *not*
drifted — the ones that say something about what the language does rather than
about how many tests exist.

A few tests are marked `ignored`. They are timing benchmarks you run on purpose
rather than gates — the ones in `ply-corpus`'s `http_cost` tests and
`interp::tests::a_cached_mention_against_the_allocation_it_replaces` in
`ply-eval`'s lib tests — plus one doc-test, which is not a benchmark. Each
benchmark prints the command that runs it. `docs/ONBOARDING.md` §2 has the wall
clock and its spread, which is wide: readings on this one command have ranged
from three minutes to twenty-nine, and the high ones describe a loaded machine
rather than the tree.

And `ignored` is not the whole of the timing-sensitive suite —
`ply-eval-tests/tests/allocation/region_arena_cost.rs::snapshot_cost_as_a_function_of_region_size`
asserts on a wall-clock growth ratio and runs by default, and it failed for us on
a machine that was busy compiling something else. On a quiet machine it passes.

Read [DESIGN.md](DESIGN.md) for the language and the reasoning,
[ROADMAP.md](ROADMAP.md) for what is built and what each milestone decided,
[docs/adr/](docs/adr/) for the decisions with their arguments,
[CONTRACTS.md](CONTRACTS.md) for the internal crate APIs, and
[docs/BOOTSTRAP-PATH.md](docs/BOOTSTRAP-PATH.md) for what still stands between
Ply and a compiler written in Ply, in the order to take it.

Native codegen is the one planned milestone that has not landed, and it is
deferred on measurement rather than on effort — see
[ADR 0011](docs/adr/0011-the-web-track.md) for the exact number that would
reopen it.

## Building

```
cargo build --workspace
cargo nextest run --workspace
./target/debug/ply test examples/
./target/debug/ply prove examples/desk.ply
./target/debug/ply hosts examples/desk.ply --host
```

## License

MIT OR Apache-2.0 — the texts are [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE) at the repository root, added 2026-08-27. Until
then neither file existed, and this line and `Cargo.toml:22` declared a licence the
repository did not ship; `CONTRIBUTING.md` §"Things known to be broken" item 7 is
where that was recorded. The thirteen workspace members inherit the expression with
`license.workspace = true`.
