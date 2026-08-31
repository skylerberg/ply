# ADR 0026 — A reachable backend, and the instrument that could not decide it

Status: accepted — **decided, not built**. Discharges `ROADMAP.md` §"What is
next" item 3 ("a decision about whether a backend is ever reachable from a
shipping command, which is M9 with an ADR"). Amends ADR 0016 §2.1, §2.4, §2.5,
§3.5 and §10.2, the last annotated in place. Amends ADR 0018 §0.5's list of what
it owes. Supersedes nothing.

> **Audit note, 2026-08-28, written by the change that carried this out.** The
> sentence below — *"nothing in this document ships a backend, and §6 is a list
> of obligations rather than a list of results"* — was true of the document and
> is no longer true of the tree. §6 items **1, 2, 3 and 6 are built**; item 4 was
> attempted, measured, and **refused on the measurement**; items 5, 7 and 8
> stand. What §4.7 forbids is still forbidden and was not done: `ply-cli` gains
> no dependency, `Cargo.lock` gains no cranelift, no workspace toolchain moves,
> and `crates/ply-codegen-spike` is neither promoted nor deleted.
>
> What ships is `ply_eval::backend::Reference` — a backend whose compiled code is
> a **second tree-walker** over the scalar-signature fragment §1.4 describes,
> installed by `ply test --backend`. It is not a code generator and it is slower
> than the machine; what it is, is the first implementor of `Compiled` on the
> shipping side of the seam, which is what §4.5 says has to exist before speed
> is worth arguing about: *"a backend must be policeable before it is fast."*
> Over `examples/` and `tests/fixtures/` it is offered 120,340 calls and enters
> **18,773** of them — §1.2's number, re-derived by a different route.
>
> > **`and it is slower than the machine` is withdrawn, 2026-08-30
> > ([ADR 0030](0030-compiled-code-on-the-front-end.md) §5).** It is slower on the
> > case this note's finding 3 measures — a **declined** body, re-run to
> > exhaustion once per offer — and faster on the case that decides whether entry
> > is worth anything. On the Ply front end it enters 190,618 times and does the
> > work in **0.0800 s against the machine's 0.2900 s**, taking the whole run from
> > 2.70 s to 2.48 s, **1.089×**. The rest of the sentence stands: it is not a
> > code generator, and §4.5's precondition is unchanged.
> >
> > §3's projection that a real front end cannot enter is withdrawn with it.
> > *"`fn read_line(buf: Bytes, ..) -> Line` **cannot cross**"* is still true of
> > `read_line` — it is refused on its **return** type, which is the finding ADR
> > 0030 §1 turns on. What does not follow, and what §3 drew from it, is that a
> > real front end's arguments are outside the fragment: a lexer's hot arguments
> > are offsets and bytes, and a byte in Ply is an `Int`, so even at the
> > pre-widening `Int | Bool` rung the front end admits 89,912 calls.
>
> Three findings from building it, each recorded where it bites:
>
> 1. **The third pair had a hole, and it was `(Err, Err)` wearing a third
>    engine's clothes.** Comparing the backed machine against the plain one only
>    where the two *engines* had already agreed on a **pass** makes a backend
>    that turns a red test **green** the one thing the comparison cannot see.
>    Found by `wrong:exceeds-budget=4` over a recursion past the machine's bound:
>    the guarded form reported nothing at all, the unguarded form reports the
>    verdict. §4.3's C4 is written against exactly this shape and it still nearly
>    happened.
> 2. **Seven of eight, not eight.** §4.5's second bullet predicted two would not
>    move unchanged and named the right two; one of them moved further than
>    predicted and one did not move at all. See the note on §4.7.
> 3. **The seam is O(n²) on a body it declines.** A `Reference` offer re-runs the
>    body to exhaustion, so a recursion that outruns the budget costs one full
>    attempt per level. One test over a 20,000-deep ladder, `/usr/bin/time -p`,
>    on a binary `.github/binary-is-current.sh` reported `current` first:
>    **26.45s real / 18.28s user** with `--backend reference` against **0.04s /
>    0.01s** with none, over 10,000 offers. Taken at a 1-minute load average of
>    9.2 — over this project's 4.0 gate — so an **observation and not a figure**;
>    the effect is three orders of magnitude and load does not reach it. This is
>    a property of the seam's `budget` contract rather than of this backend — a
>    cranelift fragment burns fuel per offer too — and nothing in §1.4's cost
>    accounting priced it.

**Read this line first: nothing in this document ships a backend, and §6 is a
list of obligations rather than a list of results.** This project's most
expensive defect class is a mechanism named everywhere a reader would look for
it and constructed nowhere (`CONTRIBUTING.md` §"The one rule", and the table
under §"The shape it keeps taking"), and an ADR that decided a backend was
reachable would be the largest possible instance of it. What is decided here is
the *question*: whether the answer is ever yes, on what evidence, and against
which instrument. What is built here is nothing.

## Context

`ROADMAP.md` §"What is next" item 3 carries an R5 audit note that states the
open question in one sentence:

> **What this item now owes is not another ratio. It is a decision about whether
> a backend is ever reachable from a shipping command, which is M9 with an ADR,
> and ADR 0016 §3.5 still requires the spike be deleted rather than promoted.**

Two facts make that question urgent rather than tidy.

**The first is that R5 measured 6.199× and no user of Ply can reach any of it.**
ADR 0018 §0.5 is blunt about it under a heading of its own — "Nothing here ships,
and that is the load-bearing sentence" — and `CONTRIBUTING.md` §"Things known to
be broken" item 13's first bullet carries the consequence: `ply test --engine
both` cannot install a backend, so the shipping CLI catches **zero** of the eight
deliberately wrong backends `crates/ply-codegen-spike/tests/mutations.rs` runs,
and the rule that a backend run must not populate the result cache is
**unenforced because it is unreachable**.

**The second is that the deferral is ordered by an instrument that cannot be
pointed at any workload but one.** `ply_corpus::w6` decides M9 from the
interpreter's share of a served HTTP request over TLS over postgres, and
`Ladder::missing()` refuses a ladder without all nine of its rungs — so a compute
kernel, or a lexer over a file, cannot be fed to it at all. §2 is that finding,
measured rather than argued, and §3.1 is careful about which recorded goal §4
then rests on, because the tree does not say what the framing around this ADR
says it does.

Everything below that is a number was re-derived on this tree on **2026-08-28**,
under a pre-registration written before any binary ran and kept outside the
repository at `/tmp/arc-compiled-path/PRE-REGISTERED.md`. Where a figure is
quoted from an earlier milestone rather than re-taken, it says so. The instrument
was checked before it was believed, per `CONTRIBUTING.md`:

```
$ .github/binary-is-current.sh target/release/ply target/release/ply-corpus
current  target/release/ply  (152 inputs checked)
current  target/release/ply-corpus  (175 inputs checked)          # exit 0
```

---

## 1. What is true today, checked rather than quoted

### 1.1 The seam is unreachable, and the record's inventory of it is stale in one place

```
$ grep -rn 'set_compiled\|Compiled' crates/ply-cli            # 0 hits, src and tests
$ grep -rniE 'jit|cranelift|codegen|backend' crates/ply-cli/src   # 0 hits
$ grep -c cranelift Cargo.lock                                     # 0
```

`EngineArg` (`crates/ply-cli/src/cli.rs:91`) has exactly three variants —
`Treewalk`, `Machine`, `Both` — and no flag anywhere in `ply --help` or `ply test
--help` installs a backend. Run rather than reasoned:

```
$ ./target/release/ply test examples --engine both --no-cache
0 failed, 186 passed, 0 cached (0.21s)
```

That is the tree-walker against the control-stack machine, and nothing else.
**Item 13's first bullet is confirmed.**

> **One figure in that bullet, and in `compiled.rs`'s copy of it, is wrong and is
> corrected in place.** Both read *"all five `set_compiled` call sites in the
> workspace are tests or the spike's own harness"* —
> `crates/ply-eval/src/compiled.rs:203` and `CONTRIBUTING.md` §"Things known to
> be broken" item 13. `grep -rn '\.set_compiled(' --include=*.rs` counts **42**
> across six files: `ply-eval/src/compiled.rs` 27, `ply-codegen-spike/tests/hazards.rs`
> 5, `ply-eval/tests/differential_corpus.rs` 3, **`ply-eval/tests/equivalence_audit.rs`
> 3**, `ply-codegen-spike/tests/mutations.rs` 2, `ply-codegen-spike/src/measure.rs`
> 2. CONTRIBUTING's own parenthetical list sums to 39 and calls it five, and it
> omits `equivalence_audit.rs` entirely.
>
> **The load-bearing half is unaffected and was re-checked one file at a time:
> every one of the 42 is a test or the spike's harness.** The count was decoration
> on a claim about *reachability*, which is why nobody noticed it was wrong for
> four days — which is itself the point of correcting it. A number carried beside
> a true sentence is still a number a reader will re-quote.

### 1.2 The seam is not inert on the shipping corpus

`crates/ply-eval/tests/differential_corpus.rs` attaches two hand-built
`Compiled` implementations — one that declines everything, one that answers by
tree-walking — to the machine over `examples/` and `tests/fixtures/`. Re-run
today:

```
$ cargo test -p ply-eval --test differential_corpus -- --nocapture
declining backend: 120576 calls offered over 1012 tests
answering backend: 18773 entered, 101567 declined, over 1012 tests
test result: ok. 6 passed; 0 failed
$ cargo test -p ply-eval --lib compiled::
test result: ok. 36 passed; 0 failed
```

**15.6% of offered calls clear all seven gates on real Ply source.** So the seam
is not a hypothetical surface waiting for a workload that never arrives: on the
repository's own corpus a backend that could answer anything `Int` or `Bool`
would be entered 18,773 times. A cranelift fragment would accept fewer, and the
number is an upper bound rather than a forecast — but it is not small, and it is
what `--engine both` would be auditing if it could.

The 36 and the 6 are exactly the counts `compiled.rs`'s own policing table
claims, so that table is current.

### 1.3 The spike builds, passes, and its published figures are stale

From `crates/ply-codegen-spike/`, `cargo +1.94.0 test --release --no-fail-fast`:
**49 passed, 0 failed, 3 ignored**, across eight test binaries plus the doc-tests
— `hazards.rs` 18, `mcts_kernel.rs` 9, `mutations.rs` 13, `spike.rs` 9,
`entry_cost.rs` 0 passed and 3 ignored on purpose ("a measurement, not a gate"),
and 0 in the lib and the two bins.

> **`CONTRIBUTING.md` §"Things known to be broken" item 1's block is stale on
> both of its figures, and the second is stale in the direction that matters.**
> It reads **"45 tests across 8 targets"** with `hazards` at 16 and `mutations`
> at 11, and it lists no `tests/entry_cost.rs` at all. It also reads that
> `cargo +1.94.0 clippy --all-targets` there "reports 13
> `not_unsafe_ptr_arg_deref` errors, all in `src/rt.rs`, which is the JIT's
> calling convention, plus 6 warnings". Re-taken today, whole output captured:
> **exit 0, zero errors, ten warnings** — five `arc_with_non_send_sync`, one
> unused import (`SLACK`, `tests/entry_cost.rs:54`), one `collapsible_match`,
> one `useless_conversion`, one `useless_vec`, one `type_complexity` —
> and `grep -c not_unsafe_ptr_arg_deref` over that log is **0**. The only
> suppression in `rt.rs` is `#![allow(clippy::missing_safety_doc)]`.
>
> The same 13-errors figure is quoted a second time, in
> `.github/workflows/ci.yml`'s `spike` job comment, as the reason that job builds
> and tests but does not lint. **That reason no longer holds**: the crate is
> clippy-error-clean and could be linted by the job that already builds it.
> `compiled.rs`'s policing table already says 13 for `mutations`, so two
> documents disagree about the same crate and `compiled.rs` is the one that is
> right — which is what item 1's own closing paragraph predicts happens when
> figures are not re-taken where they are published.

