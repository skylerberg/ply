# GAPS — integrating the four areas, and the differential against `crates/ply-syntax`

The four areas of this spike were written independently. This file is what
happened when they were put together and compared, function for function, tree
for tree, against the parser they are a port of — and the number ADR 0021 §3
says only writing the parser could produce.

Written in `spikes/ply-lexer/GAPS.md`'s style: every count in it was taken on
this tree, by a command that is named, and nothing is approximated.

---

## §H1 The corpus figure

`spikes/ply-parser/harness/tests/agreement.rs`, run by `./run.sh`. Every number
is printed by the run; none is claimed.

| | inputs | bytes | dump records | nodes | diagnostics |
| --- | ---: | ---: | ---: | ---: | ---: |
| `examples/*.ply` | 13 | 333,883 | 88,680 | 44,784 | 0 |
| `crates/ply-std/ply/*.ply` | 8 | 421,938 | 153,774 | 79,192 | 0 |
| `fixtures/*.ply` (hand-written) | 26 | 5,144 | 1,509 | 555 | 40 |
| `fixtures/reference-tests.corpus` (mined) | 716 | 19,491 | 8,918 | 2,034 | 793 |
| **total** | **763** | **780,456** | **252,881** | **126,565** | **833** |

**Disagreements: 0**, on everything above, with one boundary stated in §H4 and
priced there.

A **node** is a `S:E:TAG;` record — one AST node with its own span. A **record**
is any unit of the dump, so the extra 126,316 are list lengths, option presence
flags, enum arms and scalar payloads: the structure that makes a dropped element
shift everything after it rather than being absorbed.

**Node-tag coverage: 92 of 92.** Every tag the reference side of this dump can
emit is reached, and `the_comparison_reaches_every_tag_the_reference_side_can_emit`
fails if that stops being true. §H3 is about why that is not a stopping rule.

### The order it was built in, because it is the answer to "does it work"

`examples/clock.ply` (1,611 bytes, 462 records) was the first, and it agreed
byte for byte on the first attempt. Then the other twelve examples, then the
stdlib in size order, ending at `crates/ply-std/ply/db.ply` — 135,285 bytes,
63,215 records, 32,977 nodes — which also agreed on the first attempt. **Not one
line of any area's `.ply` file was edited to make the corpus agree.** The four
areas were written against `parser.rs` by hand and they were right.

---

## §H2 What is *not* compared, in one list

1. **Diagnostic message text and severity.** Every site carries a
   `what: Bytes` that nothing reads (`GAPS-items.md` §P10 counts 105 literals in
   that area alone), and comparing messages additionally needs
   `TokenKind::describe`'s forty arms. What *is* compared for every diagnostic:
   the code, the primary span, the label count, the note count, and each label's
   own span and primary flag. 833 diagnostics, all of them.
2. **`effect_set::expand`.** Not ported. §H4 prices it exactly.
3. **`FnDef::derived`.** The parser writes `None` at `parser.rs:723` and can
   write nothing else, so the port does not carry it. The reference dumper
   **asserts** it rather than skipping it, because a field reached and not
   emitted is the exact survivor this comparison exists to find.
4. **`Lit::Decimal`'s `mantissa` and `scale`, and `Lit::Float`'s value.** Both
   are dumped as the raw source over the literal node's own span. Ply can build
   neither an `f64` nor an `i128` from digits, so a dump carrying the value would
   be comparing `numerics.rs` against nothing at all. This is the substitution
   `spikes/ply-lexer/harness` makes for floats, made here in the direction that
   **removes** a normaliser rather than adding a second one to check the first.

That list is enforced, not remembered: `harness/tests/fields.rs` reads `ast.rs`,
takes all **144 fields of the 29 types the parser builds**, and requires each to
be named in the reference dumper. Two are absent by design and are listed with
their reason. Deleting `self.boolean(o.resource_param)` from the dumper was seen
to fail it.

