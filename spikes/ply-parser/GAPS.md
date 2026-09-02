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

### Withdrawn: the index exists, and one builtin closed three of the four rows

**`list_at<a>(xs: List<a>, i: Int) -> Option<a>` landed (ADR 0027), partly on the
strength of this entry, and the port above was rewritten against it. Everything in
§1 down to here describes a language that no longer exists. The withdrawn text is
kept because the *ranking* it argued for is the thing that was right.**

Withdrawn: *"There is no `nth`, no `head`, no `tail`, no `last` and no index
(`crates/ply-eval/src/builtins.rs:203+`)."* There is an index. There is still no
`nth`, `head`, `tail` or `last`, **and that turned out not to matter** — which is
the entry's own prediction failing in the interesting direction.

Withdrawn: *"A `list_at` builtin removes the first and fourth. `list_last` removes
the third. `list_head` removes the second. None of the three is measured."*
**One builtin removed the first, the second and the third. `list_last` and
`list_head` were never needed:** `items.first()` is `list_at(items, 0)` and
`self.diags.last()` is `list_at(xs, len(xs) - 1)`, both O(1) on `Arc<Vec<Value>>`.
ADR 0027 §7 refused to add either, and this port is the evidence that the refusal
was correct. The fourth row was **deliberately not converted** — see below.

