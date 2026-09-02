# `spikes/ply-parser` — Ply's parser, written in Ply

**This is a spike. Nothing in the workspace imports it, builds it, or runs it.**
`cargo build --workspace`, `cargo test --workspace` and `cargo clippy
--workspace` do not reach any of it; the root `Cargo.toml`'s `members` list is
untouched, and `harness/` declares its own `[workspace]` so it resolves
separately. Deleting this directory removes it completely.

It exists to take **one number**. ADR 0020 §6.2 prices a self-hosted front end
by assuming a multiplier from the lexer spike, and says so in its own words:

> To get from a lexer to a front end needs a multiplier, and there is no way to
> measure one without writing the rest. **This is an assumption and it is
> labelled as one.**

ADR 0021 §3 names the consequence: *"the only thing that would settle it is
writing the parser, which that ADR recommends against — so the figure that could
refute its central estimate is one it declines to take."*

**The deliverable is [`GAPS.md`](GAPS.md), not the parser.** Read that first;
§13 is the number.

---

## 1. What was built

A hand port of `crates/ply-syntax/src/parser.rs` (2,114 lines) into Ply, in five
modules, plus a differential harness that compares the two **tree for tree and
diagnostic for diagnostic** over every `.ply` file in the tree.

| | |
| --- | --- |
| `lexer.ply` | Was copied unmodified from `spikes/ply-lexer`. **No longer byte-identical: one arm, three lines.** See below. |
| `spine.ply` | Token access, the threaded state `P`, diagnostics, `comma_list`, `qname`, the dump encoder. |
| `types.ply` | Type expressions, generics, effect rows, parameters. |
| `patterns.ply` | Pattern forms and literals. |
| `exprs.ply` | Expressions, precedence, blocks, `handle`. |
| `items.ply` | `fn`/`type`/`effect`/`test`/`law`/`derive`/`effect set`, imports, and `run`. |
| `rewrite.ply` | The three rewrites `Parser::run` applies after the grammar — `effect_set`, `record_update` and `try_op` — which §11R.D left in Rust and the checker's input needs. `GAPS.md` §16. |
| `resolve.ply` | The second phase: `resolve.rs` and `defaults.rs` ported — the module index, declarations, scopes, the dependency order, and the defaults pass — over the trees the modules above produce. `GAPS.md` §15. |
| `tycore.ply` | `crates/ply-core`'s type language, unification, environment and printer, ported. `GAPS.md` §17. |
| `derive.ply` | The deriver: `crates/ply-derive` ported — the rules table, the syntactic walk, the emitter, the retargeting of every generated span, and the expander's diagnostics; `derive_dump` is the fifth differential's probe. `GAPS.md` §18. |
| `infer.ply` | The checker: `infer.rs` ported — declarations, signatures, every expression form, rows, spec clauses, tests, laws, derivability, the comparison, simulation and region checks, the two passes over what expansion produced, and the restored path from a `Known`; `check_dump` publishes what `check_program` publishes, or its diagnostics. `GAPS.md` §17. |
| `hash.ply` | The hasher: `crates/ply-hash` ported — the reference graph, the normalized encoding, the component hashing, the spec and own-form keys and the closure, over `std.hash.blake3`; `hash_dump` is the sixth differential's probe. `GAPS.md` §19. |
| `harness/` | A separate cargo project: the reference-side dumper, the differential, `refdump`. It enters `ply_syntax` at `parse_unexpanded`, so the two sides are the same phase; `GAPS.md` §11R.D. `tests/resolve.rs` is the second differential, over whole programs. |
| `fixtures/` | Hand-written `.ply` files plus `reference-tests.corpus`, every string literal in the reference's own test file, re-mined by `mine-fixtures.py` whenever the reference grows syntax; the agreement test prints the count. For the resolve differential: `reference-programs.corpus`, every multi-module program `resolve.rs`'s tests build, mined by `mine-programs.py`, and `resolve-programs.corpus`, hand-written programs for the error paths. For the checker: `reference-checks.corpus`, every string literal in `crates/ply-core/src/tests.rs`, mined by `mine-checks.py`, and `check-programs.corpus`, hand-written programs for the outcomes nothing mined reaches. For the deriver: `derive-programs.corpus`, one module per shape `crates/ply-derive/src/tests.rs` exercises. For the hasher: `reference-hashes.corpus`, every string literal in `crates/ply-hash/src/tests.rs`, mined by `mine-hashes.py`. |
| `GAPS.md` | **The point of the spike.** Consolidates the five area files below and takes the multiplier. |
| `GAPS-{spine,types,exprs,items,harness}.md` | The per-area records, kept: each carries its own measurements and every run of them. |