### 1.4 What the seam costs, and what it is

`crates/ply-eval/src/compiled.rs` is 2,063 lines — 578 of `Compiled`,
`crossable`, seven `Gate` variants and `admit`, and 1,485 of tests. `Machine::set_compiled`
is `machine.rs:606`, three counters hang off `Machine`, and one branch in
`compiled_answer` (`machine.rs:2044`) is reached on every interpreted call.
`pub use compiled::Compiled` is `lib.rs:54`. **No implementor exists in the
shipping workspace.** ADR 0018 §0.5 prices the branch at **0.0 allocations per
`/health` request** and **237.87 predictable branch tests**, and says in its own
voice that the wall clock of those tests was never taken.

One property of the seam decides how far §2's evidence reaches, and it is easy to
miss because it is stated as a safety rule rather than as a limit —
`compiled.rs`'s `crossable`:

```rust
pub(crate) fn crossable(value: &Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_))
}
```

Its doc comment calls this "a capability cut as much as a safety one: nothing
taking or returning a `List`, `Map`, `Record`, `Str` or `Float` can be entered at
all." **So the seam as it exists is a scalar seam.** That is the right cut for
the backend behind it — ADR 0019 §5 item 4 records that the fragment lowers
`a + b` as `Int` arithmetic whatever the operands are and fails at run time, so a
`Float` crossing would be a working program that starts raising at a call site
nobody opted into — and it is the reason the 6.199× is a fact about integer
compute and cannot be extrapolated to an HTTP request, independently of any
share.

### 1.5 The function ADR 0016's whole projection rests on cannot cross the seam

Read from both halves of the source rather than inferred, and it is the sharpest
thing this arc found that nobody had written down.

ADR 0016's `k` is measured on `std.http::read_line`, chosen by **ADR 0016 §3.1's**
rule as the request-path function with the highest per-request cost whose entire
body is inside the fragment. Its signature is `crates/ply-std/ply/http.ply:253`:

```ply
fn read_line(buf: Bytes, from: Int, budget: Int) -> Line = { ... }
```

`admit` refuses on the first argument, before any gate that has anything to do
with effects or budgets:

```rust
if !args.iter().all(crossable) {
    return Err(Gate::ArgumentShape);
}
```

`crossable` is `Int | Bool`. **`Bytes` is not crossable and neither is the
`Line` record it returns, so `read_line` is `Gate::ArgumentShape` on every call
the machine could ever offer.**

The 11.67× and the 11.68× are real numbers, and the spike's own source says which
path they are taken on. `measure::compare` times the spike side through
`Harness::compiled_call`, whose doc comment reads:

> A direct native call, **outside any machine**: ADR 0016's original path, and
> the only one that can report the fragment's own failure.

`Harness` holds a `hybrid` machine with `set_compiled` attached and it is not
what the `read_line` ratio is taken on. ADR 0016 was entitled to do that — the
spike existed to price a *ceiling*, and entry did not exist when §3 was written —
and `measure.rs` marks the boundary between the two regimes in a comment beside
the registration: *"Only the scalar-signature members are offered to the machine.
The rest are compiled and reachable from inside a native body … and would decline
on every call if registered."*

The consequence is not a defect in either document and it is load-bearing for
§4.3: **`E = 1.46×` is Amdahl projecting, onto the whole interpreter share, the
speedup of a function that the seam as built cannot enter.** It is not that the
number is unreachable pending a wiring change; the argument shape is refused by
the boundary's first line. So the served-HTTP arm of C2 is not "never measured"
— it is not measurable through this seam at all, and it would stay unmeasurable
after any amount of CLI work, until either `crossable` widens (which ADR 0019 §5
item 4 prices as a correctness hazard while the fragment lowers `a + b` as `Int`)
or the fragment learns the constructs ADR 0016 §9.2 says endpoints and codecs are
made of.

This is also the cleanest illustration of §2's whole point. The ladder's `k` and
the seam's reachability were built by different milestones against different
questions, and nothing in the tree compares them, because no instrument takes
both.

---

## 2. The instrument cannot answer the question it is being asked

### 2.1 Nine rungs, and a refusal

`ply_corpus::w6::Layer::ORDER` (`crates/ply-corpus/src/w6.rs:88`) is nine fixed
rungs — call, endpoint, framing, routing, machine, socket, tls, database,
tracing — and `Ladder::missing()` refuses a ladder that lacks any of them.
Demonstrated rather than read, by deleting one rung from a copy of the current
ladder kept in `/tmp` and never written into the tree:

```
$ ./target/release/ply-corpus w6 /tmp/.../hypothetical-no-database-rung.json benches/w6-spike-r4.json
M9: undecided — the measurement did not decide it
  - the ladder carries no `database` rung, so its total is not the stack's and
    no share can be read off it
```

**The M9 decision procedure structurally requires a served HTTP request with a
socket, a TLS record layer and a database round trip on the path.** A compute
kernel cannot be fed to it. Not "has not been" — cannot be, by the type.

That refusal is correct for what the ladder is: `Ladder::missing()` exists
because a share taken over a partial stack is a share of the wrong denominator,
which is a real defect it prevents. The defect is not in the refusal. It is in
reading the output of a nine-rung HTTP instrument as an answer about a language.

### 2.2 The verdict is decided at C3, and the share tests are never reached

Re-derived by running `ply-corpus w6` over both published pairs rather than by
quoting `ROADMAP.md`. `w6` renders and judges files and takes no measurement of
its own, so load is irrelevant to these numbers; both were confirmed
byte-identical on a second run.

