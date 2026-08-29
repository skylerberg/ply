# Language gaps found writing the parser's spine in Ply

The spine is `spine.ply`: token access, the threaded parser state, diagnostics,
the shared sequence driver, `qname`, and the dump encoder — a port of the
`impl Parser` preamble of `crates/ply-syntax/src/parser.rs` — `:112-259`,
`:441-464` and `:2059-2075`, which is 19 functions and 167 code lines — plus
the things Rust gets for free from `ply_span`, `ast.rs` and `const`.

Numbered `S1`.. so this file can be merged into a single `GAPS.md` beside the
other three areas without renumbering. Where a gap is one
`spikes/ply-lexer/GAPS.md` already recorded, it says so and reports **what
changed when the same gap met a parser** rather than filing it twice.

**Nothing here is a complaint about a missing convenience.** Two of these
(§S1, §S2) decided a data structure, one (§S3) decided the whole error design,
and one (§S6) turned a one-line mutation in the reference into an argument about
lifetimes.

---

## §S0 What this is measured against, and the one ratio it is honest to quote

| | reference | Ply | ratio |
| --- | --- | --- | --- |
| `parser.rs` preamble, code lines (blank and comment excluded) | 167 | 311 | **1.86** |
| — functions | 19 | 121 | 6.4 |
| `lexer.rs` against `spikes/ply-lexer/lexer.ply`, code lines | 950 | 420 | 0.44 |
| — total lines, the figure ADR 0020 quotes | 1,069 | 668 | 0.62 |

The lexer came out **smaller** in Ply. The spine comes out **1.86x larger**, and
the 102 extra functions decompose exactly:

| what | count | why |
| --- | --- | --- |
| token constructors `t_lparen` .. `k_false` | 47 | Ply has no constants (§S7) |
| `P` constructors `at_pos` .. `fresh_item` | 8 | no record update, and the field-order tax (§S5) |
| diagnostic-code functions `unexpected` .. `unknown_deriver` | 6 | no constants; Rust has `ply_span::codes` |
| `max_depth` | 1 | a `const` in Rust |
| the dump encoder `num` .. `hex` | 16 | **no counterpart on the reference side** — `crates/ply-syntax/src/tests.rs:1038 dump_module` is `#[cfg(test)]` and prints no spans, so the Ply side has to carry its own |
| `Span`/`Ident`/`QName` helpers | 7 | Rust has them in `ply_span` and `ast.rs` |
| the rest (`tok_index`, `bump`, `is_ident`, `is_str`, `diag1/2`, `eof_token`, `hex1/2`, …) | ~15 | §S1, §S4, §S9 |

**Do not extrapolate from 1.86.** The spine is the single most
constant-and-tuple-dense part of a parser — 47 of its 121 functions are token
names — and the areas above it (types, patterns, expressions, items) are
grammar, where the reference's `match` arms and Ply's should track much more
closely. The whole-parser figure is M6 in
`/tmp/ply-parser-spike/PREREGISTRATION.md` and this is one input to it, not a
preview of it.

---

## §S1 No list index, and this time it picks the data structure

`spikes/ply-lexer/GAPS.md` §10 files the `List` surface — `len, push, map,
filter, fold, range` (now also `iterate`) — as something that "starts to bite".
On a parser it is not a bite, it is the architecture.

A parser is `tokens[pos]`, `tokens[pos + 1]`, `tokens[pos + 2]`;
`parser.rs::kind_at` is called with 0, 1 and 2, and `at_law_start` (`:388`) uses
all three in one predicate. There is **no `nth`, no `head`, no `tail`, no
index** on a Ply `List` (`crates/ply-eval/src/builtins.rs:203+`), so
`lexer.ply`'s `List<Token>` cannot be a token buffer at all. The buffer is a
`Map<Int, Token>`, built once by a fold with a counter, and every peek is an
O(log n) red-black descent (`crates/ply-eval/src/value.rs:48`) with a
`Value::cmp` at each node where the reference does one bounds-checked load.