> **Corrected 2026-08-30.** The `lexer.ply` row read: *"Copied **unmodified**
> from `spikes/ply-lexer`; byte-identical, and needed no edit."* It needed one:
> `punct` has no arm for byte 63, so `?` — which became a token after both
> spikes were taken (ADR 0028) — lexed as an error. The two files now differ by
> that arm, a two-line comment and one test. **`spikes/ply-lexer` still has the
> hole**, and worse: as of 2026-08-30 `spikes/ply-lexer/run.sh` reaches no test
> at all, because its harness does not compile —
> `non-exhaustive patterns: &ply_syntax::lexer::TokenKind::Question not covered`
> at its `src/lib.rs:66`. That spike is what ADR 0020 §6.1, ADR 0021 and ADR
> 0022 quote throughput from. It is recorded in `.github/ci-shards.sh`'s
> `SPIKES_OUTSIDE_CI` and it is **not** fixed here.

3,650 lines of Ply against `parser.rs`'s 2,114. Written by four agents in
parallel against one shared spine; `lexer + spine + types + patterns + exprs +
items` typechecked and passed 112 in-language tests with **no edit to any area's
file**, and then agreed with the reference over the whole corpus with no edit
either.

```
./spikes/ply-parser/run.sh                    # build, test, compare
./spikes/ply-parser/run.sh --arm              # and then the sixteen mutations
./spikes/ply-parser/measure-front-end.sh      # GAPS.md §13's series, re-taken
./spikes/ply-parser/measure-multiplier.sh     # GAPS-harness.md §H5's series
```

Per-area scripts (`test-*.sh`, `arm-*.sh`) each build a temp project holding
only the modules that area needs, because `ply test <dir>` typechecks every
module in the directory and four agents wrote into this one at once.

---

## 2. What agrees

| | inputs | bytes | dump records | nodes | diagnostics |
| --- | ---: | ---: | ---: | ---: | ---: |
| `examples/*.ply` | 13 | 333,883 | 88,680 | 44,784 | 0 |
| `crates/ply-std/ply/*.ply` | 8 | 421,938 | 153,774 | 79,192 | 0 |
| `fixtures/*.ply` (hand-written) | 26 | 5,144 | 1,509 | 555 | 40 |
| `fixtures/reference-tests.corpus` (mined) | 716 | 19,491 | 8,918 | 2,034 | 793 |
| **total** | **763** | **780,456** | **252,881** | **126,565** | **833** |

> **WITHDRAWN 2026-08-30, and then repaired the same day. Both halves are below,
> because the first is what the second is evidence about.**
>
> *"Disagreements: 0."* was false when it was read: **the differential was red on
> 28 of the 763 inputs, 70.2% of the corpus by bytes**, and it had been red
> before the three language features were converted into the port. `?` and
> `{..b, f: e}` became *syntax* after this spike was taken (ADR 0028, ADR 0023)
> and the Ply parser could neither lex nor parse them. The disagreeing set was
> **identical before and after the port** (checked by running the differential
> against both trees). `GAPS.md` §11R has that record and §11R.D the decision
> taken from it.
>
> **It is green again, and against a different tree.** The comparison now enters
> at `ply_syntax::parse_unexpanded` — the module as the *grammar* built it,
> before `effect_set`, `record_update` and `try_op` rewrite it — and the Ply
> parser learned to **parse** `?`, `{..b, f: e}`, named arguments and default
> parameters without expanding any of them. That is §11R.D's decision and the
> table below is the tally it produces. The one above is kept as the 2026-08-28
> reading, not deleted.

### As it stands, 2026-08-30

| | inputs | bytes | dump records | nodes | diagnostics |
| --- | ---: | ---: | ---: | ---: | ---: |
| `examples/*.ply` | 13 | 333,851 | 92,969 | 44,769 | 0 |
| `crates/ply-std/ply/*.ply` | 8 | 422,886 | 157,119 | 77,092 | 0 |
| `fixtures/*.ply` (hand-written) | **29** | 10,729 | 4,230 | 1,662 | 60 |
| `fixtures/reference-tests.corpus` (mined) | 716 | 19,491 | 8,951 | 2,034 | 768 |
| **total** | **766** | **786,957** | **263,269** | **125,557** | **828** |

