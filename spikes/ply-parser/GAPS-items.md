# What Ply could not do, written down while porting the parser's item grammar

Area 4 of four: `run`, the import table, `item` and its seven constructs, and
**all** of the recovery machinery — `crates/ply-syntax/src/parser.rs:261-1025`.
Merge target is this spike's `GAPS.md`; the numbering is `§P<n>` so four areas
can be concatenated without collisions.

Written in the style of `spikes/ply-lexer/GAPS.md`: what I was trying to
express, what I wrote instead, and **what it cost**. Where a claim is a
measurement it carries its provenance; where it is not, it says so.

**Provenance.** Machine as in `docs/ONBOARDING.md` §Provenance, shared with
sibling agent worktrees, so no wall clock here is a clean one — and nothing
here is a wall clock. Every number below is a **count** or a **binary outcome**,
both of which are immune to the load gate `/tmp/ply-parser-spike/PREREGISTRATION.md`
§1 sets. The binary is `target/debug/ply`, built from this worktree at 21:39;
it is used to *check* and to *run*, never to time, so the staleness rule that
governs measurement does not bind — and it is stated rather than left implicit.

---

## §P1 The `List` surface has no index, and in this area that cost four separate things

`spikes/ply-lexer/GAPS.md` §10 files "no index, no `nth`, no `last`" as
something that "starts to bite". On a parser it is load-bearing, and it is not
one workaround, it is four:

1. **The token buffer.** `tokens[pos]`, `tokens[pos+1]`, `tokens[pos+2]` is what
   a parser *is*, so `lexer.ply`'s `List<Token>` is converted once into a
   `Map<Int, Token>` and every peek is an O(log n) red-black descent with a
   `Value::cmp` at each node where the reference does one bounds-checked load.
   (Spine; registered in PREREGISTRATION §2.4 as the largest single cost.)

2. **`items.first()`.** `parser.rs:271` passes the first item to
   `import_out_of_order` so its secondary label can point at it. There is no
   head. Folding the item list on every out-of-order `import` would be O(n) per
   error, so `run`'s accumulator carries `first: Option<Span>` — a **fourth
   field threaded through the module loop** to stand in for `Vec::first`.

3. **`self.diags.last()`.** `Parser::push`'s dedup rule reads the previous
   diagnostic. No `last`, so the spine carries `last_code` and `last_span` as
   **two more fields of `P`**, which every one of ~90 parse functions threads.

4. **`params.into_iter().next()`** for the single-element parenthesised type
   (`parser.rs:1063`) is a `fold` in Ply.

Nothing here is a workaround I chose over a better one. There is no other route.
A `list_at` builtin removes (1) and (4); `list_last` removes (3); `list_head`
removes (2). **Not measured:** what any of them would buy in time.

---

## §P2 The `bail: Bool` that replaces `?` — measured, and 15 of 17 guards are dead

This is the finding of this area, and it is not the one the design predicted.

The reference's `Bail` is a zero-field struct, so `PResult<T> = Result<T, Bail>`
is `Option<T>` with an empty error channel and `bail: Bool` in the threaded
state is *isomorphic*, not a shortcut. The registered claim
(PREREGISTRATION §2.2, §4.2) was that this trades `?`'s **one per call site**
for **one per callee** — 73 `?` in this area's span of the reference against
~20 guards.

### What it actually cost

| | |
| --- | ---: |
| `?` in `parser.rs:261-1025` | 73 |
| `if p.bail` guards in `items.ply` | 21 |
| `s.p.bail` guards inside `iterate` steps | 10 |
| placeholder constructors (`no_fn`, `no_item`, …) | 15 |

So the ledger is 31 guards and 15 placeholders against 73 `?`. On writing cost
alone the trade looks good.

### What it bought, measured

`arm-items.sh` deletes one guard at a time and asks whether **anything**
notices — 17 in-language tests that assert an exact tree dump, and a
differential against the shipping parser over 32 error fixtures comparing code,
every label's span, every label's primary flag and the note count.

**Of 17 guards deleted one at a time, one changed anything observable.**

The one that did is `fn_body`, and it is worth writing out because it is
precisely the failure mode the design creates:

```
fn f() -> Int where derivable(zz, a) = 1
```

`deriver` bails with `E0207` at 30..32 (`zz`). `where_clause` and
`spec_clauses` stop. `fn_body` then runs with `bail` set; `eat(t_eq())` refuses
because `eat` guards, but `at(c, e.p, t_lbrace())` is an ordinary predicate that
answers `false`, so control reaches `error_here` — which the spine does **not**
guard — and a phantom `E0001` at 32..33 is raised that the reference never
raises. With the guard, the diagnostics match the reference exactly.

