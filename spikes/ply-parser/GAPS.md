# What Ply could not do, written down while writing a parser in it

This is the deliverable of the spike. The five `.ply` modules are the vehicle.

It consolidates the four area files — [`GAPS-spine.md`](GAPS-spine.md),
[`GAPS-types.md`](GAPS-types.md), [`GAPS-exprs.md`](GAPS-exprs.md),
[`GAPS-items.md`](GAPS-items.md) — and the integration's
[`GAPS-harness.md`](GAPS-harness.md). **Those five stay.** They are the record of
what each area hit while it was hitting it, they carry the per-area measurements
and every run of them, and they are cited here by section rather than copied. What
this file adds is the thing none of them could do alone: **one ranking, across the
whole parser, of what each gap actually cost** — and §13, the figure ADR 0021 §3
says nobody had taken.

Ordered by cost, most expensive first. The ordering is not an opinion:
§1 chose the parser's central data structure, §2 created a class of bug that
cannot exist in the reference and left 63 of 83 guards unverifiable, §3 and §4 are
paid at every one of ~93 parse functions, and the entries below §8 are each under
a dozen lines. Two entries (§10, §12) record a gap that **cost nothing**, and one
(§12) records a language feature doing exactly what it was added for; a gap list
with no such entries is a gap list that went looking.

**Provenance for every number here.** Machine: `docs/ONBOARDING.md` §Provenance.
Instrument: `target/release/ply`, with `.github/binary-is-current.sh` printing
`current  target/release/ply  (152 inputs checked)` immediately before **and
after** every series in §13. Load (1-minute `uptime`) on both sides of every
series, 3.80–9.40 throughout. Timing statistic pre-registered as the **minimum
user CPU of N runs**, N = 5 under two seconds and N = 3 otherwise, wall clock
beside it, every run printed, **no run discarded**. The registration is
`/tmp/ply-parser-spike/PREREGISTRATION-MULTIPLIER.md`, written before any number
in §13 existed; the earlier ones are `PREREGISTRATION{,-SPINE,-INTEGRATION}.md`
and `PREREG-AREA1.md`. Counts (§14) carry no stopwatch and are immune to load.

**Where a claim of an area file's is repeated here it was re-derived, not
quoted.** Where my re-derivation differs from theirs the difference is stated in
place: §2's guard count and §14's line ratios both do.

---

## §1 A `List` has no index, and on a parser that is not a bite — it is the architecture

`spikes/ply-lexer/GAPS.md` §10 files the `List` surface — `len, push, map,
filter, fold, range`, now also `iterate` — as something that "starts to bite".
It ranks it tenth of fifteen. **On a parser it ranks first**, because a parser
*is* `tokens[pos]`, `tokens[pos + 1]`, `tokens[pos + 2]`: `parser.rs::kind_at` is
called with 0, 1 and 2, and `at_law_start` uses all three in one predicate.

There is no `nth`, no `head`, no `tail`, no `last` and no index
(`crates/ply-eval/src/builtins.rs:203+`). So `lexer.ply`'s `List<Token>` **cannot
be a token buffer at all**, and it is folded once into a `Map<Int, Token>` — the
only random-access container in the language. Every peek is an O(log n)
red-black descent with a `Value::cmp` at each node (`crates/ply-eval/src/value.rs:48`)
and an `Option` to unwrap, where the reference does one bounds-checked load.

What there *is* is **list patterns in `match`**, and `GAPS-types.md` §P2 read the
two evaluators to find out what they cost:

```rust
// crates/ply-eval/src/interp.rs:975 and crates/ply-eval/src/machine.rs:2301
let tail = Value::list(xs[items.len()..].to_vec());
```

`Value::List` is `Arc<Vec<Value>>`, not a persistent sequence, so `[x, ..t]`
copies the tail — **and so does `[x, ..]`**, because a bare `..` is given a
`Wildcard` sub-pattern that the evaluator materialises the tail to match against.
`crates/ply-std/ply/db.ply`'s `match ts { [TWord(w), ..] -> w, .. }` therefore
copies the whole remaining token list to look at one token. On a SQL statement
that is free. Threading `List<Token>` as "the rest" through desk.ply's 19,576
tokens the way `db.ply` threads it **would be quadratic**. That is a mechanism
claim read off both evaluators, not a measurement, and it is why the `Map` is
forced rather than preferred.

**It cost four separate workarounds, not one** (`GAPS-items.md` §P1):

| the reference writes | there is no | so the port carries |
| --- | --- | --- |
| `self.tokens[self.pos + n]` | index | `Map<Int, Token>`, a `Buf` type, and `tok_index`'s 8 lines |
| `items.first()` for a secondary label | `head` | a fourth field `first: Option<Span>` threaded through the module loop |
| `self.diags.last()` for the dedup rule | `last` | **two more fields of `P`**, `last_code` and `last_span`, threaded through ~93 functions |
| `params.into_iter().next()` | `nth` | a `fold` |

The third is the one that propagates furthest. `Parser::push` (`parser.rs:207`)
drops a diagnostic whose code and primary span match the previous one, and it
changes the diagnostic list *exactly*, so it is ported exactly. Folding the list
to find its end would be a **quadratic hidden inside a deduplication rule** —
the last place anyone would look for one.

**What it cost at run time is only partly known.** `GAPS-spine.md` §S12/S2
measured the one-time buffer build and it is **linear** (2.05 and 1.79 per
doubling, against a registered quadratic threshold of 3.4) — so the field-order
tax of §4 does not visibly apply to a `Map`, which is worth knowing beyond this
spike. §S12/S3 tried to price the build against lexing and came back
**UNMEASURED**: the difference was smaller than the spread of its own series, and
it is reported as windows rather than as a number. `GAPS-exprs.md` §3 then
measured the *per-peek* half and **refuted its own author's registered
prediction**: the `Map` lookup is about a third of the parse, not the largest
cost in it.

