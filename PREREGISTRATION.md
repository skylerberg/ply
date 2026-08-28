# Pre-registration — W4 record update syntax

Written **before** anything was built, measured or run in this worktree
(`/Users/skylerberg/.worktrees/ply/w4/record-update`, checkout `d88aae5`).
At the time of writing there is **no `target/` directory** — nothing is
compiled, so no number below can have been retrofitted to a reading.

House rule 4 applies to every number here; house rule 5 applies to every
assertion. Nothing in this file may be edited after the corresponding
observation is taken. Corrections go **below** the original text, quoting it,
per CONTRIBUTING.md §"Correct, do not delete".

---

## 0. What is being claimed

The feature: a record **update** expression, spelled `{..base, f: e, ...}`,
that produces the base record with the named fields replaced.

The hard constraint: **it must be the same definition as its expansion.**
Concretely — `DefHash` equality between the sugared spelling and one specific
longhand spelling, defined in §2.

---

## 1. Predictions recorded before observation (binary, not statistical)

Each is a yes/no observation. Each is recorded here with the prediction; the
reading goes beside it afterwards, whatever it says.

### P1 — the brief's "silently wrong limit" claim is **false**

The brief states: *"ADDING A FIELD TO `Limits` FORCES AN EDIT THERE, AND
FORGETTING IT IS A SILENTLY WRONG LIMIT RATHER THAN A TYPE ERROR."*

I predict this is **wrong**, and that forgetting it is a **type error**.
Basis, read off the source rather than guessed:
`crates/ply-core/src/unify.rs:306-309` unifies two records with
`if f1.len() != f2.len() || f1.keys().ne(f2.keys()) { return Err(mismatch()) }`
— exact key-set equality, no width subtyping — and `Limits` is a structural
record alias (`crates/ply-std/ply/http.ply:74`), so a 13-field literal passed
to `fields(buf, _, _, limits: Limits)` (`http.ply:558`) where `Limits` has 14
fields cannot unify.

**Experiment, fixed now.** Add one field `max_probe: Int` to `type Limits`
(`http.ply:74`) and to `default_limits()` **only** — not to `chunk_trailers`.
Run `target/release/ply check crates/ply-std/ply/http.ply` once (this is a
compile outcome, not a timing: N=1 is the whole population).
- **Prediction:** exactly one `E0201`-class type error (`TYPE_MISMATCH`)
  pointing at the `chunk_trailers` literal or the `fields(...)` call.
- **If instead it checks clean**, the brief is right and I am wrong, and I will
  say so in those words and re-derive the hazard.
- Either way the field is then reverted; the revert is verified by
  `git diff --stat` reported by the top-level agent, never by me (house rule 1:
  I run no git command — I will restore the file by editing it back and
  re-running `ply check`).

**Restated hazard, which stands whichever way P1 reads.** All 13 `Limits`
fields are `Int`, so a **wrong pairing** (`max_body: state.limits.max_chunk_size`)
type-checks and is silently wrong. That is the defect record update removes
structurally, because the 12 unchanged fields stop being written at all.

### P2 — the rewrite of `chunk_trailers` **moves** its `DefHash`

Under the expansion order fixed in §2, the rewritten `chunk_trailers` is not
byte-identical to the longhand at `http.ply:1017-1029` (that longhand writes
`max_header_bytes` second; the canonical expansion writes it last). So its
`DefHash` moves once and its tests re-run once.
- **Prediction:** `chunk_trailers`' `DefHash` changes; `default_limits`,
  `parse_head`, `body_step` and every other `http` definition that does not
  reference `chunk_trailers` keep theirs.
- This is **not** a violation of the hard constraint. The constraint is
  "sugar ≡ its canonical expansion", not "sugar ≡ any expansion". If I find I
  cannot state that distinction honestly, that is a STOP-AND-REPORT.

### P3 — no `FRONTEND_VERSION` or `RUNTIME_VERSION` bump is required

Because expansion happens inside `ply_syntax::parse_module` and the node is
gone before `ply-hash` sees anything, `crates/ply-hash/src/normalize.rs` emits
no new tag and no existing definition's bytes change.
- **Prediction:** every existing `DefHash` in `examples/` and
  `crates/ply-std/ply/` is unchanged by this branch, apart from
  `chunk_trailers` (P2) and its dependents.
- **Falsifier:** `crates/ply-hash/tests/audit.rs` and
  `crates/ply-cli/tests/incremental.rs` go red, or a `W0603
  CACHE_VERSION_CHANGED` appears.
