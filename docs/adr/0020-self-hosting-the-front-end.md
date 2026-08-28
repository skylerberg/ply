# ADR 0020 — Self-hosting the front end

Status: **rejected for now, with two pieces of the spike kept.**

> **Why anyone wanted this:** [ADR 0021](0021-why-bootstrap.md). This document
> prices whether Ply can host its own front end and decides that it cannot yet.
> It does not state the goal, and read alone it is a rejection with nothing
> behind it. 0021 records the claim — Ply's verification loop is O(the change)
> where every toolchain it competes with is O(the project) — and the four
> preconditions that would change this answer.

- **Rejected:** writing Ply's front end in Ply on today's interpreter. §6 prices
  it. The blocker is throughput, not expressiveness, and the compiled fragment
  cannot be assumed to close the gap — §6.3 says why, and what would measure it.
- **Accepted:** §1's finding is a defect in shipped code and is tracked as one,
  not as a spike observation. `crates/ply-std/ply/json.ply`'s `escape_runs` is
  quadratic in a client-chosen input; re-measured on the shipped module in §4.1.
- **Accepted:** the differential-harness pattern in `spikes/ply-lexer/harness`
  is the right shape for pricing any future port, with the amendment in §3.2.
- **Not decided here:** whether the fragment should grow to cover a front end.
  §6.3 states the one measurement that would settle it and does not take it.

This ADR reviews the spike at `spikes/ply-lexer/`, merged as `73ebd1c`. It was
written by re-running the spike rather than by reading its write-up. Where a
claim of the spike's is repeated here it was re-checked; where it was refuted,
the refutation is in place with the withdrawn text quoted.

---

## §0 What this reviewed, and the instrument problem it started with

The spike ports `crates/ply-syntax/src/lexer.rs` to Ply (`lexer.ply`, 647
lines), and compares the two lexers token by token over 33 `.ply` files. Its
deliverable is `GAPS.md`.

**The first thing found was not about lexers.** `crates/ply-eval/src/frame.rs`
carried a modification timestamp of `14:30:37` on a tree whose every other
source file read `14:05:23`, and `target/release/ply` — the binary behind every
wall-clock number in `GAPS.md` — was built at `14:31:21`, fifty-four seconds
later. `frame.rs` holds four of the eight `ply_eval::rc::carry` call sites, and
`carry` is the mechanism `GAPS.md` §1 rests on. The change was an unattributed
edit making a field projection out of a uniquely-owned record *move* the field
rather than clone it: the precise operation §1 measures.

So §1 was measured with an instrument that had been altered in the place it was
measuring. That is not a hypothetical: it is the W1/M8 shape — a result whose
provenance nobody checked because the result looked right.

It was then re-taken three times on a clean binary, by two parties, and it
survived. §4.1 gives the third of those. The file in the merged tree is clean:
`grep -c "Arc::get_mut" crates/ply-eval/src/frame.rs` is **0** and `frame.rs:281`
reads `Some(v) => self.go_return(v.clone())`.

**The lesson is worth more than the outcome.** The finding was correct and the
instrument was not, and only one of those two was checked before the work was
merged. Nothing in this repository's loop would have caught it: there is no CI,
`cargo test --workspace` does not reach `spikes/`, and a modified working tree
is invisible to every command in `CONTRIBUTING.md` §"The loop".

---

## §1 What was verified

Re-run in `~/.worktrees/ply/lexer-verify` at `73ebd1c`, release binary built
from that tree.

| check | result |
| --- | --- |
| `ply test spikes/ply-lexer/lexer.ply --no-cache` | **15 passed, 0 failed**, 0.03 s |
| `cd spikes/ply-lexer/harness && cargo test` | **22 passed, 0 failed** (18 integration, 4 lib), 4.71 s |
| agreement corpus | **33 files, 768,760 bytes** — re-counted with `wc -c` |
| token stream | **120,490 tokens + 25 diagnostics** |

The token count was re-derived independently rather than quoted. Dumping all 33
files with `plydump` and counting record kinds gives punctuation 69,347,
identifiers 39,429, keywords 5,256, strings 3,055, integers 2,483, byte strings
766, decimals 97, EOF 33, floats 24 — which sums to **exactly 120,490** — plus
25 diagnostics. The spike's figure is right to the token.

The code citations behind §1's mechanism were checked and all hold:
`rc::carry` at `crates/ply-eval/src/rc.rs:98`; `Env::take_unique_inner`'s
`Rc::get_mut` refusal with its *"Refuses at the first shared link"* comment at
`env.rs:133`; `DEFAULT_MAX_CALLS = 10_000` at `limit.rs:35`; and **exactly
eight** `carry` call sites — `handler.rs:208`, `machine.rs:1007/1064/1094`,
`frame.rs:107/142/263/301`. `crates/ply-std/ply/json.ply:589-599` is as
described: the inner `push(acc, ..)` is argument 0 of 2 of `escape_runs`.

### §1.1 The comparison is armed — checked with corruptions of my own

A green agreement over a comparator that cannot go red is this project's
signature defect, and the spike's own six mutation tests are not evidence that
*mine* would be caught. Two fresh corruptions of `lexer.ply`, neither used by
the spike (`++` → `plus`) nor by the pre-merge check (`{` → `|`):

| corruption | axis | result |
| --- | --- | --- |
| `emit(done, n, n, TEof)` → `emit(done, n - 1, n, TEof)` | span only; kind and payload identical | **caught** |
| `t == b"handle"` deleted from `is_keyword` | kind and payload | **caught** |

A span-only corruption is the one a token-stream comparator is usually blind to.
This one is not.

---

## §2 What the spike proved

1. **Ply can express a lexer for its own language, to the token.** Agreement is
   on spans, payloads *and* diagnostics across 768,760 bytes, including the
   whole shipped standard library and the largest file in the tree.
2. **It can lex itself.** `lexer.ply` (26,404 bytes, containing a 1,024-character
   byte-table literal) is lexed by `lexer.ply` without error. This is the
   canonical self-hosting question for a lexer and it was not in the spike's own
   test set; it was run here and it passes.
3. **§1's finding is real, is positional, and is already being paid in shipped
   code.** Three independent measurements now agree, and the shipped instance
   reproduces on the real `std.json` module (§4.1).
4. **The type checker carried the port.** `ply check` passed first try on a
   647-line hand port of 1,069 lines of Rust. That is a real result about the
   language and it is easy to overlook next to the gap list.
5. **The divergence is honestly bounded.** The two lexers differ at exactly one
   decision point — `char::is_alphabetic`/`is_whitespace` at a token's first
   byte — and the three shapes are pinned by exact-dump tests rather than
   described.

---

## §3 What the spike did not prove

The agreement figure is 768,760 bytes. That number does a lot of rhetorical work
and the coverage underneath it is narrower than it sounds.

