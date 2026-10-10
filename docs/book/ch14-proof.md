# 14. Specifications, laws and proof

A test says "this input gave this answer". A **specification** says "this holds
for every input", and Ply will try to prove it — with a proof when the claim is
decidable, and with a sample when it is not. This chapter covers specs, laws,
what `proved` really means, and how to state a cost bound.

## `requires` and `ensures`

A specification goes between a function's signature and its body:

```ply
type Account = { name: String, balance: Int }

fn adjusted(account: Account, amount: Int) -> Account
  requires amount > -1000000000 && amount < 1000000000
  requires account.balance > -1000000000 && account.balance < 1000000000
  ensures result.name == account.name
  ensures result.balance == account.balance + amount
= { name: account.name, balance: account.balance + amount }
```

- `requires` restricts the domain of the `ensures` below it.
- `result` is bound in an `ensures`.
- **`requires` is not checked at call sites**, and laws do not inherit it. It is
  part of the definition's contract for the prover, not a run-time check.
- Specs, guards and law bodies must be pure (`E0417`), except that they may raise
  or diverge.
- Specs do not change a definition's hash.
- An `ensures` implies that every resource outside the footprint is unchanged;
  there is no `old()`.

A caller in another package is checked against the spec, never the body. To let
another package unfold a body instead, write `transparent fn`; then its body is
part of what its dependents are proved against.

## Laws

```ply
law "a credit and a matching debit restore an account"
  forall (account: Account, amount: Int)
  where amount > -1000000000 && amount < 1000000000
     && account.balance > -1000000000 && account.balance < 1000000000 {
    adjusted(adjusted(account, amount), -amount) == account
  }
```

A law has a label, optional typed `forall` binders, an optional `where` guard, and
a block body. It is keyed `<module>.<label>` like a test. A binder type that
cannot be quantified is `E0418` (a sum whose function performs the type's row
parameter, or a type that holds an `opaque` value and states no `gen`).

Because `Int` arithmetic is checked, bound the domain with a guard. An unguarded
law over `Int` arithmetic is not wrong — it is `unattempted`:

```console
$ ply prove
   ~ unattempted law "doubling a non-negative number is not negative" raised: integer overflow in addition
       shrunk to n = 9223372036854775800
```

The shrink line tells you exactly why the prover gave up: `double(n)` overflows
for a large enough `n`. Adding `&& n < 4000000000000000000` to the guard makes it
`proved`.

## The tiers

| tier | claim |
| --- | --- |
| `proved` | holds for every input satisfying the guard, or a `cost` clause read off the body |
| `property` | randomized cases passed; failures shrink |
| `example` | concrete cases passed |
| `fitted` | a cost law's steps kept its bound's pace over eight or more sizes |
| `unattempted` (`W0604`) | undecided; never green, never cached |
| `defect` | Ply failed rather than the program: nothing is claimed, never cached, exit 1 |

`ply prove` reports each obligation's tier, and `E0419` is a counterexample while
`E0420` is a guard that admits no values:

```console
$ ply prove
   4 obligations · 4 proved · 0 property · 0 example   (0.01s)

   ✓ proved      p.adjusted ensures #0                    congruence · propositional · linear arithmetic · 203 steps
   ✓ proved      p.adjusted ensures #1                    congruence · propositional · injectivity · linear arithmetic · 234 steps
   ✓ proved      law "a credit and a matching debit restore an account" propositional · congruence · linear arithmetic · 1 unfolding · 511 steps
   ✓ proved      law "doubling a non-negative number is not negative" propositional · linear arithmetic · 1 unfolding · 184 steps
```

The list after each `proved` names the proof rules used: congruence,
propositional reasoning, linear arithmetic, injectivity, case analysis,
unfoldings. Those are the facts the certificate is made of, which is why a
`proved` verdict can be audited rather than merely believed.

## What `proved` covers

`proved` covers ground evaluation, enumeration of finite domains up to 4096
points, linear `Int` arithmetic, case splits, congruence, constructor
injectivity, unfolding non-recursive definitions, a `match` taking its arm over a
value whose shape is in view, exhaustive interleaving, and induction: a definition
that calls only itself and that the checker reads as ending (chapter 6) is
unrolled, and over values in view as deep as the unfolding goes.

