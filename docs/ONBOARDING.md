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

Everything below was measured on **2026-08-17**:

| | |
| --- | --- |
| machine | Apple M-series, `aarch64-apple-darwin`, 10 cores, 32 GiB |
| OS | macOS 15.7.3 (`sw_vers`), Darwin 24.6.0 (`uname -r`) — `ply-corpus` prints the Darwin release and labels it "macOS 24.6.0", which is why that string appears in its output |
| toolchain | `rustc 1.93.1 (01f6ddf75 2026-02-11)`, `cargo 1.93.1` |
| postgres | PostgreSQL 18.3 (Homebrew), running on `:5432` |

Your wall clocks will differ. The **counts** — 3,597 tests, 5,000 tests
selected down to 157, 7 obligations, 25 host handlers, 29 agreeing requests —
should not. If a count differs on your machine, that is a finding; open it as
one.

> **The test count moved twice on 2026-08-17 and the others did not.** It read
> 3,566 until R3 added three test binaries, then 3,584 until the regression audit
> after R3 added a fourth and its fixes added three tests (§2). The other four
> were re-taken on the same day against this tree and are unchanged: 5,000 → 157
> from `ply-corpus bench`, and 7 / 25 / 29 from `ply prove examples/desk.ply`,
> `ply hosts examples/desk.ply --host` and `examples/same-tests.sh`.

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

Warm (nothing changed) is **0.11s** debug and **0.15s** release, re-measured;
this line said 0.25s for both, which is the right order of magnitude and was not
re-taken when it was written. There is no build script to run, no code generation
step, no submodule, no `make`. Three binaries land:

- `target/{debug,release}/ply` — the language driver
- `target/{debug,release}/ply-corpus` — the measurement harness
- `target/release/w6-alloc` — an allocation counter used by one W6 test

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

### `crates/ply-codegen-spike` does not build. Do not start there.

`crates/` holds fourteen directories but the workspace has **thirteen members**.
`ply-codegen-spike` declares its own `[workspace]` so `cargo build --workspace`
and `cargo test --workspace` never touch it — that is deliberate (ADR 0016 §3.5
wants deleting it to be `rm -r` and nothing else).

What is *not* deliberate is that **it no longer compiles**, on any toolchain
available here. Two independent walls, both reproduced:

```
$ cd crates/ply-codegen-spike && cargo test --release
error: rustc 1.93.1 is not supported by the following packages:
  cranelift-assembler-x64@0.134.3 requires rustc 1.94.0
  cranelift-assembler-x64@0.134.3 requires rustc 1.94.0
  cranelift-assembler-x64@0.134.3 requires rustc 1.94.0
  cranelift-assembler-x64-meta@0.134.3 requires rustc 1.94.0
  ... 22 further lines, 26 in all over 17 distinct packages, ending
  wasmtime-internal-jit-icache-coherence@47.0.3 requires rustc 1.94.0
Either upgrade rustc or select compatible dependency versions with
`cargo update <name>@<current-ver> --precise <compatible-ver>`

$ cargo +1.94.0 test --release          # a 1.94.0 toolchain is installed here
error[E0164]: expected tuple struct or tuple variant, found struct variant `Stmt::Expr`
   --> src/jit.rs:371:21
error[E0164]: expected tuple struct or tuple variant, found struct variant `Stmt::Expr`
   --> src/jit.rs:739:25
error: could not compile `ply-codegen-spike` (lib) due to 2 previous errors
error: could not compile `ply-codegen-spike` (lib test) due to 2 previous errors
```

(This block used to abridge the first wall as one `cranelift-codegen` line plus
"… (23 more)". Both halves of that were wrong: the list is **26** lines, and the
package it opens with is `cranelift-assembler-x64`. `cranelift-codegen` is in
there, on lines 7–9. Re-taken above.)

`ply_eval::code::Stmt::Expr` became a struct variant `{ code, dead }`
(`crates/ply-eval/src/code.rs:121-124`) and the spike still matches it as a tuple
variant. Nothing compiles the spike, so nothing caught the drift.