### §3.1 The error paths are 0.15% of the corpus

All **25** diagnostics in the entire corpus come from the 10 hand-written
fixtures, which total **1,122 bytes**. Not one of the 23 real `.ply` files in
the tree raises a single lexer diagnostic. The spike's README says the fixtures
exist because no real file reaches the error paths, so this is disclosed — but
the disclosure and the 768,760 figure appear in different paragraphs, and half
of what `lexer.rs` does is error handling.

Float coverage is thinner still: **24 float tokens in the whole corpus**, 12 of
them in `fixtures/numbers.ply` and 12 in `crates/ply-std/ply/json.ply`. Every one
is short — `1.5`, `2.0`, `1e9`, `1.5e-3`. Decimals: 97.

**Demonstrated rather than counted (second verification pass, 2026-08-24).** The
paragraphs above are a census, and a census of a corpus is an argument about what
that corpus *could* catch. The claim underneath it was made biting instead. Both
sites in `lexer.ply::punct` that raise a diagnostic —
`err(seek(s, start + 1), start, start + 1, unexpected())`, the lone `&` and the
unrecognised byte — were replaced by `seek(s, start + 1)`, so the mutant raises
neither. **The token stream is byte-identical**; the mutant differs from the real
lexer only in that it silently accepts what `ply_syntax` refuses, which is the
one failure a token-only comparison is built to miss and the reason the dump
carries diagnostics at all.

| agreement test | corpus it reads | verdict against the mutant |
| --- | ---: | --- |
| `..._on_every_example` | 13 files, 333,595 bytes | **passed** |
| `..._on_the_kernel_benchmarks` | 2 files, 14,634 bytes | **passed** |
| `..._on_the_shipped_standard_library` | 8 files, 419,409 bytes | **passed** |
| `..._on_the_hand_written_edge_cases` | 10 files, **1,122 bytes** | **FAILED** — `rust: Some("84:85:!:E0001")`, `ply : None` |

So the 767,638 bytes of real source cannot tell the real lexer from one that
never raises a punctuation diagnostic at all. The 1,122 bytes of hand-written
fixtures are the entire difference between that mutant and a green board — a
ratio of **684 to 1** by weight, on half of what `lexer.rs` does. The spike
disclosed the shape of this; what was missing was that it is not a caveat about
breadth but a load-bearing dependency on ten small files, and any future port
should size its fixtures against that rather than against the corpus.

`lexer.ply` was restored byte-identical afterwards (`cmp` clean, 15 Ply tests and
22 harness tests green).

### §3.2 The float comparison cannot see the digits, and the README says it can

`spikes/ply-lexer/README.md` states that what the comparison checks is that the
Ply lexer *"classified the literal as a float, spanned the same bytes, and
extracted the same digits."*

> **Withdrawn: "and extracted the same digits".** It does not check that. The
> harness runs the Ply lexer's float text through Rust's `f64` parser
> (`harness/src/lib.rs::floats_to_bits`) before comparing, so any digit string
> that rounds to the same `f64` passes. What is checked is the *value*, not the
> digits.

Demonstrated, not reasoned, with two further corruptions of `lexer.ply`:

| corruption | what it does to every float in the input | result |
| --- | --- | --- |
| `float_text` appends `"0"` to the fraction | `1.5` → `1.50`, `1.5e-3` → `1.50e-3` | **AGREE** |
| `float_text` truncates the fraction to 17 significant digits | `1.00000000000000000000001` → `1.00000000000000000` | **AGREE** |

The second is an ordinary fixed-buffer bug and it is invisible. The consequence
is bounded — for a *lexer* the observable output is the `f64`, and the value is
checked — but it stops being bounded the moment a self-hosted front end converts
that text itself, which §5 of `GAPS.md` says it must eventually. The harness as
built cannot certify the digit extraction it is relied on to certify.

**One thing this found that came back green.** `1e400` — the inf-saturation case
`GAPS.md` §3 is written around — appears in no corpus file and in no fixture. It
was run here: both lexers produce `7ff0000000000000`, and the comparison agrees.
Unexplored space, now explored, no defect.

### §3.3 The Ply lexer cannot lex the files the harness writes to feed it

The harness embeds each corpus file in a generated `probe.ply` as a `b"..."`
literal. For `examples/desk.ply` that literal is 173,451 bytes and holds **5,902
escapes**. `ply_syntax::lexer` lexes it into 27 tokens. `lexer.ply` cannot lex it
at all:

```
E0502 recursion limit of 10000 nested calls exceeded
```

So the lexer agrees with the reference on 33 files and cannot process the 33
files that were written in order to ask it about them. This is not a defect the
spike introduced — it is `GAPS.md` §6's second bullet, which says
`string_lit`/`bytes_body` recurse once per escape and calls the corpus margin
"a fact about the corpus, not about the lexer". It is worth stating in the
stronger form, because the corpus that refutes it is the spike's own scaffolding.

Both cliffs were located rather than estimated:

| shape | survives | fails |
| --- | --- | --- |
| consecutive comment lines (`skip_trivia`) | 9,000 | 10,000 |
| escapes in one literal (`string_escape`) | 4,500 | 5,000 |

Against a shipped corpus whose deepest trivia run is 135 lines and whose largest
single literal holds **256** escapes (the byte tables in `json.ply`,
`router.ply` and `lexer.ply`). The margin on real code is **18x** for escapes
and **67x** for trivia runs. The margin on generated code is negative.

---

## §4 The gaps, ranked by what they cost

The spike's `GAPS.md` numbers its fifteen entries in the order it met them. This
is the order they cost, and it separates the three kinds the review was asked to
separate.

**Notation:** a bold **§N** leading an item in this section is `GAPS.md`'s
section N, not this ADR's. Plain §N references elsewhere are this ADR's.

### §4.1 The language expresses it, and slowly — and this one is a shipped defect

**§1, the positional trap.** A growing container must be built in the last
sub-expression of its enclosing node or the program is quadratic. Re-measured
here from a reproduction written off the prose rather than copied, on the clean
binary, **user CPU** as the primary statistic because it survives contention far
better than wall clock, minimum of 3, load 33→44:

| n | `toks` third of five | `toks` last of five | `toks` third, others `let`-bound first |
| ---: | ---: | ---: | ---: |
| 8,000 | 0.28 s | 0.02 s | 0.29 s |
| 16,000 | 1.09 s | 0.05 s | 1.12 s |
| 32,000 | 4.34 s | 0.09 s | 4.37 s |

Against a rule fixed before any data existed (`/tmp/verify-preregistration.md`,
written at load 85.87 before the binary finished building): accept if the
non-final column is ≥ 3.0x per doubling at two consecutive doublings and the
final column ≤ 2.5x. Measured **3.89x and 3.98x** against **2.5x and 1.8x**.
**Accept.**