**It is four call sites.** Counted on the ported tree: seven occurrences of
`list_at(` across the six modules, of which three are inside quoted withdrawn
comments and **four are code** — `spine.ply:299` (the dedup rule's `last`),
`spine.ply:333` (`tok_index`, the token buffer), `spine.ply:767` (a second `last`)
and `items.ply:942` (`items.first()`).

**What four call sites deleted:**

| the workaround §1 tabulates | what became of it |
| --- | --- |
| `Map<Int, Token>`, a `Buf` type, and `tok_index`'s 8 lines | `Ctx.toks` is a plain `List<Token>`; `Buf`, `DAcc` and the `map_insert` fold are gone |
| a fourth field `first: Option<Span>` threaded through the module loop | `ModAcc` lost the field; the call site is `list_at(s.items, 0)` |
| **two more fields of `P`**, `last_code` and `last_span`, threaded through ~93 functions | both gone; `push_diag` reads the list end directly |
| a `fold` for `params.into_iter().next()` | **not converted, on purpose** |

The fourth row is the one worth defending. `ty_paren` writes
`match ps.node { [only] -> ..., _ -> ... }`, and the arm needs *"exactly one
element"*, not *"the first element"* — the list pattern says that in one line
where `list_at` would need a `len` beside it. **A list index does not subsume a
list pattern, and this row is where the two stop overlapping.**

`P` went from nine fields to six. §4 below is written about a nine-field state and
is corrected there.

### What it bought was code, and the cost claim was already wrong

Withdrawn as a cost claim: *"Every peek is an O(log n) red-black descent with a
`Value::cmp` at each node ... where the reference does one bounds-checked load."*
The descent was real; **the conclusion drawn from it was not.** ADR 0027 measured a
`map_get` peek against a `list_at` peek at **1.02x apart**, and `GAPS-exprs.md` §3
had already refuted its own author's prediction that the `Map` lookup was the
largest cost in the parse. §13's re-take agrees: with all three features landed,
lex+parse over `examples/` fell **5.1%** and parse-alone **8.6%** — real, but
nowhere near what "every peek is a tree descent" implies, and **not attributable to
`list_at` alone**, because `?` and destructuring landed in the same tree.

**The quadratic §1 warns about cannot arise any more.** *"Folding the list to find
its end would be a quadratic hidden inside a deduplication rule — the last place
anyone would look for one."* `push_diag` now writes
`list_at(p.diags, len(p.diags) - 1)`, which is O(1).

**The honest summary of this entry: it ranked the gap first of fifteen and it was
right to, but for the wrong reason.** The index was worth having because of the
*architecture* it deleted — a container type, a fold, an accumulator field and
three fields of the threaded state — and not because of the per-peek time it saved,
which is about 2%.

---

## §2 No `?`, and the flag that replaces it leaves 63 of 83 guards unverifiable

> **WITHDRAWN IN THE TITLE AND IN THE FINDING. `?` landed (ADR 0028), partly on
> the strength of this entry, and the port above was rewritten against it. The
> 83 guards are gone — `grep` finds two mentions of `if p.bail` in the six
> modules and both are inside quoted withdrawn comments. Read the correction at
> the end of this section before trusting anything between here and it: the
> measurement below is sound, and the thing it measured cannot be written any
> more.**


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

### Corrected: the operator exists, and it did not make the guards verifiable — it made them unwritable

**`?` converts, completely.** The shape is `fn thing(c: Ctx, p: P) -> Result<R<Thing>, P>`,
where `Err(p)` is the parser at the point it gave up, diagnostics and all. This
entry's own reasoning is what makes that work: *"`parser.rs:11`'s `Bail` is a
zero-field struct ... and the error channel carries nothing."* `Bail` is empty in
Rust precisely because `self.diags` is still reachable there; carrying `P` in the
`Err` is that same fact written down. **The diagnostics survive at zero cost,
because in a threaded port the state is not "everything else" — it is the natural
error payload.**

Counted on the ported tree: **83 guard sites -> 0**, and **199 `?` operators**
against `parser.rs`'s 178. `bail` is gone from `P`, along with `with_bail`,
`clear_bail` and `bail_with`.

**Withdrawn: *"20 guards are demonstrated to matter. The other 63 are not — 43
were deleted and nothing moved, and 20 were never deleted at all."*** The
sentence was true of the tree that carried it. What replaces it is not a better
number but the disappearance of the question: **there is no bailed `P` to hand a
function, because a failure is now an `Err(P)` that a caller cannot pass down as
an argument.** ADR 0028's claim that *"`?` does not make those guards verifiable,
it makes them unwritable"* holds here in the strongest available form — **the
instrument that measured 63-of-83 has no successor, because it has nothing to
point at.** *(**That last clause is withdrawn.** It has a successor, the successor
runs, and it finds a hole. Read "The residue is half unwatched" at the end of this
section before quoting this sentence.)* What went with it:

- `spine.ply`'s suite *"a parse function called with bail true consumes nothing
  and reports nothing"* (13 assertions), deleted;
- `exprs.ply`'s *"every parse function in this module leaves a bailed state
  alone"* (28 calls) with its `bailed()`/`untouched()` helpers, deleted;
- **22 of the registered mutations became unwritable** — 8 in `arm-spine.sh`, 4 in
  `arm-types.sh`, 7 in `arm-exprs.sh`, 3 in `arm-items.sh` — and are recorded as
  withdrawn in those scripts rather than rewritten.

In-language test count 112 -> 110, verified by `grep -c '^test "'` on both trees.

**The one load-bearing guard is gone by construction, and §2's own closing claim
now covers the port too.** §2 says of the phantom `E0001` on
`fn f() -> Int where derivable(zz, a) = 1`: *"This is a class of bug that cannot
exist in the reference, where `?` returns before the caller is entered."* With
`?`, `where_clause`'s failure leaves `fn_def` at its `?` and **`fn_body` is never
entered**, so the sentence is now true of the Ply port as well. The note sits
above `fn_body` in `items.ply`.

**What it cost, and it is a real cost: 18 functions that exist only to hold a `?`.**
`?` inside an `if` branch that is a `let`'s value is `E0119`, so every optional
grammar piece became its own `fn` — `opt_generics`, `opt_ret`, `opt_row`,
`let_ty`, `lambda_params`, `arm_guard`, `clause_resume`, `else_branch`, `law_host`,
`law_guard`, `variant_fields`, `type_body`, `op_resource_param`, `test_nondet` —
with four more (`call_args_inner`, `primary_paren_inner`, `primary_list_inner`,
`record_expr_inner`) split off for the save/restore rule below. **§5 ranks "54
nullary functions" as the cost of having no `const`; this is 18 more functions for
a different reason and it is the same shape of tax**, and §14 measures it.

**47 `match`-on-`Result` sites remain, under four rules**, and they are not
failures of the conversion:

1. **A lambda has no written return type, so `?` refuses with `E0118`** — verified
   by experiment, not assumed. All 21 `iterate` steps keep a `match`; ~20 sites.
2. **`Iter` is neither `Result` nor `Option`**, so `block_step`, `match_arm_step`
   and `handle_step` can write no `?` at all (7 sites). `?`'s mode is read off the
   *head of the written return type*, so a helper hoisted out of a lambda inherits
   the lambda's problem unless it also changes its return type.
3. **A save/restore around a fallible call cannot be a `?`** — `no_brace` and
   `depth` must be put back on both paths and `?` is an early exit. 13 sites, and
   **the reference agrees**: `parser.rs:1069` writes
   `let r = self.ty_inner(); self.no_brace = saved; r` for exactly this reason,
   and `:1075` does the same for `self.depth -= 1`.
4. **Catching a failure is a `match` by design** — the 3 places `parser.rs:272`,
   `:278` and `:300` match on `Err(Bail)` instead of propagating. These are where
   `clear_bail` used to be called, and there is now nothing to clear.

### The residue is half unwatched, and that is what the successor instrument found

> Withdrawn above: *"the instrument that measured 63-of-83 has no successor,
> because it has nothing to point at."* **It has one.** `?` did not delete the
> error paths, it deleted the *flag*. The four rules above leave 26 sites that
> still propagate a failure by hand, written `Stop(Err(q))` inside an `iterate`
> step. Deleting a guard is no longer writable; **swallowing a propagated error
> is**, and it asks the same question in the same shape: corrupt it, and see
> whether anything notices.

Pre-registered at `/tmp/ply-adversarial-review/PREREG-SWEEP.md`, including the
site list, the three outcomes and the rule that a survivor counts only once an
input has been exhibited on which it differs. An exploratory pass was run first
and is disclosed there rather than discarded; the confirmatory pass reproduced it
site for site.

**Method.** Each of the 26 sites — the whole of `grep -n 'Stop(Err(q))' *.ply`,
none excluded — has its `Stop(Err(q))` replaced by the success value the enclosing
loop builds from its own accumulator, so the mutant stops the iteration with what
it has instead of propagating. The differential is then run against that mutant
and the **set** of disagreeing inputs compared with the baseline's. The set rather
than the count, because the baseline is red at 28 (see §11R) and two different
holes can share a number.

### **13 of the 26 changed nothing. 0 were invalid.**

| module | sites | detected | survived |
| --- | ---: | ---: | ---: |
| `exprs.ply` | 11 | 6 | 5 |
| `items.ply` | 8 | 4 | 4 |
| `patterns.ply` | 4 | 1 | 3 |
| `types.ply` | 2 | 1 | 1 |
| `spine.ply` | 1 | 1 | 0 |
| **total** | **26** | **13** | **13** |

**Eight of the thirteen are demonstrated holes, not equivalent mutants** — for
each, one input exists whose dump differs from the unmutated parser's, and none
of the eight is in the corpus:

| site | what it stops propagating | an input that tells them apart |
| --- | --- | --- |
| `exprs.ply:656` | a match arm's guard | `match x { A if -> 1, B -> 2, }` |
| `exprs.ply:658` | the `->` after a match arm's pattern | `match x { A 1, B -> 2, }` |
| `exprs.ply:660` | a match arm's body | `match x { A -> , B -> 2, }` |
| `exprs.ply:525` | a statement in a block | `{ 1; ]; 2 }` |
| `items.ply:468` | a spec clause's scrutinee | `fn f() -> Int requires ] = 1` |
| `items.ply:636` | a variant's fields | `type T = \| A(]) \| B` |
| `items.ply:780` | an effect-set member | `effect set S = {]}` |
| `patterns.ply:307` | a record pattern's field value | `match r { {x: , y} -> 1, }` |

The remaining five (`exprs.ply:717`, `items.ply:661`, `types.ply:362`,
`patterns.ply:254`, `patterns.ply:263`) are reported as **not distinguished**
rather than as holes: no input was found for them, and no argument is offered that
none exists.

**Why the registered sixteen do not catch this.** Read the mutation table in
`arm-harness.sh`: a dropped field, a wrong span, a swapped associativity, a
swapped precedence, a dropped list element, a widened dedup, a wrong primary flag,
a negated scalar, a collapsed qualifier, a dropped enum arm, a dropped resource, a
lost `>=` split, a wrong leaf span, a dropped block tail. **Every one corrupts the
tree the parser builds when it succeeds. Not one corrupts what it does when a
sub-parse fails.** #6 is the closest and it edits `comma_list`'s `Stop(Ok(..))`,
the success arm of the same `match`. The differential is armed across the
tree-building half of the parser and unarmed across the error-propagation half —
which is the half `?` could not convert, and the half the corpus is 96.8% free of.

**This is §15's parenthesised-pattern finding again, one level up.** There the
corpus reached 92 of 92 tags and still missed a construct; here the corpus reaches
833 diagnostics and still cannot see half the residue. *Tag coverage was not a
stopping rule, and neither is a diagnostic count.* The cheapest close is fixtures —
the eight inputs above are eight lines — and it is left open here deliberately,
because adding them moves the 763/126,565 figures this file quotes throughout and
that is a change to the headline, not a repair to a test.

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

> **Withdrawn in part (2026-08-30): the extra identifier and the extra field
> access were avoidable, and this spike did not know it.** `let` takes any
> pattern, records included, so the shape above is available today as
>
> ```ply
> let {p, node} = f(c, p);
> ```
>
> Verified on the shipping binary, including the renaming and `..` forms:
> `let {a: renamed, ..} = mk();` binds and ignores the rest. The pattern table
> in `docs/GUIDE.md` §6.3 has carried the forms all along; what it did not carry
> was the *idiom* — that this is how a function returns several things — and
> §6.1 now does, with a runnable example.
>
> **`grep -c 'let {'` over this spike's five modules answers 0, against 128
> hand-threaded call sites.** So the 128 are this port's doing rather than the
> language's, and the "one extra identifier and one extra field access per call"
> is a cost it chose without knowing. Four agents wrote those modules
> independently and none of them found the feature.
>
> **What survives, and it is most of the entry.** The count of tuple-substitute
> *types* is real and unchanged, and destructuring does not reduce it: `R<a>`,
> `Ate` and the rest still have to be declared and named, because Ply has no
> anonymous pair *type*. And §P4's sharpest case is untouched — three of the
> eight carry two lists and a state, which is §4's unsolvable shape whatever the
> binding form.
>
> **The ranking is the part to distrust now.** This entry sits third of fifteen
> on a tax that was two thirds avoidable, so a reader deciding what to build
> from this list should read it as "declarations, and a threading cost this port
> paid needlessly" rather than as a language gap of that size. That correction
> is why no tuple feature was built.

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

> **BOTH HALVES OF THAT SENTENCE ARE NOW WITHDRAWN, AND ONLY ONE OF THEM WAS
> CLOSED BY THE PORT.**
>
> *"`{..p, pos: n}` does not exist"* — **it does.** ADR 0023 (accepted, implemented
> 2026-08-27, three days before the port above) added record update, and
> `ExprKind::RecordUpdate` is parsed at `parser.rs:1573`. **The port does not use
> it anywhere**: `grep '{\s*\.\.'` over the six modules finds six hits and every
> one is a string literal or a comment. All six `P` constructors in `spine.ply`
> still spell every field out. **Of the four features that landed after this spike
> was taken, the port took three — `list_at` (§1), `?` (§2) and `let`
> destructuring — and left this one on the table.** It is the cheapest remaining
> item in this file and it is the one §4 is entirely about.
>
> *"`P` has nine — `pos`, `no_brace`, `depth`, `gt_split`, `uses_sets`, `bail`,
> `last_code`, `last_span`, `diags`"* — **`P` has six.** `list_at` deleted
> `last_code` and `last_span` (§1) and `?` deleted `bail` (§2), so the state is
> `pos`, `no_brace`, `depth`, `gt_split`, `uses_sets`, `diags`. The spine spells
> six fields out **eight times** rather than nine fields ten times, and the count
> of functions returning `P` fell from 15 to 11.
>
> **The field-order rule and the "1,602 chances" below are unaffected in kind and
> reduced in degree** — the hazard is one of construction sites times fields, and
> both fell. §13R measured what the narrower record was worth in time only as part
> of the whole port (−5.1% on lex+parse), and it is not separated from `?` or from
> the deleted `Map` build.

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

**Closed, on both sides.** `parser.rs` accepts a keyword wherever a field is
named — a type, a literal, a pattern, after `.`, an update — and refuses only the
punned forms, which bind a variable too (`docs/GUIDE.md` §3.3); this port's
`expect_field_name` does the same at its five sites, and the re-mined corpus
carries the reference's tests for it. The port still writes `eff` and
`is_nondet`: renaming them is a change to its own AST, not to what it parses,
and is left for whoever next touches those records.

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

### §11R The differential is red, it was red before the port, and the record above did not say so

**`./spikes/ply-parser/run.sh` exits 101 at the differential.** 28 of the 763
inputs disagree with `ply_syntax`. This was true before the three features were
converted and it is not the port's doing — but until now it was written only in a
phase report, and **§2, §11 and "The strategic answer" below all still read as
though the comparison were green.** They are corrected in place.

**Why.** `?` and `{..b, f: e}` became *syntax* after this spike was taken:
`TokenKind::Question` (ADR 0028) and `ExprKind::RecordUpdate` (ADR 0023). The Ply
lexer has no `?` token and `exprs.ply`'s `ERecord` has no `base` field, so the
port can neither lex nor parse either — let alone reproduce the expanded `match`
the reference emits, since `try_op::expand` and `record_update::expand` run
*inside* `parse_module`. **The corpus moved under the spike; the parser did not
move under the corpus.**

**Established twice, independently, that the port did not cause it.** The
differential was run with `PLY_PARSER_SRC` pointed at the pre-port tree and at the
ported tree, and the two disagreeing sets are **identical, all 28** — `diff` of the
sorted lists is empty. The correlation is exact in the other direction too: every
disagreeing corpus file uses `?` or `{..`, and the three stdlib files that use
neither (`net`, `signal`, `trace`) agree.

| | inputs | bytes |
| --- | ---: | ---: |
| agree, tree and diagnostics, byte for byte | **735** | 232,634 (**29.8%**) |
| disagree | **28** | 548,739 (**70.2%**) |

**The byte column is the honest headline and it is much weaker than the input
column.** The 28 are 22 mined fixtures — tiny — plus `examples/desk.ply` and five
of the eight stdlib files, which is where the corpus keeps its bulk: `desk`
159,939, `db` 130,191, `http` 126,475, `json` 67,326, `router` 55,585. **Reporting
"735 of 763 agree" without the bytes reads as a 3.7% loss; it is a 70.2% loss.**
The 26 hand-written error fixtures all still agree, and the recovery half was
re-checked directly: over 25 fresh error inputs written for this review, 23
reproduce the reference exactly — code, primary span, label count, note count and
each label's own span — and the 2 that do not are the `effect set` boundary §11
already prices. **`?` cost no diagnostic fidelity.** What it cost is that the
comparison can no longer see three quarters of the corpus by weight.

**`arm-harness.sh` cannot run at all**, and that is the instrument behaving
correctly: lines 83–89 require a green baseline and `exit 1` otherwise. So the
"16 armed, 0 survived" of §15 and README §2 is a figure about the tree *as it was
taken*, and no registered mutation has been scored against the tree as it stands.
The successor sweep in §2 works around this by comparing disagreement **sets**
rather than requiring green, and the sixteen were re-scored the same way:
**all 16 still apply to the ported source and all 16 still go red**, changing
between 29 and 152 inputs. So the port disarmed none of the registered mutations —
the hole §2 reports is not one the port opened, it is one the sixteen never
covered.

**What restoring it would take**, in order: a `?` byte in `lexer.ply` (trivial);
postfix `?` and an `ERecord` base in `exprs.ply` (modest); and then
`try_op::expand` (1,008 lines of Rust) and `record_update::expand` (529) ported
into Ply. **1,537 lines — larger than the `effect_set::expand` (535) this spike
declined and priced in §11.** That is a second spike, not a repair. The
alternative — an asserted input-level tolerance in the `desk.ply` shape — would
turn the suite green while making "0 disagreements" mean something narrower than
it means above, and is refused for that reason.

### §11R.D The decision: **compare pre-expansion**, and what that does and does not cover

**Taken 2026-08-30, from the tree in `~/.worktrees/ply/spike-parser-ci`, with every
figure below re-measured here rather than quoted from above.** The paragraph
immediately preceding this one framed the choice as two options — port the passes,
or assert a tolerance — and refused the second. **There is a third it did not
consider: read the reference's tree *before* the passes run.** It is the option
`reference_dump_unexpanded` already takes for `effect_set`, generalised from a
projection to an entry point, and it is the one taken here.

#### Four corrections to the framing, each measured

**1. There are three passes in the parser, not four. `defaults::expand` is not a
parser pass and never can be.** It is called from `resolve.rs:453`, inside
`resolve`, and `defaults.rs`'s own header gives the reason:

> *"A default lives in the **callee's** module, so matching a call against a
> signature needs the whole program — which the parser, running one file at a
> time, does not have. `resolve` is the first point that does"*

So with respect to named arguments and default parameters the harness is
**already** comparing a pre-expansion tree and has no choice about it: it enters
at `parse_recovering`, and `defaults::expand` runs two phases later. A
post-expansion comparison of *those* would require porting `resolve` — a
whole-program name resolver — which is not this spike and not the next one
either. The question is only ever about the three passes `Parser::run` gates:
`effect_set` (538), `record_update` (530), `try_op` (1,019).

**2. §H4's projection method is not available for the other two, and that is a
property of the passes.** `reference_dump_unexpanded` can un-expand effect sets
because `effect_set::expand` **records its own output in the tree** —
`write_back` fills `EffectSetDef::expansion`, so how many atoms were spliced is
*read off*. Neither of the other two records anything, and both headers state the
opposite as the design goal: record update *"rewrites every one into the plain
`ExprKind::Record` a reader would have written by hand"*, and `?` into *"the
`match` the corpus already hand-writes **129 times**"* — precisely so that sugar
and longhand carry one hash. A projection would therefore have to **guess** which
`Record` was an update and which `match` was a `?`, and it would be wrong on all
129. §H4 cannot be repeated here; it was not an oversight that it was not.

**3. 22 of the 28 disagreements are a lexer byte, not an expansion.** Re-run
first-hand: 22 mined + `desk` + `config`, `db`, `http`, `json`, `router` = 28,
exactly the set above. But every one of the 22 mined fixtures is a **Rust format
string** mined out of `tests.rs` — `{src:?}`, `{ds:#?}`, `{v:?}f` — and each
disagreement is a *diagnostic count*: the reference lexes `TokenKind::Question`
and says nothing, `lexer.ply`'s `punct` has no arm for byte 63 at all and raises
an extra `E0001` there. Fixture `#309` is the shape:

```
  rust: #1   !E0001:0:8:1:0
  ply : #2   !E0001:31:32:1:0   !E0001:0:8:1:0
```

Offset 31 is the `?`. **Not one of the 22 contains the try operator**; the mined
half of the corpus holds **zero** real uses of `?` and **zero** of `{..`. Those
22 are closed by the one line §11R already calls trivial.

**4. The 70.2% is carried by six files and 143 sugar tokens.** Counted with the
reference's own lexer over all 47 corpus files:

| file | `?` tokens | `{ ..` | bytes |
| --- | ---: | ---: | ---: |
| `crates/ply-std/ply/db.ply` | 128 | 0 | 130,191 |
| `crates/ply-std/ply/json.ply` | 7 | 0 | 67,326 |
| `crates/ply-std/ply/http.ply` | **1** | **4** | 126,475 |
| `crates/ply-std/ply/config.ply` | **1** | 0 | 8,683 |
| `crates/ply-std/ply/router.ply` | **1** | 0 | 55,585 |
| `examples/desk.ply` | **1** | 0 | 159,939 |
| | **139** | **4** | **548,199** |

**`examples/desk.ply` is 159,939 bytes and leaves the comparison for a single `?`
on line 654.** `config`, `router` and `http` are one each. The byte-weighted loss
is not proportional to feature use and never was — 143 tokens cost 548,199 bytes,
and 548,199 of the 548,739 lost bytes (99.90%) are these six files.
`examples/twin_divergence_audit.ply` and `crates/ply-std/ply/router.ply` both
contain `?` **in text** and agree; the reference lexer finds 0 and 1 `Question`
tokens in them respectively, because the rest sit inside strings and comments.
Grepping the corpus for `?` would have mis-attributed both.

#### The decision, and the argument

**Compare the tree `Parser::run` builds, before `effect_set`, `record_update` and
`try_op` rewrite it.** Four reasons, in the order they carry weight:

1. **It gives up no coverage the differential has.** Measured, not asserted: the
   differential verifies `try_op::expand` on **0** inputs and
   `record_update::expand` on **0** inputs today — every input carrying either
   sugar is in the disagreeing set — and it verifies `effect_set::expand` on
   **0** by construction, because §H4 projects it out of the one file that uses
   it. **Pre-expansion moves the covered count from 0 to 0.** What it moves is
   the corpus, from 29.8% of bytes to substantially all of it.
2. **Post-expansion is a second parser.** `record_update.rs` (530) +
   `try_op.rs` (1,019) = 1,549 lines of Rust, at §14's measured Ply÷Rust parser
   ratio of **1.73 total / 1.27 code**, is **~2,680 Ply lines** against a whole
   Ply parser of 3,650. And 1.73 is a **floor** here: it was measured on
   recursive descent over a token stream, where these are tree **rewrites** over
   an immutable AST — §7's worst case, where every unchanged node is rebuilt by
   hand. Add `effect_set` to close §H4 as well and it is ~3,611. §11R's own
   sentence stands and this only sharpens it: *that is a second spike, not a
   repair.*
3. **It makes the differential measure one thing.** As it stands a disagreement
   cannot say whether the parser or a rewrite is wrong, and §H3's sixteen
   mutations are all parser mutations — a seventeenth aimed at an expander has
   nothing on the port's side to hit. Pre-expansion also *creates* the artifact a
   later expansion spike would need: a pre-expansion reference tree, which does
   not exist today at any public entry point.
4. **The port is already producing the pre-expansion tree, and the diff shows
   it.** On `router.ply` the two trees are identical for 3,138 records and part
   at exactly the rewrite:

   ```
   rust: 10326:10391:eblk  #0   ?1  10338:10362:emat  10338:10361:eapp …
   ply : 10326:10391:eblk  #1  10330:10363:slet  10334:10335:pvar …  10338:10361:eapp …
   ```

   The reference turned `let d = decimal_of_string(text)?;` into a `match` with
   zero statements and the match as the block's tail; `exprs.ply` kept the `let`
   it read. Same spans, same `eapp`, same everything else. The port is not wrong
   about this file — it is being compared against a later phase.

#### What it costs, in the shape §H4 states its cost

Three things leave the comparison, and they are written into `GAPS-harness.md`
§H2 as item 5 so they are enforced-by-listing rather than remembered:

| | lines of Rust | corpus diagnostics it raises | what leaves |
| --- | ---: | ---: | --- |
| `effect_set::expand` | 538 | **9** (E0114 ×6, E0115 ×2, E0105 ×1) | the tree effect §H4 already projects out, and 7 of the 9 that §H4's tolerance already excuses |
| `record_update::expand` | 530 | **0** | canonical field order, `E0116`/`E0117` |
| `try_op::expand` | 1,019 | **0** | the `match` shape, the `let` split, `E0118`/`E0119` |
| *(`defaults::expand`, for completeness)* | 912 | — | never in the comparison; runs in `resolve` |

**The diagnostic half costs 9 of 833 (1.1%), and 7 of those 9 are excused
today.** `record_update` and `try_op` raise **zero** diagnostics anywhere in the
763 inputs — measured by counting `!E0116`/`!E0117`/`!E0118`/`!E0119` in the
reference dump of every input — so the differential has never seen one of their
error paths and pre-expansion gives up none. The 2,087 lines of tree rewriting
do leave permanently, and nothing in this spike will test them; that is the
honest price and it is item 5's job to keep saying so.

> **Corrected on implementing it, 2026-08-30: the diagnostic half costs 7, not
> 9, and the table's `effect_set` row is wrong by the same two.** The nine were
> counted by grepping `E0114`/`E0115`/`E0105` out of the reference's dump, which
> attributes a diagnostic by its **code**. Differencing the two entry points
> directly — `parse_recovering`'s diagnostics minus `parse_unexpanded`'s, which
> is the only way to attribute one to a *pass* — gives **E0114 ×4, E0115 ×2,
> E0105 ×1 = 7**, on 7 mined fixtures and no other input. The two missing
> `E0114`s are `items.ply`'s own refusal of `pub effect set`, which shares the
> code and is raised by the **grammar**, on both sides, and was never
> `effect_set::expand`'s.
>
> This makes the argument stronger and it is worth saying why rather than just
> correcting the number. Those 7 are **exactly** the 7 that §H4's tolerance
> excused. So the diagnostic cost of pre-expansion is not "7 of 9 were already
> excused" — it is **all of them were**, and the set of diagnostics this
> differential ever actually compared did not change by one.
>
> `harness/tests/agreement.rs`'s
> `the_rewrites_this_comparison_gives_up_raise_exactly_these_diagnostics` pins
> the 7 and prints the seven fixtures. **And it takes the half this section
> could only state in lines of Rust**: the same three passes add **3,974 nodes**
> over the corpus — `db.ply` 2,137, `desk.ply` 1,028, `http.ply` 279, `json.ply`
> 119, `router.ply` 11, `config.ply` 11 — which is the tree the comparison does
> not look at, as a number.
>
> **Re-taken 2026-09-01, on re-mining the corpus.** The reference's test file
> had grown the bit operators, tuples, lambda return types, keyword fields and
> the `?` refusals since the corpus was mined, so the pin moved with it: the
> give-up set now also carries `E0116`/`E0117` (record update) and
> `E0118`/`E0119` (`?`), every one on a mined input written to raise it, and
> the pinned map in `agreement.rs` is the current list. The tree half found
> one thing this section had stated too strongly — *no rewrite removes a
> node* — twice over: a refused `?` is unwrapped, one node fewer, and an
> update that writes every field drops its base, since nothing is copied from
> it. So the count is signed, and it is a count rather than an invariant.

**One cost is not free and must be stated: the reference crate has to grow an
entry point.** `lib.rs` declares `mod effect_set; mod record_update; mod try_op;`
privately and all three `expand` functions are `pub(crate)`, so **no external
crate can decline them.** A pre-expansion comparison therefore needs `ply-syntax`
to expose the unexpanded tree — one function, `#[doc(hidden)]`, with a test that
no shipping caller reaches it. That is a change to a shipping crate made for a
spike, it is the single real cost of this choice, and it is smaller than 2,680
lines of Ply by three orders of magnitude.

#### §11R's own list, re-priced under the decision

> Withdrawn as the only reading, not as fact: *"**What restoring it would take**,
> in order: a `?` byte in `lexer.ply` (trivial); postfix `?` and an `ERecord`
> base in `exprs.ply` (modest); and then `try_op::expand` (1,008 lines of Rust)
> and `record_update::expand` (529) ported into Ply. **1,537 lines — larger than
> the `effect_set::expand` (535) this spike declined and priced in §11.**"*

Every number in it is right and the line counts are off by one or two against
today's files (1,019 / 530 / 538 as of this measurement). What it does not say is
that **the list has a semicolon in it that is a decision point.** The first two
items restore 99.90% of the lost bytes on their own; the third is needed only if
the comparison is taken *after* the rewrites. Under the decision above the work
is the first two, plus named arguments and default parameters, which §11R does
not mention at all because §11R did not know `App` had grown a field:

| what `spikes/ply-parser/*.ply` must learn | where | est. Ply lines |
| --- | --- | ---: |
| the `?` byte → `TPunct(b"question")` | `lexer.ply` `punct`, byte 63 | 1 (+1 accessor) |
| `ETry({span, operand})` + postfix `?` | `exprs.ply` `postfix_expr`, and arms in `expr_span`, `with_span`, `dump_expr`, `node_count` | ~14 |
| `ERecordUpdate({span, base, fields})` + `{..b, f: e}` | `exprs.ply` `at_record_literal`, `record_expr_inner`, same four total matches | ~26 |
| `NamedArg` + `EApp.named` + `positional_after_named` + the `perform` refusal | `exprs.ply` `call_args_inner`; the 2-token lookahead already exists as `kind_at(c, p, n)` | ~45 |
| `Param.default` | `types.ply:72/96/373` and `dump_param` | ~20, **and a layering decision** |
| **total** | | **~110** |

**The `Param.default` row is the only one that is not mechanical**, and it is
worth naming because it is a cost of the split this spike chose rather than of
the feature. `types.ply:371` says why `param` lives there:

> *"Lives in this area rather than with items because both `fn_def` and
> `lambda_expr` need it and **it needs only `ty`**."*

A default is an **expression**, so that stops being true: `param` would need
`expr`, which is above it. Either `param` moves into `exprs.ply` or it takes the
expression parser as an argument. The reference has no such problem — its `param`
is in `parser.rs` with everything else — so this is a line item the Ply÷Rust
ratios of §14 do not capture, and §9's "small missing pieces" is where it belongs
if it is paid.

**~110 Ply lines against ~2,680.** That is the decision.

### §11R.N A fourth feature landed and the differential cannot see it — `App.named`, and a field test that is green because of an English word

`ExprKind::App` gained a third field, `named: Vec<NamedArg>` (ADR 0029), so
**every call in the corpus changed shape**, not only the ones that use a named
argument. The reference dumper takes it like this — `harness/src/lib.rs:426`:

```rust
// `named` is empty: `defaults::expand` places every named argument
// in `resolve`, which runs before anything here sees a tree.
ExprKind::App { func, args, .. } => {
    self.rec(e.span, "eapp");
    self.expr(func);
    self.list(args, Self::expr);
}
```

**That comment is false, and the `..` is the hole it excuses.** The harness's
entry point is `parse_recovering`, which is `Parser::new(source, text).run(name)`
and nothing else; `defaults::expand` is called from `resolve.rs:453`, in a phase
the harness never runs. `resolve` does not run "before anything here sees a
tree" — it runs after, and only for callers that ask for it. So `named` is
**not** empty here, and the `..` drops it on the floor.

**Seen, not reasoned about.** Three probes, each a pair of inputs of identical
length so that no span moves, through `refdump`:

| | | |
| --- | --- | --- |
| `g(1, b: 2)` vs `g(1, b: 3)` | dumps **byte-identical** | the argument's **value** is invisible |
| `g(1, b: 2)` vs `g(1, c: 2)` | dumps **byte-identical** | the argument's **name** is invisible |
| `g(1, b: h(2))` vs `g(1, b: k(9))` | dumps **byte-identical** | an entire **call subtree** inside one is invisible |

The dump of `g(1, b: 2)` is `…:eapp; …:evar; …; #1; 55:56:elit;%int;@31;` — one
positional argument, and of `b: 2` not a node, not an ident, not a span. **A Ply
parser that lexed `b`, `:`, `2` and threw all three away would produce a
byte-identical dump and the differential would pass.** That is the cheapest way
to make a named argument agree, and nothing in this spike would notice it had
been taken.

**Why `harness/tests/fields.rs` did not catch it, which is the part worth
reading twice.** That test exists for exactly this class — its own header says
*"a tree comparison that passes under a dropped field is worth nothing and this
project has shipped that exact defect before"* — and it passes over `App::named`
today. It checks that a field is **named** somewhere in `src/lib.rs`, by
whole-word split. The word `named` occurs twice in that file and **neither
occurrence is a dump**: once in the comment above, and once at line 79, in the
doc comment for the *effect-set* projection, as ordinary English —

> *"A set that was refused, one on a cycle, one **named** from another module and
> one that does not exist all contribute an empty expansion"*

**Armed, and seen to go red.** Corrupting only prose, in three steps, with the
test re-run after each: rewriting line 426's `` `named` is empty `` to
`this field is empty` — still green over `App::named`; rewriting the rest of
that line so `named argument` becomes `keyword arg` — still green; and only on
rewriting line 79's `one named from` to `one written in`, leaving zero
whole-word occurrences in the file, does it report

```
left:  {("ExprKind", "named"), ("Lit", "mantissa"), ("Lit", "scale"), ("Param", "default")}
right: {("Lit", "mantissa"), ("Lit", "scale")}
```

`src/lib.rs` was restored byte-identical afterwards and the verdict re-checked.
**The field test's green over `App::named` is bought by a sentence about effect
sets.** §H2 already carries the limit that produced this — *"It checks that a
field is **named**, not that it is **emitted**"* — recorded when renaming a
binding was found to defeat it. This is the same limit reached by a second road,
and the second road is worse: the first needed someone to edit the dumper, this
one needs nobody to do anything.

**`Param::default` is the same feature's other half and it is *not* excused —
the test fails on it right now**, and that changes where `run.sh` stops (§11R.S).

#### The finding that arrived by accident: writing this section flipped the test

The `dumper_boundaries` doc added to `harness/src/lib.rs` for §11R.D names the
fields it is about, the way documentation does. Naming them **changed
`fields.rs`'s verdict**, because that test reads the whole file as a bag of
whole words and does not care whether a word is code, a comment, or a sentence:

| `harness/src/lib.rs` | `fields.rs` says |
| --- | --- |
| as found | `left: {("Lit","mantissa"), ("Lit","scale"), ("Param","default")}` — **fails**, correctly, on the one field that is not dumped |
| with the new prose, first draft | `left: {}` — **every field of `ast.rs` reported as covered**, including the two that are deliberately absent |

The three words that did it were `default`, `mantissa` and `scale`, written in
sentences *stating that those fields are not dumped*. One of them was not even a
field reference: the phrase *"a default lives in the callee's module"*, quoting
`defaults.rs`'s own header, was enough to mark `Param::default` covered.

**So the test can be turned green by describing the hole it exists to report.**
It is now green-adjacent in both directions — silent on `App::named` because of
an unrelated sentence, and silenceable on any field by writing a true sentence
about it. The prose in `dumper_boundaries` spells all three names around
("`Param`'s fallback-expression field", "`Lit::Decimal`'s two numeric fields")
and carries a warning not to add them back, and the verdict above was re-checked
byte-for-byte against the one taken before any edit. **That is a workaround and
it is recorded as one.** The repair is to make `fields.rs` scan what the dumper
*emits* rather than what it *mentions* — at minimum, strip `//` and `///` lines
before matching, which alone would have caught `App::named` — and it is the
first item of work this phase hands on.

**What it costs today: nothing measurable, and that is the trap.** The 763-input
corpus contains **zero** named arguments and **zero** default parameters — every
call parsed to `named.len() == 0` and every `Param.default` to `None`. So the
blindness is currently free, and it will not stay free: re-mining
`crates/ply-syntax/src/tests.rs` with this spike's own `mine-fixtures.py` yields
**889 fixtures against the checked-in 716** (33,826 bytes against 22,737), and
**2 of the 173 new ones carry 6 `NamedArg` nodes and 2 carry 3 `Param.default`s**
— including `tests.rs`'s own `f(x, m: 1)`. The corpus is frozen at the moment
the spike was taken, nothing asserts it is current, and the day it is refreshed
those six subtrees enter a comparison that cannot see them.

### §11R.S Corrected in place: `run.sh` no longer reaches the differential, and the gate that stops it first is one nobody has read

§11R opens:

> Withdrawn: *"**`./spikes/ply-parser/run.sh` exits 101 at the differential.**"*

and `README.md` §Status says the same thing more strongly:

> Withdrawn: *"`run.sh` exits **101**, at the differential **and only there**.
> Everything before it passes: instrument current, **110** in-language tests
> (112 → 110; two suites became unwritable when `bail` did, `GAPS.md` §2),
> harness `--lib` 10 + **`--test fields` 1**, `cargo fmt --check` and `clippy -D
> warnings` clean."*

**It exits 101 one step earlier, at `cargo test --test fields`.** Every step of
`run.sh` was run individually, in its own order, on 2026-08-30:

| `run.sh` step | today |
| --- | --- |
| `cargo build --release -p ply-cli` | ok, 32.68s |
| `.github/binary-is-current.sh` | `current  target/release/ply  (157 inputs checked)` |
| `test-items.sh` | **0 failed, 110 passed** (119 since the four features were ported) |
| `cargo fmt --all --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test --lib` | **10 passed** |
| `cargo test --test fields` | **FAILS** — `("Param", "default")` |
| `cargo test --test agreement` | **never reached by `run.sh`** |

So the two figures the README pairs are no longer both true: `--lib 10` still is,
`--test fields 1` is not. The failure is the honest one — `Param` gained a
`default` field (ADR 0029) that the reference dumper names nowhere, and the test
built to say so says so:

```
a field of a parsed AST type is not named anywhere in the reference dumper.
left:  {("Lit", "mantissa"), ("Lit", "scale"), ("Param", "default")}
right: {("Lit", "mantissa"), ("Lit", "scale")}
```

**Two consequences, and the second is the one that matters.** First, §11R's
"28 of 763 disagree" is still exactly right — run directly, bypassing `run.sh`,
the differential reports **22 of 716 mined + 1 of 13 examples (`desk`) + 5 of 8
stdlib**, with §H4's tolerance excusing 7 mined inputs (E0114 ×4, E0115 ×2,
E0105 ×1) — but **`run.sh` no longer prints it**, so the one command README §6
names as the thing that would notice the bit-rot now stops before reaching its
own headline. Second, **`fields.rs` failed on `Param::default` while passing on
`App::named`**, and those two fields arrived in the same ADR. A field test that
catches one half of one feature and is defeated on the other half by an unrelated
English sentence (§11R.N) is not a boundary; it is a coin.

