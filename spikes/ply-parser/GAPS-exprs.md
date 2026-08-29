# What Ply could not do, written down while writing an expression parser in it

Area 3 of the parser spike. `exprs.ply` is the vehicle: a function-for-function
port of `crates/ply-syntax/src/parser.rs:1211-1844`, the expression half of the
grammar — literals, application, binary and unary operators with precedence,
`if`, `match`, blocks, records, lists, lambdas, `handle`, `with_cell`,
`with_region`, `simulate`.

In the style of `spikes/ply-lexer/GAPS.md`. Each entry says what I was trying to
express, what I had to write instead, and what it cost. Two entries (§1, §2)
record a claim the plan made that I **refuted**, and one (§3) records a
registered prediction of my own that the measurement did not support, because all
three are worth more than the claims would have been.

**Provenance.** Machine: `docs/ONBOARDING.md` §Provenance, shared with three
sibling agent worktrees building this same spike concurrently. Every number
below was taken through `target/release/ply` with
`.github/binary-is-current.sh` reporting `current  target/release/ply
(152 inputs checked)`, exit 0, immediately before the series; its output is
recorded beside each series. Load was **2.0–4.7** (1-minute average) throughout
the timing work and is printed before and after each series. **User CPU is the
primary statistic and wall clock is recorded beside it**; the statistic is the
**minimum of N**, N = 5, and **no run was discarded** — every run of every series
is printed. Statistics X1, X2 and X4 were registered in
`/tmp/ply-parser-spike/PREREGISTRATION-area3-expressions.md`, outside the
repository, **before any of their numbers existed**. The guard sweep in §4 was
not pre-registered: it came out of the arming procedure `CONTRIBUTING.md`
demands, and it is reported as an arming result with its method rather than as a
registered statistic.

---

## §1 The AST *is* representable, and the plan's risk ladder was not needed — a refutation

The plan's risk section put this first and called it the thing that stops the
spike dead:

> **The AST may not be representable.** `type Expr = {kind: EKind, span: Span}`
> with `EKind` carrying `Expr` is mutual recursion between a **named** record
> alias and a **named** ADT, and I checked: nothing in `crates/ply-std/ply/` or
> `examples/` does it.

It is representable, on the first attempt, and so is more than the ladder's first
rung. The probe that settled it was 30 lines and ran before any parser code was
written:

```ply
type Expr =
  | ELit({ span: Span, v: Int })
  | EBlock({ span: Span, stmts: List<Stmt>, tail: Option<Expr> })
  | EMatch({ span: Span, scrut: Expr, arms: List<Arm> })

type Stmt = SLet({ span: Span, value: Expr }) | SExpr(Expr)   // ADT <-> ADT

type Arm = { guard: Option<Expr>, body: Expr, span: Span }    // record <-> ADT
```

Both cycles typecheck and both evaluate. So `exprs.ply` carries `Stmt`,
`MatchArm`, `HandleClause` and `ReturnClause` as the reference's own named types
rather than as inline anonymous records, and `MatchArm` is
`{pat, guard, body, span}` exactly as `ast.rs:995` declares it. **No rung of the
fallback ladder was taken, and the fact that nothing in the shipped tree does
this turned out to be a fact about the shipped tree rather than about the
language.**

What is *not* representable is unrelated and much smaller: `Box<Expr>` has no
spelling, which costs nothing because Ply needs no box.

## §2 The token buffer's `Map` is forced, but the reason given for it was half wrong — a second refutation

The forced half is real and it is the architectural decision of this spike. The
`List` surface is exactly `len / push / map / filter / fold / range / iterate`
(`crates/ply-eval/src/builtins.rs:203+`) — **no index, no head, no tail, no
`nth`**. A parser is `tokens[pos]`, `tokens[pos+1]`, `tokens[pos+2]`, so a
`List<Token>` cannot be a token buffer at all, and `Map<Int, Token>` is the only
random-access container in the language. `lexer.ply` therefore produces a list
that is immediately folded into a map it did not want to be.