The other sixteen guards are unobservable because every *consuming* primitive
(`advance`, `eat`, `expect`, `expect_close`, `expect_ident`, `expect_gt`) guards
on `bail` itself, so a guardless parse function reads no tokens, emits no
diagnostics, builds a node its caller discards, and is indistinguishable from
the guarded one.

### The gap

**The language cannot tell the two apart, and neither can the type checker, the
112 in-language tests, or a differential against the reference over 32 error
fixtures.** `?` in Rust is correct at all 73 sites by construction. Here, 31
guards are written by hand, 1 is load-bearing, 30 are unverifiable no-ops, and
the only way I found out which was to delete them one at a time and run
everything. A reader maintaining this file has no way to know that deleting a
guard is safe, and no way to know that deleting *the* guard is not.

The mitigation is not a language feature but a discipline the language does not
enforce: **put the guard on the primitives** and the per-function guards become
defence in depth. That is the opposite of what `json.ply` does (`decode_map` /
`decode_and_then`, one number literal across seven functions,
`spikes/ply-lexer/GAPS.md` §12) and it is only available because the reference's
error channel is empty.

---

## §P3 A record field cannot be named with a keyword, and the AST being ported uses two of them

`nondet` is one of Ply's fifteen keywords. `EffectDef::nondet` and
`TestDef::nondet` are field names in the very AST this parser is a port of:

```
pub type EffectDef = { vis: Vis, name: Ident, nondet: Bool, ... }
                                              ^^^^^^
[E0001] expected a field name, found keyword `nondet`
[E0001] expected `effect` after `nondet`, found `:`
```

Checked exhaustively: **all fifteen keywords are refused as record field
names** — `fn if pub let type test else with true match false import effect
nondet handle`. The reference AST uses two of them: `nondet` here, and
`AtomExpr::effect` in Area 1.

Cost: `is_nondet` and `eff`. Two renames, and a permanent asymmetry — the Ply
AST is not field-for-field the AST it mirrors, so anything mapping between them
needs a rename table, and a reviewer diffing the two files sees a difference
that is not a difference.

This is cheaper than most entries here and it is the most surprising: Ply
reserved its keywords in the *field* namespace, where nothing could be
ambiguous — `{ nondet: Bool }` has no other reading.

**Closed in the reference**, which now accepts every keyword as a field name and
refuses only the punned forms (`docs/GUIDE.md` §3.3). The renames stay until
this port's own field parser accepts the same, since the corpus is read by both
(`GAPS.md` §6).

---

## §P4 No tuples, restated for a parser: eight record types that carry nothing

`spikes/ply-lexer/GAPS.md` §9 counted three types in the lexer that existed only
because a function answers with more than one thing. This area declares **28**
types, of which twenty are the AST and **eight exist only because Ply has no
tuples**:

`RDeriver`, `RBinders`, `RStr`, `SetAcc`, `RSetMembers`, `Rec`, `ModAcc`,
`RModule` — plus the spine's `R<a>`, `Ate` and `Acc<a>`, which stand in for the
tuple at every one of the ~90 parse functions.

Three of the eight are worse than a pair: `SetAcc`/`RSetMembers` and `ModAcc`
carry **two lists and a state**, and `Rec` carries a state and a counter, which
is §P5.

---

## §P5 A loop that grows two lists can bless only one of them, and nothing says which you lost

`spikes/ply-lexer/GAPS.md` §1's rule is that a growing container must be the
**last sub-expression of its enclosing node** or the program is quadratic. It is
stated for one container. Two of this area's loops grow two:

- `effect_set_def`'s member loop grows `atoms` and `includes`
  (`parser.rs:560-572`).
- `run`'s module loop grows `imports` and `items` (`parser.rs:264-280`).

A record literal has one last field. I ranked by which grows on real input —
`atoms` and `items` take the slot, `includes` and `imports` do not — and wrote
the reason down. On this grammar it does not matter: the largest `effect set` in
the tree has three members and an out-of-order `import` is an error path the
corpus never takes. **On a grammar where both lists were long it would be
unfixable in one loop**, and the only escape is to split the loop in two, which
`spikes/ply-lexer/GAPS.md` §1 column 3 shows doubles the recursion depth.

There is no diagnostic, no lint and no type-level marking. The choice is
invisible in the source and asymptotic in effect.

---

## §P6 `expect_gt` rewrites the token stream in place, and immutability turns that into a claim about lifetime

`parser.rs:203` splits `>=` into `>` then `=` by assigning
`self.tokens[self.pos] = Token { kind: Eq, .. }`. The buffer here is immutable,
so the rewrite is one `Int` in the threaded state naming the index whose token
now reads `=` over its second byte.