**A `list_at` builtin removes the first and fourth. `list_last` removes the
third. `list_head` removes the second.** None of the three is measured.

---

## §2 No `?`, and the flag that replaces it leaves 63 of 83 guards unverifiable

`parser.rs:11`'s `Bail` is a **zero-field struct**, so `PResult<T> =
Result<T, Bail>` is isomorphic to `Option<T>` and the error channel carries
nothing. A `bail: Bool` in the threaded state is therefore the *same type*, not
an approximation — and the guard goes on the **callee** instead of the call site,
which is strictly better than the shape `crates/ply-std/ply/json.ply` is forced
into (`decode_map`/`decode_and_then` at `:99-112`, one number literal split
across seven functions, `spikes/ply-lexer/GAPS.md` §12).

On writing cost the trade looks good: the reference has **178 `?` operators**
(`GAPS-spine.md` §S3's count; a naive `grep -o '?' | wc -l` says 187 because
doc-comment prose contains question marks) and the port has **83 guard sites** —
counted on the integrated tree, `grep -c 'if p.bail'` giving 9/10/1/27/21 and
`grep -c 'if s.p.bail'` giving 1/2/2/3/7 across spine/types/patterns/exprs/items.
*The four area files, counted before integration, sum to 77; the eleven extra are
sites added while the areas were joined, and the difference is recorded rather
than reconciled away.*

**Then each area deleted its guards one at a time and asked whether anything
noticed.** That is the entry.

| module | guards | deleted individually | changed anything observable |
| --- | ---: | ---: | ---: |
| `spine.ply` | 8 | 8 | 6 (+2 asserted **equivalent**) |
| `types.ply` | 10 | 10 | **0** |
| `patterns.ply` | 1 | 1 | **0** |
| `exprs.ply` | 27 | 27 | 13 |
| `items.ply` | 31 | 17 | **1** |
| total (as the areas counted them) | 77 | 63 | **20** |

Nothing noticed means: not the type checker, not the 112 in-language tests, and
not a differential against the shipping parser over the error fixtures.

### **20 guards are demonstrated to matter. The other 63 are not — 43 were deleted and nothing moved, and 20 were never deleted at all.**

Two of the 43 are not merely undetected but **provably equivalent mutants** — a
guard on a function whose first act is a call to an already-guarded function
cannot change anything — and `arm-spine.sh` asserts those two *stay* green, which
is the honest way to record it: an equivalent mutant is not a hole in a test, and
treating one as a hole is how a suite grows assertions about an implementation
detail. The other 41 are undetected because every *consuming* primitive
(`advance`, `eat`, `expect`, `expect_close`, `expect_ident`, `expect_gt`) guards
on `bail` itself, so a guardless parse function reads no tokens, emits no
diagnostics, builds a node its caller discards, and is indistinguishable from the
guarded one.

**The one guard in `items.ply` that is load-bearing is worth writing out**, because
it is exactly the failure mode the design creates (`GAPS-items.md` §P2):

```
fn f() -> Int where derivable(zz, a) = 1
```

`deriver` bails with `E0207` at 30..32. `where_clause` and `spec_clauses` stop.
`fn_body` then runs with `bail` set; `eat(t_eq())` refuses because `eat` guards —
but `at(c, e.p, t_lbrace())` is an **ordinary predicate that answers `false`**, so
control reaches `error_here`, which the spine does not guard, and a phantom
`E0001` at 32..33 appears that the reference never raises.

**This is a class of bug that cannot exist in the reference**, where `?` returns
before the caller is entered. `GAPS-exprs.md` §4 records that **ten functions in
that module violated the invariant on the first write** — all of them the ones
dispatched on a token kind — and that the invariant itself is one sentence Ply has
no way to state: *a parse function called with `p.bail` true answers `p` unchanged
and consumes nothing.*

Two further things the sweep taught that no amount of reading would have:

1. **The invariant's testability is input-dependent, so the obvious test is
   vacuous.** `postfix_expr` had no guard at all and its test still passed,
   because it was called on `f(1)`, whose first act reaches the *guarded*
   `qname`. Changing the input to `1` — whose first act is an unconditional
   `advance` — failed immediately. A test of this invariant must pick an input
   the unguarded body would consume, and **nothing says which inputs those are.**
2. `types.ply`'s and `patterns.ply`'s zeroes are a different thing from
   `exprs.ply`'s: those two modules have **no bail-invariant test at all**, so
   their eleven guards are unarmed. That is a hole in their tests, not in their
   parsers, and the sweep is what found it.

**The mitigation is a discipline the language cannot enforce**: put the guard on
the primitives and the per-function guards become defence in depth. A reader
maintaining these files has no way to know that deleting a guard is safe, and no
way to know that deleting *the* guard is not.

---

## §3 No tuples, and a parser's commonest type is a pair

`spikes/ply-lexer/GAPS.md` §9 counted **three** record types in the lexer that
existed only because a function answers with more than one thing, and ranked it
ninth. Here the count across the five modules is **over twenty**, and one of them
is on the return type of essentially every function in the program:

```ply
pub type R<a> = { p: P, node: a }        //  PResult<T> over &mut self
```

| type | stands for | reference |
| --- | --- | --- |
| `R<a>` | a node **and** the next state | `PResult<T>` + `&mut self` |
| `Ate` | `eat`'s `bool` **and** the next state | `fn eat(..) -> bool` |
| `Acc<a>`, `BlockAcc`, `HAcc`, `ModAcc`, `SetAcc`, `Rec`, `RowAcc`, `ListAcc`, `RecAcc` | a loop's several accumulators | locals in a `while` |
| `Start` | a buffer **and** an initial state | `Parser::new` |
| `Buf`, `DAcc` | a fold's counter **and** its container | `.enumerate()` |
| `RArgs`, `RArms` | a node, a span **and** a state | `PResult<(Vec<_>, Span)>` |
| `Op` | an operator **and** its binding power | `Option<(BinOp, u8)>` |
| `RDeriver`, `RBinders`, `RStr`, `RSetMembers`, `RModule`, `RowOut`, `ListOut`, `RecOut` | ditto | ditto |

**The tax is not the declarations. It is that `R<a>` is threaded by hand through
every call site in the parser**: `let a = f(c, p); let b = g(c, a.p); ...` where
the reference writes `let a = self.f()?; let b = self.g()?;`. One extra
identifier and one extra field access per call, ~93 functions deep.

`GAPS-items.md` §P4 makes the sharpest version of the point: three of its eight
tuple-substitutes are worse than a pair, because they carry **two lists and a
state** — and a record that carries two growing lists is §4's unsolvable case.

---

## §4 No record update, and `spikes/ply-lexer/GAPS.md` §1's field-order rule stops being local

Two gaps that are separately mild and together sharp. Neither is new; what is new
is what they do to each other on a program with a nine-field threaded state.

**No record update syntax.** `{..p, pos: n}` does not exist
(`spikes/ply-lexer/GAPS.md` §2), so changing one field of `P` means writing all
nine. The lexer's `Scan` had three fields. `P` has nine — `pos`, `no_brace`,
`depth`, `gt_split`, `uses_sets`, `bail`, `last_code`, `last_span`, `diags` — and
the spine spells all nine out **ten times**, once per constructor.

**The field-order rule.** `spikes/ply-lexer/GAPS.md` §1, measured: a growing
container built anywhere but in the **last sub-expression of its enclosing node**
is copied instead of updated in place, so an accumulator in the wrong position is
quadratic — and nothing in the language says so and nothing checks it. ADR 0020
§5.2 sharpens it: **the tax is not local.** A correct callee is destroyed by a
caller that puts the call anywhere but last.

Multiply them. Nine fields, one of which (`diags`) grows, times the 178 places the
reference writes `?` and the port threads a new state, is **1,602 chances to put
`diags` anywhere but last** — each silent, each quadratic. `spine.ply`'s answer is
a rule in its own header: *do not write a `P` literal.* Ten constructors, and
every other module calls them.

**Three things follow, and the third is the one with no answer.**

1. **The discipline is unenforceable.** Nothing stopped the other four areas
   writing a `P` literal and nothing would have reported it — not the typechecker,
   not `ply test`, not a benchmark that only runs small inputs.
2. **One instance of the tax is knowingly paid.** `{p: push_diag(a.p, d), node: X}`
   puts the push in a non-final position, so the push copies. `diags` holds 25
   elements over the whole corpus, so the total is O(625): negligible, recorded so
   that it is a decision rather than an oversight, and **it stops being negligible
   on a file of errors** — which is exactly the fixture corpus the recovery half
   needs.
3. **A loop that grows two lists at once has no correct spelling.** A record
   literal has one last field. An effect row grows `atoms` and `aliases`; `run`'s
   module loop grows `imports` and `items`; `effect_set_def` grows `atoms` and
   `includes`; `list_items` grows `rest` and `items`. The port ranks them by which
   grows on real input, gives that one the slot, and writes the reason down —
   `GAPS-types.md` §P3 goes further and writes **four record literals where the
   reference writes two `Vec::push` calls**, with *the field order differing
   between two branches of the same record type*, which is legal and which a
   reader will "tidy up". On this grammar the loser never gets long. **On a
   grammar where both lists were long it is unfixable in one loop**, and the
   escape — splitting the loop in two — is the one `spikes/ply-lexer/GAPS.md` §1
   column 3 shows doubles the recursion depth.

The same missing feature costs one more thing, in three copies. `parser.rs:1946`
is one line:

```rust
inner.span = open.to(close);
```

The Ply AST puts the span **inside every variant** (that is what made the AST
representable, §12), so "set the span" is a `match` with one arm per variant:
`exprs.ply::set_span` is **eighteen arms**, `patterns.ply::set_pat_span` is seven,
and a third existed in a pre-integration draft. **The shape that made the AST
representable is the shape that makes a span rewrite expensive**, and it is
load-bearing rather than cosmetic: `(a)` as a type is a `TVar` whose span is its
name's, while `(x)` as a pattern is a `Var` whose span is **wider** than its
name's. `arm-harness.sh` mutation #3 watches exactly that, and §15 is the story of
how nearly it went unwatched.

---

## §5 No constants: 54 nullary functions, and `at()` is four interpreter calls

Ply has no `const` and no top-level `let`. Every token the parser compares against
is a function:

```ply
pub fn t_comma() -> lexer::Tok = lexer::TPunct(b"comma")
```

**47** of them, plus **6** diagnostic codes, plus `max_depth`. `spine.ply` has 123
functions against the 19 of `parser.rs`'s preamble it ports (`GAPS-spine.md` §S0
counts 121, from the same stripper difference §14 records), and
`GAPS-spine.md` §S0 decomposes the difference exactly — 47 token names, 8 `P`
constructors (§4), 6 codes, 16 dump-encoder functions with **no counterpart on
the reference side at all**, and 7 helpers Rust gets from `ply_span` and `ast.rs`.

The cost is not the 54 declarations. It is that `at(c, p, t_comma())` is **four
interpreter calls** — `t_comma`, `at`, `kind`, `tok_at`→`tok_index` — plus a
`map_get`, plus a structural `==` on a constructor with a `Bytes` payload, where
the reference is one load and a discriminant compare. **`at` and `kind_at` are the
two most frequent operations in the whole program.**

`spikes/ply-lexer/GAPS.md` §11 saw this on the lexer's `at` and called it "a
second sighting". This is the third, and it moved from a helper to the inner loop.

Note precisely what would and would not fix it. A `const` removes one of the four
calls. `list_at` (§1) removes the `map_get`. **Neither removes the `Bytes`
comparison inside `TPunct(b"comma") == TPunct(b"comma")`** — that one is
`lexer.ply`'s representation choice, and it is the price of the lexer spike having
had no reason to number its punctuation. `GAPS-exprs.md` §2 tried to fix it with a
precomputed integer kind per token and **measured its own optimisation 21%
slower** (0.75 s against 0.62 s), because the extra record field per token cost
more than the comparison saved. That is reported as a two-implementation
comparison, not as an isolated measurement of dispatch.

---

## §6 Ply reserves its keywords in the field namespace, and the AST being ported uses two of them

```
[E0001] Error: expected a field name, found keyword `effect`
 47 │ pub type AtomExpr = { effect: QName, mode: Mode, ... }
