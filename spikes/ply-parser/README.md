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
| `lexer.ply` | Copied **unmodified** from `spikes/ply-lexer`; byte-identical, and needed no edit. |
| `spine.ply` | Token access, the threaded state `P`, diagnostics, `comma_list`, `qname`, the dump encoder. |
| `types.ply` | Type expressions, generics, effect rows, parameters. |
| `patterns.ply` | Pattern forms and literals. |
| `exprs.ply` | Expressions, precedence, blocks, `handle`. |
| `items.ply` | `fn`/`type`/`effect`/`test`/`law`/`derive`/`effect set`, imports, and `run`. |
| `harness/` | A separate cargo project: the reference-side dumper, the differential, `refdump`. |
| `fixtures/` | 26 hand-written `.ply` plus `reference-tests.corpus` — 716 inputs mined from the reference's own tests. |
| `GAPS.md` | **The point of the spike.** Consolidates the five area files below and takes the multiplier. |
| `GAPS-{spine,types,exprs,items,harness}.md` | The per-area records, kept: each carries its own measurements and every run of them. |

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

**Disagreements: 0.** Node-tag coverage **92 of 92**. `examples/clock.ply` agreed
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

`./arm-harness.sh`: **16 mutations, 16 armed, 0 survived, 0 invalid.** The three
classes ADR 0020 asks for, plus one per structural property claimed — dropped
field, wrong span, swapped associativity, swapped precedence, dropped list
element, widened dedup key, secondary label claiming primary, negated payload,
collapsed qualifier, dropped `pub`, lost `>=` split.

`harness/tests/fields.rs` covers what a dump-vs-dump comparison structurally
cannot: it reads `ast.rs`, takes all **144 fields of the 29 parsed types**, and
requires each to be named in the dumper (2 absent by design).

**Two findings from arming, both kept rather than fixed away**, and both in
`GAPS.md` §15: one mutation **survived all 763 inputs** while tag coverage read
92 of 92 (no input in the corpus contained a parenthesised pattern), and the
arming script itself **mis-scored three armed mutations as INVALID** because
`printf | grep -q` under `pipefail` reports printf's SIGPIPE.

---

## 3. What is NOT covered

Four things, each a number rather than a footnote.

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

`harness/` depends on `ply-syntax` **by path**, so it is compared against the tree
it sits in rather than against a copy. `harness/target/` and `.ply-cache/` are
both covered by `.gitignore`.

---

## Status

Taken 2026-08-28, in `~/.worktrees/ply/spike-ply-parser`, on the machine in
`docs/ONBOARDING.md` §Provenance at load 3.80–9.40.

- `./spikes/ply-parser/run.sh --arm` exits **0**.
- 112 in-language tests, in Ply, across the five modules.
- The differential: **763 inputs, 780,456 bytes, 0 disagreements**, tag coverage
  92/92.
- Arming: **16 armed, 0 survived, 0 invalid.**
- **No source file outside `spikes/ply-parser/` was modified.** Taking §4's
  measurements created three gitignored artifacts and nothing else:
  `examples/.ply-cache` and `spikes/ply-parser/.ply-cache` (written by `ply
  check` / `ply run`, removed before every timed run) and
  `spikes/ply-lexer/harness/target/`, from building that spike's existing
  `plydump` to count tokens the way ADR 0020 §1 counted them.
- **No git command was run.**