One slot is enough — and that needs an **argument the reference does not have to
make**: a split at index *k* is observable at *k* (through `kind`) and at *k+1*
(through `prev_span`); `pos` never moves backwards; and `expect_gt` is reached
only from `generics` and `type_def`, both at the close of a type parameter list,
so two such lists cannot be adjacent and a second split cannot be recorded until
`pos` is past *k+1*.

Cost: a paragraph of reasoning where Rust wrote an assignment, and a field that
every `P` constructor must copy. It is exact, and `test "`type Pair<a>= a`
splits the `>=` into `>` and `=`"` pins it. This is not a Ply defect — it is
what immutability costs when the thing being ported is a mutation — but it is
worth the entry because it is the shape a self-hosted compiler will meet
everywhere its reference mutates.

---

## §P7 A diagnostics-only comparison cannot see a wrong tree — measured, not asserted

`/tmp/ply-parser-spike/PREREGISTRATION.md` §4.4 registers that the diagnostic
signature is weak. It is weaker than that: **it is blind to the tree entirely**,
and this area can put a number on it.

Of 15 mutations in `arm-items.sh`, **three are caught only by the in-language
tree tests** and pass a comparison of code + every label's span + every primary
flag + note count over 32 error fixtures:

| mutation | what it breaks |
| --- | --- |
| `recover_to_item`'s already-at-an-item-start test | which items survive an error, on 4 of 12 fixtures |
| `looks_like_variants` treats every uppercase name as a sum | `type T = A` becomes a sum with one variant |
| `op_param` stops skipping the documentation name | `read g(x: Int)` loses its parameter type |

Ten are caught by both, one (`effect set` with a row variable) by the
differential only, and two — the dead `bail` guards of §P2 — by neither.

The lesson is not that the differential is bad. It is that **an agreement built
on diagnostics is an agreement about the error half of a parser and says nothing
about the tree**, and a spike that reported "32 of 32 agree" without this table
would be reporting a green over unexplored space. The tree half needs the Rust
harness that walks `ply_syntax::ast` with no `_` arm.

---

## §P8 The writing cost, against the lexer's — the ratio is 2.2x worse

`spikes/ply-lexer/GAPS.md` and ADR 0020 §6.2 both lean on the lexer's line
ratio. Here it is for this area, on the same measure and on a stricter one:

| | Ply | Rust | ratio |
| --- | ---: | ---: | ---: |
| lexer, total lines | 668 | 1,069 | **0.62** |
| lexer, code lines (no comment, no blank) | 420 | 950 | **0.44** |
| items, total lines (parser only, tests excluded) | 1,057 | 765 | **1.38** |
| items, code lines | 754 | 675 | **1.12** |
| lexer, `fn` count | 58 | 48 | **1.21** |
| items, `fn` count | 81 | 31 | **2.61** |

Rust span is `parser.rs:261-1025`, which is `run` through `test_def` — this
area's grammar and nothing else. `comma_list` is shared and counted in neither.

**A lexer in Ply is shorter than a lexer in Rust; this area of a parser is
longer.** The `fn` count is the sharper figure: 2.6 Ply functions per Rust
function, against the lexer's 1.2. They come from three places, all named above
— no early return and no `?` (§P2), no tuples (§P4), and every sequence needing
a named accumulator type and often a named step function (§P5).

**This bears directly on ADR 0020 §6.2** and it is the only term of its
multiplier this file can speak to. §6.2 assumes 5–10x from a lexer to a front
end. On *writing cost per line of reference*, this area is **2.2x** the lexer's
ratio (1.38 / 0.62). That is one area of one phase and **extrapolating it to a
front end is an assumption, labelled here in the same words §6.2 used**.

---

## §P9 The AST is representable, and the two halves of it are written in two different styles for a reason the language never states