| pair | S | band | k | E | ceiling | A | verdict |
| --- | ---: | --- | ---: | ---: | ---: | ---: | --- |
| `w6-ladder.json` + `w6-spike.json` | **0.3532** | 35.1–35.4% | 11.67 | **1.4770** | 1.5461 | 1.1487 | defer |
| `w6-ladder-r3.json` + `w6-spike-r4.json` | **0.3451** | 34.3–34.7% | 11.68 | **1.4610** | 1.5269 | 1.1533 | defer |

The current ladder reads 227.4µs of interpreter in a 658.9µs request, with a
−46.3µs residue charged back (attributed share 41.5%, conservative share 34.5%).
`ROADMAP.md`'s "the share is 35%, the projection 1.48x" is the **baseline** pair
rounded; the shipped ladder is 34.51% and 1.46×.

**`w6::decide` returns inside the `c3_gaps` block** — six of ADR 0016 §4's seven
levers unpriced — **before `share < defer_share`, before the band-straddle check,
before every share test there is.** The share and the projection appear in the
output only because the reasons are printed above the verdict and because
`reopens_at` is composed from them. Nothing about the interpreter's share has
decided anything in either take.

### 2.3 The share has already crossed the ladder's own categorical floor

`Criteria::defer_share` is **0.35**. The measured `S` is **0.345086**, and the
whole band 34.3–34.7% is below it, so it does not even straddle. What that means
is that if C3 were satisfied tomorrow, the ladder would not weigh anything — it
would take ADR 0016 §2.3's "report the ceiling and stop, not measure harder"
branch. Demonstrated by marking the six unpriced levers priced at 1.05 in a
`/tmp` copy:

```
$ ./target/release/ply-corpus w6 /tmp/.../hypothetical-all-levers-priced.json benches/w6-spike-r4.json
M9: keep deferring M9
  - 35% is below the 35% floor: even an infinitely fast backend is worth 1.53x
  M9 reopens when the interpreter share reaches 50% (it is 35%, a 1.53x ceiling),
  and the projection reaches 1.50x (it is 1.46x)
```

That branch is already live. The unpriced levers are the only thing standing in
front of it.

### 2.4 The ratchet is real, is stronger than `ROADMAP.md` states, and is not the criterion `ROADMAP.md` names

`ROADMAP.md:355` composes the reopen sentence out of the unmet criteria, and
§"What is next" states the problem with it in its own words:

> every one of them is stale in the same direction: **every cheaper lever that
> lands makes M9's case weaker**, and three have now landed where a code
> generator was predicted to be the answer.

The direction holds and is sharper than that. Between the two ladders, with no
regression recorded anywhere: `S` fell 0.3532 → 0.3451, `E` fell 1.4770 →
1.4610, the ceiling fell 1.5461 → 1.5269, and **the `k` a backend needs to reach
C2's `E ≥ 1.50` rose from 17.76× to 29.36×**. The bar moved away from the
backend while the backend stood still.

**But the mechanism is not the one `ROADMAP.md` names, and the difference
matters.** The interpreter did not get faster in absolute terms between the two
ladders: it went **209.3µs → 227.4µs, +8.6%**, while the whole request went
**592.6µs → 658.9µs, +11.2%**. The share fell because the *rest of the request*
— TLS, postgres, framing, the box — grew faster than the interpreter did.

So `S` is a ratio whose denominator contains a TLS record layer and a database
round trip, and it moves with facts that have nothing to do with Ply. A slower
disk under postgres lowers M9's case. A faster TLS library raises it. ADR 0017's
consequences block says the portable readings are the ratios and not the
microseconds, and it is right about the ladder's *rungs*; it does not follow that
a ratio between an interpreter and a network stack is a portable statement about
an interpreter.

**C2 is not the ratchet. C1 is.** At `S = 0.3451` a backend with `k ≥ 29.36×`
satisfies `E ≥ 1.50` with no regression anywhere — and the fragment already
measures **52.58×** where it runs, on the MCTS kernel. Run as a counterfactual
with every lever priced and the spike's times divided by 4.5 (`k = 52.55`,
`E = 1.51`):

```
M9: keep deferring M9
  - the spike compiled `std.http.read_line` and held 52.55x on its weakest input,
    which projects 1.51x end to end
  - 35% is below the 35% floor: even an infinitely fast backend is worth 1.53x
  M9 reopens when the interpreter share reaches 50%
```

**The reopen sentence collapses to a single clause, and it is a clause no amount
of backend work can satisfy.** C1 asks the interpreter to become half of a
request; the only thing that makes the interpreter half of a request is the
interpreter getting slower or the database getting faster. That is a criterion
satisfiable only by a regression or by somebody else's release notes, and a
criterion satisfiable only by a regression is not a criterion.

### 2.5 There are 1.2 points left before the criteria become unsatisfiable at `k = ∞`

`ceiling(S) = 1/(1−S) ≥ 1.50` requires `S ≥ 1/3 = 0.3333`. The measured `S` is
`0.345086`. **Below 33.33%, C2's `E ≥ 1.50` is arithmetically impossible at an
infinite `k`,** while the criteria still read as "computed rather than argued".
One more lever of the size of the constant memo — which moved `S` from 0.671 to
0.353 on its own — puts the ladder in a state where it prints a bar that no
backend in any universe can clear, and prints it in the same sentence and the
same tone as a bar that a good backend could.

### 2.6 What is *not* wrong with the ladder

Stated because §4 keeps most of it, and because a document that only listed the
defects would be dishonest about a measurement this project paid for twice.

- **The thresholds are right for the question the ladder asks.** 50% is the
  share at which an infinitely fast strategy is worth 2×, and 2× is a defensible
  price for a permanent second execution path. That reasoning is ADR 0016 §2.3
  and it survives.
- **C3 has never been a ratchet.** It compares *gains* against *gains*, and both
  sides are measured on the same workload, so the instrument's denominator
  cancels. It is the criterion that has fired every time, and it fired for a good
  reason each time: W1 predicted codegen and W2 delivered 4.8× by attacking an
  algorithm.
- **Keeping the verdict out of `Report` worked.** ADR 0016 §2.6's structural
  argument — a verdict that can be written into a file will eventually be written
  into a file — is why every number in §2.2 could be re-derived today by running
  a binary over a shipped file rather than by trusting a sentence. That property
  is what made this section possible and it must not be lost.

---

## 3. What an interpreter share can settle, and what it cannot

This section exists because §4.1 is the one decision below that does **not**
rest on a measurement in this tree, and a decision resting on a goal instead of a
number must say so in its own words rather than stand next to a number that
appears to support it. §4.2 through §4.7 do rest on measurements, all of them in
§1 and §2, and §3.1 ends by saying which parts survive if its basis is
rejected.

**What `S` settles.** On the workload `benches/w6-ladder-r3.json` was taken on —
`examples/desk.ply` served over a socket, TLS and postgres — the interpreter is
34.5% of a request, so no execution-strategy change is worth more than 1.53× end
to end on *that* workload, and the best-priced cheaper lever is worth 1.15×.
That is a fact, it is re-derivable by one command over a shipped file, and it is
a sufficient reason not to put a JIT in front of Ply's HTTP stack. **Nothing in
§4 disturbs it.**

**What `S` does not settle.** Whether Ply should have a compiled backend is a
question about what Ply is for, and §3.1 is careful about where that is written
down. A number that requires a `database` rung in its denominator cannot be
evaluated on a workload with no database, and §2.1 shows the instrument answering
`Undecided` rather than extrapolating — correctly.

On the one non-HTTP workload anyone has measured, ADR
0019 §5 puts **81.0% of executed work** inside the fragment and ADR 0018 §0.5
measures **6.199× [6.143, 6.226]** end to end with 2,162 native entries against
0.998× with zero. On those figures C1 (0.81 ≥ 0.50), C2 (`k` = 52.58 ≥ 3.0,
`E` = 4.87 ≥ 1.50) and C4 (0 disagreements over 2,396 generated cases,
re-confirmed below) would all pass and only C3 would fail. **There is no code
path in this tree that computes that**, because `w6` demands the nine rungs.

### 3.1 The basis for §4, stated so a reviewer can reject it directly

The framing this ADR was commissioned under describes Ply as "a compiled
language carrying the workloads Go, Java and Swift carry — not as low-level as
Rust, but in that class". **That sentence, or anything equivalent to it, is in no
file in this repository**, and this document is not entitled to decide a
milestone from it. Checked rather than assumed:

```
$ grep -rniE 'go, java|java and swift|swift|compiled language' \
      README.md DESIGN.md ROADMAP.md docs/adr/0021-why-bootstrap.md
(no output; exit 1)
```

`README.md` opens "**It is a research language**", and its bet is that "*generating
code is becoming free, and that what stays expensive is knowing whether it is
correct*". `DESIGN.md`'s thesis is the verification loop and its keystone is the
effect system. The only document in the tree that asks whether Ply should be fast
at compute is ADR 0018, whose status line is "**proposed. No decision here is
accepted.**"

