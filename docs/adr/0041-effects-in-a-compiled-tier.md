# ADR 0041 — Effects in a compiled tier

**Accepted as a staging, not as a build.** Both compiled tiers refuse all four
effect constructs today, and this record says which of them can be carried as
*calls*, which cannot be carried without changing the tier's calling convention,
and why the split falls where it does. It authorises the first two and declines
the third.

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

What is left is the one hard part: a region opened at the node has to be closed
on **every** exit, and a compiled body returns early from any helper that can
raise. The interpreter closes it with a stack frame, which C has no equivalent
of, so this has to be reconciled by the context at the end of a failed entry
rather than by the emitted code.

## The decision

Staged, in this order, each landing behind the differential the tier already
has:

1. **`with cell`**, whose remaining problem is closing a region on the early
   returns, not the cell itself. Independent of everything below.
2. **`perform` under the whole-program criterion**, with the binding threaded
   into `Ctx` and a refusal, naming the handler it found, when the criterion
   fails.
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
cost only that body, the coverage motivation goes with it, and stages 1 and 2
should be re-priced against what else the tier is refusing.
