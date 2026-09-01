# Onboarding

You have never seen this repository. This file gets you from `git clone` to a
change you can defend, and every command in it was run on the machine and date
in [Provenance](#provenance) with the output printed beside it.

**The rule this file follows.** A command here is either one that was *run*, with
its real output and its real wall clock, or it is labelled **not run here**.
Nothing is quoted from another document. That rule exists because sixteen
milestones of adversarial review found seven written claims that did not hold,
and the two most recent were in documentation about correct code. If you add to
this file, add measurements, not restatements.

Read in this order:

- [Provenance](#provenance) — the machine every number below came off
- [0. What Ply is, in ninety seconds](#0-what-ply-is-in-ninety-seconds)
- [1. Build](#1-build)
- [2. Test](#2-test)
- [3. Run the examples](#3-run-the-examples)
- [4. Run the server against real postgres](#4-run-the-server-against-real-postgres)
- [5. Your first change: the rename exercise](#5-your-first-change-the-rename-exercise)
- [6. Where things live](#6-where-things-live)
- [7. Which claims are checked, and which are only written](#7-which-claims-are-checked-and-which-are-only-written)
- [8. What to work on, and why M9 is deferred](#8-what-to-work-on-and-why-m9-is-deferred)
- [9. Traps](#9-traps) — the things that cost this audit time
- [10. The documents, and what each is for](#10-the-documents-and-what-each-is-for)

## Provenance

Everything below was measured on an Apple M-series machine, `aarch64-apple-darwin`,
10 cores, macOS 15.7.3 / Darwin 24.6.0, `rustc 1.93.1`, PostgreSQL 18.3.

Your wall clocks will differ, and this file gives them as magnitudes for that
reason. Four **counts** should not differ, because each is a property of what the
language does rather than of how big the tree has got — `ply prove
examples/desk.ply` → 7 obligations, `ply hosts examples/desk.ply --host` → 25
host handlers, `examples/same-tests.sh` → 29 agreeing requests, and
`ply-corpus bench` selecting 5,000 tests down to 157. If one of those differs on
your machine, that is a finding; open it as one.
## 0. What Ply is, in ninety seconds

A research language whose thesis is that the verification loop should collapse
toward zero. Five mechanisms carry it:

1. **Effects are in the type at resource granularity.** `db.read[users]` is a
   different atom from `db.write[orders]`. Two tests contend only if they share
   a resource and one of them writes, so the scheduler colours a test suite
   statically.
2. **Definitions are content-addressed.** The unit of compilation is the
   definition, not the file. A hash is taken over normalized structure with
   names erased, so **renaming changes no hash and re-runs nothing** — §5 below
   is you proving that to yourself in about a minute.
3. **Tests have an exact cache** keyed by the test's hash.
4. **Concurrency is an effect**, so races are searched exhaustively rather than
   waited for.
5. **Specs are the reviewable artifact**, and a spec's strength tier is derived
   from its evidence rather than stored.

`DESIGN.md` is the long form. You do not need it to finish this file.

## 1. Build

There is **no `rust-toolchain.toml` and no `rust-version` in any `Cargo.toml`**
(checked: `grep -rn rust-version Cargo.toml crates/*/Cargo.toml` returns
nothing). Whatever toolchain you have is what decides. 1.93.1 works.

```
cargo build --workspace
```

Measured cold, into an empty `CARGO_TARGET_DIR`. A cold build prints **157**
`Compiling` lines over 155 distinct crate names (two build twice):

| profile | command | cold wall clock | re-taken |
| --- | --- | --- | --- |
| debug | `cargo build --workspace` | **16.8s** | 16.56s |
| release | `cargo build --workspace --release` | **58.3s** | 53.85s |

> **cranelift is an unconditional dependency.** `crates/ply-cli` depends on
> `crates/ply-codegen` with **no feature flag to turn it off**, so a plain build
> compiles a code generator whether or not you will use one.
>
> **The version pin is a toolchain decision, not a dependency bump.** cranelift
> 0.132.3 declares `rust-version = "1.93.0"`, at or below the toolchain this
> repository already needs; 0.133 and later require 1.94.0. The pins in
> `crates/ply-codegen/Cargo.toml` are deliberate and say so.

Warm (nothing changed) is **0.11s** debug and **0.15s** release, re-measured;
this line said 0.25s for both, which is the right order of magnitude and was not
re-taken when it was written. There is no build script to run, no code generation
step, no submodule, no `make`. Three binaries land:

- `target/{debug,release}/ply` — the language driver
- `target/{debug,release}/ply-corpus` — the measurement harness
- `target/release/w6-alloc` — an allocation counter used by one W6 test

The `ply` binary carries a cranelift code generator since 2026-08-31, reachable
as `ply test --backend cranelift`. It is off unless a run names it: a plain
`ply test` installs no backend at all, and a run that installs one neither reads
nor writes the result cache. `ply test --help` has the grammar;
`docs/adr/0026-a-reachable-backend.md` §4.9 has what it reaches and what it
costs, including the workload where it is a **net loss**.

**Use the release binary for anything you intend to time.** The debug
interpreter is dominated by `debug_assertions`; ADR 0016 §1.6 refuses to mix the
two profiles in one comparison and so should you.

**Lint and format are clean and there is no configuration to install:**

```
cargo fmt --all --check                      # exit 0, no output
cargo clippy --workspace --all-targets       # exit 0, 0 warnings
```

Both re-verified: `fmt --check` printed nothing and exited 0, and `clippy` on a
warm target emitted **zero** lines matching `warning` or `error`. The 13.7s this
line used to attach to clippy is a *cold* figure — warm it finishes in 0.4s, so
treat 13.7s as the first-run cost and not as what you will see.

There is no `rustfmt.toml` and no `clippy.toml`; both run on defaults.

### `crates/ply-codegen-spike` is outside the workspace

It builds on the pinned toolchain — no `+1.94.0` prefix anywhere in this
repository any more — and `cargo test --release` from inside the crate is green.

**The thing to know is structural, not the transcript.** The crate declares its
own `[workspace]`, so `cargo build --workspace`, `cargo test --workspace` and
`cargo clippy --workspace --all-targets` do not compile one line of it. It has
bit-rotted that way twice, each time discovered long after the change that broke
it. CI gives it a job of its own for exactly that reason, and
`.github/ci-shards.sh verify` fails if a crate ends up in no job at all.

Two live caveats:

- It is **not clippy-clean** and never was; the project's stated gate does not
  reach it.
- `cargo test --release` green does **not** mean the spike agrees with the
  interpreters. `mcts --dir benches/kernel --only agreement` exits 1. See
  `CONTRIBUTING.md` §"Things known to be broken".
## 2. Test

```
cargo test --workspace
```

**Budget a few minutes on an unloaded machine, and do not run it under load.**
The wall clock on this command has ranged from three minutes to nearly thirty
across its history, and the slow readings are the machine rather than the tree —
in the worst of them *user time was below real time*, which is a run spending
minutes waiting for cores rather than using them.

No test count is given here on purpose. It changes on every commit that adds a
test, nothing in the tree checks it, and every re-take this file used to carry
found it stale without anything having failed. What matters at this command is
that nothing failed.

Two sections follow that are worth reading before you trust a green run.
### Five things a green suite does not prove

`cargo test --workspace` green is weaker than it looks, and the reason is always
the same: **a gate that is closed makes its tests pass without running them.**

| gate | what goes quiet | how to open it |
| --- | --- | --- |
| `PLY_PG_URL` unset | the live postgres tests pass without a server | set it at a real database |
| `PLY_TEST_DB` unset | the `ply-host` pool tests pass, printing *nothing* | set it; a *wrong* value is loud, only a missing one is silent |
| no `initdb`/`postgres`/`psql` on `PATH` | the cluster-gated suites skip | install postgres |
| not Unix | `#![cfg(unix)]` files are not compiled at all — no notice whatsoever | run on Linux or macOS |
| `crates/ply-codegen-spike` | outside the workspace, so `--workspace` never builds it | `cd` in and build it |

The worst of these is `PLY_TEST_DB`, because it prints no skip line at all — not
on stdout, not on stderr. The tests report as passing and nothing distinguishes
that from a run against a live database.

**The lesson generalises past this list.** When you add a test that depends on
something the environment may not have, make the absent case *loud*. A skip
notice on stderr is the minimum; a CI step that fails when it finds that notice
is what actually holds. Asserting the test count does not substitute — a gated
test returns early and passes, so the count is right with nothing behind it.
### Some tests assert on a wall clock and run by default

`ply-eval/tests/region_arena_cost.rs::snapshot_cost_as_a_function_of_region_size`
asserts on a timing growth ratio and is **not** in the `ignored` set. It passes
on a quiet machine and has been seen to fail on one busy compiling something
else. **If it is your only failure, re-run before you believe it.**

Several more assert a performance figure the same way. `.github/ci-shards.sh`'s
`DEFERRED` table is the list, and it is maintained by *running the shards*, not
by surveying the tree — two surveys declared themselves complete and each was
proved wrong within the hour by a shard going red on a test neither had found.
One of them reads no Rust clock at all; it parses milliseconds out of `ply
test`'s own output, so no timing vocabulary appears in it.

CI runs every deferred test alone, single-threaded, on its own runner, and the
parallel shards skip them. If you add a test that asserts on elapsed time, add
it to that table too — and prefer asserting on a *count* (allocations, copies,
passes) over a duration wherever the question allows it, because a count does
not depend on what else the machine is doing.
## 3. Run the examples

All four commands below were run and their output is what is shown.

```
$ ./target/debug/ply test examples/        # first run, empty cache
   selected 186 of 186 (0 cached)
   5 groups · 10 workers
   isolated 176 of 186 · 10 tests can contend
   0 failed, 186 passed, 0 cached (0.71s)

$ ./target/debug/ply test examples/        # second run
   selected 1 of 186 (185 cached)
   1 group · 10 workers
   isolated 176 of 186 · 10 tests can contend
   0 failed, 1 passed, 185 cached (0.00s)
```

Note the group count collapsing from 5 to 1: with only the one `nondet` test
left to run there is nothing for it to contend with. That column is the
scheduler showing its work.

The 1 that always runs is `clock.a session with no deadline never goes stale` —
a `nondet` test, which by construction opts out of the cache. That asymmetry is
the whole product; §5 is where you check it is real.

The result cache lives in `<project>/.ply-cache/` — so `examples/.ply-cache/`,
gitignored. `ply cache clear` empties it, `ply cache stats` describes it.

```
$ ./target/debug/ply test examples/bank.ply --no-cache
   ok  a lone transfer moves the money and stops at the balance
         1 interleaving · exhaustive
   ok  two unguarded transfers conserve the money whichever order they run in
         9 interleavings · exhaustive
   ok  the guarded transfer never overdraws, under every interleaving
         6 interleavings · exhaustive
   ok  the helpers add up
   0 failed, 4 passed, 0 cached (0.02s)

$ ./target/debug/ply prove examples/desk.ply
   180 definitions · 13 carry an obligation · 167 do not
   7 obligations · 2 proved · 5 property · 0 example   (4.99s)
   7 held (4.99s)

$ ./target/debug/ply hosts examples/desk.ply
   hermetic — no host handler is bound
   47 operations would bind under `--host`

$ ./target/debug/ply hosts examples/desk.ply --host
   25 host handlers · 47 operations · trusted computing base
   ... digest: b3:bf61caefa56f
```

Every count there matches what `README.md` publishes.

`ply --help` lists eleven subcommands; `check`, `test`, `prove`, `review`,
`hosts` and `cache` are the ones you will use daily.

## 4. Run the server against real postgres

`README.md` does not mention either of the two scripts that do this. They are:

| script | what it does |
| --- | --- |
| `examples/serve.sh` | serves `examples/desk.ply` on a socket — twin, postgres, or postgres+TLS |
| `examples/same-tests.sh` | W4's exit criterion end to end: same source, twin vs. postgres, byte-compared |

### `same-tests.sh` — the one to run first

It needs `psql` and `curl` on `PATH`. With no `--db` it also needs `initdb` and
`pg_ctl`, and then it **creates its own postgres cluster in a temp directory on
port 55433 and tears it down after**. It does not touch any cluster you already
have.

**It builds the release binary itself, since 2026-08-27.** This section used to
open:

> **Build the release binary first.** `same-tests.sh` uses `target/release/ply`
> and, unlike `serve.sh`, never builds it — on a fresh clone it dies at line 79
> with "No such file or directory":
>
> ```
> cargo build --workspace --release
> ./examples/same-tests.sh
> ```

Both halves are now wrong. The script runs `cargo build --locked --release -p
ply-cli` itself, and it also refuses to run against a binary that is absent or
older than a source in `target/release/ply.d` — so a stale instrument is an abort
naming the file rather than a number you would have believed. It prints what it
checked, derived from that file rather than written down. On 2026-08-27:

```
instrument: 152 sources across 12 crates in target/release/ply.d, none newer than the binary
```

Both numbers come out of `target/release/ply.d` on the run that prints them, so
a crate added tomorrow moves them without anyone editing this page.

`--locked` arrived on a second pass the same day; this paragraph had said
`cargo build --release -p ply-cli`, without it. An unlocked build re-resolves a
`Cargo.lock` that has fallen behind the manifests and says nothing about it,
which is not something a measuring script may do. If your lock is genuinely out
of date the script now stops with cargo's own `cannot update the lock file ...
because --locked was passed`; run `cargo build` once and re-run. Re-run on a worktree with
no `target/` at all: exit 0, 29 requests, and the release build dominates the
run — 54.8s of a 60.4s total. Those two are **observations**, not the figure
withdrawn just above: the 1-minute load average went from 3.2 to 32.4 across
that run and the gate is 4.0, so they say "the build is most of a cold run" and
nothing narrower. `--no-build` skips the build for the case where you meant a
specific binary; it does not skip the freshness check.

```
./examples/same-tests.sh
```

~~Measured: **4.6s**~~ — withdrawn rather than updated. That figure named a run
that did no building, and the script does now. It was not re-taken here: the
1-minute load average was 30.5 against this project's gate of 4.0, and a number
taken over that is not a number. Exit 0 and the tail are unchanged:

```
== 3. the same requests to both, compared byte for byte ==
   agreed    GET /health                                200
   ... 29 rows ...
== 4. the transaction, in the database rather than in the response ==
   committed  201, orders 3 -> 4
   rolled back 409, orders still 4, widget still 1
   and the id it consumed is gone: orders_id_seq 6 -> 7
29 requests, byte for byte identical between the twin and postgres.
```

**Step 1 reads its own counts now.** This section used to say:

> **Read step 1's counts, not just its exit code.** Step 1 runs
> `ply test examples/desk.ply --no-incremental`, and `--no-incremental`
> disables only the *front-end* cache — the result cache is untouched
> (`ply test --help` says so explicitly; `--no-cache` is what disables both).
> So on a warm `examples/.ply-cache` step 1 prints
>
> ```
>    selected 0 of 68 (68 cached)
>    0 failed, 0 passed, 68 cached (0.00s)
> ```
>
> and the script still exits 0. Nothing ran. With a cold cache the same step
> prints `0 failed, 68 passed, 0 cached (0.09s)`. If you are using step 1 as
> evidence, run `ply cache clear` in `examples/` first.

Every word of that was true, and it is what got the flag changed. Step 1 passes
`--no-cache` since 2026-08-27, so the warm-cache reading above cannot happen,
and it no longer needs you to read anything: it parses the counts line itself
and exits **1** if `cached` is not 0 or if `passed` is 0. On a warm cache it now
prints

```
   selected 68 of 68 (0 cached)
   --no-cache: results were neither read nor recorded
   0 failed, 68 passed, 0 cached (0.17s)
   68 tests evaluated, 0 served from cache
```

`ply cache clear` before running it is no longer necessary. The advice to read
counts rather than exit codes still is, everywhere else: `ply test` exits 0 over
a suite it did not run, and step 1 was the tree's purest instance of that.

### `serve.sh` — a service you can curl

```
./examples/serve.sh --memory                          # no database at all
./examples/serve.sh --db postgres://localhost/desk    # needs examples/desk.sql loaded
./examples/serve.sh --db postgres://localhost/desk --tls
```

It copies `desk.ply` into `examples/.serve/` and rewrites one line *there*, so
your working tree is not modified (`PLY_SERVE_OUT` moves that directory). It
runs `cargo build --locked --release -p ply-cli` itself — ~~unlike
`same-tests.sh`~~, a distinction that stopped existing on 2026-08-27 when
`same-tests.sh` took the same line (`serve.sh:160`) and added a freshness check
`serve.sh` does not have. `--locked` reached both lines together, and had to:
`same-tests.sh` starts this script twice, so an unlocked build here is an
unlocked build inside a run whose whole point is a tree that does not move.
It prints a generated `DESK_API_KEY` if you have not exported one.

Measured, `--memory` on port 8911:

```
$ curl -sS localhost:8911/health
{"routes":11,"service":"parts desk"}                          # 200

$ curl -sS localhost:8911/items
[{"name":"hex bolt","on_hand":500,"price":0.40,"sku":"bolt"}, …]   # 200
```

`kill -TERM` drains and the process exits **0**. Note `"routes":11` — the
service reports its own route count, which is the eleven `README.md` claims.

Two things `--memory` will not show you: no trace lines appear (the twin
discharges every `trace` atom in Ply, so nothing reaches `ply_host::trace`), and
`--memory` refuses `--tls`. Run against a database to see either.

The schema is created out of band: `psql -d desk -f examples/desk.sql`.

**One comment in `serve.sh` was wrong and is now corrected in place.** It used
to say `--db-schema desk.schema` means "the driver refuses at bind time with
`E0435` if the live database is not the one `desk.ply` describes." It does not.
`E0435 DB_SCHEMA_MISMATCH` is **raised nowhere** —
`grep -rn 'E0435\|DB_SCHEMA_MISMATCH' crates/ --include='*.rs'` returns five
hits and not one of them constructs a diagnostic: `crates/ply-span/src/lib.rs:428`
defines the constant, `:801` registers it — inside `#[cfg(test)] mod tests`, so
that hit is a test — `crates/ply-eval/src/host.rs:1106` lists it as reserved,
and `crates/ply-cli/src/artifact.rs:253` and `crates/ply-cli/src/db.rs:539` are
comments describing the check as future work. (The two `ply-span` numbers were
`:414` and `:787` until 2026-08-27, when the `codes` module grew a doc comment;
the finding is unchanged. This passage is now also a *test*: see
`crates/ply-span/tests/armed.rs` and `CONTRIBUTING.md` §"The shape it keeps
taking".)
`--db-schema` resolves the name, checks it is a nullary function returning a
`Schema`, evaluates it, and reads its table and column counts; **it never opens
a connection to compare.** `README.md` §"What is missing" documents this
correctly and the script's comment had not been updated with it. What you
actually get is `E0433 DB_PREPARE_FAILED`, per statement, on first execution.
The docs audit rewrote the comment (`examples/serve.sh:37–54`) with the
measurement beside it rather than deleting it — this was the same never-armed
check that W1 shipped, and it is worth leaving legible.

## 5. Your first change: the rename exercise

The claim to check is `README.md`'s: *a top-level function renamed project-wide
selects zero deterministic tests, and this is provable rather than heuristic.*

Do it on a copy so you are not editing the repository:

```
cp examples/*.ply /tmp/ex/ && cd /tmp/ex
../path/to/target/debug/ply test .        # 186 passed, 0 cached
../path/to/target/debug/ply test .        # selected 1 of 186 (185 cached)

# rename a top-level function and every call site
perl -pi -e 's/\bline_total\b/extended_amount/g' store.ply

../path/to/target/debug/ply test .
```

Measured result — **identical to the "nothing changed" run**:

```
   selected 1 of 186 (185 cached)
   0 failed, 1 passed, 185 cached (0.00s)
```

The 1 is the `nondet` clock test, which runs on every invocation whatever you
do. **Zero deterministic tests were selected.** Not "few": zero.

### Why zero is the *expected* answer, from the documents

Three things have to be true, and each is stated somewhere and asserted
somewhere:

1. A definition's hash is taken over normalized structure with names erased —
   `crates/ply-hash/src/normalize.rs:1-11`: *"a local binder is replaced by the
   de Bruijn level at which it was bound, and a reference to another top-level
   definition is replaced by that definition's hash. Neither a local's name nor a
   referent's name can reach the byte stream, which is what makes renaming
   free."* `CONTRACTS.md` §Modules states the same rule as a contract.
2. A test result is keyed by the test's hash. So if no hash moved, every cached
   result is still valid and selection is empty.
3. It is an asserted invariant, not an observation:
   **`crates/ply-cli/tests/suite/cli.rs:145 renaming_a_definition_re_runs_nothing`**
   writes a two-line project, renames `width` to `breadth`, and asserts the
   output contains `selected 0 of 1 (1 cached)` with the failure message *"a
   rename rebuilt something"*.

If your rename *did* select tests, one of those three broke, and (3) is the test
that localizes it.

### The same exercise at scale, in one command

`ply-corpus bench` runs five scenarios over a generated 10,000-definition
project and the third of them is exactly this:

```
cargo build --release --workspace
./target/release/ply-corpus gen --out /tmp/corpus \
    --seed 1 --modules 200 --defs-per-module 50 --tests 5000 --depth 6
./target/release/ply-corpus bench /tmp/corpus --repeats 3
```

Note the `--out`: it is **required**. `README.md:97` used to quote this recipe
without it — and `bench` without its positional corpus argument — where it fails
with `error: the following required arguments were not provided: --out <OUT>`.
The docs audit restored both there, so the README recipe now runs as printed.

Measured here (16s for `bench`, 7s for `gen`), the selection column reproduces
`README.md`'s table exactly — re-run for this audit and reproduced again:

| scenario | selected of 5,000 |
| --- | --- |
| cold — empty cache | 5000 |
| warm — nothing changed | 157 |
| **rename — `render_167` renamed; a rename must select nothing** | **157 — the same** |
| edit-leaf — 1 dependent | 158 |
| edit-hub — 898 dependents | 613 |

`gen` also reproduced byte-identically: *200 modules · 10000 definitions (8429
effectful) · 5000 tests (157 nondet) · 4748 KiB of source*.

## 6. Where things live

Thirteen workspace crates, ~160k lines of Rust. The dependency order is roughly
the order below.

| crate | lines | what it owns |
| --- | --- | --- |
| `ply-span` | 1.0k | spans, `Symbol`, **the diagnostic-code registry** |
| `ply-syntax` | 8.7k | lexer, parser, `ast` |
| `ply-derive` | 1.8k | derived codecs (JSON etc.) |
| `ply-core` | 12.2k | types, **effect rows and atoms**, inference |
| `ply-hash` | 6.7k | **content addressing**: normalization, the def graph, `DefHash` |
| `ply-eval` | 31.1k | two engines (tree-walker + control-stack machine), regions, simulation |
| `ply-store` | 8.8k | on-disk cache, obligations |
| `ply-test` | 12.2k | selection, **conflict grouping**, bisection, spec obligations |
| `ply-prove` | 12.3k | obligation discharge, tiers, evidence |
| `ply-host` | 21.4k | the trusted computing base: sockets, TLS, postgres, config, trace |
| `ply-std` | 0.3k | the shipped `.ply` modules |
| `ply-cli` | 20.0k | the `ply` binary and its subcommands |
| `ply-corpus` | 20.9k | the `ply-corpus` measurement harness |

`crates/ply-codegen-spike` (2.7k) is **outside** the workspace and does not
build — see §1.

### The two features the audit asked for, located

**Where footprint conflict grouping is decided.** Three files, in this order:

- `crates/ply-core/src/ty.rs:52` — `EffectAtom::conflicts_with`. The whole
  basis, and it is five lines: *same effect, same resource, and at least one
  writes.*
- `crates/ply-core/src/ty.rs:171` — `Footprint::conflicts_with`, the lift to
  sets.
- `crates/ply-test/src/schedule.rs:216` — **`group_by_conflict`**, which is
  the grouping proper: greedy colouring, a test joining the first class that
  conflicts with nothing already in it. Region-isolated tests clear every class,
  land in group 0, and never create a group — which is what makes adding one
  free. `parallelism()` at `:172` reports over it.

A warning about searching for this yourself: `grep -rl conflict crates/*/src`
matches **51 files** (re-counted; this line said 30, which nothing in the tree
produces), because `ply-eval/src/sim.rs` has its own unrelated
`Access::conflicts_with` (`:463`) and `StepFootprint::conflicts_with` (`:539`)
for the interleaving search. Those are step-level, not test-level. Start from
`ply-test/src/schedule.rs`.

**Where a definition's hash is computed.** Also three places:

- `crates/ply-hash/src/normalize.rs` — `Normalizer`, which produces the bytes.
  This is where the guarantee actually lives: tags per node, `u32` length
  prefixes, de Bruijn levels for locals, referent hashes for references, names
  and spans erased. `Normalizer::finish` at `:237`.
- `crates/ply-hash/src/lib.rs:56` — `DefHash::of`, literally
  `DefHash(*blake3::hash(bytes).as_bytes())`. That one line is the hash.
- `crates/ply-hash/src/lib.rs` — `hash_program` (`:177`) drives it over the
  program; `hash_component` (`:558`) handles cyclic strongly-connected
  components by partition refinement, which is the subtle part and is commented
  at length in place.

Keys that are *not* a plain definition hash have domain tags and their own
functions in the same file: `spec_hash` (`:163`), `spec_text_hash` (`:144`).
`crates/ply-hash/src/body.rs` is the self-checking on-disk body encoding.

### Finding anything else

There is no symbol index. What worked:

- Diagnostic codes are the best entry point into the CLI and host layers.
  `crates/ply-span/src/lib.rs` holds every `E0xxx` constant *and* a registry
  table pairing the constant with its string, so `grep -n E0446 crates/ -r` goes
  straight to both the definition and every raise site.
- `CONTRACTS.md`'s per-crate sections give real signatures — but read §7 before
  trusting one.
- Test names in this repository are English sentences
  (`renaming_a_definition_re_runs_nothing`,
  `the_shipped_ladder_still_describes_the_tree_it_ships_in`). Grepping test
  names for a behaviour is usually faster than grepping implementation.

## 7. Which claims are checked, and which are only written

This is the section the rest of the project's audit history is about. Be exact
about it.

### There is CI, and what it does and does not settle

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings` and the whole test suite run on every pull request and every push to
`main`. `examples/same-tests.sh` runs too, which is W4's exit criterion.

**What earns CI its keep is the gates**, not the suite. Four of the five §2
describes are the ones that return a *passing* result when their dependency is
absent, and CI forces each open:

- **`PLY_PG_URL`** is set at a postgres service container, and the job runs the
  live tests with `--nocapture` and fails if it finds the notice a skipped one
  prints. Three things make that work and all three were wrong at some point:
  cargo captures the notice on a *passing* test, so `--nocapture` is needed to
  emit it; the notice is an `eprintln!`, so the step needs `2>&1` or the grep
  reads a stream it cannot appear on; and asserting the test *count* does not
  substitute, because the tests return early and pass, so the count is right
  with no server anywhere.
- **`initdb`, `postgres` and `psql`** are put on `PATH` where
  `cluster::available()` looks, and the job fails if they are absent.
- **`#![cfg(unix)]`** compiles, because the runner is Linux, and a step fails if
  that binary reports zero tests.
- **`crates/ply-codegen-spike`** gets a job of its own, because it declares its
  own `[workspace]` and `--workspace` has never reached it. Not a skip — a crate
  nothing builds. It has bit-rotted twice with nothing to say so.

**And one check that is not a gate in that sense.** The tree checks in
`crates/ply-span/tests/armed.rs` run a second time by name, so that a rename or
a stray filter turns CI red rather than quietly reducing what CI checks. A test
that stops running reports nothing, and reporting nothing is indistinguishable
from passing — which is the failure this whole section is organised around.
### Exactly one test reads a prose document

`crates/ply-corpus/tests/w6_report_allocations.rs`'s
`the_readme_still_describes_this_request_path` reads `README.md`'s *"One
`/health` request makes N allocations and M bytes"* and compares both numbers
against a freshly counted window, within 1%. Run it with
`cargo test -p ply-corpus --test w6_report_allocations -- --nocapture` and it
prints both sides. The count does not vary with the build profile.

Nothing else. No test opens `DESIGN.md`, `ROADMAP.md`, `CONTRACTS.md` or any
ADR, and no other sentence of `README.md` is read.

**So treat every other number in the prose surface as unenforced.** That is the
reason this project keeps its documents thin on figures: an unenforced number
does not stay true, nobody finds out when it stops, and the effort of keeping it
current is spent whether or not it was worth anything. If a number matters
enough to write down, arm it the way that test does; if it does not, leave it in
the file the command wrote.
### How claims are written here

A number belongs where it was taken — in the file the command wrote, or in the
ADR that argues from it. This document carries **shapes and procedures**: which
command to run, which way a trade goes, what will bite you. Where it does give a
figure, expect a magnitude rather than a decimal.

Corrections are made in place. The record of what a number used to be is git's
job, and putting it in prose costs every future reader while helping none of
them. Keep a note beside a claim only when it would otherwise be *redone*: a
rejected alternative, a trap, or a measurement taken for the wrong question.
## 8. What to work on, and why M9 is deferred

**Start at `ROADMAP.md` §"What is next"** (the last section of the file). It is
the current queue and it is ordered on purpose: each item moves the number the
next one is judged against.

0. **Decide the regions question.** R3 ended on a decision rule fixed before it
   started and the rule fired against the design: `/health` still allocates
   **1,082** times against a pre-region **1,035**. Re-take it yourself in one
   command — `./target/release/w6-alloc --repo . --requests 200`, and the
   baseline is in `benches/w6-ladder.json`'s `boxing on hot paths` alternative.
   `ROADMAP.md` §R3 is the record.

1. Unboxed primitive representation, and monomorphization. R3's attribution
   points here: `frame::dispatch < Machine::step < Machine::call` is **45.5%** of
   what a request allocates, per `cargo test -p ply-corpus --release --test
   w6_alloc_sites -- --nocapture`.

> **"Unboxed primitive representation" is not a lever here.** `Int`, `Bool`,
> `Float`, `Unit`, `Decimal`, `Cell` and `Task` are already inline variants of a
> `Value` and allocate nothing; ADR 0019 §4 *rejects* narrowing `Value`, with the
> number that would have justified it, which is zero. A profile that ranks
> `frame::dispatch` high is ranking a **frame**, and the frame is three different
> things — attributed by value instead, its bulk was the call-argument vector,
> which is handled. **Monomorphization is untouched and still open.**

2. Evidence passing and handler specialization.
3. Re-measure codegen's ceiling — **before** re-arguing M9. The *ladder* half has
   been re-taken and ships as `benches/w6-ladder-r3.json`; render it with
   `./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`.
   The *spike* half still cannot be re-taken, because the spike does not build;
   see §1. `ROADMAP.md` §"What is next" item 3 now records that blocker.

> **The ceiling is fragment coverage, not entry.** This block used to say the
> interpreter *cannot enter* compiled code; it can, and does — `--backend
> cranelift` is a real JIT in the shipped binary. What decides whether that is
> worth anything is how much of a program falls inside the compiled fragment: on
> a compute kernel it is most of it and the win is large, on a program built out
> of the standard library it is a fraction of a percent and compiling costs more
> than entering saves. That is the lever to argue about, and ADRs 0026 and 0030
> carry the series.

Plus two small recorded obligations: delete `crates/ply-codegen-spike` per ADR
0016 §3.5, and fix `Machine::constant` refusing the memo inside any open
scheduler region, which costs a spawning service 1.77x on `/health`.

> **Do not delete the spike yet.** ADR 0016 §3.5 wants it gone, and it is the
> only instrument in the repository that can price a code generator — ADR 0018
> §1 could not have been answered without it. Deleting it and then re-arguing
> M9 from a file is the failure mode this whole section exists to prevent.

### M9 (native codegen) is deferred, not forgotten, and you can re-derive it

M9 is the one planned milestone that has not landed. It was deferred by W6
against criteria **fixed in code before any number existed**
(`ply_corpus::w6::Criteria::default()`), so the decision is re-derivable rather
than re-arguable. Run it:

```
./target/release/ply-corpus w6 benches/w6-ladder.json benches/w6-spike.json
```

> **Two ladders exist and the line above renders the older one on purpose** — it
> is the file the verdict quoted below came off, kept because it is also the only
> record of the pre-region allocation baseline. For the current tree substitute
> `benches/w6-ladder-r3.json`; the verdict is the same. **Do not run `ply-corpus
> w6 benches/*.json`**: `w6` merges last-wins and the glob sorts `-r3` first, so
> it silently renders the older file. `benches/README.md` §"There are two
> ladders" is the full note.

That prints the verdict, its inputs, and — the part that matters — the exact
condition that reopens it. The verdict block, **abridged** (marked as such
because this file's rule is that a quoted output is the real one; the elisions
are flagged inline and everything else is character-for-character):

```
M9: keep deferring M9
  - the interpreter is 35% of a request (209.3µs of 592.6µs), so the ceiling on
    any execution-strategy change is 1.55x
  - the residue is -46.3µs — negative, so the layers sum to more than the
    request they are read against and the seam is charged to the interpreter
    rather than to nobody: the attributed share is 43.1% and the one above is
    what the decision reads
  - over its repeats that share runs 35.1%–35.4%
  - the spike compiled `std.http.read_line` and held 11.67x on its weakest
    input, which projects 1.48x end to end
  - 6 of ADR 0016 §4's 7 cheaper levers are not priced, and a cheaper lever
    that has not been priced is on its own a reason to keep deferring: [… the
    six levers and their descriptions follow on this same line …]
  M9 reopens when the interpreter share reaches 50% (it is 35%, a 1.55x
  ceiling), and the projection reaches 1.50x (it is 1.48x), and the 6 unpriced
  lever(s) in ADR 0016 §4 are priced and the best of them measures at or below
  1.24x end to end
```

(An earlier pass printed this as "verbatim" with the residue and repeats bullets
cut and no mark that anything had been. The verdict has **five** bullets, not
three, and the two that were dropped are the ones qualifying the 35% the whole
decision turns on. Note that the block does not line-wrap in reality — each
bullet is one long line.)

Three of four criteria fail — C1 share, C2 ceiling and C3 nothing-cheaper, with
only C4 correctness passing (ADR 0016 lines 1149–1152 tabulate them) — and the
unpriced-lever criterion is independently sufficient on its own. The command
also prints `this report is incomplete` with **twelve** named reasons (re-counted
from the run above; this line said seven). They are six about the measurement —
four rungs whose two halves are taken on different routes, so each difference is
a route change as well as a layer; the `tracing` layer's repeats running −1.41µs
to 9.74µs, so *its printed 5.84µs did not resolve its own sign*; and the −46.32µs
residue — plus one line for each of the six unpriced levers. That is the house
style: the report argues against itself in its own output.

`ADR 0016` is where the decision lives; `ROADMAP.md` §M9 is the summary. **Do
not re-argue M9 from the numbers in either.** Re-measure.

## 9. Traps

Everything here cost this audit real time. In descending order of cost.

1. ~~**`crates/ply-codegen-spike` needs `+1.94.0`**~~, **and nothing in the
   workspace compiles it** — one wall of the two is gone as of R4 (the crate
   builds and its tests pass); the **toolchain wall is gone too as of
   2026-08-31**, when the crate moved to cranelift 0.132.3 and its CI job to
   1.93.1. This item read *"the toolchain wall stands"*. The crate is still
   outside `--workspace`, so it rots silently and has done so twice — and a
   third way that no toolchain would have caught: its agreement corpus is red
   and `cargo test` does not run it (`CONTRIBUTING.md` item 18). Its
   `--half` invocation also needs `--bin ply-codegen-spike` now that the crate
   ships two binaries. §1.
2. ~~**`examples/same-tests.sh` never builds the binary it runs.** Build release
   first or it dies on line 79.~~ **Fixed 2026-08-27** — it builds `ply-cli` in
   release itself and aborts on a binary that is absent or older than a source
   in `target/release/ply.d`. §4.
3. ~~**`--no-incremental` is not `--no-cache`.** `same-tests.sh` step 1 can print
   `0 passed` and exit 0.~~ The distinction is real and still worth knowing —
   `--no-incremental` disables the front-end cache only — but **step 1 stopped
   depending on it on 2026-08-27**: it passes `--no-cache` and then refuses
   unless it sees `cached == 0` and `passed >= 1`. §4.
4. **`README.md`'s `ply-corpus gen` command omits the required `--out`** and
   fails verbatim. §5.
5. **`serve.sh`'s `E0435` comment was false** — the error is raised nowhere.
   **Corrected in the script** by a later audit pass; the comment now carries
   the grep that shows it. §4.
6. **`grep conflict` matches 51 files** across two unrelated conflict notions.
   §6.
7. **`PLY_PG_URL` is set by nothing *locally*.** CI sets it; your machine does
   not, so the live postgres tests pass without running on your machine. `ROADMAP.md`'s gate table
   says that skip "says so, on stderr of a passing test", which is **not true
   under `cargo test`**: cargo captures it, so the default run tells you
   nothing. A fifth gate, `PLY_TEST_DB`, hides 26 more and prints nothing at
   all. §2.
8. **`CONTRACTS.md` carries stale signatures with the correction above them,
   not beside them.** §7.
9. **There is no ADR index.** `docs/adr/` is **nineteen** numbered files and
   nothing else (`ls docs/adr/*.md | wc -l`, re-taken 2026-08-21; this line
   said seventeen, which was true before ADR 0018 and ADR 0019 landed). The
   titles, in order: modules, incremental front end, cache
   storage, machine-shaped failure, control stack and world *(superseded in part
   by 0017)*, deterministic simulation, specs, host effect boundary, effect-set
   aliases, derivation-now-dispatch-deferred, then W1–W6 contracts (0011–0016),
   then regions (0017), compute-kernel performance (0018) and value
   representation (0019).
10. **ADR 0005 is partly superseded by ADR 0017** and 0005 is 46,844 characters.
    The persistent forkable `World` its §2 specifies was removed; regions on a
    bump arena replaced it. Its *title* does not say so — but its header does,
    and this list originally implied you had to work it out: `0005` lines 3–9
    carry `Status: accepted — … §2's persistent forkable world is **superseded
    by ADR 0017**` and `Superseded in part by: docs/adr/0017-regions.md (§2
    only)`, and §2 itself opens with a `> **Superseded by ADR 0017.**` block at
    line 289. Read 0017 first, then 0005 for the parts 0017 kept — §3's
    resumption semantics stand unchanged.
11. ~~**No `LICENSE` file exists** (`ls LICENSE*` → no matches) although
    `README.md:499` and the workspace root `Cargo.toml:22` declare
    `MIT OR Apache-2.0`.~~ **Fixed 2026-08-27**: `LICENSE-MIT` and
    `LICENSE-APACHE` are at the root, checked against three independent copies
    each out of `~/.cargo/registry`.
    itself. ~~`README.md:499`~~ was already stale when it was written — the file
    is 663 lines (658 then; this change added five below `## License`) and the
    licence is at **`README.md:656`**; line 499 is about type aliases. And the
    ~~copyright line, `Copyright (c) 2026 Skyler Berg`, is **inferred** from
    `Cargo.toml:23`'s repository URL and the earliest date in the prose,
    because nothing in the tree names a holder or a year. A human should
    confirm it.~~ **A human did, on 2026-08-28, and the answer was to drop it:
    `LICENSE-MIT` now carries no copyright line and begins at `Permission is
    hereby granted`. CONTRIBUTING item 7 carries what that costs.** The rest stands: all thirteen member crates inherit the
    expression with `license.workspace = true`, and only
    `crates/ply-codegen-spike/Cargo.toml` declares no license, being its own
    workspace — still true, and deliberately left alone.

~~Items 2–5, 7 and 11 are one-line fixes.~~ **Item 5 has since been made** — it
was a false comment rather than a behaviour, so a documentation pass could fix
it, and did. **Items 2, 3 and 11 were made on 2026-08-27**, and none of them was
a one-line fix: the one-line versions (add a `cargo build`, change one flag, drop
in a licence text) would each have shipped a check nobody had watched fail, or a
copyright line nobody had sourced. For items 4 and 7 read
`CONTRIBUTING.md` §"Things known to be broken", which is where all of these are
tracked and where their current state is: its item 4 records the `--out` fix at
`README.md:97`, which this list's item 4 above still describes as open, and its
item 6 records `PLY_PG_URL` as half fixed — set in CI, set by nothing locally.

## 10. The documents, and what each is for

| file | what it is | read it when |
| --- | --- | --- |
| [`README.md`](../README.md) | the measured claims, with corrections inline | first, for the numbers |
| [`DESIGN.md`](../DESIGN.md) | the language and the reasoning; §"What of this is built" is the honest state table | you need to know what a mechanism *means* |
| [`docs/GUIDE.md`](GUIDE.md) | the user-facing manual: syntax, types, effects, tests, specs, stdlib, CLI, every diagnostic code | you need to know how to *write* Ply |
| [`ROADMAP.md`](../ROADMAP.md) | milestone-by-milestone record; **§"What is next" is the queue** | you need to know what to do |
| [`CONTRACTS.md`](../CONTRACTS.md) | the crate-construction contract, 7,650 lines | you need a signature — and see §7 |
| [`docs/adr/`](adr/) | **nineteen** decisions with their arguments, `00NN-slug.md`, no index | you want to know *why*, and are prepared for **16,353** lines (`cat docs/adr/*.md \| wc -l`, re-taken 2026-08-21 by the second regression audit; it read 16,100 before ADR 0019 §7; this row said "seventeen decisions" and "14,785 lines", which was true before ADR 0018 and ADR 0019 existed, and before that said 24k, which is the whole prose surface and not the ADRs) |
| [`benches/README.md`](../benches/README.md) | what the measurement harness does and its caveats | before quoting any `ply-corpus` number |
| [`CONTRIBUTING.md`](../CONTRIBUTING.md) | how to make a change here | before your first commit |

`DESIGN.md`, `ROADMAP.md` and `README.md` all carry audit-note blocks correcting
things they used to say. Those blocks are the most reliable prose in the
repository, because each was written against a measurement. The unmarked prose
around them is the part to be careful with.
