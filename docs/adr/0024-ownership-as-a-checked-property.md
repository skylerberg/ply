# ADR 0024 — Ownership as a checked property, not an inferred hint

**Status:** accepted as a direction. It decides no syntax and no implementation.
**Date:** 2026-08-28.

> **What this decides.** That whether a value is uniquely owned stops being a
> runtime accident the evaluator discovers and becomes a **static property the
> compiler checks and the signature shows**. It is the same move ADR 0002 and
> DESIGN.md §1 made for effects, applied to the other non-local invisible
> property a Ply program has.
>
> **What it does not decide.** The surface syntax, the inference algorithm, the
> migration, or whether `List` keeps its representation. §9 sketches a surface
> and is explicitly labelled a sketch.
>
> **What it takes no measurement of, and why.** Every other performance ADR in
> this record settles its question with a number. This one cannot: the thing to
> measure is a language that does not exist yet. What it does instead is show
> that the alternative — predicting the cost rather than checking it — was built
> and **refuted by measurement** (§2), and that the remaining design space is
> narrowed by an argument rather than by a benchmark (§3). *Narrowed*, not
> closed: §3 carries a fourth door the first draft of this ADR missed.

---

## §1 The defect, which is eleven lines

`crates/ply-eval/src/value.rs:22`:

```rust
pub type Vector<T> = Arc<Vec<T>>;
```

A `List` is a flat contiguous array behind a reference count. `push`
(`crates/ply-eval/src/builtins.rs:454`) therefore has two cases and no third:

```rust
Some(items) => { items.push(x); ... }        // sole owner → O(1)
None => {
    let mut out = Vec::with_capacity(list.len() + 1);
    out.extend(list.iter().cloned());        // shared → COPY THE WHOLE ARRAY
    out.push(x);
    out
}
```

Which case a program gets is decided by whether anything else holds a reference
at that instant. That is decided by where the call sits in its enclosing
expression — the rule ADR 0020 §5.2 measured — and it **composes across function
boundaries**: a correctly written callee is made quadratic by its caller, and the
trailing sub-expression that costs the copy can be a literal constant, because
`rc::carry` never asks what the remaining sub-expression *reads*.

**It shipped.** `crates/ply-std/ply/json.ply`'s `escape_runs` was quadratic in
the number of escapes in one string, on client-influenced input — a served
response echoing attacker-influenced text paid it. Measured three times by three
parties, and found by a spike rather than by the loop.

Two properties of this defect matter more than the defect:

- **Nothing in the type says it.** Not the signature, not the effect row, not the
  spec. A reviewer reading a specification — which DESIGN.md §Thesis says is what
  a human reviews — cannot see it.
- **Nothing a model can observe says it either.** An agent writing Ply cannot see
  a refcount. It can see a type.

## §2 The alternative was built, and refuted

A static lint was written for exactly this (`W0611`, `ply-core/src/fieldorder.rs`)
and refuted by adversarial review at two sizes against the interpreter's own
counters:

| shape | truth | lint |
| --- | --- | --- |
| `len(push([], i))` at argument 0 of 2 | 200/200 in place — **no copy at all** | **fires** |
| `{ a: s.a, b: push(s.a, i) }` — push in the last field | 0/200 in place — **fully quadratic** | **silent** |
| `snd(if i > 0 { s } else { [] }, push(s, i))` | 1/200 in place | silent |
| `snd({ let y = s; y }, push(s, i))` | 0/200 in place | silent |

False positives and false negatives, the second of them on the exact shape the
lint existed for. The pass also documented its own error set incorrectly —
its module comment explained the gap as *"Anything that goes through a call is
invisible to it"*, and two of the misses contain no call at all.