**What it cost, in code:** one type (`Buf`), one fold in `start`, and
`tok_index`, which is where the clamp, the `map_get`'s dead `None` arm and
`expect_gt`'s split all have to live — 8 lines standing in for `&self.tokens[i]`.

**What it cost, at run time:** measured, and the interesting half is what could
*not* be measured. See §S12.

**Registered prediction, unchanged:** a `list_at` builtin removes this, and
`/tmp/ply-parser-spike/PREREGISTRATION.md` §2.4 predicts the `Map` is the
largest single cost in the parser. §S12's S3 is the first evidence and it says
the *build* is not where that cost is; if the prediction holds it holds at the
per-peek `map_get`.

## §S2 No `last` on a `List`, so a rule that reads one needs a redundant field

`Parser::push` (`parser.rs:207`) is four lines:

```rust
if let Some(last) = self.diags.last()
    && last.code == d.code
    && last.primary_span() == d.primary_span() { return; }
```

It drops a diagnostic with the same code and primary span as the one before it,
because a single mistake trips several expectations at one offset. **It changes
the diagnostic list exactly**, so it is ported exactly — and there is no `last`,
no `head`, no index, and no reverse. The three ways out:

1. `fold` the whole list to find its last element: O(n) per push, O(n²) overall.
   Fine at the corpus's 25 diagnostics; not fine on a file of errors, and it is
   a quadratic hidden inside a deduplication rule, which is the last place
   anyone would look for one.
2. Keep the list reversed so "last" is "first" — there is no `head` either.
3. **Carry the answer beside the list.** `P` gains `last_code: Bytes` and
   `last_span: Span`, written by `push_diag` and seeded in `start` from the
   lexer's diagnostics.

(3) is what `spine.ply` does. The cost is two fields in a nine-field state
record, replicated through ten constructors, and a seeding step in `start` that
is easy to forget: **the first parser diagnostic is deduplicated against a
*lexer* diagnostic**, because `Parser::new` seeds `self.diags` with the lexer's.
`arm-spine.sh`'s "the lexer's diagnostics do not seed the dedup key" mutation
exists because that was not obvious, and the test that catches it (`&` alone,
which is `E0001` over `0..1` from the lexer) had to be constructed on purpose.

## §S3 No `?`, and the bail flag's real price is not the guards — it is that a third of them cannot be tested

`spikes/ply-lexer/GAPS.md` §12's finding was that error accumulation cost the
lexer nothing *because a lexer never fails*, and it predicted it would bite a
parser. It does.

The design is registered in `/tmp/ply-parser-spike/PREREGISTRATION.md` §2.2 and
it is exact rather than approximate: the reference's `Bail` (`parser.rs:11`) is
a **zero-field struct**, so `PResult<T> = Result<T, Bail>` is isomorphic to
`Option<T>` and the error channel carries nothing. A `bail: Bool` in the
threaded state is therefore the same type, and the guard goes on the **callee**
rather than the call site, which is the improvement over
the shape `crates/ply-std/ply/json.ply` is forced into — `decode_map` /
`decode_and_then` at `:99-112`, one number literal split across seven functions.
Counted on this tree: `parser.rs` has **90 functions** and **178 `?`
operators**, so the guard count is bounded by 90 and the call-site count it
replaces is 178.

The spine has **8 guards** — `eat`, `expect`, `expect_close`, `expect_ident`,
`expect_gt`, `deeper`, `comma_list`, `qname` — which is every function in it
that reads a token on behalf of a caller.

