# The Ply front end's port to Ply: where it stands, and what is next

Written at the end of a long working session. `README.md` says what the spike is;
this says what the *port of the C emitter* has and has not, so the next pass
starts from the findings rather than rediscovering them.

## The two instruments, and what they say

Both are in `harness/tests/`, both run from `./run.sh`, both ratchet.

| | |
| --- | --- |
| `lower_diff.rs` | the port reaches **1179 of 1200** bodies, **1179 compared** |
| `emit_diff.rs` | 48 of 48 hand-written bodies; **842 of 1282** shipped bodies |

`reached` and `compared` were apart for most of this work, by the record updates
the lowering excluded. They now meet: the exclusion is gone.

The emitter differential pins the reference's inlining at **zero** and passes it
explicitly. What is compared is the *emitter*; the inliner is a stage of its own
and this port does not have it.

## Two pieces written and switched off, with what they cost

Both are in `emit.ply`, both reached by deleting a `None ->` and restoring the
body recorded beside it.

- **`field_of`** — the runtime's field read by name, for a base whose shape is
  not known. Reaches **565**, with **24** disagreeing.
- The **generic** half of the shape work is on. What is left of the 24 is *not*
  about releases, which is the change from earlier in the session: the reference
  reads at an offset where the port asks by name, from a shape it has and this
  port does not.

## The rules the corpus taught, all recorded at their site

Releases, in the order they were found. Each was a real difference:

1. A release counts how many times the tree reads **any** name for the object,
   not how many times one slot is read.
2. A field whose own kind is a **word** does not release its base -- and the
   generic read, which answers a word, does not either.
3. An update emits its own build, out of the base's memory, released first.
4. An update at the **tail** releases its base however often the body reads it.
5. An arm reached through a scrutinee is **still a tail**.
6. The record an update builds is the body's own.

Ownership and ordering:

- A compiled call takes **one** pass over its arguments; a builtin takes **two**;
  `map` and `filter` take one. Each is wrong in the others' place.
- An aggregate's items are held as they are evaluated; a call's are not.
- Parameters are opened at entry, in declaration order, whatever the body reads.
- The unit's tables are the **unit's**: a lambda's constants, shapes and builtins
  keep counting from what the function that builds it met.

## The named gaps

`emit_diff.rs` asserts the twelve by name. Four `std.hash` bodies want the
*deferred record local*, which is the half of deferring this port does not do: a
record whose every read is answered from the built table is never materialised,
and the local it would land in is declared at the top of the body. The other four
have causes written beside them.

## What is not started

The **inliner**. The differential pins inlining at zero precisely because of it,
and porting `opt.rs` is what lifts that pin.

## What changed after this was written

**The release family, for the next pass.** Seven named gaps are one rule: the reference
releases a record -- an update's base, a let-bound one, a parameter before the result is
built -- where the port does not. Both sides state the same four guards (an owned bare
variable, at most once per binding, exactly one read of the object unless at the tail). The
reference keys "at most once" and "one read" on the *C local's root*, charged when a name is
bound; the port keys them on the *slot*, charged over the whole body. `agreement.memory_step`
is the smallest case: the reference releases once in each of four `match` arms and the port
in none. Start there, with `PLY_EMIT_DIFF_SHOW`.

**`&&` and `||`.** The port emits the short-circuit operators as the reference does, the
right operand inside the branch: 789 to 842. The three bodies that opened differ on the same
release rule as the others in the named gaps -- seven now, one family, the next pass.

**The constant table.** A pure nullary definition answering a word is asked of
`rt_constant` as the reference asks it; purity is the checker's word, through
`infer.pure_definitions`, so the port runs its checker over the program before it emits.
The body's code table (`Em.lambdas`) holds the constants' entries beside the lambdas', and a
closure names its code by the entry's place in it. A `let` keeps its value's whole type now,
as the reference does, and a type of another module is a word. That was 623 to 789. What it
opened, three bodies that differ on the port's release rules and on declared widths, is in
the named gaps.

**A refusal carries its reason now.** `Em.refused` holds the first reason a body was refused;
a refusal poisons the state and emission runs out, so no signature changed. `emit_fn_why`
reads the reason, `emit_refusals_all` lists every refused body of a program, and the census
test in `harness/tests/emit_diff.rs` aggregates them by reason over the bodies the reference
emits. Read that census before choosing what to build next: as this was written it put a
nullary call of a definition first (the constant table wants the purity the checker
publishes), then a field read over a shape the port cannot see, then a pattern it cannot
test, then an operator it does not emit, then a parameter with no written type.

`std.json.float_json` is gone from the gaps: a lambda's emission restarted the body's
constant and shape tables instead of continuing them, which the tables differential
below is what caught.

The port emits a **whole program** now (`emit_all_program`): every module parsed, rewritten,
its derives expanded and then resolved together, so a call's default arguments are filled
and a callee's declared type is in reach across modules -- which is what the reference's
emitter sees. That closed `std.http.check_limits` (a default argument's constant) and
opened the **test roots**: a test is a nullary body named `test#<n>`, and the port emits
them as the reference does. With an expression statement's value discarded and two `Int`
literals under an operator folded as the reference folds them, the port went from 487 to
605 shipped bodies. The census in `emit_diff.rs` says what keeps it out of the rest: field
reads over shapes it cannot see, lambdas, and updates, in that order.

The port is the C tier's **producer** now, behind `PLY_C_EMITTER=ply:<dir>`
(`crates/ply-codegen/src/c/producer.rs`). `emit_fn_full` answers the text and the
tables a body names, `emit_bodies_in` frames every body of a module by length,
and the tier reads that where it reaches and emits the rest itself. The third
emitter differential holds the tables to the reference's wherever the text agrees.

The port reads **opaque** operands now: a written `Float` or `Decimal` is `TyOpaque`, a
builtin over an opaque argument answers opaque, opacity survives a `let`, and an operator
over one goes through `rt_binary_p` -- the machine's own operator -- as the reference does.
An `if` whose one arm is opaque joins at a word; every other join of differing kinds is
still refused. What is deliberately *not* kept across a `let` is the rest of a value's
type: the reference keeps it whole, and keeping a record type here reaches two more bodies
(`std.hash.round`, `std.http.method_not_allowed`) that then differ on declared `U32` field
widths and on an update's release -- the next two shapes to run down.

ADR 0042 moved the emitter's oracle. The byte-exact differential above stays as
an instrument, but the port is no longer held to agreeing with `c/emit.rs`'s
text: it is held to the suite passing under it as the C backend's producer, and
to the fixpoint of compiling itself. The two switched-off pieces and the named
gaps are therefore things to *make work*, not things to make byte-identical, and
the release rules above are the reference emitter's heuristics rather than
requirements. Read that record before the sections above.

## Beyond the port

`docs/adr/0041-effects-in-a-compiled-tier.md` carries the effects work: `with
cell` ships in both tiers, `perform`'s whole-program criterion is built
(`Source::stack_handled`), and answering a `perform` needs a decision about the
seam rather than more code -- `Compiled::enter` takes `&self` with the machine
already borrowed.

The evaluator is not started, and the thing to settle before it is what oracle it
is held to. Every error found here was found by a byte-exact one.
