# What Ply could not do, written down while porting the parser's types and patterns

Area 1 of the parser spike: `types.ply` and `patterns.ply`, a port of the
fifteen functions `crates/ply-syntax/src/parser.rs` uses for type expressions,
generics, effect rows, parameters and patterns. `spikes/ply-lexer/GAPS.md` is
the model; entries are numbered `§P1..§P12` so they do not collide with the
lexer's `§1..§15` or with the spine's.

Each entry says what I was trying to express, what I had to write instead, and
what it cost. Where a claim is a measurement it carries its provenance; where it
is not, it says so. Two entries — §P7 and §P8 — record a *registered prediction
that failed*, because that is worth more than the prediction would have been.

**Provenance for every number here.** Machine: `docs/ONBOARDING.md`
§Provenance, shared with sibling agent worktrees. Instrument:
`target/release/ply`, built from this worktree;
`.github/binary-is-current.sh` printed `current  target/release/ply
(152 inputs checked)` immediately before each series and its output is
recorded below. Load (1-minute average, `uptime`) is given on both sides of
every series. Timing statistics are the **minimum of 3 runs**, user CPU
primary, wall clock secondary — the rule in
`/tmp/ply-parser-spike/PREREGISTRATION.md` §1. No run was discarded; every run
in every series is printed. The statistics were registered in
`/tmp/ply-parser-spike/PREREG-AREA1.md` before any input was generated, except
§P7's, which was registered after its result was seen and **says so, in the
entry and in the pre-registration file**.

---

## §P1 The AST is representable, and the fallback ladder was not needed

The spike's one go/no-go (`PREREGISTRATION.md` §4.1) was that
`type Expr = {kind: EKind, span: Span}` with `EKind` carrying `Expr` is mutual
recursion between a **named** record alias and a **named** ADT, and nothing in
`crates/ply-std/ply/` or `examples/` does it.

It is not needed, and the primary shape works on the first attempt: an ADT per
sort with **anonymous records inline inside the variants**, which
`crates/ply-std/ply/http.ply:243 Line` already ships.

```ply
pub type TypeExpr =
  | TVar(Ident)
  | TCon({ span: Span, name: QName, args: List<TypeExpr> })
  | TFn({ span: Span, params: List<TypeExpr>, ret: TypeExpr, effects: Option<RowExpr> })
  | TRecord({ span: Span, fields: List<{ name: Ident, ty: TypeExpr }> })
  | TUnit({ span: Span })
```