> **Corrected 2026-08-30, by adding the column up.** The `dump records` total
> read **262,269**, and it was the only cell in either table that its own column
> does not sum to: 92,969 + 157,119 + 4,230 + 8,951 is **263,269**. A
> thousand-off transcription, found by re-running `run.sh` and adding the four
> printed tallies; every other cell in this table reproduces exactly. The four
> per-row figures were right, so nothing downstream of them moves — but the
> total is the number a reader quotes, and it was wrong.

**Disagreements: 0. Tolerances: 0** — the one this harness had is gone with the
pass it excused. Node-tag coverage **95 of 95**.

**Read the two tables together, because three numbers moved and only one of them
moved for a reason that is about this spike.**

* **766 against 763.** The 763 are all still there and all agree: 13 + 8 + 716 is
  unchanged and the 26 hand-written fixtures are unchanged. Three fixtures were
  **added** — `12-try-and-record-update.ply`, `13-named-arguments-and-defaults.ply`
  and `35-err-named-arguments-and-updates.ply` — because the corpus contained
  **zero** named arguments and **zero** default parameters, so the port could
  have read those tokens and thrown them away and nothing would have gone red
  (`GAPS.md` §11R.N measured exactly that). Mutation #17 is the demonstration:
  it is that port, and it now disagrees.
* **786,957 bytes against 780,456.** 5,585 of the difference is the three new
  fixtures. The remaining +916 is the tree's own files changing since
  2026-08-28: `examples/` fell 32 bytes and `crates/ply-std/ply/` grew 948.
* **125,557 nodes against 126,565, and this one is the decision.** Restricted to
  the same 763 inputs the figure is **124,450**. It is lower because the three
  rewrites are not run, and by exactly how much is measured rather than
  reasoned: `the_rewrites_this_comparison_gives_up_raise_exactly_these_diagnostics`
  reports **3,974 nodes** added by the rewrites over all 766 inputs, 3,585 of
  them in `examples/` and `crates/ply-std/ply/` — 2,137 in `db.ply` alone.
  **The node count went up, not down** — but by how much depends on a
  reconstruction, and the two readings are not strictly comparable. See the
  correction below.

  > **Corrected 2026-08-30, twice over.** The bullet ended: *"**Measured like
  > for like, the node count went up, not down:** 124,450 + 3,585 = 128,035
  > post-rewrite on today's files, against 126,565 on 2026-08-28's."* Two things
  > are wrong with that arithmetic and they push in opposite directions.
  >
  > 1. **3,585 is not what the rewrites add to the 763.** It is what they add to
  >    `examples/` and `crates/ply-std/ply/`, which is 21 of the 763. The other
  >    742 add 134 more — 3 in `fixtures/23-err-pub-where-it-cannot-go.ply` and
  >    131 across the mined bundle — so the figure over the 763 is **3,719**
  >    (measured per file with `nodes_the_rewrites_add`; the 766-input total of
  >    3,974 is 3,719 plus the 255 the three new fixtures add). The
  >    reconstruction is 124,450 + 3,719 = **128,169**.
  > 2. **Adding all three rewrites back overshoots**, because 126,565 was not a
  >    post-*all-three* figure. On 2026-08-28 the comparison entered at
  >    `parse_recovering` — `record_update` and `try_op` had run — but
  >    `effect_set::expand`'s effect on the tree was **projected back out** for
  >    the one file that uses sets, `examples/desk.ply`. So desk's effect-set
  >    atoms are already absent from 126,565 and must not be added twice.
  >
  > What survives either way is the sign, and it survives with margin. Drop
  > **all** of `desk.ply`'s 1,028 — a deliberate over-correction, since one `?`
  > and one `{..` in that file are not the expander's — and the reconstruction
  > is still 124,450 + 2,691 = **127,141 against 126,565**. `db.ply` alone
  > accounts for 2,137 of it and names no effect set, so no reading of this
  > makes the count fall. **The direction was right; the number was not, and the
  > word "measured" was doing work that a reconstruction cannot do.**