The half I got wrong I got wrong in my own first draft of the shared spine, and I
am recording it because I shipped it before the other areas landed. I assumed
Ply had **no structural equality on a constructor**, reasoned that
`at(c, p, t_lparen())` against `lexer::TPunct(b"lparen")` would therefore be a
byte-string comparison per peek, and built the buffer as
`{start, end, kind: Int, tok: Tok}` with a precomputed integer kind and a
`punct_code`/`kw_code` chain to fill it. **`==` works on ADT values**, so
`kind(c, p) == t_lparen()` is one builtin call, and the spine the other three
areas wrote against does exactly that. My version was replaced by theirs on
integration.

It was not even faster. Same input, same probes, same machine, the two
implementations differ only in the token representation and in the dispatch
style, and the parse term is **0.62 s with `Tok` equality against 0.75 s with the
integer kinds** — my "optimisation" was 21% *slower*, because the extra record
per token and the `gt_split` branch in every peek cost more than the comparison
saved. It is a two-implementation comparison and not an isolated measurement of
the dispatch, and it is reported as such.

**The real cost of the `List` surface is not the dispatch. It is that the
parser's most frequent operation is an O(log n) tree descent with a `Value::cmp`
at each node and an `Option` to unwrap at the end, where the reference's is a
slice index.** Priced in §3.

## §3 The Map lookup is a third of the parse, not the largest cost in it — my own prediction, not supported

The plan registered this prediction in my name:

> **Registered prediction: this lookup is the single largest cost in the parser,
> and a `list_at` builtin would remove it.**

It is not. **X4**, five probes over one 32,450-byte expression source (14,742
tokens, 8,141 AST nodes), each a `main` in a generated project whose `source()`
is that text as a `b"..."` literal, minimum of 5, user CPU primary, wall clock
beside it, instrument checked immediately before, load 3.05 → 3.11:

| probe | what it does | wall, all five | user, all five | min user |
| --- | --- | --- | --- | ---: |
| D | `bytes_len(source())` — literal and process control | 0.03 0.03 0.03 0.03 0.03 | 0.03 0.03 0.03 0.03 0.03 | 0.03 |
| L | `len(lexer::lex(source()).toks)` — lex only | 0.31 0.31 0.31 0.31 0.32 | 0.31 0.31 0.30 0.31 0.30 | 0.30 |
| M | `spine::start(source()).c.ntok` — lex **and** index into the `Map` | 0.33 0.33 0.33 0.33 0.33 | 0.33 0.33 0.33 0.33 0.33 | 0.33 |
| K | M **and** one `kind_at` per token | 0.40 0.40 0.40 0.40 0.39 | 0.39 0.40 0.40 0.39 0.39 | 0.39 |
| P | M **and** the parse, tree built and dropped | 0.97 0.95 0.96 0.97 0.96 | 0.95 0.95 0.95 0.95 0.95 | 0.95 |

Subtracting:

- **lex** = L − D = **0.27 s**
- **building the map** = M − L = **0.03 s** — 14,742 `map_insert`s, 11% of the
  lexing they index. `Map` is `RedBlackTreeMap` and persistent, so the insert
  shares structure rather than copying; `spikes/ply-lexer/GAPS.md` §1's
  positional tax does not appear to apply to it.
- **14,742 peeks** = K − M = **0.06 s**, so **≈ 4.1 µs per `kind_at`** — an upper
  bound, because it includes the `fold(range(0, ntok), ..)` that drives the sweep.
- **the parse itself** = P − M = **0.62 s**.

The parser makes roughly three to five peeks per token — a kind test, plus
`kind_at(1)` and `kind_at(2)` at the four lookahead sites. At 4.1 µs that is
0.18–0.30 s against a 0.62 s parse: **a third of it at the upper end, and less
than that in truth since 4.1 µs is an upper bound.** Large, worth a `list_at`
builtin, and **not** the single largest cost. The rest is building records and
threading state.

*Registered, not supported. `list_at` would be worth having; it would not have
changed the shape of the answer.*

## §4 The `bail: Bool` that replaces `?` costs 46 guards, and 27 of them cannot be seen to matter

This is the design decision the plan made in place of `Result` + `?`, and it is
sound: `parser.rs:11`'s `Bail` is a zero-field struct, so `PResult<T>` carries
nothing in its error channel and a flag in the threaded state is isomorphic to
it. One guard per callee beats one `match` per call site, which is what
`crates/ply-std/ply/json.ply` is forced into — 58 of 129 functions returning
`Result`, and one number literal split across seven functions.