PREREGISTRATION §4.1 makes this the go/no-go: *"`type Expr = {kind: EKind, span:
Span}` with `EKind` carrying `Expr` is mutual recursion between a named record
alias and a named ADT, and nothing in `crates/ply-std/ply/` or `examples/` does
it."*

**Green, on the primary shape, first try.** Inline anonymous records inside ADT
variants work (`EBinary({span: Span, lhs: Expr, rhs: Expr})`), inline structural
records inside lists work (`List<{name: Ident, ty: TypeExpr}>`), and a named
record type mentioning a recursive ADT works.

But note what that means for *this* area. `Item` is at the top of the sort DAG —
nothing reaches back into it — so `FnDef`, `TypeDef`, `EffectDef` and the rest
are ordinary named record types, exactly as `ast.rs` declares them. `Expr` and
`TypeExpr` cannot be, because they are in a cycle. **The AST is therefore
written in two styles, and which style a sort gets is decided by whether it is
in a cycle — a fact the language will not tell you until you try.** A reader of
`items.ply` and `exprs.ply` side by side sees two conventions and no explanation
in either file unless one is written.

---

## §P10 `error_here`'s message is carried at 105 sites and read at none

The spine carries the reference's `what` string at every expecting call so that
turning messages on later is a change to `error_here` alone rather than a
rewrite of every call site. This area supplies **105** byte-string literals for
it — 47 at expecting calls plus the dump tags — and not one is read.

They are not dead weight by mistake; they are the cheapest available answer to
"what would full message comparison cost?", which
`/tmp/ply-parser-spike/PREREGISTRATION.md` §4.4 requires be priced rather than
silently omitted. **Price, for this area: the 47 message literals are written;
what remains is `TokenKind::describe`'s ~40 arms and the `format!` that embeds
it, neither of which Ply can express without a `match` on a `Tok` returning
`Bytes` — which is one more ~40-arm function.**

---

## What did not bite

A gap list with no negative entries is a gap list that was looking for gaps.

- **Generic higher-order `comma_list<a>(.., item: (Ctx, P) -> R<a>) -> R<List<a>>`
  typechecks and runs.** PREREGISTRATION §4.2's registered fallback was eight
  hand-monomorphised copies. **Zero were needed** — the registered prediction
  held, and this area calls it at five sites with four different element types
  (`Ident`, `Param`, `TypeExpr`, `Binder`).
- **`iterate` took every sequence in the area** — 10 sites, against the
  reference's 12 `while`/`loop` in the same span, the difference being the ones
  that are `comma_list` calls in both. Custom multi-field accumulators
  (`ModAcc`, `SetAcc`, `Rec`) typecheck inside the step closure with no
  friction, and the budget `remaining tokens + 1` is a backstop that never fired
  on anything run here. **`recover_to_item` is the strongest case**: its step
  calls `bump` unconditionally on every round, so it terminates in at most
  `ntok - pos` rounds however the grammar behaved, which is a better termination
  argument than the reference's `loop` has.
- **The ceiling did not bite, including on both of M5's pre-registered
  targets.** Fifteen real corpus files parse with **zero diagnostics**, no
  `recursion limit of 10000 nested calls exceeded`, and no exhausted `iterate`
  budget:

  | file | bytes | dump bytes | diagnostics |
  | --- | ---: | ---: | ---: |
  | `examples/desk.ply` | 159,971 | 509,006 | 0 |
  | `crates/ply-std/ply/db.ply` | 135,285 | 783,170 | 0 |
  | `crates/ply-std/ply/http.ply` | 127,278 | 490,847 | 0 |
  | `examples/orders.ply` | 29,659 | 107,210 | 0 |
  | `examples/agreement.ply` | 34,807 | 134,292 | 0 |
  | `examples/hello.ply` | 24,822 | 65,873 | 0 |
  | nine more (`bank`, `store`, `ledger`, `pipeline`, `timeout`, `echo`, `report`, `signal`, `clock`) | 63,278 | 231,669 | 0 |
  | **total** | **575,100** | **2,322,067** | **0** |

  `db.ply` and `desk.ply` are exactly the two files
  `/tmp/ply-parser-spike/PREREGISTRATION.md` §1 M5 names, and its registered
  outcome — *"neither diagnostic appears"* — **holds**. Zero diagnostics is also
  the right answer: all fifteen are files the reference parses clean.
  ADR 0022 §8's *"if a Ply parser is written and the ceiling bites anyway"* did
  not happen, on the item grammar, on the largest inputs in the tree.
- **Four areas written independently against one written spine integrated on the
  first attempt** — `ply check` over `lexer + spine + types + patterns + exprs +
  items` passed with no edit to any area's file, 378 definitions, 112 tests. The
  AST's sort DAG really is the shape of the split.
- **The type checker caught the port's mistakes before it ran**, as it did for
  the lexer. Every error in this area's development was a `ply check` error, not
  a wrong answer at runtime.

---

## What this area does not do

Stated rather than left to be discovered.

- **`effect_set::expand` is not ported.** `parse_module` runs it before
  returning (`parser.rs:287`), so the reference's `RowExpr.atoms` are always
  expanded and `EffectSetDef.expansion` is always filled; here `expansion` is
  `[]` and rows carry only what was written. `examples/desk.ply` is the one
  corpus file that uses the feature. PREREGISTRATION §4.3's two fallbacks stand
  unexercised.
- **`FnDef::derived` is not carried.** The parser writes `None` into it at
  `parser.rs:723` and nothing can produce anything else, so it is constant over
  every parse. The reference-side dumper must **assert** it is `None` rather
  than skip it silently, or it is a field reached and not emitted.
- **The tree is compared only against this area's own 17 pinned dumps.** The
  differential against `crates/ply-syntax` covers diagnostics; §P7 measures what
  that misses. The tree differential is the Rust harness's job and is not done
  here.