```

`ast.rs:773` declares `pub effect: QName`; `ast.rs`'s `EffectDef` and `TestDef`
declare `pub nondet: bool`. **All fifteen Ply keywords are refused as record field
names** — checked exhaustively by `GAPS-items.md` §P3 — and Ply has no
raw-identifier escape, no `r#effect` and no backticks. So the port writes `eff` and
`is_nondet`.

Cost here: two renames, eight sites. **Cost in general: a permanent asymmetry.**
The Ply AST is not field-for-field the AST it mirrors, so anything mapping between
them needs a rename table, and a reviewer diffing the two files sees a difference
that is not a difference. Four of the fifteen keywords — `type`, `test`, `with`,
`effect` — are ordinary nouns a domain model will want.

This is the cheapest entry to fix and the most surprising: the error is at the
declaration, so it is cheap to hit, and `{ nondet: Bool }` has no other reading.

---

## §7 Immutability meets a parser that rewrites its own token stream

`parser.rs:187` splits `>=` into `>` then `=`, for `type Pair<a>= ..`, by
**assigning into the token buffer**:

```rust
self.tokens[self.pos] = Token { kind: TokenKind::Eq, span: Span::new(.., start + 1, end) };
```

The Ply buffer is immutable, so the rewrite becomes a *position*: `gt_split: Int`
in `P`, applied by `tok_index` on read. Which is fine — until you notice that a
file may contain two such splits and one `Int` holds one.