What it costs, counted:

- **46 `if p.bail` guards** across `spine.ply`, `types.ply`, `patterns.ply` and
  `exprs.ply`; **27 of them in `exprs.ply` alone**, which is one for every
  function in the file that takes a `P`.
- **Three placeholder constructors** in this module (`no_expr`, `no_clause`,
  `no_return`) plus the ones the other areas declare, because a guarded function
  must answer with a node it will never be asked for. They are variants of their
  own (`EBad`, `PBad`, `TBad`) rather than stand-in real nodes, so a placeholder
  that ever escaped into a dump would be visible there.
- **The invariant has to be maintained by hand and is not checked by anything.**
  It is one sentence — *a parse function called with `p.bail` true answers `p`
  unchanged and consumes nothing* — and Ply has no way to state it.

And here is the part worth the entry. I wrote a test per function asserting
exactly that invariant, then deleted each of the 46 guards one at a time and
re-ran the suite:

| module | guards | individually detected |
| --- | ---: | ---: |
| `spine.ply` | 8 | 6 |
| `types.ply` | 10 | 0 |
| `patterns.ply` | 1 | 0 |
| `exprs.ply` | 27 | 13 |
| **total** | **46** | **19** |

The 27 that are not detected are, in this module, **genuinely redundant**: a
deleted guard is caught by the guard of the first function the unguarded body
calls. `bin_expr`'s guard is masked by `unary_expr`'s; `postfix_expr`'s by
`primary_expr`'s; `block_expr`'s by `block_inner`'s. `arm-exprs.sh` records four
of them as `equiv` entries — mutants that are *supposed* to leave the suite green
— beside three guards that are not equivalent and go red, so that the distinction
is demonstrated rather than asserted.

The zeroes in `types.ply` and `patterns.ply` are a different thing and the areas
that own them should know: **those two modules have no bail-invariant test at
all, so every one of their eleven guards is currently unarmed.** Deleting any of
them leaves their suites green. That is not a defect in their parsers; it is a
hole in their tests, and it is the hole this section exists to have found.

Two further things the sweep taught, which no amount of reading would have:

1. **The invariant's testability is input-dependent, so the obvious test is
   vacuous.** `postfix_expr` had no guard at all and my test still passed,
   because I called it on `f(1)` — whose first act reaches the *guarded* `qname`.
   Changing the input to `1` — whose first act is an unconditional `advance` —
   made it fail immediately. A test of this invariant must pick an input the
   unguarded body would consume, and nothing says which inputs those are.
2. **Ten functions in this module violated the invariant on the first write**,
   all of them the ones dispatched on a token kind: `primary_expr`, `if_expr`,
   `match_expr`, `handle_expr`, `lambda_expr`, `record_expr`, `let_stmt`,
   `with_cell_expr`, `with_region_expr`, `simulate_expr`. In the reference this
   class of bug cannot exist, because `?` returns before the caller is entered.
   Here it is a silent read past an error and a phantom diagnostic. **This is the
   specific new hazard the design creates**, and `spikes/ply-lexer/GAPS.md` §12
   predicted its existence exactly — *"a lexer is the one front-end phase that
   dodges this"*.

## §5 `List` has no `last`, so `Parser::push`'s dedup rule needs a field of state

`parser.rs:219 Parser::push` drops a diagnostic whose code and primary span match
**the last one already recorded**:

```rust
if let Some(last) = self.diags.last() && last.code == d.code && ...
```

`List` has no `last`, no index and no reverse, so there is no way to read the
element just pushed. The state therefore carries a copy of it — `last_code:
Bytes` and `last_span: Span` in the shared `P`, two fields that exist only
because a list cannot be read from the end. (My own first spine carried
`last: Option<PDiag>`, one field instead of two, for the same reason.)

The rule is ported because it changes the list exactly, and it is armed: `arm-exprs.sh`
does not mutate it, but `arm-spine.sh` does, and `exprs.ply` has a test that one
offset raises one diagnostic.

## §6 No record update syntax, and the state has nine fields