- **If any other hash moves, I bump nothing and report.** House rule 9 says
  bump when semantics change; it does not license a bump to hide a hash move.

---

## 2. The canonical expansion, fixed before any test is written

`{..b, f1: e1, ..., fk: ek}` where `b`'s record shape is `S` expands to a plain
`ExprKind::Record` whose fields are, **in this order**:

1. every field of `S` **not** named among `f1..fk`, **sorted by field name**,
   each with value `b.<name>`; then
2. `f1: e1, ..., fk: ek`, **in the order written**.

Two properties are being bought, and both are checkable:

- **Sorted, not declaration order.** `crates/ply-hash/tests/audit.rs:656`
  (`reordering_the_fields_of_a_record_type_is_free`) is an invariant the suite
  asserts: reordering the fields of a record *type* changes no hash, because
  `normalize.rs:591-612` sorts `TY_RECORD`. If the expansion used the type's
  declaration order, reordering `type Limits` would move `chunk_trailers`'
  hash and break that test. Sorting by name is what keeps it.
- **Written fields last.** `spikes/ply-lexer/GAPS.md` §1 measures that a
  growing sub-expression (`push`) in any but the **last** position of its
  enclosing node is quadratic — 4x per doubling against 2x, four points, and
  the mechanism is read off `ply_eval::rc::carry` (`rc.rs:98`). Copies are
  pure field reads and never grow; putting them first is therefore free, and
  it makes `{..s, toks: push(s.toks, t)}` land on GAPS §1's *linear* spelling.

> **This deviates from the brief's illustrative expansion.** The brief writes
> the expansion of `{...s, a: 1}` as `{a: 1, b: s.b, c: s.c}` — updated field
> **first**. That order puts a growing field in GAPS §1's quadratic position.
> The deviation is deliberate and is recorded here rather than discovered
> later.

---

## 3. The central deliverable, and how it will be broken on purpose

**Test:** `crates/ply-hash/tests/audit.rs`, a new
`record_update_hashes_as_its_expansion`, using the existing `unchanged(...)`
helper that the file's other invariance tests use.

```
fn f(s: {a: Int, b: Int, c: Int}) -> {a: Int, b: Int, c: Int} = {..s, a: 1}
```
against
```
fn f(s: {a: Int, b: Int, c: Int}) -> {a: Int, b: Int, c: Int} = {b: s.b, c: s.c, a: 1}
```

- **Pass condition:** `hashes(sugar).defs["f"] == hashes(longhand).defs["f"]`.
- **Companion, so the test is not vacuous in the other direction:** the sugar
  must **not** equal `{a: 1, b: s.b, c: s.c}` (a different field order is a
  different definition — `crates/ply-hash/src/tests.rs:929`
  `swapping_two_record_fields_changes_the_hash` is the invariant that says so).
  Asserted with `assert_ne!`.

