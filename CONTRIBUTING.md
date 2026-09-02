# Contributing to Ply

Read [`docs/ONBOARDING.md`](docs/ONBOARDING.md) first. It gets you from a fresh
clone to a running server and a verified change, with measured timings for every
step. This file is what to do *after* that: how to make a change here without
breaking something, and — the part this project cares about most — how to write
a claim down.

- [The one rule](#the-one-rule) — and [the shape it keeps taking](#the-shape-it-keeps-taking-declared-registered-raised-nowhere)
- [The loop](#the-loop)
- [Before you open a change](#before-you-open-a-change)
- [Writing a claim down](#writing-a-claim-down) — including [a moving tree invalidates a correctness number](#a-moving-tree-invalidates-a-correctness-number-and-only-an-instrument-says-so)
  and [the binary is an instrument too](#the-binary-is-an-instrument-too-and-the-rule-for-checking-it-was-blind)
- [Adding things](#adding-things) — a diagnostic code, a host handler, an ADR
- [Where a change is likely to bite](#where-a-change-is-likely-to-bite)
- [Things known to be broken](#things-known-to-be-broken)
- [Style](#style)

## The one rule

**A claim is true only if you checked it against the thing that would satisfy
it.** "The ADR says so" is not evidence. Read the file, run the command, grep the
symbol.

This is not a slogan. Across sixteen milestones, adversarial review of this
repository found **seven** written claims that did not hold, and it is the most
expensive defect class the project produces — because a stated guarantee is
exactly what stops the next reader looking. The seven:

- M7 reported `exhaustive: true` over regions never examined.
- M8 disclosed an unsoundness and named a mitigation structurally incapable of
  firing.
- W1 advertised a footprint check that was never armed.
- W4 refused `DISCARD ALL` and documented the consequence nowhere.
- W5 listed a secret-exfiltration route as closed while the backstop covered
  five of seven map operations.
- R1's ADR asserted that snapshot-at-capture "is exactly ADR 0005's semantics".
  It was not; the two are distinguishable in one integer.
- R2 found `CONTRACTS.md` specifying a deleted type, and a required property
  claiming constant time for something linear.

The last two were documentation about *correct code*. Correct code does not
protect you here.

### The shape it keeps taking: declared, registered, raised nowhere

Several of the failures above are the same defect wearing different clothes — **a
mechanism that is *named* everywhere a reader would look for it and *constructed*
nowhere.** A diagnostic code in the registry that no call site raises. An enum
variant matched in every `match` and produced by nothing. A mitigation described
in an ADR with no code path that reaches it. Each reads, at every point a
reviewer would check, exactly like a mechanism that works.

`crates/ply-span-tests/tests/armed.rs` is what now catches it. It walks production
source — `#[cfg(test)]` items blanked — and fails when a registered code or a
covered enum variant is never constructed outside tests. Something genuinely
reserved goes in `UNARMED_CODES` or `UNARMED_VARIANTS` **with a reason and a
citation**, which is the whole point: it stops "reserved on purpose" and "we
forgot" looking identical.

> **The gate once had the defect it polices, which is worth knowing before you
> trust it.** Its scanner walked from `#[cfg(test)]` to the first `{` *or `;`*
> and blanked only the `{` case, so an item whose header contains a `;` before
> its body stayed in the production set — and one did, where the `;` belonged to
> an array type. A diagnostic constructed in that body would have armed its code
> with the gate green. Nothing in that body armed anything, so no answer was
> wrong; it was luck, not the rule.
> `a_cfg_test_item_is_not_production_whatever_its_header_looks_like` fails
> against the old walk.

**The lesson generalises past diagnostics.** When you add a mechanism that is
*registered* somewhere — a table, an enum, a handler list — ask what would fail
if it were never constructed. If the answer is nothing, you have written a
declaration rather than a mechanism, and the next reader will believe it works.
## The loop

The inner loop is fast; use it.

```
cargo build --workspace                         # seconds warm
cargo nextest run -p <crate> -p <crate>-tests   # seconds
./target/debug/ply test examples/               # under a second
```

A crate's integration suite is a package of its own, `crates/<crate>-tests`, so
that it compiles at `opt-level = 0` while the crate stays at 2; the root
manifest says why. Select the pair: cargo hands a package's binaries only to
that package's own tests, and a test that looks for one beside itself skips
when it is missing. `ply-cli` keeps its suite in-package for exactly that
reason. Unit tests stay in `src/`.

The outer loop, run before you call anything done:

```
cargo fmt --all --check                         # must be silent
cargo clippy --workspace --all-targets          # must be 0 warnings
cargo nextest run --workspace                   # must be 0 failed
```

**The suite runs under [nextest](https://nexte.st)**, which runs every test in
every binary as one pool, one process per test, where `cargo test` runs the
binaries one after another and lets one long test hold a binary's tail while
the machine idles. Install it once with `cargo install cargo-nextest --locked`
or from <https://nexte.st/docs/installation/pre-built-binaries/>; CI pins the
version in `.github/workflows/ci.yml`. `cargo test --workspace` still works and
is what runs doctests, of which there are none today. `.config/nextest.toml` is
the configuration: the wall-clock tests `.github/ci-shards.sh` names run last
and alone, with every test thread and their printed measurement shown, so a
run's tail is fourteen short tests one at a time rather than a ratio taken
under contention.

**No test count here, and no wall clock to the decisecond.** Both change on
commits that have nothing to do with either, nothing in the tree checks them, and
every re-take this file used to carry found them stale without anything having
failed. What matters at this line is that nothing failed. Take the wall clock
only on an idle machine — see the next section, and expect a wide spread if you
ignore it.

> **The heaviest test peaks near 4.2 GiB.** If the suite dies on a small
> machine, that is where to look first.

> **`PLY_PG_URL` is unset for most local runs**, so the live postgres tests pass
> without running. That is the gate every unqualified local reading is taken
> under; `docs/ONBOARDING.md` §2 has the rest of them.
### There is CI

`.github/workflows/ci.yml` runs on every pull request and on pushes to `main`:
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, the whole test suite, `examples/same-tests.sh`, and the spikes.

**Most of why it exists is the gates**, not the suite. A gate that returns a
*passing* result when its dependency is absent is the thing a human is least
likely to notice, so CI forces each open and **fails when it finds the notice a
skipped test prints**. `PLY_PG_URL`, `cluster::available()`, `#![cfg(unix)]` and
`crates/ply-codegen-spike` are asserted that way. `PLY_TEST_DB` has no notice —
those tests print nothing whatsoever when they skip — so it is pre-flighted
instead, with `test -n` and a live `SELECT 1`.

**A skipped job is not a green tick.** The `ci` aggregate job uses `if: always()`
and compares every `needs` result against `success`, so a job that is skipped —
because something it needed was skipped, or because a matrix produced no legs —
turns the required check red rather than reporting green over nothing run.

**`fmt` and `clippy` run alongside the test jobs, not ahead of them.** They are
required by the `ci` aggregate, so a lint slip is still a red check; what they
no longer do is hold every shard back by their own wall clock on a green run.
Watch a run with `gh pr checks <n> --watch --fail-fast`, which exits at the
first failed check, so a formatting slip surfaces when `fmt` finishes rather
than when the last shard does.

**The test job is sharded**, because one runner running everything is too slow.
`.github/ci-shards.sh` holds the partition and `verify` fails if a workspace
member is in no shard, in two shards, or named and absent from the tree — a
partition is a chance to lose a package silently, and a package nothing builds
is this repository's most expensive defect class. The same check covers
`spikes/`, which was added after `spikes/ply-parser` sat in **no CI job at all**
while its differential went red for two days, and `.config/nextest.toml`, which
must name exactly the wall-clock tests the script's table names: each shard
runs those last and alone and asserts, by exact name, that each one in its
packages ran.

Two things CI deliberately does not run, both recorded in §"Things known to be
broken": `./spikes/ply-parser/run.sh --arm`, and
`crates/ply-codegen-spike`'s agreement corpus.
### The suite proves less than it looks like it proves

Green is weaker than it looks, for two separate reasons.

**Gates.** Several suites pass *without running* when a dependency is absent —
`PLY_PG_URL`, `PLY_TEST_DB`, postgres binaries on `PATH`, `#![cfg(unix)]`, and
`crates/ply-codegen-spike`, which declares its own `[workspace]` and is therefore
compiled by nothing in `--workspace`. `docs/ONBOARDING.md` §2 has the table and
how CI forces each open. The worst is `PLY_TEST_DB`: it prints nothing at all
when unset, so a green run is indistinguishable from a run against a database.

**Wall-clock assertions.** Some tests assert on elapsed time and run by default.
`.github/ci-shards.sh`'s `DEFERRED` table lists them, CI runs each alone and
single-threaded, and the parallel shards skip them. **That table is maintained by
running the shards, not by surveying the tree** — two surveys declared themselves
complete and each was proved wrong within the hour by a shard going red on a test
neither had found. One of them reads no Rust clock at all: it parses milliseconds
out of `ply test`'s own output, so no timing vocabulary appears in it.

If you add a test that asserts on elapsed time, add it there too — and prefer
asserting on a **count** (allocations, copies, entries) over a duration wherever
the question allows it. A count does not depend on what else the machine is
doing, which is why the allocation-attribution suites are worth their weight and
the timing suites need a job of their own.

**By-hand obligations the suite does not carry:** `./spikes/ply-parser/run.sh
--arm`, which is the evidence that spike's differential can go red at all, and
`crates/ply-codegen-spike`'s agreement corpus. Both are recorded in
§"Things known to be broken".
## Before you open a change

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`,
   `cargo nextest run --workspace` — all three, all clean.
2. If you changed behaviour, there is a test asserting the new behaviour, named
   as an English sentence. See §Style.
3. If you changed a *guarantee*, you found and updated the document that states
   it. `grep -rn` the guarantee's words across `README.md`, `DESIGN.md`,
   `ROADMAP.md`, `CONTRACTS.md` and `docs/adr/`. **One sentence of that whole
   prose surface is read by a test** — `README.md`'s request-path allocation
   count, by `w6_report_allocations::the_readme_still_describes_this_request_path`,
   added after that sentence went stale twice. **For everything else you are the
   only check.** (Re-take the figure rather than quoting it; `docs/ONBOARDING.md`
   §7 gives the command.)
4. If you changed something the shipped measurement files describe, re-take
   them. The command is in `docs/adr/0011-the-web-track.md` §"Provenance"; the
   three tests that will fail otherwise are
   `w6_report_integrity::the_shipped_ladder_still_describes_the_tree_it_ships_in`,
   `w6_report_allocations::the_shipped_allocation_evidence_still_describes_this_request_path`
   and — if you moved what a request allocates at all —
   `w6_report_allocations::the_readme_still_describes_this_request_path`, whose
   band is 1% rather than a factor of two.
5. If you deleted or renamed a public type, `grep` `CONTRACTS.md` for it. It is
   a construction document written ahead of the code and it goes stale silently;
   `World` occurs in it **37 times on 33 lines**, and the `ply_eval` type of that
   name has not existed since ADR 0017. (`crates/ply-corpus/src/model.rs:269`
   defines an unrelated `World` that is alive and well, so grep for
   `ply_eval::world` rather than for the bare word.)

## Writing a claim down

This is the section that exists because of the seven failures above.

### Correct in place; git holds what it used to say

**Correct a false claim where it stands.** Do not leave the old figure beside the
new one, and do not add a block recording that a number moved: git already has
that, with the author, the date and the change that caused it, and prose that
duplicates it costs every future reader while helping none of them.

This repository ran the opposite convention for a long time and the result is
worth knowing. Correction blocks accreted until they were **a quarter to a third
of every major document**, nested four deep in places, and in sixty commits
touching them only two ever made a document *smaller*. Worse than the volume:
the ritual of writing a careful correction block *feels* like the rigor §"The one
rule" demands, so it gets performed instead of the rigor — and readers, human and
otherwise, arrive anchored on how a number used to be rather than asking whether
it should exist.

**Keep a note beside a claim only when the note would otherwise be redone:**

- a **rejected alternative**, with the reason — *`debug = "line-tables-only"` is
  worth 15% of build time and takes unattributed allocations from 8.5% to 98.7%*.
  Without it, somebody runs that experiment again.
- a **trap** — *a stale binary answers questions about the language wrongly*.
- a **wrong instrument** — the right number measured for the wrong question. This
  is the one that a re-take cannot catch, because every figure stands and what
  moved is the inference. ADR 0024 is the worked example.

Each of those is frozen at birth. **If a note could ever need a note of its own,
it should not have been a note** — that is the test, and everything nested in
this repository's history failed it.

**And prefer not writing the number at all.** A figure in prose that no test
reads will go stale, nobody will find out, and the effort of keeping it current
is spent regardless. Put it where the command wrote it, or arm it.
### Gate on an idle machine before measuring, not after

A wall-clock measurement on this machine is worthless above about load 4, and
the way to find that out is not to take forty windows and discard thirty by rule.
R4 §1 spent three sittings doing exactly that. Windows above load 30 spread
0.497–1.607 on a ratio whose true value was about 1.000.

Two cheap habits, the second borrowed from a peer session measuring an unrelated
crate on the same machine and offered explicitly:

- **Spin until the machine is quiet, then measure.** A loop on
  `top proc < 60% CPU && load < 4` costs nothing and replaces a sitting's worth
  of rounds you were going to throw away.
- **Check the instrument, not just the result.** Require the final re-run to
  land within N% of that session's *first* measurement. A run whose own repeat
  disagrees with it has told you the instrument drifted, and no amount of
  averaging fixes that. R4 lacked this check and would have caught its own drift
  sooner with it.

**Pre-register the filter.** Write the load threshold, the statistic and the
decision rule down before any data exists. R4 ended with a cut that cleared its
2% criterion and had to discard it, because the threshold was chosen after
seeing the numbers — the honest row was the underpowered pre-registered one whose
confidence interval straddled the bar. "Re-run until it passes" is not a
protocol.

**And know which of your numbers this even applies to.** Allocation counts,
hashes, interleaving counts and seeded replays are deterministic: they reproduce
to the digit on a burning machine, and three agents in three worktrees confirmed
R4's to the digit at load 40. Only wall clock is at risk. If the deterministic
half carries your claim, an unresolved timing number is a caveat in a harmless
place — say `UNMEASURED` and check in the raw windows so a reader can re-cut
them. That is a better artifact than a number of unknown provenance sitting
where the hole was.

### A moving tree invalidates a correctness number, and only an instrument says so

The section above is about a busy machine invalidating a *timing* number. This
is the same idea on the other axis, and it was learned the same way — by getting
a number, believing it for a minute, and then finding out where it came from.

**What happened (2026-08-24, item 11's fix).** Three ways to get a number that
describes nothing, all three met in one afternoon. Two were measured here and
the third is reported; they are marked apart because that is the point of the
section.

**Measured.** A full-suite log summed to **3,744 passed / 0 failed / 6 ignored
across 144 binaries + 26 doc-test suites**, which is a plausible-looking result
and is not a result at all: 26 is 13 twice. A `cargo test` that had been killed
left a shell holding the log file open, a second run redirected to the same
path, and the file ended up holding both runs. `grep -c '^real'` on it answers
**2**, and `grep '^     Running' | sort | uniq -d` lists the targets counted
twice.

**Measured.** The `rsync`ed copy the run was taken in changed its digest during
the run, with nothing but the suite touching it — because the suite writes into
the *source* tree: `examples/.ply-cache/frontend.dat` and
`tests/fixtures/.ply-cache/`. An instrument that cries contamination at its own
subject is worse than none, which is why the prune list below is part of it.

**Reported, not measured here.** A run that came back **3,689 passed / 6
failed**, the six being exactly the tests that go red when
`ply_eval::compiled::Gate::InternalEffects` is deleted, on a tree where the gate
was present — traced to `crates/ply-core/src/infer.rs` being written *mid-suite*
at 16:52:35, with a debug `eprintln!` in it. That log is not in this session's
hands and the figure is not re-takeable, so it is recorded as an account rather
than as a measurement. The mechanism is worth carrying even so, and the
uncomfortable half of it is this: a mid-suite write does not need a mystery
agent. Editing a file to run one targeted mutation while a background
`cargo test --workspace` is still in flight does it to you, and that is the
likeliest reading of this one.

**The rule.** A suite run is a claim about *a state of the tree*, so name the
state:

- **Run verification in a frozen copy, not in the tree you are editing.** It
  costs one `rsync` and it makes the claim attributable:

  ```
  rsync -a --exclude 'target/' --exclude '.git' --exclude '.ply-cache' \
    ~/.worktrees/ply/<branch>/ /tmp/<branch>-frozen/
  cd /tmp/<branch>-frozen && cargo nextest run --workspace ...
  ```

- **Digest the copy before and after anyway.** Prevention is what gets you a
  number; detection is what tells you a number is void. You want both, because
  a copy proves nothing if you cannot show it did not move:

  ```
  find . -path ./target -prune -o -type f \
      \( -name '*.rs' -o -name '*.ply' -o -name '*.toml' \) -print0 \
    | sort -z | xargs -0 shasum -a 256 | shasum -a 256
  ```

  **Prune `target/` and `.ply-cache/`**, or the instrument reports your own run
  as contamination. Cargo writes `.rs`, `.json` and `.toml` under `target/`; and
  the suite writes **into the source tree** — `examples/.ply-cache/frontend.dat`
  and `tests/fixtures/.ply-cache/` appear where no `.gitignore`d build directory
  is. Both were found by a digest that flagged a copy nothing but the suite had
  touched. Check the instrument the way you would check any other: take the
  digest twice with the tree held still and require the two to match *before*
  trusting a mismatch.

- **Write the log somewhere only this run can write, and check it holds one
  run.** A killed `cargo test` can leave a shell that still has the log open, so
  a second run redirecting to the same path produces a file with two runs
  interleaved in it — and the summing pipeline reads it as one. That is how
  **3,744 passed / 0 failed / 6 ignored across 144 binaries + 26 doc-test
  suites** happened here: 26 is 13 twice. `grep -c '^real'` on the log is the
  one-line check — more than one means the file is a mixture and every count
  taken from it is void.

- **The same trap, one scale down**, is already recorded in
  `crates/ply-eval/src/compiled.rs`'s test-module header: cargo fingerprints on
  second-granular mtimes, so a mutation and a `cargo test` inside the same
  second can be served the *previous* mutation's artifact. That guard covers one
  mutation cycle; this one covers a whole suite. Both are the same sentence — a
  test result describes the bytes that were compiled, not the bytes on disk when
  you read the summary.

None of the three is detectable after the fact without the instrument, which is
the whole reason to arm it before you need it. And the cheapest rule of all:
while a suite is running, do not touch the tree it is reading — start it in a
copy and leave the copy alone.

### The binary is an instrument too

`target/release/ply` is what most measurements here go through, and a stale one
answers questions about the *language* wrongly — not just slowly. It reported
`unknown name 'iterate'` during one documentation pass because the binary
predated the feature by three days, and the pass nearly wrote that down as a
fact about Ply.

Run `.github/binary-is-current.sh target/release/ply` **before** probing, not
after a confusing result:

```sh
.github/binary-is-current.sh && cargo build --release   # if it says STALE
```

It compares the binary's mtime against every input in its dep-info, which covers
the `.ply` standard library and the workspace crates — including
`crates/ply-codegen`, so a change to the JIT takes it from `current` to `STALE`.
That was checked by touching the file rather than assumed.

**The generalisation is the point.** Any instrument you measure through — the
binary, a corpus, a fixture, a saved log — can be older than the tree you are
asking about. When a result is surprising, suspect the instrument before the
tree, and say in the write-up how you established it was current.
### Say how it was checked, or say it was not

Every number gets a provenance: the machine, the profile, the command, and
whether it is one run or a best-of-N. Where something was **not** measured, write
"not measured" — `README.md` §"Where this is not competitive" does this
throughout and it is the most trustworthy section in the repository.

Never quote a figure from another document. Re-take it or cite the file that
holds it (`benches/w6-ladder.json`, `benches/w6-spike.json`) and the command
that renders it.

### Disclosing a gap does not close it

Writing down that something is unsound, unarmed or unbuilt is not the same as
fixing it, and this project has twice shipped a disclosure that read like a
mitigation. If a gap is open, say what an attacker or a wrong caller can
actually do with it, and say whether anything in the tree would notice. "Known
limitation" with no consequence attached is the shape to avoid.

The test: **name the mechanism that would fire.** If you cannot, the gap is open
and the sentence should say so in those words.
### Measure an ADR's motivating claim before accepting the ADR

If an ADR is motivated by a performance argument, that argument needs a
measurement **before** the ADR is accepted — not after the milestone ships.

ADR 0017 is the worked example, and it is the most expensive instance in the
repository. It opened by asserting that the persistent forkable world and the
zero-cost path were "mutually exclusive", reasoning that Perceus-style in-place
update fires only on uniquely-owned values and that forking keeps reference
counts high. It concluded that removing the world was therefore *forced*.

Every sentence of that was reasoned. None of it was measured. R1 and R2 removed
the world across two milestones, and allocations per `/health` went **up**, from
1,035 to 1,122. A later attribution run (`cargo test -p ply-corpus --release
--test w6_alloc_sites -- --nocapture`) showed why: the allocations were never in
the world. They were in `frame::dispatch` (24%), in `code::lower_*` running on
the request path (~24%), and — after R2 — in `region_kind::infer` running at
runtime (~12%). One profiling run before the ADR would have caught it.

**R3 finished the example, and the ending is the part worth learning from.** It
hoisted both compile-time analyses off the request path and they are now at
**0.0 allocations per request** apiece, measurable by the command above. The
figure came back to **1,082** — and 1,082 is still above 1,035, so a milestone
run to a decision rule fixed in advance handed back the answer *the design does
not look justified on this route*. `ROADMAP.md` §R3 records it.

Two lessons, and the second is the expensive one:

- The rule existed before the number, which is the only reason the answer could
  be reported instead of argued with. `ply_corpus::w6::Criteria::default()` is
  the same idea in code.
- **One of the two things R3 was scoped to remove may never have been on the
  request path at all.** The `~24%` for `code::lower_*` above was read off a
  20-request window, and one-time work divided by twenty looks exactly like that;
  the same family reads 33.8% of a 20-request window today while costing nothing
  per request. If you take an attribution, take it at two windows and fit a
  slope — `w6_alloc_sites.rs` does, and its header says why. A ranking is not a
  cost.

Note what *was* measured. R1 measured the isolation cost, the one number that
could argue against the design, and it came back zero before six agents built on
it. That was the right instinct pointed at the wrong claim: the isolation cost
was a tiebreaker, and the premise was load-bearing for the entire milestone.

So: **the claim that motivates the work is the one to measure first**, not the
claim most likely to object to it. If you cannot measure the premise, say in the
ADR that it is unmeasured and what would test it, and do not write "forced".

### Do not state a guarantee you have not armed

Before writing "X is refused / checked / verified", grep for the code that does
the refusing and confirm it can fire. Two live examples of what this catches:

- `E0435 DB_SCHEMA_MISMATCH` is defined, registered, and listed as reserved —
  and **constructed nowhere**. `examples/serve.sh` used to tell you `--db-schema`
  refuses at bind time with it. It does not, and a later audit pass corrected the
  comment in place: `examples/serve.sh:37-54` now carries the claim, the grep
  that refutes it, and what you actually get (`E0433 DB_PREPARE_FAILED`, per
  statement, on first execution). `README.md` §"What is missing" had this right
  all along. This is the W1 defect exactly — a check advertised and never armed —
  and it is left legible rather than deleted for that reason.
- ~~ADR 0011's spike figures cannot be re-taken because the spike does not
  compile.~~ Stale twice over: R4 repaired the crate, and CI's `spike` job now
  builds and tests it on every push. See §"Things known to be broken" item 1,
  which has carried the correction since 2026-08-21.

The test for whether you have armed something: **name the file and line that
raises it, and the test that proves the raise.** If you cannot, write "not
enforced" instead.

For a diagnostic code and for a variant of a covered enum, that test is now
mechanical: `crates/ply-span-tests/tests/armed.rs` fails if nothing in production
constructs it. It does not decide reachability — see §"The shape it keeps
taking" for what it reaches and what it does not — so for anything shaped like
W1's footprint check, the paragraph above still stands on its own.

### Mark what is checked

A reader cannot tell an asserted invariant from an observation. Say which:

> Selecting zero deterministic tests after a rename is an invariant the suite
> asserts — `crates/ply-cli/tests/suite/cli.rs:198
> renaming_a_definition_re_runs_nothing` — not a heuristic.

That sentence is checkable in one grep. "The rename path is safe" is not.

## Adding things

### A diagnostic code

`crates/ply-span/src/lib.rs` holds every code as a `pub const` in `codes` **and**
a registry table in its test module pairing the constant, its name and its
literal number. `every_registered_code_has_its_published_number` asserts both
that no code moved and that no two constants share a number.

Three things a new code has to satisfy, each of them checked by a named test:

1. a `pub const` in `codes` **and** a row in the registry table —
   `the_code_registry_table_is_total_over_the_codes_module` for the row,
   `every_registered_code_has_its_published_number` for the number;
2. a `Diagnostic::error(codes::NAME, ..)` or `Diagnostic::warning(codes::NAME, ..)`
   somewhere in production source —
   `every_registered_code_is_constructed_in_production` — or a row in that
   file's `UNARMED_CODES` giving the reason it is reserved and unraised, which
   is where `E0435` and `E0438` are;
3. if it reaches the constructor through a wrapper instead of literally, the
   wrapper goes in `CODE_INDIRECTION` with a reason —
   `every_diagnostic_constructor_call_names_its_code_literally`. There is
   exactly one today, `Lexer::error` in `crates/ply-syntax/src/lexer.rs`, and it
   is why `E0002 UNTERMINATED_STRING` is armed: with that list empty the check
   reports `E0002` dead, and `E0002` is not dead.

Ranges in use: `E0001`–`E0002` and `E01xx`–`E05xx` for errors (the two
single-digit ones are the generic pair and are easy to miss when you assume the
range starts at `E01xx`), and `W0601`–`W0610` for warnings. There is also a
reserved list in `crates/ply-eval/src/host.rs` (`DB_SCHEMA_MISMATCH` is at
`:1106`) naming codes a *handler* may not answer with. A code appearing in
`codes`, in the registry, and in that list is **still raised nowhere** — that is
exactly `E0435`'s situation, and that list is the reason a file-granularity grep
calls `E0435` live. The reserved list is itself a real, armed restriction
(`is_reserved_code`); it just is not a raise. Nothing has to be remembered here
any more: `every_registered_code_is_constructed_in_production` fails on a code
in that position unless `UNARMED_CODES` says why.

### A host handler

`ply hosts <file> --host` is the trusted-computing-base listing and it is the
review artifact. A new handler must appear there with its footprint, determinism
flag, linearity (`repeatable` / `at-most-once`), whether it blocks, and whether
it can receive a secret. `ply hosts` also prints a digest; if you add a handler,
that digest moves and anything pinning it must move with it.

Adding to the TCB is a decision, not a change. `Cargo.toml`'s dependency
comments record why each of the three non-obvious ones (`rustls`,
`tokio-postgres`, `rust_decimal`) is there. Write the equivalent.

### An ADR

`docs/adr/` is `00NN-slug.md` files with **no index** — the numbers are the
ordering. Pick the next free one — and **nothing you have to
remember decides this any more**: `ply-span:armed:no_two_adrs_share_a_number`
fails when two files share a number, so the check catches you rather than a
reader finding an ambiguous reference months later.

> **Do not pick an ADR number by counting the directory, or by reading the open
> pull requests.** Both have failed here, the second one twice: branches are
> written in parallel worktrees that cannot see one another, so two of them
> picked the same number before either pull request existed. Advice that assumes
> a contributor can see the other contributor's work is not advice this
> repository can follow. `no_two_adrs_share_a_number` in
> `crates/ply-span-tests/tests/armed.rs` is what catches the collision now.

A record superseded in part says so in its own opening, and the record that
supersedes it says which part. ADR 0005 is the model: its opening says the
forkable state it introduced is superseded by ADR 0017, the section that carried
it says what replaced each piece, and ADR 0017 says what it had to preserve. Do
it in both files.

**References between records name the record, not a section.** A section number
is the part that drifts — a rewrite moves every one of them at once — and
nothing in the tree checks them. Code comments name no record at all: a comment
that needs a decision behind it states the property instead, so that reading the
comment is enough.

**Amend an ADR in place; do not append a note saying what it used to say.** This
directory ran the opposite convention and §"Correct in place" is the general
rule; the ADR-specific half is that an amendment belongs in the sentence it
amends, because a reader who reaches the sentence has already believed it. What
stays beside a claim is the small class §"Writing a claim down" names — a
rejected alternative with its reason, a trap, a wrong-instrument correction.


An ADR here is expected to state the criteria *before* the measurement, in code
where possible. `ply_corpus::w6::Criteria::default()` is the model: eight
thresholds that a measurement file cannot supply, so a number cannot set the bar
it is about to clear. That is why the M9 deferral is re-derivable
(`ply-corpus w6 benches/w6-ladder-r3.json benches/w6-spike.json`) rather than
re-arguable. Name the files; see the warning in the gate table above.

## Where a change is likely to bite

| you changed | what breaks quietly |
| --- | --- |
| `crates/ply-hash/src/normalize.rs` | **every cached result everywhere.** The bytes are the identity. A change here is a cache-format change; see `CACHE_VERSION_CHANGED` (`W0603`). |
| `crates/ply-core/src/ty.rs` `conflicts_with` | test scheduling, silently — tests still pass, they just stop running concurrently, or start racing |
| `crates/ply-test/src/schedule.rs` `group_by_conflict` (`:216`) | same; `parallelism()` at `:172` is what reports it |
| `crates/ply-eval/src/code.rs` | `crates/ply-codegen-spike`, which **nothing in the workspace compiles**. It has now bit-rotted this way twice — `Stmt::Expr` becoming a struct variant, then `NodeKind::Lit` widening to `Lit(Lit, Value)` under R4. It builds today, on the default toolchain since the move to cranelift 0.132.3 (this used to read `cargo +1.94.0 test --release`): `cd crates/ply-codegen-spike && cargo test --release`. Run that after touching this file, or the only instrument for pricing codegen stops answering. CI's `spike` job runs exactly that command, so a break is caught at the pull request rather than at the next re-take |
| how a `Value` is built or shared | `crates/ply-corpus-tests/tests/r4_value_construction.rs`, the attribution ADR 0019's thresholds are fractions of. Two traps: **its attribution needs full debuginfo** — `[profile.dev] debug = "line-tables-only"` was measured on 2026-08-31 and takes unattributed from 8.5% to 98.7%, which the root `Cargo.toml` records as the reason that profile knob is not taken — and its rule table is matched against a **three-frame window whose contents differ by profile** — a rule verified only in release can leave the same allocation unattributed in debug and fail the residue ceiling there. Check both. ADR 0019 is the worked example |
| the request path | `benches/w6-ladder.json` and the two integrity tests, and the M9 verdict that reads it. Also `README.md`'s one guarded sentence — re-take it with `./target/release/w6-alloc --repo . --requests 200`, which reads **773.4** on this tree |
| `Value::cmp`, `values_equal`, or how a `Map` key is stored | the four guarantees the note on `ply_eval::Map` lists. `cmp` is deliberately **coarser** than rendering at `Decimal` (`1.50m` and `1.5m` are one key and two strings), so a key is reduced to one representative per class by `ply_eval::value::canonical_key` before it is stored — `ply_eval::value::insert_key` is the single site, and adding a second one re-opens a defect that made `map_keys` a function of insertion history for four milestones. Any new coarseness in `cmp` needs a matching arm there. `map_order.rs`, `value_semantics_audit.rs` §5 and `derivation_determinism_audit::a_decimal_keyed_map_encodes_one_body_whichever_spelling_was_written_last` are what fail; `docs/adr/0019-value-representation.md` §7 is the write-up |
| `collect_refs_inner` in `crates/ply-core/src/infer.rs` | the compiled seam's effect gate, silently. It is one walk answering two questions — the names a body mentions, and whether the body is written with `perform` or `handle` — and `Checker::mark_internal_effects` propagates the second to a fixpoint over the first. Widen the name set and definitions stop being enterable; narrow it and a definition that performs becomes enterable, which is `CONTRIBUTING.md` item 11 again. The `match` is exhaustive with no wildcard on purpose, so a **new** `ExprKind` fails to compile here rather than defaulting to "pure" — do not add a `_ =>` arm |
| the `Builtin` enum in `crates/ply-eval/src/builtins.rs` | **four checks at once, by omission.** `every_builtin_is_reachable_by_the_name_it_reports`, `exactly_the_callback_builtins_are_higher_order`, `tests::every_builtin_checks_its_argument_count` and `region_kind::tests::the_callback_builtins_are_the_six_this_module_knows` all *iterate* `Builtin::all()`, so a variant left out of `all()` is never named and therefore never checked by any of them — the suite stays green over a builtin nothing has looked at. Deleting `Builtin::ListAt` from `all()` was run against the reachability test on exactly that assumption and it stayed green. `builtin_all_is_complete_and_lists_each_name_once` was written for it; it pins the whole name list, so adding a builtin means adding its name there, and that is the point rather than the cost |
| `Builtin::arity()` | **an arity that is too *wide*, and nothing else.** ~~Nothing that a well-typed program meets~~ — corrected 2026-08-30: `builtins::call` reads `b.arity()` on every call (`builtins.rs:558`; `region_kind.rs:1086` and `value.rs:169` read it too), so an arity *narrower* than the truth reddens every test that calls the builtin, at run time. `every_builtin_checks_its_argument_count` asserts the *declared* arity is enforced, not that it is right — giving `list_at` an arity of `(2, 3)` leaves it green, because `(2, 3)` still refuses one argument and still refuses four and no well-typed call can reach the third slot. `assert` and `range` are both `(1, 2)` over schemes of 1 and 2 arguments, i.e. the table has already drifted twice in exactly that direction. Pin the argument count where it bites, which is a `ply-core` test that a call with the wrong number of arguments does not check |
| any public signature | `CONTRACTS.md`, which no test reads |
| `examples/desk.ply` | `examples/serve.sh`, whose `rewrite()` (`serve.sh:103-112`) matches exact source lines with `grep -qF` and aborts loudly if one is missing — that abort is deliberate and is the good case. How many lines it rewrites depends on the mode: `--memory` rewrites two (`:122` and `:125`), `--tls` rewrites one (`:127`), and a plain `--db` run rewrites none |

## Things known to be broken

Recorded here so nobody spends an afternoon rediscovering them. **Open items
only** — a closed item in a list of what is broken costs every future reader and
helps none of them, and git holds the ones that were fixed. Numbers are the
original ones and the gaps are where closures used to be.

3. **`examples/same-tests.sh` step 1 can be vacuous.** It passes
   `--no-incremental`, which disables only the front-end cache; on a warm
   `examples/.ply-cache` it prints `0 failed, 0 passed, 68 cached` and the script
   exits 0. `--no-cache` is the flag that forces the run.~~ **Fixed 2026-08-27,
   and the diagnosis was right on both halves.** Re-measured on this tree, one
   warm `examples/.ply-cache`, the two flags back to back:

   ```
   $ ply test examples/desk.ply --no-incremental   # the withdrawn step 1
   0 failed, 0 passed, 68 cached (0.00s)           # exit 0
   $ ply test examples/desk.ply --no-cache         # what step 1 passes now
   0 failed, 68 passed, 0 cached (0.10s)           # exit 0
   ```

   Step 1 passes `--no-cache`. The flag alone is not the fix, though: a step
   that trusts an exit status is one flag away from being vacuous again, so step
   1 now **reads its own counts** and refuses on them — `cached == 0` and
   `passed >= 1`, never `passed == 68`, so adding a test cannot turn the script
   red. Seen to fail: with the flag reverted to `--no-incremental` on a warm
   cache the script exits **1** with `step 1 served 68 test(s) from the result
   cache`; with one `test` block in `examples/desk.ply` falsified (`4.00m` →
   `4.01m` at `:2017`) it exits **1** at step 1 on `1 failed, 67 passed, 0
   cached`, and exits 0 again once the file is restored from a byte copy and
   `cmp`-verified.

   The counts line is printed by `print_summary` at
   `crates/ply-cli/src/commands/test.rs:1016`, not by
   `ply_test::RunReport::summary` (`crates/ply-test/src/report.rs:220`) — the
   two build the same shape and nothing pins either. So the guard **aborts**
   when the line does not parse rather than skipping: a check that quietly
   stopped matching would be this same defect one layer up.
8. **`w6-alloc`'s `bytes_per_request` grows with the window and nobody knows
   why.** `./target/release/w6-alloc --repo . --requests N` reports 127,954
   bytes at N=200, 177,236 at 400 and 277,417 at 800 — total bytes growing
   faster than the request count — while `allocations_per_request` falls with N
   exactly as a per-request slope plus a per-`Machine` intercept must. Something
   on that path is superlinear in the number of connections in one script.
   Consequence: the **allocation count** is the sound half of that output and the
   **byte count** may only be compared at the window a baseline was taken at,
   which for every published figure is 200. Found by R3 while re-taking the
   figure; not diagnosed, and `docs/adr/0017-regions.md` §"What must be measured"
   ¶1 says so in place.

13. **A hole in what polices the compiled seam.** No corpus in the tree
    exercises the published-row gate, so it is green over space nothing reaches.
    Adding one means a definition whose *declared* row is narrower than what its
    body performs, which the type checker refuses to construct — so the corpus
    would have to be built against a deliberately wrong checker, and that is why
    it does not exist.

    Recorded because a gate nothing exercises and a gate that works look
    identical from a test summary. `crates/ply-eval/src/compiled.rs`
    §"What polices this seam, and what does not" is the inventory.

14. **`AssertionKind::RecursionLimit` classifies nothing.** Found 2026-08-24
    while splitting the frame ceiling's diagnostic out of
    `ply_eval::limit::err_recursion_limit`, whose doc asserted the opposite:
    *"The message keeps the phrase 'recursion limit' so that ADR 0004's
    `AssertionKind::RecursionLimit` still classifies it."* The variant is
    declared at `crates/ply-test/src/slice.rs:268` and mapped to
    `"recursion_limit"` at `:284`, and it is **constructed nowhere** —
    `grep -rn 'AssertionKind::' --include=*.rs` finds exactly one variant ever
    built, `Eq`, at `slice.rs:326`. This is the `E0435` pattern: declared,
    registered, raised nowhere. Consequence is small and worth knowing — a
    consumer reading `Assertion::kind` to tell a runaway recursion from a failed
    `assert_eq` cannot, and the four tests that do tell them apart
    (`ply-cli/tests/suite/failure_classification_audit.rs`, `ply-test-tests/tests/suite/hybrid.rs`,
    `ply-test/src/tests.rs`, `ply-eval/src/tests.rs`) all match the rendered
    string instead. `limit.rs`'s doc is corrected in place; the code gap is
    **not** fixed, because deciding whether the fix is to construct the variant
    or to delete it is a `ply-test` design call and this change was in
    `ply-eval`.

    **Held open by a test since 2026-08-27, and still not fixed.** The
    disposition call has not been made; what changed is that the gap can no
    longer quietly close by being forgotten. All six unbuilt variants — `Bool`,
    `Panic`, `Runtime`, `UnhandledEffect`, `RecursionLimit`, `Deadlock` — are
    rows in `UNARMED_VARIANTS` in `crates/ply-span-tests/tests/armed.rs`, and
    `no_allowlist_entry_has_outlived_its_reason` fails the moment one of them is
    constructed or removed, so this item cannot go stale in silence. The line
    numbers above have drifted with the file: `AssertionKind` is declared at
    `slice.rs:275` today and `Eq` is built at `:345`. Neither `slice.rs` nor
    anything else in `ply-test` was changed to add the check.
15. **Nothing populates a `CausalSlice`, so `--trace` reports nothing.** Found
    2026-08-24 while checking whether item 11's seam defect could reach a user
    through `ply-test`. It cannot, and neither can anything else: the causal
    slice is the fifth "declared, registered, raised nowhere" this file records;
    §"The shape it keeps taking" counts them.
    `ply_test::SliceBuilder` is constructed in exactly one place in the
    workspace — `crates/ply-test-tests/tests/suite/bisect_audit.rs`, four times, all tests —
    and `grep -rn 'Event::Perform' --include=*.rs` matches only `slice.rs`'s own
    `match` arm and its own unit test. `Attribution::slice` starts `None`
    (`ply-test/src/lib.rs:305`) and its only writer is
    `attribution.resolve(bisection, evidence.slice)` (`diagnose.rs:109`), whose
    `evidence.slice` is `failure.attribution.slice.clone()` (`lib.rs:1331`) —
    itself. Measured rather than read:

    ```
    $ ./target/debug/ply test tests/fixtures/assertion_failed.ply --trace always --json \
        | python3 -c 'import sys,json; d=json.load(sys.stdin); \
            print([ (f["key"], f.get("causal_slice")) for f in d["failures"] ])'
    [('assertion_failed.the running total of the journal', None)]
    ```

    The pipe is there because the field is `null` in a report of a few hundred
    lines and "I did not see it" is not the same reading as "it is null".

    Consequences worth knowing. `ply-cli/src/commands/test.rs:793`'s
    `if let Some(slice) = &failure.attribution.slice` branch — the `ran: a → b →
    c` line and the "the replay did not reproduce this failure" warning — is
    dead. `Tracing`, `--trace auto|always|never`, `SliceBuilder::DEFAULT_CAP`
    and the truncation story are all live code with no producer. And ADR 0004's
    `causal_slice.observed_footprint`, which §"What a failure report carries"
    presents as the answer to "which branch was taken, and which handler
    fired", is `null` in every report the shipped binary has ever written.
    Not fixed here: wiring the tracer is a `ply-test` design call — construct
    the slice, or delete the type and its `--trace` surface — and this change
    was in `ply-eval` and `ply-core`. **One thing that was fixed, because it is
    a claim rather than a gap:** `slice.rs`'s comment on `CausalSlice::observed`
    said the observed atoms are *"a subset of the declared footprint"*, and that
    is false the moment the tracer is armed — `ply_eval`'s `Trace` records every
    `perform` including one a `handle` discharges, so a definition with an empty
    published row contributes atoms to `observed` that are in no declared row at
    all. Measured on the fixture item 11's fix added: `handled` publishes `{}`
    and running it records `{tally.read[log], tally.write[log]}`. Corrected in
    place, with the withdrawn sentence beside it.

    **Held open by a test since 2026-08-27, and still not fixed.**
    `Event::Enter`, `Event::Return` and `Event::Perform` are rows in
    `UNARMED_VARIANTS` in `crates/ply-span-tests/tests/armed.rs`, reached by
    `every_variant_of_a_covered_enum_is_constructed_in_production`. The type
    with no producer is caught only through its variants: there is no general
    "public constructor called only from tests" rule, because for a library
    crate that has a large legitimate-API false-positive surface, so
    `SliceBuilder` and `CausalSlice` themselves are **not enforced**. Nothing in
    `ply-test` was changed.

### Closed items, kept only so a reference resolves

Comments in the source and notes in other documents cite these by number. One
line each; **git holds what they said**, and the fixes are in the commits that
made them.

1. `crates/ply-codegen-spike` did not compile. It builds now, and CI has a job for it.
2. `examples/same-tests.sh` did not build the binary it ran.
4. `README.md`'s `ply-corpus gen` invocation was missing the required `--out`.
5. `examples/serve.sh` misdescribed what `--db-schema` refuses.
6. `PLY_PG_URL` was set by nothing, so the live postgres tests passed without running. CI sets it.
7. No `LICENSE`/`LICENSE-APACHE` file existed although the manifest declared `MIT OR Apache-2.0`.
16. `spikes/ply-lexer/run.sh` reached no test because its harness did not compile past the tokens ADR 0028 and ADR 0033 added. It runs now, its lexer lexes them, and CI has a job for it (`lexer-spike`).
9. The compiled-entry seam carried one of the machine's two resource bounds, so a backend answered where the machine raises.
10. The two engines disagreed on the recursion bound for deeply pending bodies, with no backend involved.
11. A definition that discharged its own effects published an empty row, so the seam's purity gate cleared it.
12. Every entry into the spike's backend cost O(the *previous* entry's peak arena).
17. `spikes/ply-parser` was in no CI job and its differential was red.
18. `crates/ply-codegen-spike`'s agreement corpus was red and its suite green, because no test ran the command: the spike's boundary handed the leaf kinds the machine's seam admits to bodies compiled over `Int` and `Bool`. The boundary checks the kind now, and the suite runs the command. The shape to notice stands: a green suite over a harness whose purpose is a differential the suite never invokes is a green result over space nothing exercises.

## Style

**Rust.** `cargo fmt` defaults; `cargo clippy --all-targets` clean. Edition
2024, resolver 3.

**Test names are English sentences.** `renaming_a_definition_re_runs_nothing`,
`the_shipped_ladder_still_describes_the_tree_it_ships_in`,
`a_shipped_definition_the_project_never_touched_is_not_a_suspect`. This is not
decoration — grepping test names for a behaviour is the fastest index this
repository has, and it is the reason a stale test is visible.

**Assertion messages state the failure, not the expectation.** `"a rename
rebuilt something:\n{text}"` beats `assert_eq!`'s default. A failing test should
be readable by someone who has never seen the file.

**Comments explain a non-obvious *why*, and one is usually a sentence.** The
model is `crates/ply-hash/src/lib.rs`'s explanation of why the component loop
re-encodes once more after the partition settles: you cannot read that off the
loop, and a reader who does not have it will delete the extra round. Do not
restate the code or a type signature.

**A definition's history does not belong in its doc comment.** Which ADR moved
it, which milestone it shipped in, what a claim used to say and what a number
used to be are all git's, and the same reasoning applies here as in `CLAUDE.md`
§"Numbers, and the failure mode they cause here" — a correction block beside the
code is a diff written by hand, badly. Comments in this tree used to carry all
of it; they do not now, and the way that grew back last time was one careful
paragraph at a time.

**Doc comments in `crates/ply-cli/src/cli.rs`, `config.rs`, `trace.rs`, `db.rs`,
`style.rs` and `crates/ply-corpus/src/main.rs` are `--help` text**, not
commentary — `clap` renders them for the user. Edit them as user-facing prose,
and do not trim them to fit a style rule about comments.

**Dependencies.** Pin the latest stable version and write, in `Cargo.toml`, why
the crate is there and why not the obvious alternative. The existing comments on
`memchr`, `rust_decimal`, `rustls` and `tokio-postgres` are the standard. Adding
a dependency to `ply-host` grows the trusted computing base that
`ply hosts --host` invites a reader to audit; treat it as a decision.

**License.** MIT OR Apache-2.0. By contributing you agree your work is licensed
under both.