Three things this needed and got, none of which the tree had exercised
together: recursion through `List<Self>`, recursion through a `List` of an
**inline** record holding `Self`, and recursion through `Option<Self>`
(`PList`'s rest binding, which is `Option<Box<Pattern>>` in `ast.rs`).
`RowExpr` and `AtomExpr` stay *named* record types because the cycle runs
`TypeExpr -> RowExpr -> AtomExpr` and stops; only the field types inside a
cycle have to be inline.

**Cost: zero.** This is the entry that says a registered risk did not
materialise, which is only visible because it was registered.

## §P2 No list index, and a parser's one real use of it is `params.into_iter().next()`

`spikes/ply-lexer/GAPS.md` §10 files the `List` surface —
`len/push/map/filter/fold/range/iterate` — as "starts to bite". On a parser it
is load-bearing, and the spine's `Map<Int, Token>` token buffer is the
architectural consequence.

Inside this area it bit once more, at `parser.rs:1076`:

```rust
if params.len() == 1 {
    return Ok(params.into_iter().next().expect("length checked"));
}
```

There is no `nth`, no `head`, no `tail` and no `last`. What there *is* — and
this is worth recording because the lexer spike never needed it — is
**list patterns in `match`**, which `crates/ply-std/ply/db.ply` uses heavily:

```ply
match ps.node {
  [only] -> { p: ar.p, node: only },
  _ -> ...,
}
```

That is the whole of Ply's indexing: a `match` reaches a **fixed prefix** of a
list and nothing else. It cannot express `xs[i]` for a computed `i`, which is
what a parser wants.

**And the head-and-tail form is O(n), not O(1).** Both engines bind the rest of
a list pattern by copying:

```rust
// crates/ply-eval/src/interp.rs:975 and crates/ply-eval/src/machine.rs:2301
let tail = Value::list(xs[items.len()..].to_vec());
```

`Value::List` is `Arc<Vec<Value>>` (`crates/ply-eval/src/value.rs:22,79`), not a
persistent sequence, so `[x, ..t]` allocates and copies the tail — and so does
`[x, ..]`, because `parser.rs:1994` gives a bare `..` a `Wildcard` sub-pattern
and the evaluator materialises the tail before matching it against the
wildcard. So `db.ply`'s `fn tok_word(ts) = match ts { [TWord(w), ..] -> w, _ -> "" }`
copies the whole remaining token list to look at one token. On a SQL statement
that is nothing; **threading `List<Token>` as "the rest" through a 19,576-token
file the way `db.ply` threads it would be quadratic**, which is the concrete
reason the spine's `Map<Int, Token>` is not merely a preference.

This is a *mechanism* claim read out of the two evaluators, not a measurement,
and it is labelled as one. What is measured is §P9.

## §P3 The rule "the growing field goes last" is per-literal, not per-type — and a step that grows two lists at once has no correct spelling

`spikes/ply-lexer/GAPS.md` §1 is the lexer spike's headline: a growing
container must be built in the last sub-expression of its enclosing node or the
program is quadratic, and nothing in the language says so.

An effect row grows **two** lists, `atoms` and `aliases`
(`parser.rs:1133 row`), and only one field of one literal can be last. The
resolution is that the rule is per-*literal*: a round grows one or the other and
never both, so the step is written with two record literals, each putting its
own growing field last.

```ply
match m.node {
  RMAtom(a) ->
    if e.ok { Continue({ p: e.p, aliases: s.aliases, atoms: push(s.atoms, a) }) }
    else { Stop({ p: e.p, node: { aliases: s.aliases, atoms: push(s.atoms, a) } }) },
  RMSet(q) ->
    if e.ok { Continue({ p: with_sets(e.p), atoms: s.atoms, aliases: push(s.aliases, q) }) }
    else { Stop({ p: with_sets(e.p), node: { atoms: s.atoms, aliases: push(s.aliases, q) } }) },
}
```

Four literals for what the reference writes as two `Vec::push` calls. **The
field order differs between the two branches of the same record type**, which
is legal — records are structural — and which is exactly the kind of thing a
reader will "tidy up". `list_items` in `patterns.ply` has the same shape with
`rest` and `items`.

The generalisation, which this area did not have to face and a later one might:
**a step that must grow two lists in the same round cannot obey the rule at
all.** There is no spelling that puts two sub-expressions last.

**Cost: 4 record literals where 2 pushes would do, at 2 sites, plus a comment at
each explaining why the field order is not tidy.**

## §P4 No record update and no mutation, so `inner.span = open.to(close)` is a seven-arm function

`parser.rs:1946` is one line:

```rust
let mut inner = self.pattern()?;
let close = self.expect_close(&TokenKind::RParen, open, "`)`")?;
inner.span = open.to(close);
```

The reference can write it because `Pattern` is `{kind: PatternKind, span: Span}`
— a wrapper with one span. The Ply AST puts the span **inside every variant**
(§P1: a wrapper would be the named-record-in-a-cycle shape), so "set the span"
is a `match` with one arm per variant:

```ply
fn set_pat_span(x: Pattern, s: Span) -> Pattern =
  match x {
    PWild(v) -> PWild({ span: s }),
    PVar(v) -> PVar({ span: s, name: v.name }),
    PLit(v) -> PLit({ span: s, lit: v.lit }),
    PCtor(v) -> PCtor({ span: s, name: v.name, args: v.args }),
    PRecord(v) -> PRecord({ span: s, rest: v.rest, fields: v.fields }),
    PList(v) -> PList({ span: s, rest: v.rest, items: v.items }),
    PBad(v) -> PBad({ span: s }),
  }
```

**Cost: 1 line becomes 11, and it is the only place in the area where a node is
rebuilt field-for-field.** GAPS.md §2 (no record-update syntax) again, but the
sting here is the interaction with §P1: the shape that made the AST
representable is the shape that makes a span rewrite expensive.

It is also *load-bearing*, and the reason `Pattern`'s variants carry a span
while `TypeExpr::Var` does not: `ty_body` hands a parenthesised type back
untouched, so `(a)` is a `TVar` whose span is its name's, while `(x)` as a
pattern is a `Var` whose span is **wider** than its name's. A dump that led with
the `Ident`'s span would agree with the reference on both and carry a different
tree for one. `arm-types.sh`'s "a parenthesised pattern keeps the inner node's
span" is the mutation that watches this, and it fires.