> **The limit of that test, found by trying to arm it the first way.** Renaming
> the *binding* — `ExprKind::WithCell { init: i0, .. }` — leaves the field name
> in the pattern and the test stays green. It checks that a field is **named**,
> not that it is **emitted**. That is the whole class it is claimed to cover: a
> field added to `ast.rs` that nobody ever wrote down. A field written down and
> then not used needs the mutations of §H3.

---

## §H3 Arming: sixteen corruptions, and the one that survived a corpus of 763 inputs

`./arm-harness.sh`. Each mutation edits a copy of the six modules under a temp
directory and runs the whole differential against it. **16 armed, 0 survived, 0
invalid** — on the second pass. Both of those numbers have a story and both are
the point.

| # | the corruption | inputs that disagreed |
| ---: | --- | ---: |
| 1 | a fn's `where` constraints are parsed and thrown away | 3 |
| 2 | a fn's effect row is never emitted | 130 |
| 3 | a parenthesised pattern keeps its inner span | 1 |
| 4 | binary operators become **right-associative** | 11 |
| 5 | `+` binds as tightly as `*` | 4 |
| 6 | every comma list loses its last member | 90 |
| 7 | a parameter's type annotation is discarded | 54 |
| 8 | the diagnostic dedup widens from code+span to code | 137 |
| 9 | a secondary label claims to be primary | 19 |
| 10 | every integer literal is dumped negated | 95 |
| 11 | `store::place` loses its module | 16 |
| 12 | `pub` is consumed and the item comes out private | 25 |
| 13 | an effect-row atom loses its resource label | 29 |
| 14 | `>=` closing a type parameter list stops splitting | 2 |
| 15 | a test's label span becomes the whole item's | 30 |
| 16 | a block's tail expression is never emitted | 53 |