The command ADR 0016 gives for reproducing its own spike half —
`cargo +1.94.0 run --release --manifest-path crates/ply-codegen-spike/Cargo.toml
-- --half benches/w6-spike.json`, at `docs/adr/0016-w6-performance.md:827` —
fails with the same two `E0164`s. ADR 0016 records the *toolchain* wall twice, at
lines 764–767 and 1105–1106; the source-incompatibility wall is newer and is
recorded nowhere in the repository but here. **ADR 0016's `11.67x`, `1.71x` and
the §9.2 census are therefore not re-takeable from this tree** — note that
`11.67x` is still *checkable*, because `benches/w6-spike.json` holds the five
input pairs it was computed from and `ply-corpus w6` recomputes it from the file
(§8). What cannot be done is measuring the spike again. The ladder half
(`benches/w6-ladder.json`) is unaffected and does reproduce — see §7.

## 2. Test

```
cargo test --workspace
```

Measured, from an already-built `target/`:

| | |
| --- | --- |
| wall clock | **339s** (5m 39s); re-taken at **324.5s**, after R3 at **352.4s** (5m 52s) and **359.7s**, and after the regression audit that followed it at **399.6s** (6m 40s) and **406.9s** — five runs of one command on one machine, which is the spread to expect |
| result | **3,597 passed, 0 failed, 4 ignored** |
| targets | **151** — 138 test binaries + 13 doc-test suites |

That reproduces `README.md`'s Status paragraph exactly, count for count, and
the audit re-ran it and got the same three numbers again (`grep -c '^     Running'`
→ 138, `grep -c '^   Doc-tests'` → 13). It is the longest thing in this file;
budget seven minutes and do not run it under load.

> **Re-taken 2026-08-17, after R3.** This table read **3,566 passed** across
> **147** targets (134 binaries), and the paragraph under it said 134. R3 added
> three test binaries — `ply-eval/tests/region_kind_sharing.rs`,
> `ply-eval/tests/lowering_sharing.rs`,
> `ply-corpus/tests/region_kind_hoisted.rs` — and nine more tests inside
> existing ones. Re-taken as `time cargo test --workspace`, summing the
> `test result:` lines: 3,584 / 0 / 4 in 352.4s, with `grep -c '^     Running'`
> → 137 and `grep -c '^   Doc-tests'` → 13. The wall clock is one run on an
> otherwise idle machine and is not a best-of-N.

> **Re-taken again 2026-08-17, after the regression audit that followed R3.**
> That audit landed a fourteenth new binary,
> `ply-eval/tests/hoist_staleness_audit.rs` (10 tests), which is why `Running`
> reads 138 rather than 137 — the block above was written before that file
> existed and is not wrong about what it measured. Fixing the two defects it
> found added three more: one in that file
> (`a_declared_unique_over_a_local_shadowing_a_definition_is_refused`), one in
> `ply-corpus/tests/w6_report_allocations.rs`
> (`the_readme_still_describes_this_request_path`), and the first doc-test
> `ply-eval` has ever had — a `compile_fail` example on `Lowering`, because a
> variance is a compile-time property no `#[test]` can observe. Re-taken the same
> way: **3,597 / 0 / 4 in 399.6s**, re-taken at **406.9s**, `Running` → **138**, `Doc-tests` → 13.

### Four things a green suite does not prove

`cargo test --workspace` green is weaker than it looks, and the four gaps are
enumerated in `ROADMAP.md`'s preamble table. Two of them will bite you here:

**Ten postgres tests skip unless you set `PLY_PG_URL`, and nothing in the
repository sets it.** They *pass* without running. Measured, both ways:

```
$ cargo test -p ply-host --lib db::scope::tests::live
test result: ok. 10 passed; 0 failed; ... finished in 0.00s

$ PLY_PG_URL=postgres://localhost/postgres cargo test -p ply-host --lib db::scope::tests::live
test result: ok. 10 passed; 0 failed; ... finished in 0.04s
```

Same line, same count, and the only tell is the wall clock. `ROADMAP.md`'s
preamble table says this gate "says so, on stderr of a passing test" — **it does
not, under `cargo test`.** Cargo captures the output of a passing test, so under
a normal `cargo test --workspace` run the notice is invisible; the whole-workspace
run above contains the string `skipped:` **zero** times — re-checked against both
its stdout and its stderr. You only see it with `--nocapture`:

```
$ cargo test -p ply-host --lib db::scope::tests::live -- --nocapture
skipped: PLY_PG_URL is unset, so the scope table was not run against real postgres
```

To actually run them:

```
PLY_PG_URL=postgres://localhost/postgres cargo test -p ply-host
```

Measured: **38s, 281 passed, 0 failed**; re-taken at 36.4s with the same 281/0
(253 + 2 + 1 + 2 + 5 + 1 + 15 + 2 across nine targets, one of which is a 31.5s
pool test). The gate is
`crates/ply-host/src/db/scope/tests/live.rs:101`; it creates and drops scratch
tables named `ply_scope_<pid>_<n>` in whatever database you name, so point it at
one you do not mind that happening in.

**`crates/ply-cli/tests/w5_shutdown.rs` is `#![cfg(unix)]`** and on a non-Unix
host is not compiled and prints nothing at all. Every W5 shutdown claim is
unproven there, silently.

The other two gates are `cluster::available()`
(`crates/ply-host/tests/support/cluster.rs:39` — `ROADMAP.md` cites `:38`, which
is the blank line above it; skips without `initdb` or `postgres` on `PATH`, and
says so on stderr) and the codegen spike's own workspace (§1).

### One test is wall-clock sensitive and runs by default

`ply-eval/tests/region_arena_cost.rs::snapshot_cost_as_a_function_of_region_size`
asserts on a timing growth ratio and is **not** in the `ignored` set. It passed
here, and passed again on the audit's re-run. On a machine busy compiling
something else it has been seen to fail. If it is your only failure, re-run on a
quiet machine before you believe it.

The four `ignored` tests, verbatim from the run: three timing benchmarks in
`ply-corpus/tests/http_cost.rs`, each of which prints its own recipe —

```
test the_cost_of_a_head_is_linear_in_the_number_of_fields ... ignored, timing;
  run with `cargo test -p ply-corpus --test http_cost -- --ignored --nocapture`
```

— and one doc-test, `crates/ply-host/src/db/pool.rs - db::pool::job (line 107)`.
That matches `README.md`'s "three are timing benchmarks … and the fourth is a
doc-test, not a benchmark."

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

**Build the release binary first.** `same-tests.sh` uses `target/release/ply`
and, unlike `serve.sh`, never builds it — on a fresh clone it dies at line 79
with "No such file or directory":

```
cargo build --workspace --release
./examples/same-tests.sh
```

Measured: **4.6s**, exit 0, and the tail is

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

**Read step 1's counts, not just its exit code.** Step 1 runs
`ply test examples/desk.ply --no-incremental`, and `--no-incremental` disables
only the *front-end* cache — the result cache is untouched (`ply test --help`
says so explicitly; `--no-cache` is what disables both). So on a warm
`examples/.ply-cache` step 1 prints

```
   selected 0 of 68 (68 cached)
   0 failed, 0 passed, 68 cached (0.00s)
```

and the script still exits 0. Nothing ran. With a cold cache the same step
prints `0 failed, 68 passed, 0 cached (0.09s)`. If you are using step 1 as
evidence, run `ply cache clear` in `examples/` first.

### `serve.sh` — a service you can curl

```
./examples/serve.sh --memory                          # no database at all
./examples/serve.sh --db postgres://localhost/desk    # needs examples/desk.sql loaded
./examples/serve.sh --db postgres://localhost/desk --tls
```

It copies `desk.ply` into `examples/.serve/` and rewrites one line *there*, so
your working tree is not modified (`PLY_SERVE_OUT` moves that directory). Unlike
`same-tests.sh` it does run `cargo build --release -p ply-cli` itself. It prints
a generated `DESK_API_KEY` if you have not exported one.

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
hits and not one of them constructs a diagnostic: `crates/ply-span/src/lib.rs:414`
defines the constant, `:787` registers it, `crates/ply-eval/src/host.rs:1106`
lists it as reserved, and `crates/ply-cli/src/artifact.rs:253` and
`crates/ply-cli/src/db.rs:539` are comments describing the check as future work.
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
   **`crates/ply-cli/tests/cli.rs:145 renaming_a_definition_re_runs_nothing`**
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