**Deciding M9 on that framing would reproduce, one document later, the exact
defect ADR 0021 was written to fix.** That ADR's own opening:

> ADR 0020 answers *can Ply host its own front end today?* — no, and it measures
> why. It does not say why anyone wanted one. **That rationale existed only in a
> conversation**, which meant the next reader would find a rejection with no goal
> behind it. This is the goal.

So the target is **not** the basis. What is, and it is recorded, accepted and has
nothing to do with an HTTP request's interpreter share:

**ADR 0021 §4 item 3 puts compiled entry on the critical path of an ADR accepted
as a statement of intent.** Verbatim:

> 3. **The fragment, entered at token granularity.** The profile in ADR 0020 §6.3
>    shows dispatch dominating builtin bodies twenty to one — machine step 43.8%,
>    refcount traffic 26.5%, every builtin body together 1.3% — **so compilation
>    removes the right half.** What is unmeasured is the cost at one entry per
>    token rather than one per file.

That is a codegen item, on a critical path, in a document whose claim is that
Ply's verification loop is O(the change) while every toolchain it competes with
is O(the project) — and whose own numbers are 915.8 s against 0.070 s. **The
reason to want a backend that is written down in this tree is the bootstrap
track, not throughput on a served request**, and the ladder cannot see it at all:
a lexer over a file has no socket, no TLS and no database, so `w6` answers
`Undecided` for it by §2.1's mechanism.

Two things follow, and both are stated as limits rather than as support.

- **The 6.199× is corroboration and is not the reason.** One kernel, one program,
  one box, one pre-registered run whose own pre-registration forbade re-running
  it, through a seam that passes only `Int` and `Bool`. If it had come back at
  1.5×, §4.1 would read the same and §4.3's C2 would be what stopped it.