**The cost is the argument, not the field.** A split at index *k* is observable at
*k* (through `kind`) and at *k+1* (through `prev_span`, which reads `pos - 1`) and
nowhere else; `pos` never moves backwards; `expect_gt` is reached only from
`generics` and `type_def`, both at the close of a type-parameter list, so two such
lists cannot be adjacent. One slot suffices. The reference never has to make that
argument, because its rewrite is permanent.

It is also **true today and silently false after some later change to how the
parser backtracks**, and Ply has no `debug_assert` shape to pin it with. Two tests
stand in. This is not a Ply defect — it is what immutability costs when the thing
being ported is a mutation — and it is the shape a self-hosted compiler will meet
everywhere its reference mutates.

---

## §8 No `unreachable!()` that is also an expression, so four placeholder variants exist

`parser.rs:1897` writes `_ => unreachable!()` in the negative-literal arm, because
the two-token lookahead that reached it already proved the token is a number. Ply
has no expression that means "cannot happen" **and has the arm's type**, and no
way to tell the exhaustiveness checker an arm is dead.
`crates/ply-std/ply/db.ply:963` meets the same wall and answers a plausible value.

This port answers **a value the reference cannot build** — `TBad`, `PBad`, `LBad`,
`MBad`, `EBad` — so that if the arm is ever reached, the differential shows a tag
the reference cannot emit rather than a literal that looks right. `panic` exists
as a builtin and was not used: it would abort the run rather than produce a
comparison, and the point of a placeholder is to be **visible in the comparison**.

**Cost: one extra variant per sort, and the knowledge that an impossible arm is
silently a real one.** The payoff is measurable and was measured:
`GAPS-types.md` §P12 reports `badlit` and `badmode` as the only two of that area's
28 dump tags never reached, and that is the *evidence* that no nested placeholder
escapes into a tree — which is what the four variants bought.

---

## §9 Small missing pieces, each cheap, listed because a bootstrap pays them all

- **No `min` / `max`.** `(self.pos + n).min(len - 1)` is a nested `if`. Three in
  the spine, `span_to` included.
- **No `saturating_sub`.** One more `if` inside `tok_index`.
- **No `matches!`.** `is_ident` and `is_str` are two-arm `match`es where the
  reference writes one line inline.
- **No statement form.** Everything is an expression, so the reference's
  `self.advance();` — which drops the span — is `bump`, a second function.
- **No builder chaining.** `Diagnostic::error(..).primary(..).secondary(..).note(..)`
  becomes `diag1`/`diag2` taking a note **count**, because notes are prose and
  prose is not compared (§11).
- **No `mem::replace`.** Eight of the reference's ten
  `mem::replace(&mut self.no_brace, ..)` sites are in the expression grammar, and
  the Rust shape restores the saved value **on the error path for free**, because
  the `?` is in the caller. In Ply the restore is written by hand after the call;
  get it wrong and `no_brace` leaks out of a failed scrutinee into the recovery
  that follows — **a bug with no analogue in the reference and no diagnostic
  here** (`GAPS-exprs.md` §7). `arm-exprs.sh` arms both the set and the clear.
- **No `Box`.** Costs nothing; Ply needs no box.

---

## §10 No Unicode, and this time it costs nothing — with the reason, which is the interesting part

`parser.rs:2078 starts_upper` asks `char::is_uppercase`, and it decides
constructor-versus-binder at every bare name in a pattern and every bare name in a
type. Ply has no character type and no Unicode table
(`spikes/ply-lexer/GAPS.md` §7), so `spine.ply`'s is ASCII `A`..`Z`.

**The two parsers cannot differ on this, and the reason is upstream rather than
lucky.** `lexer.ply` refuses any token whose first byte is ≥ 128 with its own
`X0001`, so an identifier that reaches `starts_upper` has an ASCII first byte by
construction. The divergence is the lexer's, already recorded in
`spikes/ply-lexer/README.md`, and this is not a second one.