**Deliberate break, run before the test is credited** (house rule 5 — a green
result over unexplored space is this project's signature defect):

1. Change the expander's sort from `sort()` to `sort().reverse()`. The equality
   assertion must go **red**. Restore.
2. Change the expander to emit written fields **first**. The equality assertion
   must go **red**. Restore.
3. Make the expander drop one copied field. The equality assertion must go
   **red** *and* `ply check` on the fixture must report a type error. Restore.

All three transcripts are reported. A break that does not go red means the test
is not testing what it says, and the feature is not shipped until it does.

---

## 4. Test counts — the statistic, its runs and its decision rule

The reported figure is a **count of passing and failing tests**, not a time, so
the run count is 1 per command: a test count is deterministic and a second run
of the same binary on the same tree is not a second sample.

Commands, fixed now, reported with exact `test result:` lines and with the
per-target breakdown rather than a total only:

```
cargo test -p ply-syntax
cargo test -p ply-core
cargo test -p ply-hash
cargo test -p ply-eval
cargo test -p ply-cli --test stdlib
cargo test -p ply-cli --test w3_http_audit
cargo test -p ply-cli --test http_endpoint
cargo test -p ply-cli --test routing_audit
cargo test -p ply-cli --test incremental
cargo test -p ply-cli --test modules_hash_audit
cargo fmt --all --check
cargo clippy --workspace --all-targets
```

**Decision rule:** the change ships only if every command above is green and
`clippy` reports **zero** warnings. `cargo test --workspace` is **not** run
(house rule 10: 9.5–29 minutes).

**Gates that pass without running, and are therefore not evidence** (house rule
11, CONTRIBUTING §"Things known to be broken" item 6): `PLY_PG_URL` and
`PLY_TEST_DB` are unset locally, so ten postgres tests and 26 pool tests report
green in 0.00s without executing. None of them reach this change; I will say
so rather than counting them.

`crates/ply-codegen-spike` declares its own `[workspace]` and is not reached by
`--workspace`. This change does not touch `crates/ply-eval/src/code.rs`'s
`NodeKind` or `Stmt`, so the spike is not expected to break; if `code.rs`
changes at all, `cd crates/ply-codegen-spike && cargo +1.94.0 test --release`
is run and reported.

## 4b. Instrument check, before any binary is used to observe anything

House rule 6. Before every observation taken with `target/release/ply`:

```
find crates \( -name '*.rs' -o -name '*.ply' \) -newer target/release/ply
```

must print **nothing**.

> **`-o -name '*.ply'` was added by adversarial review, and the `.rs`-only form
> is what let the wrong `examples/` figure above through.** This block read:
>
> > ```
> > find crates -name '*.rs' -newer target/release/ply
> > ```
>
> `crates/ply-std/src/lib.rs` `include_str!`s every `crates/ply-std/ply/*.ply`
> **into the binary**, so editing `http.ply` without rebuilding leaves an
> instrument that disagrees with the tree — and the `.rs`-only check reports
> **fresh**, because the stale input is a `.ply`. This was reproduced during the
> review: with `http.ply` newer than the binary, the old command printed nothing
> while the extended one printed `crates/ply-std/ply/http.ply`, and the hash
> readings taken in between were wrong. The command and its (empty) output are reported beside
the observation. If it prints anything, the observation is discarded and the
binary rebuilt — this is the one case where a reading is dropped, and it is
dropped because the instrument was wrong, not because the number was.

---

## 5. No timing measurement is planned

Nothing in this workstream is a performance claim. If one becomes necessary —
for instance if the `http` suite's wall clock visibly moves after the
`chunk_trailers` rewrite — it is pre-registered **here, first**, with:
statistic = **minimum of N runs** (N=5 under 2s, N=3 otherwise), both user CPU
and wall clock reported, and `uptime` recorded before and after the series. No
run discarded after the fact.

The `chunk_trailers` rewrite is expected to be performance-**neutral** by
construction: the expansion emits the same 12 field reads the longhand emits,
in a different order, and all 12 are pure reads.

---

## 6. The STOP-AND-REPORT conditions, fixed now

Any one of these ends the workstream with a report rather than a shipped
change. They are written down now so that none of them can be renegotiated
once the code is half-built.

- **S1.** The hash-equality test in §3 cannot be made to pass without changing
  `crates/ply-hash/src/normalize.rs`'s byte stream. (Changing it is a
  cache-format change — CONTRIBUTING §"Where a change is likely to bite" —
  and would move every cached result everywhere.)
- **S2.** The expansion cannot be made a function of a single module's own
  bytes. Gate 1 (ADR 0002) skips a file on its content hash; ADR 0013 §1.4
  pins effect-set expansion inside `parse_module` for exactly this reason. If
  record-update expansion needs a type declared in *another* module in order
  to be correct, then either gate 1 becomes unsound or the feature must refuse
  cross-module bases — and if it can do neither, it stops here.
- **S3.** Any `DefHash` outside `chunk_trailers` and its dependents moves.
- **S4.** The deliberate breaks in §3 do not go red.
- **S5.** The expander's shape resolution and `ply-core`'s inference can
  disagree in a way that is **not** caught as a diagnostic — i.e. a wrong
  expansion that type-checks. This is the green-over-unexplored-space failure
  and it is the one I am most afraid of; §7 is the guard.

**No `FRONTEND_VERSION` bump is used to paper over any of S1–S5.**

---

## 7. The guard against S5, stated as a checkable claim

The expander computes a field set `S` for the base from syntax alone. If `S`
is wrong, the emitted literal is wrong. The claim to be checked is:

> **A wrong `S` is always a diagnostic, never a wrong answer.**

Two halves, and only the first is free:

- **`S` too wide** — the expansion emits `b.x` for an `x` the base lacks.
  `crates/ply-core/src/infer.rs:3171-3196` reports `UNKNOWN_NAME` "no field
  `x` on this record". Enforced by inference already.
- **`S` too narrow** — the expansion emits a record of the wrong width.
  `unify.rs:306` rejects it wherever the result meets a known record type.
  **This is not total**: a `{..s}` whose result is never constrained would go
  unnoticed. So the expander additionally records, on the expanded node, the
  base expression and the field set it assumed, and inference asserts the
  base's inferred `Type::Record` key set equals `S ∪ {written fields}`,
  reporting a new diagnostic when it does not.