## §P5 No `unreachable!()` that is also an expression

`parser.rs:1897` writes `_ => unreachable!()` in the negative-literal arm,
because the two-token lookahead that reached it already proved the token is a
number. Ply has no expression that stands for "cannot happen" and has the arm's
type — no `panic!` in expression position that the checker accepts as `Pattern`,
and no way to tell the exhaustiveness checker the arm is dead.

`crates/ply-std/ply/db.ply:963` meets the same wall and writes it down:

```ply
// Unreachable: the six above are every constructor. The checker reasons
// about list length before it reasons about the head, so it asks anyway.
_ -> "a token",
```

`db.ply` answers a plausible value. This area answers **a value the reference
can never build** — `LBad`, a `Lit` arm with no analogue in `ast.rs` — so that
if the arm is ever reached, the dump shows a tag the reference cannot emit
rather than a literal that looks right.

There *is* a `panic` builtin (`crates/ply-eval/src/builtins.rs`), and it was not
used, because reaching it would abort the run rather than produce a comparison,
and the point of the placeholder is to be *visible in the comparison*.

**Cost: one extra variant per sort (`TBad`, `PBad`, `LBad`, `MBad`), and the
knowledge that an impossible arm is silently a real one.**

## §P6 `effect` is a Ply keyword, so the Ply AST for Ply's effects cannot name its field what `ast.rs` names it

```
[E0001] Error: expected a field name, found keyword `effect`
 47 │ pub type AtomExpr = { effect: QName, mode: Mode, ... }
```

`ast.rs:773 AtomExpr` has `pub effect: QName`. Rust reserves nothing here; Ply
reserves `effect` because `effect Foo { .. }` is a declaration form. So the
port's field is `eff`, and every reader of both files has to hold the rename.

