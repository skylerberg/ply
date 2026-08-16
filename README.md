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

Every number below was taken on one machine — Apple M-series, macOS 24.6.0,
release profile, PostgreSQL 18.3 — and is reproducible from this repository. The
serving numbers are one run, in `benches/w6-ladder.json` and
`benches/w6-spike.json`, printed by `ply-corpus w6`; the loop numbers are another,
from `ply-corpus gen` and `ply test`. Nothing here is quoted from an earlier
milestone: the two files are written by the commands that took them, and two
tests fail if the tree stops matching what they say.

## The loop

On a generated project of **200 modules, 10,000 definitions and 5,000 tests**
(4.7 MB of source, 157 of the tests nondeterministic and therefore never
cacheable):

```
$ ply test                                  # empty cache
   0 failed, 5000 passed, 0 cached                      0.86s wall

$ ply test                                  # nothing changed
   selected 157 of 5000 (4843 cached)
   0 failed, 157 passed, 4843 cached                    0.42s wall
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

And where a warm run's 311ms of work actually goes:

| phase | ms | share |
| --- | --- | --- |
| typecheck | 157.5 | 50.7% |
| hash | 73.1 | 23.5% |
| parse | 58.5 | 18.8% |
| **execute** | **10.4** | **3.3%** |
| cache open | 1.9 | 0.6% |
| select | 0.4 | 0.1% |

Two things follow, and the second is more interesting than the first. Running the
tests is 3.3% of a warm loop, so a faster evaluator would buy almost nothing here
— that is the argument that has kept native codegen deferred for nine milestones,
and [Serving](#serving) is where it inverts. And opening a content-addressed
store of 10,000 definitions costs 1.9ms, which is what lets the cache sit in the
inner loop rather than be a build artifact.

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
   ╭─[ src/user.ply:8:13 ]
   │
 8 │   assert_eq(stamp(), stamp())
   │             ───┬───
   │                ╰───── reaches `clock.read`, and `clock` is declared `nondet`
   │
 7 │ test "it stamps" {
   │      ─────┬─────
   │           ╰─────── test `it stamps` is deterministic
   │
   │ Note 1: `clock.read` is performed inside something this expression calls
   │ Note 2: handle it here, e.g. `handle <body> with { clock.now() -> <value> }`
   │ Note 3: or declare this `test/nondet`, which opts out of the cache and
   │         re-runs every time
```

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
`property` with their case and rejection counts. There is no `tier` field
anywhere — a tier is computed from the evidence, so a `proved` that was really a
sample cannot be asserted. Coverage is in the default output: 167 of 180
definitions carry no obligation, and it says so without a flag.

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

| layer | µs | share |
| --- | --- | --- |
| entering the interpreter at all | 0.09 | 0.0% |
| the route's own body — a memo hit, see below | 1.15 | 0.2% |
| HTTP/1.1 framing: request line, field block, response encode | 97.83 | 16.5% |
| routing: matching one path against the eleven-route table | 51.04 | 8.6% |
| the serving loop: recv, perform, handler walk, send, keep-alive | 105.55 | 17.8% |
| the socket, the reactor, the blocking pool | 46.19 | 7.8% |
| the TLS record layer, steady state | 3.73 | 0.6% |
| postgres: the boundary, the wire and the server | 327.55 | 55.3% |
| tracing: encoding a JSON record and writing it | 5.84 | 1.0% |
| *residue — everything no substitution separated* | *−46.32* | *−7.8%* |
| **total, measured** | **592.64** | |
| the same syscalls in Rust answering the same bytes | 15.68 | the request is **37.8x** this |

And what a client sees, at concurrency 1 with a 41-byte request head, each row
against a Rust floor replaying **that row's own response**:

| workload | req/s | p50 | p99 | vs floor |
| --- | --- | --- | --- | --- |
| no database, plaintext | 2,860 | 344µs | 439µs | 23.0x |
| one select, plaintext | 1,715 | 576µs | 636µs | 37.2x |
| one select, TLS | 1,704 | 576µs | 623µs | 37.4x |
| one select, TLS, tracing to JSON | 1,687 | 581µs | 650µs | 37.8x |
| no database, TLS, tracing to JSON | 3,778 | 254µs | 289µs | 17.4x |

Read those together and the picture is unusual. **TLS costs 0.6% of a request
and tracing to JSON costs 1.0%** — the two features that sound expensive are
about 1.6% between them, and neither difference is bigger than the spread of its
own repeats, which the table in the ADR prints beside it. There is no I/O to
hide behind either: the whole socket layer is 7.8%, and the interpreter is
**35%** of a request after the measurement's own seam is charged against it.

