# ADR 0041 — Effects in a compiled tier

**Accepted as a staging. Stage 1 is built: the compiled tier carries `with
cell`.** Every
effect construct was refused when this was written. This record says which of
them can be carried as *calls*, which cannot be carried without changing the
tier's calling convention, and why the split falls where it does. It authorises
the first two, declines the third, and records what building the first one
taught.

> **What this decides.** That `with cell` and a `perform` that provably reaches a
> host handler are compiled, because both are calls: they return a value to the
> frame that asked, and no continuation is captured. That the criterion which
> makes the second one sound is a **whole-program** property, checked where the
> unit is built. That `handle` stays refused, and that a body containing one
> falls back exactly as it does now.
>
> **What it does not decide.** That `resume` is restricted; the language keeps
> multi-shot resumption and ADR 0034 is untouched. That effects are slow, or
> that this is where the tier's time goes — see §"What this is not motivated by".
> That `simulate` is carried; it opens a region and schedules tasks, which is the
> scheduler's, not the emitter's.

## What this is not motivated by

**Effects are not what the compiled tier is losing.** Run the largest effectful
example under `--backend c` with `PLY_C_REFUSALS=1` and a cache directory of its
own, and the tier takes almost all of it — and **not one** of the refusals is an
effect construct. They are cascades from a module sitting outside the compiled
unit, plus a few the type fragment does not fix. The command is the measurement:

```sh
PLY_C_CACHE=$(mktemp -d) PLY_C_REFUSALS=1 ply test examples/desk.ply --backend c --no-cache
```

`spikes/ply-parser/harness/tests/effects.rs` is the other half — the census of
how many shipped bodies reach each construct, printed by the spike's `run.sh`.
It is a small share of bodies, and `handle` is the rarest of the three that
appear at all.

So this record is not a performance argument, and §"Measure an ADR's motivating
claim" does not have a claim to price here. It is a **coverage** argument: a
refused body poisons every caller in its unit, so the constructs are worth
carrying for the cascade they stop, not for the bodies they are.

## Why `handle` is not a node you add to an emitter

`resume` is multi-shot (`docs/GUIDE.md` §7.7). ADR 0034 states the consequence
for the interpreter: a frame holds no scope, and every frame records only
quantities relative to the top, so a captured extent can splice onto any stack at
any height without a frame being rewritten.

A C function's frame can do none of that. Carrying `handle` means every function
with a non-empty effect row stops being a C function and becomes a state machine
over a heap frame — the tier's calling convention, not its emitter. That is a
larger change than the tier itself was, it slows every effectful body whether or
not any handler ever resumes twice, and the census says it would be bought for
the rarest construct of the three.

**The rejected alternative, so it is not re-proposed.** Let `rt_perform` walk the
handler stack at run time and, on finding an interpreted `handle`, abandon the
compiled frame and re-run the call interpreted. It is unsound: the compiled body
may already have allocated, mutated a cell, or performed another operation, and
re-running repeats all of it.

## The criterion that makes `perform` a call

A `perform` walks the stack for a handler and, finding none, is answered by the
host: the handler runs and **returns**, which is a call. So a `perform` is
compilable exactly when it cannot find a stack handler, and that is decidable as
a whole-program property — no `handle` clause for the effect anywhere in the
program, no `with cell` for the resource, and not a `task.*` operation, which a
`simulate` answers by opening a region.

It is sound against an interpreted caller, which is the case worth stating: a
compiled body can be called from inside an interpreted `handle`, but if the
program contains no handler for the effect, no interpreted frame can be one.

The criterion is checked where the unit is built, which is where the program is
already known, and a body that fails it is refused with the reason.

## What it costs, which is not nothing

`Ctx` has no route to the machine. It is built once per backend from the tables
alone, and answering a `perform` needs the host binding, the hermetic and
footprint checks, and a span to hang a diagnostic on. Threading that in is an
API change between `ply-eval` and `ply-codegen`, and it is most of the work this
record authorises — the emitter's part is one node.

**`with cell` needs almost none of it, and the reason is worth stating** because
it is not the reason one would guess. A cell is not reached through the handler
stack at all: `with cell` allocates one and binds a first-class `Cell` value,
and `cell_get`, `cell_set` and `cell_update` are *builtins*. A compiled body can
therefore already use a cell it is handed — what is refused is only the node that
creates one. `Ctx` already owns the arena that would allocate it, and the region
kind a site opens is a per-program analysis (`region_kind::infer`), so it is
known where the unit is built and can be emitted as a constant.

**Closing the region on every exit turned out not to be the problem it looked
like.** A compiled body returns early from any helper that can raise, and there
is no frame to hang an unwind on, so the close is emitted on the success path
only. A failed body therefore leaves the region open — and that is *already* the
condition the seam declines on, so the machine answers the call itself, which is
what a failure does anyway. The missing close costs a re-run that was happening
regardless.

What that needs is a seam that measures a **balance** rather than a total. The
gate it replaced asked whether the entry had touched the arena at all, which no
body opening a cell can pass; the arena's depth and live-slot count both come
back down, so a `with cell` that ends is balanced and a region left open is not.
The gate that used to refuse `cell_get` and `cell_set` outright goes with it:
the only cell either can be handed is one a `with cell` in the same body opened,
because a cell is not a crossable argument.

**And the finding that decided where the work went: the fragment was the
in-process tier's.** A definition entered the compiled set only if that tier's
dry run accepted it, whatever tier would emit it — so implementing a construct
in the emitted-C tier alone changed nothing, and `with cell` had to land in
both. ADR 0042 retired that tier for this reason among others; the compiled set
is now the emitter's own fixpoint.

## The decision

Staged, in this order, each landing behind the differential the tier already
has:

1. **`with cell`** — **built.** Three runtime helpers, a node in each tier, and
   a seam that measures the arena's balance instead of refusing anything that
   touches it. Independent of everything below.
2. **`perform` under the whole-program criterion.** The criterion itself is
   built -- `Source::stack_handled` walks every definition's lowered body and
   answers whether an operation could find a handler rather than the host, which
   is what makes the host case a call. What is not built is answering it, for
   the reason above: the first step is a decision about the seam, not a helper.
   This is where the definitions are, a dozen directly and most of `http`'s
   cascade behind them.
3. **`handle`** — **not taken.** Reopen it when a body containing one is what a
   measurement shows the tier losing, and price the state-machine transform
   against the interpreter before writing any of it.

## What would make this wrong

That the criterion is too coarse: a program with one `handle` for one effect
refuses every `perform` of that effect, including those that could never reach
it. A per-effect criterion is what is written above; a per-*call-site* one would
need the call graph, and if refusals cluster on programs that have a handler
somewhere, that is the next thing to build rather than stage 3.

That the cascade argument is wrong: if refusing an effectful body turns out to
cost only that body, the coverage motivation goes with it, and stage 2 should be
re-priced against what else the tier is refusing. The instrument prints that
comparison every run, and the largest single reason in the library is not an
effect at all -- it is the `Decimal` literal, which costs nine definitions and
thirty-four test roots and is nothing this record covers.

That the balance is too weak a check. It says the arena was given back; it does
not by itself say no cell *escaped*, and what stops that is a separate refusal of
an answer holding one. If a third route out of a compiled body is ever added —
another handle-shaped value, another way to hand a word to the machine — the two
have to be re-read together, because neither is sufficient alone.