The general shape: **Ply has no raw-identifier escape** (Rust's `r#effect`), so
a keyword is unusable as a field name, a parameter name or a binder anywhere in
the language. The keyword set is fifteen words — `fn if pub let type test else
with true match false import effect nondet handle` — and four of them
(`type`, `test`, `with`, `effect`) are ordinary nouns that a domain model wants.

**Cost here: one rename, six sites. Cost in general: unbounded and silent —
the error is at the declaration, so it is cheap to hit and cheap to fix, but a
field cannot be named what the thing is called.**

## §P7 The `bail` guards are not load-bearing, and the registered way of arming them does not work

**This is the entry that records a registered prediction failing.**

`PREREGISTRATION.md` §3.4 registers arming instrument 4:

> **Bail-guard deletion.** Remove one `if p.bail` guard and confirm the error
> fixtures go red. This is the failure mode design decision §2.2 buys and it is
> the one with no analogue in the lexer spike.

It does not go red. Two measurements, both over `types.ply` and `patterns.ply`
as shipped.

**One: what a mutation to a single guard changes.** All 41 non-loop
`if p.bail` guards, one mutant each, evaluated on 16 malformed inputs covering
both sorts (`{x Int}`, `{x Int, y: Bool}`, `{: Int}`, `(Int -> Bool`, `List<a`,
`List<a b>`, `{x: Int y: Bool}`, `m::T::U`, `() -> / e`, `Foo<>`, `[a b]`,
`Some(a b)`, `{x: , y}`, `[a, ..b c]`, `{,}`):

| deleting one guard changes | count |
| --- | ---: |
| nothing observable | 35 |
| the tree only | 6 |
| **the diagnostic list** | **0** |

Deleting **all 41 at once** still changes no diagnostic — the lists are
byte-identical — and changes the tree on every one of the 16 inputs.

**Two: what the area's own suite can kill.** All 50 `if p.bail` guards in the
two files, including the ones inside `iterate` steps, one mutant each, oracle =
the 68 in-language tests:

| | count |
| --- | ---: |
| guards written | 50 |
| **killed by the suite** | **6** |
| equivalent / not killed | 44 |

The six killable ones are all the same shape — a guard on the result of a
delimiter close, deciding between a real node and a placeholder:
`types.ply:156, :179, :190, :357, :393` and `patterns.ply:240`.

The mechanism is measurable rather than guessable, and it is the reference's own
dedup rule. `Parser::push` (`parser.rs:219`) drops a diagnostic with the same
code and the same primary span as the one before it; an unguarded parse function
called with `bail` set re-reads the token that caused the error and raises the
same code at the same offset. Turning the dedup rule off and re-running the same
four combinations:

| | dedup on | dedup off |
| --- | ---: | ---: |
| guards on | 16 diagnostics | 18 |
| guards off | 16 | 20 |

**The dedup rule absorbs exactly the diagnostics the guards suppress.**

What the guards *do* buy is the placeholder node — and the reference discards a
bailed item in `recover_to_item` just as this design does, so that difference is
invisible in the reference's own output too.

### How this stands against `GAPS-spine.md` §S3

§S3 finds the same phenomenon in the spine and reaches a different number:
2 of its 8 guards are equivalent mutants, so **6 of 8 are killable**, and it
proposes that M6 be reported as two numbers — guards written, and guards a
mutation can kill. That proposal is right and this is the second number for
Area 1: **6 of 50**, against the spine's 6 of 8.

The two are not in conflict and the gap between them is the finding. §S3's
guards sit on `eat`, `expect`, `expect_close`, `expect_ident`, `expect_gt`,
`deeper`, `comma_list` and `qname` — the functions that *touch a token first*,
where a missing guard immediately reads a token it should not. Area 1's guards
sit on functions whose first act is a call to one of those, so the spine's
guards are already doing the work by the time the area's are reached. **The
ratio is 75% killable at the bottom of the call graph and 12% one layer up**,
and a whole-parser figure that averages them would say nothing about either.

Three consequences, and I am careful about how far each reaches:

1. **Within Area 1, `bail` is over-determined.** Every function guards at its
   top *and* every call site checks the callee's answer, so almost no single
   deletion is detectable. I wrote both because there is no type that says which
   discipline is in force, and writing one requires knowing the other holds
   everywhere.
2. **The plan's cost estimate for the design stands; its benefit estimate does
   not.** `PREREGISTRATION.md` §2.2 prices the `bail: Bool` design at ~93 guards
   against `json.ply`'s per-call-site `match`. Area 1 has **50** guards in 15
   functions. What 44 of them buy, on the evidence here, is nothing observable.
3. **This does not generalise to Areas 3 and 4 and I have not tested that it
   does.** `recover_to_item` depends on where `pos` is left, and an unguarded
   expression parser could consume tokens that change where recovery resumes.
   The claim is about types and patterns.

`arm-types.sh` records one killable guard as `arm` and three unkillable ones as
`equiv` — mutants asserted to stay green — rather than as holes, because calling
an equivalent mutant a hole is how a suite grows tests that assert an
implementation detail.

## §P8 Arming found two tests that watched nothing, and both were coverage gaps rather than weak assertions

`types.ply` and `patterns.ply` passed every one of their first 22 tests on the
first run, which is exactly when a green over unexplored space is invisible.
`arm-types.sh` corrupts one thing at a time and asserts the suite fails. Two of
the first 21 mutations did not arm:

- **"the type parameter list no longer stops at `|`"** — `generics` is called
  only from `items.ply` and `exprs.ply`, so nothing in this area reached it.
- **"an unclosed record type loses the label on its opening brace"** — no type
  fixture left a delimiter open, so `expect_close`'s secondary label was never
  compared against `expect`'s absence of one.

Both were fixed by adding fixtures (`dump_generics_of`, `dump_param_of`, and
`{x: Int`), not by weakening the mutation. **21 of 21 now arm.** This is the
lexer spike's finding repeated: `arm-spine.sh`'s header records six such tests
in the spine.

> **`dump_param_of` moved on 2026-08-30** and so did the `param` it drives, to
> `exprs.ply`, because a parameter's default is an expression (ADR 0029;
> `../GAPS.md` §11R.X). This area's own suite therefore no longer reaches
> `param` at all — the second bullet above is now true of it as well as of
> `generics`, and `test-exprs.sh` is where those four tests run. Nothing about
> the first two bullets' *finding* changes; what changed is which area's
> entry-point list has to carry it.

## §P9 A1-M4 — every accumulator in this area is linear

`PREREGISTRATION.md` §1 M4 registers the prediction **linear**, ratio per
doubling in [1.6, 2.6]; ≥ 3.4 is quadratic.