The two numbers to be suspicious of are the endpoint's 1.15µs and the database's
55%. The endpoint is a *memo hit*: `/health`'s whole body is a nullary pure
definition, and the constant memo evaluates one of those once per process. In
the previous take of this table that row was **127µs** — the route table was
rebuilt from its pattern strings on every request — and removing it is worth
**1.77x on `/health` and 1.15x on `/items`**, end to end, on the real binary.
That is the kind of thing this project keeps finding: the cheap algorithmic fix
beats the expensive execution-strategy fix. It has now happened three times.
The database's 55% is large partly because `/items`' own JSON encode sits inside
that rung, which the ADR discloses rather than nets out.

Which is why there is still no code generator. A Cranelift spike compiled the
innermost scanning loop of the HTTP parser and hit **11.67x** on its weakest
input, agreeing with both interpreters on every input first. Applied to a 35%
interpreter share that projects **1.48x** end to end — under the 1.50x bar fixed
before any of these numbers existed, and under the 1.55x ceiling an infinitely
fast backend would have. Three of the four criteria now fail. The full argument,
including three measured reasons to think even the 1.48x is optimistic, is
[ADR 0016](docs/adr/0016-w6-performance.md).

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
re-evaluated per call. Measured: disabling the memo by source substitution costs
**1.77x** on `/health` on the sequential loop and **1.00x** on the
task-per-connection one, where there is nothing left to disable — the same
service is 263.5µs a request sequentially and 470.6µs spawning.

**A real browser's request head costs 1.5x a `curl`-sized one.** The throughput
table above is taken with a 41-byte head. Measured separately, over the in-memory
store at 40 concurrent connections: a two-field, 63-byte head costs 987.3µs per
request and a thirteen-field, 569-byte browser head costs 1,472.4µs — 1,013 req/s
against 679. Cost is proportional to *fields parsed* rather than to bytes
received, which is the good regime; a browser simply sends more fields. A req/s
number quoted without its head length is worth less than it looks, and that
includes the one at the top of this file.

**No cancellation, no backpressure, no load shedding.** A request still live at
the drain deadline loses its connection with no response and the process exits
`3`. An overloaded service queues until something times out. Backpressure was
promised by one milestone and explicitly withdrawn by the next; it is not
subtle, it is absent.

**The in-memory test double is slower than the database it replaces.** In
process the twin's `/items` handler is **544.6µs** a call, **344.9µs** of which
is the memory engine parsing its SQL in Ply, and every twin clause writes its
whole state back through a persistent map. Tests that use the double
for speed will be disappointed; they use it for isolation and determinism, which
it does deliver.

**The tracing sink is quadratic in a test.** `std.trace`'s `Sink` appends with
`push`, so a collecting twin holding N records is O(N²). W5 measured it; W6 did
not re-take it.

**`bytes_slice` and `bytes_split` copy.** `Value::Bytes` is `Arc<[u8]>` with no
slicing, so taking a sub-slice allocates. Response write counts and copies were
**not measured**.

**`--engine both` is not free.** The guarantee that the tree-walking evaluator
and the control-stack machine agree costs two runs, and the two are not the same
speed: the tree-walker is 2.73x faster on the request path.

**The request-path allocation count is large.** One `/health` request makes
**1,035 allocations and 0.124 MB** to produce a 107-byte response.

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

**Migrations.** A schema is a value, and `--db-schema` materialises it, reads
`information_schema` and `pg_constraint`, and refuses to start on any difference
— which is most of what a migration tool is bought for. But there is no
versioning, no up and down, no ordering across deploys and no diffing a live
database into a change script. Calling that a migrations story would be
generous.

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
`cargo test --workspace` runs 3,206 tests across 123 binaries; all pass, and the
four marked `ignored` are timing benchmarks you run on purpose.

Read [DESIGN.md](DESIGN.md) for the language and the reasoning,
[ROADMAP.md](ROADMAP.md) for what is built and what each milestone decided,
[docs/adr/](docs/adr/) for the decisions with their arguments, and
[CONTRACTS.md](CONTRACTS.md) for the internal crate APIs.

Native codegen is the one planned milestone that has not landed, and it is
deferred on measurement rather than on effort — see
[ADR 0016](docs/adr/0016-w6-performance.md) for the exact number that would
reopen it.

## Building

```
cargo build --workspace
cargo test --workspace
./target/debug/ply test examples/
./target/debug/ply prove examples/desk.ply
./target/debug/ply hosts examples/desk.ply --host
```

## License

MIT OR Apache-2.0