That is not a bad implementation to be improved. **A lint is a partial oracle
over an undecidable-in-practice dynamic property**, and the review's finding is
what a partial oracle looks like when someone finally measures it. The lint is
closed (PR #41).

## §3 The remaining design space is closed by an argument

> Value semantics + a flat array + two owners + append ⇒ **somebody must copy.**

That is a theorem, not an implementation detail, so it cannot be benchmarked
away. It leaves exactly three doors, and the third is the only one that keeps
what Ply wants:

1. **Give up value semantics.** `push` mutates and the old binding is gone. This
   is Go and Java. It removes the trap by removing the operation — there is no
   "push to a list someone else holds" to be expensive. The price is aliasing
   bugs, and Go's `append` sharing a backing array is its own famous footgun.
2. **Give up the flat array.** A persistent vector makes the shared case an
   O(log n) path copy instead of an O(n) full copy. The cliff becomes a slope and
   worst-case list building goes O(n²) → O(n log n), with no annotation and no
   analysis. The price is a permanent O(log n) on **indexing**, which is the
   operation every real program does most.
3. **Give up the second owner.** If the compiler proves there is one owner, the
   fast path is the only path. This is Rust, and it is the only door that keeps
   value semantics, O(1) indexing and native speed together.

> **Narrowed (2026-08-28, by the decision-record sweep): the space is not as
> closed as this section presents it.** Three doors are the ones this record
> knows, and at least a fourth exists — **make the copy visible instead of the
> ownership**. A `push` that is O(1) only in a linear argument position with the
> copying case spelled differently (`push_copy`), or a narrow array type whose
> only operations are append and freeze, puts the property in the type or the
> error without a whole-value ownership check. It is smaller in scope and worse
> ergonomically, and it fails differently — it moves the problem to conversion
> sites. §6 dismisses a `Builder` type on ergonomic grounds; **that is not a
> refutation**, and this ADR should not claim a closed space while a reviewer can
> find an unrefuted door in a paragraph. Swift's `isKnownUniquelyReferenced` and
> Koka's `fip` annotations live here.
>
> **And the claim door 3 actually earns is predictability, not speed.** Nothing
> here prices a program written so that every genuine two-owner copy is explicit
> against today's opportunistic `Arc::get_mut`. The two-owner case does not
> disappear under any door; what changes is whether you can see it coming. That
> is the real argument and it is the one to make.

**Swift is the control that shows door 3 is required rather than merely
available.** Swift has value semantics over a flat copy-on-write buffer — Ply's
design exactly — and has Ply's trap exactly, as a well-known performance footgun
in a mainstream, compiled, heavily-resourced language. Reaching for a better data
structure is not what saved anyone; nobody escaped this by being cleverer about
the representation.

## §4 The instrument error this ADR exists to correct

The first version of this argument recommended door 2, and its reasoning is
withdrawn here rather than deleted, because the shape of the error is the point.

> **Withdrawn: "the index cost is nearly invisible, so a persistent vector is
> close to free."** The evidence offered was ADR 0020 §6.3's profile — the
> machine's step and dispatch at **43.8%** of executed time, refcount traffic at
> **26.5%**, and every builtin body together at **1.3%** of leaf samples. Since
> indexing happens inside a builtin body, making it three pointer chases instead
> of one moves a few percent of a few percent.
>
> **The measurement is sound and the argument is not.** It says the interpreter's
> dispatch overhead is large enough to hide a data-structure regression. That is
> true, and it is an argument for **remaining an interpreter**. Ply intends to
> compile and to carry the workloads Go, Java and Swift carry; the target deletes
> the dispatch, at which point the index cost is fully exposed and permanent. A
> decision that is correct about today's implementation, and silently assumes
> today's implementation is the destination, is the error — not the number.

**And the same number has a second trap in it, which this ADR nearly walked
into.** "Every builtin body together at 1.3%" is a share measured *underneath*
43.8% dispatch and 26.5% refcount traffic. Delete both — which is what compiling
does — and the builtin share rises **by construction**. `push`'s O(n) copy is
inside that 1.3%. So read carelessly, the profile this ADR cites says the shipped
defect it was written about is negligible. ADR 0017 §R3 already states the general
form — *"A window share is not a request cost"* — and it applies to the numerator
here as much as to the denominator.

This ADR asserts the general form, because the record contains more of it:
**an interpreter ratio can settle a question about this evaluator and cannot
settle a question about the language.** ADR 0016's deferral of M9, ADR 0018's
verdict on compute kernels, ADR 0019 §4's rejection of a narrower `Value` and
ROADMAP's "What is next" queue are all ordered by such ratios, and each should be
read asking which question it actually answered.

## §5 The decision

**Whether a value is uniquely owned becomes a property the compiler checks and
the signature carries.**

Consequently:

- `List` keeps a **flat array**. O(1) index, O(1) amortised append, no cliff.
- Where a program genuinely needs two owners it says so — an explicit `copy`, and
  the O(n) is **visible in the source**. This is strictly better than Swift,
  which copies implicitly and silently at the same point.
- The optimisation stops being opportunistic. Today a `push` that reuses is a
  lucky outcome; under this decision it is a guaranteed one, and its absence is a
  compile-time event rather than a production one.

The precedent is the effect row and the argument is the same one. An effect is a
non-local property that decides whether code is **correct**, invisible in the
text, and Ply's answer was to put it in the type. Ownership is a non-local
property that decides whether code is **shippable**, invisible in the text. Same
problem; the record already contains the answer to it.

## §6 What Ply needs is much smaller than what Rust needs

Rust's difficulty is not ownership. It is **references** — lifetimes, borrows,
pointers into the interior of a value, and the analysis that keeps them sound.
That is where the learning curve lives.

Ply has none of it. There are no references in the surface language; values are
values. What Ply needs is whole-value ownership — *is this value's owner count
provably one here* — with no lifetime system, no borrow regions, and no interior
pointers. Clean did this without lifetimes; Roc is reaching for it.

**And the ergonomic form is inference, not annotation.** An early draft of this
argument proposed a distinct `Builder` type that a program converts into and out
of. That is the clunky form, it is not what Rust does, and it should not be built:
Rust's ergonomics come from working exclusivity out at use sites so the author
writes ordinary code. Ply should infer, report in the signature, and require a
word from the author only where the answer is "this is shared and the copy is
real".

## §7 The analysis already exists, as a hint

`crates/ply-eval/src/rc.rs:74-83`:

```rust
pub enum Own {
    Borrowed,   // the binding is read again later, so the read clones
    Owned,      // the last use of a binding — the machine may move the value
}
```

with the doc: *"It is an optimization hint and never a permission: see the module
comment for why a wrong `Owned` cannot change what a program means."* There are
`Dead` sets, and `code.rs`'s lowering already threads liveness.

> **Withdrawn (2026-08-28, by the decision-record sweep): "The change is to
> promote it from a hint to a guarantee and surface it, which is a smaller
> distance than starting from nothing."** The second clause is the error and it
> understates the work badly. `rc.rs:41-47` states where correctness actually
> comes from: `take_unique` "empties a binding only when every link from the head
> of the chain down to that binding is uniquely owned … **A wrong `Owned`
> therefore costs a wasted walk, never a wrong answer.**" The guarantee is
> carried by a **dynamic guard**, and `Own` is permitted to be wrong precisely
> because the guard is there. Promoting `Own` deletes the thing currently holding
> the property up: the analysis has to become *sound* where today it is
> deliberately allowed not to be. "The analysis half already exists" is true of
> the dataflow skeleton and **false of any guarantee**.

So what exists is a **dataflow skeleton and a place to hang the check**, not a
half-built guarantee. That is still worth more than starting from nothing, and it
is much less than this section first claimed.

**Two further cautions from the same sweep, both about reading `rc.rs` as source
material.** The same module doc records that multi-shot resumption is safe *because*
"a resumed frame is cloned out of the continuation's shared segment, so its scope
is shared, so nothing in it is ever taken" — a static owner analysis gets no such
escape, and §9's continuation bullet is where that lands. And `carry` is an
artifact of this evaluator's environment representation: a scope here is a
persistent `Rc` chain shared by a closure, a continuation frame and the current
evaluation, and **a compiled target with flat frames has no such chain**, so
`carry` and the ratios attached to it do not survive the target. Designing the new
analysis around them would be this ADR's own §4 error, committed inside the ADR
that names it.

## §8 A sketch of the surface, explicitly not decided

Alongside an effect row, an ownership marking on parameters:

```ply
fn build(xs: List<Int>, n: Int) -> List<Int> = ...           // xs is shared; push copies
fn build(own xs: List<Int>, n: Int) -> List<Int> = ...       // xs is consumed; push is in place
```

Nothing here is decided: whether the marking is on the parameter or in the row,
whether it is inferred and only *shown*, what it does to `derive`, and what the
error message says when a program needs a `copy`. Those are the next ADR's, and
the sketch is here only so the shape is arguable.

## §9 Consequences worth stating before anyone starts

- **The two engines.** Both must agree on what is checked. A static property is
  easier here than a dynamic one — it is decided before either engine runs — but
  ADR 0005 §6 scheduled the tree-walker for deletion at M6 and it still ships,
  and this is another change that two evaluators pay for twice.
- **Hashing.** If ownership enters a type or a signature it enters normalization,
  and every `DefHash` moves once. That is a `FRONTEND_VERSION` bump and one full
  re-run, and it should happen deliberately and once.
- **The compiled fragment.** This is where the payoff is. A guaranteed-unique
  `push` lowers to a native append with no refcount check; a checked property is
  exactly the kind of fact a code generator can use and a runtime hint is not.
- **The standard library will move.** `json.ply`, `http.ply` and `router.ply` all
  contain accumulator shapes written around the current rule, and some exist only
  because of it.
- **`Map` is already fine and should be left alone.** It is
  `rpds::RedBlackTreeMap`, a persistent tree with structural sharing, so a shared
  insert copies a path. It has no cliff and needs no ownership to be fast. The
  asymmetry is worth noticing: **the language already made this choice correctly
  for one container**, and the trap exists only in the one built on `Arc<Vec>`.

## §10 What would make this wrong

- **If ownership inference cannot be made to work without annotation burden**,
  and Ply acquires a borrow checker's learning curve in a language whose premise
  is that a model writes the code and a human reads specifications. That is the
  central risk and it is not small. Door 2 is the fallback, and its cost — a
  permanent O(log n) index — is a known quantity rather than a discovered one.
- **If the compiled target does not arrive.** Then the interpreter's dispatch
  overhead really does hide the representation cost, §4's withdrawn argument
  becomes correct again, and door 2 is the cheaper answer. This ADR is a bet on
  the same world ADR 0021 §2 bets on, and it fails in the same world.
- **If indexing turns out not to matter** — if real Ply programs iterate and fold
  and rarely index. The current `List` surface has no index at all
  (`len/push/map/filter/fold/range`), which is *weak evidence for* that and was
  briefly mistaken for strong evidence. Designing the container around an API
  that is small because the language is young is its own version of §4's error.
- **If a fourth door exists.** The three in §3 are the ones this record knows.
  An argument that finds another is the most valuable refutation of this ADR.

## §11 Relationship to the rest of the record

- **ADR 0002, DESIGN.md §1** — the effect row is the precedent this rests on.
- **ADR 0020 §5.2** — measured the positional rule; **§6.3** is the profile whose
  misuse §4 corrects.
- **ADR 0021 §4** — item 1 of the critical path is "a lint for the field-order
  rule". **That item is superseded by this ADR**, and 0021 should be corrected in
  place to say so: the lint was built and refuted, and the precondition it named
  is answered by checking rather than by warning.
- **ADR 0019 §4** — rejected narrowing `Value` on interpreter evidence; re-read
  under §4's general form.
- **ADR 0017** — regions already enforce non-escape (a `Cell` in a region's result
  type is a compile error). Whether that machinery is what a whole-value ownership
  system builds on is an open question this ADR does not answer.

## §12 Provenance

No new measurement was taken, and §0 says why. The figures quoted are: ADR 0020
§6.3's profile (43.8% / 26.5% / 1.3%, two windows, `/usr/bin/sample`); ADR 0020
§4.1 and §5.2's positional and composition series; and the four lint rows in §2,
taken by the adversarial review of PR #41 at n = 200 and n = 400 against
`ply_eval::rc::Stats`, engine pinned to the machine.

Code read and verified on `main` at `607f9e3`: `value.rs:22`, `value.rs:79`,
`builtins.rs:454`, `rc.rs:74-83`, and that `Value::Map` is
`rpds::RedBlackTreeMap`.