Instrument current (`152 inputs checked`); load 3.75 before, 3.79 after; min of
3 user-CPU seconds, control (n = 1) subtracted.

| accumulator | n = 400 | 800 | 1600 | ratios |
| --- | ---: | ---: | ---: | --- |
| `comma_list` of `ty_field`, `{f0: Int, ..}` | 0.11 | 0.22 | 0.51 | 2.00, 2.32 |
| `record_fields`, `{f0: a0, ..}` | 0.12 | 0.24 | 0.49 | 2.00, 2.04 |
| `list_items`, `[a0, ..]` | 0.07 | 0.14 | 0.28 | 2.00, 2.00 |
| `comma_list` of `ty`, `T<a0, ..>` | 0.07 | 0.14 | 0.29 | 2.00, 2.07 |

Every run in every series: 0.02/0.02/0.02, 0.13/0.13/0.13, 0.24/0.24/0.25,
0.53/0.54/0.55; 0.02/0.02/0.02, 0.14/0.15/0.14, 0.26/0.26/0.26, 0.53/0.52/0.51;
0.02/0.02/0.02, 0.09/0.09/0.09, 0.16/0.17/0.17, 0.30/0.31/0.31; 0.02/0.02/0.02,
0.10/0.10/0.09, 0.16/0.17/0.17, 0.32/0.31/0.32.

**All eight ratios are in [2.00, 2.32]. The registered prediction holds.** The
consolation the plan offered — a parser accumulates hundreds of short lists
rather than one file-long one — turns out not to be needed: these are single
lists of up to 1,600 elements and they are linear, because the growing field is
last in every literal that pushes to it.

## §P10 A1-M5 — the ceiling does not bite, and there are two orders of magnitude of headroom

ADR 0022 §8 names this spike: *"If a Ply parser is written and the ceiling bites
anyway."* `ty_inner` and `pattern` are two of the reference's three `deeper()`
sites, so this area is where it would.

Registered outcome (`PREREG-AREA1.md` A1-M5): at nesting 17 and 128, a parse
with no diagnostic; at 129, exactly one diagnostic with one note; and at no
depth either `recursion limit of 10000 nested calls exceeded` or an exhausted
`iterate` budget. Inputs `List<List<..<Int>..>>` and `Some(Some(..(x)..))`.

| nesting | 17 | 128 | 129 | 301 | 2001 |
| --- | --- | --- | --- | --- | --- |
| `ty` diagnostics | 0 | 0 | 1 | 1 | 1 |
| `pattern` diagnostics | 0 | 0 | 1 | 1 | 1 |
| `recursion limit` | never | never | never | never | never |
| exhausted `iterate` budget | never | never | never | never | never |

**Registered outcome met exactly.** 128 is `MAX_DEPTH` and is accepted; 129 is
one diagnostic with one note, which is `deeper`'s.

The number ADR 0022 §8 actually wants is the headroom, so I lifted `MAX_DEPTH`
to 1,000,000 and bisected for where Ply's own 10,000-call ceiling bites:

| | last depth that parses | first depth that hits the ceiling |
| --- | ---: | ---: |
| `ty` | 1,661 | 1,681 |
| `pattern` | 1,681 | 2,401 |

So the `ty` chain costs about **six interpreter calls per grammar level**
(10,000 / ~1,670), and:

- against the reference's own `MAX_DEPTH = 128`, the headroom is **13x**;
- against ADR 0020 §5.1's measured corpus maximum of **17**, it is **98x**.

**The ceiling cannot bite on any input the reference itself accepts**, because
`MAX_DEPTH` cuts in thirteen times earlier. That is a stronger statement than
"it did not bite on the corpus", and it is the one ADR 0022 §8 asked for.

## §P11 A1-M6 — a parser in Ply is 1.02x its Rust reference by line, where the lexer was 0.62x

Counts, no stopwatch, so immune to the load gate.

| | Ply | reference | ratio |
| --- | ---: | ---: | ---: |
| parse functions (the 15 ported), non-comment non-blank lines | 406 | 397 | **1.02** |
| + AST declarations for these sorts | 486 | 495 | 0.98 |
| dump functions | 41 | — | no counterpart |

`spikes/ply-lexer/GAPS.md` §15 records the lexer at **668 / 1,069 = 0.62**.

