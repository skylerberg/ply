# Benchmarks

`ply-corpus` generates a synthetic Ply project of a given size and reports where
a run's wall clock goes. The corpus is not a toy: it compiles, typechecks and
passes `ply test` before any number is taken, and generation fails loudly if it
does not.

```
benches/run.sh                       # the default size ladder, 250 → 10,000 definitions
benches/run.sh 40,25,500             # one size: modules,defs_per_module,tests
PLY_BENCH_REPEATS=3 benches/run.sh   # slower, less noise
```

Or drive the binary directly:

```
cargo run --release -p ply-corpus -- gen --out /tmp/c --modules 200 --defs-per-module 50 --tests 5000 --depth 6
cargo run --release -p ply-corpus -- bench /tmp/c --repeats 3 [--json]
cargo run --release -p ply-corpus -- sweep --out /tmp/sweep --sizes "40,25,500 80,25,1000"
cargo run --release -p ply-corpus -- measure /tmp/c --repeats 3 [--json]
```

## What is measured

Nine phases, timed separately: discover, read, parse, resolve, typecheck, hash,
cache open, select, execute. The split is the point — a total tells you nothing
about a system whose thesis is that most of the work should be skipped.

Five scenarios, each with the source tree and the cache restored afterwards so
the order they run in cannot leak into the numbers:

| scenario | state |
| --- | --- |
| `cold` | cache cleared before every repeat; every test runs |
| `warm` | nothing changed; every deterministic test is a cache hit |
| `rename` | a top-level definition renamed corpus-wide |
| `edit-leaf` | one definition's body changed, few dependents |
| `edit-hub` | a widely depended upon definition's body changed |

The `rename` and `edit-*` mutations are value-preserving by construction, so a
mutated corpus still passes and a scenario never measures failure formatting.

## What `measure` adds

`bench` reports a whole run and therefore hides which engine cost what: a
worker is built per rayon thread per concurrency group, so a setup cost
proportional to the program size is charged many times over and reads as
interpreter speed. `measure` separates the two and prices the claims ADR 0005
makes on their own:

| section | question |
| --- | --- |
| throughput | the two engines on one corpus, one worker, one thread — worker setup, a first pass, and a steady pass apart from each other |
| fork | `World::fork` against rebuilding the same fixture, at five world sizes |
| multi-shot | resuming zero, one, two and four times, plus `Stack::capture` and `Stack::resume` against pending-frame count |
| scheduling | world-isolated against shared tests, and the groups the shared ones alone need |
| `Store::open` | against the cache the corpus has already filled |

`--engine <treewalk\|machine>` restricts the throughput table to one engine and
`--only-throughput` drops the rest, which together is what to point a profiler
at. `crates/ply-corpus/tests/frame_cost.rs` counts the allocations a frame push
and pop cost, which is the machine-independent half of the same question.

## What `sim` adds

`measure` prices the machine ADR 0005 built; `sim` prices the search ADR 0006
built on top of it. It drives the real scheduler through the real machine, and
the one thing it does that `ply test --measure-reduction` cannot is choose the
root set per trial — which is what makes a median over many trials possible.

```
cargo run --release -p ply-corpus -- sim <corpus> [--trials N] [--budget N] [--json]
```

| section | question |
| --- | --- |
| exploration | interleavings pruned, the same search with the recording's clocks withheld, and an unpruned enumeration — three columns, one driver |
| race finding | interleavings to the first failure, `dpor` against sampling, median and worst over `--trials` roots, with misses counted rather than dropped |
| throughput | seeds per second, where one seed is one whole test replayed from a fresh world |

The middle column of the exploration table is the honest way to price the
happens-before filter without checking out the tree that predates it: an empty
stamp is documented to mean "no synchronization known", so a driver that clears
the stamps reproduces the older search exactly.

A corpus generated with `--concurrent-tests` at a chosen `--conflict-density` is
what the reduction is measured against; `--tasks-per-test` and `--steps-per-task`
are the two exponents the schedule count grows in.

## What `serve` adds

`bench`, `measure` and `sim` all price a *test* run, where execution is a few
percent of the wall clock and the cache is the whole point. Serving inverts
that: there is no front end on the request path, so the interpreter is the
request path. `serve` is the number W6's decision on M9 turns on.

```
cargo run --release -p ply-corpus -- serve --repo . [--requests N] [--concurrency 1,8,32] [--json]
cargo run --release -p ply-corpus -- serve --repo . --baseline --load-headers 0,8
```

`--baseline` adds W1's `fold`-based scans as a second column everywhere, so the
before and after of the byte builtins are one table taken on one machine rather
than a number quoted from a milestone ago.

It needs no corpus. The program under measurement is `examples/hello.ply`
itself, read from `--repo` and rewritten only in its port and connection count,
so what is timed is the endpoint W1 shipped rather than a copy of it that can
drift.