**The instrument's other blind spot, found while checking it.**
`.github/binary-is-current.sh` is sound and reports `current` — its dep-info arm
covers 144 `.rs` and all eight `crates/ply-std/ply/*.ply`, which is what it
claims. But nothing anywhere checks that
`fixtures/reference-tests.corpus` is current with respect to
`crates/ply-syntax/src/tests.rs`, which `mine-fixtures.py` generates it from.
`agreement.rs` asserts only `fixtures.len() > 700`. It is 716; re-mining today
gives 889 (§11R.N). **The corpus is a checked-in artifact of a generator with no
freshness gate, in a spike with no CI job** — the same class as the binary this
repository built a whole script to catch, one directory over.

---

### §11R.X Implementing it: what the decision cost, and the four things taking it found

**Done 2026-08-30, in `~/.worktrees/ply/spike-parser-ci`. `run.sh --arm` exits
0.** §11R.D decided; this is what carrying it out actually took, including the
places the decision's own arithmetic was wrong. Pre-registered at
`/tmp/ply-preg/PREREG-restore-differential.md` before any number below existed.

#### What was written, against §11R.D's estimate of ~110 Ply lines

| §11R.D's row | estimate | actual | where |
| --- | ---: | ---: | --- |
| the `?` byte → `TPunct(b"question")` | 1 (+1) | **1** + 2 of comment + 1 test | `lexer.ply` `punct` |
| `ETry` + postfix `?` | ~14 | **12** | `exprs.ply` `postfix_expr` and four `match`es |
| `ERecordUpdate` + `{..b, f: e}` | ~26 | **45** | `exprs.ply`, and it needed `record_update_base` and `record_field`'s second-`..` refusal, which §11R.D did not list |
| `NamedArg` + `EApp.named` + ordering + the `perform` refusal | ~45 | **58** | `exprs.ply` `call_args_inner`, `call_arg`, `split_args`, `refuse_named` |
| `Param.default` | ~20 **+ a layering decision** | **48**, and the decision was forced | `exprs.ply`, moved out of `types.ply` |
| — | — | **6** | `spine.ply`: `t_question` and three diagnostic codes |
| **total** | **~110** | **~170** | |