If that inference-side assertion is not implemented, this document says so in
those words, and the feature ships marked **"not enforced"** for the narrow
case or does not ship. It does not ship claiming a guarantee it has not armed
(CONTRIBUTING §"Do not state a guarantee you have not armed").

---

## 8. New diagnostic codes claimed now

`crates/ply-span/src/lib.rs` needs both the `pub const` and the registry row
(CONTRIBUTING §"Adding a diagnostic code"), and each needs one fixture:

- `E0116 RECORD_UPDATE_SHAPE` — the base's record shape is not resolvable from
  this module.
- `E0117 RECORD_UPDATE_FIELD` — a named field is not a field of the base.

If §7's inference-side assertion is built it needs a third; it is **not**
claimed here, because a code that exists and is raised nowhere is the shape
CONTRIBUTING §"declared, registered, raised nowhere" is about.

---

# Observations

Appended after the fact, never edited into the sections above. Each reading sits
against the prediction it was written for, whatever it says.

## P1 — **I was right and the brief was wrong**, and the tax is four times larger than either of us said

Experiment exactly as written in §1: `max_probe: Int` added to `type Limits` and
to `default_limits()` only, then
`target/release/ply check crates/ply-std/ply/http.ply` (instrument check run
first, `find crates -name '*.rs' -newer target/release/ply` printed nothing).

**Reading: `compilation failed (4 errors)`, all `E0201 TYPE_MISMATCH`.** So
forgetting a site is a **type error**, not a silently wrong limit, and the
brief's *"FORGETTING IT IS A SILENTLY WRONG LIMIT RATHER THAN A TYPE ERROR"* is
withdrawn. The mechanism is the one predicted: `unify.rs` compares record key
sets exactly, and the diagnostic prints both 14-field and 13-field key sets.

The prediction said "exactly one" error and there were **four**. The three I did
not know about are `limits_with` (`:1666`), `limits_keeping` (`:2399`) and
`limits_streaming` (`:2844`) — three more sites that spell `Limits` out. The
*tax* §1 was measuring is four times what the brief recorded; the *hazard* is
smaller. Reverted by editing the file back; the revert was verified
byte-identical against a copy taken before the edit, and `ply check` returned to
`checked 2 modules, 150 definitions, 56 tests`.

The restated hazard stands and is now armed rather than asserted: a **mispairing**
(`max_body: state.limits.max_chunk_size`) type-checks, and
`crates/ply-cli/tests/stdlib.rs chunk_trailers_copies_every_limit_it_does_not_replace`
goes red on exactly that while `ply check` stays green — shown, not argued (BREAK 6).

## P2 and P3 — confirmed, and measured over the whole tree rather than asserted

`chunk_trailers`' `DefHash` moved: `39eb48587ef7` → `7f15a7e6db49`.

Measured with `ply hash --json --deps` over `crates/ply-std/ply/http.ply` before
and after the rewrite, compared programmatically rather than by eye:

- **206 entries, 40 moved, 166 unchanged.**
- Every moved entry has `http.chunk_trailers` in its closure — "moved but not a
  dependent: none".
- Every dependent moved — "dependent but did not move: none".
- `examples/`: **1,428 entries, 84 moved — every one of them inside
  `chunk_trailers`' dependency cone.** `crates/ply-std/ply/` as a whole:
  1,209 entries, nothing moved outside `http`.

> **Corrected by adversarial review. This line read:**
>
> > - `examples/`: **1,428 entries, 0 moved.** `crates/ply-std/ply/` as a whole:
> >   1,209 entries, the same 40, all in `http`.
>
> The `0` was an artefact of a **stale binary**, and the prediction it was
> reporting on is unaffected: the moved set still equals the dependent set
> exactly, so S3 still did not fire. `crates/ply-std/src/lib.rs` `include_str!`s
> every `crates/ply-std/ply/*.ply` into the binary, so `import std.http` resolves
> to the compiled-in copy and never to the file on disk; re-running `ply hash`
> after editing `http.ply` **without rebuilding `ply-cli`** compares a binary
> against itself and reports `0 moved`. §4b's instrument check cannot catch this
> — `find crates -name '*.rs' -newer target/release/ply` prints nothing, because
> the stale input is a `.ply`, not a `.rs`. **§4b should read `-name '*.rs' -o
> -name '*.ply'`.**
>
> Re-taken with a binary verified fresh against both extensions, on corpora copied
> out of each checkout with `.ply-cache` excluded, twice, identical both times:
> 1,428 entries, **84 moved**; moved-set == dependent-set of
> `std.http.chunk_trailers`; "moved but not a dependent: none"; "dependent that
> did not move: none". 19 of the 84 are `desk.*` — `examples/desk.ply` imports
> `std.http`, so it is a transitive dependent and its moving is the intended
> behaviour, not a violation.

