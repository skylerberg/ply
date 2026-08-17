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
`w3` prices the service W3 shipped — `examples/desk.ply`, eleven routes, real
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
| per route | each of the eleven routes on its own, so a mix has a decomposition rather than an average |
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

## What `w6` adds

`serve`, `w3`, `w4` and `w5` each priced one milestone against the thing it
replaced. None of them priced the **total**, and the total is what decides
whether M9 comes forward. `w6` assembles the accumulated stack into one table
and applies the criteria `docs/adr/0016-w6-performance.md` §2 pins.

```
cargo run --release -p ply-corpus -- w6 benches/w6-ladder.json benches/w6-spike.json
cargo run --release -p ply-corpus -- w6 benches/w6-ladder.json benches/w6-spike.json --json
```

Those two files are the W6 run itself, kept in the repository: `w6-ladder.json`
is the nine rungs, the engine substitution, the offerings, the limits and the
alternatives, and `w6-spike.json` is the codegen spike's half. Neither contains a
verdict or a threshold — a test asserts that a serialized report carries
neither — so the decision printed above them is recomputed from
`w6::Criteria::default()` on every run.

It takes no measurement of its own. It merges one or more measurement files
field by field — so the ladder run and the spike run produce their halves
independently — assembles the ladder, applies the criteria, and prints the
tables, the verdict and an audit of what the report still owes.

**Nine rungs, each one substitution, each measured both ways.** A rung carries
`with_micros` and `without_micros` taken in the same arena in the same run, so a
layer is their difference and no timer inside the machine has to be trusted:

| rung | `with` against `without` | the layer |
| --- | --- | --- |
| `call` | a function returning a constant, against not calling the machine | entering the interpreter at all |
| `endpoint` | the route's handler over an in-memory store, against that constant | the route's own body, through its derived JSON encoder |
| `framing` | `parse_head` and `encode` around it, against without | HTTP/1.1 framing |
| `routing` | `table()` and `route_of()` above it, against without | the route table build, and one match |
| `machine` | the whole `serve_one` over `SimNet`, against calling the pieces | recv, the handler walk, the perform, send, teardown |
| `socket` | the real TCP host under the same loop, against `SimNet` | the socket, the reactor, the pending token |
| `tls` | `--tls`, against plaintext | the TLS record layer, handshake excluded |
| `database` | `run`, against `run_memory` | postgres, the wire, and the server |
| `tracing` | `--trace json` to `/dev/null`, against `--trace off` | the sink |

Four things about the table are load-bearing:

**The residue is printed.** The total is *measured* end to end, not summed, and
`total − Σ layers` gets its own row. Folding it into the nearest plausible layer
would be claiming an attribution the measurement did not earn.

**A positive residue is not credited to the interpreter, and a negative one
is.** A positive residue is time no substitution separated, so leaving it out of
the numerator makes the share a lower bound. A negative residue is the opposite
fact — the layers sum to more than the request they were read against, which can
only be the in-process arena over-counting against the served denominator — and
leaving *that* out would leave the numerator inflated in the direction M9's case
rests on. `Ladder::conservative_share` charges it back, and it is the share
`decide` reads.

**A negative layer is a result, not a rounding error.** It means the
substitution did not isolate what it claimed to, and a ladder carrying one above
5% of the total is `Undecided` rather than decided.

**Every rung carries the worst of its repeats as well as the best.** A layer is
a *difference* between two numbers, so a rung whose layer is 1% of either side
carries both sides' noise; the band is printed beside the layer, a rung whose
band spans zero is an audit finding rather than two printed decimals, and the
interpreter share is printed as the range its own repeats produced. A share
whose band falls on both sides of a criterion's bar is `Undecided`: that ladder
answers whichever run was taken.

**Every rung names the route it was taken on**, and so do the floor and the
total. Two rungs on two routes have a difference that is a route change as well
as a layer, and a `total / floor` whose two sides answer different bytes is not
a multiple — so `Denominators` spells out what each side did and the audit
reports a report that leaves them blank.

**And the criteria are in code, not in the file.** `ply_corpus::w6::Criteria`
holds the thresholds and `ply_corpus::w6::LEVERS` holds ADR 0016 §4's seven
cheaper levers, which is what C3 — "every alternative in §4 is priced" — is
checked against. Checking it against the file's own list made an empty list
satisfy it vacuously, so deleting one field of the measurement file turned a
deferral into an advance; the roster in code is why it cannot. A lever counts as
priced only with a ratio **and** the sentence saying what the ratio is between,
because `priced: true` is a boolean somebody can type. A `Report` still carries
no criteria field and no verdict field, and a test asserts that a serialized one
contains neither.

### Taking the ladder

`w6` judges a report; `w6-ladder` takes one.

```
cargo run --release -p ply-corpus -- w6-ladder \
    --repo . --db postgres://ply@127.0.0.1:5439/desk \
    --requests 512 --iterations 2000 --repeats 3 \
    --concurrency 1 2 4 8 16 32 --per-conn 32 --requests-per-point 2500 \
    --served-repeats 3 --machine "Apple M-series (macOS 24.6.0)" \
    --postgres "PostgreSQL 18.3" --out benches/w6-ladder.json
cargo run --release -p ply-corpus -- w6-ladder --no-served --only sim --requests 200000
```

**That command writes `benches/w6-ladder.json`** — the whole file, not a
fragment of one: the nine rungs, the engine substitution, the offerings, the
limits, the §4 roster with whatever this run priced of it, and the
`not_measured` list. It used to emit a differently shaped document with an empty
`alternatives` array, and the shipped file was assembled by hand around it; a
contributor who followed the staleness guards' "re-take the ladder" would have
dropped the evidence C3 is decided against. A file the command cannot reproduce
is a file nobody can re-take.