**The estimate was 35% low and every line of the overrun is in the two rows
§11R.D flagged as uncertain.** The three mechanical rows came in at 13 against
an estimated 15.

**The layering decision was not a choice.** §11R.D wrote it as "either `param`
moves into `exprs.ply` or it takes the expression parser as an argument". The
second is not available: `Param` gains a field of type `Option<Expr>`, and
`Expr` is declared in `exprs.ply`, which imports `types.ply`. A module cannot
name a type from a module that imports it. So the **type** must move, and the
parser and dumper move with it. `types.ply` keeps a withdrawal note where the
declaration stood, quoting the sentence — *"it needs only `ty`"* — that stopped
being true.

#### The reference-crate cost, which §11R.D called the single real one

`crates/ply-syntax` grew `#[doc(hidden)] pub fn parse_unexpanded`, and `run` was
split into `parse_all` (the grammar and the recovery loop) plus the four lines
that gate the three rewrites, so that **the gate is the only difference** between
the two entry points and there is no second copy of the loop.
`parse_unexpanded_is_reached_by_no_shipping_caller` walks every `.rs` under
`crates/` and fails if any file but the three that are allowed to names it. Seen
to fail: appending `// parse_unexpanded` to `crates/ply-eval/src/lib.rs` reports
that file by path, and it was restored.