`{..p, pos: n}` does not exist (`spikes/ply-lexer/GAPS.md` §2), so every state
transition lists every field. The lexer's `Scan` had three; the parser's `P` has
**nine** — `pos`, `no_brace`, `depth`, `gt_split`, `uses_sets`, `bail`,
`last_code`, `last_span`, `diags` — and the spine spells all nine out **eight
times**, once per `with_*` helper. Adding a tenth field to `P` is an edit to
eight record literals, and forgetting one is a type error rather than a silent
bug, which is the one mercy.

The same gap costs this module an eighteen-arm function. `primary_expr`'s
parenthesised arm is `inner.span = open.to(close)` in the reference — one field
assignment. Here the span lives inside each variant's payload record and there is
no update syntax, so it is `set_span`, eighteen arms of
`ELit(v) -> ELit({ span: s, lit: v.lit })`, to move one integer pair.
`patterns.ply::set_pat_span` is the same function again for the same reason, and
`respan` in my pre-integration draft was a third.

## §7 `no_brace` has to be restored on the bailing path too, and `mem::replace` got that for free

Eight of the reference's ten `mem::replace(&mut self.no_brace, ..)` sites are in
this area. The Rust shape is:

```rust
let saved = std::mem::replace(&mut self.no_brace, false);
let r = self.block_inner();
self.no_brace = saved;          // runs whether `r` is Ok or Err
r
```

The restore happens on the error path because the `?` is in the *caller*, not
here. In Ply the state is threaded, so the restore is
`{ p: with_no_brace(r.p, saved), node: r.node }` and it is only correct because
I remembered to write it after the call rather than inside the success branch.
Get it wrong and `no_brace` leaks out of a failed scrutinee into the recovery
that follows — a bug with no analogue in the reference and no diagnostic here.
`call_args` carries a comment saying so, and `arm-exprs.sh` arms both the set and
the clear.

## §8 No tuples, so three functions answer with a record that is not even `R<a>`

`spikes/ply-lexer/GAPS.md` §9 counted three record types in a lexer that existed
only because a function answers with more than one thing. A parser's *commonest*
type is "a node and the next state", so the shared `R<a> = {p: P, node: a}` pays
that tax once for all ~93 functions. But three functions in this module answer
with **three** things and need a record of their own:

- `call_args` → `RArgs = {p, close, args}` (`PResult<(Vec<Expr>, Span)>`)
- `match_arms` → `RArms = {p, close, arms}` (`PResult<(Vec<MatchArm>, Span)>`)
- `bin_op` → `Op = {op, bp}` (`Option<(BinOp, u8)>`)

plus three loop accumulators that exist for the same reason (`BlockAcc`, `HAcc`,
and `Acc<MatchArm>` from the spine). Six record declarations for six tuples.

## §9 `iterate` is the thing that makes this work, and the ceiling never came near — X1 and X2

ADR 0022 §8 asks what happens *"if a Ply parser is written and the ceiling bites
anyway"*. On this area it does not, and the margin is two orders of magnitude.

**X1/X2, registered before the numbers existed**, on the integrated modules with
a `dmax` high-water mark added to a scratch copy of the shared spine (the shipped
spine is untouched). Deterministic, so N = 1. Instrument checked; load 4.65.

| shape | size | nodes | `depth` high-water | ceiling diagnostic | exhausted budget |
| --- | ---: | ---: | ---: | --- | --- |
| left-associative `+` chain | 5,000 terms | 9,999 | **1** | none | none |
| list literal | 5,000 elements | 5,001 | **2** | none | none |
| block of `let` statements | 2,000 statements | 4,002 | **2** | none | none |
| parenthesis nesting | 100 deep | 1 | **101** | none | none |
| mixed (the X4 source) | 14,742 tokens | 8,141 | **6** | none | none |

`MAX_DEPTH` is 128 and `DEFAULT_MAX_CALLS` is 10,000. **A five-thousand-term
chain is one frame.** That is the concrete claim ADR 0022 §2 makes about the
reference — every sequence driven by a loop, recursion reserved for grammar
nesting — re-derived in Ply, and it is the thing `iterate` was added for.

Two things worth stating beside it:

- **The contrast with the lexer spike is the whole point.** `lexer.ply` drives
  its scan with `fold(range(0, n + 1), ..)` because `fold` cannot stop, and
  wastes 140,108 of 159,684 iterations on `desk.ply` — 87% no-ops, and a
  `range` list of 159,684 boxed `Int`s materialised to be discarded (GAPS §5).
  This parser has eight `iterate` drivers, materialises no `range`, and stops on
  the real condition. **The budget `c.ntok - p.pos + 1` is a backstop that
  cannot fire**, the shape `crates/ply-std/ply/http.ply:1584 stream_chunks` uses.
- **The budget is strictly better than the reference's `while`.** An item parser
  that consumed nothing would spin `parser.rs`'s loop forever; here it exhausts
  the budget and reports. That is one place the Ply port is safer than what it
  ports.

The one shape that *is* bounded by nesting rather than by length is
parenthesisation, at 101 frames for 100 parens, and `MAX_DEPTH` catches it at
128 exactly as it does in the reference — `exprs.ply` has a test at 200 that
bails with one diagnostic and one at 60 that does not.

## §10 What it costs, and the ratio the spike exists to produce — for *these two phases on this input*

Same X4 series as §3, plus a Rust control: a binary linking `ply-syntax` **by
path** and calling `lexer::lex` and `parser::parse_expr` over the *same* source
text, timed the same way. Both sides subtract a lex from a lex-plus-parse. On the
Ply side the lex term is `L − D` and the parse term is `P − M`, so the 0.03 s of
map-building sits in **neither** — it is work the reference does not do at all,
and burying it in one of the two terms would flatter or penalise the ratio for a
reason that has nothing to do with parsing. It is priced on its own in §3.

| | Ply | Rust |
| --- | ---: | ---: |
| lex | 0.27 s | 237.8 µs |
| parse (the phase alone) | 0.62 s | 482.3 µs |
| **parse ÷ lex** | **2.30** | **2.03** |

Rust control, N = 5, minimum, per rep over 2,000 reps, load 2.89 → 2.98:
`lex` 240.7 / 237.8 / 243.9 / 246.7 / 247.1 µs; `parse_expr` (which includes its
own lex) 720.0 / 725.2 / 739.0 / 733.1 / 739.4 µs.

**R = 2.30 ÷ 2.03 = 1.13.** A second, earlier take of both series — kept and
reported rather than discarded — gives Ply 2.59 and Rust 1.87, so **R = 1.39**.
Across both takes **R is between 1.1 and 1.4**.

What that means, in the words ADR 0020 §6.2 used about itself: for **two phases
of six, on one synthetic expression-shaped input**, Ply pays a roughly
**uniform** per-phase tax rather than a disproportionate one on the phase that
builds records and lists. The specific worry §6.2 raises about its own estimate
— that extrapolating from a lexer would understate a parser because a parser
allocates more — is not visible here at more than about 1.4x.

**This is emphatically not M1.** M1 is defined over the 23 real corpus files and
needs the Items area and a whole-module entry point. This is an expression-only
probe on generated source, and it must not be quoted as M1. What it can settle is
the *shape* question, and it settles it only for these two phases.

The absolute figures, since they are the other half of the picture and the less
flattering half:

- Ply, lex + parse: **35 KB/s**, **16,000 tokens/s**.
- Rust, lex + parse: **45 MB/s**, **20.5 M tokens/s**.
- **≈ 1,280x.**

`spikes/ply-lexer/GAPS.md` §15 reports "roughly an order of magnitude slower"
and that figure is not comparable to this one: it compares a Ply lexer against
`ply check`, which is six phases plus content hashing plus file I/O. Phase
against the same phase, it is three orders. A self-hosted front end at this speed
would not be a slow part of the build; it would be the build.

The walk cost is **UNMEASURED** by my own registered rule: `parse_count` (total
tree walk) minus `parse_pos` (parse, drop the tree) is 0.02 s against a spread in
the `parse_pos` series of 0.01 s and in the `parse_count` series of 0.02 s. The
difference is not larger than its own noise, so it is reported as its windows and
not as a number. What it does establish is that the walk is small enough not to
distort the comparison with the Rust control, which does not walk.