It *would* become a second one the moment `lexer.ply` grew a Unicode table. That
is the shape of every deferred gap in a bootstrap: **it does not compound until
the thing below it is fixed.**

---

## §11 What the comparison cannot see, said before it is green

Three silences, all sized rather than adjectival. `GAPS-harness.md` §H2 is the
enforced list; this is the ranking of it.

1. **Diagnostic message text and severity.** `error_here`, `unclosed`, `expect`,
   `expect_ident` and `expect_gt` all carry a `what: Bytes` naming what was
   expected and **never read it** — **134 call sites** in the reference, and
   `GAPS-items.md` §P10 counts 105 literals written for it in one area alone. The
   parameter is carried anyway so that turning messages on is a change to
   `error_here` alone rather than a rewrite of 134 call sites: *the literals are
   paid, the table is not.* Turning it on additionally needs
   `TokenKind::describe`'s ~40 arms — one more ~40-arm `match` returning `Bytes`.
   Severity is dropped because every parser diagnostic is `Severity::Error`; a
   warning added to the parser would be invisible.
2. **`effect_set::expand` is not ported** — 521 lines. §H4 prices the hole
   exactly: 1 of 21 corpus files (**21.2% of corpus bytes**), 20 of 716 mined
   fixtures, **0 trees still disagreeing**, 7 inputs where a diagnostic differs.
   For those the expansion is projected back out of **the reference's own output**
   rather than reimplemented, and the tolerance's count is asserted at 7 so it
   cannot grow.
3. **`Lit::Decimal`'s mantissa and scale, and `Lit::Float`'s value.** Both sides
   dump the raw source over the literal's own span. Ply can build neither an `f64`
   nor an `i128` from digits (`spikes/ply-lexer/GAPS.md` §3, §4), so a dump
   carrying the value would compare `numerics.rs` against nothing. Note the
   direction: this **removes** a normaliser where the lexer spike's float hole
   added one, which is why ADR 0020 §3.2's amendment does not recur here.

**What *is* compared** is wider than the lexer spike's code-plus-span, and
deliberately: `parser.rs` has 45 diagnostic sites and **all but four raise
`codes::UNEXPECTED_TOKEN`**, so code plus span is a weak signature when almost
every diagnostic shares a code. Every label's own span, every label's primary
flag and the note count are compared too — 833 diagnostics, all of them.

**And a diagnostics-only comparison is blind to the tree entirely**, which
`GAPS-items.md` §P7 measured rather than asserted: of 15 mutations, **three are
caught only by the in-language tree tests** and pass a comparison of code + every
label's span + every primary flag + note count over 32 error fixtures. That is the
argument for the 854 code lines of Rust in `harness/src/lib.rs` — a reference-side
dumper that walks `ast.rs` **with no `_` arm in any `match`**, so a variant added
to the AST breaks compilation instead of being silently skipped.

---

## §12 The entries that are not gaps: `iterate`, and an AST that turned out to be representable

Two registered risks that did not materialise, both recorded because they were
written down as risks *first*.