Induction is structural. On an `Int` binder the claim is proved at `n <= 0` and
then at `n > 0` from itself at `n - 1`; on a `List` binder at `[]` and then at
`[h, ..t]` from itself at `t`. A kernel checks each induction from the claim
alone before refuting its cases, whatever proposed it.

Two things are deliberately outside that fragment. Congruence and injectivity
read `==` as a value's constructors and fields say, which a `Float` (`NaN !=
NaN`) and a type that states a `key` (chapter 4) are not; and arithmetic is the
integers', which an operator at a type that states `numeric` is not. A claim that
holds a value of either, at any depth, is never `proved`: it is run, over a finite
domain or over a sample.

## Sampling, and generators

A sampled claim draws each binder from its type: a scalar, a list, a record or a
sum from its structure, and a type that states a generator through it:

```ply
import std.gen
import std.gen (Gen)

pub type Date = new { year: Int, month: Int, day: Int }

gen for Date by dates

fn dates() -> Gen<Date> =
  gen::map2(gen::int_between(1, 12), gen::int_between(1, 28), |month: Int, day: Int|
    { year: 2000, month: month, day: day })
```

```console
$ ply prove
   ✓ property    law "a drawn date is in the year 2000"   200 cases · 0 rejected
   ✓ property    law "a drawn day is a real day of a month" 200 cases · 0 rejected
```

A law over `Date` draws each value through `dates`, so a sample sees only dates
the module makes — which is also what lets a law in any module quantify over an
`opaque` type that states a generator (§3.2). A generator is `f: () -> Gen<T>`,
or one that takes a generator per parameter. It performs nothing but
`abort.raise`, and it is not part of its type's hash: editing one re-checks no
definition and re-runs no test, and draws again exactly the samples drawn through
it.

A counterexample **shrinks** as it was drawn — a structural value by its parts,
and a generated one by replaying a shorter or lower record of the draws that made
it — so what a refutation shows is still a value its generator makes.

## Lemmas

A law with no guard, once proved, is a lemma for every claim written below it in
its module. Its trigger is the first call its body always makes whose arguments
name every binder; where a claim makes a call that fits it, the law at that call
is a fact:

```ply
fn reverse(xs: List<Int>) -> List<Int> =
  match xs { [] -> [], [x, ..rest] -> push(reverse(rest), x) }

law "a push reversed leads" forall (ys: List<Int>, x: Int) {
  match reverse(push(ys, x)) { [h, ..t] -> h == x && t == reverse(ys), [] -> false }
}

law "reverse twice is identity" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }
```

Both are `proved`, the second by structural induction on `xs` citing the first.

## `law schema`: a law over definitions

A `law schema` states a law once, over the definitions it is about, and a law
instantiates it:

```ply
pub law schema round_trip<a, b>(encode: (a) -> b, decode: (b) -> Option<a>)
  forall (x: a) { decode(encode(x)) == Some(x) }

type Color = | Red | Green | Blue

fn enc(c: Color) -> Int = match c { Red -> 0, Green -> 1, Blue -> 2 }
fn dec(n: Int) -> Option<Color> =
  match n { 0 -> Some(Red), 1 -> Some(Green), 2 -> Some(Blue), _ -> None }

law "a color round-trips" = round_trip(enc, dec)
```

```console
$ ply prove
   ✓ proved      law "a color round-trips"                case analysis over p.Color (3 arms) · congruence · 2 unfoldings · 6 steps
```

An instantiation gives one argument for each parameter, in order, each a
definition name, a lambda or a value. Its own label, key, tier and cache entry are
its own; it is proved and sampled exactly as the law written out would be. A
schema's row parameter is what the definitions it is given may raise, so a schema
over effectful functions is expressible. `std.laws` ships the schemas the standard
library states its laws with.

## Cost

A `cost` clause bounds the steps a call of its definition takes, and a cost law
states how a body's steps grow with a size:

```ply
fn pairs(xs: List<Int>, ys: List<Int>) -> Int
  cost len(xs) * (len(ys) + 1)