**So relative to its own Rust reference, this area's Ply is 1.6x more verbose
than the lexer's Ply was** — and the port is otherwise function-for-function,
so the difference is the language meeting a parser rather than a lexer. It is
two sorts of four and 15 functions of ~93; it is not the whole multiplier, and
extrapolating it to one is an assumption, in the same words ADR 0020 §6.2 used
of its own.

Where the extra lines go, counted:

| | count | what it replaces |
| --- | ---: | --- |
| `if p.bail` guards (6 killable; §P7) | 50 | `?` |
| `R<a> = {p: P, node: a}` returns | every one of 21 `fn`s | a tuple |
| record types existing only for lack of tuples | 6 (`RowAcc`, `RowOut`, `ListAcc`, `ListOut`, `RecAcc`, `RecOut`) | a tuple |
| placeholder constructors | 6 (`no_ty`, `no_row`, `no_atom`, `no_generics`, `no_param`, `no_pattern`) | `Err(Bail)` carrying nothing |
| extra ADT variants for placeholders | 4 (`TBad`, `PBad`, `LBad`, `MBad`) | — |
| `iterate` sites | 4 | the reference's 5 `while` in these functions |
| hand-monomorphised copies of `comma_list` | **0** | — |

`spikes/ply-lexer/GAPS.md` §9 counted three record types in the lexer that
existed only because a function answers with more than one thing. This area has
**six**, in a third of the code, plus `R<a>` on every function — and `R<a>` is
the one that matters, because a parse function's answer *is* a pair.

The last row settles `PREREGISTRATION.md` §4.2: generic higher-order
`comma_list<a>(c, p, close, item: (Ctx, P) -> R<a>) -> R<List<a>>`, with its
callback handed on to `iterate`, typechecks and is used from this area at four
sites with three distinct element types (`TypeExpr`, `Pattern`,
`{name: Ident, ty: TypeExpr}`). **Registered prediction of zero copies: held.**

## §P12 A1-M7 — what the area's own fixtures reach, and the two tags they do not

Dump-tag coverage over this area's 30 fixtures, every unreached tag named:

- **emittable: 28** — `tvar tcon tfn trec tuni tbad row atm prm gen read write
  badmode pwld pvar plit pctr prec plst pbad int bool str bytes float dec unit
  badlit`
- **reached: 26**
- **not reached: 2** — `badlit` and `badmode`

Both are placeholder tags, and their being unreached is a result rather than a
gap. `badlit` is the `unreachable!()` arm of §P5. `badmode` is `no_atom()`'s
mode, reached only if a bailed `AtomExpr` escapes into a dumped tree — and it
does not, because a bail discards the enclosing row too. `tbad` and `pbad` *are*
reached, because `dump_type_of` dumps the top-level node even on bail; so the
dump does show a placeholder when one is the answer, and shows none when one is
nested. **The two unreached tags are the evidence that no nested placeholder
escapes**, which is what having them cost four extra variants for.

This is 28 tags of the area's own dumpers, not of the whole parser; the
whole-parser figure is the harness's M7.

---

## What the language handled fine

Worth saying, because a gaps list read alone is a hatchet job.

- **Generic higher-order functions.** `comma_list<a>(.., item: (Ctx, P) ->
  R<a>)` with a generic record answer and the callback handed to `iterate`
  typechecks and infers with no annotation at the call sites. This was a
  registered risk and it never bit.
- **`iterate`.** All four sequences in this area are one `iterate` each, with a
  budget of `c.ntok - p.pos + 1` — a backstop that cannot fire. It never fired.
  Against `lexer.ply`'s `fold(range(0, n + 1))`, which cannot stop and wastes
  87% of its rounds on `desk.ply` (GAPS.md §5), this wastes **zero** and
  materialises no `range` list. ADR 0022 added `iterate` for exactly this and it
  does exactly this.
- **Structural equality on ADT values.** `at(c, p, t_colon())` is
  `kind(c, p) == TPunct(b"colon")` and needs no `derive`, no dictionary and no
  dispatch. `spikes/ply-lexer/GAPS.md` §13's "no dispatch mechanism, and it did
  not bite" holds here too.
- **Inline anonymous records in ADT variants** carried the whole AST (§P1).
- **Exhaustive `match` with no `_` arm** on every AST sort, which is what makes
  adding a variant break the dumper rather than silently skip it — the same
  guarantee the harness gets on the Rust side from destructuring with no `..`.