**`iterate` did exactly what ADR 0022 added it for.** Twenty-one `iterate` sites
across the five modules drive every sequence in the parser, against the
reference's 16 `while` and 5 `loop`. `comma_list<a>(c, p, close, item: (Ctx, P) ->
R<a>) -> R<List<a>>` is one `iterate` replacing the reference's loop at **all 14**
of its call sites, it typechecks generic and higher-order on the first attempt,
and the registered fallback of eight hand-monomorphised copies was needed **zero**
times.

The depth measurements are the answer to ADR 0022 §8's *"if a Ply parser is
written and the ceiling bites anyway"*, and it does not:

| shape | size | `depth` high-water |
| --- | ---: | ---: |
| left-associative `+` chain | 5,000 terms | **1** |
| list literal | 5,000 elements | **2** |
| block of `let` statements | 2,000 statements | **2** |
| parenthesis nesting | 100 deep | **101** |
| the mixed 14,742-token probe | 8,141 nodes | **6** |

**A five-thousand-term chain is one frame.** `MAX_DEPTH` is 128 and
`DEFAULT_MAX_CALLS` is 10,000. `GAPS-types.md` §P10 then lifted `MAX_DEPTH` to a
million and bisected for where Ply's own ceiling actually bites: **1,661 levels for
a type, 1,681 for a pattern** — about six interpreter calls per grammar level. So
against the reference's own `MAX_DEPTH = 128` the headroom is **13×**, and against
ADR 0020 §5.1's measured corpus maximum of 17 it is **98×**. **The ceiling cannot
bite on any input the reference itself accepts**, because `MAX_DEPTH` cuts in
thirteen times earlier. That is stronger than "it did not bite on the corpus", and
it is what ADR 0022 §8 asked for.

Two things beside it. The budget `c.ntok - p.pos + 1` is a **backstop that cannot
fire** (a `Continue` costs at least one token), in the shape
`crates/ply-std/ply/http.ply:1584 stream_chunks` uses, and it materialises no
`range` list — against `lexer.ply`'s `fold(range(0, n + 1), ..)`, which cannot
stop and wastes **140,108 of 159,684 rounds on `desk.ply`**, 87% no-ops plus a
list of 159,684 boxed `Int`s built to be discarded. And **the Ply version is the
safer one**: an `item` that consumed no token and did not bail would spin the
reference's `while` forever, with no timeout anywhere in `ply-test` or `ply-cli`
to stop it; here it exhausts the budget and reports.

**The AST is representable, and the fallback ladder was not needed.** The spike's
one go/no-go was that `type Expr = {kind: EKind, span: Span}` with `EKind`
carrying `Expr` is mutual recursion between a named record alias and a named ADT,
and nothing in the shipped tree does it. Two areas probed it independently and
both got it on the first attempt: an ADT per sort with **anonymous records inline
inside the variants**, recursion through `List<Self>`, through a `List` of an
inline record holding `Self`, and through `Option<Self>`. `crates/ply-std/ply/http.ply:243`
was the precedent and it held. **That nothing in the shipped tree does this turned
out to be a fact about the shipped tree, not about the language.**

---

## §13 What it costs to run — the figure ADR 0021 §3 says nobody had taken

> ADR 0020 §6.2: *"To get from a lexer to a front end needs a multiplier, and
> there is no way to measure one without writing the rest. **This is an assumption
> and it is labelled as one.** In conventional compilers lexing is 10–20% of
> front-end time, which puts a full Ply front end at 5–10x the lexer's cost — call
> it 1,700–3,400 tokens/s."*

Pre-registered at `/tmp/ply-parser-spike/PREREGISTRATION-MULTIPLIER.md` before any
number below existed, including the exclusion rule and the predictions. This is a
**second, independent series** taken after `GAPS-harness.md` §H5's, in one sitting,
because §H5's own caveat and ADR 0020 §8's last bullet name the same defect —
figures compared across probe shapes and across sittings. **Here the Ply lexer, the
Ply parser and the Rust front end are all taken on one machine at one load band.**

Five probes per file, in five project directories each holding the same six
modules so that module typechecking is identical and cancels in every difference:
`Z` = `bytes_len(source())`, `L` = `len(lex(source()).toks)`,
`P` = `len(parse(source()).node.items)`, `LD` = `string_len(lexer::dump(source()))`
— **ADR 0020 §6.1/§15's own probe shape** — and `PD` = `string_len(items::dump(source()))`.
`.ply-cache` removed before every run.

### The multiplier

| file | bytes | tokens | lex (`L−Z`) | parse (`P−L`) | lex+parse (`P−Z`) | **(P−Z)/(L−Z)** | **(P−L)/(L−Z)** | lexing's share |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `examples/desk.ply` | 159,971 | 19,576 | 0.49 | 0.73 | 1.22 | **2.49** | 1.49 | 40% |
| `crates/ply-std/ply/db.ply` | 135,285 | 29,213 | 0.87 | 2.01 | 2.88 | **3.31** | 2.31 | 30% |
| `crates/ply-std/ply/http.ply` | 127,278 | 17,662 | 0.50 | 0.76 | 1.26 | **2.52** | 1.52 | 40% |
| `crates/ply-std/ply/json.ply` | 63,370 | 11,668 | 0.26 | 0.47 | 0.73 | **2.81** | 1.81 | 36% |
| `crates/ply-std/ply/router.ply` | 54,397 | 8,644 | 0.21 | 0.34 | 0.55 | **2.62** | 1.62 | 38% |
| **`examples/` as a whole (13 files)** | **333,883** | **45,041** | **1.15** | **1.86** | **3.01** | **2.62** | 1.62 | **38%** |

Minimum user CPU seconds. All five files survive the registered spread rule and
span 1.33× (<2 required), so the pre-registration permits a headline:

### **Lex-plus-parse costs 2.49–3.31× lexing alone. Parsing alone costs 1.49–2.31× lexing. Lexing is 30–40% of what has been spent after two phases.**

This **replicates** `GAPS-harness.md` §H5's independent series (2.44 / 3.03 / 3.30
/ 2.81 / 3.19) — every file within ±25%, the largest deviation `http.ply` at −24%
— and `GAPS-exprs.md` §10's 2.30 for parse ÷ lex on generated inputs, taken by a
third agent with a third probe design before either of the other two existed.

### Throughput, and the term §6.1 could not separate

| | this series | ADR 0020 §6.1 |
| --- | ---: | ---: |
| Ply lexer alone (`L−Z`) | **33,578–44,877 tok/s** | not separated |
| Ply lexer through `dump` (`LD−Z`), §6.1's shape | **26,083–36,462 tok/s** | ~17,000 tok/s |
| Ply lex+parse (`P−Z`) | **10,143–16,046 tok/s** | — |
| Rust front end, `ply check examples/` cold | **450,410 tok/s** (0.10 s user / 0.13 s wall) | ~215,000 tok/s (0.21 s user) |
| Rust front end warm | 0.01 s user / 0.02 s wall | 0.03 s user |

**The dump was 15–22% of what §6.1 measured** (`(LD−L)/(LD−Z)`; registered
prediction ">10%", held). §6.1's ~17,000 tokens/s therefore charged the Ply lexer
with a rendering step worth about a fifth of its own figure, and `GAPS-harness.md`
§H5's caveat that its ratios and §6.1's absolutes "are not directly comparable" is
now closed with a number instead of a warning.

**Both engines are 2–3× faster than §6.1 recorded, and the ratio between them is
not.** That is the registered prediction that missed, and it is the most useful
thing in this section: §6.1's cold `ply check` was 0.21 s user and is 0.10 s here;
its lexer figure was ~17,000 tok/s and the same probe shape gives 26,083–36,462.
Taken as a ratio in one sitting, **the Rust front end is 10.0–13.4× the Ply
lexer** — against §6.1's cross-sitting **12.6×**. §6.1's headline survives its own
sensitivity note.

### The scale figure, measured rather than projected

**Ply lex+parse over the identical 13 files `ply check examples/` reads costs
3.01 s user. The Rust front end does six phases over the same files in 0.10 s.**

### **That is 30×, measured, in one sitting, for two phases against six. Warm it is 301×.**

### What this settles about §6.2, and what it does not

**Settled.** §6.2's reasoning was borrowed — *"in conventional compilers lexing is
10–20% of front-end time"* — and on this language it is wrong at the point where
it can be checked: after two of the front end's phases, lexing is still **30–40%**
of what has been spent. That is now measured on this language and this corpus.

**Not settled, and it is most of it.** A front end is not lex plus parse.
`crates/ply-syntax/src/resolve.rs` is 1,177 code lines — 62% of `parser.rs`'s
1,898 — and inference is larger than either. Neither is written in Ply.
**§6.2's 5–10× band is not refuted.** Given the measured 2.62 over `examples/`,
the band requires the four unwritten phases to cost between **1.5× and 4.6× what
parsing cost**, which the line counts make entirely plausible. *That sentence is
arithmetic on a measured term and an assumed one, and it is labelled.*

**ADR 0020 §8's break condition is not met.** It names the figure that would
weaken the estimate: *"If a Ply parser and typechecker cost only 2x the lexer
rather than 5–10x."* Lex-plus-parse alone is already 2.49–3.31×, with the
typechecker unwritten.

**§6.2's derived numbers move, and its conclusion does not.** Redoing §6.2's own
arithmetic with today's measured lexer term instead of §6.1's (**projected** —
every figure in this paragraph multiplies a measured 1.15 s by §6.2's assumed
band):

| | §6.2 said | on today's measured lexer term |
| --- | --- | --- |
| a Ply front end over `examples/` | 13–27 s | **5.8–11.5 s** |
| its throughput | 1,700–3,400 tok/s | **3,900–7,800 tok/s** |
| against the Rust front end | 60–130× | **58–115×** |

**Both halves of §6.2's absolute figures roughly halved, and the ratio it rests on
did not move.** The rejection in ADR 0020 §7 is not sensitive to anything this
spike measured.

### Every run

Series logs, complete and undiscarded, at `/tmp/ply-parser-spike/series-frontend.log`,
`-2.log`, `-3.log`, `-examples.log`; derived tables in `derived.txt` and
`derived-examples.txt`; outcomes appended below the line in
`PREREGISTRATION-MULTIPLIER.md`. `./measure-front-end.sh` re-takes all of it.

**The registered 25% spread rule fired three times and every failed series is
kept.** Series 1's `desk.ply` `P` spread 27% (1.52/1.79/1.92/1.93/1.92); series
2's `desk.ply` `L` spread 64% and `LD` 32%; series 1's `router.ply` `PD` spread
74% (0.72 → 1.25, wall 0.73 → 3.09). `desk.ply` was taken a third time and
`router.ply` a second, both at 0–1% spread, and those are the reported rows.
Reporting a re-take is a selection, so **all three series are printed** and the
rule that judged them was registered before any of them ran.

---

## §14 What it costs to write

Counts, no stopwatch, so immune to the load gate. Segmentation is
`GAPS-items.md` §P8's, fixed before the integration began: in-language `test`
blocks excluded, code lines neither blank nor `//`. Re-derived here rather than
quoted; the script is in `/tmp/ply-parser-spike/writing-cost.txt`.