**S3 did not fire.** No `FRONTEND_VERSION` or `RUNTIME_VERSION` bump was made or
needed (P3 confirmed).

## §3 — the three deliberate breaks, plus three more the work asked for

All six were run, seen red, and restored, and the restoration was re-run green
each time.

| # | break | what went red |
| --- | --- | --- |
| 1 | copy sort reversed (`.reverse()`) | `record_update_hashes_as_its_expansion` — "hash changed", two `DefHash`es printed |
| 2 | written fields emitted **first** | same test, same assertion |
| 3 | expander drops one copied field (`copies.pop()`) | same test **and** `ply check` on the fixture: `E0201 expected {a: Int, b: Int, c: Int}, found {a: Int, b: Int}` |
| 4 | expander leaves the node unexpanded | `no_record_update_survives_parse_module_anywhere_in_the_tree` — "an unexpanded record update escaped `parse_recovering`" |
| 5 | expander copies nothing (`copies.clear()`) | both engines, `a_record_update_agrees_with_its_longhand_on_both_engines`: `expected {a: 1, b: 20, c: 3}, found {b: 20}` |
| 6 | `chunk_trailers` reverted to the longhand **with a mispaired limit** | `chunk_trailers_copies_every_limit_it_does_not_replace` — and `ply check` stayed **green**, which is the hazard |

**One thing break 3 taught that was not pre-registered.** The first run of
`ply check` after break 3 reported the file clean. That was not a hole: gate 1
(ADR 0002) had skipped the file on its unchanged content hash and served the
cached result, because the *compiler's* front end had changed and its version had
not. `rm -rf .ply-cache` produced the predicted `E0201`. This does not affect the
shipped change — a file that adopts the syntax has new bytes — but it means every
CLI observation in this workstream was taken against a cleared cache, and a
future workstream that changes expansion in `parse_module` without moving
`FRONTEND_VERSION` should know that gate 1 will not notice.

## §5 — no timing measurement was taken, and none was needed

Nothing here is a performance claim, and no `uptime` series was run because no
number was reported. The `chunk_trailers` rewrite emits the same twelve field
reads the longhand emitted, in a different order, and all twelve are pure.

## §7 — what is armed, and what is marked "not enforced"

Armed: `W ⊆ S` (`E0117`); refusal on every uncertainty (`E0116`), including the
shadowing case, which has its own test in three positions
(`a_shadowing_binder_refuses_rather_than_using_the_outer_type`); `S` too wide is
`E0101` from inference; `S` too narrow is `E0201` wherever the result meets a
known record type, shown by `a_narrower_result_annotation_is_a_mismatch`.

**Not enforced**, in those words: there is **no per-node assertion** that the
base's inferred `Type::Record` key set equals `S ∪ W`. §7 offered to build one by
recording the assumed field set on the expanded node; that was not built, because
recording anything on the node is exactly what the hash constraint forbids and a
side table on `Module` is a larger change than the hole justifies. The residual
case is a `{..s}` whose result no annotation ever constrains. What stands in for
the assertion is an argument with a test behind it, not a check: `S` is computed
from the *same written annotation* inference uses to type the binder, so there is
no independent field set to disagree with, and the residual risk is a bug in this
pass rather than a program a user can write. `crates/ply-core/tests/record_update.rs`
is what exercises it.

## Unregistered but reported: how much the module-local restriction excludes

The planner named "if the count of record literals the restriction excludes is
most of them, this is the wrong change as scoped" as a stop-shaped risk, so it is
counted rather than asserted. Not a pre-registered statistic — it is a **heuristic
census**, and it is labelled one because the method is a regex and not the parser.

Method: every `.ply` file in the tree; within each brace-delimited literal, count
fields written `name: base.name` (a self-named copy); a literal with two or more
is a record-update candidate; the base's type is taken from the nearest preceding
`base: T` annotation and checked against the `type T` declarations in that file.

Reading: **48 candidate sites across 14 files; 39 have the base's type declared
in the same file.** Of the nine that do not, seven are the regex failing on a
nested path (`head.request`, `lo.value`) rather than a real cross-module base;
`examples/desk.ply`'s `h.request` against `std.http`'s `Request` is the clearest
genuine exclusion. The restriction costs roughly one site in ten. The risk did
not fire.
