# ADR 0028 — The `?` operator

Status: accepted — implemented in W4.
Date: 2026-08-30
Closes: `spikes/ply-parser/GAPS.md` §2 — the verifiability half of it; see
"What this does *not* do".
Corrects: `spikes/ply-lexer/GAPS.md` §12 and this record's own ADR 0020 §5.2,
in place — see "Corrections owed elsewhere".
Constrained by: ADR 0001 (a definition's identity is its normalized structure),
ADR 0002 (the driver hashes before it infers, and gate 1 skips a file on its
content hash alone), **ADR 0023 (record update)**, which this follows almost
exactly.

## Context

`spikes/ply-parser/GAPS.md` §2 ranks "no `?`" third among the language gaps a
recursive-descent parser hits, and it is careful to say that the *writing* cost
is not the problem:

> `parser.rs`'s `Bail` is a zero-field struct, so `PResult<T> = Result<T,
> Bail>` is isomorphic to `Option<T>` … A `bail: Bool` in the threaded state is
> therefore the *same type*, not an approximation — and the guard goes on the
> **callee** instead of the call site, which is strictly better than the shape
> `crates/ply-std/ply/json.ply` is forced into.

The problem it measured is **verifiability**. The spike deleted its `bail`
guards one at a time and asked whether anything noticed: of 83 guards, 63 are
unverifiable — 43 were deleted and nothing moved, not the type checker, not 112
in-language tests, not a differential against the shipping parser over error
fixtures; 20 were never deleted at all. Only 20 demonstrably matter. The cause
is that every consuming primitive guards on `bail` itself, so a guardless parse
function reads no tokens, emits no diagnostics, builds a node its caller
discards, and is indistinguishable from the guarded one.

That is 63 lines of code no reviewer can justify, in a language whose thesis is
that what a human reviews is a specification. **`?` does not make those guards
verifiable. It makes them unwritable**, which is strictly stronger: there is no
guard to delete, no invariant "a parse function called with `p.bail` true
consumes nothing" for a reader to maintain, and no phantom diagnostic from an
unguarded predicate answering `false`.

## The constraint that shapes everything below

The same one ADR 0023 states, and for the same reason:

> **`e?` must be the same definition as the `match` it stands for.**

If it were not, converting a site would move its `DefHash` and every dependent's,
split one value into two cache entries, and turn "the module's behaviour is
unchanged" into an assertion rather than a measurement. It is asserted by
`crates/ply-hash/tests/suite/audit.rs try_hashes_as_its_longhand`, and measured over
the corpus under "Consequences — the measurement".

## Decision 1 — Expansion runs inside `ply_syntax::parse_module`

An unexpanded `ExprKind::Try` never escapes the parser. `parse_module` calls
`try_op::expand` **after** `effect_set::expand` and `record_update::expand`, and
`parse_expr` — which has no `fn` around it, and therefore no written return type
— runs `expand_bare`, where every `?` refuses rather than leaking.

**On the pass order.** `?` runs last, after `effect_set` and `record_update`.

> **The reason first given for that was wrong, and is withdrawn (review,
> 2026-08-30).** It read:
>
> > **The pass order is load-bearing.** `record_update` reads written `let x: T`
> > annotations to find a base's field list. A `?` expanded first would already
> > have turned `let x: T = e?;` into an `Ok(x)` arm binder, which
> > `bind_pattern` binds *untyped*, and a `{..x, f: 1}` after it would become a
> > spurious `E0116`.
>
> **That mechanism cannot occur.** `let x: T = e?;` is the one shape
> `split_at` refuses outright — `Cx::annotated_let`, `E0119`, Decision 5 — so it
> is never turned into an arm binder by anything. The only statement whose
> binder moves onto the success arm is an **unannotated** `let`, which
> `bind_pattern` was going to bind untyped in either order. A `?` anywhere
> *inside* an annotated `let`'s value leaves the `let` and its annotation
> standing (`consumes` is false), which is what
> `a_try_that_is_the_whole_value_of_an_annotated_let_is_e0119` documents.
>
> **What is true instead:** each pass is written to walk through the other's
> node — `record_update` has an arm for `ExprKind::Try`, and `try_op::scan` and
> `is_pure` have one for `ExprKind::RecordUpdate` — so neither order is a
> hazard, and the choice is a free one. Measured rather than argued: with the
> two calls swapped, `ply hash` over `crates/ply-std/ply/` and `examples/`
> reports **0 moved entries** against the shipping order (1,211 and 1,429
> entries), both corpora still `check`, and all 251 `ply-syntax`, 6
> `ply-core/try_op`, 56 `ply-hash/audit` and 45 `ply-eval/equivalence_audit`
> tests stay green. **Nothing gates this order**, and a reader should treat it as
> a convention, not an invariant. `router.ply`, `http.ply` and `desk.ply` are the
> corpus modules that write both `{..b, f: e}` and `?`.

**Not as a node in the evaluators**, which is the obvious design and the
expensive one. `ExprKind::RecordUpdate` cost 14 arms across 9 crates; under
expansion those are `unreachable!` or one-line recursion, and this change's are
the same. Under a live node six of them would be *semantics*: an unwind in
`interp.rs`, an unwind in the control-stack machine, a typing rule in `infer.rs`
threading the enclosing function's return type **and its row**, a normalization
tag, a `ply-prove` lowering. That is ADR 0001's "four implementations, four
chances to disagree", and it buys a construct that is not equal to any longhand.

Two consequences worth stating rather than implying:

- **Effect rows are untouched by construction.** The pass introduces a `match`
  and two constructor applications, all pure. The row of `C[e?]` is the row of
  the longhand, character for character. There is no row rule for `?` because
  there is no `?` after the parser, which is why the constraint cannot be got
  wrong.
- **Tail calls are preserved.** `json.ply`'s `array_items` and the whole
  `parse_or`/`parse_and`/`parse_not` chain in `db.ply` recurse per element; the
  continuation lands in the `Ok` arm's tail, exactly where it is today.

`ply-prove`'s arm is a `Blocker::UnexpandedSugar`, sharing
`ExprKind::RecordUpdate`'s: a prover's safe answer is "I did not reason about
this" and never a term it guessed.

## Decision 2 — The canonical expansion, failure arm first

```text
region  C[e?]   =>   match e { Err(er) -> Err(er), Ok(x) -> C[x] }
                     match e { None    -> None,    Some(x) -> C[x] }
```

and for a `?` that is the whole value of an unannotated `let`, the block splits
at that statement and the `let`'s own pattern becomes the success arm's binder:

```text
{ s..; let p = e?; rest }  =>  { s..; match e { Err(er) -> Err(er),
                                                Ok(p)   -> { rest } } }
```

**Arm order is measured, not chosen.** `normalize.rs` writes match arms in
source order, unlike record fields, which it sorts — so this parameter decides
whether converting the corpus moves 129 hashes or zero. Pre-registered rule
**D1** fixed the answer to `max(S1, S2)` before counting. The corpus writes the
failure arm first **129 times to 3** for `Result` (the three are `json.ply`'s
`decode_map` and `decode_and_then` and one site in `desk.ply`) and **11 to 6**
for `Option`, the same direction. Failure first, for both, one rule.

**The `let`-pattern case is not a convenience.** Without it the general rule
would emit `Ok(t) -> { let p = t; rest }`, which is a different definition with a
different hash, and the corpus conversion would move every site.

> **ADR 0023's mixed-length field names have no analogue here, and imitating
> them would have been cargo cult.** They exist because record-update copies are
> *sorted*, and a suite written `a`/`b`/`c` cannot tell sorting by name from any
> comparator that ties on length. Match arms are not sorted. What has to be
> pinned instead is that the order is the corpus's *and that reversing it is
> visible*, so `try_hashes_as_its_longhand` carries a reversed longhand and an
> `assert_ne!` against it. Binder names are erased by de Bruijn levelling, which
> is why a synthesized `?0` can equal a longhand's `x` at all — and the longhands
> in that test use ordinary names, so a levelling that leaked one fails it.

**Synthesized binders are unwritable on purpose.** They are `?0`, `?1`, … — a
`?` cannot occur in an identifier, which is what `ModuleName::qualify` relies on
for the same reason. A binder named `t` would **capture** a `t` the author wrote
in the expression it wraps, and the capture would type-check wherever the two
happened to agree.

## Decision 3 — The mode is the enclosing function's *written* return type

Head `Result` gives `Ok`/`Err`; head `Option` gives `Some`/`None`; expansion
follows this module's own `type` aliases to get there and no further.

The parser has no types — the driver hashes before it infers (ADR 0002,
`driver.rs`: parse → resolve → hash → gate 2 → infer), which is precisely why
ADR 0023 put record-update expansion here and not in normalization — so `?`
cannot be resolved from the scrutinee's type. What *is* available is the same
thing record update reads: text written in this file. `fn parse_expr_text(s:
String) -> Result<Expr, DbError>` says `Result`, on the line above.

**Cross-module aliases are refused**, for ADR 0023 §4's reason: gate 1 skips a
file whose raw bytes are unchanged, so a meaning read across a module boundary
would let an edit in the declaring module leave a stale expansion behind in a
file that never moved.

**Serving `Option` as well as `Result` is corpus-backed, not speculative.**
Pre-registered rule **D2** said serve it only if the corpus wants it: it does —
17 `Option` binds (`db.ply` 9, `router.ply` 4, `config.ply` 1, `orders.ply` 2,
`ledger.ply` 1). Two operators for the two types was rejected: one operator whose
meaning is fixed by the signature the reviewer is already reading beats two a
reviewer has to keep apart.

## Decision 4 — Where `?` may stand, and why the rule is `is_pure`

A **return position** is the function body, the tail of a block in return
position, both branches of an `if` in return position, and every arm body of a
`match` in return position. A `?`'s **region** is the nearest enclosing
statement, or the nearest enclosing return position, whichever is inner.

`?` is admitted when, from the region root down to it:

1. every step is **unconditional** — not an `if` branch, a `match` arm or guard,
   or the right operand of `&&`/`||` — and does not leave a **nested block**,
   which is a separate rule with a separate reason: lifting a `?` out of
   `{ let z = f(n); g(z)? }` would take it out of `z`'s scope. A block in
   *return* position is not nested in this sense and is reached by the
   return-position walk, never by the scan; and
2. everything evaluated **before** it is **pure** — literally
   `ply_syntax::ast::is_pure`.

Both hold vacuously when `?` is the whole value of a `let`, which is every
conversion under "Consequences — the measurement".

**`is_pure` is the predicate `ply-hash` already uses** to license reordering a
run of `let` statements in `commutable_run`, and it was **moved** into
`ply-syntax` rather than duplicated. The lift is a reordering, `commutable_run`
is a reordering, and the licence is the same one: *a failure that happens in one
order happens in every order*, and a call or a `perform` breaks that. Two
implementations could drift, and a drift would mean normalization reordering
something `?` refused to, or the reverse. The move was free — the pinned digest
`crates/ply-hash/tests/suite/map.rs a_map_body_normalizes_to_a_pinned_hash` is
unmoved, and `a_run_of_pure_lets_still_commutes_after_the_predicate_moved`
asserts the behaviour it licenses.

**Refusal rather than hoisting.** `f(g(), h()?)` could be handled by
A-normalizing the prefix into synthetic `let`s. That is complete and
order-preserving, and it is rejected for ADR 0023 §3's reason, verbatim in shape:
restricting the sugar to a case where it *is* the longhand is what makes it one
definition. Hoisting introduces a second expansion shape nobody wrote, which is a
second thing to review.

> **One rule turned out to be partly redundant, and saying so is cheaper than
> discovering it later.** Ply's `if` branches are always blocks, and the scan
> does not enter a nested block for an unrelated reason (lifting out of one
> would take the `?` out of the scope of the block's own binders). So the
> conditional ban does the work that is uniquely its own at a **`match` arm**
> and at the **right operand of `&&`/`||`**, which are not blocks. Deleting the
> ban for `if` alone still changes behaviour — it makes an impure `if` look pure
> to what follows — but it is the `match` arm case that the "runs on the other
> path" fixture under "What was seen to fail" actually exercises.

## Decision 5 — Two codes, and no third

- **`E0118 TRY_SCOPE`** — this file gives `?` no meaning here. A `Why` enum in
  the ADR-0023 style makes the note name the rule the writer hit: the enclosing
  `fn` wrote no `->`; the return type's head is neither `Result` nor `Option`;
  the head is a type parameter, a generic alias or a cross-module name; the `?`
  is inside a **lambda**, a **`handle` clause, body or return clause**, a
  `with_cell`, a `with_region`, a `simulate`, a `test`, a `law`, or a
  `requires`/`ensures`; or this module declares its own `Ok`/`Err`/`Some`/`None`,
  which would capture the expansion.
- **`E0119 TRY_POSITION`** — the `?` is where its early exit would change what
  runs, or would discard something written. `let x: T = e?;` is `E0119`: the
  expansion has no `let` left to carry `T` on. Measured cost of that refusal is
  **zero** — the corpus has three annotated `let`s and all three are `http.ply`'s
  `let base: Limits = default_limits();`. A `?` *inside* an annotated `let`'s
  value is fine, because the `let` survives the split.

**There is deliberately no third code and no typing rule.** By the time inference
runs, `e?` *is* the `match`, so a `Result<_, E1>` bound in a `-> Result<_, E2>`
function is an ordinary `E0201`. **`?` does no error conversion**: there is no
`From` in Ply and this does not invent one. Eight corpus sites map their error
(`Err(e) -> Err(in_index(index, e))`) and `?` leaves every one alone.

> **The design said that `E0201` lands "at the `?`'s span", and as first built it
> did not.** The synthesized `match` took the *operand's* span, so
> `use_it(1, g(n)?)` underlined `g(n)`, and the block shape underlined the whole
> body. The pass now carries the `?`'s own span — `g(n)?`, operand included —
> into the `match` and both its arms, which fixes the first case. The second is
> not a defect and is not fixed: what disagrees there is the *function body*
> against its declared return type, so the span is the body, and a hand-written
> `match` in the same place gets exactly the same diagnostic.
> `crates/ply-core/tests/suite/try_op.rs
> a_try_over_a_different_error_type_is_an_ordinary_type_mismatch` asserts both
> halves — that some `E0201` underlines `g(n)?`, and that the block shape fails
> the same way its longhand fails — rather than the stronger claim the design
> made. Spans are not part of the normalized form, so none of this moves a hash.

**The module-declares-its-own-`Ok` refusal is not hypothetical.** GUIDE §5.7 is
explicit that constructor names are not reserved, so it is writable today. No
corpus module does it; every `?` in one that does is refused rather than
captured.

## Consequences — the measurement

Method and raw output: `/tmp/ply-try-operator/PREREGISTRATION.md` and
`RESULTS.md` (design phase, counts taken before the design was fixed) and
`PREREGISTRATION-IMPL.md` / `RESULTS-IMPL.md` (this phase, written before any
number in it existed). Every hash run used a binary verified fresh by
`.github/binary-is-current.sh`, which covers `.rs`, `.ply` and dep-info — the
stdlib `.ply` are `include_str!`ed, so an `.rs`-only check is blind to a stale
`db.ply`, and ADR 0023's own corrected paragraph is what happens when that is
missed.

**139 sites converted. No definition hash moved.**

| corpus | entries | moved |
| --- | ---: | ---: |
| `crates/ply-std/ply/` | 941 definitions + 270 tests | **0** |
| `examples/` | 1,067 definitions + 362 tests | **0** |

Conversions: `db.ply` 128 (119 `Result`, 9 `Option`), `json.ply` 7, `http.ply` 1,
`config.ply` 1, `router.ply` 1, `desk.ply` 1. Every count matches the design
phase's pre-registered figures. Corpora copied out of the checkout with
`.ply-cache` excluded; taken twice, byte-identical both times.

**Zero moved is a claim about the gate as much as about the change.** With the
two arms emitted in the other order, the same conversion moves **392 of 1,211**
entries in `crates/ply-std/ply/`. 0 against 392 is what makes "the module's
behaviour is unchanged" a measurement: identical hashes are identical
definitions, one cache entry, nothing re-run.

`ply test --engine both` is `0 failed, 176 passed` over `crates/ply-std/ply` and
`0 failed, 186 passed` over `examples`, on both sides of the conversion.

**No `FRONTEND_VERSION` bump.** It stays `0.16.0`, and `BODY_ENCODING` and
`FRONTEND_FORMAT` stand. Three independent reasons, in the order they can be
checked: no `.ply` in the tree contained a `?` outside a string or a comment
before this change, so `TokenKind::Question` changes no existing file's token
stream; `normalize.rs` gained one `unreachable!` and lost `is_pure`, and no byte
of the encoding moved (`a_map_body_normalizes_to_a_pinned_hash` is unmoved); and
**`ply-derive` emits no `?`**. `crates/ply-derive/src/emit.rs` keeps emitting
`json::decode_and_then(...)` and hand-written `Err(de) -> Err(de)` matches, so
`generated_form_audit.rs` and `derivation_determinism_audit.rs` keep their
pinned text verbatim. Putting `?` into generated code *would* be a
`FRONTEND_VERSION` bump — gate 1 keys on raw file content, so a file whose bytes
did not change would reuse a stale generated definition — and it is deliberately
not in this change. `std.json`'s `decode_and_then` and `decode_map` therefore
stay `pub` and unconverted, and GUIDE §14.4 needs no edit.

### One site refused, and it was not predicted

`examples/desk.ply`'s `decoded` writes `match acc { Err(p) -> Err(p), Ok(xs) ->
… }` **inside a `fold` lambda**. `?` refuses a lambda, the compiler said so, and
the site was reverted to its `match`.

> **The design phase's `RESULTS.md` recorded, as an auxiliary check, "Of the 129
> S1 sites, 0 sit under a lambda."** That is **wrong**, and wrong in the
> reassuring direction: it is 1 of 129, and the one is `examples/desk.ply`.
> The `db.ply` half of the claim — 0 of 119 — holds, and `db.ply` is still the
> demonstration. The lambda restriction now has a measured cost of exactly one
> shipped site, which is a better number than "zero" because it is true.

## What this does *not* do

**It does not help `json.ply` much, and the brief expected it to.** The brief
names `json.ply` as "the best available demonstration". It is not. Counted on
the file as it stood: **24** two-arm matches over `Ok`/`Err`, of which **7
convert**; 2 are the `Ok`-first combinators `ply-derive` depends on
(`decode_map`, `decode_and_then`); **4 map their error**
(`Err(e) -> Err(in_field(name, e))` and three like it), which `?` cannot
express; and the remaining **11** are tests and predicates that inspect or
discard a `Result` — `Ok(_) -> false` — rather than bind one, so there is
nothing for `?` to do in them. On top of that, most of the codec half lives
inside `decode:` **lambdas**, where `?` is refused by design.

> **The brief, and this ADR's first draft after it, said "16 two-arm `Result`
> matches", with a breakdown of 7 + 2 + 4 that leaves three unaccounted.** The
> count is 24 and the residue is 11, both re-derived with a brace-balanced
> scanner over the pre-conversion file. The figure that matters — 7 convertible
> — was right; the denominator was not, and a breakdown that does not add up is
> the shape of a number nobody re-derived.

**`db.ply` is the demonstration** — 128 sites, every one in a named `fn` with a
written `->`.

**It does not collapse the seven-function number chain**, and the claim that it
would is corrected under "Corrections owed elsewhere".

**It will never convert an error.** `?` as pure sugar cannot call a `From` that
does not exist, and 8 corpus sites map their error. If the project later wants
`map_err`-at-`?`, this sugar cannot grow into it: that needs a dispatch
mechanism (GUIDE §19.2: there is none, by decision) or a typed node. Anyone
reading `?` as "Rust's `?`" will hit this.

**`?` in a lambda is the restriction most likely to be regretted.** A closure
returning `Result` is idiomatic in every language that has `?`, and here it is
`E0118`. The line is defensible — a lambda has no written return type, so there
is nothing to read the mode off — and it costs one shipped site. The lift path
exists and is stated: give lambdas a written return type, then `?` inside one
becomes legal with **no change to the expansion**. Ply's lambda syntax has no
return-type slot today, so that is a follow-on, not a hole.

**This is the wrong design outright if Ply grows a real `return` or
exceptions.** Then the node exists anyway, `?` should be built on it, and the
parser expansion is throwaway. Nothing in `docs/adr/` proposes one and GUIDE
§19.2 rules both out. Related: the number-chain finding below is evidence that
the *actual* missing feature there is `return`, and `?` is the smaller half of
that problem.

**The residual risk is a float, and it is silent.** Every other part of this
either works or produces a diagnostic. If the position rules are wrong, `?`
moves a `perform` across a branch and the program *runs*, differently, with the
same types and the same row — a row is a set and does not see order. What closes
it in practice is that the pass is small, its refusals are total, and the
conditional rule has been **seen to fail on a running program**, not that
anything checks the transformation against an independent account of what should
have run. That is the same honest statement ADR 0023's Decision 5 makes about
too-narrow record updates.

## What was seen to fail

House rule 5: a gate nobody has watched fail is not a gate. Every corruption
below was applied, the named test watched to go red, and the corruption removed.
Full table in `/tmp/ply-try-operator/RESULTS-IMPL.md`.

| corruption | what went red |
| --- | --- |
| drop the failure arm | 14 `ply-syntax` tests + `try_hashes_as_its_longhand` |
| failure arm loses its constructor (`_` for `Err(er)`) | 12 `ply-syntax` tests + `try_hashes_as_its_longhand` |
| swap the arm order | `try_hashes_as_its_longhand`, **and 392 corpus entries move** |
| region = the whole block rather than the statement | `a_block_splits_at_the_statement_carrying_the_try` |
| the `let`-pattern case dropped | 4 `ply-syntax` tests + `try_hashes_as_its_longhand` |
| the hygiene check deleted | `a_module_that_declares_its_own_ok_refuses_every_try` |
| the lambda barrier removed | `a_try_with_no_readable_return_type_is_e0118` |
| `try_op::expand` made a no-op | 17 tests, including `no_try_survives_parse_module_anywhere_in_the_tree` |
| the conditional-crossing check deleted | `a_try_behind_a_conditional_is_e0119`, **and a running fixture** |
| `Result` and `Option` modes swapped | 4 of `ply-core/tests/suite/try_op.rs`'s 6, including `a_try_adds_nothing_to_the_row` |
| the cross-module check on the return type deleted | `a_return_type_named_through_another_module_refuses` |
| the `match` carries the operand's span rather than the `?`'s | `a_try_over_a_different_error_type_is_an_ordinary_type_mismatch` |
| the failure arm's pattern can never match | both `ply-eval/tests/suite/equivalence_audit.rs` cases |
| `expand_bare` made a no-op | `parse_expr_refuses_a_try_rather_than_leaking_one` |
| `is_pure` says a call is pure | `a_run_of_pure_lets_still_commutes_after_the_predicate_moved` **and** `a_try_with_an_impure_prefix_is_e0119` |
| **`is_pure` replaced by `\|_\| true`** | **nothing, on the first attempt** |

**All 33 tests this change adds** — 22 in `ply-syntax/src/tests.rs`, 3 in
`ply-hash/tests/suite/audit.rs`, 6 in `ply-core/tests/suite/try_op.rs`, 2 in
`ply-eval/tests/suite/equivalence_audit.rs` — have been watched red at least once, and
that is a count taken by re-running every corruption above and unioning the tests
that failed, rather than a recollection. (It read **29 / 18** before review; the
four added under "What the adversarial review found" were each watched red under
the corruption they exist for, and the corruption each answers is named in that
table.) The first pass of that union came back
**28 of 29**: the one it missed was
`a_run_of_pure_lets_still_commutes_after_the_predicate_moved`, whose subject is
`commutable_run` rather than `?` — and the corruption it wanted is the one that
argues for sharing the predicate at all. Make `is_pure` call an `App` pure and
**both callers' gates fire at once**, which is the property the move was for.

**The two `--engine both` cases deserve one honest sentence.** Their failure mode
is an *engine disagreement*, and no mutation of `try_op.rs` can produce one,
because the pass emits a `match` both engines have evaluated since W1 — which is
this design's safety claim, not an accident. What the last corruption shows is
the weaker but real thing: they are sensitive to the expansion's meaning, and
they go red when the failure arm stops short-circuiting.

Two of those deserve their own paragraph.

**`is_pure := \|_\| true` left every test green.** The gate as first written used
a call and a `perform` prefix — and the scan answers "impure" for an `App` and a
`Perform` *structurally*, without consulting the predicate. `is_pure` is only
consulted for an `if` branch, a `match` arm, the right operand of `&&`/`||`, and
a nested block, so the mutant was invisible. Four cases in those positions were
added and the mutant is now red. This is reported rather than quietly repaired
because it is exactly the defect this project ships repeatedly: a green result
over unexplored space.

**The conditional mutant was shown non-equivalent, not merely red.**
`/tmp/ply-try-operator/scratch/crossing.ply` puts a `?` whose operand *performs*
in a `match` arm. Under the shipping compiler it is `E0119`. Under the mutant it
compiles, and the test fails with `expected 0, found 1`: the operand ran on the
path that does not reach it. A pure operand would have made the mutant
equivalent, and an equivalent mutant is not a hole.

**The escape guard is armed.** `no_try_survives_parse_module_anywhere_in_the_tree`
parses every `.ply` in the repository **plus an appended file that actually
writes `?`** — including three `?`s written where they are refused, so the
refusal path's unwrapping is covered too — on both `Module`-returning entry
points. No `.ply` in the tree wrote a `?` before this change, so without the
appended file the guard would pass whether or not expansion ran at all. That is
the record-update guard's own comment, and the same trap.

## What the adversarial review found

Four gates were green over unexplored space, and one behaviour was wrong. All
five are fixed here; each fix was watched red under the corruption it exists for
before being believed.

| what | how it was found | now gated by |
| --- | --- | --- |
| `sequence` walks a call's parts **left to right**, and nothing tested the direction | reversing the iteration reddened **0** of 354 tests, and made `Ok(two(side(n), g(n)?))` — the exact shape GUIDE §6.10 spells out as `E0119` — compile and drop `side`'s effect | `a_call_argument_scan_stops_at_an_impure_argument_and_not_before_one`, which asserts the refusal **and** the lift, because only the pair pins an order |
| the bare `e?;` statement's canonical form (`Ok(_)`, statement gone) | deleting the arm reddened **0** of 354; no `.ply` in the corpus writes one, so the hash gate is silent, and the compiler still ran the program correctly while emitting a **different definition with a different hash** | `a_bare_try_statement_binds_nothing_and_keeps_the_wildcard` |
| three of the six `E0118` barriers — a `handle` body, clause and return clause, and a `with_cell` body | removing any one of the four `under(..)` strings reddened **0**; a `?` there answered `E0119` instead, against GUIDE §6.10's "every one of those is `E0118`" | `a_try_inside_a_handler_or_a_cell_is_e0118` |
| **`import m (Err)` captured the expansion** | `shadowing_ctor` inspected only `Item::Type`, but a selective import binds a constructor unqualified in the same `Namespace::Value`; the author got `E0201`/`E0205` on a `match` they never wrote, pointed at their own `?` | `shadowing_ctor` now reads `module.imports` too, refusing with `Why::ShadowedByImport`; `a_module_that_imports_ok_or_err_unqualified_refuses_every_try` |
| the pass-order rationale | see Decision 1 | nothing, and that is now stated |

The fourth is the only behaviour change: it turns a confusing cascade into
`E0118` with a note saying to qualify the import. It moves no hash — no corpus
module imports `Ok`, `Err`, `Some` or `None` by name — and the corpus was
re-hashed after it to say so rather than assume it.

**What the review tried and could not break.** `ply hash` over a module written
both ways, on five shapes none of this change's tests use — a run of three `?`,
a `?` in a `match` scrutinee in return position, a `?` in an `if` condition in
return position, an `Option` `?` under recursion in tail position, and a `?` in a
return-position `if` branch — gives one digest per pair, and `e??` agrees with
its nested longhand. The corpus conversion was re-measured independently from
the pre-conversion files: **0 moved entries** in both corpora, with the
instrument shown live by swapping one longhand's arms and watching its digest
move. 2,000 sequential and 60 nested `?` in one module compile and run on both
engines. Every runnable example in GUIDE §3.5, §6.10 and §19.4 was executed under
`--engine both`, and every refusal example produces exactly the code the GUIDE
names for it. Record-literal fields *are* evaluated and hashed in source order
(`Interp::eval_record`, `normalize.rs`'s `E_RECORD`), so the left-to-right scan
matches the evaluator there too.

## Corrections owed elsewhere

`spikes/ply-lexer/GAPS.md` §12 and ADR 0020 §5.2 are corrected in place, quoted
verbatim and marked withdrawn. What each said, and what is true instead, is set
out there rather than repeated here.

`spikes/ply-parser/GAPS.md` §2 carries the same claim onward and is **out of
bounds for this change** by instruction, so it is not edited. A reader arriving
at it from there should read the corrected `spikes/ply-lexer/GAPS.md` §12.