| | total lines | code lines | `fn` |
| --- | ---: | ---: | ---: |
| `spikes/ply-lexer/lexer.ply` | 618 | 370 | 58 |
| the Ply parser, 5 modules | 3,650 | 2,403 | 318 |
| `crates/ply-syntax/src/lexer.rs` | 1,069 | 950 | 48 |
| `crates/ply-syntax/src/parser.rs` | 2,114 | 1,898 | 98 |

| ratio | total | code | `fn` |
| --- | ---: | ---: | ---: |
| Ply parser ÷ Ply lexer | **5.91** | **6.49** | **5.48** |
| Rust parser ÷ Rust lexer | **1.98** | **2.00** | **2.04** |
| Ply ÷ Rust, on the lexer | 0.58 | 0.39 | 1.21 |
| Ply ÷ Rust, on the parser | **1.73** | **1.27** | **3.24** |

(`GAPS-harness.md` §H6 gives 5.95 / 6.50 / 5.52 for the first row from a slightly
different test-block stripper. The difference is four lines and it changes
nothing; both are recorded rather than reconciled.)

### **The step from lexer to parser costs Ply 2.99× what it costs Rust.**

**A lexer in Ply is *shorter* than its Rust original (0.58). A parser in Ply is
*longer* (1.73).** The two languages cross over somewhere between the two phases,
and the `fn` column says where it goes: **3.24 Ply functions per Rust function on
the parser, against 1.21 on the lexer.** §1, §2, §3 and §5 are the four causes,
and §5 alone accounts for 54 of the extra functions.

**This is the other reading of "multiplier" and it points the other way from
§13.** On *running* cost, Ply pays a roughly uniform per-phase tax and §6.2's
worry about its own estimate does not show up (§13, and `GAPS-exprs.md` §10's
R = 1.1–1.4 against a Rust control on the same input). On *writing* cost, the tax
is emphatically not uniform: it is 0.58 on a lexer and 1.73 on a parser. Redoing
§6.2's arithmetic on writing cost instead — a front end 3× a parser in Rust would
be **~18×** a lexer in Ply rather than 5–10× — **is an extrapolation from one
phase and is labelled as one, in §6.2's own words.**

One more number about method rather than about Ply: `harness/src/lib.rs` is **854
code lines of Rust**, one preorder walk of `ast.rs`, against `parser.rs`'s 1,898.
**A differential is not free** — the reference side of a parser dump is nearly
half the size of the thing it checks — and anyone pricing "and we would verify it
with a differential" should carry that line.

---

## §15 What the arming found, which is the part worth reading twice

Every area armed its own suite by corrupting the thing each check watches and
confirming it fails. **The first pass of every one of them found tests that were
watching nothing.**

- **`spine.ply`: six of its mutations left the suite green.** Two were bail guards
  whose bailed state sat on a token the function would have rejected anyway, so
  the missing guard raised a *duplicate* diagnostic at the same span and
  `push_diag` dropped it. Two were fixtures that **ended for the right reason by
  accident** — `a, b` ends because the comma is missing, not because the input
  ends, so "end of input is not a stop condition" was invisible until the fixture
  became `a, b,`. That is the specific way a parser test goes vacuous: **a
  sequence has several reasons to stop and a fixture usually triggers more than
  one.**
