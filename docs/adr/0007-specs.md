# 7. Specs

Status: accepted — **implemented**
Date: 2026-08-13

> **Corrected by the W6 documentation audit.** This line read "the decision
> types, the AST and the diagnostics landed; everything that decides an
> obligation is outstanding". Everything that decides one has since landed,
> verified against the tree rather than against a later ADR: the prover is
> `crates/ply-prove/src/prove/` (`lower.rs`, `egraph.rs`, `solve.rs`,
> `arith.rs`, `term.rs`), the property engine is
> `crates/ply-prove/src/property.rs`, the shrinker is
> `crates/ply-prove/src/shrink.rs`, and `ply prove` is a real subcommand
> (`Command::Prove` → `commands::prove::execute`). W2 then amended the prover's
> `Float` rule — see ADR 0012 §A3 and §A6, which moved `PROVER_VERSION` to
> `0.4.0` because an obligation over a hidden `Float` is now refused where it
> was previously certified.
Builds on: `0002-incremental-frontend.md` and `0003-cache-storage.md`, whose
content-addressed front end is what makes an obligation discharge once ever;
`0006-deterministic-simulation.md`, whose `exhaustive: true` is the only place a
`proved` in this milestone comes from execution.

**What is landed**, so that six concurrent implementations cannot disagree about
the parts a disagreement would be silent in: `ply_span::codes`' five new codes;
`ply_syntax::ast`'s `Item::Law`, `LawDef`, `Binder`, `SpecClause`; `ply_core`'s
`SpecInfo`, `LawInfo`, `DefInfo::spec` and `CheckOutput::laws`; and the whole of
`ply-prove`'s decision vocabulary — `Tier`, `Evidence`, `Certificate`, `Rule`,
`Discharge`, `Obligation`, `Coverage`, `ProvePlan` and the two cache keys.
Everything that *decides* — the parser, the purity check, the prover, the
generator, the shrinker, the two commands — is specified here and implemented
against this.

## Context

DESIGN.md's thesis has two halves. The first — that the verification loop
collapses toward zero — is what M0–M7 built, and it is demonstrable: renaming a
function selects zero tests, a `det` test caches forever, a concurrent test can
report `exhaustive: true`. The second half has never been built at all:

> what remains for a human to review is a **specification** rather than an
> implementation.

Nothing in M0–M7 makes that true. A reviewer today reads implementations. The
test suite tells them the implementations agree with the tests, and the tests are
themselves implementations — concrete programs over concrete values, which a
reviewer must also read to know what they assert. The artifact that would let
review stop at the claim does not exist.

So M8 is the closing argument, and it has exactly one way to fail: **by lying.**
Every prior milestone could produce a wrong answer — a mis-selected test, a
missed interleaving, a bad culprit — and every one of them fails loudly and
locally. This milestone can produce a wrong answer *wearing a certificate*. A
reviewer who is told an obligation is `proved` stops reading, which is the entire
point of telling them, and which is why a wrong `proved` is worth more damage
than every other defect in this system put together.

Hence:

> **A tier label is a truth claim.** When in doubt, report the weaker tier.

Everything below is downstream of that sentence, including several places where
the design gives up reach it could have had.

## Decision

### 0. The rule everything else follows from

> **A spec is a claim *about* a definition, not part of it. An obligation is
> discharged at the strongest tier the system can *demonstrate*, and the tier is
> derived from the evidence rather than asserted alongside it.**

Two halves, and both are structural rather than procedural.

The first half decides hashing (§4): a spec is erased by normalization, so
writing one changes no definition hash, re-runs no test, and rebuilds nothing.
The claim gets its own hash, which covers the definition's — so the *obligation*
invalidates when the implementation moves, while the *implementation* does not
invalidate when the claim moves. That asymmetry is exactly the asymmetry review
has.

The second half decides the type of `Discharge` (§5.6): there is no field
anywhere in this milestone that says "the tier is `proved`". A tier is computed
from the evidence a discharge is carrying, and the only evidence that computes to
`Proved` is a `Certificate`, which only the prover can construct and which names
the rules it used. A component that wants to report `proved` has to produce a
proof; there is no other spelling.

---

### 1. Surface syntax

#### 1.1 Clauses on a definition

```
fnDef  := "pub"? "fn" IDENT generics? "(" params ")" ("->" type)? ("/" row)?
          specClause* ("=" expr | block)
specClause := ("requires" | "ensures") expr
```

```ply
fn withdraw(acct: Account, amount: Int) -> Account
  requires amount > 0
  requires amount <= acct.balance
  ensures result.balance == acct.balance - amount
  ensures result.id == acct.id
= Account { id: acct.id, balance: acct.balance - amount }
```

Clauses appear in source order and there may be any number of each. Each
`ensures` is its **own obligation**, discharged and reported at its own tier: a
definition whose first postcondition is proved and whose second is sampled
should be told so, and one clause per definition would force the pair to share
the weaker label.

`requires` and `ensures` are **contextual** keywords, recognized only between a
`fn` header and its body — the same device as `resume` (ADR 0005 §1.4) and
`simulate` (ADR 0006 §2.1). `lexer::is_ident("requires")` stays true, no `Kw`
variant is added, and a program that already has a function or a local named
`requires` is unaffected. The position is unambiguous: after the optional `->
type` and `/ row`, the only tokens the grammar previously admitted were `=` and
`{`.

A clause's expression is parsed with the parser's existing `no_brace` flag set,
exactly as an `if` condition is, so `ensures p(x) { .. }` is a clause followed by
a block body and never a record literal. `=` is not an operator in Ply, so the
`= expr` form terminates a clause expression with no lookahead.

#### 1.2 `result`

`result` is bound in an `ensures` clause to the definition's return value. It is
an ordinary local binder, introduced into the clause's scope **beside the
parameters rather than inside them**, so:

- `result` in a `requires` clause is `UNKNOWN_NAME` (E0101), with a note saying
  it is bound only in `ensures`. A precondition that could name the result would
  be a claim about a value that does not exist yet.
- a parameter named `result` on a definition that has an `ensures` clause is
  `DUPLICATE_DEFINITION` (E0105), pointing at both. Silently shadowing the
  parameter would change what an existing program's `ensures` means depending on
  a parameter name; silently shadowing `result` would make the postcondition
  unwritable with no diagnostic. Neither is acceptable, and the error costs
  nothing: a definition with no `ensures` may still call a parameter `result`,
  which several files in `examples/` do.

`result` is not a keyword. `lexer::is_ident("result")` stays true.

#### 1.3 Laws

```
lawDef := "law" STRING ("forall" "(" binder,* ")")? ("where" expr)? block
binder := IDENT ":" type
```

```ply
law "credit and debit cancel"
  forall (a: Account, n: Int) where n > 0 && n <= a.balance {
    credited(debited(a, n), n) == a
  }

law "reverse is an involution"
  forall (xs: List<Int>) {
    reverse(reverse(xs)) == xs
  }

law "map fuses"
  forall (xs: List<a>, f: (a) -> b, g: (b) -> c) {
    map(map(xs, f), g) == map(xs, |x| g(f(x)))
  }
```

`law` is a contextual keyword at item position — nothing else in the grammar
starts an item with a bare identifier, so there is no ambiguity and `fn law(..)`
still parses as a function. `forall` and `where` are contextual within a law
header. The `where` guard is parsed under `no_brace`, so the block that follows
is the law's body.

A binder's type is **mandatory**. Inferring it from the body would make a law's
meaning depend on how the body happened to be written and would make the
generator's job unstated; `Binder` therefore carries a non-optional `TypeExpr`
rather than reusing `Param`, so the invariant is unrepresentable rather than
checked.

`forall` is optional. A law with no binders is a ground claim
(`law "empty reverses" { reverse(nil) == nil }`), and §5.3 gives it a tier that
is stronger than it looks.

A law is labelled, not named: it is keyed `<module>.<label>` exactly as a test
is (`TestInfo::key`), it cannot be `pub`, and nothing can reference it. Two laws
with the same label in one module are `DUPLICATE_DEFINITION`.

#### 1.4 What was considered and rejected

**`assert` in the body instead of an expression.** `ensures assert_eq(result, x)`
reads like a test and would let a clause use the assertion machinery's structured
diff. Rejected because an assertion is an *action* and a spec must be a
*proposition*: the prover needs a Boolean term to negate, and an obligation whose
statement is "this program did not raise" cannot be proved statically about
anything. The structured diff is recovered at the property tier by rendering the
counterexample bindings, which is strictly more useful — it names the input, not
the intermediate.

**`ensures` naming the function rather than binding `result`** (`ensures
withdraw(acct, amount).balance == ..`). No new binder, no shadowing question. It
also invites a clause that calls the function it specifies with *different*
arguments, which is a claim about a different call, and it makes every
postcondition re-evaluate the definition once per mention. `result` is one
binder and it means one thing.

---

### 2. A spec expression is pure

> **A spec expression's row must be empty.**