**What the comparison gives up, as an exit code rather than a paragraph.** The
three rewrites raise **7** diagnostics over the corpus — E0114 ×4, E0115 ×2,
E0105 ×1, on 7 mined fixtures — and add the 3,974 nodes above. That 7 is pinned
by a test, and taking it corrected `GAPS.md` §11R.D, which had said 9: two of
the E0114s are `items.ply`'s own refusal of `pub effect set`, which shares the
code and is raised by the grammar on both sides. **All seven were already
excused by the tolerance this change deleted**, so nothing the differential ever
actually compared was given up. `examples/clock.ply` agreed
byte for byte on the first attempt and so did every file up to `db.ply` (135,285
bytes, 63,215 records); **not one line of any area's `.ply` file was edited to
make the corpus agree.**

Both sides emit the same flat dump: every node in preorder **with its own span**,
every list's length, every `Option`'s presence, every enum arm, every scalar
payload, then every diagnostic's code, primary span, label count, note count, and
each label's own span and primary flag. The grammar is written twice — once in
`spine.ply` §"The dump encoder", once in `harness/src/lib.rs`, where **no `match`
has a `_` arm**, so a variant added to `ast.rs` stops the harness compiling rather
than being silently skipped.

### The comparison is armed

> **The note that stood here is spent.** It read: *"This figure can no longer be
> re-taken as written. `arm-harness.sh` requires a green baseline (lines 83–89)
> and refuses to run against the red differential above, so the count below is
> the one taken on 2026-08-28."* The baseline is green, the script ran, and the
> figure below is today's. The successor sweep it points at is still the honest
> caveat and is unchanged: `GAPS.md` §2 found **13 of the 26 remaining
> error-propagation sites unwatched**, 8 of them demonstrably so. Every mutation
> below corrupts the tree the parser builds when it *succeeds*; not one corrupts
> what it does when a sub-parse fails.

`./arm-harness.sh`: **22 mutations, 22 armed, 0 survived, 0 invalid**, taken
2026-08-30, 299 s. The three classes ADR 0020 asks for, plus one per structural
property claimed — dropped field, wrong span, swapped associativity, swapped
precedence, dropped list element, widened dedup key, secondary label claiming
primary, negated payload, collapsed qualifier, dropped `pub`, lost `>=` split —
plus six added with the four features: a discarded named argument, a wrong span
on `?`, `{..b}` collapsed to a plain record, a discarded default expression, a
missing `E0124`, and the `?` byte lexing as `%`.

**One of the sixteen was replaced rather than dropped, and it is recorded in the
table itself.** #7 named `types.ply`'s `param`, which moved to `exprs.ply` when a
parameter gained a default expression; the replacement is the identical
corruption at the identical parser in its new home, so the property it watches —
*every `Option` emits its presence* — is unchanged. The other fifteen anchors
still apply as written.

`harness/tests/fields.rs` covers what a dump-vs-dump comparison structurally
cannot: it reads `ast.rs`, takes all **149 fields of the 30 parsed types**, and
requires each to be named in the dumper (2 absent by design).

> **Corrected 2026-08-30: it said 144 fields of 29 types, and it said it while
> being defeated by an English sentence.** The count is 149/30. More importantly
> the test read `src/lib.rs` *whole*, comments included, so `ExprKind::App`'s
> keyword-argument field counted as covered because an unrelated doc comment
> about effect sets contained the word — and the test could be silenced on any
> field by writing a true sentence about it. It now strips `//` and `///` before
> matching, on both files. `GAPS.md` §11R.N is the measurement; the repair is
> armed by `a_field_named_only_in_a_comment_does_not_count_as_covered`.

**Two findings from arming, both kept rather than fixed away**, and both in
`GAPS.md` §15: one mutation **survived all 763 inputs** while tag coverage read
92 of 92 (no input in the corpus contained a parenthesised pattern), and the
arming script itself **mis-scored three armed mutations as INVALID** because
`printf | grep -q` under `pipefail` reports printf's SIGPIPE.

---

## 3. What is NOT covered

Four things, each a number rather than a footnote.