## §11 `effect` is a keyword, so the AST cannot use `ast.rs`'s field name

`AtomExpr.effect` and `HandleClause.effect` are the reference's field names.
`effect` is a Ply keyword and there is no raw-identifier escape — no `r#effect`,
no backticks — so both are `eff` here, in this module and in `types.ply`. It
costs nothing at runtime and it means a reader diffing the port against `ast.rs`
finds a name that is not there. Recorded because a reviewer *will* notice it and
should not have to rediscover why.

## §12 The reference has an `unreachable!()` and Ply has no expression for it

`parser.rs:1899`'s negative-literal arm ends `_ => unreachable!()`, guarded by a
`matches!` two tokens earlier. Ply has no expression that means "cannot happen"
and has the arm's type, so the arm has to answer something. `patterns.ply` answers
`LBad`, this module answers `no_expr()`, and in both cases a value the reference
cannot produce is now producible if the guard is ever wrong. `panic` exists as a
builtin but takes the arm out of the pure fragment.

Small, and it is the third file in this spike to hit it.

## §13 What the language handled fine

Stated because a gap list with no negative entries is a gap list that went
looking.

- **The generic, higher-order sequence driver typechecks.** The plan registered
  the risk that `comma_list<a>(c, p, close, item: (Ctx, P) -> R<a>) -> R<List<a>>`
  — generic, with a callback handed on to `iterate` — would not, and would become
  eight hand-monomorphised copies. It does, and this module calls it at five
  sites with four different element types — `Expr` twice (call arguments and list
  items), `RecField`, `Param` and `Ident`. **Zero copies needed**, which is what
  was predicted.
- **`iterate` is exactly the right primitive**, and §9 is the evidence. Five
  drivers in this module — `bin_expr`'s left spine, `postfix_expr`'s chain,
  `block_inner`'s statements, `match_arms` and `handle_rest` — plus the spine's
  `comma_list` behind five more call sites. No `range`, no wasted round, depth 1.
- **Mutually recursive types are ordinary.** §1.
- **The typechecker caught the port's mistakes before it ran.** 1,204 lines of
  Ply against 634 lines of Rust (`parser.rs:1211-1844`), ported by hand.
  An earlier draft of this module — written against a spine of my own, before
  the shared one landed — typechecked on the second attempt (the first failed on
  one duplicate definition) and then passed twenty-three of its twenty-seven
  tests on the first run; **all four failures were wrong numbers in my
  hand-computed expected strings, not wrong parses.** That is a small finding of
  its own: a dump comparison is only as good as the arithmetic in its fixtures,
  and hand-counting byte offsets is where this work actually goes wrong. The
  version in this directory is that file re-expressed against the shared spine,
  so its clean first run is a translation's, not a port's, and is not evidence of
  anything.
- **Structural equality on constructors works** and made the whole `Tok`
  comparison layer a non-problem (§2).
- **`test` blocks in the language are good.** Twenty-seven of them here, named as
  English sentences, run by `./test-exprs.sh` in 0.08 s.

## The strategic answer, for this area only

Can Ply host the expression half of its own parser today? **Yes, and the thing
that made it possible is `iterate`.** Without it the left spine of `bin_expr`,
the postfix chain, the statement loop and the arm loop would each have been a
recursion per element, and `crates/ply-std/ply/db.ply` at 29,213 tokens would
have died a third of the way through. With it the deepest stack this area
produces on any shape tested is **101 frames against a bound of 10,000**.

The two things that would most change the experience, in order:

1. **A `list_at` builtin.** Not because it is the largest cost (§3 says it is
   not) but because its absence is what forces the entire token buffer into a
   `Map` and makes the most frequent operation in the parser an O(log n) descent.
   It is the one gap here that changes the architecture rather than the
   ergonomics.
2. **Record update syntax.** §6, and it compounds with
   `spikes/ply-lexer/GAPS.md` §1 exactly as that entry warned: the workaround for
   the positional tax is *the order you write the fields in*, and there are nine
   of them written out eight times.

Third, and much smaller than I expected going in, is the absence of `?`. The
`bail: Bool` design works, the guards are mechanical, and the one real hazard it
creates is testable — §4 — once you know to test it.