#### Four findings, in the order they arrived

**1. The differential went green on the first run, over the whole corpus.** No
`.ply` file was edited to make a corpus file agree — the same property the
original spike claims for itself in README §2. What was edited afterwards was
six in-language expectation strings, all of them the two new `#0;`/`?0;` records
the dump grammar gained, and one of those was a `prm` inside a `fn` — which is
how the `Param` move was confirmed not to have changed anything else.

**2. §11R.D's diagnostic price was wrong by two, in the direction that helps.**
See the correction inside §11R.D above: 7, not 9, and all 7 were already excused.
The lesson is about method rather than about effect sets — **a diagnostic cannot
be attributed to a pass by its code**, because two passes may share one, and
`items.ply` and `effect_set::expand` both raise `E0114`.

**3. An assertion written for the comparison caught a measurement, not a bug —
and that is why it moved.** `Dumper::effect_set` was given
`assert!(d.expansion.is_empty())` to replace §H4's projection. It fired
immediately, on `nodes_the_rewrites_add`, which deliberately dumps an *expanded*
tree through the same encoder to price what leaves. The assertion was right and
its **location** was wrong: "which entry point produced this tree" is a property
of `reference_dump`, not of how a node is encoded. It moved there, and the note
on `Dumper::effect_set` now says so. Recorded because the assertion was armed by
accident — it went red before anybody tried to make it.

**4. `fields.rs`'s repair is done, and it is the item this phase was handed.**
§11R.N ended: *"The repair is to make `fields.rs` scan what the dumper *emits*
rather than what it *mentions* — at minimum, strip `//` and `///` lines before
matching, which alone would have caught `App::named` — and it is the first item
of work this phase hands on."* The minimum is taken: `code_only` strips `//` and
`///` from **both** files before matching, `a_field_named_only_in_a_comment_does_not_count_as_covered`
arms it with the two shapes that caused the defect, and the field count is
re-taken at **149 fields across 30 parsed types, 2 deliberately absent** (it
said 144/29). The `dumper_boundaries` workaround — spelling `default`,
`mantissa` and `scale` *around* rather than writing them — is no longer load-
bearing, and its warning is corrected in place rather than deleted, because the
larger limit it names is untouched: **naming is still not emitting.** A field
read into a binding and never pushed to the output is green here, and
`arm-harness.sh` is the only thing that would catch it.

#### Two things found by running the generator, neither of them the parser