The brief asked for three classes — a dropped field, a wrong span, a swapped
associativity. Those are #1/#2/#13/#16, #3/#15, and #4. The rest are one per
structural property the dump grammar claims: list lengths (#1, #6), option
presence (#2, #7, #16), enum arms (#12), scalar payloads (#10), spans (#3, #15),
diagnostic shape (#8, #9), and the one tolerance the harness grants (#13, which
corrupts precisely the row atoms `examples/desk.ply`'s projection compares).

### The finding: #3 survived, and tag coverage did not notice

On the first pass **mutation #3 survived**. Deleting `pattern_body`'s
`inner.span = open.to(close)` rewrite — the one `patterns.ply`'s own header
singles out as the thing "a dump that led with the `Ident`'s span would agree
with the reference on `(x)` while carrying a different tree" — changed **nothing**
across 21 real files, 25 hand-written fixtures and all 716 mined inputs. **Not
one input in the corpus contained a parenthesised pattern.**

Tag coverage was **92 of 92 at the time**. Every node tag the grammar can emit
was reached and this decision still was not, because `(x)` and `x` produce the
same tag and differ only in a span. That is the argument against tag coverage as
a stopping rule, and it is why `fixtures/11-parenthesised-pattern.ply` exists and
says so in its own header. With it, #3 goes red on exactly one input — which is
the other half of the finding: the fixture that closes a hole is often the only
thing in the corpus that touches it.

### The other finding, in the instrument rather than the parser

The first arming pass scored **three armed mutations as INVALID**. The tell was
that they were the three with the *most* disagreements. The cause was in the
script: under `set -o pipefail`,

```sh
printf '%s' "$out" | grep -q "disagree on"
```

reports the pipeline's status as `printf`'s **SIGPIPE**, because `grep -q` exits
at the first match and closes the pipe — so the more output a mutation produced,
the more reliably it was scored as not matching. `arm-harness.sh` now matches
with `[[ "$out" == *"disagree on"* ]]` and has no pipeline, and the comment above
`set -u` says why. An arming script that silently mis-scores its loudest results
is the same defect class as the thing it is there to prevent.

---

## §H4 The `effect set` boundary, priced

`items.ply` does not port `crates/ply-syntax/src/effect_set.rs` (521 lines: a
cycle search with an explicit stack, a post-order fixed point, a canonicalising
sort, a write-back, and three diagnostics with multi-note bodies). The plan
ranked it last and this spike did not reach it. `parse_recovering` runs it
*inside* `Parser::run`, so the reference tree for a file that uses sets is
post-expansion and the port's is not.

Rather than drop those inputs, the harness projects the expansion back out of
the reference's **own output**:

* `expand` appends one set's `expansion` to `row.atoms` per entry of
  `row.aliases`, and those expansions are sitting in the tree because
  `write_back` put them there. How many atoms were appended is therefore *read
  off*, not recomputed; no line of `effect_set.rs` is reimplemented.
* A set that was refused, one on a cycle, one named from another module and one
  that does not exist all carry an empty expansion, which is exactly the zero
  atoms `expand` splices for them.
* `write_back` gives the expansion to the first declaration of a name and to no
  later one, so the map is built first-wins.

**What that costs, exactly:**

| | count | share |
| --- | ---: | ---: |
| corpus files needing the projection | 1 of 21 (`examples/desk.ply`) | **21.2% of corpus bytes** |
| mined fixtures needing it | 20 of 716 | 2.8% of that set |
| inputs where the **tree** still disagreed after projecting | **0** | — |
| inputs where a **diagnostic** disagreed | 7 of 763 | 0.9% |

The diagnostics are deliberately **not** projected: `expand` raises `E0105`,
`E0114` and `E0115`, and `items.ply` raises `E0114` for reasons of its own, so
telling them apart by code would be guessing. The tolerance is shape-based and
four conjuncts wide — the input must use sets, the trees must be *identical*, the
port's diagnostics must be a **prefix** of the reference's (which they must be,
since `expand` runs last and can only append), and every extra must carry an
effect-set code. The count of inputs it excuses is asserted at 7, so the hole
cannot grow quietly.

`examples/desk.ply` — 159,971 bytes, 38,573 records — agrees **exactly** under
the projection, diagnostics included, because it parses clean.

---

## §H5 The one term of ADR 0020 §6.2's multiplier that writing the parser could measure

> ADR 0020 §6.2: *"To get from a lexer to a front end needs a multiplier, and
> there is no way to measure one without writing the rest. **This is an
> assumption and it is labelled as one.** In conventional compilers lexing is
> 10–20% of front-end time, which puts a full Ply front end at 5–10x the lexer's
> cost — call it 1,700–3,400 tokens/s."*

Pre-registered in `/tmp/ply-parser-spike/PREREGISTRATION-INTEGRATION.md` before
any of these numbers existed, including the decision rule and the prediction.
`./measure-multiplier.sh`. Three probes per file differing only in what they do
with the source — `bytes_len` (Z), `len(lex(src).toks)` (L),
`len(parse(src).node.items)` (P) — minimum **user CPU** over N runs, wall clock
printed beside it, `.github/binary-is-current.sh` exit 0 before the series,
`uptime` before and after, every run printed, nothing discarded.

| file | bytes | tokens | Z | L | P | (P−Z)/(L−Z) | lex+parse |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `examples/desk.ply` | 159,971 | 19,576 | 0.04 | 0.54 | 1.26 | **2.44** | 16,046 tok/s |
| `crates/ply-std/ply/db.ply` | 135,285 | 29,213 | 0.04 | 0.62 | 1.80 | **3.03** | 16,598 tok/s |
| `crates/ply-std/ply/http.ply` | 127,278 | 17,662 | 0.04 | 0.47 | 1.46 | **3.30** | 12,438 tok/s |
| `crates/ply-std/ply/json.ply` | 63,370 | 11,668 | 0.04 | 0.30 | 0.77 | **2.81** † | 15,984 tok/s |
| `crates/ply-std/ply/router.ply` | 54,397 | 8,644 | 0.06 | 0.33 | 0.92 | **3.19** | 10,051 tok/s |

Load 2.87–4.87 throughout, unchanged across the series.

† **`json.ply`'s first series is UNMEASURED by the rule registered in advance**,
and both series are reported. Its first `L` spread 48% (0.33/0.36/0.49) and its
`P` spread 41% (0.99/1.40/1.32), against a registered threshold of 25%; the
figure it would have given is 3.36. The second series, taken with nothing else
running, spread 3% on both and gives 2.81. **Every other file's `L` and `P`
series spread 1–9%** — the four that were never in question span 2.44 to 3.30.

### **Lex-plus-parse costs 2.44–3.30× lexing alone. The registered prediction was that it would exceed 5. It does not.**

Read the quantity carefully, because the ambiguity in the phrase "the multiplier"
is where a wrong conclusion would come from. `(P−Z)/(L−Z)` is **lex-plus-parse
divided by lex**, so the parse step on its own is `(P−L)/(L−Z)`:

| file | lex+parse ÷ lex | **parse ÷ lex** | lexing's share of lex+parse |
| --- | ---: | ---: | ---: |
| `desk.ply` | 2.44 | 1.44 | 41% |
| `db.ply` | 3.03 | 2.03 | 33% |
| `http.ply` | 3.30 | 2.30 | 30% |
| `json.ply` (2nd series) | 2.81 | 1.81 | 36% |
| `router.ply` | 3.19 | 2.19 | 31% |

**`GAPS-exprs.md` §10 measured parse ÷ lex at 2.30 in Ply and 2.03 in Rust on
generated expression-shaped inputs, and said in terms that it was not this
number.** On the real corpus it comes out at 1.44–2.30. Two agents, two
workloads, two probe designs, one answer. That agreement is worth more than
either figure alone, and it was not arranged: those runs were taken before this
area existed.

### What it does and does not say about ADR 0020 §6.2

**What it settles.** §6.2's reasoning was borrowed: *"in conventional compilers
lexing is 10–20% of front-end time"*, therefore 5–10x. In Ply, after two of the
front end's phases, **lexing is already down to 30–41%** of what has been spent.
That is now measured on this language and this corpus rather than imported from
someone else's compiler, and it is the term ADR 0021 §3 says nobody had.

**What it does not settle, and this is most of it.** A front end is not lex plus
parse. `crates/ply-syntax/src/resolve.rs` is 1,347 lines and inference is larger
than either; neither is written in Ply and neither is priced here. Extending the
measurement needs the same arithmetic §6.2 did, with one term now known:

* if the remaining phases together cost about what parsing cost, the front end
  lands near **4–5.5x** the lexer — at or just under the bottom of §6.2's band;
* if they cost about twice parsing — which the line counts make at least as
  likely — it lands at **6–8x**, inside the band.

**So §6.2's band is not refuted. My registered prediction is.** I predicted the
parser step alone would exceed 5x, on §6.2's own stated reason that the phases
after lexing are the ones that build records and lists. It is 1.4–2.3x. The tax
`spikes/ply-lexer/GAPS.md` §1 and §10 describe is real and it is **not
concentrated in the tree-building phase** — the same conclusion `GAPS-exprs.md`
§10 reached from the other direction and the same one ADR 0020 §9's own
sensitivity note flags as the thing that would matter.

**What would still break the estimate, and does not.** ADR 0020 §9: *"If a Ply
parser and typechecker cost only 2x the lexer rather than 5–10x, the estimate
falls."* Lex-plus-parse is already 2.44–3.30x with the typechecker unwritten, so
that condition is not met and the estimate stands on this evidence.

**Throughput, the other half of §6.2.** It projected 1,700–3,400 tokens/s for a
Ply front end. Lex-plus-parse measures **10,051–16,598 tokens/s**. One caveat and
it is load-bearing: §6.1's ~17,000 tok/s lexer figure was taken through
`lexer.dump`, and both probes here exclude dumping, so the *ratios* above are
internally consistent while the two absolute figures are not directly
comparable. Against the Rust front end's ~215,000 tokens/s, lex-plus-parse in Ply
is **13–21x slower for two phases of six** — where §6.1 recorded 12.6x for the
first phase alone.

### Every run, per house rule 4

Series 1, load 2.87 before / 4.74 after, `binary-is-current.sh` exit 0 before it:
```
  desk.ply
    Z run 1: user 0.05s wall 0.05s
    Z run 2: user 0.04s wall 0.05s
    Z run 3: user 0.04s wall 0.05s
    Z run 4: user 0.04s wall 0.05s
    Z run 5: user 0.04s wall 0.05s
    L run 1: user 0.54s wall 0.54s
    L run 2: user 0.55s wall 0.57s
    L run 3: user 0.54s wall 0.54s
    P run 1: user 1.26s wall 1.27s
    P run 2: user 1.27s wall 1.28s
    P run 3: user 1.26s wall 1.27s

  db.ply
    Z run 1: user 0.04s wall 0.05s
    Z run 2: user 0.04s wall 0.05s
    Z run 3: user 0.05s wall 0.05s
    Z run 4: user 0.04s wall 0.05s
    Z run 5: user 0.04s wall 0.05s
    L run 1: user 0.62s wall 0.63s
    L run 2: user 0.62s wall 0.63s
    L run 3: user 0.63s wall 0.64s
    P run 1: user 1.85s wall 1.87s
    P run 2: user 1.82s wall 1.84s
    P run 3: user 1.80s wall 1.81s

  http.ply
    Z run 1: user 0.04s wall 0.05s
    Z run 2: user 0.04s wall 0.05s
    Z run 3: user 0.04s wall 0.05s
    Z run 4: user 0.04s wall 0.05s
    Z run 5: user 0.04s wall 0.05s
    L run 1: user 0.47s wall 0.48s
    L run 2: user 0.47s wall 0.48s
    L run 3: user 0.48s wall 0.49s
    P run 1: user 1.50s wall 1.62s
    P run 2: user 1.82s wall 2.16s
    P run 3: user 1.46s wall 1.56s

  json.ply
    Z run 1: user 0.06s wall 0.07s
    Z run 2: user 0.05s wall 0.06s
    Z run 3: user 0.05s wall 0.06s
    Z run 4: user 0.05s wall 0.06s
    Z run 5: user 0.05s wall 0.06s
    L run 1: user 0.33s wall 0.34s
    L run 2: user 0.36s wall 0.37s
    L run 3: user 0.49s wall 0.61s
    P run 1: user 0.99s wall 1.02s
    P run 2: user 1.40s wall 1.75s
    P run 3: user 1.32s wall 1.74s

  router.ply
    Z run 1: user 0.06s wall 0.07s
    Z run 2: user 0.06s wall 0.07s
    Z run 3: user 0.06s wall 0.07s
    Z run 4: user 0.06s wall 0.08s
    Z run 5: user 0.06s wall 0.07s
    L run 1: user 0.33s wall 0.35s
    L run 2: user 0.36s wall 0.41s
    L run 3: user 0.33s wall 0.34s
    P run 1: user 0.92s wall 1.01s
    P run 2: user 1.00s wall 1.20s
    P run 3: user 0.92s wall 1.05s
```

Series 2 — `json.ply` retake only, load 3.49 after, taken with nothing else
running. The first series is kept above and is not discarded; this is a second
series and is labelled as one:
```
  json.ply
    Z run 1: user 0.04s wall 0.05s
    Z run 2: user 0.04s wall 0.05s
    Z run 3: user 0.04s wall 0.05s
    Z run 4: user 0.04s wall 0.05s
    Z run 5: user 0.04s wall 0.05s
    L run 1: user 0.30s wall 0.30s
    L run 2: user 0.30s wall 0.31s
    L run 3: user 0.30s wall 0.31s
    P run 1: user 0.77s wall 0.78s
    P run 2: user 0.78s wall 0.79s
    P run 3: user 0.79s wall 0.80s
```

---

## §H6 Writing cost, whole parser against whole lexer

The other reading of "multiplier", and the one the three area GAPS files each
took a slice of. Segmentation is `GAPS-items.md` §P8's, fixed before this area
started: in-language `test` blocks excluded, code lines are neither blank nor
`//`.

| | total lines | code lines | `fn` |
| --- | ---: | ---: | ---: |
| `spikes/ply-lexer/lexer.ply` | 619 | 370 | 58 |
| the parser, 5 modules | 3,684 | 2,405 | 320 |
| `crates/ply-syntax/src/lexer.rs` | 1,070 | 950 | 48 |
| `crates/ply-syntax/src/parser.rs` | 2,115 | 1,898 | 98 |

| ratio | total | code | `fn` |
| --- | ---: | ---: | ---: |
| Ply parser ÷ Ply lexer | **5.95** | **6.50** | **5.52** |
| Rust parser ÷ Rust lexer | **1.98** | **2.00** | **2.04** |
| Ply ÷ Rust, on the lexer | 0.58 | 0.39 | 1.21 |
| Ply ÷ Rust, on the parser | **1.74** | **1.27** | **3.27** |

**The step from lexer to parser costs Ply 3.0x what it costs the reference
language** (5.95 ÷ 1.98). The registered M3 prediction — that this ratio exceeds
1 — holds, and the size of it is the finding: a lexer in Ply is *shorter* than
its Rust original (0.58) and a parser in Ply is *longer* (1.74), so the two
languages cross over somewhere between the two phases.

The `fn` column says where it goes: 3.27 Ply functions per Rust function on the
parser against 1.21 on the lexer. `GAPS-items.md` §P5 and `GAPS-types.md` name
the three causes — no early return and no `?`, no tuples, and every sequence
needing a named accumulator type and often a named step function.

**If §6.2's arithmetic is redone with a measured coefficient instead of the
conventional-compiler one, on writing cost**, a front end that is 3x a parser in
Rust would be ~18x a lexer in Ply rather than 5–10x. That is an extrapolation
from one phase and it is labelled as one, in §6.2's own words.

---

## §H7 The reference dumper costs 854 lines of Rust, and that is a number about the method

`harness/src/lib.rs` is 1,089 lines, 854 of them code, 69 functions: one
preorder walk of `ast.rs` emitting the same grammar `spine.ply`'s dumper emits
in 244 code lines of Ply spread across five modules.

Two things follow.

1. **A differential is not free.** ADR 0020 §7 says to keep the lexer spike's
   harness pattern, and it is right, but the reference side of a *parser* dump is
   nearly half the size of the parser being compared against — 854 code lines
   against `parser.rs`'s 1,898. Anyone pricing "and we would verify it with a
   differential" should carry that line.
2. **`no _ arm` is what makes it worth the 854 lines.** Not one `match` in that
   file has a wildcard, so a variant added to `ast.rs` stops the harness
   compiling. A wildcard would emit nothing for the new variant, the Ply side
   would emit nothing either, and the comparison would stay green over a feature
   neither side implements. That is `docs/adr/0020` §3.1's finding in a different
   costume.

---

## §H8 Error paths are 3.2% of the compared bytes, and 100% of the diagnostics

`docs/adr/0020` §3.1 records the lexer spike's headline of 768,760 bytes turning
out to be **0.15% error paths** as the thing not to repeat. Here:

* the 21 real `.ply` files — **755,821 bytes, 96.8% of everything compared** —
  raise **zero** diagnostics between them. Agreeing on them says nothing at all
  about `recover_to_item`, about any "cannot be `pub`" path, or about a single
  unclosed delimiter.
* the fixtures — **24,635 bytes, 3.2%** — raise **all 833**.

That is a 21x improvement on the lexer spike's ratio and it is still the weaker
half of the corpus by volume. It is deliberate: `fixtures/reference-tests.corpus`
is **every one of the 716 distinct string literals in
`crates/ply-syntax/src/tests.rs`**, extracted mechanically by
`mine-fixtures.py` with no filter, because a filter is a place to quietly drop
the fixture that would have gone red. Some of those 716 are expected-output
strings rather than inputs; they are kept, counted, and not distinguished, and
both parsers must agree on how to recover from them.

**Two of the sixteen mutations in §H3 are invisible to the 755,821 real bytes**,
and both are caught by the fixtures. Measured by running each mutation against
the corpus tests alone and counting where the disagreements landed:

| mutation | corpus files | hand fixtures | mined |
| --- | ---: | ---: | ---: |
| #3 parenthesised-pattern span | **0** | 1 | 0 |
| #14 the `>=` split | **0** | 1 | 1 |
| #1 dropped `where` constraints | 1 | 1 | 1 |
| #5 `+` binds as tightly as `*` | 3 | 0 | 1 |
| #12 `pub` dropped | 14 | 0 | 11 |

Only three of the 21 real files carry a `+` next to a `*` in a way that
distinguishes the two binding powers, and exactly one carries a `where`
constraint. A corpus is not a test suite even when it is three quarters of a
megabyte.

---

## §H9 Four areas, one directory, no edits — and what that says about the split

`lexer + spine + types + patterns + exprs + items` typechecked and passed 112
in-language tests with **no edit to any area's file**, and then agreed with the
reference over the whole corpus with no edit either. The AST's sort DAG really is
the shape of the split, as `GAPS-items.md` says.

What it cost anyway, from the four hand-offs: two agents wrote a `spine.ply`
that was thrown away (about 35 minutes between them) because the areas were
launched before the spine existed; one agent built a complete standalone
spine+types+patterns to make its own area verifiable and discarded all of it
(about half a session); and one area's hand-written expectations had to be
reconciled against another's dump tags. **The parallel split was free at the
seams and expensive at the start.** If this is run again the spine wants to be
built and published before the areas begin, which is what the build order said.

---

## §H10 What is still missing, ordered by what it would buy

1. **`effect_set::expand`.** 521 lines of Rust; would remove the only tolerance
   in the harness and bring `examples/desk.ply`'s rows and 7 mined fixtures into
   the unprojected comparison. It is also the last unported piece of
   `ply_syntax`'s parse phase, so porting it would make "the parser is ported"
   true without a footnote.
2. **Diagnostic messages.** `TokenKind::describe`'s ~40 arms plus reading the
   `what: Bytes` already carried at every site. Would turn 833 compared
   diagnostics from "code + spans + shape" into the whole diagnostic.
3. **More error-path fixtures.** 3.2% by bytes is 21x better than the lexer
   spike and still thin. The specific gap §H3 found — a decision reached by no
   input in a 763-input corpus while tag coverage read 92 of 92 — is unlikely to
   be the only one. A per-*site* coverage count (which of the parser's ~120
   diagnostic call sites is reached) would find the rest; tag coverage will not.
4. **The other phases.** §H5's multiplier is lexer→parser. The next term is
   `crates/ply-syntax/src/resolve.rs` at 1,347 lines — 71% of `parser.rs`, so on
   §H6's writing-cost ratio a Ply port of it would be another ~2,600 lines — and
   after that inference, which is larger than both. Until one of them exists,
   §6.2's lexer→**front-end** multiplier stays at least two terms short, and §H5
   is the first of them and nothing more.