> **Corrected 2026-08-30 (morning): it is now seven, and item 2 is one of a set
> of three.** `record_update::expand` (530 lines) and `try_op::expand` (1,019)
> run inside `Parser::run` beside the `effect_set::expand` of item 2 and are not
> ported either, which is the entire 28-input / 70.2%-of-bytes disagreement §2
> above now reports. `ExprKind::App` also gained a keyword-argument field and
> `Param` a fallback-expression field (ADR 0029) that the reference dumper emits
> nowhere — a named argument is currently **invisible** to the comparison.
>
> **Corrected again the same day, after the decision was implemented.** Three of
> those seven are closed and one changed shape:
>
> * **Item 2 is withdrawn as written and replaced by item 2′ below.** Nothing is
>   projected any more, because `effect_set::expand` does not run: the
>   comparison enters at `parse_unexpanded`. The four-conjunct tolerance and its
>   asserted count of 7 are deleted.
> * **The two undumped fields are dumped.** `Param`'s fallback expression is an
>   `Option` in the `prm` record; a named argument is a `narg` node with its own
>   span, its name and its value, in a list that carries its length. Mutations
>   #17 and #20 are the evidence.
> * **The three rewrites leaving the comparison is the new item 2′**, and it is
>   larger than what it replaces: 2,087 lines of Rust that nothing in this spike
>   tests, priced at 7 diagnostics and 3,974 nodes over the corpus, both
>   measured by a test rather than asserted here.
>
> `GAPS.md` §11R.D is the decision and its argument, §11R.N the field that was
> invisible, §11R.X what implementing it cost and what it found.
> `GAPS-harness.md` §H2 is the enforced list.

1. **Diagnostic message text and severity are not compared.** Every site carries
   a `what: Bytes` that nothing reads — 134 call sites in the reference. Turning
   messages on additionally needs `TokenKind::describe`'s ~40 arms. What *is*
   compared for all 833 diagnostics: code, primary span, label count, note count,
   and each label's own span and primary flag.
2. **`effect_set::expand` is not ported** — 521 lines, the last unported piece of
   `ply_syntax`'s parse phase. Rather than drop `examples/desk.ply`, the
   expansion is projected back out of **the reference's own output** (the atom
   count is read off `EffectSetDef::expansion`, which the reference filled; no
   line of `effect_set.rs` is reimplemented). Cost, exactly: **1 of 21 corpus
   files (21.2% of corpus bytes), 20 of 716 mined fixtures, 0 trees still
   disagreeing, 7 inputs where a diagnostic differs.** The tolerance is four
   conjuncts wide and its count is asserted at 7 so it cannot grow.
3. **`FnDef::derived` is asserted `None`, not dumped.** The parser can write
   nothing else; if that changes, the assert fires.
4. **Float and decimal *values*.** Both sides dump the raw source over the
   literal's own span, because Ply can build neither an `f64` nor an `i128` from
   digits. Note the direction: this **removes** a normaliser where the lexer
   spike's float hole added one, so ADR 0020 §3.2's amendment does not recur.

**And the corpus itself is not a test suite.** Error paths are **3.2% of the
compared bytes and carry 100% of the 833 diagnostics** — 21× the lexer spike's
0.15% that ADR 0020 §3.1 records as the thing not to repeat, and still the weaker
half. Two of the sixteen mutations are invisible to all 755,821 bytes of real
source.

**What is not built at all:** `resolve.rs` (1,177 code lines) and inference. This
is two phases of a front end, and every figure below says so.

---

## 4. The multiplier, with its provenance

**Pre-registered** at `/tmp/ply-parser-spike/PREREGISTRATION-MULTIPLIER.md`
before any number existed, including the exclusion rule and the predictions.
**Instrument checked** with `.github/binary-is-current.sh` (exit 0, 152 inputs)
immediately before *and after* every series. **Correctness first**: the
differential was run to completion before the first probe was written.
**Minimum user CPU** of N runs, N = 5 under two seconds and N = 3 otherwise, wall
clock beside it, `uptime` on both sides, load 3.80–9.40, **every run printed and
none discarded** — the registered 25% spread rule fired three times and all three
failed series are printed alongside their re-takes. Full logs in
`/tmp/ply-parser-spike/series-frontend*.log`; `./measure-front-end.sh` re-takes it.

Five probes per file differing only in what they do with the source — a control,
the lexer, the parser, the lexer through its dump, the parser through its dump.
The fourth is **ADR 0020 §6.1's own probe shape**, so its published figure is
re-taken rather than quoted.

### Measured