It runs two other binaries the way a reader would: `ply`, for the served rows,
and `w6-alloc`, which counts what one request allocates. `w6-alloc` is its own
binary because a counting `#[global_allocator]` is a whole-binary decision and
`ply-corpus` is where the clocks are.

Rungs 1–6 run in this process against `examples/desk.ply` with a driver
appended; rungs 7–9 start the real binary with one flag moved. `--only` runs one
phase on its own and prints what it cost, which is what a sampling profiler is
pointed at. `--detail` writes the raw in-process and served rows beside the
report. `benches/w6-ladder.json` is one such run, which `w6` renders and judges:

```
cargo run --release -p ply-corpus -- w6 benches/w6-ladder.json benches/w6-spike.json
```

Five things about how the rungs are taken differ from the shape ADR 0016 §1.2
sketches, and each is a measurement rather than a preference:

**The endpoint rung is a memo hit, because `/health`'s whole body is a
constant.** `health()` takes no parameters and performs nothing, so the constant
memo evaluates it once per process and every later request reads the remembered
value. That is what a served `/health` request really costs, so it is what the
rung reports — and it is why the `/items` handler over the twin is measured
beside the ladder, as the closest thing here to a route body's own cost.

**Rungs 1–6 are taken on `/health`, not `/items`.** The ADR anticipates this: a
pure call to the `/items` handler needs a store, and the only store available in
process is `std.db`'s memory engine, which parses its SQL in Ply on every call.
That scanner is on no served request path, so putting it inside the `endpoint`
layer would price the twin. It is measured on its own instead — `w6_items` in
the driver — and reported beside the ladder.

**The `database` rung is `/items` against `/health` on one postgres binary**,
not `run` against `run_memory`. For the same reason: the twin's `without` is
dearer than the postgres `with`, so the ADR's substitution comes out negative
and prices `std.db` rather than the database. Both numbers are taken.

**Every rung reuses a connection for 32 requests**, as the served rows do. A
rung that opened a connection per request carries an accept, a close and a
`serve_connection` set-up per request that the total is not paying, and the
difference lands in the residue as a negative number.

**The ladder is read off the sequential accept loop, not the
task-per-connection one ADR 0016 §1.6 pins.** `--accept sequential|task-per-conn`
chooses, and it defaults to `sequential` for a measured reason: `task.spawn`
opens a production region for the life of the server and the constant memo is
refused inside any open region, so a spawning service memoizes nothing while the
in-process rungs, which are one connection at a time, do. Reading a memo-active
numerator against a memo-inert denominator would put the two arenas in different
regimes. **Both** loops are swept on every run either way — the one the ladder is
not read off becomes its own labelled offering rows, which is what §1.6 asks —
and what the difference costs is a row in the limits. Both sides of every served
rung come from the **same** concurrency, the one the total was read off, so a
layer is one flag moved rather than one flag moved and two rows selected; and
where the throughput curve is flat, the row is the lowest concurrency within 5%
of the best rather than whichever point noise favoured.

## What `regions` adds

Every other section prices something that has been built. `regions` prices
something that has not: ADR 0017 §6 removes the forkable world, and the one
thing that costs is tests which parallelize today *because* each got its own
world and would be grouped by footprint conflict without one.

```
cargo run --release -p ply-corpus -- regions examples --jobs 8
cargo run --release -p ply-corpus -- regions <corpus>... --jobs 8 --json
cargo run --release -p ply-corpus -- regions --hypothetical 176:1,176:8,176:176
```

It colours the same test set twice — once with the world-backed exemption
`ply_test::shared_footprint` applies today, once without it — and reports the
group count, the critical path and a modelled makespan for each.

Four things about it are load-bearing:

**Only `cell` moves.** `ply_test::WORLD_BACKED` is exactly `["cell"]`, so that
exemption is the whole of what forking buys the scheduler. `AMBIENT` —
`sim.read`, a seed — is a claim about inputs rather than about memory and stays
exempt on both sides; dropping it too would report a loss ADR 0017 does not
cause, and every simulated test in the corpus would be in the number.

**The colouring is the runner's.** `regions::colour` is
`ply_test::group_by_conflict` with the projection lifted out, and both a unit
test and `tests/region_isolation_cost.rs` assert it reproduces that function
exactly on the projection `ply-test` applies. On `examples/` it reproduces the
five groups and the `[179, 1, 2, 2, 2]` sizes `ply test --explain` prints.

**A group is a barrier.** `ply_test::run` finishes one group before starting the
next and builds a worker per pool thread per group, so a schedule is not
`sum / jobs` and an extra group is not free. `regions::makespan` replays the
counter `execute_group` hands indices out on, and charges the worker.

**The modelled absolute is low and the ratio is not.** Per-test durations are
taken at one job, so they carry none of the contention eight concurrent workers
put on the allocator; the modelled makespan runs 50–95% under the measured one
and the run prints both. Both colourings are modelled from the same durations,
so the error is in the absolutes and cancels out of the ratio between them.

`--hypothetical cells:labels` appends a corpus that does not exist — `cells`
tests carrying a `cell` atom spread over `labels` region labels — because the
measured corpora carry none at all and a risk that is only ever reported as
zero is a risk nobody can size.

## Reproducing a corpus

A corpus is a pure function of its spec and seed, both recorded in the
`corpus.json` the generator writes next to the sources. Re-running `gen` with
the same flags reproduces it byte for byte, which is why the generated trees are
gitignored.