The third column is the important one. It reproduces `GAPS.md`'s own correction:
binding every other field to a `let` before the push makes `s.toks` the last
*mention* of `s` and the program stays quadratic. The rule is positional in the
enclosing node, not about the variable. Two people have now written the
last-mention explanation down and both were wrong; the corrected text is in
`GAPS.md` §1 and in `spikes/ply-lexer-rc/fieldorder.ply` lines 9-20.

> **Residual, not corrected by the merge.** `fieldorder.ply:46` and `:50` still
> carry the inline comments *"`s.toks` is not the last mention of `s`"* and
> *"`s.toks` is the last mention of `s`. Nothing else changed."* — the withdrawn
> framing, restated as the salient difference, twenty-six lines below the block
> that withdraws it. Both are true of those two functions and neither states the
> rule. Worth a one-line fix.

The call-argument form confirms it is not record-specific — `sink(push(xs,i),i,i)`
at 0.28/1.09/4.33 against `sink2(i,i,push(xs,i))` at 0.01/0.03/0.06, user CPU,
same conditions. (The fast column's first doubling is 3.0x, above my threshold,
at 0.01 s where timer granularity dominates; the second is 2.0x. Stated rather
than smoothed.)

**It is being paid in shipped code, and this was re-measured on the shipped
module rather than a copy.** A program calling `json::encode_string` on a string
of *k* characters that all require escaping, running the real
`crates/ply-std/ply/json.ply`, user CPU, min of 3, load ~41:

| k escapes in one string | user |
| ---: | ---: |
| 1,000 | 0.03 s |
| 2,000 | 0.07 s |
| 4,000 | 0.22 s |
| 8,000 | 0.79 s |

Ratios 2.33x, 3.14x, 3.59x — approaching 4x as the linear start-up term washes
out. `GAPS.md` measured a standalone reproduction and got 0.06/0.22/0.81 at
k = 2,000/4,000/8,000; the shipped module gives 0.07/0.22/0.79. **The spike's
figure is confirmed on better evidence than the spike had.**

This is the highest-cost item in the whole gap list and it has nothing to do
with self-hosting. It is quadratic behaviour in the standard library's JSON
serializer, in the length of a string, and a served response that echoes
attacker-influenced text through `encode_string` pays it. **Not traced to a
concrete request path here** — that is the next step, not a claim.

### §4.2 The language cannot express it

**§3, `Float`.** There is no `float_of_string`, no `float_to_string`, no
`parse`; `float_of_decimal` cannot reach `inf`. Confirmed against the builtin
table: the numeric surface is `decimal_of_string`, `decimal_to_string`,
`decimal_of_int`, `decimal_of_float`, `int_of_decimal`, `float_of_decimal`,
`decimal_round`, `decimal_div` and nothing else. A Ply lexer therefore cannot
produce `TokenKind::Float(f64)`; it produces the literal's text and something
else converts it. This is the one item in `GAPS.md` that is a hole rather than a
tax, and §3.2 above shows the harness cannot check the substitute.

**§8, no file IO.** No shipped effect has a file operation, so a source file
reaches a Ply program as a literal or not at all. Cheap today (§3.3 shows the
literal costs 0.01 s to parse) and absolute: a self-hosted front end needs
either a file effect or a Rust driver that hands it bytes.

### §4.3 Merely unfamiliar — and one of them is simply wrong

**§4, the numeric bounds.** `GAPS.md` says:

> *"Ply has **no `int_of_string`**. `decimal_of_string` exists but tops out at 28
> significant digits, which is below both bounds. And Int arithmetic is
> *checked* (`interp.rs:1215`, `checked_add`), so the usual trick — accumulate
> and look at the sign — raises before the overflow can be observed."*

> **Withdrawn: "which is below both bounds".** The first sentence is wrong and
> the workaround it justifies is largely unnecessary. The real signatures are
> `decimal_of_string : (String) -> Option<Decimal>` and
> `int_of_decimal : (Decimal, Rounding) -> Option<Int>`
> (`crates/ply-core/src/numerics.rs:285-288`, and `string_of_bytes : (Bytes) ->
> String` bridges the lexer's `Bytes`). Both answer an `Option`, so neither
> raises and nothing needs to be accumulated. Run on the six boundary values
> `fixtures/numbers.ply` tests:
>
> | input | result |
> | --- | --- |
> | `9223372036854775807` (`i64::MAX`) | `Some(Int 9223372036854775807)` |
> | `9223372036854775808` | Decimal fine, `int_of_decimal` → **`None`** |
> | `99999999999999999999` | Decimal fine, `int_of_decimal` → **`None`** |
> | `79228162514264337593543950335` (mantissa max) | Decimal fine |
> | `79228162514264337593543950336` | `decimal_of_string` → **`None`** |
> | `1000000000000000000000000000000` | `decimal_of_string` → **`None`** |
>
> Those are exactly the two bounds `lexer.rs` decides with `parse::<i64>()` and
> `parse::<i128>().filter(|m| *m <= (1<<96)-1)`. `decimal_of_string` does not
> "top out below both bounds": it accepts the full 29-digit mantissa maximum and
> declines one above it, which *is* the mantissa test, and 19 digits is well
> inside it, which makes `int_of_decimal` the other test.
>
> **The second sentence is true and this review first called it wrong.** Int
> arithmetic *is* checked (`crates/ply-eval/src/interp.rs:1215`,
> `BinOp::Add => a.checked_add(b)`, verified), so accumulate-and-look-at-the-sign
> genuinely does raise before the overflow is observable. It is simply not
> load-bearing, because accumulating is not the only route to the value.

Consequence, stated narrowly because the first draft of this paragraph
overstated it: `int_max()`, `dec_max()`, `int_of_digits()` and the two
comparison arms at `lexer.ply:345` and `:371` — roughly ten lines — are
replaceable by two builtin calls that already exist.

> **Withdrawn: "`int_max()`, `dec_max()`, `strip_zeros` and `int_of_digits` ...
> about 25 lines of `lexer.ply` ... exist for no reason."** `strip_zeros` is not
> only a bound-check helper: `lexer.ply:369` uses it to build the `TDec`
> mantissa payload, which is a digit string and has no builtin. It stays. And
> the four functions are about fourteen lines, not twenty-five.

This is a **discoverability** gap, not a language gap, and it belongs in a
different bucket from §1 and §3. It is the clearest instance of the review's
third category, and it was found by running the builtins rather than by reading
the gap list — which is also how the overstatement above was found, one step
later.

**§13, no dispatch mechanism.** `GAPS.md` records this as a negative result and
it is right to: a lexer has nowhere that wants open dispatch.

**§14, mutable state.** Never reached for. Also a negative result.

### §4.4 Real taxes, correctly ranked as taxes

§2 (no record update — `http.ply:1016-1029` writes 13 `Limits` fields to change
one, confirmed), §7 (no `byte_of_int`; the 1,024-character table appears in
`json.ply:627` and `lexer.ply:29` for the identical reason), §9 (no tuples),
§10 (the `List` surface is `len/push/map/filter/fold/range` — confirmed against
the builtin table: no index, concat, reverse, prepend or sort), §11
(`bytes_at_or`, still **not measured**, as `GAPS.md` says).

§12's count was re-derived: **57 of 129** `fn` definitions in `json.ply` return a
`Result`, against the 58 `GAPS.md` reports with an explicit "close rather than
exact" hedge. The hedge is honest and the figure stands.

### §4.5 Five stale claims in the merged tree, all fixed while this was written

> **The count in this heading moved twice, which is the point of the section.**
> It read *"One stale claim in the merged tree"*, then *"Two stale claims in the
> merged tree, both fixed while this was written"*. Each re-read of the spike
> found another. Three of the five are in `lexer.ply` and are recorded at the
> foot of this section; the two below were the ones the first passes saw.

`spikes/ply-lexer/harness/tests/agreement.rs:241-245` opened:

> *"The corpus above is every `.ply` file in the tree and **all of it is ASCII**
> — checked, and the reason this test exists."*

Both halves are false. The corpus is 33 of the **109** `.ply` files in the tree
outside `spikes/` (`find . -name '*.ply' -not -path './target/*' -not -path
'./spikes/*' | wc -l`), and it holds **1,543** non-ASCII bytes — a figure
`every_non_ascii_byte_in_the_corpus_is_somewhere_both_lexers_agree` pins with an
`assert_eq!` at `:314`, in a test carrying two correction blocks of its own that
say so. The spike withdrew this claim in its README and in that test's doc
comment and left it standing in the doc comment one test above.

`GAPS.md` §5 called `crates/ply-std/ply/db.ply` *"the largest `.ply` file in the
tree ... is **29,212**"*. It is the file with the most **tokens** (29,213 by
`plydump`) and the second largest by **bytes** (135,285 against `desk.ply`'s
159,683). The claim §5 rests on — that a recursive scanner dies a third of the
way through it — holds either way.