**`mine-fixtures.py` requires every string literal in `crates/ply-syntax/src/tests.rs`
to be printable ASCII, and nothing said so until one was not.** Adding
`parse_unexpanded_is_reached_by_no_shipping_caller` with an em dash in its
assertion message stopped the corpus generator with a bare `AssertionError`
printing the whole literal. The miner takes *every* string literal in that file
by design — §"The extraction is mechanical" argues for it — so its ASCII rule is
a constraint on the **reference crate's test messages**, imposed by a spike, and
written down nowhere. The message was changed to ASCII with a comment saying
why; the alternative — teaching the miner to skip or escape non-ASCII — is a
change to what the corpus contains and is not made under this heading.

**The corpus is still stale, and by more than §11R.N measured.** Re-mining today
gives **898 fixtures, 34,487 bytes** against the checked-in **716 / 22,737**
(§11R.N read 889 / 33,826 before this session's own test literals were added).
`agreement.rs` asserts only `fixtures.len() > 700`. **Deliberately not
re-mined**: it would take the differential from 766 inputs to 948, which is a
change a reviewer should see on its own rather than folded into this one, and
the real fix is a freshness gate rather than a one-off regeneration. What *has*
changed is the consequence: §11R.N's warning was that re-mining would bring 6
`NamedArg` and 3 `Param.default` subtrees into a comparison that could not see
them. It can see them now. So this is a stale artifact rather than a blind spot,
which is a strictly smaller problem — and it is the first item this phase hands
on, ahead of anything in §11R.D.

#### And two arming scripts had a mutation that tested itself, both pre-existing

Found by running all four per-area `arm-*.sh` scripts to check that moving
`param` had not invalidated anything. It had not. Two of their mutations were
already dead, in files this change did not touch:

| script | mutation | why it had stopped landing |
| --- | --- | --- |
| `arm-types.sh` | *an uppercase bare name in a pattern becomes a binder* | its anchor began `else if is_bare(q.node) &&`; `patterns.ply:206` reads `if is_bare(q.node) &&` and always has. A typo, live since the script was written. |
| `arm-items.sh` | *a `FnDef` span stops at its own keyword* | its anchor named `body.node`, which stopped existing when ADR 0028's `?` replaced the `bail` flag and `fn_def` began destructuring — `let {p, node: body} = fn_body(c, p)?`. |

Both re-anchored rather than dropped, with the reason written above each, and
both now **arm**. Neither is one of `arm-harness.sh`'s registered sixteen, so
neither figure in §H3 or README §2 moves.

**What is worth taking from it is the reporting shape, not the two typos.**
Both scripts *did* say so — `MUTATION DID NOT LAND` and `MUTATION MISSED` — and
both still exited 0 on the run that said it, so the message sat in a log nobody
read. `arm-harness.sh` is the one that gets this right: `NOT APPLIED` counts as
`invalid`, and the script's last line is
`[ "$survived" -eq 0 ] && [ "$invalid" -eq 0 ]`. The other three should adopt
it; that is a handed-on item and not done here.

#### What is now watched that was not

| | before | after |
| --- | --- | --- |
| inputs compared | 735 of 763 | **766 of 766** |
| bytes compared | 232,634 (29.8%) | **786,957 (100%)** |
| tolerances | 1, four conjuncts wide, 7 inputs | **0** |
| `App`'s keyword arguments | invisible: `g(1, b: 2)`, `g(1, c: 2)` and `g(1, b: h(2))` dumped identically | a `narg` node with its own span, in a length-carrying list |
| `Param`'s fallback expression | not dumped; `fields.rs` failing on it | an `Option` in `prm`, with the expression under it |
| `?` and `{..b, f: e}` | unlexable, unparsable | `etry` and `erup`, 95/95 tag coverage |
| mutations | 16, unrunnable against a red baseline | **22 armed, 0 survived, 0 invalid** |
| CI | none, anywhere | `parser-spike`, required through `ci`'s `needs:` |

**And what is not.** 2,087 lines of `effect_set.rs`, `record_update.rs` and
`try_op.rs` are outside this comparison permanently — 7 diagnostics and 3,974
nodes of it, measured. `defaults::expand`'s 912 lines never were in it and
cannot be. That is `GAPS-harness.md` §H2 item 5's job to keep saying, and it now
has a test's output to say it with.

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

> **RE-TAKEN IN §13R BELOW, AND THE FIGURES IN THIS SECTION ARE SUPERSEDED RATHER
> THAN WRONG.** They were measured on five files that have since changed — every
> one of the five — and against a Rust front end that has since grown two
> expansion phases. §13R takes both arms of the ported parser in one sitting and
> says which of this section's conclusions survive. **The 2.62 multiplier is now
> 2.44; the "30×" below is not comparable to §13R's 17.2× and neither supersedes
> the other.** Read §13R before quoting anything here.

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

> **Withdrawn as a current figure.** It was measured in one sitting and is sound
> for the tree it was taken on. Today the same two phases measure **17.2×** cold
> (§13R) — but the denominator is a different program (`try_op::expand` and
> `record_update::expand` now run inside `parse_module`), the estimator differs
> (user+sys against user), and the load band differs. **The honest statement is
> that this figure has not been re-taken, only re-measured on a moved target.**

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

### §13R Re-taken after `list_at`, `?` and `let` destructuring — both arms in one sitting

Pre-registered at `/tmp/ply-multiplier-retake/PREREGISTRATION.md` before any number
in it existed, including the predictions and the rule for what would mean the three
features did not pay for themselves. **Nothing here is compared against a figure
recorded above.** The pre-port parser was rebuilt as a *runnable* arm and the two
were run back to back, interleaved run by run, on one machine at one load band,
against the same `target/release/ply`.

**How the before-arm was established, since git was not available.** A candidate
pre-port tree was found in the port phase's own scratch and required to reproduce
the port's pre-port dump snapshot **byte for byte over all 763 corpus inputs and
3,063,669 dump bytes**. It does. The check is not vacuous: the same comparison
against the port's mutation-#4 mutant fails on 11 inputs. The after-arm likewise
reproduces the port's final snapshot, and **the two arms produce byte-identical
output on all 763 inputs** — so this is a timing comparison between two programs
that compute the same answers, which is the only kind worth taking.

**The corpus moved, and this is the first finding.** Every one of the five files
§13 registered has changed since §13 ran:

| file | §13 bytes | today | `?` in it now |
| --- | ---: | ---: | ---: |
| `examples/desk.ply` | 159,971 | 159,939 | 1 |
| `crates/ply-std/ply/db.ply` | 135,285 | 130,191 | **129** |
| `crates/ply-std/ply/http.ply` | 127,278 | 126,475 | 9 |
| `crates/ply-std/ply/json.ply` | 63,370 | 67,326 | 7 |
| `crates/ply-std/ply/router.ply` | 54,397 | 55,585 | 5 |

**So §13's per-file figures could not have been re-taken even on an idle machine:
the inputs are different files.** The brief's rule — take both arms in one sitting,
never compare against a recorded figure — is vindicated twice over here, once by
load and once by the corpus. Note also that `db.ply` now holds 129 `?`, which the
Ply lexer has no token for: **128 of its dump's markers are errors**, so `db.ply`
measures error recovery as much as parsing, and is reported but not leant on.

### The multiplier, both arms, `examples/` (13 files, 333,851 bytes, 45,028 tokens)

Minimum user+sys CPU (`rusage`) of N runs, N = 5 under 2 s wall and N = 3
otherwise, arms interleaved at run level, every run kept.

| | BEFORE | AFTER | after ÷ before |
| --- | ---: | ---: | ---: |
| lex (`L−Z`) — **the control** | 1.194 s | 1.199 s | **1.004** |
| lex+parse (`P−Z`) | 3.087 s | 2.930 s | **0.949** |
| parse alone (`P−L`) | 1.893 s | 1.730 s | **0.914** |
| **multiplier `(P−Z)/(L−Z)`** | **2.585** | **2.443** | **0.945** |
| parse ÷ lex `(P−L)/(L−Z)` | 1.585 | 1.443 | 0.910 |
| lexing's share | 39% | 41% | |

**The control is what makes the rest readable.** `lexer.ply` is byte-identical
between the arms — `cmp` says so — so `L−Z` is the same work twice, and it moved
**0.4%**. Everything else moved several times that, in one direction, on one
machine, minutes apart. Load ran 6.4 -> 10.8 across the series and **3 of 102
probe-series exceeded the registered 25% spread rule**, against 13 of 24 in a first
series taken at load 6.9 -> 12.4 which was re-taken; both are kept, in
`/tmp/ply-multiplier-retake/` (`raw-series1-FAILED-SPREAD.json`, `raw-series2.json`),
and declaring the re-take is part of the result.

### **Lex-plus-parse got 5.1% cheaper and parsing alone 8.6% cheaper. The multiplier fell from 2.59 to 2.44.**

Per file, the multiplier fell on **every one of the 12 inputs over 13 KB**
(desk −8.0%, db −6.9%, http −3.8%, json −5.3%, router −3.7%, agreement −6.5%,
hello −8.0%, orders −3.0%, store −5.1%, twin −4.0%, bank −2.9%, ledger −0.8%) and
rose on **all five under 6 KB** (clock +10.7%, echo +7.3%, report +7.0%,
timeout +3.5%, pipeline +3.0%), where `L−Z` is 0.005–0.026 s and fixed costs
dominate. Over the five files §13 registered, taken as one tree, the multiplier is
**2.832 -> 2.667**. Per-file range: **2.44–3.23 before, 2.33–3.06 after.**

**What did not get cheaper: typechecking the parser itself.** Summed over the 13
probes, the `Z` term rose **3.7%** (0.745 -> 0.773 s), and a direct probe puts it at
+12.7% for the six modules alone. The port added 16 functions (§14), and they have
to be checked. **This cancels out of the multiplier by construction — `Z` is
subtracted from both `L` and `P` — but it is a real cost the multiplier hides.**
That the subtraction is legitimate was checked rather than assumed: a directory
holding the six modules costs 0.053 s at `Z` against 0.003 s for `probe.ply` alone,
so `ply run` typechecks all six whether or not they are imported, and the term is
genuinely common to `Z`, `L` and `P`.

### What the features bought, stated at the altitude the design supports

**Registered, and it fails: the three-part test for "the features did not pay for
themselves" needed (a) no run-time win, (b) code down less than 5%, and (c) `fn`
up. (b) and (c) held; (a) did not.** So they did buy something measurable — and it
is modest: **~5% of run time and ~6% of code (§14), against 16 more functions.**

**No per-feature attribution is claimed, and none is possible from this design.**
`list_at`, `?` and destructuring landed in one tree and were measured as one change.
The mechanism claims — that the win is the deleted `Map<Int,Token>` build fold, the
83 removed guard tests, and `P` going nine fields to six on ~1,602 constructions,
rather than the per-peek swap ADR 0027 already measured at **1.02x** — are
*mechanism*, consistent with the numbers but not separated by them. §1's
prediction that the index would pay in time is the one that should be read as
refuted: four call sites cannot account for 5%.

### The scale figure — measured today, and NOT a re-take of §13's 30×

| | today, one sitting |
| --- | ---: |
| Ply lex+parse over `examples/`, ported | **2.930 s** |
| `ply check examples/` cold, 6 phases | **0.17 s** user+sys (0.15 user, 0.02 sys) |
| `ply check examples/` warm | 0.03 s |
| **Ply lex+parse ÷ Rust front end, cold** | **17.2×** (18.2× pre-port) |
| Ply lex alone ÷ Rust front end, cold | 7.1× |
| Ply lex+parse ÷ Rust front end, warm | 97.7× |

**This is not §13's 30× moving to 17×, and it must not be read that way.** Three
things differ at once. §13's Rust arm was **user-only at load 4.5**; this one is
**user+sys at load ~10**. And, decisively, **today's `ply check` is not the same
program**: `try_op::expand` (1,008 lines) and `record_update::expand` (529) now run
*inside* `parse_module` and did not exist when §13 ran. The denominator grew a
phase and a half. **A same-sitting comparison of the two Rust front ends is not
available without git, so no claim is made about which way the Rust side moved.**

Throughput, same series: Ply lexer alone **37,717 -> 37,550 tok/s** (the control),
Ply lex+parse **14,588 -> 15,370 tok/s**.

### What §13's conclusions survive

**§6.2's 5–10× band is still not refuted, and the port did not move it.**
Lex-plus-parse is 2.44× lexing after the three features, against 2.59× before —
**ADR 0020 §8's break condition ("only 2x the lexer") is still not met**, and it is
not close, with four phases unwritten. **The finding that lexing is 30–40% of
lex-plus-parse survives at 41%** — the port made parsing cheaper, so lexing's share
went *up*, which is the opposite of what would weaken §6.2's borrowed "10–20%".

**Measured here:** every figure in this §13R, on both arms, in one sitting.
**Carried over from §13 and not re-taken:** the `LD`/`PD` dump terms and the
"dump was 15–22% of §6.1" finding — the probe shapes were not re-run because the
port does not touch `lexer.ply`, and §13's control-arm figure for them stands as
recorded, at a different load, on different files.

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

> **Re-counted in §14R: 2.86 before the port and 2.89 after it**, against a
> `parser.rs` that has itself grown from 2,114 lines to 2,245. The sentence
> survives; its third digit does not.

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

### §14R Re-counted after the three features — one counter, both arms

No clock, so no load gate — but the same rule applies about comparing against a
recorded figure, and here it bites in a way §14 could not have anticipated:
**`parser.rs` itself has grown.** It is 2,245 lines today against the 2,114 §14
recorded, because `TokenKind::Question` and `ExprKind::RecordUpdate` are now parsed
there. **The denominator of every "Ply ÷ Rust" ratio in §14 moved, for the same
features that moved the numerator.** Both are therefore given below.

The counter is re-derived rather than quoted, and **validated before use**: it
reproduces §14 exactly on `spikes/ply-lexer/lexer.ply` (618/370/58), `spine.ply`
(702/313/123) and `patterns.ply` (384/253/20), and reads `types`/`exprs`/`items`
2/19/8 total lines higher, because §14's stripper also swallowed the blank
separators between `test` blocks and two test-helper `fn`s in `exprs.ply`. Both
counts are recorded and neither is silently substituted — §14 already does this
once, for `GAPS-harness.md` §H6's four-line difference. **Every before/after delta
below uses one counter on both arms, so the offset cancels.**

| | total lines | code lines | `fn` |
| --- | ---: | ---: | ---: |
| the Ply parser, 5 modules, **before** | 3,679 | 2,405 | 320 |
| the Ply parser, 5 modules, **after** | **3,718** | **2,260** | **336** |
| delta | **+39** | **−145** | **+16** |
| `spikes/ply-lexer/lexer.ply` (untouched) | 618 | 370 | 58 |
| `crates/ply-syntax/src/lexer.rs` **today** | 1,077 | 953 | 49 |
| `crates/ply-syntax/src/parser.rs` **today** | 2,245 | 1,990 | 101 |

| ratio | total | code | `fn` |
| --- | ---: | ---: | ---: |
| Ply parser ÷ Ply lexer, before | 5.95 | 6.50 | 5.52 |
| Ply parser ÷ Ply lexer, **after** | **6.02** | **6.11** | **5.79** |
| Ply ÷ Rust on the parser, before (today's `parser.rs`) | 1.64 | 1.21 | 3.17 |
| Ply ÷ Rust on the parser, **after** (today's `parser.rs`) | **1.66** | **1.14** | **3.33** |
| Ply ÷ Rust on the parser, after (against §14's **recorded** 2,114/1,898/98) | 1.76 | 1.19 | **3.43** |
| Rust parser ÷ Rust lexer, today | 2.08 | 2.09 | 2.06 |

### **Code lines fell 6.0%. Function count rose 5.0%. The two headline columns moved in opposite directions, and that is the finding.**

`?` deleted 83 guards and `let` destructuring collapsed 163 hand-threaded field
reads, which is where the 145 lines went. But `?` in return position needs a
function to return from, so 18 new ones exist only to hold one (§2), and
**Ply-functions-per-Rust-function got worse: 3.17 -> 3.33** against today's
`parser.rs`, or 3.24 -> 3.43 against the one §14 measured. §14's diagnosis stands
and sharpens: *"the `fn` column says where it goes."* It still does. **The feature
that removed the most code also added the most functions.**

**Total lines went *up* 39 while code lines went *down* 145** — about 184 lines of
comment. That is house rule 2 being obeyed: every withdrawn claim is quoted in
place rather than deleted. It is worth stating plainly because it is a cost of the
method and not of the language: **on this repository's own rules, correcting a
record costs more lines than the change being recorded saves.**

**What §14's headline sentence becomes.** *"The step from lexer to parser costs Ply
2.99× what it costs Rust"* was 5.91 ÷ 1.98. On today's files it is 6.02 ÷ 2.08 =
**2.89** after the port, and 5.95 ÷ 2.08 = **2.86** before it. **The three features
did not measurably change the lexer-to-parser step; both arms sit within 1% of each
other on it.** §14's *"a lexer in Ply is shorter than its Rust original (0.58), a
parser in Ply is longer (1.73)"* survives with the parser figure at **1.66**, and
the crossover it names is unmoved.

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

> **Withdrawn (2026-08-30):** *"Yes — it does, and it agrees with the reference
> over 763 inputs, 780,456 bytes and 126,565 nodes, with zero disagreements and one
> priced boundary."* It was true of the tree it was written on. **Today the
> differential is red on 28 of 763 inputs — 70.2% of the corpus by bytes — because
> `?` and `{..b, f: e}` became syntax after the spike was taken.** The answer below
> is unchanged in kind and its evidence is now narrower: see **§11R**.

**Yes — it does, and it agrees with the reference on 735 of 763 inputs, tree and
diagnostics, byte for byte; the 28 it does not are the ones written in syntax the
port predates, and that set is identical before and after the port (§11R).** ADR 0020
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

---

## §15 The second phase: `resolve.rs` and `defaults.rs`, ported behind a second differential

`docs/BOOTSTRAP-PATH.md` step 6 asks for the other four phases "each ported the
way the parser was: a reference dumper on the Rust side, a corpus, and mutations
that prove the comparison can go red." `resolve.ply` is the first of them.

**What was ported.** The module index and its duplicate-module refusal; each
module's declarations, first declaration winning and a sum type's constructors
as public as the type; the scope each import builds — module binders, aliases,
selective imports, and the six diagnostics of `ScopeBuilder`; `traverse`'s
iterative DFS, with the same postorder and the same once-per-cycle canonical
rotation; and the whole of `defaults.rs`: the admissibility of a default, its
qualification against the module that wrote it, the signatures, the call
filling with its four diagnostics, and the implicit imports a spliced default
adds to the caller's scope. `harness/src/lib.rs`'s `reference_resolve_dump`
writes the tables, the load order, the diagnostics with their module index and
the post-defaults trees in one record encoding; `resolve_dump` writes the same
from the port.

**What agrees.** `harness/tests/resolve.rs`: the standard library as one
program, the standard library with each example, every multi-module program
`resolve.rs`'s own tests build (`mine-programs.py`, `fixtures/reference-programs.corpus`)
and a hand-written bundle of the error paths the tree never reaches
(`fixtures/resolve-programs.corpus`). The error-path and reference programs
agreed on the port's first run; the standard library did not, and the reason was
not the resolver: this spike's lexer had never learned hex literals, so
`hash.ply`'s masks parsed as zero — the parser differential had excused the
module by name (`POSTDATES_THE_PORT`) and this comparison could not. The lexer
now reads them, the exemption list is empty, and it stays so that the next
surface to land is named there rather than skipped. `arm-resolve.sh` carries
mutations across the three parts, each of which the fast half of the comparison
catches, and prints how many.

**What it cost to write, and the one thing the language charged.** The port is
short beside the parser's. The tax was not the threading of state — a `fold` over a record
does what `&mut self` does — and not `IndexMap`, whose insertion order a list
keeps; it was record update. `{..b, f: v}` expands in the parser (ADR 0029) and
therefore needs the base's field list *in the same file*: a `let` without a
written type is refused with `E0116`, and a type declared in another module —
every AST node the port rewrites — has no field list this file can name at all.
So each of the ~30 places that rebuild an expression node spell every field of
that node, and every threaded state is a type declared in `resolve.ply` with
its binders annotated. It is a real cost and a cheap one to remove: an
expansion that ran after inference could read the shape from the type; ADR 0029
chose the parse-time expansion for the hash's sake, and that decision is where
the change would have to be re-taken.

**What the comparison cannot see.** Only what the parser's cannot (§11): the
prose of a diagnostic. The trees it compares are the post-defaults,
pre-rewrite trees, which is the phase both sides run.

---

## §16 The three rewrites, ported: the checker reads what `Parser::run` writes

§11R.D decided to compare **pre-expansion** trees and priced the three rewrites
`Parser::run` applies after the grammar — `effect_set::expand`,
`record_update::expand`, `try_op::expand` — as the reference-crate cost of that
decision, left in Rust. The next phase re-opens the question from the other
side: `crates/ply-core` reads the **expanded** tree, and unreachable arms in its
inferrer say so (`ExprKind::RecordUpdate` and `ExprKind::Try` "expanded away by
`parse_module`"). A checker written in Ply therefore needs the rewritten tree
first, and `rewrite.ply` is the port of all three.

**What was ported.** The effect-set table with its duplicate, cross-module and
unknown-set refusals, the cycle search with its one-per-hit report, the
post-order expansion with `canonicalize`'s sort-and-dedup, the write-back into
each set's own item, and the row walk that appends every alias's expansion —
including the reference's own omission, which is that a lambda's written return
type is not walked. The record-update expansion with its scope of written types,
the alias chase, the sorted copies over the base's span, and `E0116`/`E0117`.
And the try operator whole: the mode read off the written return type through
aliases, the constructor-shadowing refusal, the evaluation-order scan that lifts
the first reachable `?` to a fresh binder, the block split at a statement, the
return-position walk through `if` and `match`, the `wrap` with its failure arm
first, and the sweep that refuses what is left with `E0118` and `E0119`. The
binders are numbered per item, as the reference's are.

**What agrees.** `harness/tests/rewrite.rs`: `dump_expanded` against
`parse_recovering`'s dump over every example, the standard library, the
hand-written fixtures and the reference's own inputs — which carry every
diagnostic the three passes raise (§11R.D's re-take counted them). All four
agreed on the port's first run; `arm-rewrite.sh` arms them.

**Two things the port found.** The immutable tree is where the rewrites' cost
lives, and it is the same cost §15 met: every node the walk rebuilds spells
every field, so a pass the reference writes as a `&mut` walk is here a fold that
answers the node again. And the rewrites are not independent of the *order*
`Parser::run` gives them — a `?` inside a written field of an update meets the
record-update pass first, and the try pass then scans the record literal it
became; the port keeps that order and the differential is what says it matters.

---

## §17 The checker, ported: `infer.rs` behind a fourth differential

`docs/BOOTSTRAP-PATH.md` step 6 called inference with rows "the hard one".
`tycore.ply` is `ty.rs`, `unify.rs`, `env.rs` and `print.rs` — the type
language, the substitution and unification over it, the environment with
instantiation and generalization, and the printer schemes are published with.
`infer.ply` is `infer.rs`: the prelude, the declarations, the signatures, the
component-wise check of definitions with Tarjan's order, every expression form
including `perform`, `handle`, cells, regions and `simulate`, numeric settling,
the written-signature requirement, spec clauses, tests, laws with the
quantifiability of their binders, the derivability predicate behind `where`
clauses and `Map` keys, the comparison-of-functions check, the `simulate` and
region escape checks, and the internal-effect marking. `check_dump` writes what
`check_program` publishes — every definition's scheme, footprint, performed row,
constraints and internal-effect flag; every test's and law's footprint; every
effect and constructor — or the diagnostics, deduplicated as the reference
deduplicates them, and `harness/tests/infer.rs` compares it with
`reference_check_dump`.

**What agrees.** The standard library as one program, every example without a
`derive` checked together with it, the resolver's programs, a hand-written
bundle for the checker (`fixtures/check-programs.corpus`), and every input
`mine-checks.py` mines from `crates/ply-core/src/tests.rs` the way
`mine-fixtures.py` mines the parser's — most of those are error paths, so that
is where the diagnostics are compared code by code and label by label. The one
thing that stood between the first run and agreement over the green programs
was not the checker: a test label with a backslash crosses two escaping layers
on its way out of `ply run --json`, and the harness's extractor had undone one.
`arm-infer.sh` arms the comparison.

**What the last disagreement taught.** The reference's `deduplicated` keeps two
diagnostics apart by their *message* and their label texts, not only by code,
spans and note count: a parameter default of the wrong type is reported once as
"parameter default" and once per call site the defaults pass copied it into as
"argument type", at the same span, and both survive. The port's diagnostic
carried none of that, so its dedup was coarser and one of the mined inputs
disagreed by a count of one. `Diag` now carries a `text` — the unification
context and the two printed sides — that the dump does not print and the dedup
compares; every `expect` site names the reference's context string. The cheap
key looked right for a long time, because nothing in the tree reports the same
code twice at one span.

**What the arm taught.** Four mutations were caught by nothing — a body that
performs more than it declares, a numeric operand left unsettled, a test
footprint dropped, a closed row absorbing an atom — because every corpus the
fast half ran was a program that checks, and the mined inputs happen to hold
none of those shapes; two more were equivalent mutations (generalization over
the environment's variables, which is always empty at top level, and naming
scheme variables head-first, which is body order for a generalized scheme).
The hand-written checker bundle exists to hold the first four, and the two were
re-aimed. A mutation that stays green is a corpus gap or an equivalent mutant,
and the run says which only after the corpus is read.

**What the language charged this time.** The same tax §15 and §16 met, and one
more: a threaded state with thirty fields is written back in full at every step,
so every `let` that is later updated needs its type written — which a checker,
whose every step answers a state, does a few hundred times. The port did not need
`Map` for anything but the substitution and the environment, where a program the
size of the standard library needs the lookup to be logarithmic; everything the
reference keeps in an `IndexMap` for its order is a list here, and the order is
what the dump compares.

**The restored path.** `Known` — the interfaces the driver hands in for
definitions whose bodies it need not walk again, and the footprints of tests it
need not re-infer — is ported with `check_program_with`: a group whose every
member is known is published from its interfaces, its scheme adopted into this
run's variables, its constraints recovered by unifying the restored scheme with
the signature just built, and its clauses still typed. The instrument is the
checker's own output: `check_dump_known` checks a program, hands back what it
published, checks again from those interfaces and dumps the second check, and
the reference does the same through `check_program_with`, so the two restored
paths are compared over the standard library and every bundle. The derive
expansion, and the two checker passes that read what it produced, are §18. One
thing the port
asked of the code generator rather than of the language: `map_fold` is the one
callback builtin the checker uses that the fragment did not lower, and
`parser_census::the_census_over_the_parser_spike` pins that no callback is
refused, so the runtime gained the loop.

## §18 The deriver, ported: `ply-derive` behind a fifth differential

`derive.ply` is `crates/ply-derive` — `rules.rs`, `walk.rs`, `emit.rs`,
`retarget.rs` and `lib.rs`: the table that says what a type constructor means to
a derivation, the syntactic walk that refuses a field before anything is
generated, the emitter that writes a dictionary as Ply source, the parse of that
source back through the same parser, the retargeting of every span in it to the
`derive` item's, and the expander with its orphan, collision, missing-runtime and
internal-error diagnostics. `harness/tests/derive.rs` compares `derive_dump` —
each module expanded on its own, its generated sources byte for byte and its
diagnostics — with `reference_derive_dump`, over a hand-written bundle of every
shape the reference's own tests exercise (`fixtures/derive-programs.corpus`),
every example and every standard-library module; `arm-derive.sh` arms it. The
checker's pipeline now expands before it resolves, the two examples that
`derive` join the fourth differential, and `check_derives` and
`attribute_generated` — the passes that read what expansion produced — are in
`infer.ply`, keyed on the `derived` list `resolve.ply`'s `Mod` carries beside
the module, since `FnDef::derived` is not a field the port's parser writes.

**What agreed first time.** The golden pin the reference keeps of its own output
matched byte for byte before the differential ran: an emitter is string
concatenation, and string concatenation ports. The retargeting walk is where the
language charged: a generated tree's spans cannot be stamped at the tokens,
because the parser reads literal text through node spans, so the walk rebuilds
every node kind of the AST with every field spelled — the same record-update tax
§15 met, paid once more, over forty record shapes.

**One table.** `con_shape` now lives in `derive.ply` and `infer.ply` imports it,
as `derivable.rs` reads `ply_derive::rules::shape`: the type the deriver can
encode and the type the checker admits are decided in one place, which is the
only way they cannot disagree.