Enforced in the type system, in `ply-core`, as one row-purity test per clause:

```
Γ, params ⊢ e : Bool / ρ        ρ.is_pure()            (else E0417)
──────────────────────────────────────────────────────  requires
Γ ⊢ requires e

Γ, params, result : T ⊢ e : Bool / ρ        ρ.is_pure()  (else E0417)
──────────────────────────────────────────────────────  ensures
Γ ⊢ ensures e

Γ, binders ⊢ g : Bool / ρ_g     ρ_g.is_pure()            (else E0417)
Γ, binders ⊢ b : Bool / ρ_b     ρ_b ⊆ {sim.read}         (else E0417)
──────────────────────────────────────────────────────  law
Γ ⊢ law "…" forall (binders) where g { b }
```

`Row::is_pure` is `atoms.is_empty() && tail.is_none()`, which already exists and
is already what "an empty effect row" means. The tail matters: a spec inside an
effect-polymorphic function whose clause calls a row-polymorphic argument has row
`{| e}`, which is not pure, because it is not pure for every instantiation.

**Why this is not a restriction to be relaxed later.** A spec that can perform
effects can change what it observes. An `ensures` that writes to the resource it
is judging is not a weak specification, it is a meaningless one — the
post-state it reports is the post-state it caused. And a property run evaluates a
clause hundreds of times: an effectful clause would perform hundreds of times,
against a world nothing set up, and its footprint would enter the definition's
row and therefore the cross-test conflict graph, so attaching a spec would change
which *tests* may run concurrently. Purity is what keeps a claim from being a
participant.

Three consequences worth naming:

- **`ply prove` needs no conflict graph.** Every obligation is pure, so no two
  obligations contend, so they are all in group 0 for any number of them. The
  scheduling machinery of M4 is not consulted and is not needed. This falls out
  of the rule rather than being arranged.
- **A spec contributes nothing to any footprint.** `DefInfo::footprint` is
  unchanged by attaching a clause, so E0412 is unchanged, `Isolation` is
  unchanged, and the cross-test conflict graph is unchanged. Required test.
- **`SpecInfo::footprint` is carried anyway,** always empty, so an audit can
  assert emptiness against a value rather than against a comment.

#### 2.1 The one exception: `sim.read` in a law body

A law body's row may be exactly `{sim.read}`, which is what a body containing a
`simulate` region has (ADR 0006 §2.2). Such a law is a **concurrency law**, and
§6 is its whole story.

The exception is narrow on purpose:

- it is the **body** only. A `where` guard must be exactly pure: the guard
  decides which values the law is a claim about, and a domain that depends on a
  seed is a different domain per run.
- it is `sim.read` only, never an arbitrary atom. `sim.read` is a read of an
  input no program can write (ADR 0006 consequences, `Isolation` widening), which
  is precisely why it is not a way for a claim to disturb the world.
- a `requires` or `ensures` clause may **not** carry it. A pre/post condition is
  a claim about one call, not about a search, and there is no seed at a call
  site to name.

---

### 3. What a law quantifies over

#### 3.1 Values, including function values

A binder's type is any Ply type, and Ply already types a function as
`(a) -> b / e`. `forall (f: (Int) -> Int)` therefore costs the type system
nothing and buys the laws worth having — map fusion, fold/append, "sorting by any
key is a permutation".

Two restrictions, both consequences of §2:

- **A function-typed binder's row must be empty.** `forall (f: (Int) -> Int / {db.read[users]})`
  is `E0418`: applying `f` inside the body would make the body impure, so the
  binder is unusable. Written without a `/ ..` the row is empty and this never
  fires.
- **A binder's type must be inhabitable by the generator.** `Cell<_>`, `Task<_>`
  and any type reaching one are `E0418` at check time rather than a gap at prove
  time, because a law nobody can ever check is a claim nobody will ever read, and
  the user should learn that when they write it.

**Type variables in a binder** are permitted, and are handled differently by the
two tiers, which is the honest reading rather than a compromise:

- The **prover** treats a type variable as an **uninterpreted sort**. A proof
  over an uninterpreted sort is a proof for every instantiation, so a `proved`
  polymorphic law is genuinely polymorphic and `Certificate::sorts` records
  which variables stayed uninterpreted.
- The **property tier** cannot generate a value of an unknown type, so it
  monomorphises each variable to `Int` and records
  `CaseReport::instantiations`, which the report prints. `property` on a
  polymorphic law is a claim about `Int` and says so.

Effect-row variables in a binder are `E0418` for the same reason a non-empty row
is.

#### 3.2 Laws do **not** quantify over handlers in v1

The law everyone wants to write is not about values:

```ply
// not writable in M8
law "any lawful state handler round-trips"
  forall (H: handler(state)) {
    handle { state.put[s](5); state.get[s]() } with H == 5
  }
```

It is not writable because **a handler is syntax, not a value**. `handle body
with { .. }` is an `ExprKind::Handle` whose clauses are a `Vec<HandleClause>`;
there is no `Value::Handler`, no type that a handler inhabits, and therefore
nothing for a `forall` binder to range over. `ply-eval`'s prompts hold clause
ASTs and captured environments (ADR 0005 §1.1), not a first-class object.

Recording it as the natural follow-on, with what it would take:

1. **A handler type.** Something like `Handler<{db.read[r], db.write[r]}, ρ>` —
   a type indexed by the set of atoms it discharges and by the residual row of
   its clauses. `ply-core::ty::Type` gains a variant, which is a change to a
   module marked *pinned* and therefore a cross-crate breaking change.
2. **`Value::Handler`,** a closure-like value holding the clause set and its
   environment, plus a `handle body with h` form where `h` is an expression. The
   machine's `Prompt` (ADR 0005 §1.1) is already almost this shape, so the
   evaluator half is small; the typing half is not.
3. **A normalization story.** A handler value's identity must be its clause set
   up to renaming, so the normalizer needs a handler discriminant and the usual
   de Bruijn treatment of its binders — a `BODY_ENCODING` bump.
4. **A generator for handlers,** which is the part that decides whether the
   feature is worth it. Generating "an arbitrary lawful handler" means generating
   an arbitrary function per operation, which §8.1 can do, plus deciding what
   *lawful* means, which is itself a spec. Realistically the property tier would
   sample a handful of handlers and the honest tier for such a law would be
   `example` far more often than `property`.

The reason to defer it is (4), not (1)–(3): the mechanism is a milestone's work
and the *evidence* it would produce is weak. A handler-parametric law that could
only ever be sampled is worth less than a value-parametric law that can be
proved. When first-class handlers arrive for their own reasons — a scheduler
chosen at run time, a handler stored in a record — the law form follows in a
week.

---

### 4. Laws and specs are content-addressed

#### 4.1 A spec is erased by normalization

