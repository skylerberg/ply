# ADR 0023 — Record update

Status: accepted — implemented in W4.
Date: 2026-08-27
Renumbered: this shipped for one round as `docs/adr/0022-record-update.md`.
`0022` is `docs/adr/0022-the-call-ceiling.md`, written on a sibling branch and
open as PR #34; two branches picked the same free number because each looked
only at its own `docs/adr/`. Anything citing "ADR 0022 — Record update" means
this file.
Closes: `spikes/ply-lexer/GAPS.md` §2, and withdraws one sentence of it (see §6).
Constrained by: ADR 0001 (a definition's identity is its normalized structure),
ADR 0002 (gate 1 skips a file on its content hash alone), ADR 0009 / ADR 0013
§1.3–§1.4 (the precedent for expanding inside `parse_module` and for accepting
module-locality as the price).

## Context

Ply had no record update. Changing one field of a record meant writing every
other field out, which cost `lexer.ply` three functions that exist only to spell
a field list once each (`GAPS.md` §2) and cost `std.http`'s `chunk_trailers`
thirteen lines to change one bound.

The thirteen-line one is the interesting case. `Limits` is the record ADR 0013 §4
puts every bound a program runs under into — `max_body`, `max_chunk_size`, the
timeouts — and all thirteen of its fields are `Int`. So
`max_body: state.limits.max_chunk_size` type-checks, and is a silently wrong
bound in an HTTP server.

## The constraint that shapes everything below

Ply is content-addressed. A definition's `DefHash` is over its normalized form,
a test re-runs iff its hash is absent from the cache, and two definitions that
compute the same value should be *one* definition with one cache entry.

So the requirement is not "record update should work". It is:

> **`{..s, a: 1}` must be the same definition as the record literal it stands
> for.**

If it were not, every definition in the tree that adopted the sugar would move,
and the two spellings of one value would become two definitions, each re-running
its own tests. That is asserted, not argued:
`crates/ply-hash/tests/audit.rs record_update_hashes_as_its_expansion`.

## Decision 1 — Expansion runs inside `ply_syntax::parse_module`

An unexpanded `ExprKind::RecordUpdate` never escapes the parser. `parse_module`
calls `record_update::expand` immediately after `effect_set::expand`, and
`parse_expr` — which has no module around it — runs the same pass with an empty
context so that a spread there refuses rather than leaking.

**Not in normalization, which is the obvious place.** The driver hashes *before*
it infers (`crates/ply-cli/src/driver.rs`: parse → resolve → hash → gate 2 →
infer), which ADR 0002 pins deliberately. Normalization therefore has no types
and cannot enumerate the base's fields. And changing `normalize.rs`'s byte
stream is a cache-format change that moves every cached result everywhere.

**Not in each consumer.** A `RecordUpdate` that survived into `ply-hash`,
`ply-core` and `ply-eval` would be four implementations of one construct — the
exact shape ADR 0001 rejected `.` for qualified references over: "Four
implementations, four chances to disagree." Instead those crates carry arms that
`unreachable!`, and the arms are safe because the escape is *checked*:
`crates/ply-syntax/src/tests.rs
no_record_update_survives_parse_module_anywhere_in_the_tree` parses every `.ply`
file in the repository plus a file that uses the syntax, on both `Module`-
returning entry points, and asserts none survives.

`ply-prove` is the one that does not `unreachable!`. A prover's safe answer is
"I did not reason about this" and never a term it guessed, so its arm records a
blocker instead — `Blocker::UnexpandedSugar`, added for this and named for what
it is. It is deliberately **not** `Blocker::Region`, which is what the arm first
carried: a record update is not a `perform`, a `handle` or a `simulate`, and
`ply-corpus` prints a blocker's label to a reader. The arm is unreachable today,
so no report can contain it and no test can watch it fire; what *is* checked is
the other half — `ply_corpus::discharge::label` matches `Blocker` exhaustively
with no wildcard, so a variant cannot be added without a row to print, and
deleting the row fails the build with `E0004`.

## Decision 2 — The canonical expansion

`{..b, f1: e1, ..., fk: ek}` becomes a plain `ExprKind::Record` whose fields are:

1. every field of `b`'s shape **not** named among `f1..fk`, **sorted by field
   name**, each with value `b.<name>`; then
2. `f1: e1, ..., fk: ek`, **in the order written**.

Both halves are load-bearing.

**Sorted, not the type's declaration order.** `crates/ply-hash/tests/audit.rs
reordering_the_fields_of_a_record_type_is_free` is an invariant the suite
asserts: `normalize.rs` sorts `TY_RECORD`, so `{a: Int, b: Int}` and
`{b: Int, a: Int}` are one type. Expanding in declaration order would make
reordering a `type` move the hash of every update written against it, breaking
that invariant. `reordering_the_updated_type_is_still_free` pins the fix.

**By name, and that has to be pinned by a test that can see the difference.**
Every field name in a suite written `a`/`b`/`c` orders identically under sorting
by name and under any comparator that compares length first, so such a suite
says nothing about which one ran. One mixed-length pair is not enough either: it
only rules out the length direction it happens to disagree with. So the three
tests that pin the order — `copies_are_sorted_by_name_and_not_by_length`
(`crates/ply-syntax/src/tests.rs`), `record_update_hashes_as_its_expansion` and
`a_projected_base_hashes_as_its_expansion` (`crates/ply-hash/tests/audit.rs`) —
each carry a pair whose longer name sorts first *and* a pair whose shorter one
does. A length-first comparator in either direction goes red.

**Written fields last.** `spikes/ply-lexer/GAPS.md` §1 measures a growing
sub-expression in any but the last position of its enclosing node as quadratic —
4x per doubling against 2x, mechanism read off `ply_eval::rc::carry`. Copies are
pure field reads and never grow, so emitting them first is free, and it puts
`{..s, toks: push(s.toks, t)}` on §1's linear spelling.

## Decision 3 — The base is a path, not an expression

`s` and `state.limits` are bases; `f()` is not.

This is not a parser convenience. The expansion writes `base.f` once per copied
field, exactly as the longhand does. A base with a call or a `perform` in it
would run that base once per field — thirteen times for `Limits`. Restricting it
to a repeatable, pure path is what makes the sugar and the longhand the *same
definition* rather than merely the same value.

The lift, when a call base is wanted, is one line of Ply and needs nothing from
the compiler:

```ply
fn limits_keeping(n: Int) -> Limits = {
  let base: Limits = default_limits();
  {..base, max_keep_alive: n}
}
```

That block is `crates/ply-std/ply/http.ply`'s `limits_keeping`, verbatim.

## Decision 4 — The shape is read from this module only

Expansion reads this module's own `type` items and the type annotations written
in this file. `{..cfg, x: 1}` where `cfg: std::http::Limits` is `E0116`.

This is the same restriction ADR 0013 §1.3/§1.4 accepted for effect sets, for
the same reason: gate 1 (ADR 0002) skips a file whose raw bytes are unchanged,
so a file's hashes must be a function of its own bytes. A shape read across a
module boundary would let an edit in the declaring module leave a stale expansion
behind in a file that never moved — and a stale expansion is a **wrong record**,
not merely a stale name.

The cost is real and is not hidden: the stdlib gets the win at its own definition
sites, and a user of `std.http` writing `{..limits, max_body: n}` does not.

**Counted before the restriction was accepted, rather than assumed to be small.**
A heuristic scan of every `.ply` file in the tree — record literals with two or
more fields written `name: base.name`, which is what a record update replaces —
finds **48 candidate sites across 14 files**, of which **39 have the base's type
declared in the same file** and are expandable today. The nine remaining are
mostly the scan failing to resolve a nested path rather than genuine
cross-module bases; `examples/desk.ply`'s `h.request` against `std.http`'s
`Request` is the clearest real exclusion. So the restriction costs on the order
of one site in ten, not most of them, which is what makes it a deferral rather
than the wrong shape of feature. Method and raw output are in
`PREREGISTRATION.md`. The
lift path, should a program want it, is the two-pass shape a cross-module
resolution needs, plus a cache key over the declaring module's *shape* rather
than over the importing file's bytes. Neither is cheap and nothing in W4 needed
it.

Also refused, each with its own note: a generic alias (`P<t>` has no field list
until it is applied), a sum type, a type this file does not declare, an alias
chain deeper than sixteen (a bound rather than a cycle check, because this runs
before anything has rejected `type A = B` / `type B = A`), and a binder with no
written type — including one that **shadows** an annotated binder. Shadowing
removes the annotation rather than keeping it: expanding against the outer
binder's shape would be a record of some other type's width, and the reader would
be looking at a diagnostic about a literal they did not write.

## Decision 5 — Two codes, and no third

- `E0116 RECORD_UPDATE_SHAPE` — the base has no record shape this file can name.
- `E0117 RECORD_UPDATE_FIELD` — a named field is not a field of the base. An
  update replaces; it does not widen, because the result's type is the base's
  type.

There is deliberately **no** typing rule for a record update and no third code
for a width violation. By the time inference runs, `{..s, a: 1}` *is* the literal
that copies `s`'s other fields, so it meets the same exact-key-set unification
(`crates/ply-core/src/unify.rs`) every record literal meets. That is what makes a
wrong shape a diagnostic:

- **too wide** — the expansion emits `b.x` for an `x` the base lacks, and
  inference reports `E0101` "no field `x` on this record";
- **too narrow** — the expansion emits a record of the wrong width, and `unify`
  rejects it wherever the result meets a known record type.

The second is **not total**, and this ADR says so rather than dressing it up: a
`{..s}` whose result is never constrained by any annotation would go unnoticed.
What closes it in practice is that the shape is computed from the *same written
annotation* inference uses to type the binder, so there is no independent "real"
field set to disagree with — the residual risk is a bug in this pass, not a
program a user can write. `crates/ply-core/tests/record_update.rs` is what checks
that claim; a per-node assertion is **not enforced**.

## Consequences

`chunk_trailers` is one line where it was thirteen. `limits_keeping` and
`limits_streaming` are two each and `limits_with` is four, counting decision 3's
`let base: Limits = default_limits();` lift as one of them. Their `DefHash`es
moved once and so did their transitive dependents', and **the moved set is that
dependent set exactly** — nothing moved that is not a dependent, and no
dependent failed to move.

| corpus | entries | moved | of which |
| --- | --- | --- | --- |
| `crates/ply-std/ply/http.ply` | 206 (150 definitions + 56 tests) | **47** | 23 definitions + 24 tests |
| `examples/` | 1,428 (1,067 definitions + 361 tests) | **91** | 44 `desk.*`, 47 `std.http.*` |
| the other seven shipped modules | — | **0** | — |

`examples/` moves at all because `examples/desk.ply` imports `std.http`, so it is
a transitive dependent and moving is what content addressing is *for*; the 47 in
that row are `std.http`'s own entries, which the listing carries because the
corpus imports the module. No definition or test written under `examples/` moved
other than `desk`'s 44. That is not a violation of the constraint above: the
constraint is "sugar ≡ its canonical expansion", not "sugar ≡ the longhand that
happened to be in the file".

> **Two figures here were wrong, and the first was wrong in the reassuring
> direction.** The paragraph read:
>
> > `chunk_trailers` is one line. Its `DefHash` moved once, and so did its
> > transitive dependents' — 40 of 206 entries in `std.http`, and no other
> > definition in `examples/` (1428 entries, 0 moved) or in the other seven
> > shipped modules.
>
> **The `0` is what a stale binary reports.** `crates/ply-std/src/lib.rs`
> `include_str!`s every `crates/ply-std/ply/*.ply` **into the binary**, so
> `import std.http` resolves to the copy compiled in and never to the file that
> was edited: re-running `ply hash` after editing `http.ply` without rebuilding
> compares a binary against itself. CONTRIBUTING's prescribed instrument check
> cannot see it either, because the stale input is a `.ply` and that check looks
> only at `.rs`. Use
> `find crates \( -name '*.rs' -o -name '*.ply' \) -newer target/release/ply`.
> Re-taken on binaries verified fresh against both extensions, on corpora copied
> out of each checkout with `.ply-cache` excluded, twice and identical both
> times: the `chunk_trailers`-only tree moves **84 of 1,428** in `examples/`,
> every one a dependent. The 91 above is that tree plus the three `Limits`
> helpers this ADR originally left alone.
>
> **And "40 of 206" was a scope, not a total.** It counted the 20 definitions
> and 20 tests of `std.http` that moved, which is right, but "40" alone reads as
> "40 definitions". A neighbouring reading of "60 moved (40 definitions + 20
> tests)" is not a second measurement of the same thing and is not a
> contradiction: it comes from hashing the whole `crates/ply-std/ply/` directory,
> where `http.ply` is listed **twice** — once as the module `http`, the file, and
> once as `std.http`, the copy compiled into the binary and reached through
> another module's `import` — and then keying the comparison on the bare name.
> That key deduplicates one half and not the other: a definition's name already
> carries its module, so `http.serve` and `std.http.serve` survive as two rows,
> while a test's name is its title, so the titles that appear under both modules
> collapse into one. Definitions doubled, tests not: 40 + 20. Keyed on
> `(module, name)` the same corpus reports 80. Both are artefacts of the corpus
> and the key. One file, one honest denominator: `ply hash` on `http.ply`
> itself.

No `FRONTEND_VERSION` or `RUNTIME_VERSION` bump. Nothing about normalization's
byte stream changed and no stored type moved; the sugar is gone before `ply-hash`
sees anything. Measured rather than assumed: a binary built from this branch's
compiler with the **base** `http.ply` embedded hashes the base corpus to
**0 moved entries** in `examples/`, in `crates/ply-std/ply/` and in `http.ply`
alone, so every entry in the table above is attributable to the `.ply` edit and
none of it to the compiler.

## What this does *not* do

**The field-order trap (`GAPS.md` §1) is narrowed, not removed.** §1 measures a
growing sub-expression in any but the **last** position of its enclosing node as
quadratic — 4× per doubling against 2×, the mechanism read off
`ply_eval::rc::carry`. The expansion emits copies first and written fields last,
and a copy is a pure field read that never grows, so a single-write update whose
value grows — `{..s, toks: push(s.toks, t)}` — is *always* on §1's linear
spelling: the expansion has nothing it can put after it.

What remains is exactly the several-writes case, and it is worth stating in full
rather than as a caveat:

- `{..s, toks: push(s.toks, t), pos: p}` is **still quadratic**. `pos: p` is
  emitted after the growing `toks`, so the growing field is not last in the
  record node, which is the position §1 measures.
- `{..s, pos: p, toks: push(s.toks, t)}` puts the growing field last and so
  lands on §1's *linear* spelling, and computes the same record value. That is
  §1's measured rule applied, not a fresh measurement of this expression.
- **The two are not the same definition.** Written fields are emitted in the
  order written and field order is part of a record's hash
  (`crates/ply-hash/src/tests.rs swapping_two_record_fields_changes_the_hash`),
  so reordering the writes moves the `DefHash` and re-runs the tests that reach
  it. The linear spelling is available and it is not free.

So the rule a writer must still keep is narrower than before — *a growing field
must be written last among the fields you write*, rather than last among all
thirteen — but it is not empty. It is, however, **syntactic**: which written
field grows and where it sits are both readable at the update site with no
types, which is what makes it lintable. This ADR does not implement that lint;
a separate workstream owns it. It does not claim the trap is gone.

**It does not fix `default_limits()`**, and it cannot: that function constructs a
`Limits` from nothing, so there is no base to update, and it must go on naming
every field. It is the one irreducible spelling of the record and the residual
place a wrong *constant* can hide — `max_chunk_size: 4096` is as well typed as
`max_chunk_size: 1048576`. Nothing in this ADR helps there; what does is that it
is now the **only** such site.

> **`limits_with`, `limits_keeping` and `limits_streaming` are no longer on this
> list.** This paragraph continued:
>
> > nor `limits_with` / `limits_keeping` / `limits_streaming` (`http.ply:1654`,
> > `:2388`, `:2833`), which re-spell `Limits` by hand and were left alone so
> > that no `DefHash` outside `chunk_trailers`' cone moved in this change.
> > `limits_keeping` and `limits_streaming` are exactly the
> > `{..default_limits(), one_field: n}` shape and become one line each under
> > decision 3's `let` lift, whenever moving their hashes is acceptable.
>
> They were held back so that the hash-movement criterion could be read against
> `chunk_trailers` alone. That criterion is established, so the deferral was
> spent: all three now take decision 3's `let` lift, and the moved set is still
> the transitive-dependent set of what changed, exactly. Deferring them was the
> right order to do the work in and the wrong place to stop — a site left
> hand-written is a site that can be mispaired, and `limits_keeping`'s twelve
> unvaried bounds were mispairable in silence: crossing `max_chunk_size` and
> `max_chunk_line` there left `ply check` reporting
> `checked 2 modules, 150 definitions, 56 tests` and every targeted suite green.
> What is asserted now is the property, not the spelling:
> `crates/ply-cli/tests/stdlib.rs the_limits_helpers_vary_only_the_bounds_they_are_named_for`.
>
> One mispairing survives conversion and is named rather than implied.
> `limits_with` writes seven bounds from seven `Int` parameters, so
> `max_chunk_size: chunk_line` still type-checks; the guard left is the naming
> convention, and it is asserted —
> `limits_with_pairs_each_bound_with_the_parameter_named_after_it` requires each
> written bound to take the parameter its own name is `max_`-prefixed from.