- **ADR 0021 §4 item 3's own unmeasured half is unmeasured still.** "The cost at
  one entry per token rather than one per file" is a question about *entry*, and
  the nearest thing in the tree to an answer is
  `crates/ply-codegen-spike/tests/entry_cost.rs` — three tests, `#[ignore]`d on
  purpose ("a measurement, not a gate"), which established that an entry once
  cost O(the previous entry's peak arena) — 0.375 µs after a 4-slot predecessor,
  68.083 µs after a 19,584-slot one, **181×** — and that `Ctx::end` fixed it,
  re-taken end to end at 1.499× / 1.202×. That is the failure mode that would
  make per-token entry unaffordable, it is closed, and **the evidence lives in
  three ignored tests inside the crate ADR 0016 §3.5 wants deleted.** §4.7 is
  written with that in front of it.

**If a reviewer rejects ADR 0021 §4 item 3 as a basis** — because the bootstrap
track is itself speculative, or because a token-granularity entry is not the same
thing as a backend a user reaches — then §4.1's answer weakens to "not decidable
here", and §4.2 through §4.7 stand unchanged, because each of those rests on a
measurement in §1 or §2 rather than on a goal. That is the intended failure mode
of this section: it is separable.

---

## 4. Decisions

### 4.1 Yes. A compiled backend is reachable from a shipping command

`ROADMAP.md` item 3's question is answered **yes**, on the basis §3.1 sets out
and no other: compiled entry is item 3 of ADR 0021 §4's critical path, that ADR
is accepted as a statement of intent, and the thing it is a critical path *to* —
a front end hosted in Ply, verified in time proportional to an edit — has no
socket, no TLS and no database on it, so the instrument that has been ordering
this deferral cannot see it and answers `Undecided` for it.

The question the record has actually been answering is a narrower one — *should
this HTTP service have a JIT* — and its answer is no, is well measured, and is
untouched by anything here (§3, first paragraph).

Three things this "yes" is not, said now because each is a way it will be
misread:

- **It is not "advance M9".** §4.4's verdict is defer, on C3 and C4.
- **It is not a claim that a backend is close.** §1.5 records a function whose
  compiled speedup the entire HTTP projection rests on and which the seam refuses
  on its first line.
- **It is not permission to promote the spike.** §4.7.

**What changes is the standing of the question.** M9 has been deferred by an
instrument that structurally could not consider the case for it; from here it is
deferred by two obligations that name work. What stands between here and a
reachable backend is §4.3's C3 and C4, and neither is a number that must rise.

### 4.2 The W6 ladder is withdrawn from the role of deciding M9

The ladder is **not** withdrawn as a measurement. It remains the best account
this project has of where a served request's time goes, it stays in
`benches/`, it stays re-derivable, and §3's first paragraph stands unamended.
What is withdrawn is its authority over M9, on the three grounds §2 measures:
it refuses every workload but one (§2.1), its share moves with the network
(§2.4), and its reopen sentence names a criterion that only a regression can
satisfy (§2.4) and is 1.2 points from naming one that nothing can satisfy (§2.5).

**A ratchet may not be left standing while a decision is taken around it**, so
this is a change to code and not only to a document. `w6::decide` must stop
returning a verdict about **M9** and start returning a verdict about what it
measured. Concretely, and this is the obligation §6 carries:

- `Verdict::label` and `Decision::reopens_at` name **"a code generator for the
  served HTTP workload"**, not M9. The strings change; the thresholds do not.
- `Decision` grows a field naming the workload its share was taken on, and the
  rendered sentence carries it, so that "35% of a request" cannot be re-read as
  "35% of Ply".
- ADR 0016 §10.2's reopen sentence is annotated in place with a pointer here.

After that change the ladder's answer is the same and is true: **do not put a JIT
in front of this HTTP stack; price the six unpriced levers instead.** It simply
stops claiming to have decided a question about the language.

### 4.3 The criteria that replace it, and each of them can fire

Written here before they are implemented, in ADR 0016 §2.1's order, and each is
stated so that a contributor can make it true by doing work rather than by
waiting for a number to drift.

The single structural change is that **a verdict names a workload class.** There
is no global answer to "should Ply have a backend"; there is an answer for
MCTS-shaped compute and a different answer for a served HTTP request, and ADR
0016's mistake was not its thresholds but its assumption that one workload could
stand for the language.

> **M9 advances for a workload class when all four hold, each taken on that
> class.**
>
> **C1 — Coverage, measured as work rather than as time.** The fragment accepts
> **≥ 50%** of the workload's *executed work*, counted in executed lowered nodes.
> Not the interpreter's share of a wall clock.
>
> *Why this is not the old C1.* A work share does not move when the interpreter
> gets faster at the same work, and does not move at all when TLS or postgres
> gets slower — which is what actually moved `S` between the two ladders (§2.4).
> It moves when a lever *deletes* work, which is honest: a backend genuinely has
> less to do. It is satisfiable by widening the fragment, which is backend work.
> Measured today: **81.0% on `benches/kernel`** (ADR 0019 §5), 2–5% on an HTTP
> request (ADR 0016).
>
> *And the threshold is inherited rather than re-derived, which is the weakest
> line in this section.* ADR 0016 §2.3 justified 50% by an argument about a
> **time** share — at 50% an infinitely fast strategy is worth 2× end to end, and
> 2× is the least that buys a permanent second execution path. A work share is a
> different quantity and that derivation does not transfer to it unchanged. **50%
> is therefore provisional**, and what would settle it is the measurement §7's
> first bullet asks for: both shares taken on one workload, and the difference
> between them read rather than assumed. Until that exists, C1 is a bar whose
> number is borrowed and whose *shape* — work, not time; named workload, not a
> fixed one — is what this ADR is actually changing.
>
> **C2 — A delivered speedup, not a projection.** **≥ 3.0×** end to end on that
> workload, **measured with a backend attached and native entries counted**, with
> a control arm at zero entries.
>
> *Why this is not the old C2.* `E = 1/((1−S) + S/k)` is Amdahl over a share, and
> ADR 0016 §10.3 lists three measured reasons to doubt its own 1.48× — a 1.02×
> direct measurement of the same function's end-to-end value, a fragment
> reaching 141 of 366 functions, and a coverage cliff that takes 11.67× to 1.71×
> the moment two callees stay interpreted. A projection that its own author
> calls "an upper bound with three measured reasons to doubt it" may not clear a
> gate. R5's 6.199× against a 0.998× control **is** such a number; ADR 0016's
> 1.48× is not, and ADR 0018's withdrawn 4.86× ceiling is the standing proof
> that a projection through this seam can be wrong in either direction.
> Measured today: **6.199× [6.143, 6.226]** on `benches/kernel`; on HTTP, never
> taken, and **not takeable through the present seam at all** — the function the
> HTTP projection is built from takes `Bytes` and returns a record, and `admit`
> refuses it on its first line (§1.5).
>
> **C3 — Nothing cheaper. Kept, with one word changed.** ADR 0016 §2.1 reads
> "Every alternative in **§4** is **priced**, and `(E − 1) ≥ 2 × (A − 1)`"; here
> the list is the **workload's**, not §4's, because §4's seven levers are levers
> on a served request and half of them (connection reuse, response buffering,
> framing) do not exist on a compute kernel. Everything else — the pricing
> method, the margin, the gains-not-ratios reading, and §2.5's rule that a single
> unpriced lever defers on its own — is unchanged.
>
> *Why this one is kept as it was.* It is the only one of the four that
> was never a ratchet: both sides are gains on the same workload, so the
> instrument's denominator cancels, and it can be satisfied by measuring rather
> than by waiting. It has also been right every time. ADR 0016 §2.5's
> independence clause is kept with it: an unpriced lever defers on its own.
>
> **C4 — Correctness, with measured sensitivity, through a shipping command.**
> Agreement on every input, **and** the corpus that produced the agreement must
> have been seen to fail, **and** the eight wrong backends must be caught by a
> command a user can run.
>
> *Why this one is strengthened.* "0 disagreements" is the exact shape of result
> `CONTRIBUTING.md` §"The one rule" names as this project's most expensive
> defect class, and `wrong.rs`'s own module header says so first. C4 as ADR 0016
> wrote it is satisfied by a corpus that compares nothing. The third clause is
> new and is §4.5.

### 4.4 Where those criteria stand today, on both workloads

| | compute kernel (`benches/kernel`) | served HTTP (`examples/desk.ply`) |
| --- | --- | --- |
| **C1 — coverage ≥ 50% of executed work** | **pass** — 81.0%, 34 of 34 functions, 745 of 745 nodes | **fail** — 2–5% |
| **C2 — measured ≥ 3.0× with entries counted** | **pass** — 6.199× [6.143, 6.226], 2,162 entries against a 0.998× / 0-entry control | **not measurable** — `read_line` takes `Bytes`, so `admit` refuses it on its first line (§1.5) |
| **C3 — nothing cheaper, all priced** | **fail** — `sqrt`/`ln` as prelude builtins is **≈2.5× on the whole kernel** (ADR 0019 §5 item 5), inferred by Amdahl over three fields of one file and **not priced end to end**; ADR 0018 §4's `Map`/record/list machinery is 19.0% of executed work and outside the fragment whatever compiles | **fail** — 6 of 7 levers unpriced |
| **C4 — agreement, sensitivity, and a shipping oracle** | **fail on the third clause** — sensitivity is measured (below) and the shipping CLI catches zero of eight | **fail** — same |

**Verdict: defer, for both classes, on C3 and C4.** And the difference from every
previous deferral is the shape of what is owed: C3 is a measurement somebody can
take next week, and C4 is the subject of §4.5. Neither asks a ratio to move on
its own, and neither can be satisfied by a regression.

**A third workload class is named and not judged, deliberately: the bootstrap
front end.** §3.1 makes it the recorded basis for §4.1, and no row is offered for
it here, because ADR 0021 §4 item 3's own sentence — "what is unmeasured is the
cost at one entry per token rather than one per file" — means C1 and C2 have
never been taken on it. Writing a row for it would be the vacuous green this
project keeps producing. **Taking that measurement is the highest-value item in
§6 that this ADR does not itself require**, because it is the one workload where
a positive answer would order M9 against something already accepted rather than
against a preference.

The sensitivity C4's second clause demands does exist, at corpus scale, and was
re-taken today rather than quoted:

```
$ ./target/release/mcts --dir ../../benches/kernel --only agreement
2,396 cases + 24 whole-kernel searches, 0 disagreements,
49,489 native entries, 22 distinct compiled functions entered,
34 of 34 functions and 745 of 745 nodes inside the fragment

$ ./target/release/mcts --dir ../../benches/kernel --mutate off-by-one
mutation offered 152,548 calls, changed 104,892 answers,
1,649 DISAGREEMENTs, noticed by 25 subjects; exits non-zero
```

**1,649 against `ROADMAP.md`'s recorded 1,635**, because the fragment widened to
34 of 34 functions after that table was taken. The corpus has measured
sensitivity, which is the thing C4's second clause is for.

One blind spot in it is real, is the spike's own finding, and is carried forward
into §7 rather than smoothed over: **a whole-kernel search is a weak oracle.** 20
of 24 searches notice a corrupted `nth_move`, and every compiled function except
a search's entry points is offered **zero** times during a search, because the
hook sees nothing under an entered root. The per-function generated cases are
what cover them, and a future harness that ran only whole-kernel searches would
report the same green over a much smaller explored space.

### 4.5 The contract for reachability: a backend must be policeable before it is fast

This is the clause that answers "under what contract", and it inverts the order
the record has been assuming.

> **Built, 2026-08-28, and the clause is met on its first half and met with a
> named exception on its second.** `ply test --backend <spec>` attaches one, on
> `--engine machine` as well as on `--engine both`, and under `--engine both` it
> is a **third** engine compared against the plain machine rather than against
> the tree-walker — so a divergence reported is the backend's and nothing else's,
> which is the attribution item 1 below is paying for. It catches **seven** of
> the eight configurations. The eighth is the unbounded runaway and it escapes;
> the note on §4.7 has the reason and the measurement, and "seven" is written
> here rather than "the eight" because the difference is the whole of what this
> clause is for.
>
> > **Read one word narrower, 2026-08-30: "seven of the eight" is a property of
> > `Reference`, not of the seam, and this clause is a condition on *any*
> > backend.** Traced through the types rather than argued. `ply test`'s only
> > install route is `InterpExecutor::with_backend`
> > (`crates/ply-test/src/lib.rs:895`), whose signature is
> > `(&'static ply_eval::Fragment, BackendSpec)` — a `Fragment`, not a
> > `dyn Compiled`. It reaches a backend only through `Fragment::attach`
> > (`crates/ply-eval/src/backend.rs:257`), which returns `Reference`, or
> > `Mutant` wrapping `Rc<Reference>` concretely. So **the eight corruptions
> > police one implementation of `Compiled` and there is no route by which a
> > second one is offered to them.**
> >
> > That is not a naming quibble: two of the eight need operations the
> > `Compiled` trait does not have. `Unoffered` asks the registry
> > (`Fragment::holds`) whether a body exists, which is the distinction between
> > "declined" and "never had one"; `ExceedsBudget` re-runs the body with fuel
> > that is *not* the machine's budget (`Reference::run`). `Compiled` is
> > `describes` and `enter` and nothing else (`compiled.rs:379`), so a cranelift
> > backend arriving tomorrow is policed by **none** of the eight until the
> > mutations are lifted off `Reference` onto a trait that carries those two
> > operations, and `with_backend` stops naming a concrete type.
> >
> > `crates/ply-cli/tests/backend.rs`'s 14 green tests are therefore evidence
> > about `Reference` and are read, in the sentence above and in
> > `CONTRIBUTING.md` item 13, as evidence about the shipping path's ability to
> > catch a wrong backend. **The gap is exactly the shape §"The one rule" names**
> > — a green result over space nothing exercises — and it is recorded here
> > rather than fixed because lifting the mutations is a change to production
> > source with its own review, and because the clause it qualifies is the
> > gate M9 has to pass rather than one this ADR passed.

**No backend may ship until `ply test --engine both` can attach one and catch the
eight.** Not because policing is more valuable than speed, but because it is
*upstream* of it: a backend whose wrong answers no shipping command can detect is
not a backend that can be shipped at any ratio, and every argument about `k`
presumes a correctness story that does not currently exist outside a crate ADR
0016 §3.5 requires be deletable.

Three things follow, and each is a checkable obligation rather than a sentiment.

1. **`--engine both` becomes three pairs, and ADR 0016 §2.2 priced that
   correctly as a permanent cost.** What §2.2 could not price, because the number
   did not exist, is what it buys: over `examples/` and `tests/fixtures/`, 1,012
   tests offer the seam **120,576** calls and **18,773** clear every gate
   (§1.2). A third pair over that corpus is a real oracle on real source, not a
   ceremony.

2. **The eight wrong backends must be catchable from the workspace, without
   cranelift.** `Mutation` (`crates/ply-codegen-spike/src/wrong.rs`) has seven
   wrong variants plus `None` as the control, and eight *configurations* are
   tested because `ExceedsBudget` is exercised bounded (`=4`) and unbounded.
   Every one of them is a wrapper over `Compiled::enter`, and
   `differential_corpus.rs` already holds two `Compiled` implementations and a
   1,012-test corpus in the shipping workspace. **The mutations do not need a
   code generator; they need something that answers, and `backends::TreeWalker`
   answers 18,773 times over that corpus.** (`backends::Declining` is the other
   implementation and is the control: it answers nothing, so it can host no
   corruption — which is exactly why a harness must assert the offer count before
   it asserts the catch.) Moving them is the prerequisite in §4.7, and it is what
   makes C4's second clause reproducible by `cargo test --workspace` instead of
   by a crate on a different toolchain.

   Two of the eight will not move unchanged, and saying which is the point of
   naming them:
   - `answers=…` **cannot be produced by a backend at all.** `Gate::PublishedRow`
     and `Gate::InternalEffects` mean the mutant is never asked, and what stands
     is an offer count of zero. Pricing that gate requires deleting a machine
     line, which produces `observed footprint — left {tally.read, tally.write},
     right {}` — and `tests/fixtures/self_handled_effect.ply` already does this
     on the workspace corpus.
   - `exceeds-budget` unbounded is a native stack overflow, caught only from
     **outside** the process (`run_guarded` / `Ended::as_disagreement`). Any
     workspace harness that runs it must run it as a child, or it will report the
     most catastrophic failure available as its quietest.

3. **`wrong-type` is not caught where a reader expects.** Read from the file and
   confirmed by the run: `compiled_refusals` stays **0**, because `Bool` and
   `Int` both cross. It is caught downstream, on the value axis and by a type
   error in the caller. A future harness that watched `compiled_refusals()` for
   it would watch a counter that never moves.

### 4.6 The result-cache rule is armed before a backend exists, not after

`Machine::set_compiled`'s doc comment states the rule and, unusually for this
project, states its own unenforceability in the same breath:

> A run with a backend attached is a third execution strategy, and a cached
> `Pass` is a claim about the authoritative engine. That rule is **not enforced
> for a backend** — it is unreachable, because `cache_bypassed` at
> `crates/ply-cli/src/commands/test.rs:335` takes a `&TestArgs` with no `Machine`
> in scope and no shipping command can install one. The day a flag can, that line
> moves in the same change.

Verified by reading every candidate: `EngineChoice::bypasses_cache`
(`ply-eval/src/lib.rs:172`) reads a three-variant enum and knows nothing about a
backend; `cache_bypassed` takes `&TestArgs`, which has no field that could carry
a `Machine`; the write itself is `store.put(*key, Outcome::Pass)` in
`ply-test/src/lib.rs`; and `Machine::compiled_counts()` — the fact that would
answer the question — has **no caller outside `ply-eval`'s own tests and the
spike's harness**. **The rule is unenforced twice over: the flag that would set
it does not exist, and the fact that would detect it is never read.**

Note the interlock, because it is a trap: `--engine both` already implies no
cache, so a backend installed on the `--engine both` path would be cache-safe
**by accident**, while a backend on the default `--engine machine` path would
not. Enforcement may not be that accident.

**Decision: the rule is armed in two stages, and the first stage lands before any
backend does.**

- **Stage one — the tripwire, buildable today with no backend in existence.** A
  test in the `crates/ply-span/tests/armed.rs` tradition — a check over
  production source rather than over behaviour — named
  `a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache`. It
  fails when `set_compiled` acquires a caller in production source (excluding
  `crates/*/tests/`, `benches/` and `#[cfg(test)]` items, exactly as `armed.rs`
  defines production) unless `cache_bypassed`'s inputs have grown a way to see
  it. Its non-vacuity is demonstrable the way `armed.rs`'s is: add a call site
  and watch it go red. **This converts a rule stated on a doc comment into a
  test that fires on the exact change that would break it**, which is item 13's
  whole complaint about it.

- **Stage two — the diagnostic, owed by M9 and specified here so that M9 cannot
  choose a weaker shape.** The precedent is `cache_escapes`
  (`ply-cli/src/commands/test.rs:303`), which is the same class of rule for host
  binding: it walks the finished `RunReport`, finds results written to the cache
  whose test could reach the host, and turns each into an `INTERNAL_ERROR` that
  says "this is Ply's fault — the runner and the binding disagree about what this
  test can do". The backend rule is that shape one field over. `TestResult`
  carries the native entry count from the `Machine` that ran it; a written `Pass`
  with a non-zero count is the diagnostic. Named
  `backend_escapes`, beside its precedent.

  The cost is stated rather than waved at: it plumbs `compiled_counts()` out of
  `ply_test::Worker` (`crates/ply-test/src/lib.rs:867`, `machine_lowering` —
  which is also the exact place a backend would be installed) into `TestResult`,
  which is a signature change across `ply-test` and its report and JSON schema.
  It is not one line, and the one-line version — adding a clause to
  `cache_bypassed` — is stage one's *flag* half and covers only a backend that
  arrives by that flag.

  **Stage two is the version that survives a backend arriving by any route**, and
  the reason to specify it now is that stage one alone would let M9 ship the
  cheap version and call the rule enforced.

ADR 0016 §2.2's own objection lands squarely on the flag half and is not answered
here: `RUNTIME_VERSION` (`ply-store/src/lib.rs:90`, `"0.12.0"`) keys
`(RUNTIME_VERSION, DefHash) -> Outcome`, so **an opt-in JIT is an opt-in cache**,
and ADR 0011 §4 already refused that shape for `--host` on the grounds that
`ply check` must not disagree with itself. This ADR does not resolve it, and M9
may not treat it as resolved. It is listed in §5.

### 4.7 ADR 0016 §3.5 — one clause honoured, one amended, with both quoted

§3.5's last bullet reads:

> It may **not** be kept because it works. It is thrown away whatever the
> verdict; an `Advance` schedules M9, and M9 is a milestone with an ADR, not a
> promotion of a spike.

**That clause is honoured without qualification and this ADR promotes nothing.**
`ply-cli` gains no dependency, `Cargo.lock` gains no cranelift, no workspace
toolchain moves, and §4.3's criteria are not written to let the spike through
them. The spike is 6,909 lines of source built to price a ceiling, its `rt.rs` is
a JIT calling convention, and it has bit-rotted twice while nothing noticed. It
is not a shipping component and the path to one does not run through it.

§3.5's *other* clause is the deletion requirement, whose stated reason is:

> so that deferring M9 deletes one feature block and one dependency line, and
> nothing else in the workspace knows it existed.

> **That reason is already false, and R5 is what made it false — corrected in
> place in §3.5 itself, 2026-08-22.** After R5, `crates/ply-eval` carries
> `compiled.rs`, a public `Compiled` trait, `Machine::set_compiled`, three
> counters and a branch on every interpreted call, **all of which survive
> `rm -r crates/ply-codegen-spike`**. So the deletion no longer buys what §3.5
> says it buys. Performing it today removes the only implementation of `Compiled`
> in existence and leaves the declaration standing — which makes the
> declared-nowhere-constructed shape *worse*, not better. `crates/ply-span/tests/armed.rs`
> covers registered diagnostic codes and the variants of enums under
> `crates/ply-test/src` plus `Severity`; **it does not reach traits and does not
> reach `ply-eval`**, so nothing in the tree would say so.

**Decision: the deletion requirement is amended, narrowly, and made conditional
on something checkable instead of on a milestone boundary.**

> **Amended (this ADR).** `crates/ply-codegen-spike` is deleted when — and only
> when — the seam's **measured sensitivity** exists inside the shipping
> workspace: the eight wrong backends of `tests/mutations.rs`, reproduced over
> the `Compiled` doubles in `crates/ply-eval/tests/`, running under
> `cargo test --workspace`, with a corpus that has been seen to fail. Until
> then the spike is the only thing in this repository that has ever demonstrated
> that a wrong backend would be noticed, and deleting it would leave the seam
> policed by 36 unit tests over doubles and a claim.
>
> Everything §3.5 says about *promotion* is unchanged and is honoured above.
> Nothing about this amendment permits the spike to be depended on, shipped,
> linted into the workspace gate, or read as a backend Ply has.

Two obligations attach to that condition so it cannot quietly become permanent —
which is the failure mode of the deletion requirement it replaces, recorded
undone in ADR 0016 §11 and again in `ROADMAP.md` §"What is next":

- `.github/ci-shards.sh`'s `KNOWN_OUTSIDE` entry read *"its own workspace on
  purpose, per ADR 0016 3.5, so that deferring M9 deletes it with rm -r"*. That
  reason is the one R5 falsified. **It now carries the condition above** — done
  in this change, not scheduled — so that the file whose whole purpose is to
  make an unbuilt crate a deliberate choice rather than an accident carries the
  *real* choice. `.github/workflows/ci.yml`'s `spike` job carries it too, beside
  the note that it goes in the same change.
- The condition is a §6 obligation with a named test, so that "the spike is still
  here" and "the sensitivity is still not in the workspace" are the same
  sentence, checkable in one place.

> **The condition was tested on 2026-08-28 and came back NOT satisfied. The
> spike stays, and §7's fifth bullet is what happened.** That bullet read: *"The
> mutation harness may not survive the move … If the moved harness turns out to
> have less sensitivity than 1,649 disagreements, §4.7's deletion condition will
> have been satisfied on paper by something weaker than what it replaced. The
> mitigation is that the condition names measured sensitivity and not a test
> count."* It was right, and the mitigation worked.
>
> Seven of the eight configurations moved, and on the axes that moved they are
> **stronger** than what they replace: the corpus is 1,116 real tests rather than
> 2,396 generated cases, and it is `cargo test --workspace` rather than a crate
> on another toolchain. One run, 2026-08-28, printed by the tests that take it:
>
> | configuration | where it now runs | tests reporting it | answers changed |
> | --- | --- | ---: | ---: |
> | `off-by-one` | `ply-eval/tests/differential_corpus.rs` | 146 of 1,116 | 9,451 |
> | `inverted` | same | 51 | 216 |
> | `stale` | same | 259 | 501 |
> | `wrong-type` | same | 515 | 460 |
> | `unoffered` | same | 901 | 487 |
> | `answers=` | same — **an offer count of 0, which is the gate** | 0 | 0 |
> | `exceeds-budget=4` | `ply-cli/tests/backend.rs`, on a corpus that outruns the bound | 1 | 1 |
> | `exceeds-budget` unbounded, terminating body | same | 1 | 1 |
> | **`exceeds-budget` unbounded, non-terminating body** | **nowhere** | — | — |
>
> **The last row is why the condition fails, and the reason is structural rather
> than an unfinished afternoon.** §4.5's second bullet said this one "is a native
> stack overflow, caught only from **outside** the process". That is true of the
> *spike's* backend, which is native code on a fixed stack: it dies by signal 6
> in seconds and `run_guarded` / `Ended::as_disagreement` report the corpse.
> `Reference` is a tree-walker whose frames grow on the heap through `stacker`,
> so the same corruption does not crash — it **hangs**, measured at no output and
> no exit in 45 seconds against 0.03s for the run that reports. A harness can
> run it as a child; what it cannot do is tell a hang from work, and a wall clock
> is not a disagreement.
>
> So `crates/ply-codegen-spike` is still the only thing in this repository that
> has demonstrated that a backend which ignores its budget entirely would be
> noticed, which is the sentence this amendment turns on. What would discharge
> the condition is a reporter outside the run **and** a backend whose runaway
> actually dies — `run_guarded`'s shape moved into the workspace, over something
> with a bounded stack. Recorded in `.github/ci-shards.sh`'s `KNOWN_OUTSIDE`
> entry, `.github/workflows/ci.yml`'s `spike` job and `CONTRIBUTING.md`
> §"Things known to be broken" item 1, so that "the spike is still here" and
> "the eighth mutant is still not reproduced" stay the same sentence.

**This is the third document to touch this obligation, and that is a reason for
suspicion rather than for confidence.** The difference claimed here, and it
should be held to it: ADR 0016 §11 and `ROADMAP.md` both recorded a deletion that
*ought* to happen and named no condition under which it would; this one names a
condition, and the condition is a test somebody writes.

### 4.8 The seam stays

Stated as a decision because "delete the spike" is routinely read as implying it,
and R5's correction to §3.5 shows the two are different acts.

`compiled.rs` stays: the trait, `crossable`, the seven gates, `admit`,
`set_compiled`, the three counters, and the `enter_code` branch. Three reasons,
in order of weight.

1. **It is the answer to an architectural question, and the answer was
   measured.** ADR 0018 §0 said "make the interpreter able to enter compiled
   code, or the ceiling is 5.26× however much of the fragment you accept". Entry
   turned 0.998× into 6.199×, and §0.5 withdrew its own ceiling as an artifact of
   a body-only attribution. That finding is the most valuable thing R5 produced
   and the seam is its standing form.
2. **Its contract cost four review rounds to get right and every round is
   recorded in it.** The frame bound that let a backend answer where the machine
   raised; `Gate::InternalEffects` and the transitivity it needs, which a
   one-hop bit does not give; the arena reset that cost O(the previous entry's
   peak); `crossable`'s refusal of `Float` in front of a fragment that lowers
   `a + b` as `Int`. Deleting the seam deletes the 36 tests that hold those, and
   the next backend rediscovers them by review or does not.
3. **It costs 0.0 allocations per request**, measured against a binary with the
   call site deleted, at three window sizes, with the arms alternated.

And the honest cost of keeping it, which §7 turns into a way this can be wrong:
**the wall clock of the 237.87 branch tests per `/health` request has never been
taken**, on either binary, although both existed. ADR 0018 §0.5 says so about
itself. 0.0 allocations is not 0 cost, and this ADR does not take that
measurement either.

---

## 5. What this ADR does not decide

- **Which backend.** Cranelift, LLVM, a bytecode VM with a template JIT, or
  ahead-of-time compilation are all still open. §4.3's criteria are written
  against a `Compiled` implementation and say nothing about how one is produced.
- **The cache-key objection.** ADR 0016 §2.2's "an opt-in JIT is an opt-in cache"
  and ADR 0011 §4's refusal of that shape for `--host` are unanswered here.
  §4.6's stage two enforces that a backend run does not *write* the cache; it
  does not answer what a cache key should be for a world where a backend is the
  normal path.
- **Determinism.** ADR 0016 §2.2 names it as the sharpest cost — a backend that
  reassociates arithmetic or reorders argument evaluation breaks seeded replay
  and `proved` **silently**. The present seam evades it by being off inside a
  `simulate` region and by handing back at most one scalar. A backend that
  matters will not evade it, and nothing here says how it is checked.
- **The toolchain.** cranelift 0.134.3 needs rustc ≥ 1.94.0; the repository pins
  no toolchain and CI runs `fmt`/`clippy`/`test` on 1.93.1 with the `spike` job
  alone on 1.94.0. Because §4.7 promotes nothing, no workspace toolchain decision
  is forced by this ADR. One is forced by M9.

  > **The last sentence is withdrawn, 2026-08-30, on a measurement.** It read:
  > *"One is forced by M9."* **The 1.94.0 floor is a property of the version
  > `crates/ply-codegen-spike/Cargo.toml` pins, not of cranelift.** Read off the
  > crates.io index, `rust-version` per release: `0.132.0`–`0.132.3` declare
  > **1.93.0**, and 1.94.0 first appears at `0.133.0`. All five crates the spike
  > uses — `cranelift-jit`, `-codegen`, `-frontend`, `-module`, `-native` — agree
  > on that boundary.
  >
  > Run rather than read, on this machine's default `stable` (**rustc 1.93.1**,
  > the version CI pins in six jobs), with `cranelift-jit = "=0.132.3"`: a probe
  > compiles the body of `lexer.is_digit` — the hottest definition the widened
  > seam admits on the Ply front end — through `JITBuilder`, `FunctionBuilder`,
  > `define_function` and `get_finalized_function`, and **calls the native code**.
  > `is_digit(47,48,53,57,58,97) = [0,1,1,1,0,0]`. The check was seen to fail
  > before it was believed: flipping one expected answer reports `left: [0, 1, 1,
  > 1, 0, 0] / right: [0, 1, 1, 1, 1, 0]`, so it reads the JIT's output rather
  > than its own constant. Whole cranelift stack built clean on 1.93.1 in 23.86s
  > on aarch64-apple-darwin.
  >
  > So M9 forces a version *choice*, not a toolchain *move*: `0.132.x` keeps the
  > 1.93.1 pin and `0.133+` moves it. What M9 still forces is the other half of
  > this ADR's cost, and that half is unchanged and was re-measured the same day
  > — an optional, default-off cranelift dependency puts **31 packages** into the
  > shipping `Cargo.lock` and takes `grep -c cranelift Cargo.lock` from **0 to
  > 44**, with the feature **off** and the crate excluded from
  > `workspace.members`. A lockfile entry is not conditional on a feature.
  >
  > Two boundaries on this. The spike's own source does **not** compile against
  > `0.132.3`: `cargo check` reports **11 errors, all in `src/jit.rs`** —
  > `ir::MemFlagsData` (renamed `MemFlags`), eight `iadd_imm_s`/`icmp_imm_s`
  > call sites (the `_s` suffix is a 0.133 addition), and two signature changes
  > around `stack_load`/`stack_addr`. They are naming and arity drift rather than
  > a missing capability, but **they were not ported**, so nothing here says a
  > twelfth error is not behind the eleventh. And this bullet moves the
  > *toolchain* line only; §4.7's refusal to promote the spike is untouched.