**The result that was not expected: two of the eight are unkillable.** A guard
on a function whose first act is a call to an already-guarded function is an
*equivalent mutant* — deleting it changes nothing observable. `qname` is one
(its first act is `expect_ident`, which guards) and `comma_list` is the other
(its `iterate` step's first test is `s.p.bail`). `arm-spine.sh` asserts both
stay green, which is the honest way to record it: an equivalent mutant is not a
hole in a test, and treating one as a hole is how a suite grows assertions about
an implementation detail.

That matters for what the guard-deletion instrument
(`/tmp/ply-parser-spike/PREREGISTRATION.md` §3.4) can prove. **A raw count of
one guard per parse function overstates it**: only guards on functions that
touch a token before their first guarded callee are killable at all. In the
spine that is 6 of 8, and `parser.rs`'s 90 functions bound the whole-parser
count. The
right statistic for M6 is therefore two numbers — guards written, and guards a
mutation can kill — and the second is the one that says how much the design is
actually being checked.

## §S4 No tuples, and a parser's commonest type is a pair

`spikes/ply-lexer/GAPS.md` §9 counted three record types in the lexer that
existed only because a function answers with more than one thing. The spine
alone has **six**, and one of them is on the return type of essentially every
parse function in the whole port:

| type | stands for | reference |
| --- | --- | --- |
| `R<a> = { p: P, node: a }` | a node **and** the next state | `PResult<T>` over `&mut self` |
| `Ate = { p: P, ok: Bool }` | `eat`'s `bool` **and** the next state | `fn eat(..) -> bool` |
| `Acc<a> = { p: P, out: List<a> }` | the driver's seed | two locals in a `while` |
| `Start = { c: Ctx, p: P }` | `lex` gives a buffer **and** an initial state | `Parser::new` |
| `Buf = { i: Int, m: Map<Int, Token> }` | the indexing fold's counter **and** map | `for (i, t) in .enumerate()` |
| `DAcc` | the diagnostic fold's key **and** list | ditto |

The tax is not the six declarations, it is that `R<a>` is threaded by hand
through every call: `let a = f(c, p); let b = g(c, a.p); ...` where the
reference writes `let a = self.f()?; let b = self.g()?;`. That is one extra
identifier and one extra field access per call site across the whole parser.

## §S5 No record update, plus GAPS §1's positional tax, forces a constructor discipline

Two gaps that are separately mild and together sharp.

`spikes/ply-lexer/GAPS.md` §2: no record update syntax, so changing one field of
`P` means writing all nine. `spikes/ply-lexer/GAPS.md` §1, **measured**: a
growing container built anywhere but in the last sub-expression of its enclosing
node is copied instead of updated in place, so an accumulator in the wrong
position is quadratic — and nothing in the language says so and nothing checks
it. ADR 0020 §5.2 sharpens it further: the tax is **not local**, a correct
callee is destroyed by a caller that puts the call anywhere but last.

`P` has nine fields, one of which (`diags`) grows. The reference has **178 `?`
operators**, and each is a place the Ply port threads a new state by hand; nine
fields × 178 is 1,602 chances to put `diags` anywhere but last, each of which is
silent, and each of which is a quadratic. So `spine.ply`
writes the `P` literal **exactly ten times**, in ten constructors, and the rule
in its header is *do not write a `P` literal*.

Two things worth writing down about that.

- **The discipline is unenforceable.** Nothing stops the other three areas
  writing a literal, and nothing would report it if they did — not the
  typechecker, not `ply test`, not a benchmark that only runs small inputs.
  The only instrument is the shape check (M4, and §S12's S2 here), and it has to
  be pointed at the right accumulator to see anything.
- **One instance of the tax is knowingly paid.** `{p: push_diag(a.p, d),
  node: X}` puts the push in a non-final position, so `p.diags` is aliased and
  the push copies. `diags` holds 25 elements over the whole 770 KB corpus, so
  the copy is O(25) and the total is O(625) — negligible, and recorded here so
  that it is a decision rather than an oversight. It stops being negligible on a
  file of errors, which is exactly the fixture corpus the recovery half needs.

## §S6 Immutability turns `expect_gt`'s one-line mutation into an argument about lifetimes

`parser.rs:187` splits `>=` into `>` then `=` for `type Pair<a>= ..` by
**rewriting the token stream in place**:

```rust
self.tokens[self.pos] = Token { kind: TokenKind::Eq, span: Span::new(.., start + 1, end) };
```

The Ply buffer is immutable, so the rewrite is a *position*: `gt_split: Int` in
`P`, applied by `tok_index` on read. Which is fine — until you notice a file may
contain two such splits, and one `Int` holds one.

The reference does not have to think about this, because its rewrite is
permanent. The Ply version needs an argument, and here it is: a split at index
`k` is observable at `k` (through `kind`) and at `k + 1` (through `prev_span`,
which reads `pos - 1`) and nowhere else; `pos` never moves backwards; so a
second split at `k' > k` cannot be reached until `pos` is past `k + 1`. One slot
suffices. **That argument is the cost** — not the field, not the four lines in
`tok_index`. It is also the kind of thing that is true today and silently false
after some later change to how the parser backtracks, and there is no
`debug_assert` shape in Ply to pin it with; the two tests
`expect_gt splits a >= ..` and the `prev_span` half of it are what stand in.

## §S7 No constants: 47 nullary functions, and what one `at()` costs

Ply has no `const` and no top-level `let`. Every token the parser compares
against is therefore a function:

```ply
pub fn t_comma() -> lexer::Tok = lexer::TPunct(b"comma")
```

47 of them, plus 6 diagnostic codes, plus `max_depth`. `at(c, p, t_comma())` is
**four interpreter calls** — `t_comma`, `at`, `kind`, `tok_at`→`tok_index` —
plus a `map_get`, plus a structural `==` on a constructor with a `Bytes`
payload, where the reference is one load and a discriminant compare.

`spikes/ply-lexer/GAPS.md` §11 saw the same shape on the lexer's `at` and called
it "a second sighting". This is the third, and on a parser it is the innermost
loop rather than a helper: `at` and `kind_at` are the two most frequent
operations in the whole program.

Note what would and would not fix it. A `const` would remove one of the four
calls. A `list_at` builtin (§S1) would remove the `map_get`. Neither removes the
`Bytes` comparison inside `TPunct(b"comma") == TPunct(b"comma")`, which is a
byte-string compare where the reference compares a `u8` discriminant — that one
is `lexer.ply`'s representation choice, and it is the price of the lexer spike
having had no reason to number its punctuation.

## §S8 `iterate` did exactly what ADR 0022 added it for — and the reference is the one with the missing guard

The one entry in this file that is not a gap.

`comma_list` is a single `iterate(seed, budget, step)`, and it replaces
`parser.rs:2065`'s `while` at all **14** of its call sites (`self.comma_list(`
12, plus 2 written on a continuation line). Three things came out of writing it.

1. **It typechecks generic and higher-order.** `comma_list<a>(c, p, close,
   item: (Ctx, P) -> R<a>) -> R<List<a>>`, with a generic record accumulator and
   the callback handed on to `iterate`, compiled first try.
   `/tmp/ply-parser-spike/PREREGISTRATION.md` §4.2 registered the risk that it
   would not and that fourteen call sites would become eight hand-monomorphised
   copies. **M6's "hand-monomorphised copies" is 0 for the driver**, as
   predicted.
2. **The budget is a backstop that cannot fire**, in
   `crates/ply-std/ply/http.ply:1584 stream_chunks`'s `fuel + 1` shape:
   `c.ntok - p.pos + 1`, because a `Continue` costs at least one token. Nothing
   is materialised — the contrast with `lexer.ply`'s `fold(range(0, n + 1), ..)`
   is the whole point of the spike's Rule 2, and on `examples/desk.ply` that
   `fold` wastes **140,108 of 159,684** rounds to produce 19,576 tokens — 87%
   of the loop, `spikes/ply-lexer/GAPS.md` §5 — and builds the `range` list of
   159,684 boxed `Int`s to do it.
3. **The Ply version is the safer one.** An `item` that consumed no token and
   did not bail would hang the reference's `while` forever, with no timeout
   anywhere in `ply-test` or `ply-cli` to stop it (ADR 0022 §1.3). The same bug
   in Ply exhausts the budget and reports. That is `iterate`'s budget being an
   argument, and it is worth recording as a place where the ported program is
   better than the original rather than merely equal to it.

## §S9 Small missing pieces, each cheap, listed because a bootstrap pays them all

- **No `min` / `max`.** `(self.pos + n).min(len - 1)` becomes a nested `if`.
  Three occurrences in the spine, `span_to` included.
- **No `matches!`.** `is_ident` and `is_str` are two-arm `match`es where the
  reference writes one line inline.
- **No statement form.** The reference writes `self.advance();` and drops the
  span. Everything in Ply is an expression, so dropping it is `bump`, a second
  function.
- **No builder chaining.** `Diagnostic::error(..).primary(..).secondary(..).note(..)`
  becomes `diag1`/`diag2` with a note **count**, because notes are prose and
  prose is not compared (§S11).
- **No `saturating_sub`.** One more `if` inside `tok_index`.

## §S10 No Unicode, and this time it costs nothing — with the reason

`parser.rs:2078 starts_upper` asks `char::is_uppercase`, and it decides
constructor-versus-binder at every bare name in a pattern and every bare name in
a type. Ply has no character type and no Unicode table
(`spikes/ply-lexer/GAPS.md` §7), so `spine.ply`'s is ASCII `A`..`Z`.

**The two cannot differ on this parser, and the reason is upstream rather than
lucky.** `lexer.ply` refuses any token whose first byte is ≥ 128 with its own
`X0001` (`spikes/ply-lexer/README.md` §"Where this disagrees on purpose"), so an
identifier that reaches `starts_upper` has an ASCII first byte by construction.
The divergence is the lexer's, already recorded there, and this is not a second
one. It *would* become a second one the moment `lexer.ply` grew a Unicode table,
which is the shape of every deferred gap in a bootstrap: it does not compound
until the thing below it is fixed.

## §S11 What the comparison does not check, said before it is green

Two silences, both sized.

- **Diagnostic message text.** `error_here`, `unclosed`, `expect`, `expect_ident`
  and `expect_gt` all carry a `what: Bytes` naming what was expected and **never read it**. Counted on this tree, that is **134 call sites**:
  `self.expect(` 33, `self.expect_ident(` 39, `self.expect_close(` 30,
  `self.expect_gt(` 3, `self.error_here(` 19, `self.unclosed(` 10. Building the
  real message needs `TokenKind::describe`'s ~40 arms as well. The parameter is
  carried anyway so that turning messages on is a change to `error_here` alone
  rather than a rewrite of 134 call sites — the literals are paid, the table is
  not.
- **Severity.** Every parser diagnostic is `Severity::Error`, so the signature
  drops it. If a warning is ever added to the parser the dump will not see it.

What *is* compared is code, every label's span, every label's primary flag, and
the note **count** — wider than the lexer spike's code-plus-span, because `parser.rs` has **45 diagnostic
sites** — 16 `Diagnostic::error` written out, 19 `self.error_here(`, 10
`self.unclosed(` — and all but four of them are `codes::UNEXPECTED_TOKEN`, so
code plus span is a weak signature when almost every diagnostic shares a code. How weak is M7
in `/tmp/ply-parser-spike/PREREGISTRATION.md`, and it is a count over the error
fixtures, not an adjective.

## §S12 What was measured, including the one that came back UNMEASURED

Pre-registered in `/tmp/ply-parser-spike/PREREGISTRATION-SPINE.md` before any
number existed. Release binary; `.github/binary-is-current.sh` exited 0
(`current target/release/ply (152 inputs checked)`) immediately before each
series; `uptime` on both sides; **no run discarded**.

### S1 — does building the buffer hit the call ceiling? **No.**

| file | bytes | `ntok` | `len(lex().toks)` | index vs list | recursion diagnostic |
| --- | --- | --- | --- | --- | --- |
| `crates/ply-std/ply/db.ply` | 135,285 | 29,213 | 29,213 | **agrees** | none |
| `examples/desk.ply` | 159,971 | 19,576 | 19,576 | **agrees** | none |

"Agrees" is byte equality between a dump built by walking the `Map` with
`tok_index` over `range(0, ntok)` and a dump built by `map`ping the `List` the
fold consumed — so it checks the **indexing**, which is the new thing, and not
the lexing, which `spikes/ply-lexer` already established. desk.ply's 19,576
matches `spikes/ply-lexer/GAPS.md` §5 and §15 exactly.

Registered outcome met. Load 3.84 before and after.

### S2 — is the buffer build linear? **Yes.** Registered prediction confirmed.

Generated input, `fn f<i>(a: Int, b: Int) -> Int = a + b * 2 - 1` repeated, so
token count is the only thing that varies. Min of 5, user CPU, every run printed.

| series | tokens | all five runs | min | control (min) |
| --- | --- | --- | --- | --- |
| build-N | 16,801 | 0.46 0.46 0.45 0.44 0.46 | 0.44 | 0.01 |
| build-2N | 33,601 | 0.90 0.91 0.91 0.91 0.91 | 0.90 | 0.01 |
| build-4N | 67,201 | 1.79 1.74 1.65 1.61 1.61 | 1.61 | 0.01 |

Ratio per doubling: **2.05** (N→2N) and **1.79** (2N→4N); control-subtracted,
2.07 and 1.80. An earlier series at a quarter of the scale (4,201 / 8,401 /
16,801 tokens) gave **2.00** and **2.06**. Registered rule: `[1.6, 2.6]` is
linear, `≥ 3.4` is quadratic. All four doublings are linear and none is within
reach of 3.4.

Load 3.78 before, 4.42 after — the *after* reading crosses ADR 0022 §0.1's gate
of 4.0, so **wall clock is not carried** for this series and only user CPU is.
The `build-4N` series drifts 10.1% first-run-to-last (1.79 → 1.61), marginally
over the registered 10% cut, so by the letter of the rule that one series is
**UNMEASURED** — its window is printed above and its ratio points *further*
toward linear, not away. The other three doublings clear the cut.

**So `spikes/ply-lexer/GAPS.md` §1's positional tax does not visibly apply to a
`Map`.** That is worth knowing beyond this spike: the rule was measured on a
`List`, a persistent red-black tree shares structure on insert, and a `Map`
accumulator therefore appears to be safe in a position that would ruin a `List`
one. The field is written last in `start` anyway.

### S3 — what does indexing cost on top of lexing? **UNMEASURED.**

Registered with no prediction, and with the rule that if `(B − L)` is smaller
than the spread of its own series the ratio is UNMEASURED and the windows are
printed instead. That is what happened.

| series | tokens | all five runs | min |
| --- | --- | --- | --- |
| lexonly-N | 16,801 | 0.40 0.40 0.41 0.41 0.41 | 0.40 |
| build-N | 16,801 | 0.35 0.35 0.36 0.39 0.38 | 0.35 |
| lexonly-2N | 33,601 | 0.78 0.77 0.74 0.74 0.73 | 0.73 |
| build-2N | 33,601 | 0.75 0.77 0.81 0.84 0.85 | 0.75 |
| lexonly-4N | 67,201 | 1.46 1.47 1.31 1.31 1.33 | 1.31 |
| build-4N | 67,201 | 1.65 1.54 1.58 1.58 1.59 | 1.54 |

At N the difference is **negative** — the build, which is the lex plus the fold,
minimised lower than the lex alone. That is not a result, it is the between-series
drift: `build-N` minimised at 0.44 in the S2 series and 0.35 here, 26% apart.

An interleaved retake at 4N (seven rounds, L then B in each round, so a drift
hits both) did not separate them either: `lexonly` 1.76 1.82 1.51 2.03 1.49 1.52
1.30, `build` 2.04 1.68 1.73 2.06 1.59 1.42 1.41 — a min-to-min gap of 0.11
inside per-series spreads of 0.73 and 0.65.

**Reported as UNMEASURED, with the windows.** The only thing the data supports
is a loose upper bound: the indexing fold is somewhere under the series spread,
~0.7 s user CPU on 67,201 tokens, which is too loose to be interesting. What it
*does* bear on is `/tmp/ply-parser-spike/PREREGISTRATION.md` §2.4's prediction
that `Map<Int, Token>` is the largest single cost in the parser: whatever that
cost is, **it is not the one-time build**. If the prediction holds it holds at
the per-peek `map_get`, and that needs a parser to measure.

## §S13 What the arming found, which is the part worth reading twice

`spine.ply`'s first 18 tests passed on the first run. `arm-spine.sh` corrupts
one thing at a time and asserts the suite fails; on its first run **six of the
mutations left the suite green**, which is six tests that were watching nothing:

| mutation that survived | why the test could not see it |
| --- | --- |
| bail guard deleted from `expect` | the bailed state sat on a token `expect` would reject anyway, so the missing guard raised a *duplicate* diagnostic at the same span, `push_diag` dropped it, and nothing moved. The fixed test puts the bailed state on a token each function would **accept**. |
| …from `expect_ident` | same |
| …from `qname` | **equivalent mutant** — see §S3 |
| `comma_list`: end of input is not a stop condition | the fixture `a, b` ends because the comma is missing, not because the input ends. The fixed fixture is `a, b,`. |
| `comma_list`: a missing comma does not end the list | every fixture had a comma between every pair. Added `a b)`. |
| a list no longer emits its length | `dump_list` was not reached by any test — `dump_qname` uses `dump_opt` and `dump_diags` uses `nlist` directly. |

The suite is now 26 tests and 39 mutations, of which **37 are armed and 2 are
asserted equivalent**. The two equivalent ones are the §S3 finding and are
asserted to *stay* green, so if either ever stops being equivalent the script
says so.

Two of the six were fixtures that ended for the right reason by accident, which
is the specific way a parser test goes vacuous: **a sequence has several reasons
to stop and a fixture usually triggers more than one.**

## §S14 A process finding, not a language one

Four agents wrote against this spine concurrently, and the largest area
(`items.ply`, 42 KB) was written against an *assumed* API before the spine
existed. It assumed `PDiag`, `dummy_span`, `bare`, `qualified`, `diag1`,
`diag2`, `is_ident`, `Ate`, `clear_bail`, `with_sets` and `bump`; the spine had
written `Diag`, `no_span`, `bare_qname`, `qualified_qname`, `d1`, `d2`,
`at_ident`, `Eaten`, `caught`, `with_uses_sets` and no `bump` at all — eleven
collisions and four functions that did not exist (`bump`, `is_str`,
`starts_upper`, `dump_bool`).

The spine took the other file's names, because 42 KB of written code outranks a
naming preference, and added the four. It is recorded because the same shape
will recur in any parallel bootstrap: **the interface is the deliverable that
has to exist first, and "the spine is built and armed first" was in the plan and
still did not happen early enough.**

---

## What the language handled fine

Listed because a gaps file that only lists gaps misleads about the ratio.

- **Generic higher-order functions.** `comma_list<a>` with a `(Ctx, P) -> R<a>`
  callback handed to `iterate`, and `dump_list<a>` / `dump_opt<a>` with
  `(a) -> Bytes`. All first try. This was the registered risk in
  `/tmp/ply-parser-spike/PREREGISTRATION.md` §4.2 and it did not materialise.
- **Recursive ADTs with inline anonymous record payloads** —
  `EBinary({span: Span, op: Bytes, lhs: Expr, rhs: Expr})` and
  `List<{name: Ident, value: Expr}>`. This was the **go/no-go** in §4.1, the one
  that stops the spike dead, and it works. `crates/ply-std/ply/http.ply:243` and
  `:853` were the precedent and the precedent held.
- **Cross-module ADTs.** `match t { lexer::TIdent(b) -> .., .. }` and
  `lexer::TPunct(b"comma") == lexer::TPunct(b"comma")` both work, so the token
  vocabulary can live in the spine and the lexer can stay unmodified.
- **`lexer.ply` was copied in unmodified and needed no edit.** The plan said an
  edit to it would itself be a finding; there is none to report.
- **`Map<Int, Token>` at corpus scale** — §S12's S1 and S2.