> **Both were corrected in the tree during this review**, by the author of the
> spike, with the withdrawn text quoted in place, so the present tense above
> describes the tree as it was reviewed rather than as it now stands. Recorded
> in the past tense here because an ADR that silently describes a fixed defect
> as live is the same failure in the other direction.

**Three more, all in `lexer.ply`, none of them found by the spike's own review
or by the first pass of this one.** They were corrected in place during a second
verification pass, withdrawn text quoted beside each:

| line | withdrawn | actual |
| --- | --- | --- |
| `:16-19` | *"Tokens accumulate in a `Map<Int, Token>` and not a `List<Token>`, because `push(s.toks, t)` where `s` is a record is **quadratic**: the field read leaves the list aliased by the record it came out of, so `push` copies rather than updating in place. Measured; `GAPS.md` §1."* | the accumulator **is** a `List<Token>` (`Scan`, `:93`), and the mechanism given is the one `GAPS.md` §1 spends a correction block withdrawing |
| `:14` | *"`examples/desk.ply` alone is 24,847 tokens"* | **19,576**, which is what `GAPS.md` §5 and §15 both say |
| `:146` | *"The deepest run in the corpus this is checked against is 136 lines"* | **135**, which is what `GAPS.md` §6 says |

The first is worth more than the other two together, and it is the strongest
single illustration in this review of why the repository keeps finding this
defect. It is `lexer.ply`'s **header comment** — the first prose a reader of the
file meets — and it does not merely name the wrong data structure. It restates,
in the voice of a measurement and with a citation to `GAPS.md` §1, precisely the
explanation that `GAPS.md` §1 exists to withdraw. The corrected statement was
already in the same file, 60 lines below, on `Scan`. So the spike carried the
withdrawn claim and its withdrawal simultaneously, with the withdrawn one first
and better placed, and three reviews read past it — because a header comment
reads as orientation rather than as a claim, and nothing in this project's loop
treats it as one.

The correction to `agreement.rs` also closed a hole this review had left open.
The 86 uncompared files hold **95,419** bytes, and one of them —
`tests/fixtures/unterminated_string.ply` — is the only file outside the spike
that raises a lexer diagnostic. It was compared by hand and the two lexers agree
on it exactly, `E0002` included. That is a real narrowing of §3.1's gap: the
error-path evidence is now ten fixtures **and** the one real file in the tree
that reaches an error path, rather than ten fixtures alone.

---

## §5 The parser, which is the next component and is harder

The lexer port was function-for-function. **A parser port cannot be**, and that
is the finding of this section rather than a list of aggravations.

### §5.1 The gap that changes category

> **Its premise is withdrawn by ADR 0022 (2026-08-27).** This section's two
> load-bearing sentences were:
> *"A recursive-descent parser consuming N top-level definitions, or N list
> elements, or N arguments, recurses once per element unless it is folded"* and
> *"The lexer's fold-over-a-range escape hatch does not generalise, because a
> recursive-descent parser's recursion **is** the grammar."*
>
> Neither holds of the reference implementation this repository ships.
> `crates/ply-syntax/src/parser.rs` drives **every** sequence with a loop — 16
> `while` and 5 `loop`, one per sequence, with the shared `comma_list`
> (`:2059`, loop at `:2065`) called from **fourteen** sites covering argument
> lists, list and record literals, parameters, generic arguments and pattern
> arguments. It even climbs precedence iteratively (`bin_expr` `:1222`, `while`
> at `:1224`), recursing only for the right operand, so its depth is bounded by
> the six binding powers `bin_op` (`:2096`) declares rather than by operand
> count — which also withdraws *"at perhaps 15 precedence levels that is ~255
> frames"* below.
>
> It reserves recursion for grammar nesting, and **bounds that itself**:
> `const MAX_DEPTH: u32 = 128` (`parser.rs:23`), enforced by `deeper()` (`:244`)
> at `ty_inner` (`:1035`), `unary_expr` (`:1244`) and `pattern` (`:1846`). Against
> the corpus maximum of 17 this section measures, and a ceiling of 10,000,
> grammar nesting in this design cannot reach the ceiling — the parser refuses
> at 128 first.
>
> ADR 0022 also adds `iterate`, an early-terminating loop that is depth 1 on
> both engines, so the escape hatch generalises further than "fold over a
> range": it no longer has to run to a conservative bound. See ADR 0022 §2 for
> the citations, re-verified, and §3 for `fold` at 500,000 elements in 22.3 MiB
> at depth 1.
>
> **What is not withdrawn:** §6's throughput finding, which is this ADR's actual
> reason for deciding against self-hosting today. §5.1 was a second, independent
> objection; only that one falls.