### There is no CI

**`.github/` does not exist.** No workflow, no hook, no pipeline. Every "the
test suite asserts it" in every document means *someone has to run
`cargo test --workspace` locally, and nobody is watching whether they did.*
Assume nothing has been run since the last person who said so.

### Exactly one test reads a prose document, and it reads one sentence of it

> **This section read "No test reads any prose document" and that was true when
> it was written.** A regression audit on 2026-08-17 found `README.md`'s
> request-path allocation figure stale for the second time in one milestone —
> the second time *inside the block correcting the first* — and armed the one
> sentence rather than writing the finding down again. What follows is re-taken.

Checked: `grep -rn '\.md"' crates/*/tests crates/*/src` filtered to file-opening
calls returns **one** hit,
`crates/ply-corpus/tests/w6_report_allocations.rs:163
the_readme_still_describes_this_request_path`. It reads `README.md`'s *"One
`/health` request makes N allocations and M bytes"* and compares both numbers
against a freshly counted 200-request window, within **1%** — a tighter band than
the factor of two the test beside it holds `benches/w6-ladder.json` to, because
that file is a dated artifact and this sentence is present tense about this tree.
Run it: `cargo test -p ply-corpus --test w6_report_allocations -- --nocapture`
prints both sides, and the count is the same in debug and release because an
allocation count does not vary with a profile.

Nothing else. No test opens `DESIGN.md`, `ROADMAP.md`, `CONTRACTS.md` or any
ADR, and no other sentence of `README.md` is read. Every other number, signature
and guarantee in the prose surface is unenforced by the suite — **24,951 lines**
across
`README.md`, `DESIGN.md`, `ROADMAP.md`, `CONTRACTS.md` and the seventeen ADRs
(`cat DESIGN.md ROADMAP.md CONTRACTS.md README.md docs/adr/*.md | wc -l`), or
**26,777** counting `benches/README.md`, `CONTRIBUTING.md` and this file. Both
figures move whenever anyone edits any of those files, so re-take them rather
than quoting them.

So the checked/written boundary is:

**CHECKED — machine-verified against the tree, will fail if the tree moves:**

- The two *guarded* measurement files, and only those. `benches/` holds three
  since R3 — `w6-ladder-r3.json` is the post-R3 re-take and **nothing reads it**,
  which `benches/README.md` states rather than leaves to be discovered; it is a
  measurement, not a guard. `benches/w6-ladder.json`
  and `benches/w6-spike.json` are read by
  `ply-corpus/tests/w6_report_integrity.rs:304
  the_shipped_ladder_still_describes_the_tree_it_ships_in` and
  `ply-corpus/tests/w6_report_allocations.rs:115
  the_shipped_allocation_evidence_still_describes_this_request_path`. Both run
  under `cargo test --workspace`. Read the doc comment at
  `w6_report_integrity.rs:280-302` — it opens *"the staleness guard, and the one
  this audit exists because nothing had"* — for what they actually assert: rung
  *shape* as a fraction of the framing rung in every profile, and absolute
  microseconds **in release only, within a factor of four either way**. That band
  is deliberately wide and is not a tight guard.
- **One sentence of `README.md`** — its request-path allocation count, both
  numbers, within 1%, by `w6_report_allocations.rs:163
  the_readme_still_describes_this_request_path`. See the section above for why
  that one and nothing else.
- Behavioural invariants stated as tests, e.g. the rename invariant at
  `ply-cli/tests/cli.rs:145`. There are 3,597 tests; how many of them pin a
  documented guarantee rather than an implementation detail is **not measured
  and no document claims a figure for it.**
- Anything you can re-run from this file. The loop numbers, the selection table,
  the obligation counts, the host-handler counts and `same-tests.sh`'s 29
  agreements all reproduced in this audit.

**WRITTEN — true when someone typed it, unverified since:**

- Every prose claim in `README.md` bar the one sentence above, and every one in
  `DESIGN.md`, `ROADMAP.md`, `CONTRACTS.md`
  and the seventeen ADRs.