= fold(xs, 0, |a: Int, x: Int| fold(ys, a, |b: Int, y: Int| b + x * y))
```

```ply
import std.list (sort)
import std.math (ilog2)

law "sorting is n log n" forall (n: Int) where n > 1 cost n * ilog2(n) {
  sort(map(range(0, n), |i: Int| i * 7919 % n))
}
```

```console
   ✓ fitted      law "sorting is n log n"                 12 sizes to 4096 · from 2048 to 4096 steps grew 2.12×, the bound 2.18×
   ✓ proved      law "cost of pairs"                      cost bound · 1 steps
```

`ply prove` first reads the steps off the body. A builtin is at most a step;
`map`, `filter`, `fold` and their siblings call a lambda back as many times as an
argument's size allows; a call of a definition costs its steps at the sizes of its
arguments; a recursion is read when every call back takes a part of one list
parameter a pattern took a head off. Steps within the bound are `proved`; anything
else falls back to the cost law `cost of <name>`, which meters calls at increasing
sizes and reports `fitted`, `outgrown` (`E0464`) or a gap.

A cost law runs the body at the sizes 1, 2, 4 … 4096 the guard keeps and stops at
a size that takes more than ten million steps. Over the last two spans the steps
may grow no faster than the bound does, give or take a twentieth of a doubling,
so constant factors and lower-order terms do not count. That is what makes a
`cost` law usable: you state the shape of the growth, not a machine's timing.

A function parameter may name the steps a call of it takes, after its type:

```ply
fn each_of<| e>(xs: List<Int>, f: (Int) -> Int / e cost k) -> Int / e
  cost len(xs) * k
= fold(xs, 0, |a: Int, x: Int| a + f(x))
```

Only the definition's `cost` clauses read that name, and at a call it stands for
the steps of the function given. This is how a cost bound composes through a
higher-order function.

## Running the prover

`ply prove` reports the definitions carrying no obligation, then each obligation's
tier. Flags:

- `--filter SUBSTRING` restricts to obligations whose owner's name holds it.
- `--prove-cases N` (below 25 keeps only `example`), `--prove-roots N`,
  `--prove-budget N` (spent reports `property`), `--shrink-budget N`.
- `--prove-steps N` is the calls per evaluation of a claim; an evaluation past it
  leaves the obligation `unattempted`, and the number keys the cached result, so
  more budget is a stronger claim.
- `--reach` asks the static tier alone about every obligation and reports what it
  decided and where it left the decidable fragment. Under `--json` each then
  carries `reach`.

A `proved` obligation is cached under its claim's hash, which reads another
package's definitions by their contracts, so it stands across an edit to a
dependency's body. A sampled one is cached under the hash of every implementation
its cases run and of each generator its points are drawn through. Only a claim
that held is cached; `unattempted` among them is discharged again by every run.

> **Try it.** Take a function with a subtle invariant, write the invariant as an
> `ensures`, and run `ply prove`. If it comes back `unattempted`, read the reason
> and the shrink — it will usually name the arithmetic that escaped, and the fix
> is a guard. If it comes back `property`, the claim is true but outside the
> decidable fragment; consider whether a lemma above it can get it to `proved`.

## Summary

- `requires` and `ensures` state a function's contract; `result` is the answer.
  They are for the prover, not checked at call sites, and free of a hash.
- A `law` states a claim over `forall` binders with an optional guard.
- Tiers: `proved`, `property`, `example`, `fitted`, `unattempted`, `defect`.
  `proved` is a decidable fragment plus induction; a guard bounds `Int` arithmetic
  so overflow stays out.
- `gen for T by f` says how a proof draws a `T`; samples shrink.
- A proved law with no guard is a lemma for the claims below it.
- A `law schema` states a law once and instantiates it over definitions.
- `cost` clauses and cost laws state and check growth, not timing.

Next: getting a value's equality, order, encoding and display without writing the
code by hand.