**§5, the 10,000-call ceiling, stops being a tax and becomes an architecture
constraint.** The lexer escaped it with `fold(range(0, n + 1), start, one)` — a
loop over an eagerly materialised list of integers, driven by the machine's step
protocol so it nests nothing. That trick works because lexing is a *flat* state
machine: one step per byte, no nesting.

Recursive descent is not flat. Two separate depths, and both are bounded by
10,000:

- **Grammar nesting.** Measured across the 23 real `.ply` files (the 10
  fixtures are too small to matter): the deepest bracket nesting is **17**
  (`desk.ply`), and 12 or less everywhere else. At perhaps 15
  precedence levels that is ~255 frames. Comfortable.
- **Sequence recursion, which is the problem.** A recursive-descent parser
  consuming *N* top-level definitions, or *N* list elements, or *N* arguments,
  recurses once per element unless it is folded. `desk.ply` is 19,576 tokens.
  Any parse function that recurses per token or per item dies at 10,000, and
  there is no flag: `grep max_calls crates/ply-cli/src/` returns exactly one
  line, `engine.rs:244`.

The lexer's fold-over-a-range escape hatch does not generalise, because a
recursive-descent parser's recursion *is* the grammar. To stay under the ceiling
a Ply parser must be an explicit-stack pushdown automaton with its state in a
fold accumulator — a different program from `crates/ply-syntax/src/parser.rs`,
which cannot then be ported function-for-function and cannot be differentially
compared function-for-function either.

### §5.2 Gaps that get worse

