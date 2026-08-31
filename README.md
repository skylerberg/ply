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

Every number below was taken on one machine — Apple M-series, macOS 24.6.0,
release profile, PostgreSQL 18.3. The serving numbers are one run, in
`benches/w6-ladder.json` and `benches/w6-spike.json`, taken by `ply-corpus
w6-ladder` and *rendered and judged* by `ply-corpus w6`; the loop numbers are
another, from `ply-corpus gen` and `ply test`. The two files are written by the
commands that took them, and two tests fail if the tree stops matching what they
say — `w6_report_integrity::the_shipped_ladder_still_describes_the_tree_it_ships_in`
and `w6_report_allocations::the_shipped_allocation_evidence_still_describes_this_request_path`.

**Where a number here has been checked, this file says so.** A documentation
audit re-ran what could be re-run without a postgres server, and the corrections
are inline, marked, and keep the original claim beside the measurement rather
than quietly dropping it. Two things a reader should know up front: the warm-loop
`execute` figure was wrong by 12x and is corrected below, and the start-up
database-schema check described under [What is missing](#what-is-missing) was
never built. The serving profile and the throughput table reproduce exactly
against the committed measurement files.

## The loop

On a generated project of **200 modules, 10,000 definitions and 5,000 tests**
(4.64 MiB of source — 4,862,564 bytes, which the generator prints as 4748 KiB;
"4.7 MB" here was neither the binary nor the decimal reading of it — 157 of the
tests nondeterministic and therefore never cacheable):

```
$ ply test                                  # empty cache
   0 failed, 5000 passed, 0 cached                      2.60s wall

$ ply test                                  # nothing changed
   selected 157 of 5000 (4843 cached)
   0 failed, 157 passed, 4843 cached                    0.52s wall
```

> **Corrected: the cold figure read `0.86s` and does not reproduce.** Measured
> here with `/usr/bin/time` on the corpus below, after `ply cache clear`: **2.60s**
> cold and **0.52s** warm. The counts — 5000, 157, 4843 — reproduce exactly, and
> so does everything the front end does; it is the same `execute` under-reporting
> the phase table below documents, and it moves the cold run by 3x. The warm
> figure was close enough to stand (0.42s published, 0.52s measured).

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

And where a warm run's work goes. `ply-corpus bench` times **nine** phases; all
nine are here, because a column that does not add up to its own total is the one
thing a reader cannot check:

| phase | ms | share |
| --- | --- | --- |
| typecheck | 162.8 | 37.2% |
| **execute** | **125.1** | **28.6%** |
| hash | 77.5 | 17.7% |
| parse | 60.0 | 13.7% |
| discover, read, resolve | 9.3 | 2.1% |
| cache open | 2.0 | 0.5% |
| select | 0.5 | 0.1% |
| **total** | **437.2** | |

> **Corrected: the `execute` row was wrong by 12x, and it was the load-bearing
> one.** This table used to publish `execute` at **10.4ms / 3.3%** against a
> 311ms total, and to list only six of the nine phases. Re-taking it on the
> documented corpus — `ply-corpus gen --out <dir> --seed 1 --modules 200
> --defs-per-module 50 --tests 5000 --depth 6`, which reproduces byte-identically,
> then `ply-corpus bench <dir> --repeats 3` twice — gives the numbers above (this
> recipe omitted the **required** `--out` and `bench`'s positional corpus
> argument, so as printed it failed with `error: the following required arguments
> were not provided: --out <OUT>`; both are restored) (the two runs agreed to
> within 3% on every row). **This is not a machine-speed artifact.** Every other
> phase reproduced the published figure closely: parse 60.0 against 58.5,
> typecheck 162.8 against 157.5, hash 77.5 against 73.1, cache open 2.0 against
> 1.9, select 0.5 against 0.4 — and the three phases the old table omitted sum to
> 9.3ms against the 9.2ms its own total implied. One row moved, by 12x, and the
> total moved with it. The corrected 437ms total is also the one consistent with
> the 0.52s warm wall clock measured above; 311ms never was.
>
> One caveat in the other direction, from `benches/README.md`: `bench` builds a
> worker per pool thread per concurrency group, so its `execute` phase carries
> setup charged per group and **over**-states interpreter time. The true cost of
> running the tests is bounded below by that caveat and above by 28.6%; what it
> is not is 3.3%. `ply-corpus measure` is the harness that separates the two.
>
> The selection table above this one reproduced exactly — 157, 157, 158, 613, and
> 898 dependents.

Two things follow, and the second is more interesting than the first. The front
end still dominates a warm loop — typecheck, hash and parse are **68.6%** of it
between them, against execute's 28.6% — so a faster evaluator buys less here than
a faster type checker would. That is the shape of the argument that has kept
native codegen deferred, and [Serving](#serving) is where it inverts. But it is a
weaker argument than this file used to make: at the published 3.3% a faster
evaluator was worth almost nothing, and at 28.6% it is worth something. The case
for deferring M9 does not rest on this number — it rests on the *served* profile
below, which is measured independently — and
[ADR 0016](docs/adr/0016-w6-performance.md) is where the decision actually lives.

And opening a content-addressed store of 10,000 definitions costs **2.0ms**,
measured on both re-takes, which is what lets the cache sit in the inner loop
rather than be a build artifact.

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

**A real browser's request head costs about 1.5x a `curl`-sized one.** The
throughput table above is taken with a 41-byte head. This page used to give that
cost as four exact figures — "a two-field, 63-byte head costs 987.3µs per request
and a thirteen-field, 569-byte browser head costs 1,472.4µs — 1,013 req/s against
679", over the in-memory store at 40 concurrent connections. **Those four numbers
have no source in this repository**: they are in no ADR, in neither
`benches/*.json`, and no `ply-corpus` subcommand takes a sweep of that shape (the
head sweeps are `serve --load-headers`, which is `examples/hello.ply` at 23 and
503 bytes, and `w3`'s "fields or bytes" section, which is in-process). They are
withdrawn here rather than re-stated, because the one thing this file promises is
that its numbers are reproducible from this tree, and these were not. What
survives them is the *shape*, which the head sweeps do show and which is the
claim that matters: cost is proportional to **fields parsed** rather than to
bytes received — `ply-corpus serve` reports 84x the head bytes costing 1.90x the
time, a µs/byte column that falls from 1.92 to 0.043 as the head grows. That is
the good regime; a browser is dearer simply because it sends more fields. A req/s
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

**The tracing sink is quadratic in a test — on the tree-walker.** `std.trace`'s
`Sink` appends with `push`; on `--engine machine`, the default, a collecting twin
holding N records costs N pushes and **zero** whole-list copies *if the caller
threads the sink in last position too*, and on `--engine treewalk` it costs one
whole-list copy per record however either is written.

> **Corrected 2026-08-27, by counting rather than by re-taking W5's clock.**
> This read *"so a collecting twin holding N records is O(N²). W5 measured it;
> W6 did not re-take it."* — no engine named, and stale on the default one since
> the survey in ADR 0020 §7 item 3 moved `append`'s growing field to the last
> position of its record literal. Counted on the shipped module with
> `ply_eval::rc::stats()` at N = 200 / 400 / 800: machine **0 / 0 / 0** copies,
> tree-walker **200 / 400 / 800**. `spikes/ply-lexer/GAPS.md` §1 is the rule and
> `crates/ply-eval/tests/stdlib_accumulator_cost.rs` is what now asserts both
> halves; until that test, nothing asserted either, and this sentence was the
> only place the cost was written down at all.
>
> **The mechanism, because "appends with `push`" alone teaches the wrong
> lesson.** `push` grows a `List` **in place** when the caller is its last owner
> (`crates/ply-eval/src/builtins.rs`, `Arc::get_mut`) and copies the whole array
> only when something else can still see it. Which branch runs is decided by
> **position**: `rc::carry` (`crates/ply-eval/src/rc.rs:98`) hands a pending
> frame a live clone of the scope whenever any sub-expression of the enclosing
> node remains, and never asks what those remaining sub-expressions read. So the
> only lesson "avoid `push`" offers is not available — `push` is the language's
> sole list primitive and `trace.ply`'s own `cons` is written out of it. Last
> position is **necessary and not sufficient**, and on the tree-walker it buys
> nothing at all, which is what the counts above show.

**`bytes_slice` and `bytes_split` copy.** `Value::Bytes` is `Arc<[u8]>` with no
slicing, so taking a sub-slice allocates. Response write counts and copies were
**not measured**.

**`--engine both` is not free.** The guarantee that the tree-walking evaluator
and the control-stack machine agree costs two runs, and the two are not the same
speed: the tree-walker is 2.82x faster on the request path.

> **Corrected (regression audit, 2026-08-17).** This read **2.73x**, which is
> the engine substitution in `benches/w6-ladder.json` — a dated pre-region file.
> The sentence is present tense, so it wants the current ladder:
> `./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`
> renders the same substitution at **2.82x** (treewalk 56.34µs against machine
> 158.92µs). Both are one rig; the conclusion — the oracle is not free and the
> engines are not the same speed — is unchanged.

> **And the sign is not constant, which "2.82x faster" invites a reader to
> assume. Added 2026-08-27.** The request path builds no large accumulator. On
> code that does, the tree-walker is *slower and asymptotically worse*: it runs
> no reference counting, so every `push` copies its list whatever position it is
> written in, where the machine rewrites in place. Encoding a string of k
> escapes through `std.json`, minimum of 5, both engines back to back on one
> machine state: over k = 1,000 → 8,000 the machine grows **7.8x** (linear
> predicts 8x) and the tree-walker **74–83x** (quadratic predicts 64x), which is
> **12.9x slower** than the machine at k = 8,000 against 1.2x at k = 1,000. The
> counts behind that are in
> `crates/ply-eval/tests/stdlib_accumulator_cost.rs` and ADR 0020 §7 item 3.
>
> **And it is three runs, not two, when `--backend` is given (2026-08-28).**
> `ply test --backend <spec>` attaches a compiled backend, and under `--engine
> both` that backend is a **third** engine — compared against the plain machine
> rather than against the tree-walker, so that a divergence reported is the
> backend's and nothing else's. ADR 0016 §2.2 priced the third pair as a
> permanent cost before it existed;
> [ADR 0026](docs/adr/0026-a-reachable-backend.md) §4.5 is why it is paid.
>
> The backend that ships today is `reference` — a second tree-walker over the
> scalar-signature fragment, **not** a code generator, and slower than the
> machine. It exists so that a wrong backend can be *caught* before a fast one is
> argued about: `--backend wrong:<mutation>` installs one of eight deliberately
> wrong backends, and `ply test` catches seven of them. Nothing about this
> makes Ply faster, and a run with a backend attached neither reads nor writes
> the result cache.
>
> > **"The backend that ships today is `reference`" is withdrawn, 2026-08-31.**
> > A second one ships: `--backend cranelift` is a real cranelift JIT
> > (`crates/ply-codegen`), compiled into the `ply` binary with no feature flag
> > and no second toolchain. The rest of the paragraph stands for `reference`
> > and the last sentence stands for both.
> >
> > What it is worth, and both halves are measured rather than promised. On
> > `benches/kernel` — a compute loop, which is almost entirely inside the
> > fragment — it is **4.871×** against no backend, and it enters 96.0% of the
> > calls it is offered. On `examples/` — a program built out of the standard
> > library, which is almost entirely outside it — it enters **1.1%** and the
> > run is **2.76× slower**, because compiling costs more per run than entering
> > 1.1% of calls saves. Min of 21 interleaved windows, null control inside the
> > series, taken above this project's load gate and labelled as observations;
> > [ADR 0026](docs/adr/0026-a-reachable-backend.md) §4.9 is the full account.
> >
> > **Corrected 2026-08-31 by an independent re-take on rotated arms.** Both
> > figures replicate (4.927× and 0.353×). But `examples/` is a 468 ms run whose
> > fixed compile cost is most of the window, so *2.76× slower* does not
> > generalise: on the Ply **front end** — `spikes/ply-parser`, the workload
> > [ADR 0030](docs/adr/0030-compiled-code-on-the-front-end.md) measured, 2.85 s
> > — the code generator is **0.969×**, a 3.2% loss. It still loses, and it loses
> > because its fragment there covers **6** definitions against `reference`'s
> > **69**. §4.9 has both series.
> >
> > `ply test` catches **eight of eight** wrong backends under `cranelift`
> > against seven under `reference`. The eighth is a backend that ignores the
> > call budget over a recursion with no base case: on native frames it dies,
> > and a dead child is something the test harness can see.

**The request-path allocation count is large.** One `/health` request makes
**773 allocations and 108,200 bytes** to produce a 107-byte response.

> **Corrected (R4, 2026-08-21).** This sentence read **1,082 allocations and
> 127,955 bytes**, which the block below put there on 2026-08-17. R4 landed the
> three levers of `docs/adr/0019-value-representation.md` — §1's free list for
> the call-argument vector, and §2's two halves, a literal's `Value` built once
> at lowering and one `Value` per constructor mention per thread. Re-taken on
> this tree, three consecutive runs byte-identical:
>
> ```
> $ ./target/release/w6-alloc --repo . --requests 200
> {"allocations_per_request":773.4,"bytes_per_request":108199.93,
>  "requests":200,"response_bytes":107,"route":"/health"}
> ```
>
> **773 against 1,082, so the paragraph's point is weaker than it was and it
> still stands**: 773 allocations to produce 107 bytes. The split between the
> three levers is measured rather than apportioned — each was A/B'd against the
> same tree with only its own change swapped, and the deltas are additive to the
> digit: **§1 −178.0, §2's literal half −65.0, §2's constructor half −45.0** per
> request on the (20, 200) slope, summing to the 911.5 → 623.5 that
> `cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture`
> prints. Read the byte figure only against another 200-request reading:
> `bytes_per_request` rises with the window for a reason nobody has diagnosed
> (`CONTRIBUTING.md` §"Things known to be broken" item 8).

> **Corrected again (regression audit, 2026-08-17).** This sentence read
> **1,122 allocations and 131,677 bytes**, which the block below had just put
> there. R3 hoisted `region_kind::infer` and body lowering off the request path
> after that block was written and nothing re-took this paragraph with them, so
> the correction reproduced the defect it was correcting: a present-tense claim
> the tree had moved under. Re-taken on this tree:
>
> ```
> $ ./target/release/w6-alloc --repo . --requests 200
> {"allocations_per_request":1081.87,"bytes_per_request":127954.65,
>  "requests":200,"response_bytes":107,"route":"/health"}
> ```
>
> **1,082 against W6's pre-region 1,035, so the paragraph's point stands and
> the direction does too** — the region track is still up on this route. The
> ~40 that came off are the one-time `region_kind::infer`; `ROADMAP.md` §R3 is
> the record and `docs/adr/0017-regions.md` §"What must be measured" ¶1 carries
> the 1,035 → 1,082 → 1,122 → 1,082 progression. Read the byte figure only
> against another 200-request reading: `bytes_per_request` rises with the
> window for a reason nobody has diagnosed (`CONTRIBUTING.md` §"Things known to
> be broken" item 8).

> **Corrected (docs pass, 2026-08-17).** This sentence read **1,035 allocations
> and 0.124 MB** in the present tense. That is W6's number and what the ladder
> table above still renders, because that table is `benches/w6-ladder.json`, a
> dated measurement file. This paragraph is not: it claims a current fact, and
> R1/R2 moved it. Re-taken on the shipped tree:
>
> ```
> $ ./target/release/w6-alloc --repo . --requests 200
> {"allocations_per_request":1122.335,"bytes_per_request":131677.4,
>  "requests":200,"response_bytes":107,"route":"/health"}
> ```
>
> **1,122 against W6's 1,035 — the region milestone moved this the wrong way.**
> ADR 0017 "What must be measured" §1 (`docs/adr/0017-regions.md:382` onward;
> the 1,122 reading and its amortization are at `:428-440`) records
> why: about 40 of the 87 are `region_kind::infer`, run once per `Machine` and
> so amortizing to nothing over a server's lifetime, and the rest is arena
> wiring on a route that allocates no cells. The point the paragraph is making
> is unaffected — `/health`'s ~1,000 allocations are `Rc<Value>` boxes on the
> framing, routing and encode path.

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

`cargo test --workspace` takes about **three minutes** on an unloaded machine,
and all of it passes.

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
rather than gates — the ones in `ply-corpus --test http_cost` and
`interp::tests::a_cached_mention_against_the_allocation_it_replaces` in
`ply-eval`'s lib tests — plus one doc-test, which is not a benchmark. Each
benchmark prints the command that runs it. `docs/ONBOARDING.md` §2 has the wall
clock and its spread, which is wide: readings on this one command have ranged
from three minutes to twenty-nine, and the high ones describe a loaded machine
rather than the tree.

> **One target used to dominate that spread, and no longer does.**
> `r4_value_construction` attributes every allocation in a request to the code
> that asked for it, which meant a backtrace per allocation; it read **70.9s in
> debug against 25.6s in release**, and the profile `cargo test --workspace`
> runs is the slow one. The capture was never the expensive half — the
> *resolve* was, and it ran per allocation rather than per code address. That
> target is **6.7s** in a debug workspace run now, down from 46.2s, with every
> figure it prints unchanged. The release comparison has not been re-taken.

And `ignored` is not the whole of the timing-sensitive suite —
`ply-eval/tests/region_arena_cost.rs::snapshot_cost_as_a_function_of_region_size`
asserts on a wall-clock growth ratio and runs by default, and it failed for us on
a machine that was busy compiling something else. On a quiet machine it passes.

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

MIT OR Apache-2.0 — the texts are [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE) at the repository root, added 2026-08-27. Until
then neither file existed, and this line and `Cargo.toml:22` declared a licence the
repository did not ship; `CONTRIBUTING.md` §"Things known to be broken" item 7 is
where that was recorded. The thirteen workspace members inherit the expression with
`license.workspace = true`.