`requires`, `ensures` and every `law` are **erased by the normalizer**, exactly
as names, spans, `pub` and module membership are (ADR 0001's rule). Therefore:

> **Adding, editing or deleting a spec changes no definition hash anywhere, so
> it re-runs no test and rebuilds nothing.**

That is a headline invariant of the same shape as "renaming a function selects
zero tests", and it is a required test. It is also the property that makes the
milestone usable: the spec is the artifact a reviewer edits, and an artifact
whose every edit invalidates the whole test suite is an artifact nobody edits.

The alternative — folding a clause into the body it is attached to — was
considered and is worse in both directions. It would make a spec edit
invalidate every transitive dependent's hash, and it would make two definitions
that compute the same thing under different claims into two definitions, which
is false: they are one computation.

#### 4.2 What an obligation is keyed by

```
spec_hash(owner, kind, index, clause) =
    blake3( b"ply.spec.1"
          ‖ owner_def_hash                 -- 32 bytes
          ‖ kind_u8                        -- 1 requires, 2 ensures
          ‖ index_le_u32                   -- position among the owner's clauses
          ‖ normalize(clause) )            -- the normalizer's stream
```

`normalize(clause)` is the ordinary body encoding: the owner's parameters and
`result` become de Bruijn levels, a free reference to a top-level definition
contributes **that definition's hash**, names and spans are erased.

A law is hashed by the ordinary path, like a test: it is an item with a body, it
gets its own discriminant byte, its binder types and guard and body are
normalized together, and `HashOutput::laws[i]` is its `DefHash`.

**The obligation key is that hash.** No wrapper: a law's hash is already a
definition hash of a law item, and a spec hash is already domain-tagged.

The two properties this buys, stated as the invariants they are:

1. **An obligation discharges once and stays discharged until something in its
   closure changes.** The key covers the clause's own structure, the hashes of
   every definition the clause names, and — through `owner_def_hash` — the entire
   transitive closure of the definition being specified.
2. **Editing the implementation invalidates the obligation.** This is the
   permissive-direction failure and it is the one that must not ship: a key that
   omitted `owner_def_hash` would leave a discharged `ensures` discharged after
   its definition was rewritten, which is a cached proof of something no longer
   true. `owner_def_hash` is the first field of the stream for that reason, and
   the required test edits a body and asserts the obligation re-runs.

#### 4.3 How it composes with the result cache

Obligation results live in their own map, `obligations`, in their own file
`obligations.json`, read lazily on the first question — ADR 0003's addendum
measured what happens when a per-test payload is folded into `results.json`, and
the answer was `Store::open` at three times its budget.

What the store holds is an **`Evidence`**, not a `Discharge`. `Evidence` has no
variant for a refutation, a vacuity or a gap, so the "never cached" rows of the
table below are enforced by the type the cache is written in rather than by the
discipline of whoever writes it — the same device as §5.6, applied to
persistence.

The namespace is keyed on `(PROVER_VERSION, key)`. `PROVER_VERSION` is a new
`pub const &str` in `ply-store`, independent of `RUNTIME_VERSION`, because a
prover that learns a new rule must be able to *upgrade* a tier without
invalidating a single test result, and a runtime change must invalidate test
results without invalidating a proof that never ran a program.

The rule, which is ADR 0006 §8.1's rule with one word changed:

| discharge | written under |
| --- | --- |
| `Held` at `Proved` | the bare obligation key |
| `Held` at `Property` or `Example` | `prove_key(key, plan)`, and **never** the bare key |
| `Refuted` | nothing |
| `Vacuous` | nothing |
| `Unattempted` | nothing |

```
prove_key(key, plan) =
    blake3( b"ply.prove.key.1" ‖ key
          ‖ cases_le_u32 ‖ prove_budget_le_u32
          ‖ roots_len_le_u32 ‖ roots_le_u64*
          ‖ sim_plan_digest )
```

`shrink_budget` is deliberately **not** in the digest: it can only change the
minimality of a counterexample, failures are never cached, so it cannot change a
cached claim.

**The asymmetry is the whole operational value of `proved`.** A sampled
discharge is a claim about the plan that sampled it, so widening the plan
re-runs it — reading it under a wider plan would let `--prove-cases 10` satisfy a
run that asked for a thousand. A proof is not a search: it is a claim about all
inputs satisfying the guard, so it is valid under every plan and it costs nothing
forever. A proved obligation is the only thing in this system that a wider search
does not have to re-examine.

`Refuted` and `Vacuous` are not cached for the reason a failing test is not: a
red result re-runs until it goes green.

#### 4.4 What the incremental front end owes

Gate 2 (ADR 0002) skips re-inferring a definition whose `DefHash` is unchanged.
Because a spec is erased from that hash, **a spec edit does not move it**, so
gate 2 would skip a definition whose clause is new or changed and the clause
would never be typed.

> **Spec clauses are never skipped by gate 2.** A definition restored from
> `KnownDef` has its clauses typed against the restored `Scheme`, every run in
> which its file was parsed.

Gate 1 is what makes this cheap and correct: a spec edit is a file edit, so the
file's `content_hash` moves, so the file is parsed, so its clauses are in hand.
A file that gate 1 skipped has clauses that are byte-identical to the ones whose
hashes are in the fingerprint, so nothing was missed. `SourceFingerprint` gains
a `specs` field — one `(DefHash, Vec<DefHash>)` per definition carrying clauses,
plus the file's law hashes — for the same reason it already carries `defs` and
`tests`: so a skipped file can still contribute its obligations to the run.

Typing a clause against a restored interface costs one pass over a small pure
expression, so a project with a spec on every definition pays one extra
traversal per changed file and nothing at all for an unchanged one.

---

### 5. The tier contract

This is the heart of the ADR, and its correctness matters far more than its
reach.

```rust
pub enum Tier { Example, Property, Proved }     // Ord: Example < Property < Proved
```

| tier | what it claims |
| --- | --- |
| `proved` | a static argument covering **every** input satisfying the guard |
| `property` | randomized cases, the count reported, shrinking on failure |
| `example` | concrete cases, and **no** coverage claim |

#### 5.1 What qualifies for `proved`

An obligation is `proved` iff a decision procedure over the fragment below
answers **valid** for

```
    guard  ⟹  body
```

with every binder replaced by a fresh symbolic constant of its type — which is
what makes the answer a universal statement rather than a statement about one
case.

The fragment, exactly, and nothing outside it:

**(a) Linear integer arithmetic.** `+`, binary `-`, unary `-`, and
multiplication where **at least one factor is an integer literal**. Comparisons
`== != < <= > >=` over `Int`. Nothing else is arithmetic:

- `x * y` with both symbolic is **not** in the fragment. The term is
  uninterpreted and participates only in congruence closure.
- `/` and `%` are **not** in the fragment as *values*, at all, including by a
  literal. Division is expressible in Presburger arithmetic and implementing it
  correctly is real work that is easy to get subtly wrong; `x / 2 * 2 == x`
  reported `proved` is exactly the defect this milestone must not ship. An
  uninterpreted `/` costs the fragment `x / 1 == x` and buys the guarantee that
  a wrong division rule cannot exist. Their *definedness* is decided, by (g).
- Overflow: the prover's terms are mathematical integers, and (g) is what makes
  that a statement about Ply rather than about ℤ.

**(b) Propositional structure.** `&&`, `||`, `!`, and `if c { a } else { b }` at
`Bool`. Handled by case-splitting over the atoms — DPLL without learning, since
the formulas are tiny.

**(c) Case analysis over ADTs.** A `match` splits on the scrutinee's outermost
constructor. Within each arm the constructor's fields become fresh symbolic
constants of their declared types. This is **exhaustive and terminating for every
ADT, recursive or not**, because every value of a sum type has exactly one
outermost constructor — the split is over the constructor set, not over the value
space. A recursive type is therefore split to depth 1 and its fields stay opaque,
which is precisely the boundary where induction would be needed and is not
available.

A type is **finite** iff its constructor graph is acyclic and every field type is
finite. `Bool` and a nullary enum are finite; `Int`, `String`, `List<_>` and
anything in a constructor cycle are not. Finiteness is used only by (f).

**(d) Structural equality and congruence closure.** Equality is over terms, in an
E-graph, with:

- constructors **injective** (`C(x̄) == C(ȳ) ⟺ x̄ == ȳ`) and **distinct**
  (`C(..) != D(..)` for `C ≠ D`);
- record projection reducing over a record literal, and records equal iff every
  field is;
- every other application — a user function, an unfoldable call that was not
  unfolded, `*` over two symbolics, `/`, `%` — an **uninterpreted function
  symbol**, closed under congruence: `x == y ⟹ f(x) == f(y)`.

Treating a Ply function value as an uninterpreted symbol is sound in the
direction that matters: an equality that holds for an arbitrary `f` holds for
every actual `f`. It is what makes `forall (f: (Int) -> Int, x: Int) { f(x) == f(x) }`
a proof and `forall (f, xs) { map(map(xs, f), g) == .. }` not one.

**(e) Bounded unfolding of non-recursive definitions.** A call to a user
definition that is **not** a member of a recursive SCC may be inlined, its body
substituted for the call, to a depth of at most `UNFOLD_DEPTH = 3`. A call to any
member of a recursive SCC is **never** unfolded and stays uninterpreted.

This is the rule that decides the milestone's reach, and it is drawn where the
mathematics draws it: unfolding a recursive definition needs induction to
terminate at a general statement, M8 has no induction, so anything whose truth
depends on a recursive definition's behaviour over unbounded data falls to
`property`. `reverse(reverse(xs)) == xs` is `property`. It should be, and no
amount of clever unfolding makes it otherwise.

The SCC data is already on hand: `ply-hash` computes it for component hashing
(DESIGN.md §3, Tarjan), so "is this definition recursive" is a lookup.

**(f) Exhaustive enumeration of a finite domain.** When every binder's type is
finite and the product of their cardinalities is at most
`ENUMERATION_BOUND = 4096`, the obligation may be decided by evaluating the body
at every point of the domain. Every point held ⟹ `proved`, by
`Rule::ExhaustiveEnumeration`. Any point failed ⟹ `Refuted`, and the failing
point is the counterexample with no shrinking needed, since it is already a
member of a domain enumerated in a fixed order.

This is a genuine proof — the domain was covered — and it is why
`law forall (b: Bool) { b || !b }` is `proved` for two evaluations rather than
sampled for two hundred.

**(g) Definedness, which every other rule is conditional on.**

> A proof is issued only once the prover has also decided that **every input
> satisfying the guard has an answer**.

(a)–(f) reason over mathematical integers and over total function symbols. Ply
evaluates over `i64` with checked arithmetic, raises on a zero divisor, and has
no termination checker. Where the two disagree there is no result for a
postcondition to be true of, so an obligation over such an input is not one a
tier claiming "every input satisfying the guard" may cover. `x + 1 > x` is valid
over ℤ and **raises** at `i64::MAX`; `spin(x) == spin(x)` is valid for a total
symbol and never returns for `fn spin(x) = spin(x)`. Neither is proved.

So lowering records a **requirement** per construct, discharged in the same
decision as the goal and under the same guard:

| construct | requirement |
| --- | --- |
| `+`, binary `-`, unary `-`, `*` by a literal | the mathematical result is in `[i64::MIN, i64::MAX]` |
| `+`, `-`, `*` whose coefficients left the prover's own range | unsatisfiable: the result is not a value |
| `/`, `%` | the divisor is not `0`, and the pair is not `(i64::MIN, -1)` |
| a call this prover did not inline | unsatisfiable, unless the callee is a constructor, a quantified function binder, or one of the four total prelude functions |
| everything else | none |

Three properties of that table carry it:

- **Every `Int` is an `i64`,** which is a theorem and not an assumption. The
  requirement proof may assume `MIN ≤ x ≤ MAX` of every term that denotes a Ply
  `Int` — which is what puts `x + 1` under `x < 100`, and `-x` under `x >= 0`,
  back inside the fragment. It may **not** assume it of `result`, nor of
  anything built from `result`: `result` is a value only if the definition
  returned one, so assuming its width beside `result == <body>` would let a goal
  prove its own definedness.
- **A requirement is conditional on the path it was reached under.** The `else`
  arm of an `if` and the right operand of `&&` owe nothing where they do not
  run, which is why `if x < 0 { 0 - x } else { x }` is decided under `x > -1000`
  rather than refused outright.
- **A guard's own requirements may not assume the guard.** A guard is evaluated
  to decide whether it holds; one that raises has no domain to speak of, and the
  obligation is `Unknown` rather than `Vacuous`.

The cost is real and is the point: an unbounded numeric law falls to the
property tier, where the generator's `i64::MIN` and `i64::MAX` draws report
`Unattempted { Raised }` — the honest answer, and the one the reader can act on
by writing the bound. `examples/bank.ply`'s `adjusted` carries exactly that
bound, for exactly that reason.

#### 5.2 What is inconclusive, and what that means

Anything the fragment does not decide is **inconclusive**, and:

> **An inconclusive proof attempt reports `property`. Never `proved`.**

Inconclusive covers: a term outside the fragment; a case split whose branches
did not all close; a step budget spent (`--prove-budget`, default 10,000
inference steps, charged per obligation); a needed unfolding refused because the
callee is recursive.

**And inconclusive never reports `Refuted` either.** If the prover finds the
negation *satisfiable*, it has a model over uninterpreted symbols, and such a
model need not correspond to any actual Ply value — an uninterpreted `f` in the
model can be a function no closure computes. Reporting that as a counterexample
would be a confidently wrong red, which is the failure mode symmetric to a
confidently wrong `proved`. The static side of this milestone is **refutation-
incomplete on purpose**: it either proves or shrugs, and the property tier does
the refuting, with a value it actually ran.

#### 5.3 Ground evaluation is a proof

A closed obligation — no binders, or all binders enumerated by (f) — whose body
is a pure Boolean term evaluates to exactly one value. Evaluating it to `true` is
a decision procedure for it, and the tier is `Proved` by
`Rule::GroundEvaluation`.

This is not a loophole, it is the degenerate case of (f) with a domain of size
one, and stating it explicitly stops an implementer from reporting `example` for
the strongest possible evidence. The bound is the ordinary one:
`DEFAULT_MAX_CALLS`. An evaluation that raises or exceeds it is not a proof and
not a refutation — it is `Unattempted { Raised }` per §5.5.

#### 5.4 What `property` and `example` mean, and the number that separates them

A property run:

1. draws `plan.cases` candidate binder tuples (default 200) per root;
2. **rejects** those failing the guard;
3. evaluates the body at each **kept** tuple.

Then:

| kept | outcome |
| --- | --- |
| `kept >= MIN_PROPERTY_CASES` (25) and all held | `Held` at `Property` |
| `0 < kept < 25` and all held | `Held` at `Example` |
| `kept == 0` | `Vacuous` |
| any kept tuple failed | `Refuted`, shrunk per §8.2 |

`example` is therefore not a thing a user asks for. It is what the system
honestly reports when the guard was tight enough that a coverage claim would be
a lie — `where n > 0 && n <= a.balance` over randomly drawn accounts rejects most
of what it is handed, and being told `property, 200 cases` when seven cases ran
is exactly the misreport this milestone exists to avoid.

`MIN_PROPERTY_CASES` is a constant and not a fraction of `plan.cases`, so a run
at `--prove-cases 5` can only ever produce `example`. That is correct: you asked
for fewer cases than a property claim needs.

#### 5.5 The outcomes that are not tiers

```rust
pub enum Discharge {
    Held(Evidence),
    Refuted(Counterexample),
    Vacuous(Vacuity),
    Unattempted(Gap),
}
```

**`Vacuous`** — the guard admitted nothing, so the obligation is trivially valid
and says nothing. `guard ⟹ body` with an unsatisfiable guard is valid, and a
system that reported it `proved` would turn a typo in a guard into a proof of
everything. Two ways it fires, both strong evidence:

- the prover *proved* the guard unsatisfiable within the fragment;
- a property run kept **zero** of a full case budget **and a directed search for
  a value the guard admits also found none**.

It is `E0420`, an error, and `ply prove` exits 1 on it. A vacuous obligation is
always a defect in the spec — which is why the second bullet needs its second
half. The generator draws from the whole of a type, so a guard admitting nine
integers a million away from zero is one two hundred draws will never satisfy;
that is a fact about the *search*, and reporting "the guard admits no value" for
it states something false about the program and fails the build on it. So before
the error is raised the guard is evaluated at the points its **own literals**
name, and a value found there rebuts the vacuity with something that actually
ran. The same value vouches for the domain of a static argument the prover had
already completed, which is the one place a witness upgrades a proof rather than
replacing it.

**`Unattempted`** — the system could not decide, and says so rather than
labelling it. Four reasons:

- `Gap::UnhandledEffect(Footprint)` — checking an `ensures` means *calling* the
  definition, and a definition performing `db.write[accounts]` needs a handler
  that nothing supplies. See §7.3.
- `Gap::Ungeneratable { param, ty }` — a parameter of a type the generator cannot
  inhabit (`Cell<_>`, `Task<_>`, a function type with a non-empty row). This is a
  gap rather than the compile error a *law* binder gets, because forbidding it
  would forbid attaching a spec to a higher-order definition.
- `Gap::Raised { bindings, diagnostic }` — evaluating a case raised: a runtime
  error, a division by zero, the recursion limit. A spec that raises is not
  false, so this is not a refutation; the raising input is shrunk with "still
  raises" as the predicate, because a minimal raising input is worth exactly what
  a minimal falsifying one is.
- `Gap::GuardNotSampled { generated, witness }` — the guard kept none of a full
  budget and does admit a value, which is carried. The search missed the domain;
  the spec is not at fault, so this is `W0604` and exit 0 rather than the `E0420`
  above.

An `Unattempted` obligation is `W0604`, is counted in the summary, and does
**not** fail the run. It also does not count toward coverage — a definition whose
only obligation is undischargeable is a definition a reviewer still has to read,
and §9.3 is emphatic that the count must say so.

#### 5.6 A tier is never upgraded on a guess, structurally

There is no `tier` field. `Discharge::tier()` is a function of the evidence:

```rust
pub enum Evidence {
    Proof(Certificate),
    Cases(CaseReport),
}

impl Evidence {
    pub fn tier(&self) -> Tier {
        match self {
            Evidence::Proof(_) => Tier::Proved,
            Evidence::Cases(c) if c.kept >= MIN_PROPERTY_CASES => Tier::Property,
            Evidence::Cases(_) => Tier::Example,
        }
    }
}
```

A component that wants to report `proved` must hand over a `Certificate`, whose
`rules: Vec<Rule>` names every inference rule used and whose `steps` says how
much work it took. `Rule` is a closed enum containing exactly §5.1's rules and
§6's, so an audit test can assert that every certificate produced over the corpus
mentions only fragment rules — a prover that grew a rule nobody sanctioned is
caught by a `match` that stops compiling.

`Certificate::guard_satisfiable` is a required field, not an optional one: a
certificate that did not establish the guard has a domain it cannot vouch for, so
constructing one with `guard_satisfiable: false` and reporting `Held` is the
`Vacuous` path, and the audit asserts every `Held(Proof(c))` has
`c.guard_satisfiable`.

This is the single most important structural decision in the milestone. Every
other honesty rule here is a discipline; this one is a type.

---

### 6. Concurrency laws

A law whose body's row is `{sim.read}` is discharged by **execution**, and it is
the one place in M8 where a `proved` does not come from a static argument.

```ply
law "transfers conserve value"
  forall (n: Int) where n > 0 && n <= 100 {
    simulate {
      let a = task.spawn(|| transfer(alice, bob, n));
      let b = task.spawn(|| transfer(bob, alice, n));
      task.join(a); task.join(b);
      balance(alice) + balance(bob) == 200
    }
  }
```

The condition, exactly:

> A concurrency law discharges as `proved` **iff every one of the following
> holds**:
>
> 1. `plan.sim.mode == SimMode::Dpor` — a sampled or single-interleaving run has
>    no exhaustiveness to claim;
> 2. `Exploration::exhaustive` is `true` — the frontier emptied, so every
>    interleaving at scheduler-visible granularity ran (ADR 0006 §6.4);
> 3. `Exploration::exhausted` is `false` — the budget was not spent;
> 4. `Exploration::failure` is `None`;
> 5. **and the value domain was covered too**: the law has no binders, or every
>    binder's type is finite and §5.1(f)'s enumeration ran over all of them.
>
> **Otherwise `property`**, whatever the exploration said.

Condition 5 is the one an implementer will drop, and dropping it is the
milestone's worst available defect. An exhaustive interleaving search over
*sampled values* proves something about those values and nothing about the law:
`exhaustive: true` is a claim about schedules, and a law over `n: Int` ranges
over 2⁶⁴ of them. The two coverage claims are independent and `proved` needs
both.

The certificate for this path is `Rule::ExhaustiveInterleaving { interleavings }`,
deliberately a distinct rule so that the audit test in §11 can find every
execution-derived proof in a corpus and check it against conditions 1–5.
`Certificate::sorts` is empty for it — this is a proof about one program, not
about an uninterpreted sort.

**Caching.** §4.3's rule applies unchanged and is sound here for a reason worth
stating: `exhaustive` means the frontier emptied, so raising the budget cannot
reach an interleaving the search did not, so the claim is plan-independent and
belongs under the bare key. A `property` concurrency law is keyed by
`prove_key`, whose digest includes the sim plan — so widening `--sim-budget`
re-runs it, exactly as it re-runs a seeded test.

**Everything else about the region is ADR 0006's.** The law body's `simulate`
is an ordinary region: it is machine-only (`E0504` under `--engine treewalk`),
it may not nest (`E0416`), a `Task` may not escape it (`E0413`), a stuck region
is `E0414`, and a replay whose enabled set does not match is `E0415`. M8 adds no
scheduling machinery and no new interleaving semantics — it adds one reading of a
field ADR 0006 already computes.

---

### 7. Frame conditions come from footprints

#### 7.1 Ply does not need a `modifies` clause

The classic tarpit of program verification is the frame problem: an `ensures`
says what changed, and a caller also needs to know what *didn't*, so Dafny and
Why3 make the user write a `modifies` clause and then prove it. Writing it is
tedious, getting it wrong is easy, and it is the single largest source of
"specification that is longer than the code".

Ply already infers it. A definition's **footprint** is a closed row of
`(effect, resource, mode)` atoms — exactly what it can touch, at resource
granularity — and it has been computed for every definition since M2, checked as
an upper bound against any annotation, and used to schedule tests. The frame
condition is its complement:

```rust
pub enum Frame {
    /// The footprint is empty: the result is a function of the arguments, and
    /// nothing else can have changed.
    Pure,
    /// Every resource outside this set is unchanged.
    Writes(BTreeSet<(Symbol, Resource)>),
}

pub fn frame_of(footprint: &Footprint) -> Frame;
```

`Frame::Writes` holds the `(effect, resource)` pairs of the footprint's **write**
atoms only. A read changes nothing, so it does not narrow a frame.

An `ensures P` on a definition `f` therefore means, in full:

> `P` holds of `(arguments, result)`, **and** every resource outside
> `frame(footprint(f))` has the same contents after the call as before it.

#### 7.2 The second half is already proved, by the type system

The frame half of that sentence is **not** an obligation. It is not something the
prover establishes, not something a property run samples, and not something a
user writes. It is a consequence of the effect system's soundness, established
once for every definition in the language before any obligation exists: a
definition cannot perform an atom outside its row, an annotation is an upper
bound that inference must fit inside, and a handler's own effects are unioned
into the `handle` rule's result so a handler backed by a socket reports network
access.

So `Obligation::frame` is carried, printed, and never checked — it is evidence
already in hand. That is what the resource-granular effect system has been paying
for since M2, and this is the milestone where the bill comes back.

#### 7.3 `old()` does not exist, and does not need to

Ply is a value language. `withdraw(acct, amount) -> Account` returns a new
`Account`; the pre-state is `acct`, which is still in scope, still bound, and
still exactly what it was. There is nothing for an `old()` operator to recover.

Where state *is* mutable it lives in an effect, and an effect is exactly what a
spec is forbidden to perform (§2) — so no spec can name mutable state at all,
and no spec needs to say when it read it. This is the same restriction as
purity, seen from the other side, and it is why the frame is reportable rather
than provable.

The cost is stated plainly: **M8 can specify what a definition computes and
cannot specify what it does to the world.** `ensures` over an effectful
definition is a claim about its return value under an inferred frame; it cannot
say "the balance in `db[accounts]` decreased". Saying that needs a way to name a
resource's contents in a pure expression — a `state(db, accounts)` term with a
model of the resource behind it — and that is a milestone of its own. This is
the largest gap in M8 and §12 names it as such.

#### 7.4 Where the frame bites: `Gap::UnhandledEffect`

Checking an `ensures` at the property tier means calling the definition. A
definition whose footprint is non-empty needs handlers for those atoms, and
`ply prove` has none: there is no `test` around the obligation, no `handle` in
scope, and inventing one would be inventing a behaviour and then testing against
it.

The rule:

- footprint **empty** → checkable. `Frame::Pure`, and the obligation is a total
  specification of the definition.
- footprint discharged entirely by `simulate` — only reachable inside a law body
  — → checkable.
- otherwise → `Unattempted { UnhandledEffect(footprint) }`, reported, counted,
  and **not** covered.

The static prover still attempts such an obligation first: a proof needs to run
nothing. It will almost always be inconclusive, because a body containing a
`perform` or a `handle` cannot be unfolded and stays uninterpreted, so the
practical answer is the gap. Attempting anyway costs a bounded number of steps
and occasionally proves something true of any implementation.

The follow-on is syntax that supplies handlers to an obligation — the natural
form being a `with` block on a law, reusing `handle`'s clause syntax verbatim so
that the test double and the production resource still cannot drift. It is
deferred because it is a design about *where* those handlers come from and how
they are hashed, and because the pure case is where specs pay first.

---

### 8. Generation and shrinking

#### 8.1 Generating a value

Deterministic, from counter-mode BLAKE3, exactly ADR 0006 §4.2's device with a
new domain:

```
draw(root, obligation_key, counter) =
    blake3( b"ply.gen.stream.1" ‖ root_le_u64 ‖ obligation_key ‖ counter_le_u64 )[0..8]
```

**The stream is keyed by the obligation, not only by the root.** That is the
same argument ADR 0006 makes for separating the `sched` and `rand` streams:
without it, adding a law would shift every later law's cases, so an unrelated
edit would change which counterexample a failing obligation reports and a
bisection over it would name the wrong definition.

| type | generated |
| --- | --- |
| `Unit` | `()` |
| `Bool` | one bit |
| `Int` | with edge bias: `0`, `1`, `-1`, `i64::MIN`, `i64::MAX` drawn with fixed probability, then a value whose magnitude is drawn from a size parameter growing with the case index |
| `String` | length 0..16 biased small, from a 16-character alphabet |
| `List<T>` | length 0..16 biased small, elements independently |
| record | each field, in the type's field order, which is sorted |
| ADT | a constructor by index; at depth ≥ `GEN_DEPTH` (4) only constructors with no recursive field are drawn, so generation terminates for every recursive type |
| `(ā) -> b` | a member of a fixed **function family**: the constants of `b`, plus `\x -> h(x)` where `h` derives its answer from the argument's rendering and the draw. Every member is pure, total, extensionally deterministic, and has a printable description, so a counterexample naming a function names something a reader can act on |
| type variable | monomorphised to `Int`, recorded in `CaseReport::instantiations` |
| `Cell<_>`, `Task<_>` | not generatable — `E0418` for a law binder, `Gap::Ungeneratable` for a parameter |

`i64::MIN` and `i64::MAX` being drawn on every run is what makes the boundary a
*reported* gap rather than a silent one: an obligation the proved tier declined
under §5.1(g) lands here, and the first thing this tier does is evaluate it at
the width of the type. It is no longer standing in for a soundness argument —
§5.1(g) is that — and the reason it cannot is worth stating, because the earlier
draft of this ADR relied on it and was wrong twice over: `Prover::discharge_with`
answers from the static tier before any case is drawn, so a proof is never
sampled by an ordinary run; and Ply's arithmetic is checked, so the divergence
surfaces as `Gap::Raised` rather than as a refutation. A mitigation that fires in
neither place is not one.

#### 8.2 Shrinking a counterexample

M5 shrinks the definition *set* to find a culprit (ADR 0004 §4). M8 shrinks the
*inputs* to find a minimal counterexample. ADR 0004 said explicitly that nobody
should build a value shrinker speculatively because there was nothing to shrink;
there is now.

Candidate order per type, fixed so that two runs agree byte for byte:

| type | candidates, in order |
| --- | --- |
| `Int` | `0`, `n/2` repeatedly toward 0, `n - sign(n)`, and `-n` when `n < 0` |
| `Bool` | `false` |
| `Unit` | none |
| `String` | `""`, the first and second halves, then each character lowered toward `'a'` left to right |
| `List<T>` | `[]`, the first and second halves, then each single element removed left to right, then each element shrunk in place left to right |
| record | each field shrunk in place, in field order |
| ADT | each field whose type equals the value's own type (the recursive positions), then a constructor of lower `CtorInfo::index` whose fields can be filled from this one's, then each field shrunk in place |
| function | toward the constant function returning the smallest value of the return type |

The two requirements, and they are the whole of the honesty here:

1. **A shrunk value must still falsify the obligation.** Every candidate is
   re-evaluated and only an actually-falsifying one is accepted. The shrinker
   assumes no monotonicity of any kind.
2. **A shrunk value must still satisfy the guard.** A candidate outside the
   guard's domain is not a smaller counterexample, it is a counterexample to a
   different claim, and accepting one would report an input the law never spoke
   about. Rejected before it is even evaluated.

**Termination is structural, not budgetary.** Every type has a `size(v) -> u64`
(saturating), a candidate is accepted only if its size is **strictly smaller**,
and the walk is greedy — the first accepted candidate at each step becomes the
new value. So the process terminates whatever the budget is, and
`--shrink-budget` (default 500 *candidate evaluations*, never seconds — ADR 0004's
rule) bounds wall clock rather than correctness.

**The result is deterministic**, being a function of the original value, the
obligation and the budget. Two runs over the same failure produce byte-identical
minimal counterexamples, which is what makes today's artifact diffable against
yesterday's. Required test.

**The original is kept.** `Counterexample::original` alongside `bindings` and
`shrinks`, because "shrank from a list of 400 elements to `[0, 1]` in 11 steps"
is the sentence that tells a reader the space was searched, and a minimal value
alone does not.

**Schedule shrinking is not in M8**, for ADR 0006 §6.5's reason: truncating a
choice path changes what the suffix means. A concurrency law's counterexample
carries the `Race` pair and the seed, which is the actionable half and is exact.

---

### 9. Output

#### 9.1 `ply prove`

```
$ ply prove
   41 definitions · 18 carry an obligation · 23 do not
   26 obligations · 7 proved · 16 property · 2 example · 1 unattempted   (0.42s)

   ✓ proved     ledger.withdraw           ensures #0    linear arithmetic · 2 unfoldings · 41 steps
   ✓ proved     ledger.withdraw           ensures #1    congruence · injectivity · 12 steps
   ✓ proved     "credit and debit cancel"               case analysis over ledger.Account · congruence
   ✓ proved     "transfers conserve value"              exhaustive over 12 interleavings
   ✓ property   ledger.fee                ensures #0    200 cases · 0 rejected
   ✓ example    ledger.settle             ensures #0    7 cases kept of 200 · guard rejected 193
   ~ unattempted ledger.post              ensures #0    performs {db.write[accounts]}: no handler

   ✗ refuted    "reverse is an involution"                                   src/list.ply:41:1
       forall (xs: List<Int>)  →  xs = [0, 1]
       shrank from [4, 9, 2, 7, 1] in 6 steps · root 41 · case 118
       frame: pure

   1 refuted, 25 held (0.42s)
```

The coverage line is **first** and is not behind a flag; §9.3 is why.

`--json` emits one object with `"schema_version": 1`, carrying every obligation,
its key, its owner, its span, its frame, its discharge, the certificate or the
case report, the counterexample if any, the plan, and the coverage block.

Flags, mirroring `ply test` where the meaning is the same: `--json`, `--explain`,
`--filter <substring>`, `--jobs <n>`, `--no-cache`, and
`--prove-cases <n>` / `--prove-budget <n>` / `--shrink-budget <n>` /
`--prove-roots <n>`. The simulation flags `--sim`, `--sim-budget` and `--seed`
apply to concurrency laws unchanged.

Exit `0` when every obligation is `Held` or `Unattempted`; `1` on any `Refuted`
or `Vacuous`; `2` on a compile error. An `Unattempted` obligation is a reported
gap, not a failure — making it one would mean a spec could never be attached to
an effectful definition.

**`ply prove` never calls `observe_definitions`,** for ADR 0004 §4's reason: a
definition exercised by an obligation has not been vindicated as a *test*
subject, and marking it seen would empty the next `ply test`'s suspect set.

#### 9.2 `ply review --changed`

```
$ ply review --changed
   6 definitions changed since the last accepted review

   ledger.withdraw          implementation changed · spec unchanged
     ensures #0   proved     ✓ still holds
     ensures #1   property   ✓ still holds        200 cases
     → review the obligations, not the diff

   ledger.fee               implementation unchanged · spec changed
     ensures #0   proved     ✓ holds              (was: property)
     → review the spec diff; the implementation did not move

   ledger.settle            implementation changed · spec changed
     ensures #0   example    ✓ holds              7 cases kept of 200
     → review both; 7 concrete cases is what the machine checked

   ledger.post              implementation changed · no spec
     → read the implementation

   ledger.audit_row         implementation changed · no spec
   ledger.format_row        implementation changed · no spec

   2 of 6 changed definitions carry no obligation
```

The table those four shapes come from is the milestone's whole argument:

A row is only reached when the definition carries at least one obligation that
**holds**. An obligation the machine could not discharge is not a claim it
established, so a definition whose only obligation is a gap falls to the last
row — §5.5 already says a gap "does not count toward coverage", and the advice
has to agree with the count or one of them is lying. `unspecified_changed` is
the number that discloses the blind spot, and it counts that definition.

| implementation | spec | what a reviewer does |
| --- | --- | --- |
| changed | unchanged | read the obligations. The claim is fixed and still holds; the diff is an implementation detail. **This is the cheapest review in the system.** |
| unchanged | changed | read the spec diff, and nothing else. The implementation did not move. |
| changed | changed | read both — and the tier says how much the machine already checked. |
| unchanged | unchanged | nothing to review. |
| either | **none** | read the implementation, line by line, exactly as today. |

**The baseline is what a human last accepted, not what a machine last ran.**
`ply review --accept` writes a `ReviewRecord` per definition — its `DefHash` and
its spec hashes — keyed by the definition's **program-wide name**. Keying by name
is deliberate and is the same trade ADR 0004 makes for `PassRecord`: the whole
point is to survive an edit that moves the hash, so the key has to be the thing
that does not move. Renaming a definition therefore loses its review baseline and
it shows up as newly-unreviewed, which costs one re-read and never a false
"unchanged".

Records live in `reviews.json`, read lazily on the first question, for ADR 0003's
reason.

#### 9.3 Coverage is in the default output, never behind a flag

```rust
pub struct Coverage {
    pub definitions: usize,
    /// Carries an `ensures`, or is named directly by a law that holds.
    pub covered: usize,
    /// Program-wide names, sorted. The surface where review still costs what it
    /// costs today.
    pub uncovered: Vec<Symbol>,
    pub by_tier: BTreeMap<Tier, usize>,
}
```

A definition is **covered** iff it carries at least one `ensures` clause whose
obligation is `Held`, or it is **directly named** by a law whose obligation is
`Held`. Three deliberate choices in that sentence:

- **`requires` alone does not cover.** A precondition restricts a domain; it
  makes no claim about behaviour, so a definition carrying only preconditions is
  a definition a reviewer must still read.
- **A refuted, vacuous or unattempted obligation covers nothing.** A claim the
  machine could not or did not establish is not evidence, and counting it would
  make the number go up exactly when the system got less trustworthy.
- **Directly named, not transitively reachable.** A law over `credited` and
  `debited` covers those two. Taking the transitive closure would let one law
  over one hub definition claim the whole program, which is the shape every
  coverage metric fails in.

> **The count of definitions carrying no obligation is exactly the surface where
> review still costs what it costs today.**

Which is why it is printed on every run, in both commands, ahead of the results,
and why `uncovered` is a **list of names** and not only a number: a number is
something to feel bad about and a list is something to work through. Hiding it
behind `--coverage` would turn an honest tool into a marketing one — a project
with three proved obligations and four hundred unspecified definitions would
print three green ticks and nothing else, which is a *worse* artifact than the
one M7 shipped, because it invites a reviewer to stop.

---

### 10. Diagnostics

Five new codes. `ply_span::codes` is append-only and existing numbers do not
move.

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0417 | `EFFECT_IN_SPEC` | a `requires` / `ensures` / `where` row is not empty; a law body's row is not a subset of `{sim.read}` | the program's |
| E0418 | `UNQUANTIFIABLE_TYPE` | a `forall` binder's type cannot be quantified over: no generator (`Cell`, `Task`), a function type with a non-empty row, or an effect-row variable | the program's |
| E0419 | `OBLIGATION_REFUTED` | a counterexample was found | the program's |
| E0420 | `VACUOUS_OBLIGATION` | the guard admitted no values | the program's |
| W0604 | `OBLIGATION_NOT_DISCHARGED` | `Unattempted`, with the `Gap` in the note | nobody's; it is a gap |

Reuse rather than invent, in three places where a new code would have been the
lazy answer:

- a clause that is not `Bool` is `TYPE_MISMATCH` (E0201);
- `result` in a `requires` is `UNKNOWN_NAME` (E0101) with a note;
- a parameter named `result` beside an `ensures` is `DUPLICATE_DEFINITION`
  (E0105) pointing at both;
- two laws with one label in one module are `DUPLICATE_DEFINITION` (E0105).

E0419 and E0420 join ADR 0005's `E0501`/`E0502` row: the program is at fault,
`Failure::defect` is `false`. E0419's message leads with the shrunk bindings and
its notes carry the original and the shrink count, because the input is the
answer and the search is the evidence.

---

### 11. Validating it

The property being validated is that **a tier label is true**, and the way that
property breaks is never loudly.

- **The certificate audit.** Every `Held(Proof(c))` produced over `examples/`,
  `tests/fixtures/` and the generated corpus is checked: every `Rule` in
  `c.rules` is a fragment rule, `c.guard_satisfiable` is true, and `c.steps` is
  within the budget. `Rule` being a closed enum means a prover that grew a rule
  nobody sanctioned fails to compile before it fails an audit.
- **The differential tier audit**, which is the one that would catch a lying
  prover. For every obligation the corpus reports `proved`, run it at the
  property tier as well, at 1,000 cases across 8 roots. A `proved` obligation
  that a sampled run **refutes or raises at** is a **defect in Ply**, reported as
  such (`Status::Panicked`, not bisected, ADR 0006 §9's row for E0415), and it
  fails the audit loudly. This is the direct analogue of `--engine both`, and it
  exists for the same reason: a claim that two mechanisms agree is only worth
  what the comparison costs.

  **The raise half is not optional.** A proof claims that every input satisfying
  the guard has an answer *and* that the answer is `true`; a refutation denies
  the second and a raise denies the first. Ply's arithmetic is `checked_*` and
  its recursion is bounded, so an obligation the prover got wrong about totality
  can only ever come back as `Gap::Raised` — an audit looking for
  `Discharge::Refuted` alone cannot fail on the defect it exists for.
- **The unfolding-boundary test.** A recursive definition is never unfolded:
  `reverse(reverse(xs)) == xs` reports `property` and not `proved`, and the
  certificate for a nearby non-recursive law names `Unfold` with a depth at most
  3.
- **The concurrency-law condition test.** A concurrency law over a binder is
  `property` even when the exploration reports `exhaustive: true` — §6's
  condition 5, which is the one an implementer will drop.
- **Determinism.** Two runs of `ply prove --json` over one project are
  byte-identical, including the order of obligations, the shrunk counterexample,
  and the certificate's rule list. `--jobs 1` and `--jobs 16` agree byte for
  byte.
- **The cache-honesty test.** A `property` discharge is never present under a
  bare obligation key in `obligations.json`; a `proved` discharge is read under a
  wider plan without re-running; widening `--prove-cases` re-runs every sampled
  obligation and none of the proved ones.
- **The invariant test.** Adding, editing and deleting a spec each select **zero**
  tests and change **zero** definition hashes.

`PROVER_VERSION` starts at `0.1.0`. `FRONTEND_VERSION` and `BODY_ENCODING` bump
because a law enters the AST and the normalizer; `RUNTIME_VERSION` does **not**,
because the evaluator gains no semantics — a spec is evaluated by the same
machine running the same programs.

---

### 12. What M8 is not

Named here so nobody builds one speculatively.

- **A general-purpose theorem prover.** §5.1 is the entire fragment and it is
  deliberately small. The reach of the prover matters far less than the honesty
  of the label, and every extension has to pay for itself against the risk of a
  wrong `proved`.
- **An SMT integration.** No Z3, no CVC5, no external solver, ever. A solver is a
  trusted oracle whose *version* changes the answer, and a `proved` label must be
  reproducible from the definition set alone — the same argument that put
  counter-mode BLAKE3 in ADR 0006 §4.2 instead of a PRNG crate. A proof that
  depends on which binary was on the path is not a proof this project can cache.
- **A termination checker.** Nothing here decides whether a definition
  terminates, which is why §5.1(g) refuses rather than assumes: a call this
  prover did not inline carries a definedness requirement nothing discharges, and
  a member of a recursive component is never inlined. `DEFAULT_MAX_CALLS` bounds
  an evaluation and an obligation that hits it is `Unattempted { Raised }`, never
  `proved` and never `refuted`.
- **Induction.** No structural induction, no well-founded recursion, no
  user-supplied lemmas or hints. This is what puts every claim about a recursive
  definition over unbounded data at `property`, and it is the single largest
  restriction on reach.
- **Quantifier alternation.** There is no `exists`. Every obligation is a
  universally quantified implication, which is what makes a counterexample a
  witness and a proof a decision.
- **Call-site precondition checking.** `requires` is a **filter on the domain of
  the `ensures` clauses beside it**, not a contract checked at every call. A
  caller that violates a precondition is not diagnosed. Checking it needs a
  path-sensitive analysis of every caller and a story for what happens when the
  condition is not decidable there; it is the natural next milestone and it is
  not this one. **A reader of a Ply spec must not read `requires` as "the
  compiler enforces this".**
- **Specifying effects.** §7.3: an `ensures` cannot say what a definition did to
  a resource, because a spec may not name mutable state. **This is the largest
  gap in the milestone.** Closing it needs a pure term denoting a resource's
  contents and a model of the resource behind it.
- **Handler-parametric laws.** §3.2, with the four things it would take.
- **A bit-vector semantics for `Int`.** §5.1(g) makes the ℤ reasoning honest by
  refusing to certify an obligation whose arithmetic can leave `i64`; it does not
  make the fragment *decide* what happens there. A law that is true precisely
  because of how `i64` behaves at the boundary is outside it, and reaching one
  needs a bit-vector decision procedure of its own.
- **Runtime contract checking.** A `requires` is not evaluated when a program
  runs, in any mode. Making it so would put a spec into a definition's *behaviour*
  and therefore into the meaning of its hash, and would tax the green path that
  the whole language exists to make fast.
- **Spec-derived code, spec-derived tests as source, refinement types, dependent
  types.** None of them.

## Consequences

- **No headline invariant moves.** Renaming a function still selects zero tests;
  moving a definition still changes no hash; incremental and `--no-incremental`
  still agree byte for byte; a `nondet` atom in a `det` test is still E0412;
  bisection still names the culprit; `--engine both` still reports no divergence;
  a seeded simulation still replays exactly; `Store::open` is untouched by a
  cache that is read lazily on its first question. M8 adds a claim about a
  definition, not a change to what a definition is.
- **A new headline invariant joins them:** *writing a spec re-runs no test*. It
  is the same sentence as "renaming a function re-runs no test" and it is true
  for the same reason.
- **`ply-prove` joins the workspace.** It depends on `ply-span`, `ply-syntax`,
  `ply-core`, `ply-hash` and `ply-eval`, and nothing depends on it but `ply-cli`.
  It does not depend on `ply-test`: obligations are not tests, they have their
  own selection rule, their own cache namespace and no conflict graph.
- **`ply-store` gains two lazily-read files** — `obligations.json` and
  `reviews.json` — and one new version constant, `PROVER_VERSION`. Neither is
  read at `Store::open`, which is what keeps the 5 ms budget intact.
- **`SourceFingerprint` gains `specs`,** so a gate-1 skip still contributes its
  obligations. This is a `FRONTEND_VERSION` bump.
- **A green `ply test` and a green `ply prove` are different claims,** and the
  summary of neither implies the other. A project with 47 passing tests and 23
  uncovered definitions is told both numbers, on every run.
- **The coverage number can go down when nothing got worse.** Adding a
  definition without a spec lowers it, which is correct and is the point: the
  metric measures the review surface, and a new unspecified definition is new
  review surface.

## Required tests

Syntax and typing:

1. `requires` and `ensures` parse in any order and any number, before both the
   `= expr` and the block body forms.
2. A local, a parameter or a function named `requires`, `ensures`, `law`,
   `forall`, `where` or `result` still parses — every one is contextual.
3. `ensures p(x) { .. }` parses as a clause plus a block body, not a record
   literal.
4. `result` in a `requires` is `UNKNOWN_NAME` with a note naming `ensures`.
5. A parameter named `result` on a definition with an `ensures` is
   `DUPLICATE_DEFINITION`; the same definition without an `ensures` compiles,
   which `examples/timeout.ply` already relies on.
6. A spec expression performing any effect is `E0417`, naming the atom and the
   performing expression.
7. A spec expression whose row is an unsolved tail (`{| e}`) is `E0417`.
8. A `law` binder of type `Cell<Int>`, of a function type with a non-empty row,
   or mentioning an effect-row variable is `E0418`.
9. A law body whose row is `{sim.read}` type-checks; a `where` guard whose row is
   `{sim.read}` is `E0417`.
10. A clause that is not `Bool` is `TYPE_MISMATCH`.
11. Attaching a spec changes neither `DefInfo::footprint` nor the test
    conflict graph nor `Isolation`.

Content addressing:

12. Adding, editing and deleting a `requires`, an `ensures` and a `law` each
    change **zero** definition hashes and select **zero** tests.
13. Editing a definition's body changes its `ensures` obligation's key.
14. Editing a definition named by a law changes that law's hash.
15. Renaming a definition named by a law changes no law hash.
16. Moving a definition or a law between modules changes no hash.
17. Reformatting a clause or a law changes no hash; reordering two `ensures`
    clauses changes both their keys, because the index is in the key.

The tier contract:

18. `Discharge::tier()` returns `Proved` only for `Evidence::Proof`, and there is
    no other constructor that yields it.
19. Every `Certificate` produced over the corpus names only fragment rules and
    has `guard_satisfiable: true`.
20. An obligation whose proof attempt spends its step budget reports `property`,
    never `proved` and never `refuted`.
21. A term outside the fragment — `x * y` with both symbolic, `x / 2`, `x % 3` —
    is uninterpreted, so `x / 2 * 2 == x` is **not** proved.
22. `reverse(reverse(xs)) == xs` reports `property`: a recursive definition is
    never unfolded.
23. A non-recursive definition is unfolded to at most depth 3, and the
    certificate says so.
24. `forall (b: Bool) { b || !b }` is `proved` by `ExhaustiveEnumeration` over 2
    points, not sampled.
25. A ground law is `proved` by `GroundEvaluation`.
26. `forall (f: (Int) -> Int, x: Int) { f(x) == f(x) }` is `proved` by
    congruence, with `f` uninterpreted.
27. A polymorphic law that is proved has its type variables in
    `Certificate::sorts`; the same law at `property` records
    `instantiations: [a := Int]`.
28. A guard the prover shows unsatisfiable is `Vacuous`, `E0420`, exit 1 — never
    `proved`.
29. A property run that keeps zero of a full case budget, over a guard no
    directed search can satisfy either, is `Vacuous`. One that keeps zero over a
    guard a directed search *can* satisfy is `Unattempted { GuardNotSampled }`,
    `W0604`, exit 0 — and if the prover had already decided the body, the witness
    completes the certificate and the obligation is `proved`.
30. A run keeping 7 of 200 cases reports `example`; the same obligation with a
    guard keeping 180 reports `property`.
31. An `ensures` on a definition performing an unhandled effect is
    `Unattempted { UnhandledEffect }`, `W0604`, exit 0, and does not count as
    covered.
32. An evaluation that raises is `Unattempted { Raised }` with a shrunk raising
    input, and is neither `Refuted` nor `Held`.
33. **The differential tier audit**: every corpus obligation reported `proved`
    survives 1,000 sampled cases across 8 roots. A refutation **and a raise** are
    each classified as a defect in Ply.
    *Enforced by `every_proof_a_generated_corpus_produces_survives_a_wide_sample`
    (`crates/ply-corpus/tests/tier_audit.rs:26`), verified in the 2026-08-17
    docs pass. It sweeps seeds 1..=6, re-samples at `cases: 1_000` over
    `roots: 0..8`, and `disagreement` treats both `Discharge::Refuted` and
    `Gap::Raised` as defects, which is the `i64`-versus-ℤ shape §5.1(a)
    disclosed. **It also closes the M7 hole**: the run ends
    `assert!(audited >= 100, "only {audited} proofs were audited")`, so an audit
    that examined nothing is red rather than green — which is precisely the
    defect M7 shipped when it reported `exhaustive: true` over regions never
    examined. Worth copying anywhere else this project asserts a property over
    a set it computed.*
33a. §5.1(g): a claim valid over ℤ and raising at `i64::MAX` is not proved, and
    the same claim under a guard that bounds its arithmetic is. `x / y == x / y`
    is not proved and `where y > 0 { x / y == x / y }` is. An obligation over a
    definition in a recursive component is not proved and does not count toward
    coverage.

Concurrency laws:

34. A concurrency law with no binders whose exploration is exhaustive is
    `proved`, with `Rule::ExhaustiveInterleaving`.
35. The same law with one `Int` binder is `property`, even at
    `exhaustive: true`. This is §6's condition 5.
36. Under `--sim random` or `--sim once` a concurrency law is `property`
    whatever it reports.
37. An exploration that spends its budget is `property` and is not cached under
    the bare key.
38. A `simulate` in a `requires`, an `ensures`, or a `where` guard is `E0417`.

Caching:

39. A `property` discharge is never written under the bare obligation key.
40. A `proved` discharge is read under a wider plan without re-running.
41. Widening `--prove-cases` re-runs every sampled obligation and no proved one.
42. Bumping `PROVER_VERSION` re-attempts every obligation and re-runs no test.
43. `Refuted` and `Vacuous` are never cached.
44. `Store::open` is unchanged in cost with a populated `obligations.json` and
    `reviews.json`.

Shrinking:

45. A shrunk counterexample still falsifies the obligation and still satisfies
    the guard; a candidate that leaves the guard's domain is rejected.
46. Two runs over the same refutation produce byte-identical shrunk bindings.
47. Shrinking terminates on a value whose shrink candidates are exhausted before
    the budget, and the size measure strictly decreases at every accepted step.
48. `Counterexample::original` carries the un-shrunk value and `shrinks` its
    step count.

Output and review:

49. The coverage line is present with no flags, in both commands.
50. A definition carrying only `requires` is **not** covered.
51. A definition named directly by a holding law is covered; one reachable only
    transitively is not.
52. A refuted or unattempted obligation covers nothing.
53. `ply review --changed` reports all five rows of §9.2's table on a project
    built to contain one of each.
54. `ply review --accept` writes a record keyed by name; renaming a definition
    loses its baseline and reports it as unreviewed rather than as unchanged.
55. `ply prove --json` is byte-identical across two runs and across `--jobs 1`
    and `--jobs 16`.
56. `ply prove` never calls `observe_definitions`: the next `ply test`'s suspect
    set is unchanged by a prove run.

## Alternatives considered

**Specs as part of the definition body.** No new hash, no new cache namespace,
no `SourceFingerprint` change — the whole of §4 disappears. Rejected because it
inverts the invalidation the milestone needs: every spec edit would move the
definition's hash and every transitive dependent's, so adding a postcondition to
a leaf function would re-run the entire suite. The artifact a reviewer is meant
to edit continuously would be the most expensive thing in the project to touch.
It is also false: two definitions computing the same thing under different claims
are one computation.

**One obligation per definition rather than per clause.** Simpler reporting and
a smaller key space. It also forces a definition whose first postcondition is
proved and whose second is sampled to report the weaker label for both, which
throws away exactly the information the tier exists to carry.

**A `modifies` clause.** The industry-standard answer to the frame problem, and
Ply is the one language that does not need it: the footprint is inferred, at
resource granularity, and checked as an upper bound. Adding `modifies` would ask
the user to restate — and get wrong — a fact the type system already holds.

**An SMT solver behind `proved`.** Enormously more reach for a fraction of the
code. Rejected in §12 and worth restating: a `proved` label whose truth depends
on which solver binary was on the path is not cacheable, not reproducible, and
not auditable. The whole design of this project is that an artifact is a function
of the definition set; a solver is a second, unversioned input.

**Reporting the strongest tier the system *believes*, upgrading on a heuristic.**
The version of this milestone that demos better. It is the one defect the project
cannot ship, and §5.6 makes it structurally unavailable rather than merely
forbidden.

**Making `example` a tier a user asks for**, with syntax for naming concrete
cases. It is a test. Ply has tests, they are cached, selected and scheduled, and
a second worse spelling of one is not worth a keyword. `example` is what the
system reports when a property run was too thin, which is the only time the label
carries information.

**Checking preconditions at call sites.** The thing that would make `requires`
mean what a reader expects. It needs a path condition for every call and a story
for the undecidable case, and building half of it — checking only the calls the
fragment can decide — would produce a `requires` that is enforced *sometimes*,
which is worse than one that is honestly documented as never enforced.

**A `--coverage` flag.** Rejected in §9.3. A tool that reports its successes by
default and its blind spots on request is a tool that misleads its user by
default.

**Deriving property tests from specs into the test suite,** so that `ply test`
covers obligations too. It conflates two claims with two cache keys, two
selection rules and two exit codes, and it would put a sampled claim into a
namespace whose entire promise is that a cached pass is provably unnecessary to
re-run.