| | this series | ADR 0020 §6.1 |
| --- | ---: | ---: |
| **lex+parse ÷ lex** | **2.49–3.31** per file, **2.62** over `examples/` | assumed, not measured |
| **parse ÷ lex** | **1.49–2.31** | — |
| lexing's share of lex+parse | **30–40%** | §6.2 borrowed "10–20%" |
| Ply lexer alone | 33,578–44,877 tok/s | not separated |
| Ply lexer through `dump` (§6.1's shape) | 26,083–36,462 tok/s | ~17,000 tok/s |
| Ply lex+parse | 10,143–16,046 tok/s | — |
| Rust front end, `ply check examples/` cold | 0.10 s user / 0.13 s wall | 0.21 s user |
| Rust front end warm | 0.01 s user / 0.02 s wall | 0.03 s user |

Two derived figures, both from the same sitting rather than across sittings — the
defect ADR 0020 §8's last bullet names in its own 12.6×:

- **The Rust front end is 10.0–13.4× the Ply lexer.** §6.1's cross-sitting 12.6×
  replicates.
- **Ply lex+parse over the identical 13 files `ply check examples/` reads costs
  3.01 s user, against 0.10 s for the Rust front end doing six phases. That is
  30×, measured, two phases against six.** Warm, 301×.

Three replications worth more than any of them alone: this series reproduces
`GAPS-harness.md` §H5's independent series (2.44–3.30) with every file within
±25%, and `GAPS-exprs.md` §10's 2.30 for parse ÷ lex on generated inputs — three
agents, three probe designs, one answer, and the earlier two were taken before
this one existed.

### Writing cost, which is a different quantity and is labelled as one

| ratio | total lines | code lines | `fn` |
| --- | ---: | ---: | ---: |
| Ply parser ÷ Ply lexer | **5.91** | 6.49 | 5.48 |
| Rust parser ÷ Rust lexer | **1.98** | 2.00 | 2.04 |
| Ply ÷ Rust, on the lexer | 0.58 | 0.39 | 1.21 |
| Ply ÷ Rust, on the parser | **1.73** | 1.27 | 3.24 |

**The lexer→parser step costs Ply 2.99× what it costs Rust.** A lexer in Ply is
*shorter* than its Rust original; a parser is *longer*.

### Projected — and every figure in this block is arithmetic, not a measurement

§6.2's own band applied to today's **measured** lexer term (1.15 s over
`examples/`), for comparison with what §6.2 computed from §6.1's:

| | §6.2 said | on today's measured lexer term |
| --- | --- | --- |
| a Ply front end over `examples/` | 13–27 s | **5.8–11.5 s** |
| its throughput | 1,700–3,400 tok/s | **3,900–7,800 tok/s** |
| against the Rust front end | 60–130× | **58–115×** |

---

## 5. What this says about ADR 0020's rejection

**Plainly: the rejection was right, and this spike makes the case for it stronger
than ADR 0020 could.**

**§6.2's assumed 5–10× band is not refuted, and it is no longer resting on
nothing.** The first of its terms is now measured: lex-plus-parse costs 2.49–3.31×
lexing. For the band to hold, the four unwritten phases must together cost
**1.5–4.6× what parsing cost** — plausible on the line counts, since `resolve.rs`
alone is 62% of `parser.rs` and inference is larger than either. **Nothing here
lets anyone say whether §6.2 is optimistic or pessimistic about the whole front
end, and this spike does not say it.** What it can say is that §6.2's *reasoning*
was borrowed and is wrong at the one point it can be checked: it assumed lexing is
10–20% of front-end time, and after two of six phases lexing is still 30–40%.

**ADR 0020 §8's break condition is not met.** It names the figure that would
weaken the estimate — *"if a Ply parser and typechecker cost only 2x the lexer
rather than 5–10x, the estimate falls to 5–11 s and the conclusion weakens without
reversing"*. Lex-plus-parse alone is already 2.49–3.31× **with the typechecker
unwritten**, so that condition is not met and the estimate stands.

**The number that decides it got worse, not better.** ADR 0020 §6.1 measured one
phase at 12.6× the whole Rust front end and called it a floor. Two phases measure
**30×** on identical input in one sitting — and there are four more.

**And the conclusion turned out to be insensitive to the thing that moved.** Both
halves of §6.1's absolute figures are 2–3× faster on this machine than when they
were taken, and the ratio between them did not move: 10.0–13.4× here against
§6.1's 12.6×. Redoing §6.2's arithmetic on today's numbers gives **58–115×**
against its own 60–130×. **A self-hosted front end at today's interpreter speed
would still be two orders of magnitude slower than the loop it exists to make
fast.**

**What the spike does refute is a different objection, and it was already
withdrawn.** ADR 0020 §5.1 held that a recursive-descent parser could not be
written in Ply at all — that it would recurse per element and blow the
10,000-call ceiling, forcing an explicit-stack automaton that could not be
differentially compared. ADR 0022 refuted the premise; this spike is the
demonstration. **The deepest frame in a 5,000-term expression chain is one**, the
ceiling never fired on any input, and against the reference's own `MAX_DEPTH` of
128 there is 13× headroom — so the ceiling **cannot** bite on any input the
reference itself accepts. `iterate` is the reason, and it did exactly what ADR
0022 added it for.

So: **expressiveness is not the blocker and is less of one than the lexer spike
suggested** — every risk registered in advance (the AST, generic higher-order
`comma_list`, the ceiling) failed to materialise. **Throughput is the blocker,
exactly as ADR 0020 §7 said.** The three changes that would move it are unchanged
from ADR 0021 §4's list, and `GAPS.md` re-ranks them with a parser's evidence: a
lint for the field-order rule first (it composes across 178 call sites through a
nine-field state, and its two-growing-lists case has no correct spelling at all),
`list_at`/`list_head`/`list_last` second, `const` third.

**One item this spike adds to that list**, because only a bootstrap sees it:
replacing `?` with a `bail: Bool` is *isomorphic* to the reference's
`PResult<Bail>` and is the right design, and it still leaves an invariant Ply
cannot state, nothing checks, **ten functions violated on the first write**, and
**63 of 83 guards cannot be shown to matter**. `GAPS.md` §2.

---

## 6. Why the harness is a separate cargo project

The same reason `spikes/ply-lexer/harness/Cargo.toml` gives, and with the same
cost. Adding a fourteenth workspace member moves the test-target counts that
`README.md`, `CONTRIBUTING.md` and `docs/ONBOARDING.md` all state, and a spike
should not make three documents stale. The cost is that `cargo test --workspace`
does not reach it, so **this will bit-rot exactly as `crates/ply-codegen-spike`
did**: the next change to `ply_syntax::ast` will break `harness/src/lib.rs` and no
workspace command will notice. `run.sh` is the one command that would.

> **This paragraph was a prediction and it came true, so it is now a record.**
> Between 2026-08-28 and 2026-08-30 four language features landed. The
> `harness/src/lib.rs` `match` over `ExprKind` stopped compiling on two new
> variants, exactly as written above; the differential went red on 28 of 763
> inputs; `tests/fields.rs` began failing on a field the AST had gained; and
> nothing said any of it, because **`run.sh` was in no CI job at all**.
>
> **There is a job now.** `.github/workflows/ci.yml`'s `parser-spike` runs
> `./spikes/ply-parser/run.sh` on every pull request, gated on `fmt` and
> `clippy` like every other expensive job and named in the `ci` aggregate's
> `needs:` so that a skipped run is a red tick rather than a green one. It was
> watched to fail three ways and to stay green on a control before it was
> believed; the four runs are recorded in the job's own comment.
>
> **And the check one level up.** `.github/ci-shards.sh verify` now covers
> `spikes/` the way it covers `crates/`: a directory there that no job runs and
> no entry excuses fails the `plan` job. That is what would have caught this in
> the first place, and it is what now says out loud that `spikes/ply-lexer` is
> in the same condition and worse.

`harness/` depends on `ply-syntax` **by path**, so it is compared against the tree
it sits in rather than against a copy. `harness/target/` and `.ply-cache/` are
both covered by `.gitignore`.

---

## Status

### 2026-08-30, in `~/.worktrees/ply/spike-parser-ci` — green, armed, and in CI

- `./spikes/ply-parser/run.sh --arm` exits **0**.
- **119 in-language tests**, in Ply, across the six modules — 110 plus nine
  written for the four features: the `?` token and its tier, the record-update
  base and its two refusals, a named argument and the two diagnostics that point
  at one, and a parameter's default with the lambda refusal that has no span.
- The differential: **766 inputs, 786,957 bytes, 0 disagreements, 0
  tolerances**, tag coverage **95/95**. Of those, the 763 that §2's first table
  counted are all present and all agree; three fixtures were added for the two
  features the corpus contained none of.
- Arming: **22 armed, 0 survived, 0 invalid**, 299 s. One of the original
  sixteen was replaced (its anchor moved with `param`) and six were added.
- `harness/tests/fields.rs`: **2 passed** — the field test, and the test that
  arms its comment-stripping repair.
- The comparison is against `ply_syntax::parse_unexpanded`, a `#[doc(hidden)]`
  entry point added to a shipping crate for this spike. That is the single real
  cost of the decision and `crates/ply-syntax/src/tests.rs`'s
  `parse_unexpanded_is_reached_by_no_shipping_caller` is what keeps it to one
  caller; it was watched to fail by naming the function in `ply-eval`.
- **In CI**: `parser-spike`, required through the `ci` job's `needs:`. Local
  cost, on the machine in `docs/ONBOARDING.md` §Provenance at load 6–10:
  **34.5 s wall / 259 s CPU** for the cold `--locked --release -p ply-cli`
  build, **18.2 s wall / 32.6 s CPU** for everything after it. **The two-core
  runner figure has not been taken.**
- **No git command was run.**

### 2026-08-28, in `~/.worktrees/ply/spike-ply-parser`

On the machine in `docs/ONBOARDING.md` §Provenance at load 3.80–9.40.

- `./spikes/ply-parser/run.sh --arm` exits **0**.
- 112 in-language tests, in Ply, across the five modules.
- The differential: **763 inputs, 780,456 bytes, 0 disagreements**, tag coverage
  92/92.
- Arming: **16 armed, 0 survived, 0 invalid.**

**Re-checked 2026-08-30 morning, after `list_at`, `?` and `let` destructuring
were converted into the port. Three of those five lines no longer hold, and the
two that do were re-run rather than quoted. This block is the *diagnosis*; the
block above it is what was done about it the same day, and the two lines below
marked SUPERSEDED are the ones the repair moved:**

- **SUPERSEDED — `run.sh` exits 0 now** (`--arm` included), and `fields.rs`
  passes because `Param`'s fallback expression is dumped. What follows is the
  diagnosis as it stood, kept because it is the reason the entry point moved.
  `run.sh` exited **101** — but **not** at the differential, and this line was
  corrected rather than replaced. It read: *"at the differential and only there.
  Everything before it passes: instrument current, **110** in-language tests
  (112 → 110; two suites became unwritable when `bail` did, `GAPS.md` §2),
  harness `--lib` 10 + `--test fields` 1, `cargo fmt --check` and `clippy -D
  warnings` clean."* Every step was re-run individually on 2026-08-30 and all of
  those hold **except the last**: `cargo test --test fields` **fails**, on
  `("Param", "default")` — a field ADR 0029 added to the AST that the reference
  dumper names nowhere. `fields.rs` runs *before* the differential in `run.sh`,
  so **the differential is no longer reached by the one command that runs it**,
  and the 28-input figure below was taken by invoking it directly. `GAPS.md`
  §11R.S has the step-by-step.
- **SUPERSEDED — 766 of 766 agree.** As diagnosed: the differential was **735 of
  763 agreeing, 28 disagreeing — 70.2% of the corpus by bytes.** Not caused by
  the port: the disagreeing set was identical when the same differential ran
  against the pre-port tree. Cause and cost in `GAPS.md` §11R; the decision that
  closed it in §11R.D, and what implementing it cost in §11R.X.
- Arming, re-taken by substitution: `arm-harness.sh` refuses a red baseline by
  design, so each of the 16 was applied to the ported source and scored on whether
  the disagreeing **set** changed. **All 16 still apply and all 16 still go red**,
  changing between 29 and 152 inputs — the port disarmed none of them. (Run
  normally against the green baseline on 2026-08-30 evening, 15 of the 16 apply
  unchanged and the sixteenth was replaced; see §2.)
- But the 16 watch one half of the parser. A successor sweep over the sites `?`
  could not convert — 26 hand-written `Stop(Err(q))` propagations — found **13 of
  26 change nothing across all 763 inputs**, 8 of them demonstrably not equivalent
  mutants. `GAPS.md` §2, "The residue is half unwatched".
- Re-taken and reproduced: the §13R multiplier. An independent interleaved series
  got **2.632 before / 2.471 after** against §13R's 2.585 / 2.443, with the
  byte-identical-`lexer.ply` control at **1.017** against its 1.004 — same
  direction, same size, on a different sitting.
- **No source file outside `spikes/ply-parser/` was modified.** Taking §4's
  measurements created three gitignored artifacts and nothing else:
  `examples/.ply-cache` and `spikes/ply-parser/.ply-cache` (written by `ply
  check` / `ply run`, removed before every timed run) and
  `spikes/ply-lexer/harness/target/`, from building that spike's existing
  `plydump` to count tokens the way ADR 0020 §1 counted them.
- **No git command was run.**