- **Whether 6.199× holds anywhere else.** One kernel, one program, one box, one
  pre-registered run whose pre-registration forbade re-running it, through a seam
  that passes only `Int` and `Bool`. ADR 0018 §0.5 lists this among what a reader
  still does not know and this ADR does not move it.

---

## 6. What must be built, and the test that says it happened

In dependency order. Each line names the artifact that makes it checkable,
because a decision recorded without one is what §4.7's amendment is trying not to
repeat.

1. **Stop the ladder claiming M9, in code.** `w6::Verdict::label` and
   `Decision::reopens_at` name the served HTTP workload; `Decision` carries the
   workload its share was taken on. *Checked by* an existing test's expectations
   moving — `an_unpriced_alternative_defers_whatever_the_share_says` renders the
   sentence — plus one asserting the rendered verdict names a workload. **The
   prose half is done**: ADR 0016 §10.2's reopen sentence is annotated in place
   in this change, with the 29.36× and the counterfactual beside it, so a reader
   who never reaches this file still sees that the sentence no longer decides
   M9.
2. **The cache tripwire**, §4.6 stage one:
   `a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache`.
   *Non-vacuity demonstrated by* adding a production `set_compiled` call site and
   recording that it goes red, in the file's header, the way `armed.rs` records
   its own.