- **`types.ply`: two of 21 did not arm**, both coverage gaps rather than weak
  assertions — `generics` is called only from other areas, and no type fixture
  left a delimiter open, so `expect_close`'s secondary label was never compared
  against `expect`'s absence of one. Fixed by adding fixtures, not by weakening
  the mutation. 21 of 21 now arm.
- **The integrated differential: 16 armed, 0 survived, 0 invalid — on the second
  pass.** Both first-pass results were kept rather than fixed away, and both are
  findings:

  **Mutation #3 survived a corpus of 763 inputs.** Deleting `pattern_body`'s
  `inner.span = open.to(close)` changed nothing across 21 real files, 25 fixtures
  and 716 mined inputs — **no input in the corpus contained a parenthesised
  pattern** — while tag coverage read **92 of 92**. *That is the argument against
  tag coverage as a stopping rule*, and `fixtures/11-parenthesised-pattern.ply`
  closes it and says so.

  **The arming script mis-scored three armed mutations as INVALID**, and the tell
  was that they were the three with the *most* output: `printf '%s' "$out" |
  grep -q` under `set -o pipefail` reports printf's SIGPIPE. A green instrument
  over a red result, which is this repository's signature defect wearing the other
  hat.

- **Two of the sixteen mutations are invisible to all 755,821 bytes of real
  source** (#3 above and #14, the `>=` split) and are caught only by fixtures.
  **Error paths are 3.2% of the compared bytes and carry 100% of the 833
  diagnostics** — 21× the lexer spike's 0.15% that ADR 0020 §3.1 records as the
  thing not to repeat, and still the weaker half. *A corpus is not a test suite
  even when it is three quarters of a megabyte.*

---

## What the language handled fine

A gap list with no negative entries is a gap list that was looking for gaps.

- **Recursive ADTs with inline anonymous record payloads carried the whole AST**,
  first try, in two areas independently. This was the spike's go/no-go (§12).
- **Generic higher-order functions.** `comma_list<a>(c, p, close, item: (Ctx, P)
  -> R<a>) -> R<List<a>>`, with a generic record accumulator handed on to
  `iterate`, typechecks and infers with no annotation at the call sites — **14
  call sites with 8 distinct element types** (`TypeExpr`, `Pattern`, `Ident`,
  `Param`, `Binder`, `Expr`, and two inline anonymous records). Registered
  fallback: eight hand-monomorphised copies. **Zero were needed.**
- **`iterate` is the right primitive and it is better than the reference's
  `while`** — depth 1, no materialised `range`, and a budget that turns an
  infinite loop into a report (§12).
- **Structural equality on ADT values**, cross-module, with no `derive` and no
  dispatch: `kind(c, p) == lexer::TPunct(b"comma")` needs nothing declared.
  `spikes/ply-lexer/GAPS.md` §13's "no dispatch mechanism, and it did not bite"
  holds on a parser too.
- **Exhaustive `match` with no `_` arm on every AST sort**, which is what makes
  adding a variant break the dumper rather than silently skip it — the same
  guarantee the Rust harness gets from destructuring with no `..`.
- **The type checker caught the port's mistakes before it ran.** Every error in
  development was a `ply check` error, not a wrong answer at run time — against a
  2,114-line reference ported by hand, by four agents, in parallel.
- **`lexer.ply` was copied in unmodified and needed no edit.** The plan said an
  edit to it would itself be a finding; there is none to report.
- **`Map<Int, Token>` at corpus scale is linear**, and the field-order tax of §4
  does not visibly apply to it (`GAPS-spine.md` §S12/S2).

---

## The strategic answer

Can Ply host its own parser?

**Yes — it does, and it agrees with the reference over 763 inputs, 780,456 bytes
and 126,565 nodes, with zero disagreements and one priced boundary.** ADR 0020
§5.1's objection that a recursive-descent parser could not be written in Ply at
all was refuted by ADR 0022 before this spike started, and this spike is the
demonstration: **the deepest frame in a 5,000-term expression is one.**

**Expressiveness is not the blocker, and it is less of a blocker than the lexer
spike suggested** — every risk that was registered in advance (the AST, generic
higher-order `comma_list`, the call ceiling) failed to materialise. What is left
is a language tax that is *asymmetric between reading and writing*: **on writing
cost the parser is 3.0× what the same step costs Rust (§14), while on running
cost it is a roughly uniform per-phase multiplier (§13).**

**Throughput is the blocker, exactly as ADR 0020 §7 said, and this spike makes
the number worse rather than better.** ADR 0020 measured 12.6× for one phase.
Two phases now cost **30× the whole Rust front end, measured on identical input in
one sitting** — and four phases are unwritten.

**The single most valuable change is still not a language feature.** It is making
§4's field-order rule visible — a lint, an `--explain` line, anything that says
*this `push` will copy*. `spikes/ply-lexer/GAPS.md` said so, ADR 0020 §7 agreed,
and this spike raises the stakes: the rule composes across 178 call sites through
a nine-field state, one instance of the tax is knowingly paid in `push_diag`, and
**§4's two-growing-lists case has no correct spelling at all.** Second is
`list_at` / `list_head` / `list_last`, which would remove §1 — the entry that
chose this program's central data structure. Third is `const`, which would remove
54 functions and one of the four interpreter calls in the parser's innermost
operation.

And one that only a bootstrap sees: **§2 has no fix that is a feature.** The
`bail: Bool` design is *isomorphic* to the reference's `PResult`, it is the right
design, and it still leaves an invariant that Ply cannot state, that nothing
checks, that ten functions violated on the first write, and that 63 of 83 guards
cannot be shown to matter. A self-hosted front end will meet that at every phase,
and the only instrument that found it was deleting each guard by hand and running
everything.
