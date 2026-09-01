# Dead surfaces: what is there, what was promised, and what to do about it

Three surfaces the catalogue records as open, investigated and costed. **This
document changes no code.** It deletes nothing, wires nothing, and corrects no
other file — including the stale claims section 5 records, which are handed on
rather than fixed. Where it disagrees with `CONTRIBUTING.md`, `ROADMAP.md` or an
ADR, the disagreement is stated and the correction is somebody else's change.

- [0. How to disbelieve this report](#0-how-to-disbelieve-this-report)
- [1. The causal slice and `--trace`](#1-the-causal-slice-and---trace) (item 15)
- [2. `AssertionKind::RecursionLimit`](#2-assertionkindrecursionlimit) (item 14)
- [3. The compiled seam](#3-the-compiled-seam) (item 13, first bullet)
- [4. Is the pattern still spreading? A gate specification](#4-is-the-pattern-still-spreading-a-gate-specification)
- [5. Stale claims found outside the three surfaces, handed on](#5-stale-claims-found-outside-the-three-surfaces-handed-on)

---

## 0. How to disbelieve this report

> **Round 2, 2026-08-27: four claims in this report have been withdrawn in
> place, and the recommendations are unchanged.** Adversarial review reproduced
> the central measurements independently and found three defects in how they
> were written up. All three are corrected below, each in a block that quotes the
> withdrawn text verbatim:
>
> | | what was wrong | where the correction is | effect on the recommendation |
> | --- | --- | --- | --- |
> | D1 | §2.1 asserted, **as a measurement**, that only `diagnostic.message` distinguishes a runaway recursion from a failed `assert_eq`. It is `E0501` vs `E0502` — two registered codes. The pre-registered S4 decision rule had committed to that wording in advance, and its statistic could not license it | §2.1 | **none** — §2.4 and §2.5 already rested on the opposite, true fact |
> | D2 | three grep transcripts were printed trimmed, in a report whose exit criterion is *"a command or grep printed with its output"* | §1.1, §2.1 (twice) | none for two of them; the third corrects a cost number, "four tests" → 10 in the maintained tree, 14 with the spike |
> | D3 | the `failures` digest was printed with no canonicalization stated, so a reviewer computing it the usual way got a different value | §1.1 | none |
>
> The D1 defect is the one that matters: a pre-registered rule that encodes a
> conclusion broader than the statistic gating it cannot go red on the
> observation it names. **That is this repository's signature defect, committed
> by the document written to catalogue it.** It is treated at length in §2.1
> rather than summarised away here.
>
> Round-2 measurements were pre-registered before they were taken, in
> `/tmp/.../PREREG-r2-dead-surfaces-corrections.md`, outside the repository —
> two round-1 branches collided on a root-level `PREREGISTRATION.md`. Whatever
> matters from it is quoted where it is used.

**Where it was taken.** Worktree `~/.worktrees/ply/w5/dead-surfaces-report`, at
commit `d88aae5`. Nothing in the tree was edited while the numbers were taken.
If `main` has moved, this describes `d88aae5` and says so.

**The instrument.** `target/debug/ply`, built in this worktree at 20:24 local,
and the same binary in both rounds — its mtime is unchanged. Run immediately
before each runtime series:

```
$ find crates -name '*.rs' -newer target/debug/ply
$                                      # prints nothing — no .rs source is newer

$ find crates \( -name '*.rs' -o -name '*.ply' \) -newer target/debug/ply
$                                      # prints nothing — no .ply is newer either
```

The second form is round 2's, and it is wider on purpose. The first is the
`CONTRIBUTING.md` recipe, which is blind to a stale `.ply`: `crates/ply-std/src/lib.rs`
`include_str!`s every stdlib module into the binary, so an edited `.ply` is
compiled in and invisible to an `.rs`-only check. Neither form changes any
conclusion here — the fixtures this report runs are in `tests/fixtures`, not the
stdlib — but the wider check is the one that can be trusted, so it is the one
recorded. Note that the corrected recipe as *written* in the house rules,
`find crates -name '*.rs' -o -name '*.ply' -newer BIN`, is mis-parenthesised and
can never print nothing; §5 item 9 records that for `r2-instrument`.

**The protocol.** `PREREGISTRATION.md` at the worktree root, written before any
number in this document existed, carries the statistic, the run count and the
decision rule for each series, plus two dated amendments made *before* the runs
they govern. Every statistic here is deterministic — the nullity of a JSON field,
or a count of grep sites — which `CONTRIBUTING.md` §"And know which of your
numbers this even applies to" exempts from the load gate (*"deterministic: they
reproduce to the digit on a burning machine"*). **No timing or performance number
is in this document**, and the pre-registration forbids adding one without a
dated amendment first. Load is recorded anyway and it is high:

| series | runs | load before | load after |
| --- | --- | --- | --- |
| S1 — `causal_slice`/`assertion` nullity, 3 flag arms | 15 | 8.38 | 8.38 |
| S4 — the same on a runaway recursion | 5 | 49.99 | 49.99 |
| S3 — the grep inventory | 3 | 15.89 | 16.54 |
| §4 gate check | 3 | 14.20 | 14.20 |
| R1 (round 2) — `diagnostic.code` by failure class, 2 arms | 10 | 11.26 | 12.28 |
| R1b (round 2) — the 60-fixture sweep | 60 | 10.08 | 9.75 |
| R3 (round 2) — the digest, re-derived, 3 flag arms | 15 | 5.29 | 5.29 |

Every repeated series in that table was identical across all its runs, in both
rounds — S1, S4, S3, the §4 gate check, R1 (5 rounds per arm) and R3 (5 rounds
per arm) each collapsed to one value. **R1b is the exception and is not a
repeated series**: it is 60 *different* fixtures run once each, so "identical
across runs" is not a claim that can be made of it and is not made. It is
reported for its spread, not its stability. That distinction is the point of the
load reading: it is evidence that these numbers do not depend on load, not an
excuse.

**One instrument in this workstream already lied, and it lied stably.** This
belongs at the top rather than in a footnote, because it is the same species of
defect the report is about. The first version of section 4's check returned
**25** registered-but-unconstructed codes, identically on three consecutive runs.
25 was wrong. Its string-literal stripper was a regex, the regex mis-parsed
`r#"…"#`, and the mis-parse cascaded and deleted **76% of `infer.rs`**
(240,286 bytes → 58,210), hiding a constructor sitting in plain sight at
`crates/ply-core/src/infer.rs:5410`. It was caught by hand-checking five of the
25 names against the tree. The number is recorded **void**, with its cause, in
`PREREGISTRATION.md` §S2, and it is why section 4's central demand is a positive
control rather than a cleverer regex: *three identical runs of a broken
instrument are three identical wrong numbers.*

**What is load-bearing and what is corroboration.** Every claim of the form
"nothing constructs X" is an argument from absence, and absence is what greps are
worst at — a macro, a `cfg`, a re-export, a trait default. So for the slice and
the assertion the load-bearing evidence is **runtime observation of the shipped
binary**, which no spelling can fool: S1 and S4's 20 runs across three flag
settings in round 1, and in round 2 a further 25 runs, 60 fixtures and three
engines. The greps corroborate and localise. Where only a grep supports a claim,
the text says so — and §2.1 records what happened when a claim that only a
*runtime* series could support was made about fields that series never looked at.

**What this workstream changed.** Round 1: two files, this one and
`PREREGISTRATION.md`. Round 2: **this file only** — `PREREGISTRATION.md` is
deliberately left carrying the over-broad S4 wording D1 withdraws, because a
pre-registration is a record of what was committed to before the numbers existed
and editing it afterwards would destroy the only thing it is for. No source file,
no ADR, no `CONTRIBUTING.md`, no `CONTRACTS.md`, no `ROADMAP.md`, in either
round. **This worktree ran no git command** in either round, so that claim is
asserted here and **checked by the parent agent running `git status`** — not by
me. If `git status` shows anything else, this report is in breach of its own
brief and the extra change should be reverted rather than reviewed.

---

## 1. The causal slice and `--trace`

*Catalogue item 15.*

### 1.1 What is actually there

**Measured first, read second.** S1: `ply test tests/fixtures/assertion_failed.ply
--json --no-cache` under each of `--trace never`, `--trace auto`, `--trace
always`, arms interleaved, five rounds, fifteen runs.

`--no-cache` is on every invocation deliberately, and it was added to the
pre-registration as an amendment before any run. `CONTRIBUTING.md` §"Things known
to be broken" item 3 records a step that reported `0 failed, 0 passed, 68 cached`
and exited 0 — a green result over a run that never happened. Concluding "the
field is null" from a run that did not occur would be a particularly stupid
instance of this repository's signature defect.

```
$ for r in 1 2 3 4 5; do for a in never auto always; do
    ./target/debug/ply --color never test tests/fixtures/assertion_failed.ply \
      --json --no-cache --trace $a > r${r}_${a}.json
  done; done
$ python3 -c "…json.load…"          # per run: causal_slice, assertion, suspects[].ran/.depth
r1_never   failures=1 causal_slice=[None] assertion=[None] ran=[None] depth=[None]
r1_auto    failures=1 causal_slice=[None] assertion=[None] ran=[None] depth=[None]
r1_always  failures=1 causal_slice=[None] assertion=[None] ran=[None] depth=[None]
…                                   (all 15 identical)
r5_always  failures=1 causal_slice=[None] assertion=[None] ran=[None] depth=[None]
```

**15 of 15.** No arm differs from any other arm, and no run differs from any run
in its arm.

**`--trace` changes nothing, and the catalogue does not say this.** The
`failures` array is identical across all fifteen runs. **The canonicalization,
which the first version of this section did not state, is:**

```python
hashlib.sha256(json.dumps(failures, sort_keys=True).encode()).hexdigest()
```

`json.dumps` with `sort_keys=True` and **Python's default separators**, `', '`
and `': '` — *not* the compact `separators=(',', ':')` that "canonical JSON"
usually means. Under that canonicalization, re-derived here on a fresh 15-run
series, CPython 3.14.1:

```
$ for r in 1 2 3 4 5; do for a in never auto always; do
    ply --color never test tests/fixtures/assertion_failed.ply --json --no-cache --trace $a
  done; done                       # then digest failures[] two ways
sha256 over json.dumps(failures, sort_keys=True)                       : {'7962884c3f8d40d2': 15}
sha256 over json.dumps(failures, sort_keys=True, separators=(',',':')) : {'4057a8497bdcf0ac': 15}
```

Full digests:

| canonicalization | sha256 |
| --- | --- |
| `json.dumps(failures, sort_keys=True)` — the one this report used | `7962884c3f8d40d21ea0bde1fe6d4af6a9b8ae037e2f1dd9ffca3e47c9ae9549` |
| `json.dumps(failures, sort_keys=True, separators=(',',':'))` — compact | `4057a8497bdcf0acf73fc2f1d68398d69031f0da62a2c12956005f81de2c1d09` |

> **Correction in place (2026-08-27): the digest was printed without its
> canonicalization, which made it unreproducible.** The text read:
>
> > *"The `failures` array is **byte-identical** across all fifteen runs — same
> > SHA-256 of its canonical JSON, `7962884c3f8d40d2…`, for `never`, `auto` and
> > `always` alike"*
> >
> > *"`python3 -c "…hashlib.sha256(json.dumps(d['failures'],sort_keys=True))…"`"*
>
> An independent reviewer digesting the same array under
> `json.dumps(sort_keys=True, separators=(',',':'))` — the usual reading of
> "canonical JSON" — got `4057a8497bdcf0ac…` and could not reproduce the printed
> value. Both digests are correct. They are digests of different byte strings,
> because Python's default `json.dumps` separators insert a space after every
> `,` and `:`, and the compact ones do not.
>
> Precisely where it went wrong, since that is the whole subject here: the
> printed command was *literally* right — `json.dumps(d['failures'],sort_keys=True)`
> passes no `separators`, so it gets the defaults. But it was wrapped in `…` at
> both ends, so a reader could not tell whether the elision hid a `separators=`
> argument, and the prose beside it said **"canonical JSON"**, which points at
> the compact form. An elided command plus a phrase that names a *different*
> convention is not a stated canonicalization. **The digest is not withdrawn —
> it reproduces, 15 of 15, and is re-derived above — but the canonicalization is
> now stated in full and both forms are printed, so neither reader is
> stranded.**
>
> One word is withdrawn: **"byte-identical"**. It is true of the serialized form
> under a fixed canonicalization, which is what was measured; it is not a
> property of the array independent of one. The `failures` array carries no
> `duration_ms` or other varying field (checked: no key under `failures[]`
> matches `dur|ms|time`), which is *why* the digest is stable, and that is the
> claim that should have been made.

Diffing a whole report between `--trace never` and `--trace always` leaves
exactly one non-timing difference — the echoed string the CLI prints back at
`crates/ply-cli/src/commands/test.rs:1383`:

```
$ diff <(…r1_never.json…) <(…r1_always.json…)
135c135
<   "trace": "never"
---
>   "trace": "always"
```

(The other diff hunks are `duration_ms` fields.) So the flag's only observable
effect on the artifact is to repeat itself back.

**Why. The chain, each link checked.**

| link | site | what it is |
| --- | --- | --- |
| the field starts `None` | `crates/ply-test/src/lib.rs:283`, initialised at `:305` | `Attribution::slice: Option<CausalSlice>`, set to `None` by `from_suspects` |
| its only writer | `crates/ply-test/src/lib.rs:312` `Attribution::resolve(bisection, slice)` | called from `crates/ply-test/src/diagnose.rs:109` |
| what that writer is handed | `crates/ply-test/src/lib.rs:1331` `slice: failure.attribution.slice.clone()` | **itself** — the loop is closed and starts at `None` |
| the flag's destination | `crates/ply-cli/src/commands/test.rs:349-352` writes `ply_test::Options::trace` | `crates/ply-test/src/diagnose.rs:22` — **and nothing reads it** |

The last row is new and the catalogue does not carry it. `diagnose::Options` has
three fields; `bisect` and `budget` are read inside `diagnose` (`diagnose.rs:76`,
`:96`), `trace` is not read anywhere in `ply-test`:

```
$ grep -rn '\.trace\b\|trace:' --include='*.rs' crates/ply-test/src
crates/ply-test/src/diagnose.rs:22:    pub trace: Tracing,
crates/ply-test/src/diagnose.rs:30:            trace: Tracing::Never,
crates/ply-test/src/diagnose/tests.rs:570:            trace: Tracing::Never,
```

> **Correction in place (2026-08-27): the transcript above was printed with its
> last line removed, and the sentence under it counted the trimmed output.** The
> block printed two of the grep's three lines, dropping
> `crates/ply-test/src/diagnose/tests.rs:570`, and the sentence read:
>
> > *"Two definitions, zero reads."*
>
> **Withdrawn, restated: three occurrences, still zero reads.** The third is a
> `#[cfg(test)]` module building an `Options` literal — a third *write* of the
> field, not a read of it. Re-run raw in this worktree, the grep emits 3 lines,
> identical on 3 of 3 runs. The section's conclusion is unchanged and is mildly
> reinforced: the number of sites that *read* `Options::trace` is still zero, and
> the restored line is one more site that only ever sets it. It is restored
> anyway, because this report's own exit criterion is *"a command or grep printed
> with its output"*, and a transcript trimmed to the length of the sentence under
> it does not meet that criterion whichever way the conclusion falls. A trimmed
> transcript presented as raw output is the failure this report is about.

Zero reads. `Tracing`'s own predicates have no callers at all:

```
$ grep -rn 'traces_first_run\|traces_replay\|Tracing::parse' --include='*.rs' . | grep -v '^\./target'
crates/ply-test/src/slice.rs:146:    pub fn traces_first_run(self) -> bool {
crates/ply-test/src/slice.rs:150:    pub fn traces_replay(self) -> bool {
```

Two `fn` definitions and no call site; `Tracing::parse` (`slice.rs:137`) has zero
occurrences outside its own definition.

**The producers.** S3, three identical runs:

```
$ grep -rn 'SliceBuilder::new()\|SliceBuilder::with_cap(' --include='*.rs' . | grep -v '^\./target'
crates/ply-test-tests/tests/suite/bisect_audit.rs:993    SliceBuilder::new()      test
crates/ply-test-tests/tests/suite/bisect_audit.rs:1021   SliceBuilder::new()      test
crates/ply-test-tests/tests/suite/bisect_audit.rs:1038   SliceBuilder::with_cap(2) test
crates/ply-test-tests/tests/suite/bisect_audit.rs:1079   SliceBuilder::new()      test
crates/ply-test/src/slice.rs:188             SliceBuilder::with_cap(DEFAULT_CAP)  its own Default impl
crates/ply-test/src/slice.rs:433             SliceBuilder::new()      its own #[cfg(test)] mod
crates/ply-test/src/slice.rs:485             SliceBuilder::with_cap(2) its own #[cfg(test)] mod

$ grep -rn 'Event::Perform(' --include='*.rs' . | grep -v '^\./target'
crates/ply-test/src/slice.rs:243             the match arm that consumes one
crates/ply-test/src/slice.rs:505             its own unit test
```

> **Correcting the brief and `CONTRIBUTING.md` item 15 in the same breath.** Both
> say `SliceBuilder` *"is constructed in exactly one place in the workspace —
> `crates/ply-test-tests/tests/suite/bisect_audit.rs`, four times, all tests"*. It is
> **seven sites in two files**: the four in `bisect_audit.rs`, two more in
> `slice.rs`'s own `#[cfg(test)]` module, and one inside its own `Default` impl
> at `slice.rs:188`, which is a definition rather than a use. The substantive
> claim survives untouched — **no production code outside `slice.rs` constructs
> one** — and the runtime observation above is what actually establishes it.

**The dead consumers. There are two, and the catalogue names one.**

```
$ sed -n '792,807p' crates/ply-cli/src/commands/test.rs
    if let Some(slice) = &failure.attribution.slice
        && slice.traced
        && !slice.stack.is_empty()
    {
        let path: Vec<&str> = slice.path().iter().map(|n| n.as_str()).collect();
        lines.push(format!("  {} {}", style.dim("ran:"), path.join(" → ")));
        if !slice.reproduced { … "the replay did not reproduce this failure" … }
    }

$ sed -n '310,313p' crates/ply-test/src/report.rs
fn ran_path(attribution: &Attribution) -> Option<String> {
    let slice = attribution.slice.as_ref()?;
    if !slice.traced || slice.stack.is_empty() {
        return None;
```

`report.rs:310`'s `ran_path` is called at `report.rs:243` and returns `None` on
every input the binary has ever handed it. It is `ply-test`'s own text
projection; `test.rs:793` is the CLI's. Both are dead, in two different crates.

**The consequence nobody has written down: the suspect ranking does not rank.**
`Suspect::ran` and `Suspect::depth` are written in exactly one place —

```
$ grep -n 'suspect.ran = \|suspect.depth = ' crates/ply-test/src/lib.rs
320:                suspect.ran = slice.did_run(&suspect.name);
321:                suspect.depth = slice.depth_of(&suspect.name);
```

— inside `resolve`'s (`lib.rs:316-322`) `if let Some(slice) = &slice && slice.traced &&
slice.reproduced` guard, whose `slice` is always `None`. So both are always
`null` (confirmed in all 15 S1 runs and all 5 S4 runs), and `Suspect::rank` at
`lib.rs:255` collapses:

```rust
let tier = match (self.culprit, self.ran, self.depth) {
    (true, ..) => 0,                       // reachable
    (false, _, Some(_)) => 1,              // UNREACHABLE — depth is always None
    (false, Some(true), None) => 2,        // UNREACHABLE — ran is always None
    (false, None, None) => 3,              // reachable
    (false, Some(false), None) => 4,       // UNREACHABLE — ran is always None
};
…
(tier, self.depth.unwrap_or(usize::MAX), inherited, self.name.as_str())
//     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ a constant, in every run
```

**Three of the five tiers are unreachable and the secondary sort key is a
constant.** The ranking that ships is: culprits first, then everything else by
`Derived`-ness and then by name. ADR 0004 specifies five tiers and says
"innermost first"; two tiers ship.

### 1.2 What the decision record promised, and what a reader believes today

**ADR 0004 "The causal slice"** (`docs/adr/0004-machine-shaped-failure.md` §5)
specifies `stack`, `entered` and `observed`, and closes: *"`ply-eval` gains the
recorder; the hook is `Interp::apply` for named closures and the perform site for
atoms."*

**ADR 0004** prints an example failure object with a populated `causal_slice`,
then justifies each field in a 20-row table (`0004:403-422`).

**`CONTRACTS.md:1315`** heads the type's section **"Causal slice — landed in
`ply-test::slice`"**. That is the sentence a reader believes: *landed*.

**ADR 0004 already carries an in-place correction** (`0004:424-459`), dated
2026-08-24, headed:

> **Correction in place (2026-08-24): four rows of this table describe a field
> no report has ever carried.**

**That correction undercounts, by more than a factor of two.** Counted from the
ADR and checked against the S1 and S4 output:

```
$ python3 -c "…rows of the §7 table…"
total rows in the §7 table: 20
  403 culprit.verdict     407 culprit.groups   412 causal_slice.stack        417 suspects[].ran
  404 culprit.skipped     408 culprit.reason   413 causal_slice.entered[]…   418 suspects[].depth
  405 culprit.confidence  409 culprit.search   414 causal_slice.observed_…   419 footprint.declared vs observed
  406 culprit.definitions 410 assertion.expected/actual   415 causal_slice.reproduced   420 test_hash
                          411 assertion.first_difference.path                421 location  422 diagnostic
```

**Nine of the twenty rows describe a field that is `null` in every report this
binary writes.** The four the correction names (412–415), plus:

- **417 `suspects[].ran`** — *"`false` means it cannot have caused this,
  whatever its hash did"*. Always `null`.
- **418 `suspects[].depth`** — *"how close to the failure it sits; the ranking is
  already applied, this is why"*. Always `null`, and per §1.1 the ranking is
  **not** applied.
- **419 `footprint.declared` vs `observed`** — *"a declared atom that never fired
  is a branch not taken"*. The left half is real, built from `test.footprint` at
  `crates/ply-cli/src/commands/test.rs:1489`; the right half is
  `failure.attribution.slice…observed` at `:1493-1498` and is therefore always
  `null`. **The comparison the row asks for cannot be performed.**
- **410 and 411, the `assertion` rows** — section 2 of this report.

**What a reader of the docs believes today, in one sentence each.** From ADR 0004: that a failure report names the path through the source to the failure, the
call counts along it, which handler fired, and which suspects did not run. From
ADR 0004: that suspects are sorted innermost-first. From `CONTRACTS.md:1315`:
that the causal slice has landed. From `ply test --help`: that `--trace always`
will *"trace the first execution too"*. **All five are false of the shipped
binary**, and only the first is corrected anywhere.

### 1.3 Option ARM

The two halves of the slice differ by an order of magnitude in cost and should be
priced separately, because **the cheap half is the dangerous one**.

#### 1.3a `observed` — nearly free, and the trap

`ply_eval` already records it. Both engines call `Trace::record` on every
`perform` (`crates/ply-eval/src/machine.rs:784`,
`crates/ply-eval/src/interp.rs:817`), `Trace` is cleared per entry point
(`interp.rs:314`, `machine.rs:834`, both inside `reset()`), and the value is
already exposed through the trait `ply-test` drives the engines with:

```
$ sed -n '51,53p;104,110p' crates/ply-eval/src/differential.rs
    fn observed_footprint(&self) -> Option<Footprint> { None }        # trait default
    fn observed_footprint(&self) -> Option<Footprint> {               # Interp
        Some(self.trace().footprint().clone())
    }
    fn observed_performs(&self) -> Option<u64> { Some(self.trace().performs()) }
```

`ply-test` runs tests through `run_one<E: ply_eval::Evaluator>`
(`crates/ply-test/src/lib.rs:881`), so arming `observed` is: call
`e.observed_footprint()` after `run_one`, carry it, and stop passing `None`.
**Two files, `ply-test/src/lib.rs` and `ply-test/src/diagnose.rs`; no evaluator
change at all.**

> **This is where an already-corrected claim becomes false again, on the other
> axis.** `crates/ply-test/src/slice.rs:47-67` corrected, in place, on
> 2026-08-24, a comment that read *"The atoms actually performed, which is a
> subset of the declared footprint."* The correction's own closing paragraph says
> the fix *"is to the contract rather than to an output anyone has seen"* —
> precisely because nothing builds one. **Arming `observed` produces the first
> output anyone has seen.** `Trace::record` fires on every `perform` including
> one a `handle` inside the call discharges, and discharging is exactly what
> keeps an atom out of a published row, so the first real `observed` will contain
> atoms that appear in **no declared row anywhere** — on the fixture item 11
> added, `tests/fixtures/self_handled_effect.ply`, `handled` publishes `{}` and
> records `{tally.read[log], tally.write[log]}`. Any ARM that stops at the cheap
> half must budget for re-correcting three places: `slice.rs:47-67`'s comment,
> ADR 0004 row 414 (`causal_slice.observed_footprint` — *"which branch was taken,
> and which handler fired"*), and ADR 0004 row 419 (`footprint.declared` vs
> `observed`). ADR 0004's correction block already got halfway there; it corrects
> the *reading* of the field but leaves the rows as written.

#### 1.3b `entered` / `stack` / `reproduced` — the expensive half

Nothing in `ply_eval` records a call. `Trace` holds a `Footprint` and a `u64`
(`crates/ply-eval/src/trace.rs:21-24`) and nothing else. What ARM needs:

| what | where | risk |
| --- | --- | --- |
| `Enter`/`Return` events with the qualified name and the caller's span | `Interp::apply` (`crates/ply-eval/src/interp.rs:722`) **and** `Machine::apply` (`crates/ply-eval/src/machine.rs:1861`) — CONTRACTS.md:1382 names only the first, and Ply has had two engines since | a push and a pop per call on both hot paths. ADR 0004 anticipates this and is why tracing is specified to run on a **re-run** |
| a traced re-run, and the reproduction check | `ply-test/src/lib.rs`, `diagnose.rs`; `Options::trace` finally read | a second execution of a failing test, per failure |
| `truncated` handling all the way to the artifact | already written (`slice.rs:220-241`, `report.rs:455`) | none — this half is built |
| **the result-cache question** | unanswered anywhere in the record | a traced re-run is a second execution strategy. Whether its result may enter the cache is the same question §3 asks about a backend, and it is **not** settled by any ADR. If the answer changes evaluation semantics, house rule 9 requires bumping `RUNTIME_VERSION` (`crates/ply-store/src/lib.rs:83`, currently `0.11.2`) with a reason. **Flagged as open, not answered here.** |

#### 1.3c The suite assertions that go red

Nine, in three files. Every one currently asserts the null:

```
$ grep -rn 'causal_slice"\].is_null\|assertion"\].is_null\|observed"\].is_null\|attribution\.slice\.is_none' --include='*.rs' . | grep -v '^\./target'
crates/ply-cli/tests/suite/cli.rs:1643:    assert!(f["footprint"]["observed"].is_null());
crates/ply-cli/tests/suite/cli.rs:1644:    assert!(f["assertion"].is_null());
crates/ply-cli/tests/suite/cli.rs:1645:    assert!(f["causal_slice"].is_null());
crates/ply-cli/src/commands/test.rs:2008:            f["assertion"].is_null(),        # "the evaluator carries no payload yet"
crates/ply-cli/src/commands/test.rs:2017:        assert!(f["causal_slice"].is_null(), "nothing traced this run");
crates/ply-cli/src/commands/test.rs:2020:            f["footprint"]["observed"].is_null(),
crates/ply-test/src/tests.rs:1835:    assert!(failure.attribution.slice.is_none());
crates/ply-test/src/tests.rs:2010:    assert!(failure["causal_slice"].is_null());
crates/ply-test/src/tests.rs:2011:    assert!(failure["assertion"].is_null());
```

**Six of the nine go red on the cheap half alone**, and the reason is worth
stating because it is not obvious: `footprint.observed` is read *through* the
slice (`crates/ply-cli/src/commands/test.rs:1493-1498` filters on `s.traced`),
so arming `observed` means constructing a `CausalSlice` with `traced: true` and
empty `stack`/`entered` — which makes `causal_slice` non-`null` too, with three
of its five fields empty. The six are `cli.rs:1643`, `cli.rs:1645`,
`test.rs:2017`, `test.rs:2020`, `tests.rs:1835`, `tests.rs:2010`. **The full
slice ARM makes the same six red and no more** — the remaining three
(`cli.rs:1644`, `test.rs:2008`, `tests.rs:2011`) are about `assertion` and belong
to section 2, which is why they are listed here rather than counted here.

An implementer has a choice at that point, and it should be made deliberately
rather than fallen into: either publish a half-empty `causal_slice` (and accept
that a consumer reading `stack: []` cannot tell "traced and nothing on the stack"
from "the stack half is not built"), or re-plumb `footprint.observed` to read
from somewhere other than the slice. The second is cleaner and is one more file.

Two of the nine carry messages that are themselves claims about the gap — *"the
evaluator carries no payload yet"* and *"an untraced run must not claim an empty
observed footprint"* — and the second is a **good** assertion that must survive
in some form: `null` and `[]` are different findings, and ARM must keep them
different rather than delete the test that says so.

### 1.4 Option DELETE

**What goes.** `CausalSlice`, `SliceBuilder`, `Event`, `Entered`, `Frame`,
`Tracing` (`crates/ply-test/src/slice.rs:13-270` — 258 of the file's 532 lines,
plus its unit tests, `#[cfg(test)]` from `:360` to the end); `Attribution::slice` (`lib.rs:283`) and the
`resolve` guard at `:316-322`; `Suspect::ran` / `Suspect::depth` (`lib.rs:231`,
`:233`) and three tiers of `rank()`; `slice_json` (`report.rs:455`) and
`ran_path` (`report.rs:310`); the `test.rs:793` branch; `Evidence::slice`
(`diagnose.rs:55`) and `Options::trace` (`:22`); the `--trace` flag on `ply test`
(`crates/ply-cli/src/cli.rs:376-377`); and the four `SliceBuilder` tests in
`bisect_audit.rs`.

**`SCHEMA_VERSION` bumps, 4 → 5.** By `report.rs:18-19`'s own rule — *"Bumped
whenever a field in the failure artifact changes meaning or leaves"* — and the
doc block below it is explicit that an *addition* is not a bump while a departure
is. `causal_slice`, `suspects[].ran`, `suspects[].depth` and `footprint.observed`
all leave. This is a breaking change to a consumer that reads the artifact.

**Documents that need correcting in place** (quoting the withdrawn text, per the
house convention):

| document | what needs withdrawing |
| --- | --- |
| `docs/adr/0004-machine-shaped-failure.md` §5 | all of it |
| `docs/adr/0004-machine-shaped-failure.md` §7 | the example object's `causal_slice`, `suspects[].ran` and `suspects[].depth` |
| `docs/adr/0004-machine-shaped-failure.md` §7 | six rows of its field table, and the `observed` half of a seventh |
| `docs/adr/0004-machine-shaped-failure.md` §6 | its "innermost first" ranking, which becomes unimplementable |
| `CONTRACTS.md:1170` | `Attribution::slice` |
| `CONTRACTS.md:1315-1335` | *"Causal slice — **landed** in `ply-test::slice`"* and the type listing |
| `CONTRACTS.md:1382` | *"`ply-eval` gains the tracer — hooked at `Interp::apply`"* |
| `CONTRACTS.md:1393` | the `--trace <auto\|always\|never>` line in the flag block |
| `CONTRIBUTING.md:1616-1650` | item 15, and the `CausalSlice` row of §"The shape it keeps taking" |
| `crates/ply-eval/src/limit.rs:80-95` | its correction block cites the slice |

**The capability genuinely lost, stated two ways because they differ.**

ADR 0004's thesis is at `slice.rs:1-6`: *"Forty definitions in the closure and
three on the stack is the difference between a list to read and an answer to act
on, and the two are not derivable from each other."* `suspects[].change` narrows
by **edit**; only `ran` narrows by **execution**. Deleting the slice makes "this
suspect did not run, so it cannot have caused this" permanently underivable from
the artifact — bisection can name a culprit but cannot exonerate a non-culprit,
and the two are different questions.

**But be honest about which loss this is.** It is *"the language gives up a
documented capability it has never once delivered"*, not *"a working feature is
removed"*. No user has ever seen a `ran` field that was not `null`. A DELETE that
prices this as losing a shipped capability is overstating it; a DELETE that
prices it as free is understating it, because ADR 0004 is the only place the
intended shape is recorded and deleting the code deletes the design.

### 1.5 Recommendation

**ARM the cheap half (`observed`) now; DELETE `entered`/`stack`/`reproduced` and
the `--trace` flag; write down the one open question.**

*One-line reason:* `observed` is already computed by the evaluator and thrown away
— two or three files of wiring buys back three of ADR 0004's nine dead rows — while
`entered`/`stack` needs a push and a pop per call in **two** engines to deliver a
field that has never existed, which is a milestone, not a fix.

**Rough cost each way.**

| | files touched | tests that go red | documents to correct | new risk |
| --- | --- | --- | --- | --- |
| ARM `observed` only | 2-3 (`ply-test/src/lib.rs`, `diagnose.rs`, and `commands/test.rs` if `footprint.observed` is re-plumbed off the slice) | **6** — `cli.rs:1643`, `:1645`, `test.rs:2017`, `:2020`, `tests.rs:1835`, `:2010`; two of them must be *rewritten* rather than deleted, to keep `null` and `[]` apart | 3 (`slice.rs:47-67` again, ADR 0004 rows 414 and 419) | the trap above: the first real output contains atoms in no declared row |
| ARM everything | + `ply-eval/src/interp.rs`, `machine.rs`, `trace.rs`, `slice.rs`, `report.rs`, `commands/test.rs` | the same **6** | + ADR 0004, `CONTRACTS.md:1382` | a push and a pop per call in both engines; result-cache question unanswered; possible `RUNTIME_VERSION` bump |
| DELETE everything | 8 source files | **6**, deleted rather than fixed, plus the 4 `SliceBuilder` tests in `bisect_audit.rs` | 10 sections across 4 documents | `SCHEMA_VERSION` 4 → 5, breaking for artifact consumers |

The split recommendation costs the least and closes the most: it removes the
`--trace` flag that provably does nothing (§1.1's identical digests across the
three arms are the evidence), keeps the one field the engines already pay for,
and leaves the
expensive half as an explicit, costed deferral rather than as dead code that
reads like a feature.

**If the split is rejected, DELETE beats full ARM.** ADR 0004's own standard
is *"A field an agent cannot act on is noise … a field that cannot answer it
should be deleted rather than defended."* Nine rows currently fail that test.

---

## 2. `AssertionKind::RecursionLimit`

*Catalogue item 14.*

### 2.1 What is actually there — and the brief understates it

**First, the line numbers in the brief and in `CONTRIBUTING.md` are stale, by
exactly +19.**

```
$ grep -n 'RecursionLimit\|AssertionKind::Eq\|enum AssertionKind' crates/ply-test/src/slice.rs
275:pub enum AssertionKind {
287:    RecursionLimit,                          # brief and CONTRIBUTING.md:1603 say 268
303:            AssertionKind::RecursionLimit => "recursion_limit",   # they say 284
345:            ..Assertion::new(AssertionKind::Eq)                   # they say 326
```

Every one is +19. The +19 is the in-place correction block on
`CausalSlice::observed` at `slice.rs:47-67` that item 11's fix added — **the
correction that closed one claim silently invalidated the coordinates of the
claim next to it**, in three places: `CONTRIBUTING.md:1603` and `:1606`, the §"The
shape it keeps taking" table row at `CONTRIBUTING.md:57`, and
`crates/ply-eval/src/limit.rs:85`, whose own correction block reads *"finds `Eq`
built at `slice.rs:326`"*. A correction carrying a stale coordinate is worth
recording on its own: this is the second-order form of the defect this repository
keeps catching.

**Second, and this changes the recommendation: it is not one variant that
classifies nothing. Nothing constructs an `Assertion` at all.**

```
$ grep -rn '\bAssertion\b' --include='*.rs' . | grep -v '^\./target' | grep -v AssertionKind \
    | sed 's|^\./||'                 # the trailing sed only strips the leading ./ that
crates/ply-test/src/report.rs:10:use crate::slice::{Assertion, CausalSlice};
crates/ply-test/src/report.rs:498:fn assertion_json(assertion: &Assertion) -> Value {
crates/ply-test/src/lib.rs:370:    pub assertion: Option<Assertion>,
crates/ply-test/src/slice.rs:321:pub struct Assertion {
crates/ply-test/src/slice.rs:330:impl Assertion {
crates/ply-test/src/slice.rs:332:        Assertion {
crates/ply-test/src/slice.rs:341:    pub fn eq(expected: impl Into<String>, actual: impl Into<String>) -> Assertion {
crates/ply-test/src/slice.rs:342:        Assertion {
crates/ply-test/src/slice.rs:349:    pub fn with_difference(mut self, difference: Difference) -> Assertion {
crates/ply-test/src/slice.rs:354:    pub fn with_message(mut self, message: impl Into<String>) -> Assertion {
```

> **Correction in place (2026-08-27): the transcript above was printed with half
> its lines removed, and the sentence under it counted the half that was
> printed.** The block showed 5 of the grep's 10 lines — `slice.rs:332`, `:341`,
> `:342`, `:349` and `:354` were dropped, and the `:330` line carried an
> editorial annotation, `# new, eq, with_difference, with_message`, that the
> grep does not emit. The sentence read:
>
> > *"Five occurrences: one import, one renderer, one field, one declaration, one
> > impl block. **Zero constructions.**"*
>
> **Withdrawn, restated: ten occurrences, and "zero constructions" needs a
> qualifier.** The five restored lines are all inside `Assertion`'s own `impl`
> block — three return types (`:341`, `:349`, `:354`) and **two struct literals**,
> `:332` inside `Assertion::new` and `:342` inside `Assertion::eq`. Those two are
> constructions. The defensible claim is **zero constructions outside the type's
> own constructors, and no caller of those constructors**, which is what the next
> grep establishes and what the runtime series confirms: `failures[].assertion`
> is null in every artifact this binary writes. The conclusion survives; the
> printed evidence for it did not, and it is restored raw here.

The type's own constructors have no callers either — the single `Assertion::new`
/ `Assertion::eq` call site in the entire workspace is `slice.rs:345`, inside
`Assertion::eq` calling `Assertion::new`:

```
$ grep -rn 'Assertion::new(\|Assertion::eq(' --include='*.rs' . | grep -v '^\./target'
crates/ply-test/src/slice.rs:345:            ..Assertion::new(AssertionKind::Eq)
```

And the field is hard-wired to `None` at the one place a `Failure` is built:

```
$ sed -n '1523,1533p' crates/ply-test/src/lib.rs
                failures.push(Failure {
                    name: test.name.clone(),
                    key: test.key.clone(),
                    diagnostic: diagnostic.clone(),
                    defect,
                    host: host_backed,
                    suspects,
                    assertion: None,          # <- crates/ply-test/src/lib.rs:1530
                    attribution,
```

**So `AssertionKind::RecursionLimit` is a symptom, not a surface.** Constructing
one variant of a type that nothing constructs changes nothing. The dead surface is
`Assertion` — `kind`, `expected`, `actual`, `first_difference.path`, `message` —
the whole ADR 0004 `assertion` object.

**Third, measured, on the failure the item is actually about.** S4, pre-registered
as an amendment before it was run, five runs of the audit's own `RUNAWAY` program
(`crates/ply-cli/tests/suite/failure_classification_audit.rs:219-220`) in a scratch
directory:

```
fn spin(n: Int) -> Int = spin(n + 1)
test "spins" { assert_eq(spin(0), 0) }

$ ply --color never test --json --no-cache      # x5
run1 code= E0502 assertion= None causal_slice= None msg= recursion limit of 10000 nested calls exceeded
run2 code= E0502 assertion= None causal_slice= None msg= recursion limit of 10000 nested calls exceeded
run3 code= E0502 assertion= None causal_slice= None msg= recursion limit of 10000 nested calls exceeded
run4 code= E0502 assertion= None causal_slice= None msg= recursion limit of 10000 nested calls exceeded
run5 code= E0502 assertion= None causal_slice= None msg= recursion limit of 10000 nested calls exceeded
```

**5 of 5.**

> **Correction in place (2026-08-27): the sentence that stood here was false,
> and it was asserted as a measurement.** It read:
>
> > *"**5 of 5.** So the artifact distinguishes a runaway recursion from a failed
> > `assert_eq` **only** by the rendered `diagnostic.message` string — measured,
> > not inferred."*
>
> **Withdrawn.** The artifact carries a second distinction and it is
> machine-readable, and the S4 transcript immediately above shows half of it, in
> its first column: `code= E0502`. Re-measured here on the same binary, both
> arms, five interleaved rounds each, `--no-cache` on every invocation:
>
> ```
> $ # arm A: tests/fixtures/assertion_failed.ply   arm B: the RUNAWAY program above
> assert_r1   code=E0501  assertion=None  causal_slice=None  msg='assertion failed: expected 480000, found 470000'
> assert_r2   code=E0501  …            (5 of 5 identical)
> runaway_r1  code=E0502  assertion=None  causal_slice=None  msg='recursion limit of 10000 nested calls exceeded'
> runaway_r2  code=E0502  …            (5 of 5 identical)
> ```
>
> `E0501` is `ASSERTION_FAILED` (`crates/ply-span/src/lib.rs:522`) and `E0502` is
> `RUNTIME_ERROR` (`:526`) — two distinct **registered** codes, in the same field
> of the same artifact. **"Only" is false**, and it was false against this
> report's own printed output.
>
> The `contains("recursion limit` grep that stood under the sentence, offered as
> proof that string-matching was all that was available, was itself trimmed: see
> the second correction below.

**What the corrected sentence says.** `failures[].assertion` is null on both
kinds of failure — the `assertion` object classifies nothing, which is the claim
this section is actually about and which S4 does establish. What the artifact
*does* carry is `diagnostic.code`. Diffing the two failure objects field by
field, every difference is one of three kinds:

```
$ python3 -c "…flatten both failure objects, print keys whose values differ…"
.diagnostic.code            'E0501' vs 'E0502'        <- registered code: machine-readable classification
.diagnostic.message         'assertion failed: …' vs 'recursion limit of …'
.diagnostic.labels[0].message   'these values are not equal' vs 'this call is too deeply nested'
.diagnostic.notes[0], [1]   'expected: 480000' / 'actual: …' vs 'check for a recursive call …' / 'innermost calls: …'
.key .name .module .location.* .test_hash .suspects[0].{name,hash}
                            <- these differ because the two programs are different programs, not
                               because the failures are of different kinds
```

So the honest statement is: **the artifact distinguishes the two by
`diagnostic.code`, a registered machine-readable code, and additionally by three
rendered-string fields (`diagnostic.message`, `labels[].message`, `notes[]`).
The one field ADR 0004 designated for this job, `assertion`, is null on both.**

**The wider sweep the original series did not run.** S4 and S1 between them cover
two failures. Run over all 60 `tests/fixtures/*.ply`, five produce a failure
object, and they carry **five hits across four distinct codes** — while all four
slice/assertion fields are null in every one:

```
$ for f in tests/fixtures/*.ply; do ./target/debug/ply --color never test "$f" \
    --json --no-cache; done          # 60 fixtures
fixtures run: 60
fixtures producing a failure object: 5
  assertion_failed   code=E0501  assertion=None  causal_slice=None  ran=[None]                    depth=[None]
  bank_race          code=E0501  assertion=None  causal_slice=None  ran=[None,None,None,None,None] depth=[None,…]
  deadlock           code=E0414  assertion=None  causal_slice=None  ran=[]                        depth=[]
  runtime_error      code=E0502  assertion=None  causal_slice=None  ran=[None]                    depth=[None]
  unhandled_effect   code=E0303  assertion=None  causal_slice=None  ran=[None]                    depth=[None]
```

That is the strongest form of both halves at once: the `assertion` object is
dead across every failure the fixture suite can produce (5/5), **and** the
artifact already classifies those failures four ways without it.

**And the engine check neither series took.** The runaway program under all
three `--engine` settings — the two engines separately and the differential
`both` — because a classification that holds on only one engine is not a
classification:

```
$ for e in treewalk machine both; do ply --color never test --json --no-cache --engine $e; done
engine=treewalk  code=E0502  assertion=None  causal_slice=None  ran=[None]  depth=[None]
engine=machine   code=E0502  assertion=None  causal_slice=None  ran=[None]  depth=[None]
engine=both      code=E0502  assertion=None  causal_slice=None  ran=[None]  depth=[None]
```

All three settings agree, and the `failures` arrays are identical under both
canonicalizations named in §1.1 — `sha256` `155f8542e6aa3fcf…` with default
separators, `72616412138d236f…` compact, the same value in all three settings
either way. (Naming which is which is the point of §1.1's correction; a digest
without its canonicalization is not a checkable number.) This is
the same fact `crates/ply-cli/tests/suite/failure_classification_audit.rs:231` already
asserts in the tree — `assert_eq!(failure["diagnostic"]["code"], "E0502")`, over
the same three engines, in the very test this section quoted the `RUNAWAY`
program *from*. A standing test asserting on `diagnostic.code` was cited two
paragraphs above the sentence claiming only the message distinguishes them.

> **The pre-registration pre-committed to the false wording, and that is the part
> that matters.** `PREREGISTRATION.md` §5's S4 decision rule reads, verbatim:
>
> > *"All 5 runs `assertion == null` → the report states that the artifact
> > distinguishes a runaway recursion from a failed `assert_eq` **only** by the
> > rendered `diagnostic.message`, measured rather than inferred, and that
> > constructing `AssertionKind::RecursionLimit` alone would change nothing
> > because nothing constructs an `Assertion`."*
>
> **What that statistic could license.** S4's statistic is the nullity of
> `failures[0].assertion`. Nullity of that one field licenses the rule's second
> clause exactly — nothing constructs an `Assertion`, so constructing one variant
> of `AssertionKind` changes nothing that any report carries. That clause is
> sound and it survives.
>
> **What it could not license.** The first clause is a claim about *every other
> field in the artifact*: that no field but `diagnostic.message` separates the
> two failures. No observation of `assertion`'s nullity can bear on what
> `diagnostic.code` contains. The rule encoded a conclusion strictly broader than
> the statistic gating it, so the "all 5 runs null" branch could be taken while
> the conclusion it authorised was false — which is what happened. **A
> pre-registered rule whose conclusion is wider than its statistic cannot go red
> on the observation it names, so passing it explores nothing. That is the same
> species of defect this report was written to catch, committed by this report.**
>
> Sharper still: S4's own statistic list included `failures[0].diagnostic.code`,
> and the series *recorded* `E0502` in all five runs. The refuting datum was in
> the series' own printed output. The pre-registered wording walked past it,
> because the wording had been fixed before the number existed and nothing in the
> rule made the number able to contradict it.
>
> **`PREREGISTRATION.md` is not edited.** A pre-registration is a record of what
> was committed to beforehand; rewriting it afterwards would destroy the only
> thing it is for, and this workstream may in any case edit only this file. The
> correction belongs here, where the claim was published. A reader comparing the
> two should expect §5's S4 rule to still contain the over-broad wording, and
> should read this block as its withdrawal.

**Does this change the recommendation for this surface? No — and the reason is
an internal contradiction in this report worth stating plainly.** §2.4 and §2.5
already rest on the *opposite* fact. §2.4 proposes *"a code-based classifier
(`E0502` is already the code — S4 shows it in every run)"*; §2.5's one-line
reason is that *"`E0502` in the artifact (measured in all five S4 runs) is a
cheaper branch field than a cross-crate `Assertion` channel"*. Both were written
from the same S4 output as the withdrawn sentence, and both contradict it. So
**DELETE stands, and the true version makes it slightly stronger**: the
replacement classifier the recommendation asks for already exists in the artifact
and does not have to be built. What the over-claim damaged is this report's
standing as a measurement, not the advice it gives.

**The tests that match on the message string.** The grep that stood here was
trimmed like the two above, and its untrimmed output changes a cost number the
recommendation quotes:

```
$ grep -rn 'contains("recursion limit' --include='*.rs' . | grep -v '^\./target' \
    | sed 's|^\./||'                 # grep prepends when rooted at `.`; nothing else is filtered
crates/ply-eval-tests/tests/suite/equivalence_audit.rs:1940:            .contains("recursion limit of 50 nested calls exceeded"),
crates/ply-eval-tests/tests/suite/equivalence_audit.rs:2048:    assert!(machine.message.contains("recursion limit"), "{machine:?}");
crates/ply-eval/src/compiled.rs:1508:            baseline.contains("recursion limit of 8 nested calls exceeded"),
crates/ply-eval/src/tests.rs:313:    assert!(d.message.contains("recursion limit"), "{}", d.message);
crates/ply-eval/src/tests.rs:408:    assert!(message.contains("recursion limit"), "{message}");
crates/ply-eval/src/machine/tests.rs:534:/// > `d.message.contains("recursion limit")`. Deep non-tail recursion is
crates/ply-eval/src/machine/tests.rs:556:        !d.message.contains("recursion limit"),
crates/ply-cli/tests/suite/failure_classification_audit.rs:242:                .contains("recursion limit of 10000 nested calls exceeded"),
crates/ply-cli/tests/suite/failure_classification_audit.rs:590:                .contains("recursion limit of 10000 nested values exceeded"),
crates/ply-codegen-spike/tests/mcts_kernel.rs:550:            said.contains("recursion limit of 10000 nested calls exceeded"),
crates/ply-test/src/tests.rs:1359:        failure.diagnostic.message.contains("recursion limit"),
crates/ply-codegen-spike/tests/hazards.rs:625:        message.contains("recursion limit of 400 nested calls exceeded"),
crates/ply-codegen-spike/tests/hazards.rs:649:            .contains("recursion limit of 10000 nested calls exceeded")),
crates/ply-codegen-spike/tests/hazards.rs:675:            .contains("recursion limit of 600 nested calls exceeded")),
crates/ply-test-tests/tests/suite/hybrid.rs:298:        diagnostic.message.contains("recursion limit"),
crates/ply-codegen-spike/tests/mutations.rs:539:            .contains("recursion limit of 600 nested calls exceeded")),
```

> **Correction in place (2026-08-27): "four tests" is wrong, and this report
> inherited the number instead of counting it.** The block above previously
> printed six line references in four files, folded onto four lines, and the
> sentence introducing it read:
>
> > *"The four tests the catalogue names are doing the only thing available to
> > them"*
>
> **Withdrawn, restated: 16 occurrences in 10 files, in 15 distinct `#[test]`
> functions.** Thirteen of those functions assert the phrase is **present**, one
> (`a_frame_ceiling_that_was_asked_for_is_a_diagnostic_not_a_crash`,
> `machine/tests.rs:556`) asserts it is **absent** — which is equally
> load-bearing on the string — and one occurrence
> (`machine/tests.rs:534`) is a doc comment. **Fourteen tests depend on the
> message text, not four.**
>
> Split by workspace, because §3.1 establishes that `crates/ply-codegen-spike`
> declares its own `[workspace]` and `cargo test --workspace` never reaches it,
> and ADR 0011 says it is thrown away whatever the verdict:
>
> | | occurrences | files | `#[test]` fns | of those, depend on the string |
> | --- | --- | --- | --- | --- |
> | main workspace | 11 | 7 | 11 | 10 (9 present, 1 absent) |
> | `ply-codegen-spike` | 5 | 3 | 4 | 4 |
> | total | 16 | 10 | 15 | 14 |
>
> **The number that should drive the cost is 10 tests in 7 files**, not 14 in 10
> and not four in four: the spike's four are real but sit outside the workspace
> and outside the maintained tree. Both numbers are given so neither has to be
> re-derived. The "four" traces to
> `crates/ply-eval/src/limit.rs:80`'s own correction block, which names four
> *files* (`ply-cli/tests/suite/failure_classification_audit.rs`,
> `ply-test-tests/tests/suite/hybrid.rs`, `ply-test/src/tests.rs`, `ply-eval/src/tests.rs`)
> and misses six more, `crates/ply-codegen-spike`'s three among them. Recorded
> for the next change in §5.
>
> **Effect on the recommendation: direction unchanged, cost larger.** DELETE's
> claim that *"nothing that works today stops working"* is unaffected — all
> sixteen keep matching a message string that DELETE does not touch. What grows
> is §2.5's second half, "give them a code-based assertion beside the string
> one": that is **10 tests in 7 files across three crates in the maintained
> tree** — 14 in 10 across four crates if `ply-codegen-spike` is counted — not
> four tests in four files. Roughly two and a half times the work the report
> implied, and still small. The cost table in §2.5 is corrected accordingly.

### 2.2 What the decision record promised

**ADR 0004**'s example object (`0004:355-362`) shows a populated `assertion`,
and two rows of its table justify it:

| row | what it promises |
| --- | --- |
| `assertion.expected/actual` (`0004:410`) | *"the diff, structured, so it is not parsed out of a rendered message"* |
| `assertion.first_difference.path` (`0004:411`) | *"where to look inside a large value — the field that turns a 400-element list diff into a line number"* |

**`CONTRACTS.md:1382`** promises *"a structured `Assertion` payload alongside the
diagnostic"* as part of what `ply-eval` gains. **`CONTRACTS.md:1334`** lists the
enum: `pub enum AssertionKind { Eq, Bool, Panic, Runtime, UnhandledEffect,
RecursionLimit }`.

**Neither ADR 0004 nor `CONTRACTS.md` carries any correction for this.** ADR 0004's 2026-08-24 correction block covers `causal_slice` and does not mention
`assertion`.

**What a reader believes today:** that `failures[].assertion.kind` is the field to
branch on to tell a recursion blow-up from a comparison failure, and that
`assertion.expected`/`actual` spare them parsing a rendered message. Row 410 says
so in exactly those words. Both are false in every report the binary has written,
and the tests in the tree that parse the rendered message prove it — 10 in the
main workspace, 14 counting `ply-codegen-spike`'s (§2.1; the count read "four"
until it was corrected there).

### 2.3 Option ARM

**Not "construct the variant".** The payload does not exist where `Failure` is
built. `lib.rs:1530` has a `Diagnostic` and nothing else; expected and actual
values live in the evaluator, at the point the assertion failed.

| what | where | note |
| --- | --- | --- |
| build an `Assertion` at the failure sites | `ply-eval`: the `assert_eq` / `assert` builtins, `panic`, `err_recursion_limit` (`crates/ply-eval/src/limit.rs:97`), the unhandled-`perform` path, the deadlock path | seven variants; each site is in the crate that holds the values, which `ply-test` is not |
| carry it out of the evaluator | `Diagnostic` has no slot for it; either a side channel on the engine or a new field | this is the design call `CONTRIBUTING.md` item 14 says was deferred, and it is a **cross-crate** one: `ply-eval` must not depend on `ply-test` |
| structured `expected`/`actual`/`first_difference` | needs a `Value` renderer and a diff walk | `slice.rs:317-320`'s own doc explains why they are strings: *"a faithful serialization of a `Value` would commit this schema to the evaluator's representation"* |
| set the field | `crates/ply-test/src/lib.rs:1530` | one line, last |

**Tests that go red:** three — `crates/ply-cli/tests/suite/cli.rs:1644`,
`crates/ply-cli/src/commands/test.rs:2008` (whose message *"the evaluator carries
no payload yet"* is a direct statement of this gap), and
`crates/ply-test/src/tests.rs:2011`.

**Cheapest honest ARM, worth naming separately:** construct only `kind` and
`message` and leave `expected`/`actual`/`first_difference` `None`. That is
reachable — `err_recursion_limit` and the panic path both know their kind — and it
would make `assertion.kind` the branch field ADR 0004 promises without any
`Value` serialization. It arms rows 410 and 411 **only partially**, so ADR 0004
would still need correcting, and the string-matching tests — 10 in the main
workspace, 14 counting the spike's (§2.1) — would still be the only thing that
actually classifies unless they are rewritten.

### 2.4 Option DELETE

**What goes:** `Assertion`, `AssertionKind`, `Difference` (`slice.rs:271-359`),
`Failure::assertion` (`lib.rs:370`), `assertion_json` (`report.rs:498`), and the
`"assertion"` key in the artifact.

**`SCHEMA_VERSION` bumps 4 → 5** — a field leaves.

**Documents to correct in place:** ADR 0004's example object (`0004:355-362`)
and rows 410–411; `CONTRACTS.md:1334`; `CONTRIBUTING.md` item 14 and its row in
§"The shape it keeps taking"; `crates/ply-eval/src/limit.rs:80-95`, whose
correction block exists only to explain why the phrase "recursion limit" is kept.

**The capability genuinely lost:** structured comparison output. A consumer would
keep matching `diagnostic.message`, which is what all 16 occurrences of it
already do, so
**nothing that works today stops working**. The real loss is that
`err_recursion_limit`'s message text becomes load-bearing API by default rather
than by decision — `limit.rs:80`'s doc already says *"the phrase is load-bearing
only for whatever matches on the string, which is four tests"* (quoted as
written; that count is itself wrong, see §2.1 and §5), and DELETE makes
that permanent. A DELETE should therefore be accompanied by making that explicit:
either a code-based classifier (`E0502` is already the code — S4 shows it in every
run) or a documented statement that the message string is the contract.

**Delete only `RecursionLimit` — the narrow reading of item 14 — is the one option
to reject outright.** It removes a variant from an enum nothing constructs, leaves
`Assertion` dead, and closes a catalogue item without changing what any report
carries. That is a green result over unexplored space in documentation form.

### 2.5 Recommendation

**DELETE `Assertion`, `AssertionKind` and `Difference`; keep the
string-matching tests as the standing classifier — 10 of them in the maintained
tree, 14 counting `ply-codegen-spike`'s — and give them a code-based assertion
beside the string one.**

*One-line reason:* the payload has never existed, the tests that need the
distinction already have a working one, and `E0502` in the artifact
(measured in all five S4 runs, on all three engines, and asserted already by
`failure_classification_audit.rs:231`) is a cheaper branch field than a
cross-crate `Assertion` channel that ADR 0004 specifies and nothing has ever
built.

**Rough cost each way.**

| | files touched | tests red | documents | note |
| --- | --- | --- | --- | --- |
| ARM, kind + message only | 4-5 (`ply-eval` failure sites, a channel, `ply-test/src/lib.rs`, `slice.rs`) | 3 | ADR 0004 rows 410-411 still need correcting — partially armed is still wrong | the honest minimum |
| ARM, full payload | + a `Value` renderer and a structural diff in `ply-eval` | 3 | rows 410-411 corrected by being made true | the largest of the three; ADR 0004 is the only design |
| DELETE | 3 (`slice.rs`, `lib.rs`, `report.rs`) | 3, deleted | 5 sections in 3 documents | `SCHEMA_VERSION` 4 → 5; pairs naturally with §1's delete, one bump for both |
| the code-based assertion DELETE should carry with it | 7 test files in 3 crates (`ply-cli`, `ply-eval`, `ply-test`) in the main workspace; 3 more in `ply-codegen-spike`, which is its own workspace and is to be thrown away (§3.1, ADR 0011) | 0 — the additions are assertions beside existing passing ones | none | **corrected 2026-08-27**: this row said "four tests" before §2.1's third correction; it is 10 tests in 7 files in the maintained tree, 14 in 10 counting the spike. Purely additive, and the CLI-level ones can assert `E0501` vs `E0502` directly, which `failure_classification_audit.rs:231` already does |

> **Correction in place (2026-08-27).** The row above is new, and the two
> sentences it replaces in §2.5's headline and one-line reason read *"keep the
> **four** string-matching tests as the standing classifier"* and *"the **four**
> tests that need the distinction already have a working one"*. Both counts came
> from `limit.rs:80` rather than from a grep run here. Corrected throughout §2 to
> 10 tests in the maintained tree, 14 counting the spike's; see §2.1. The direction of the recommendation is unchanged — DELETE is
> still cheapest and still breaks nothing — but the accompanying work is roughly
> three times the size this table implied.

**Sequencing note.** If §1 and §2 are both actioned, do them in one change:
`SCHEMA_VERSION` should move 4 → 5 once, not twice, and both surfaces live in
`slice.rs` and `report.rs`.

---

## 3. The compiled seam

*Catalogue item 13, first bullet. The largest of the three.*

> **Overtaken by events, twice, and left standing as a dated survey — 2026-08-31.**
> This section was a survey of a seam **nothing shipping implemented**, and every
> option it costs is written from that position. Two changes since have moved
> the ground under it, and this note is here so that a reader arriving from the
> table of contents does not cost an ARM that has already been paid for.
>
> * **2026-08-28.** `ply_eval::backend::Reference` — a tree-walking backend —
>   and `ply test --backend`. ADR 0026 and §4.6.
> * **2026-08-31.** `crates/ply-codegen` — a **cranelift JIT** in the shipping
>   workspace, `ply test --backend cranelift`, 31 cranelift packages in
>   `Cargo.lock`, no feature flag and no second toolchain. ADR 0026 and
>   §4.9.
>
> Three specific rows below are now false and are named rather than edited,
> because this document's own header says it corrects no other file and the same
> discipline applies to correcting itself out of a survey into a status page:
>
> * §3.1's *"the only two real implementors are in the spike, which does not
>   build on this tree's toolchain (cranelift 0.134.3 needs rustc 1.94.0)"* —
>   there are two shipping implementors, and the spike builds on 1.93.1 since
>   it moved to cranelift 0.132.3.
> * §3.3's *"the result-cache rule, armed | unwritten"* — it is written, in both
>   of ADR 0026's stages, and each has been watched to fail.
> * §3.3's *"**Tests that would go red:** none, directly — which is itself the
>   finding. There is no test asserting the CLI *cannot* attach a backend"* —
>   there are 28, in `crates/ply-cli/tests/suite/backend.rs`, asserting what happens
>   when it does.
>
> §3.4's DELETE-A costing is the part that survives intact and is still the best
> account of what `rm -r crates/ply-codegen-spike` costs; ADR 0026 records
> the deletion condition as met and names the two open findings that hold it.

### 3.1 What is actually there

**The headline claim holds exactly, and it is the strongest evidence in this
report because it is an empty grep with a wide net:**

```
$ grep -rn 'Compiled\|set_compiled' crates/ply-cli | wc -l
0
```

Zero occurrences of either token anywhere in `crates/ply-cli` — source, tests,
every file. The shipping CLI has no route to a backend.

**The trait has no production implementor either:**

```
$ grep -rn 'impl .*Compiled for ' --include='*.rs' . | grep -v '^\./target'
crates/ply-eval/src/compiled.rs:658:    impl Compiled for Double {          # inside #[cfg(test)] (boundary at :578)
crates/ply-eval-tests/tests/suite/differential_corpus.rs:352:    impl Compiled for Declining {
crates/ply-eval-tests/tests/suite/differential_corpus.rs:394:    impl Compiled for TreeWalker {
crates/ply-eval-tests/tests/suite/equivalence_audit.rs:1775:    impl Compiled for Budgeted {
crates/ply-codegen-spike/src/wrong.rs:275:impl Compiled for Mutant {
crates/ply-codegen-spike/src/entry.rs:346:impl ply_eval::Compiled for SpikeBodies {
```

Six implementors: four in tests, two in the out-of-workspace spike. This agrees
with `compiled.rs:174`'s own line — *"No implementation of [`Compiled`] exists in
this workspace."*

**The `set_compiled` inventory in two documents is stale.** S3, three identical
runs:

```
set_compiled lines in .rs (incl. decl + doc comments)   48
  ... minus doc-comment lines                           43
  ... minus the declaration (machine.rs:606)            42   <- CALLS
files containing a set_compiled CALL                     6
```

| file | calls | what it is |
| --- | --- | --- |
| `crates/ply-eval/src/compiled.rs` | 27 | its own `#[cfg(test)]` module |
| `crates/ply-codegen-spike/tests/hazards.rs` | 5 | the spike |
| `crates/ply-eval-tests/tests/suite/differential_corpus.rs` | 3 | `ply-eval`'s own test |
| `crates/ply-eval-tests/tests/suite/equivalence_audit.rs` | 3 | `ply-eval`'s own test |
| `crates/ply-codegen-spike/tests/mutations.rs` | 2 | the spike |
| `crates/ply-codegen-spike/src/measure.rs` | 2 | the spike's harness |
| | **42** | across **6** files |

> **Correcting `CONTRIBUTING.md:1530-1533` and
> `crates/ply-eval/src/compiled.rs:201-205`, which carry the same sentence.**
> Both read: *"all five `set_compiled` call sites in the workspace are tests or
> the spike's own harness (2 in `ply-codegen-spike/src/measure.rs`, 5 in its
> `hazards.rs`, 3 in its `mutations.rs`, 27 in `ply-eval/src/compiled.rs`'s own
> tests, 2 in `ply-eval-tests/tests/suite/differential_corpus.rs`)."* That enumerates
> 2+5+3+27+2 = **39 across 5 files**. Actual: **42 across 6**. `mutations.rs` has
> 2 calls and a doc comment, not 3; `differential_corpus.rs` has 3, not 2; and
> `crates/ply-eval-tests/tests/suite/equivalence_audit.rs`'s 3 are counted by neither
> document — they were added by the items 9/10 fix. **The substantive claim
> survives untouched:** every one of the 42 is a test or the spike's harness, and
> `crates/ply-cli` contains zero. Handed on, not fixed here.

**What the seam costs and what it buys, from the record.** ADR 0011's
correction block records the cost: *"a public `Compiled` trait,
`Machine::set_compiled`, three counters on `Machine`, and a branch in
`Machine::enter_code` taken on every interpreted call — none of it with a shipping
implementor or caller, all of it surviving the `rm -r`. … It costs 0.0
allocations per `/health` request … and 237.87 predictable branch tests, and it
buys nothing that ships."* `crates/ply-eval/src/compiled.rs` is 2,063 lines, of
which **578 are production** and the rest is the `#[cfg(test)]` module beginning
at `:578`.

**The spike itself:**

```
$ find crates/ply-codegen-spike -name '*.rs' | xargs wc -l | tail -1
   10083 total
$ head -12 crates/ply-codegen-spike/Cargo.toml
# Its own workspace on purpose. …
[workspace]
$ grep -n 'codegen-spike\|1.94' .github/workflows/ci.yml
439:    name: crates/ply-codegen-spike
454:          toolchain: "1.94.0"
```

10,083 lines of Rust, its own `[workspace]` (so `--workspace` never reaches it),
five non-optional cranelift dependencies at `0.134.3`, and a dedicated CI job
pinned to rustc 1.94.0 because cranelift requires it.

### 3.2 What the decision record promised, and what it forbids

**ADR 0011 "What the spike may not do"** (`0016:564-599`) — three sentences
that bear directly on any recommendation here:

> It may **not** be wired into `ply run`, `ply test` or any other command.

> It may **not** be kept because it works. It is thrown away whatever the verdict;
> an `Advance` schedules M9, and M9 is a milestone with an ADR, not a promotion of
> a spike.

And its own in-place correction (`0016:574-590`), which is the honest half:

> **Corrected in place (R5 review, 2026-08-22): the last clause is no longer true,
> and R5 is what made it untrue.** … What is false is *"nothing else in the
> workspace knows it existed"*. After R5, `crates/ply-eval` carries `compiled.rs`
> … all of it surviving the `rm -r`. That is a deliberate change made by R5 under
> ADR 0018's "make the interpreter able to enter compiled code", and no ADR
> recorded the amendment until this block.

**ADR 0011** (`0016:1309-1315`) records the deletion as still owed:

> **One obligation is outstanding.** §3.5 and "Not in W6" require that the spike be
> deleted when W6 closes. It has not been … so `rm -r crates/ply-codegen-spike` is
> the whole deletion. Its measurements survive in `benches/w6-spike.json`, which is
> what the decision reads.

**`ROADMAP.md`'s "What is next" item 3**, in the R5 audit note (`ROADMAP:1900-1909`):

> **This does not re-open M9 and nothing here argues that it does.** The 6.199× is
> measured at a seam **no shipping command can reach** … What this item now owes is
> not another ratio. It is a decision about whether a backend is ever reachable
> from a shipping command, which is M9 with an ADR, and ADR 0011 still
> requires the spike be deleted rather than promoted.

**What a reader believes today.** From ADR 0011 unaugmented: that deferring
M9 costs `rm -r` and one dependency line, and that nothing in the workspace knows
the spike existed. That is corrected in place — a reader who reads the block
learns the truth. From `compiled.rs`'s own §"What polices this seam": that the
seam is policed by 36 + 6 + 13 + 25 tests. **That is true and it is also the
trap** — those tests all install a backend themselves. What no document says
plainly outside item 13 is that **the entire policed surface is unreachable from
every command a user can type**, so the 6.199× end-to-end speedup R5 measured is
available to nobody, the shipping CLI catches zero of the eight deliberately wrong
backends in `crates/ply-codegen-spike/tests/mutations.rs`, and the rule that a
backend run must not populate the result cache is unenforced *because it is
unreachable*.

### 3.3 Option ARM

ARM here means: wire a backend into `ply-cli` so `set_compiled` has a shipping
caller.

**Three documents forbid it without an ADR first, and this report is not
overruling them.** ADR 0011 says a backend *"may **not** be wired into `ply
run`, `ply test` or any other command"* and that the spike *"may **not** be kept
because it works"*; ADR 0011 records the deletion as owed; `ROADMAP.md`'s item
3 says what this owes *"is a decision about whether a backend is ever reachable
from a shipping command, which is M9 with an ADR"*. **A recommendation to wire the
seam is a recommendation to do something the record forbids until an ADR says
otherwise. The 6.199× is a reason to write that ADR, not a licence to skip it.**

**What it would cost, if the ADR were written:**

| what | where | risk |
| --- | --- | --- |
| a backend the CLI can construct | nothing in `crates/*` implements `Compiled`; the only two real implementors are in the spike, which does not build on this tree's toolchain (`ROADMAP:1145-1147`: cranelift 0.134.3 needs rustc 1.94.0, 1.93.1 is installed) | the ARM's first step is promoting or rewriting 10,083 lines the record says to delete |
| the result-cache rule, armed | unwritten; `ply-store`'s cache is keyed on `(RUNTIME_VERSION, DefHash)` (`crates/ply-store/src/lib.rs:2`) | a third execution strategy whose results must not be kept. Getting this wrong is a **silent** wrong answer, cached — this project's signature defect in its worst form |
| a flag, and its interaction with `--engine` | `crates/ply-cli/src/cli.rs` | `--engine` already documents *"Anything but the default neither reads nor writes the result cache"* (`cli.rs:379-381`), which is the shape of the rule and is precedent for it |
| entry-point defect | `CONTRIBUTING.md` item 9 gates this per `compiled.rs:210-213` | named as a blocker by the module's own doc |

**Tests that would go red:** none, directly — which is itself the finding. There
is no test asserting the CLI *cannot* attach a backend, so wiring one would break
nothing and would be caught by nothing. The 36 + 6 + 13 + 25 tests
`compiled.rs:183-189` counts all install backends themselves and would keep
passing whatever the CLI did.

### 3.4 Option DELETE

**Two different deletions hide under one word, and conflating them is the mistake
this section exists to prevent.**

**DELETE-A: `rm -r crates/ply-codegen-spike`.** This is what ADR 0011 and §11
require. R5 verified it leaves the workspace green **by performing it** — ADR 0011's correction records `cargo build --workspace --all-targets` and `cargo
test --workspace --no-fail-fast` green at 155 test binaries, 3,680 passed, 0
failed, and `grep -c cranelift Cargo.lock` at 0. Confirmed structurally here: the
root `Cargo.toml` `members` list does not name it, and the crate carries its own
`[workspace]`.

- **What goes:** 10,083 lines, five cranelift dependencies, one CI job at 1.94.0.
- **What does NOT go:** `crates/ply-eval/src/compiled.rs` — 578 production lines
  plus ~1,485 of tests — `Machine::set_compiled`, the three counters, and the
  `enter_code` branch. ADR 0011's own correction says exactly this: *"all of
  it surviving the `rm -r`."* **DELETE-A does not remove the dead seam. It removes
  the only two real implementors of it.**
- **Capability lost:** the ability to re-take §9.1's and §9.2's numbers, which
  `ROADMAP:1140-1144` records as already unavailable in `benches/w6-spike.json`
  and which `ROADMAP:1145-1147` records as already un-re-takeable on this
  toolchain. So the loss is thinner than it looks — but it is not nothing, and it
  is exactly what ADR 0011 means by *"Its measurements survive in
  `benches/w6-spike.json`, which is what the decision reads."*

**DELETE-B: remove the seam too** — `compiled.rs`, `Machine::set_compiled`, the
counters, the `enter_code` branch.

- **What goes:** 2,063 lines in `ply-eval`, plus the 6 tests in
  `differential_corpus.rs` and 3 in `equivalence_audit.rs` that install backends.
- **Capability lost:** the *measured* result R5 obtained — 6.199× end to end with
  2,162 native entries against 0.998× with zero — becomes un-reproducible, and
  ADR 0018's prerequisite ("make the interpreter able to enter compiled code")
  is un-built. That is a real loss and it is upstream of any future M9.
- **`RUNTIME_VERSION` question:** removing the `enter_code` branch changes no
  observable evaluation result on a machine with no backend (`compiled: None` at
  `machine.rs:345` on every machine the CLI builds), so on the face of it no bump
  is needed. **Stated as a question, not an answer** — house rule 9 is a rule
  about evaluation semantics, and whoever does this must decide it deliberately.

### 3.5 The third option the brief's framing hides

**DELETE-A and keep the seam, deliberately.** That is what the tree does *today*,
by accident: the spike is present but un-buildable on this toolchain, the seam is
present and unreachable, and no document ratifies either state. An ADR could
ratify it on purpose — the seam is retained as ADR 0018's prerequisite, with a
written statement that it is unreachable from any command, that the result-cache
rule is therefore vacuous, and that arming it is M9's first step. That converts a
dead surface into a **documented, costed deferral**, which is the difference this
whole report is about.

It also has a concrete effect nothing else here does: it removes the seam from
this catalogue. A surface that is unreachable *and says so in an ADR* is not
"declared, registered, raised nowhere"; it is a deferral.

### 3.6 Recommendation

**DELETE-A now (`rm -r crates/ply-codegen-spike`, which three documents already
require and R5 already verified), and keep the seam under a short ADR that states
it is unreachable and why — the third option, not DELETE-B and not ARM.**

*One-line reason:* the deletion is an outstanding obligation the record names
twice and a reviewer has already executed successfully, while the seam is the
only built artefact of ADR 0018's prerequisite and deleting it would throw away
the one measured 6.199× that a future M9 ADR has to argue from.

**Rough cost each way.**

| | files touched | tests red | documents | note |
| --- | --- | --- | --- | --- |
| ARM (wire a backend) | `ply-cli` + a backend implementor + the cache rule | 0 red, **which is the problem** | needs a **new ADR** (M9) before any of it; ADR 0011 and ROADMAP item 3 forbid it otherwise | largest by far; the cache rule is a silent-wrong-answer risk |
| DELETE-A | `rm -r` one directory; one CI job; `CONTRIBUTING.md` item 1 | 0 — verified by R5 performing it | ADR 0011 closes; `ROADMAP.md`'s "two smaller obligations" loses one; `benches/README.md`'s `+1.94.0` commands | ~15 minutes of work that has been owed since W6 closed |
| DELETE-B (also the seam) | `compiled.rs`, `machine.rs`, 2 test files in `ply-eval` | 9 backend tests deleted, 36 `compiled::` tests deleted | + ADR 0018's prerequisite becomes un-built; ADR 0011's correction needs a further correction | throws away R5's only reachable result |
| **DELETE-A + ratify the seam** | `rm -r`, one CI job, **one new short ADR** | 0 | ADR 0011 closes; the seam leaves this catalogue by being documented rather than by being armed | **recommended** |

**What this recommendation is not.** It is not a claim that M9 should advance;
`ROADMAP.md` is explicit that *"every cheaper lever that lands makes M9's case
weaker"*. It is not a recommendation to promote the spike, which ADR 0011
forbids in those words. It is the smallest action that closes an obligation and
stops a dead surface reading like a live feature.

---

## 4. Is the pattern still spreading? A gate specification

### 4.1 The recommended check does not catch the known instance

`CONTRIBUTING.md:57-63` recommends:

> The check that finds one takes a minute: `grep -rn '<TypeName>::<Variant>'` or
> `grep -rn '<ConstructorFn>' --include=*.rs`, and then read the hits for one that
> is not a test and not the declaration.

Applied to all 83 registered diagnostic codes at file granularity — "does any
non-test, non-declaration **file** mention this code?" — it returns **zero**:

```
$ python3 -c "…for each of the 83 codes, grep -rl, drop tests/ and the declaration…"
codes with NO non-test, non-declaration FILE mentioning them: 0
```

**Zero, over a population that provably contains the catalogue's own example.**
E0435 survives the check because of this:

```
$ grep -rn 'codes::DB_SCHEMA_MISMATCH' --include='*.rs' crates
crates/ply-span/src/lib.rs:787:            ("DB_SCHEMA_MISMATCH", codes::DB_SCHEMA_MISMATCH, "E0435"),
crates/ply-eval/src/host.rs:1106:    codes::DB_SCHEMA_MISMATCH,
```

`crates/ply-eval/src/host.rs` is a production file, it is not a test, it is not
the declaration, and the occurrence is an element of the `RESERVED_CODES` array
(`host.rs:1083-1114`, 22 entries) — a **listing**, which exists precisely so no handler can
impersonate the code. A reader following the recommendation finds a hit in
production code and moves on.

**The defect in the recommendation is not the grep. It is the granularity and the
missing role.** An occurrence of a name can play at least five roles and only one
of them arms it.

### 4.2 The specification

A check that catches E0435 must do six things. Each is stated as a requirement,
with the reason it is there.

1. **Enumerate the population mechanically.** Parse `pub const NAME: &str =
   "Exxxx";` out of `crates/ply-span/src/lib.rs`'s `codes` module. Never hand-list
   the population — a hand-list is the same defect one level up.

2. **Count at SITE granularity, never file granularity.** E0435 is the proof: two
   files mention it, zero sites construct it.

3. **Classify every site by the role it plays, and count only constructions.**
   Five roles suffice for this population:

   | role | how it is recognised |
   | --- | --- |
   | `declaration` | inside `pub mod codes` in `ply-span/src/lib.rs` |
   | `test` | in a file reachable from a crate root **only** through a `#[cfg(test)] mod`, or under `tests/` / `benches/`, or inside a `#[cfg(test)]` item |
   | `listing` | the nearest unclosed bracket is `[` — an array or slice literal |
   | `call:<f>` | the first argument of a call to something **not** in the constructor set |
   | `construction` | the first argument of a call to a member of the constructor set |

   The `call:<f>` role is load-bearing: it makes a **missing constructor visible**
   rather than silently reclassifying the site as "not a construction". §4.4's
   E0002 demonstration is what that buys.

4. **Enumerate the constructor set; do not assume one spelling.** The check must
   ship a mode that prints every callee taking a code as its first argument, so
   the set is derived from the tree:

   ```
   $ python3 unarmed_gate.py . --callees
   callees taking a `codes::NAME` as their FIRST argument, outside tests and the declaration:
      335  Diagnostic::error     <- in CONSTRUCTORS
       57  Diagnostic::warning   <- in CONSTRUCTORS
       16  self.error            <- in CONSTRUCTORS
   ```

   Three, not one. `self.error` is `crates/ply-syntax/src/lexer.rs:810`, a
   crate-local wrapper, and it is the **only** builder of E0002
   UNTERMINATED_STRING (at `lexer.rs:544`, `:576`, `:606`, `:638`). §4.4 shows what
   a check that knows only `Diagnostic::error` reports.

5. **Require a positive control before any zero is believed.** Five hand-verified
   production constructions, none of which may appear in the output. This is not
   ceremony: §0's void result was stable across three runs and wrong, and it is
   the positive control — not a third run — that would have caught it. A negative
   control runs beside it: E0435 must appear, or the check does not do the one job
   it was written for. **Both are printed with every number, including the runs
   that pass.**

6. **Carry an explicit allow-list, and fail on anything outside it.** An unarmed
   code that is on the list is a known, recorded gap. An unarmed code that is not
   is a **new** instance and must exit non-zero. Without this the check reports a
   growing number and a growing number is not a gate.

**Exit codes:** `0` pass, `1` a new unarmed code, `2` a control failed — and `2`
must be distinguishable from `1`, because a failed control means the number is
void rather than bad.

### 4.3 The check, run against this worktree

Three identical runs (`md5` of the output identical across all three), load 14.20
before and after:

```
$ python3 unarmed_gate.py .
registered code constants                83
sites classified                         1404
codes with >=1 production construction   81
REGISTERED, NEVER CONSTRUCTED            2

POSITIVE control — these 5 must NOT be listed: PASS
    OK E0102 UNKNOWN_TYPE          expected crates/ply-core/src/infer.rs:1545     first construction crates/ply-core/src/infer.rs:1545
    OK E0109 MODULE_CYCLE          expected crates/ply-syntax/src/resolve.rs:805  first construction crates/ply-syntax/src/resolve.rs:805
    OK E0203 OCCURS_CHECK          expected crates/ply-core/src/infer.rs:5410     first construction crates/ply-core/src/infer.rs:5410
    OK E0437 DB_POOL_EXHAUSTED     expected crates/ply-host/src/db/pool.rs:1490   first construction crates/ply-host/src/db/pool.rs:1490
    OK E0443 ARTIFACT_INVALID      expected crates/ply-cli/src/artifact.rs:1179   first construction crates/ply-cli/src/artifact.rs:1179
NEGATIVE control — E0435 must be listed:       PASS

unarmed codes, every site and the role it plays:
  E0435  DB_SCHEMA_MISMATCH   [allow-listed]
      declaration              crates/ply-span/src/lib.rs:414
      listing                  crates/ply-eval/src/host.rs:1106
      test                     crates/ply-span/src/lib.rs:787
  E0438  DB_UNMODELLED_SIDE_EFFECT   [allow-listed]
      declaration              crates/ply-span/src/lib.rs:434
      listing                  crates/ply-eval/src/host.rs:1107
      test                     crates/ply-span/src/lib.rs:792

GATE: pass — controls hold; every unarmed code is allow-listed.
```

Role histogram over all 1,404 sites: 876 `test`, 408 `construction`, 83
`declaration`, 22 `listing`, 15 `other`. All 15 `other` sites were read by hand:
they are comparisons (`d.code == codes::X` at `ply-test/src/lib.rs:1791`,
`:1808-1810`, `ply-eval/src/differential.rs:683`, `ply-eval/src/host.rs:462`,
`ply-core/src/infer.rs:3571`, `ply-cli/src/migrate.rs:49`), format-string
interpolations (`ply-cli/src/commands/run.rs:365`, `ply-eval/src/host.rs:1145`)
and **two re-tags** — `d.code = codes::INTERNAL_ERROR`
(`ply-core/src/infer.rs:3580`) and `diagnostic.code = codes::RUNTIME_ERROR`
(`ply-eval/src/host.rs:1142`). The re-tags are a sixth role a stricter version
should name: a code assigned to an already-built diagnostic is arguably a
construction of the *reported* code. Both codes involved are conventionally
constructed elsewhere, so it changes no answer here — recorded so the next person
does not have to rediscover it.

**The second finding, and it must be stated narrowly.** The check returns E0438
DB_UNMODELLED_SIDE_EFFECT with the identical E0435 shape — declaration, registry
row, `RESERVED_CODES` entry, plus a fixture at
`tests/fixtures/db_unmodelled_side_effect.ply` — and E0438 is **absent** from
`CONTRIBUTING.md` §"The shape it keeps taking", whose table has five rows.
**This is not a new defect.** `ROADMAP.md:861-870` already documents it, dated
2026-08-17: *"That check was never built. `E0438` exists as a registered code and
as a `RESERVED_CODES` entry so no handler can impersonate it, and it is raised
nowhere."* The narrow, true statement is: **the catalogue's table is missing one
instance the repository already knows about, so its count of five is at least
six.** Overstating this as a discovery would be the same species of error the
table exists to catch.

### 4.4 Seen to fail — four ways, none of them on this worktree

House rule 5: a passing check is vacuous until it has been seen to fail. All four
demonstrations were run against a **copy** of `crates/` in the scratchpad. The
worktree was never modified.

**FAIL-1 — a newly registered code that nothing constructs.** Added
`pub const NEVER_BUILT: &str = "E0999";` to the copy's `codes` module:

```
  E0999  NEVER_BUILT   [*** NEW ***]
      declaration              crates/ply-span/src/lib.rs:207
GATE: FAIL — unarmed and not allow-listed: ['E0999']
exit 1
```

**FAIL-2 — the E0435 shape itself, introduced.** Took E0426
HOST_CONTINUATION_RESUMED, which has exactly one construction, and changed that
one site (`crates/ply-eval/src/machine.rs:2754`) to name a different code —
leaving the declaration, the registry row, the `RESERVED_CODES` entry and
**twelve test sites** intact:

```
  E0426  HOST_CONTINUATION_RESUMED   [*** NEW ***]
      declaration   crates/ply-span/src/lib.rs:332
      listing       crates/ply-eval/src/host.rs:1096
      test          crates/ply-cli/tests/suite/w5_trace_audit.rs:310,
                    crates/ply-eval-tests/tests/suite/host_boundary.rs:316,
                    crates/ply-eval-tests/tests/suite/host_linearity_audit.rs:218, :318, :366, :436, :475, :599, :640,
                    crates/ply-eval-tests/tests/suite/transaction_scope_audit.rs:364, :391,
                    crates/ply-span/src/lib.rs:760
GATE: FAIL — unarmed and not allow-listed: ['E0426']
exit 1
```

This is the important one. Twelve test sites and a production listing means the
recommended file-granularity grep sees a healthy code. The site-and-role check
sees zero constructions.

**VOID — the instrument's own historic defect, caught by its control.** Swapped
`strip_code` for S2 v1's regex (`re.sub(r'r#*"(?:.|\n)*?"#*', '""', src)`) and
changed nothing else:

```
registered code constants                83
sites classified                         1010          <- 394 sites vanished
REGISTERED, NEVER CONSTRUCTED            24            <- the historic wrong number
POSITIVE control — these 5 must NOT be listed: FAIL ['E0102', 'E0109', 'E0203', 'E0437', 'E0443']
    ?? E0203 OCCURS_CHECK   expected crates/ply-core/src/infer.rs:5410   NO CONSTRUCTION FOUND
NEGATIVE control — E0435 must be listed:       PASS
GATE: VOID — a control failed. The count above is not evidence.
exit 2
```

24 against the historic 25, and **all five positive controls fire**. Note that the
*negative* control still passes — E0435 is still reported — so a check with only a
negative control would have reported 24 as a triumph. This is the concrete
argument for requirement 5.

**CONSTRUCTOR-SET — the assumption a reader would make.** Narrowed `CONSTRUCTORS`
to `{Diagnostic::error, Diagnostic::warning}`, the set anyone would write without
running `--callees`:

```
REGISTERED, NEVER CONSTRUCTED            3
POSITIVE control — these 5 must NOT be listed: PASS      <- the controls do NOT catch this
  E0002  UNTERMINATED_STRING   [*** NEW ***]
      call:self.error   crates/ply-syntax/src/lexer.rs:544, :576, :606, :638
      declaration       crates/ply-span/src/lib.rs:207
      test              crates/ply-span/src/lib.rs:695, crates/ply-syntax/src/lexer.rs:913, :992,
                        crates/ply-syntax/src/tests.rs:837
GATE: FAIL — unarmed and not allow-listed: ['E0002']
exit 1
```

**A false positive, and the five controls do not catch it** — none of them is
lexed. What catches it is the `call:self.error` role, which puts the four real
construction sites and their callee on screen. That is why requirement 3 lists the
`call:<f>` role separately instead of folding it into "not a construction", and
why requirement 4 exists at all.

### 4.5 What is handed to the gate workstream

- The specification above: requirements 1–6, the five roles, the three exit codes.
- A working reference implementation, `unarmed_gate.py`, with `--callees` and
  `--sites` modes, held in this session's scratchpad rather than in the
  repository (this workstream may create one file, and it is this one). It is
  ~260 lines of Python with no dependencies; the gate workstream should re-derive
  rather than inherit it, using §4.4's four demonstrations as its acceptance
  tests.
- **The generalisation, which matters more than the script.** This population
  happens to be diagnostic codes. The three surfaces in sections 1–3 are the same
  shape over different populations: `AssertionKind` variants, `Event` variants,
  `impl Compiled for`. The specification transposes directly — enumerate the
  population from its declaration, count sites, classify by role, enumerate the
  constructor set, control before believing a zero, allow-list the known. A gate
  built only for `codes::` would leave the other three unwatched. **A concrete
  first transposition, with the answer already known from this report:** run it
  over `AssertionKind`'s seven variants (expect: 1 of 7 constructed, `Eq`, at
  `slice.rs:345`) and over `Event`'s three (expect: 1 of 3 constructed outside
  tests — none). Both should be red today, and a gate that reports them green is
  broken.

---

## 5. Stale claims found outside the three surfaces, handed on

Every one is a small fix and every one is **out of scope for this workstream**,
which may create one file and change nothing else. Recorded with coordinates so
the next change does not have to rediscover them.

1. **The +19 line drift**, §2.1: `CONTRIBUTING.md:57` (the §"The shape it keeps
   taking" table row), `CONTRIBUTING.md:1603` and `:1606`, and
   `crates/ply-eval/src/limit.rs:85` all cite `slice.rs:268` / `:284` / `:326`.
   Actual: `:287` / `:303` / `:345`.

2. **The "five `set_compiled` call sites" count**, §3.1: `CONTRIBUTING.md:1530-1533`
   and `crates/ply-eval/src/compiled.rs:201-205` both enumerate 39 across 5 files.
   Actual: 42 across 6; `crates/ply-eval-tests/tests/suite/equivalence_audit.rs`'s 3 are in
   neither.

3. **`SliceBuilder` "constructed in exactly one place"**, §1.1:
   `CONTRIBUTING.md:1621-1624`. Actual: seven sites in two files, none in
   production outside `slice.rs`'s own `Default` impl.

4. **ADR 0004's correction says "four rows"**, §1.2:
   `docs/adr/0004-machine-shaped-failure.md` §7. Nine of the rows describe a
   field that is null in every report the binary writes.

5. **`CONTRIBUTING.md`'s closing summary of item 12 contradicts item 12's own
   in-place correction.** The summary bullet at `CONTRIBUTING.md:1684-1686` is
   live text and reads *"**12 is fixed (2026-08-24).** `Ctx::begin` no longer
   walks the previous entry's arena; `Ctx::end` clears it at the end of the entry
   that filled it, and the shrink is amortized over `SHRINK_EVERY` entries."*
   Item 12's own correction, at `CONTRIBUTING.md:1429-1432`, withdraws exactly
   that clause: *"The paragraph above read 'and the shrink is amortized over
   `SHRINK_EVERY` = 64 entries instead of running per entry' … **The window is
   gone.**"* The summary was not updated with the item.

6. **`CONTRACTS.md:7368-7369` and `:7375` describe a `ply test` flag set that does
   not exist.** They list `ply test [...] --trace <..> --trace-level <..>` and say
   `--trace` *"defaults to … `off` under `ply test`"*. Checked against the binary:

   ```
   $ ./target/debug/ply test --help | grep -A3 trace
         --trace <WHEN>
             Record which definitions a failing test actually entered. `auto`
             traces the re-run of a failure; `always` traces the first execution too
             [default: auto]
   ```

   `TestArgs` (`crates/ply-cli/src/cli.rs:374-377`) carries `trace: When` and no
   `TraceOptions`; there is no `--trace-level` on `ply test`. This is a *different*
   `--trace` from `ply run`'s, and CONTRACTS conflates the two. It also means §1's
   DELETE would remove a flag CONTRACTS already describes incorrectly.

7. **`CONTRIBUTING.md` §"The shape it keeps taking" counts five instances and the
   population is at least six**, §4.3: E0438 is missing from the table and is
   already documented at `ROADMAP.md:861-870`.

8. **`crates/ply-eval/src/limit.rs:80`'s correction block says four tests match
   on the "recursion limit" string; 15 `#[test]` functions in 10 files contain
   the phrase and 14 depend on it**, §2.1 (added 2026-08-27). Its text — *"the phrase is load-bearing only for
   whatever matches on the string, which is four tests
   (`ply-cli/tests/suite/failure_classification_audit.rs`, `ply-test-tests/tests/suite/hybrid.rs`,
   `ply-test/src/tests.rs`, `ply-eval/src/tests.rs`)"* — names four files.
   Actual: `grep -rn 'contains("recursion limit' --include='*.rs' .` finds **16
   occurrences in 10 files, in 15 distinct `#[test]` functions**; 13 assert the
   phrase is present, one asserts it is absent, one is a doc comment. The six
   files it misses are `ply-eval-tests/tests/suite/equivalence_audit.rs`,
   `ply-eval/src/compiled.rs`, `ply-eval/src/machine/tests.rs`, and
   `ply-codegen-spike/tests/{hazards,mcts_kernel,mutations}.rs`. This is a
   correction block that is itself stale — the same second-order shape as item 1
   above, where a correction carried a coordinate the correction next to it had
   invalidated. Whoever fixes `limit.rs:80` should note that it is quoted, as
   written, at §2.4 of this report.

9. **`CONTRIBUTING.md`'s instrument-freshness recipe cannot print nothing as
   written** (added 2026-08-27, and it is `r2-instrument`'s to fix, recorded here
   because this report leans on it). The recipe that replaces the `include_str!`-
   blind `find crates -name '*.rs' -newer target/release/ply` is written
   `find crates -name '*.rs' -o -name '*.ply' -newer target/release/ply`. `find`
   binds that as `(-name '*.rs') OR (-name '*.ply' AND -newer BIN)`, so the
   `-newer` test never applies to the `.rs` arm and the command lists every `.rs`
   file in the tree on every run, fresh binary or not — a check that can only
   ever look failed, which is the mirror image of a check that can only ever look
   green. The parenthesised form is the one that works:

   ```
   $ find crates \( -name '*.rs' -o -name '*.ply' \) -newer target/debug/ply
   $                                    # silent — .rs and .ply both current
   ```

   Run in this worktree before every series in this document's round-2 pass; see
   §0.

---

## Appendix — every command in this report, and where its output lives

| § | command | runs | result |
| --- | --- | --- | --- |
| 0 | `find crates -name '*.rs' -newer target/debug/ply` | 2 | silent both times |
| 1.1 | `ply test tests/fixtures/assertion_failed.ply --json --no-cache --trace {never,auto,always}` | 15 | `causal_slice`, `assertion`, `suspects[].ran`, `suspects[].depth` all null, 15/15; `failures` array digest identical 15/15 |
| 1.1, 3.1 | the S3 grep table (12 counts) | 3 | identical 3/3 |
| 2.1 | `ply test --json --no-cache` on the audit's `RUNAWAY` program | 5 | `assertion` null 5/5, `diagnostic.code` `E0502` 5/5 |
| 4.1 | the recommended check, at file granularity, over all 83 codes | 1 | **0** — it catches nothing |
| 4.3 | `unarmed_gate.py .` | 3 | identical 3/3; both controls PASS; 2 unarmed, both allow-listed |
| 4.4 | `unarmed_gate.py` on a modified copy, four ways | 1 each | exit 1, exit 1, exit 2, exit 1 — seen to fail |

Round 2, 2026-08-27, same binary:

| § | command | runs | result |
| --- | --- | --- | --- |
| 0 | `find crates \( -name '*.rs' -o -name '*.ply' \) -newer target/debug/ply` | 4 | silent every time |
| 1.1 | the S1 command, re-run, digesting `failures[]` under two canonicalizations | 15 | `7962884c3f8d40d2…` 15/15 and `4057a8497bdcf0ac…` 15/15 — one array, two canonicalizations |
| 1.1 | `python3 -c "…scan failures[] for a key matching dur\|ms\|time…"` | 1 | none — which is why the digest is stable |
| 2.1 | `ply test --json --no-cache` on `assertion_failed.ply` and on `RUNAWAY`, interleaved | 5 + 5 | `E0501` 5/5 and `E0502` 5/5; `assertion` null 10/10 |
| 2.1 | flatten-and-diff the two failure objects | 1 | one registered-code difference, three rendered-string differences, the rest program identity |
| 2.1 | `ply test --json --no-cache` over all 60 `tests/fixtures/*.ply` | 60 fixtures × 1 | 5 failure objects, 4 distinct codes, all four slice/assertion fields null 5/5 |
| 2.1 | the same on `RUNAWAY` under `--engine {treewalk,machine,both}` | 3 | `E0502` on all three; `failures[]` digests identical |
| 1.1, 2.1 | the three trimmed greps, re-run raw | 3 each | 3, 10 and 16 lines — against 2, 5 and 6 printed |
| seen-to-fail | three probes corrupted on scratchpad copies, then restored | 1 each | each went red on the corruption and green again on restore; §"Seen to fail" below |

**Seen to fail (round 2).** House rule 5: a passing check is vacuous until it has
been watched fail. Each round-2 probe was broken deliberately, on a copy, never
on the worktree:

- **The code-distinction probe** (§2.1): rewrote `diagnostic.code` to `E0501` in
  a *copy* of the runaway artifact. The probe printed *"RED: the artifact carries
  no code-level distinction (both E0501)"* and exited 1. Against the untouched
  artifacts it exits 0 and prints `E0501 vs E0502`. So it reads the field it
  claims to read rather than printing a constant.
- **The trimmed-transcript probe** (§1.1, §2.1): run against a *copy* of
  `crates/ply-test` with `diagnose/tests.rs:570` deleted, the probe called a
  2-line transcript complete — it follows the tree rather than a hard-coded
  count. Against the real tree with the report's printed count of 2 it exits 1;
  with the raw count of 3 it exits 0.
- **The digest probe** (§1.1): flipping one character of `diagnostic.message` in
  a *copy* of the artifact moved the digest to `c5422ad857b70caa…` and the probe
  went red. The same probe also goes red when asked for the compact-separator
  digest, which is the failure the reviewer actually hit and is what identified
  D3.

After every demonstration, `find crates \( -name '*.rs' -o -name '*.ply' \)
-newer target/debug/ply` was re-run against the worktree and stayed silent: the
corruptions never touched it.

`PREREGISTRATION.md` at the worktree root carries the decision rule each round-1
command was committed to before it was taken, including the two dated amendments
and the one void result — and, per §2.1's D1 correction, the one S4 rule that was
committed to a conclusion its statistic could not license. Round 2's rules are in
`/tmp/.../PREREG-r2-dead-surfaces-corrections.md`, outside the repository.