3. **The eight mutants, in the workspace**, over the `Compiled` doubles in
   `crates/ply-eval/tests/`, with the offer/answer counts asserted before the
   catch is asserted — the three-step shape `mutations.rs` already uses, whose
   middle step is the one usually missing. *Checked by* the same self-test that
   file applies to itself: replacing every `Mutation` with `Mutation::None` must
   fail all but the control and the gate test.
4. **The spike is deleted** the day 3 lands, per §4.7, and
   `.github/workflows/ci.yml`'s `spike` job with it.
   `.github/ci-shards.sh`'s `KNOWN_OUTSIDE` reason already carries that
   condition, rewritten in this change rather than left for the change that
   discharges it.
5. **C3 on the compute kernel.** Price ADR 0019 §5 item 5's `sqrt` and `ln` as
   prelude builtins end to end on `benches/kernel` — ≈2.5× is inferred by Amdahl
   over three fields of one file and is not a measurement — and re-take ADR 0018
   §4's 19.0% `Map`/record/list share on a hybrid run rather than on the pre-R5
   attribution it currently rests on.
6. **C4's third clause**, which is M9's own first task and not a prerequisite to
   it: `--engine both` attaching a backend, `backend_escapes` beside
   `cache_escapes`, and the eight caught by a command a user can run.