Two tables. The first separates the layers of one request **by substitution
rather than by instrumentation** — every rung runs the same endpoint and changes
only what is underneath it, so a difference between two rows is that layer's
cost and no timer inside the machine has to be trusted:

| rung | under the endpoint | the layer it adds |
| --- | --- | --- |
| `answer` | nothing; a pure call | the HTTP parse and the response build |
| `ply-handler` | a `handle` written in Ply | performing `net.*` and dispatching a clause |
| `host-sim` | the `SimNet` host handler | the host boundary: resolve, footprint check, decode |
| `host-tcp` | the `TcpHost` host handler | the socket and the blocking pool |
| `rust-floor` | no Ply at all | the denominator: the same syscalls with no interpreter |

`ply-handler` and `host-sim` serve the same operation sequence over the same
connection count, which is what makes their difference ADR 0008 §5's twin
comparison priced rather than a comparison of two different programs.

The second is the head-length sweep, and it is what ADR 0012 §5's exit
criterion is stated in: the same `answer` over heads grown by adding header
lines the parser never reads, so every point parses the same three fields and
differs only in how much buffer a scan crosses. The table prints the ratio it
found at both ends rather than a pinned number, because the ratio is the claim
and the machine is not: at 84 times the length W1's folds cost tens of times as
much and the byte builtins cost about once as much. A `µs/byte` column that
falls as the head grows is the claim — a request's cost is a function of fields
parsed rather than of bytes received.

The third drives the real `ply` binary over loopback from client threads and
reports what a client observed — throughput and p50/p95/p99 — for the sequential
endpoint, for a task-per-connection variant on the production scheduler, and for
a Rust server answering the same bytes. Latency there is client-observed and
includes the client, which is why the `rust-floor` row is on the same table
rather than in prose. `--load-headers` says how long a head the client sends at
each point, and the column is printed: a load number taken at one head length
says nothing about another under W1's scans, and very nearly everything under
W2's.

**One request is not one number.** The endpoint's cost was a function of head
length while every scan folded over the buffer, so a `serve` table is only
meaningful beside the request it was taken with. `REQUEST` is a 63-byte head;
a browser sends five to ten times that, which is `--load-headers 8`.

## What `payload` adds

`serve` prices a request with no body. `payload` prices what W2 put on the path
once there is one: a derived JSON codec, `Map`, and derivation's own cost to the
front end and the cache.

```
cargo run --release -p ply-corpus -- payload [--lines 1,40,200] [--shape 40:0,40:1600] [--json]
```

| section | question |
| --- | --- |
| JSON | encode and decode megabytes per second through a *derived* codec, at payload sizes an endpoint receives |
| shape | whether a decode is priced by the fields it visits or the bytes it crosses, with `json::parse` timed apart from the codec above it |
| `Map` | insert, get, `map_keys` and `map_fold` at four sizes, with the `fold` scaffold subtracted **and printed** |
| order | `map_keys` compared across separate processes and across three insertion orders |
| derivation | two projects differing only in whether their types carry a `derive`: definition count, cache size, cold and warm check, cold and warm test |

The shape table is the head sweep's question asked of a payload: `--shape
lines:pad` holds the line count still and widens one string field, so the field
count is fixed down a column while the byte count grows. A flat `µs/req` down
that column means the cost is the fields; a rising one would mean a scan is
still crossing the buffer per byte.

## What `w3` adds

`serve` prices one endpoint with no routing, no body and no connection reuse.
`w3` prices the service W3 shipped — `examples/desk.ply`, ten routes, real
HTTP/1.1 framing, keep-alive and TLS — and re-takes `serve`'s field-proportional
sweep against it, because full framing is the thing most likely to have put a
per-byte cost back on the request path.

```
cargo run --release -p ply-corpus -- w3 --repo . [--json]
cargo run --release -p ply-corpus -- w3 --repo . --no-load          # the in-process half
cargo run --release -p ply-corpus -- w3 --repo . --concurrent       # a task per connection
cargo run --release -p ply-corpus -- w3 --repo . --w2-baseline      # W2's endpoint on the same machine
```

| section | question |
| --- | --- |
| routes | throughput and tail latency at each concurrency, over a mix of the read routes |
| stages | where one request's time goes: the route table, the match, the framing, one endpoint, the encode |
| per route | each of the ten routes on its own, so a mix has a decomposition rather than an average |
| fields or bytes | three axes — header bytes at a fixed field count, header count, and body bytes |
| keep-alive | the same work over one to a hundred requests per connection |
| TLS | one route over both transports, with the handshake timed apart from the request |
| aliases | `/ {Desk}` against its expansion, compared as hashes, as stored bytes and as footprints |

Two things about the tables are load-bearing:

**Read the handshake off the concurrency-1 rows.** `desk.ply` serves one
connection at a time and the TLS handshake completes on the server's first
`recv`, so a client that connected while the server was busy times the queue and
not the cryptography.

**The last request on every connection carries `Connection: close`.** That is
what a client does, and it is also what keeps the run from exhausting the
ephemeral port range: whichever peer closes first holds the port in `TIME_WAIT`
for twice the segment lifetime, and a client opening a thousand connections a
second cannot afford to be that peer.

The load sections drive the real `ply` binary over loopback. The stages, per
route and shape sections run in process over `SimNet`, because what they price
is the parse, the route and the encode, and a syscall in the middle of that has
a bigger variance than the thing being measured.

## What `w4` adds

`w3` prices a service whose store is a value in a cell. `w4` prices the one W4
put a real postgres behind, and every section of it is a *substitution* rather
than a total: a query against loopback postgres costs tens of microseconds of
server and protocol time whatever issued it, so the only number that says
anything about the language is the difference between two rows that differ in
one layer.

```
cargo run --release -p ply-corpus -- w4 --db postgres://ply@127.0.0.1:5433/bench --no-load
cargo run --release -p ply-corpus -- w4 --repo . --db postgres://ply@127.0.0.1:5433/desk \
    --no-ops --no-sizes --no-pool          # the load half, against the desk's own schema