- `CONTRACTS.md` in particular is a **construction** document, not an API
  reference. Its own preamble says so: *"Crates are implemented concurrently
  against them, so a signature here is a promise other crates have already been
  written to call."* It describes what was to be built, and the tree has moved
  under it — `World` occurs **37 times on 33 lines** (`grep -o '\bWorld\b'
  CONTRACTS.md | wc -l` against `grep -c World CONTRACTS.md`; this line said "33
  times", which is the line count, not the occurrence count) and the type it
  means is gone. Be precise about "gone", because `CONTRACTS.md:1516`'s own
  correction block is not: it says *"there is no `ply_eval::world`, no `World`
  and no `CellId` in `crates/`"*, and `grep -rn '\bWorld\b' crates/*/src`
  returns **20 hits**. Nine of them are a real, live, *unrelated* type —
  `ply_corpus::model::World` (`crates/ply-corpus/src/model.rs:269`), a struct in
  the corpus generator's cost model — and the other eleven are doc comments in
  `ply-eval` and `ply-test` explaining what the removed type used to do
  (`task_regions.rs` ×7, `arena.rs:760`, `ply-test/src/region.rs` ×2,
  `ply-test/src/schedule.rs:4`). What is actually true is the narrower claim,
  and it does check out: there is no `ply_eval::world` module
  (`ls crates/ply-eval/src/world*` → no matches), no `ply_eval` `World`, and no
  `CellId` type anywhere — the only two `CellId` strings in the tree are prose
  at `crates/ply-eval/src/escape.rs:9` and `:29`. There is a
  correction block at `CONTRACTS.md:1516` with a translation table
  (`World` → `TaskRegions`/`Arena`, `World::cells` → `Arena::slots`, and so on),
  but the stale signature at `:1535` is nineteen lines below it. Read upward for
  a correction block before you believe any signature there.
- ADR 0016's spike numbers, which as of §1 above cannot be re-taken at all.

### How the corrections are written, and why

Where an audit found a false claim, the convention is to **correct in place and
keep the original beside the measurement** rather than delete. `README.md` uses
`> **Corrected: …**` blocks; `ROADMAP.md` uses `> **Audit note …**`. A withdrawn
claim that was silently removed teaches nobody. Follow the convention — see
`CONTRIBUTING.md` §"Writing a claim down".

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
2. Evidence passing and handler specialization.
3. Re-measure codegen's ceiling — **before** re-arguing M9. The *ladder* half has
   been re-taken and ships as `benches/w6-ladder-r3.json`; render it with
   `./target/release/ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`.
   The *spike* half still cannot be re-taken, because the spike does not build;
   see §1. `ROADMAP.md` §"What is next" item 3 now records that blocker.

Plus two small recorded obligations: delete `crates/ply-codegen-spike` per ADR
0016 §3.5, and fix `Machine::constant` refusing the memo inside any open
scheduler region, which costs a spawning service 1.77x on `/health`.

### M9 (native codegen) is deferred, not forgotten, and you can re-derive it

M9 is the one planned milestone that has not landed. It was deferred by W6
against criteria **fixed in code before any number existed**
(`ply_corpus::w6::Criteria::default()`), so the decision is re-derivable rather
than re-arguable. Run it:

```
./target/release/ply-corpus w6 benches/w6-ladder.json benches/w6-spike.json
```

> **Two ladders exist since R3, and the line above renders the older one on
> purpose** — it is the W6 file the verdict quoted below came off, kept because
> it is also the only record of the pre-region allocation baseline. For the
> current tree substitute `benches/w6-ladder-r3.json`, taken 2026-08-18: the
> verdict is the same (`keep deferring M9`) and the numbers inside it move a
> little — share 35%, ceiling **1.53x**, projection **1.46x**. **Do not run
> `ply-corpus w6 benches/*.json`**: `w6` merges last-wins and the glob sorts
> `-r3` first, so it silently renders the older file. `benches/README.md`
> §"There are two ladders" is the full note.

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

1. **`crates/ply-codegen-spike` does not compile** — two walls, neither
   recorded outside this file. §1.
2. **`examples/same-tests.sh` never builds the binary it runs.** Build release
   first or it dies on line 79. §4.
3. **`--no-incremental` is not `--no-cache`.** `same-tests.sh` step 1 can print
   `0 passed` and exit 0. §4.
4. **`README.md`'s `ply-corpus gen` command omits the required `--out`** and
   fails verbatim. §5.
5. **`serve.sh`'s `E0435` comment was false** — the error is raised nowhere.
   **Corrected in the script** by a later audit pass; the comment now carries
   the grep that shows it. §4.
6. **`grep conflict` matches 51 files** across two unrelated conflict notions.
   §6.
7. **`PLY_PG_URL` is set by nothing**, so ten postgres tests pass without
   running — and `ROADMAP.md`'s gate table says that skip "says so, on stderr of
   a passing test", which is **not true under `cargo test`**: cargo captures it,
   so the default run tells you nothing. §2.
8. **`CONTRACTS.md` carries stale signatures with the correction above them,
   not beside them.** §7.
9. **There is no ADR index.** `docs/adr/` is seventeen numbered files and
   nothing else. The titles, in order: modules, incremental front end, cache
   storage, machine-shaped failure, control stack and world *(superseded in part
   by 0017)*, deterministic simulation, specs, host effect boundary, effect-set
   aliases, derivation-now-dispatch-deferred, then W1–W6 contracts (0011–0016),
   then regions (0017).
10. **ADR 0005 is partly superseded by ADR 0017** and 0005 is 46,844 characters.
    The persistent forkable `World` its §2 specifies was removed; regions on a
    bump arena replaced it. Its *title* does not say so — but its header does,
    and this list originally implied you had to work it out: `0005` lines 3–9
    carry `Status: accepted — … §2's persistent forkable world is **superseded
    by ADR 0017**` and `Superseded in part by: docs/adr/0017-regions.md (§2
    only)`, and §2 itself opens with a `> **Superseded by ADR 0017.**` block at
    line 289. Read 0017 first, then 0005 for the parts 0017 kept — §3's
    resumption semantics stand unchanged.
11. **No `LICENSE` file exists** (`ls LICENSE*` → no matches) although
    `README.md:499` and the workspace root `Cargo.toml:22` declare
    `MIT OR Apache-2.0`, and all thirteen member crates inherit it with
    `license.workspace = true`. (Only `crates/ply-codegen-spike/Cargo.toml`
    declares no license, being its own workspace.)

Items 2–5, 7 and 11 are one-line fixes. **Item 5 has since been made** — it was
a false comment rather than a behaviour, so a documentation pass could fix it,
and did. The rest are behaviour or absent files and are still open;
`CONTRIBUTING.md` §"Things known to be broken" is where they are recorded for
whoever takes them.

## 10. The documents, and what each is for

| file | what it is | read it when |
| --- | --- | --- |
| [`README.md`](../README.md) | the measured claims, with corrections inline | first, for the numbers |
| [`DESIGN.md`](../DESIGN.md) | the language and the reasoning; §"What of this is built" is the honest state table | you need to know what a mechanism *means* |
| [`ROADMAP.md`](../ROADMAP.md) | milestone-by-milestone record; **§"What is next" is the queue** | you need to know what to do |
| [`CONTRACTS.md`](../CONTRACTS.md) | the crate-construction contract, 7,650 lines | you need a signature — and see §7 |
| [`docs/adr/`](adr/) | seventeen decisions with their arguments, `00NN-slug.md`, no index | you want to know *why*, and are prepared for 14,785 lines (this row said 24k, which is the whole prose surface, not the ADRs) |
| [`benches/README.md`](../benches/README.md) | what the measurement harness does and its caveats | before quoting any `ply-corpus` number |
| [`CONTRIBUTING.md`](../CONTRIBUTING.md) | how to make a change here | before your first commit |

`DESIGN.md`, `ROADMAP.md` and `README.md` all carry audit-note blocks correcting
things they used to say. Those blocks are the most reliable prose in the
repository, because each was written against a measurement. The unmarked prose
around them is the part to be careful with.