- **§1 gets worse in two ways and better in one, and the first draft of this
  bullet had it wrong.**

  > **Withdrawn: "§1 gets much worse ... a parser has one such site per grammar
  > production", asserted with no measurement behind it.** The direction is
  > right and the magnitude was guessed. Measured below.

  `spikes/ply-lexer-nesting/nesting.ply` — written by another lane during this
  review, and run here rather than cited — isolates the two questions a parser
  raises. User CPU, min of 3, load 44–54:

  | where the *caller* puts a correctly-written callee | n=8,000 | n=16,000 | |
  | --- | ---: | ---: | --- |
  | the whole body — nothing follows it | 0.02 s | 0.04 s | linear |
  | argument 0 of 2, followed by a variable | 0.31 s | 1.13 s | **quadratic** |
  | argument 0 of 2, followed by a **constant** | 0.31 s | 1.15 s | **quadratic** |
  | argument 1 of 2 — nothing follows it | 0.03 s | 0.05 s | linear |

  **The rule composes, and it is not local.** `node` is written correctly in all
  four rows — its `push` is the last sub-expression of its own record literal —
  and the caller destroys it anyway. The third row is the sharp one: `carry` is
  `if remaining { env.clone() }` and never asks what the remaining
  sub-expression *reads*, so a literal `0` sitting after the call is enough to
  make the program quadratic. For a parser this means a correct combinator gives
  its caller no protection, and the thing that removes the protection can be a
  constant.

  **What gets better** is the grouping. The copy is O(the list's current
  length), so the cost is O(k·m) for k pushes spread over lists of length m:

  | k pushes, all in the non-final position | k=8,000 | k=16,000 | |
  | --- | ---: | ---: | --- |
  | into one list (the lexer's shape) | 0.29 s | 1.12 s | **quadratic** |
  | into lists of ten (a parser's shape) | 0.02 s | 0.03 s | linear |

  A lexer accumulates one list of every token in the file — m = k = 19,576 for
  `desk.ply`, the worst case. A parser accumulates mostly short lists: a block's
  statements, a call's arguments, a match's arms. So per site a parser is far
  cheaper, and the honest statement is that **a parser trades one catastrophic
  accumulator for hundreds of cheap ones plus a non-local rule** — except for
  whatever is module-wide, which is the lexer's shape again.
- **§9 (no tuples) becomes the dominant shape.** Every parse function returns
  "a node and the next index". That is the single most common type in a
  recursive-descent parser and it is a record declaration each time.
- **§12 stops being free.** `GAPS.md` §12 records that error accumulation cost
  nothing *because a lexer never fails* — it answers with tokens beside
  diagnostics. A parser with error recovery does fail, and `json.ply` is the
  preview: 57 of 129 functions returning `Result`, no `?`, no do-notation,
  hand-written `decode_map`/`decode_and_then`, and one number literal split
  across seven functions purely to bind an `Ok`.
- **§10 (the `List` surface)** starts to bite: list patterns, argument lists and
  match arms want index, `nth` and reverse, and there are none.

### §5.3 A gap a lexer cannot hit

**Value depth.** `MAX_VALUE_DEPTH = DEFAULT_MAX_CALLS = 10_000`
(`crates/ply-eval/src/limit.rs:45`) bounds how deep a value may nest before a
*structural walk* over it refuses. The lexer's `Tok` is flat — nine variants, no
recursion — so nothing in the spike touches this. An AST is recursive by
definition, and every comparison, hash and render of it is a structural walk.
The bound is generous relative to the measured nesting of 17, but it is a
distinct ceiling from the call ceiling and the lexer provides no evidence about
it.

---

## §6 What the front end would cost, and whether that is acceptable

### §6.1 Measured

All figures: release binary from `73ebd1c`, front-end cache cleared before each
run, **user CPU seconds** as the primary statistic and wall clock beside it, with
the load recorded. User CPU is used because this machine carried three other
agent worktrees running test suites throughout and its 1-minute load average
ranged 17–88; wall clock at that contention is not reproducible and user CPU
very nearly is. Minimum of N, N stated.

**The Ply lexer, four files, min of 3, load 41–45:**

| file | bytes | tokens | user | KB/s | tokens/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| `examples/desk.ply` | 159,683 | 19,576 | 1.19 s | 131 | 16,451 |
| `crates/ply-std/ply/db.ply` | 135,285 | 29,213 | 1.53 s | 86 | 19,094 |
| `crates/ply-std/ply/http.ply` | 124,749 | 17,539 | 1.13 s | 108 | 15,521 |
| `crates/ply-std/ply/json.ply` | 63,370 | 11,668 | 0.69 s | 90 | 16,910 |

KB/s varies by 1.5x across the four; **tokens/s varies by 1.2x**, so tokens is
the right unit and the figure is **~17,000 tokens/s**.

`GAPS.md` §15 reports 85 KB/s and 10,470 tokens/s for `desk.ply`, taken as wall
clock at load 13–18. Re-taken here: 1.76 s wall (against its 1.87 s) and 1.19 s
user. **The spike's figure survives**; it was a wall-clock figure under load and
is about 1.6x pessimistic against the load-corrected one. The `bytes_len`
control re-takes at 0.00 s user / 0.01 s wall, confirming that essentially all
of it is the Ply lexer running.

**The Rust front end, for scale, min of 5, load 37:** `ply check examples/` —
lex, parse, resolve, typecheck, effect-infer and content-hash 13 files and 21
modules, 333,595 bytes of project source and 45,041 tokens — is **0.21 s user /
0.28 s wall cold**, and **0.03 s user / 0.04 s warm**.

> `GAPS.md` §15 reports 0.43 s for this at load ~29. The re-take is 0.28 s wall
> at load 37 — *faster* on a busier machine. The spike's figure was pessimistic;
> correcting it makes the comparison worse for self-hosting, not better.

That is **~215,000 tokens/s for the whole front end**, against ~17,000 tokens/s
for a Ply lexer alone: **12.6x slower for the first phase of six.**

### §6.2 Assumed

To get from a lexer to a front end needs a multiplier, and there is no way to
measure one without writing the rest. **This is an assumption and it is labelled
as one.** In conventional compilers lexing is 10–20% of front-end time, which
puts a full Ply front end at 5–10x the lexer's cost — call it 1,700–3,400
tokens/s. If anything that is optimistic for Ply specifically, because the
phases after lexing are the ones that build records and lists, which is where
§1's tax and §9's absence land.

At that rate `ply check examples/` — 45,041 tokens, 0.21 s today — becomes
**13–27 seconds**. Roughly **60–130x**.

That is the answer to whether this is acceptable for the loop it is meant to make
fast, and it is not close. `CONTRIBUTING.md` §"The loop" puts
`./target/debug/ply test examples/` at 0.31 s warm; the verification loop this
work exists to accelerate is currently sub-second, and a self-hosted front end at
today's interpreter speed would make it the slowest thing in the build by two
orders of magnitude. **The incrementality argument does not rescue it either:**
the warm path is 0.03 s, so the cache is already buying 7x, and a self-hosted
front end would have to be cached by machinery that is itself in Rust.

### §6.3 The compiled fragment does not obviously close this, and the reason is specific

The natural objection is that the fragment being built in parallel makes this
moot. It does not, and the reason is worth stating precisely because it prices
that work.

`crates/ply-codegen-spike` compiles a Ply definition to native code through
cranelift, and `benches/w6-spike-r4.json` supports **11.68x** on `read_line`-shaped kernel
code. That file holds raw microsecond pairs, not a ratio: 11.68x is the most
conservative expression in it — interpreter *best* against spike *worst*,
minimised across its five inputs (`5.8615625 / 0.5019375`). The optimistic
reading of the same file is 14.2x. Re-derived here from the file rather than
quoted from `CONTRIBUTING.md`. Applying that to 1,700–3,400 tokens/s would give 20,000–40,000
tokens/s and a 1.1–2.3 s `ply check examples/` — still **5–11x** worse than
today's 0.21 s. So even at its full measured speedup the fragment does not make a
self-hosted front end competitive on the cold path.

**And the speedup should not be assumed to transfer at all.** I first wrote that
the fragment lowers no `Bytes` builtins and would refuse a lexer outright. That
is wrong and is withdrawn:

> **Withdrawn: "the fragment lowers no `Bytes` builtins, so a lexer is outside
> it."** There is a generic path. `jit.rs:508 admissible_builtin` refuses only
> higher-order builtins, `cell_get`/`cell_set` and `secret_of_string`; every
> other builtin, `bytes_at` and `bytes_scan` included, is admitted and
> dispatched through the `rt_builtin` helper (`jit.rs:295`, `:1169-1172`).
>
> > **Line number corrected (2026-08-27, ADR 0022).** `admissible_builtin` is
> > at `jit.rs:537`, not `:508`; the citation above is left as written and
> > corrected here rather than edited. What it refuses is unchanged and now
> > includes `iterate`, whose `Builtin::higher_order()` is true — refused by the
> > same first branch that refuses `fold`, which is expected and is not a
> > regression.

What `rt_builtin` does is the point (`rt.rs:353-372`): it calls
`ply_eval::builtins::call(b, args, ..)` — **the identical interpreter builtin
body**. So compiled code gets native arithmetic, native control flow and no
per-call frame protocol, and pays interpreter price for every byte operation. A
lexer's inner loop is `bytes_at`, `bytes_scan`, `bytes_slice`,
`bytes_concat_all` — builtin dispatch, not arithmetic — which is exactly the half
the fragment does not accelerate. The 11.68x was measured on a workload whose hot
operations *are* the half it does accelerate.

**The transfer was unmeasured. It has now been measured, and the answer is the
one that favours the fragment.**

> **Superseded within this ADR.** This paragraph read: *"So the transfer is
> unmeasured, and this ADR does not guess it. The measurement that would settle
> it is one attribution run splitting the Ply lexer's time between
> `ply_eval::builtins::call` and everything else. If builtins dominate, the
> fragment buys the front end little and needs open-coded `Bytes` primitives
> before it is relevant. If dispatch dominates, the fragment is the right lever.
> That is one profile, it is cheap, and nothing in the tree has taken it."* The
> profile was taken rather than left as a recommendation, because it was named
> as the highest-value decision-relevant measurement in the document and it cost
> six seconds.

`/usr/bin/sample` against the release binary running `lexer.ply` over four
distinct slices of `examples/desk.ply` (distinct so a pure-function memo cannot
collapse them), 1 ms interval, 6 s window, attribution by walking the call graph
and charging each subtree to its outermost matching frame. **Two independent
windows, load ~40:**

| | window 1 | window 2 |
| --- | ---: | ---: |
| samples under `run::evaluate` | 3,930 | 4,909 |
| of those, anywhere under `ply_eval::builtins::*` | 179 — **4.6%** | 200 — **4.1%** |

**Dispatch dominates builtin bodies by roughly twenty to one.** Where the CPU
actually is, by leaf sample (window 1): the machine's own step and dispatch
**43.8%**, reference-counting and `Drop` traffic — `Value`, `Env`, `Chain`,
`pool::link` — **26.5%**, the continuation stack **15.0%**, `malloc`/`free`
**2.8%**, and every builtin body together **1.3%**. `memcmp`/`memcpy`, which is
where a byte-scanning workload's "real work" lives, is **2.8%**.

So the objection this section was written to answer does not hold: a lexer is
*not* builtin-bound. Its cost is the interpreter's per-step protocol and the
refcount churn around it, which is precisely the half the fragment removes. That
makes the fragment the right lever for a front end on this evidence, and it
makes open-coded `Bytes` primitives a second-order concern rather than a
prerequisite.

**Three things this does not license, and the first is the one that bites.**

- **The fragment cannot take the loop.** `admissible_builtin` refuses every
  higher-order builtin, and `lex`'s whole scan is
  `fold(range(0, n + 1), start, ..)` while `dump` is two `map`s. So `lex`,
  `dump`, `hex` and `int_of_digits` are refused outright, and what the fragment
  could accept is the per-token work beneath them — `token_at`, `punct`, `ident`,
  `number`, `string_lit` and the predicates. That means **one entry per token**,
  not one per file.
- ~~**One entry per token meets `CONTRIBUTING.md` item 12 head on.**~~ **The
  premise was withdrawn hours after this was written.**

  > **Corrected (2026-08-24).** This bullet read: *"One entry per token meets
  > `CONTRIBUTING.md` item 12 head on: every entry costs O(the previous entry's
  > peak arena), measured there at 181x. At one entry per token that is on the
  > hot path rather than beside it. Not measured for this workload, and it could
  > plausibly erase the gain entirely."* Item 12 was fixed in PR #24 and 181x is
  > a withdrawn figure. It is corrected here rather than deleted because it was
  > this ADR's stated reason to doubt the fragment.

  `Ctx::begin` no longer touches the arena; `Ctx::end` clears it at the end of
  the entry that filled it, so an entry pays for its own work and its successor
  pays for nothing. Re-measured by the lane that fixed it, paired arms in one
  binary: **180.888x / 181.667x before, 1.202x / 1.499x after.** Checked here at
  the mechanism rather than the ratio, since a re-run at this load would be
  noise: `begin` is a single `is_empty` check plus a recovery path
  (`crates/ply-codegen-spike/src/rt.rs`), read at the unit as **0 ns at every
  rung**, against `end` at **4.168 ns per slot the entry itself used**, with the
  shrink amortized over `SHRINK_EVERY` = 64 entries.

  **The question therefore changed shape — it is no longer carry-over but a
  per-entry constant, and the constant is small.** Arithmetic, not measurement:
  at a generous 100 slots per token-sized entry, the corpus's 120,490 tokens
  cost 120,490 x 417 ns of arena work, about **50 ms**, against the ~7.1 s this
  lexer spends on that corpus at its measured 17,000 tokens/s — **under 1%**.
  **Still unmeasured for this workload** and still worth measuring, but it is now
  an open question about a ~1.2x carry-over and a sub-1% constant rather than a
  181x multiplier. That is the difference between *probably fatal* and *probably
  fine, go and check*. A second lane is refining the fix further, so the figure
  may move down again.
- **The 4.6% is a share of the interpreter's time, not a predicted speedup.**
  Removing dispatch for the compiled fraction does not make the compiled
  fraction free; the 11.68x on `read_line` is the only measured speedup and it
  was taken on a workload with no such entry pattern.

The profile is checked in as a method rather than a file: the command is in §9.

Two further interactions, both recorded rather than resolved:

- `admissible_builtin` refuses `cell_get`/`cell_set`, so the cell-based lexer
  `GAPS.md` §14 priced as the alternative to the fold is *excluded from the
  fragment*. The two routes to making a Ply lexer fast are mutually exclusive
  today.
- ~~`CONTRIBUTING.md` §"Things known to be broken" item 12: every entry into the
  fragment costs O(the previous entry's peak arena), measured at 181x. A front
  end entered once per `lex()` is fine; one entered per parse function is not.
  **Not measured for this workload.**~~ **Withdrawn: item 12 was fixed
  (2026-08-24, PR #24), and both the figure and the conclusion drawn from it are
  void.** An entry now pays for its own arena and its successor pays for nothing
  — 181x carry-over became 1.202x / 1.499x — so "one entered per parse function
  is not [fine]" no longer follows. §6.3 carries the re-measurement, the
  mechanism check, and the per-entry arithmetic that replaces this.

---

## §7 Decision

**Do not write Ply's front end in Ply on today's interpreter.** Not because Ply
cannot express one — it expressed a lexer that agrees with the reference on
768,760 bytes and lexes itself — but because §6 prices it at 60–130x the loop it
is meant to make fast, and §6.3 shows the fragment cannot be assumed to close
that.

Ranked, what would change the answer:

1. ~~**An attribution run splitting the Ply lexer's time between builtin bodies
   and dispatch** (§6.3).~~ **Taken.** Builtin bodies are **4.6% / 4.1%** of
   evaluation across two windows; dispatch and refcount traffic are the rest.
   The fragment is the lever, not a distraction — with the entry-rate caveat
   §6.3 now carries. **What replaces it at the top of this list** is the
   measurement that caveat names: **the fragment's actual throughput at one
   entry per token**, which is the entry pattern a front end would produce.
   The reason to worry has shrunk since this item was written: it first cited
   item 12's 181x carry-over, which was **fixed the same day** (1.202x / 1.499x
   after — §6.3). What is left is a per-entry constant that arithmetic puts
   under 1% of current lexing time, so this is now *confirm the expectation*
   rather than *check for a cliff*.
2. **Making §1 visible.** A lint, a `--explain` line, anything that says *this
   `push` will copy*. `GAPS.md` calls this the highest-value change and this
   review agrees, for two reasons the spike could not see. §4.1 shows the trap
   is already in the standard library, so the lint pays for itself with no
   self-hosting at all. And §5.2 shows the precondition is **not local**: a
   correctly written function is made quadratic by its caller, and by a caller
   whose offending sub-expression can be a constant. That rules out the cheap
   remedy — a coding convention — because there is no local property an author
   can check. A lint is not the convenient fix here; it is the only one.
3. **Fixing `escape_runs`** (§4.1). Shipped, quadratic, client-influenced input.
   `GAPS.md` §1 records that the obvious fix — splitting so each `push` is last —
   doubles the recursion depth and breaks the module at k = 8,000; the fix that
   works is one `push` per escape in last-argument position.
4. ~~**A loop, or a raisable call ceiling.** §5.1 makes this the difference
   between a portable parser and a rewritten one.~~ **The loop is delivered and
   the raisable ceiling is refused, by ADR 0022 (2026-08-27).** `iterate(seed,
   budget, step)` is an early-terminating loop that is depth 1 on both engines
   — asserted at
   `crates/ply-eval/tests/equivalence_audit.rs:2194`, which runs 500,000 steps
   under `with_max_calls(8)` while the same loop written as tail recursion
   raises at the same cap. A bare `--max-calls` flag is refused because results
   are cached as `(RUNTIME_VERSION, DefHash) -> Outcome` and shipping code
   writes only `Outcome::Pass` (`ply-test/src/lib.rs:1429`, `:1558`): raising
   the bound is monotone and safe, **lowering it silently returns a `Pass` for a
   program that would now raise**. ADR 0022 §5. And §5.1's premise, which is
   what put this item on the list, does not hold — see the correction there.
5. **`Float` construction** (§4.2). The only absolute hole, and the smallest of
   the five in impact, because §3.2 shows the text-passing substitute works.

**Keep** the differential harness. It is the right shape and it is armed on
every axis but one — kind, payload, span, dropped token and dropped diagnostic
all go red under mutation (§1.1) — and the one exception is the float digits,
which pass two deliberate corruptions (§3.2). Amend it so the digits are
compared as digits. The stale doc comment §4.5 opened on was fixed while this
was being written.

**Do not keep** the implication that 768,760 bytes of agreement is broad
coverage. It is 0.15% error paths and 24 float tokens (§3.1).

---

## §8 What would make this wrong

- **The multiplier in §6.2 is assumed, not measured.** If a Ply parser and
  typechecker cost only 2x the lexer rather than 5–10x, the estimate falls to
  5–11 s and the conclusion weakens without reversing. If they cost 20x it gets
  worse. Writing the parser is the only thing that settles it, and this ADR
  recommends against writing it — so the number that would refute this ADR is
  one it declines to take. That is a real weakness and it is why §7's first item
  is a profile rather than a port.
- **`ply check examples/` is not a like-for-like baseline, and the two errors
  push opposite ways.** Counting only `examples/`' 45,041 tokens while the run
  also resolves eight `std` modules **understates** the Rust front end's
  throughput, so the 12.6x in §6.1 is a floor rather than an estimate. Against
  that, crediting the Rust side with six phases where the Ply side does one
  **overstates** the gap for the phase actually compared. Neither was separated
  here; the honest reading is that lexing alone is at least 12.6x off and the
  whole-front-end comparison in §6.2 is where the assumption lives.
- **Every wall-clock figure was taken at load 17–88** on a machine shared with
  three other worktrees. User CPU is reported precisely because it is the robust
  half; if user CPU is itself distorted by cache contention at this load — not
  checked — the absolute figures move. The *shape* results in §4.1 (4x per
  doubling against 2x) do not depend on it.
- ~~**§6.3's conclusion rests on reading `rt_builtin`, not on profiling it.**~~
  **Closed by measurement**, and it resolved against the reading: the machine's
  per-call overhead *does* dominate the builtin bodies (4.6% / 4.1% builtins
  over two windows), so the fragment transfers better than §6.3 first argued and
  §7's ranking moved accordingly. What is still unmeasured is narrower and is
  stated in §6.3: the per-entry arena cost at one entry per token.
- **The profile is a sampling profile of one workload on one input.** Symbol
  attribution in a release build can be distorted by inlining — a builtin body
  inlined into `Machine::step` would be charged to dispatch. The leaf histogram
  corroborates rather than assumes (builtins 1.3% of leaves, `memcmp`/`memcpy`
  2.8%), but a counter-based attribution would settle it and none exists.
- **The 12.6x in §6.1 divides two numbers taken at different loads** (17,000
  tokens/s at load 41–45, 215,000 at load 37). Both are user CPU and the gap is
  an order of magnitude, so the conclusion is not sensitive to it, but the ratio
  is not a clean single-sitting figure.

---

## §9 Provenance

Machine: the one in `docs/ONBOARDING.md` §Provenance, shared throughout with
three other agent worktrees, two of them running `cargo test --workspace`.
1-minute load average recorded with every series and ranging **17.40 to 88.73**.

Tree: `~/.worktrees/ply/lexer-verify` at `73ebd1c`, detached. Release binary
built from that tree; `crates/ply-eval/src/frame.rs` verified clean (§0).

Two files in that tree were written by another party during this review:
`spikes/ply-lexer/lexer.ply`'s mtime moved at 15:07, and
`spikes/ply-lexer-nesting/` appeared at 15:21. `lexer.ply` was re-checked
afterwards — 647 lines and 26,404 bytes, unchanged, three content windows
byte-identical to the ones read before the move, and its 15 tests and the 33-file
agreement still green — so the verification in §1 stands. It is recorded because
§0 is about exactly this and a second instance should not go unwritten.
`spikes/ply-lexer-nesting/nesting.ply` is that party's reproduction; §5.2's
measurements are mine, taken by running it.

Statistic pre-registered before any measurement existed — written down at load
85.87, while the binary was still building, and reproduced here in full so a
reader does not have to take it on trust:

> Minimum of N runs; N = 5 where a run is under 2 s, N = 3 otherwise. Minimum,
> because on a loaded machine the minimum is the closest estimate of the
> unloaded time and no run is discarded after the fact. Load (`uptime`, 1-minute)
> recorded immediately before and after every series. Prefer any deterministic
> counter over wall clock if one exists.
>
> Decision rule for §1's shape claim: **accept** if, over two consecutive
> doublings of n, the non-final column is ≥ 3.0x per doubling at both and the
> final column is ≤ 2.5x at both; **reject** if either fails. Chosen because the
> prediction is 4x against 2x and load noise multiplies both columns roughly
> equally, so the ratio is the load-robust statistic.

No deterministic counter turned out to exist — `ply run --json` reports no step,
call or allocation count — so wall clock was unavoidable and user CPU was used
as the robust half. No run was discarded after the fact.

Commands, all from the worktree root:

```
./target/release/ply test spikes/ply-lexer/lexer.ply --no-cache
cd spikes/ply-lexer/harness && PLY_BIN=../../../target/release/ply \
  cargo test -j 2 -- --test-threads=2 --nocapture
./spikes/ply-lexer/harness/target/release/plydump <file.ply>

# §6.3's attribution profile. The probe is `lexer.ply` plus a generated
# `probe.ply` holding examples/desk.ply as a b"..." literal, whose entry point is
#   fn main() -> Int =
#     fold(range(0, 4), 0, |a: Int, i: Int|
#       a + string_len(dump(bytes_slice(source(), 0, bytes_len(source()) - i))))
# — four distinct slices so a pure-function memo cannot collapse them into one.
./target/release/ply run <probe-dir> & PID=$!
sleep 1; /usr/bin/sample $PID 6 1 -file sample.txt; wait $PID
# Attribution: walk the call graph, charge each subtree to its outermost
# matching frame, and divide by the subtree under `run::evaluate`.
```

Not re-run here, and stated as not re-run: `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets` and `cargo test --workspace`. This
review changed no file outside `docs/adr/`, so the workspace gates are
untouched by it — but they were also not taken, and another lane was running the
suite concurrently. `spikes/ply-lexer/harness` has its own `[workspace]` and is
reached by none of them; it will bit-rot exactly as `crates/ply-codegen-spike`
did, which its own README says.

One measurement in this document was discarded and re-taken: the first attempt
at §4.1's shipped-`json.ply` series used `json.encode_string` where the module
member is `json::encode_string`, so every run failed to compile and the harness
timed the failure at 0.01 s and reported it as a row. The harness now refuses to
record a run whose `--json` output does not carry `"ok": true`. Recorded because
it is the same defect this ADR is about: a green number over a program that
never ran.