```

| section | question |
| --- | --- |
| `ops` | one statement through the boundary at several concurrencies, against the same statement issued by `tokio-postgres` with no Ply in the path, and against the twin |
| `sizes` | one `order by … limit 1` against the rows it sorts, on the twin and on postgres — the axis a per-statement comparison hides |
| `pool` | throughput against pool size, and what a pool smaller than the open scopes does |
| `crud` | a route that hits the database against one that does not, over the real binary and a real socket, on both stores |

Three things about it are load-bearing:

**It manages no database.** `--db` names one this harness may create and drop a
table called `part` in; the `crud` section additionally expects
`examples/desk.sql` to have been loaded, because `--db-schema desk.schema` is
passed and a start-up refusal is the point of that flag.

**The twin's fixture is subtracted and printed.** `std.db`'s memory engine
builds its tables through its own scanner, which costs tens of milliseconds and
is setup rather than an operation. The `twin fixture` row is that cost and every
`ply-twin` row has it removed.

**`E0437` is a deadline and not a capacity check.** A pool of one with
thirty-two open scopes completes, because acquisition queues; it refuses only
when a caller waits longer than `--db-acquire-ms`. The exhaustion table prints
both sides for that reason.

## What `w5` adds

`w3` priced HTTP and `w4` priced the database. `w5` prices what it takes to
*operate* the result: a trace call, a stop, and a deploy.

```
cargo run --release -p ply-corpus -- w5 --no-served --no-drain --no-transaction
cargo run --release -p ply-corpus -- w5 --repo . --concurrent \
    --db postgres://ply@127.0.0.1:5433/desk
```

| section | question |
| --- | --- |
| `events` | one trace operation under each sink, against the same loop performing none |
| `served` | the same routes under the same load with only `--trace` moved, over three stacks |
| `drain` | how long a stop takes with N requests in flight, and what the deadline does to them |
| `transaction` | whether a transaction open at the deadline commits, rolls back, or is lost |
| `deploy` | the artifact's bytes, the binary's bytes, and what an incremental transfer would have saved |

Four things about it are load-bearing:

**`bare` is not a configuration a Ply service can be in.** There is no disabled
path — a row cannot be conditional on a flag — so `bare` is
`crates/ply-corpus/ply/w5.ply`'s loop with the `perform` *deleted*, which is what
a level check at the call site buys every other language. The gap between it and
`discard` is what a service pays, on every request, forever, for tracing it
turned off, and it is the number ADR 0015 §1.4 owes rather than promises away.

**The twin's row is taken at a smaller count, and the count is printed.**
`std.trace`'s `Sink` appends with `push`, so holding N records is O(N²); a twin
row taken at twenty thousand would be a number about list append. A twin lives
inside one test holding tens of records, which is the size it is priced at.

**A collecting sink never writes into a pipe.** A pipe nobody drains fills at
64KiB and blocks the writer, so a server writing one JSON line per request into a
piped stderr stops serving partway through a point. Every collecting row hands
the run a file or `/dev/null`, and the `records` column is the lines that
actually arrived.

**The transaction section asserts against the database, never the driver.** A
second session holds a row lock so the desk's `UPDATE` blocks with its `INSERT`
already done, and the outcome is read from the table and from
`pg_stat_activity`. The order *sequence* is read too, because a sequence is not
transactional: without it, a table whose row count did not move is equally
consistent with a transaction that never wrote anything.

## Reproducing a corpus

A corpus is a pure function of its spec and seed, both recorded in the
`corpus.json` the generator writes next to the sources. Re-running `gen` with
the same flags reproduces it byte for byte, which is why the generated trees are
gitignored.