7. **C1 and C2 on the bootstrap front end**, which is the measurement ADR 0021
   §4 item 3 says is missing and the one workload where a positive answer would
   order M9 against something already accepted. It needs no CLI work and no
   backend the seam can enter today: the executed-work share of `spikes/ply-lexer`
   inside the fragment is a census, and the entry-granularity cost is the
   question `entry_cost.rs` was built next to.
8. **If the compiled-workload target is real, write it down.** ADR 0021 exists
   because a rationale that lives only in a conversation leaves the next reader
   with a rejection and no goal behind it, and §3.1 shows this ADR was nearly
   the same failure one document later. An ADR-0021-shaped statement of what Ply
   is for at run time — or a `README.md` that keeps saying "research language" —
   is what settles whether §4.1 has a second basis or only the one.

**Seven edits are done rather than owed**, in place and with the withdrawn text
quoted, per `CONTRIBUTING.md` §"Correct, do not delete". The first two are this
ADR taking effect where a reader would meet the old answer; the other five are
stale figures, none load-bearing and each one a number a later reader would have
re-quoted:

- **ADR 0016 §10.2's reopen sentence** is annotated in place — see item 1 above.
- **`ROADMAP.md` §"What is next" item 3 is discharged** where it asks for this
  decision, and its "Two smaller obligations are open" paragraph now carries
  §4.7's condition in place of the deletion it recorded as owed.

- `compiled.rs`'s policing table cited `CONTRIBUTING.md` item 9 as the gate on
  wiring a backend into the CLI. **Item 9 is closed** — "Fixed 2026-08-24,
  together with item 10 — they were one defect" — and the only gate that block
  named which is still open is the result-cache rule. It now says so and points
  at §4.6.
- The same table, and `CONTRIBUTING.md` item 13's copy of it, said **five**
  `set_compiled` call sites over a list summing to 39 and omitting
  `equivalence_audit.rs`. Both now say 42, over the six files that hold them,
  with the withdrawn list quoted (§1.1).
- `CONTRIBUTING.md` item 1's spike block said "45 tests across 8 targets" and
  "13 `not_unsafe_ptr_arg_deref` errors … plus 6 warnings". Re-taken and
  corrected to 49 passed / 3 ignored over eight binaries plus doc-tests, and 0
  errors / 10 warnings (§1.3).
- `.github/workflows/ci.yml`'s `spike` job gave the same 13 errors as its reason
  for not linting. That reason is withdrawn in place; the crate is
  clippy-error-clean and the job could lint it once the ten warnings are dealt
  with.
- `.github/ci-shards.sh`'s `KNOWN_OUTSIDE` entry gave its reason as "so that
  deferring M9 deletes it with rm -r", which R5 falsified. It now carries §4.7's
  condition, so that the file whose purpose is to make an unbuilt crate a
  deliberate choice records the choice that was actually made.

---

## 7. What would make this wrong

In order of how hard it would be to see.

- **The replacement C1 is a work share, and nobody has taken one on an HTTP
  request.** The 81.0% for the kernel comes from ADR 0019 §5 and is a body-only
  accounting — the same accounting ADR 0018 §0.5 showed produces a *wrong
  ceiling*, because it charges the call-site machinery to an unattributed bucket
  that entry also deletes. If a work share is systematically wrong in the same
  direction, C1 is a bar that passes things it should refuse, and it is the
  criterion this ADR moved. **The check is to compute both shares on the same
  workload and see whether they disagree by more than the residue.** This ADR did
  not do it.
- **§4.1 rests on one item of one ADR, and that ADR decides no
  implementation.** ADR 0021 is "accepted as a statement of intent. It decides no
  implementation", and §4 item 3's compiled-entry claim inherits that standing.
  If the bootstrap track is abandoned, or if a token-granularity entry turns out
  to be a different thing from a backend a user reaches — the seam admits whole
  definitions and refuses anything but `Int` and `Bool`, and a lexer's inner loop
  passes `Bytes` — then §4.1's basis is gone and the answer reverts to "not
  decidable here". **§3.1 is written to be separable for exactly this reason**,
  and a reviewer who rejects it should find §4.2 through §4.7 still standing.
- **The framing this ADR was commissioned under is not in the tree, and it would
  have carried the decision if §3.1 had not checked.** A first draft of this
  document opened §4.1 with "a language whose stated target is the work Go, Java
  and Swift carry", and no file in this repository says that. If a later revision
  quietly restores it — or if a `README.md` acquires it without an ADR-0021-shaped
  document behind it — that revision is the defect, and §6 item 8 is what should
  have happened instead.
- **A ratio is not a product decision.** 6.199× on an MCTS kernel, against Rust
  at "roughly an order of magnitude" (`ROADMAP.md` §"Compute kernels"), may leave
  Ply in the same place relative to the language a user would otherwise pick.
  §4.3 has no criterion for that at all, and adding one is not obviously
  possible.
- **The mutation harness may not survive the move.** §4.5 asserts the eight are
  wrappers over `Compiled::enter` and so need no code generator. Two of them
  already need special handling (an effectful definition is never offered; an
  unbounded budget kills the process), and a third — `stale` — needs a corpus
  that *varies arguments*, which the 1,012-test workspace corpus does by running
  real programs rather than by generating cases. If the moved harness turns out
  to have less sensitivity than 1,649 disagreements, §4.7's deletion condition
  will have been satisfied on paper by something weaker than what it replaced.
  The mitigation is that the condition names measured sensitivity and not a test
  count.
- **The whole-kernel oracle is weak and the replacement C4 may inherit that.** 20
  of 24 searches notice a corrupted `nth_move`; every compiled function except a
  search's entry points is offered zero times during a search. A future harness
  built on whole-program runs alone would report the same green over a much
  smaller space. C4's second clause is written against that and does not prevent
  it.
- **Keeping the seam may cost something nobody has measured.** 237.87 branch
  tests per `/health` request, wall clock never taken, on either of two binaries
  that both existed. If that number is not free, §4.8's third reason is wrong and
  the seam is a permanent tax on the engine that ships for the benefit of one
  that does not.
- **And the ordinary way**: if §6's list is still unbuilt in six months, this ADR
  will have been a fourth document about an obligation instead of the end of one,
  and §4.7's amendment will have been the mechanism. The condition was chosen to
  be checkable for exactly that reason, and a reader who finds the spike still
  present and item 3 of §6 still unwritten should treat this document as the
  defect.

---

## Not in this ADR

The backend itself; the choice of code generator; monomorphization, unboxed
mutable arrays, evidence passing and the rest of ADR 0018 §2–§7, none of which
has an end-to-end price on a workload that can enter compiled code; the cache key
a compiled path needs; and the determinism check a reassociating backend would
require. Each is a milestone, and this one decides only which question they are
ordered by.
