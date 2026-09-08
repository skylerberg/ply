# ADR 0043 — Effects by evidence passing in the compiled tier

**Accepted, and the first three stages are built.** ADR 0042 ordered the
self-hosting work and placed effects fourth: `handle` in the compiled tier is
to be re-priced against evidence passing before anything else on the evaluator
is started. This record is that pricing and the design the tier took. It does
not start the fifth step.

**Built:** the runtime's handler frames, `handle` with tail-resumptive clauses
and a `return` clause, the zero-shot `resume` as an unwind, `with cell`, the
per-operation rule in the unit's fixpoint, and the performed atoms crossing the
seam into the machine's trace. The emitter written in Ply carries every one of
them, the reference none; they compile with the chain entered whole
(`PLY_C_EMITTER=ply-whole:<dir>`), and the audit is green over the standard
library and the examples in that mode. The host route is not built: a
`perform` no frame answers is refused before it is compiled, by the rule below,
so nothing reaches the runtime's "no handler" failure from a shipped program.
What the tier refuses over the shipped corpus now is a `perform` of an
operation the run's host binding would answer, a `resume` called off the tail,
and their callers; `simulate` and `task.*` were refused when this was written
and are ADR 0044's first stage, built. The `Float` and `Decimal` literals the
reference never carried are constants the runtime holds, and compiling their
bodies found the rule the inline `Int` arithmetic had wrong on both sides: an
operator over two words of a type the emitter cannot see goes through the
runtime, which is the one that knows.
Two things the building found are in the design below where they belong: a
name the runtime reads out of the unit's table is written as the placeholder
the unit resolves, and a produced body's cache key carries the emitter's own
identity.

> **What this decides.** That a handler in the compiled tier is a *frame the
> runtime keeps*, installed by the `handle` site and searched by the `perform`
> site, so that a tail-resumptive clause is a plain call on the performer's
> stack and nothing in a C function changes shape for it. That a clause which
> binds `resume` and never calls it -- the shipped corpus's one case -- is an
> unwind to its `handle` through the failure path every call site already
> checks. That a clause which calls `resume` other than in tail position, or
> more than once, is refused by the tier until the fifth step's runtime gives
> it heap frames, and that this is a refusal of *bodies* the machine still
> runs, not of the language. That a compiled `perform` is sound exactly when
> every `handle` with a clause for its effect is in the compiled unit, which
> the unit's fixpoint decides per effect, and that a `perform` nothing on the
> stack answers reaches the host through a route the runtime context carries.
>
> **What it does not decide.** What the runtime is once the machine is gone;
> that is the record after this one. Whether `simulate` is carried; it opens a
> region and schedules tasks, and stays the scheduler's. Whether multi-shot
> resumption is restricted; it is not, and the tier's refusal of it is a cost
> paid by the machine for now, measured below.

## The semantics being carried

`docs/GUIDE.md` §7.6 and §7.7 say what a handler means, and the machine in
`crates/ply-eval/src/handler.rs` is the reference for it:

- `handle <body> with { clauses }` discharges atoms. A clause is
  `effect.op[resource](params) -> body`. A `perform` walks the handler stack
  innermost-out for a frame with a clause for its effect, operation and
  resource, and the first match answers.
- A clause with no `resume` is **tail-resumptive**: its body's value goes
  straight back to the perform site. The machine still captures the extent
  between the perform and the handle and splices it back, because that is one
  mechanism for every clause; the value the performer sees is the clause
  body's, and nothing else observes the capture.
- A clause with `resume k` binds the delimited continuation. Its body has the
  whole `handle` expression's type, and may invoke `k` zero, one or many times.
- An optional `return x -> body` clause transforms the body's value.

Over the shipped corpus, counted where the design was written:

```sh
grep -c "handle " crates/ply-std/ply/*.ply examples/*.ply | awk -F: '{s+=$2} END {print s}'
grep -c "resume" crates/ply-std/ply/*.ply examples/*.ply | awk -F: '{s+=$2} END {print s}'
```

| sites | count |
| --- | --- |
| `handle` | 101 |
| clauses binding `resume` | 1, `std.db.transaction`'s rollback, which never calls it |

Every other clause in the tree is tail-resumptive. That is the whole reason the
design below is cheap: the common case is a call, and the one uncommon case is
an unwind the tier already knows how to do.

## Why not the state-machine transform ADR 0041 priced

ADR 0041 §"Why `handle` is not a node you add to an emitter" is right about what
multi-shot resumption costs a C frame: a continuation that can be spliced onto
any stack at any height cannot be a C frame, so carrying it *everywhere* means
every effectful function becoming a state machine over a heap frame. That
transform slows every effectful body whether or not any handler ever resumes
twice, and the census above says no shipped handler does.

Evidence passing is the other design, the one Ply's own reference counting came
from. The handler is passed to the callee as a value -- here, kept on a stack
the runtime context carries, which is the same thing under dynamic scope -- and
a clause that does not capture is called like any function, on top of the
performer's stack. Only a clause that *does* capture pays for it, and only that
clause's `handle` needs its callers as heap frames. The tier can refuse exactly
those bodies and carry every other, which is what makes the next step a
deletion rather than a rewrite.

## The runtime

`Ctx` gains a handler stack. Each frame records the effect, the resource label
it was written with (or none), a table of clauses -- operation name, the
closure the clause's body became, whether it binds `resume` -- and the closure
of the `return` clause when there is one. Four helpers carry it:

- `rt_handle_push(ctx, frame)` and `rt_handle_pop(ctx)`, emitted around the
  `handle` body. The body is emitted inline, in the enclosing function, as
  `with cell`'s body is today.
- `rt_perform(ctx, effect, op, resource, args)`. Innermost-out over the stack,
  the first frame whose effect and resource match and whose table has the
  operation answers. A tail-resumptive clause is called through `rt_call` with
  the operation's arguments and its value is the perform's value; the frame
  stays installed while the clause runs, because the clause's own row is part
  of the `handle`'s row (§7.6's typing rule) and a perform from inside it must
  find the frames *below* -- so the search for that inner perform starts below
  the answering frame, which is what the machine's capture-and-splice does
  implicitly. A clause that binds `resume` is called with `k` as a token whose
  only compiled use is the tail position; if the clause returns without
  calling `k`, the value it returns is the `handle` expression's, and
  `rt_perform` sets the context's failure code to *unwinding to frame `n`*
  with that value and returns. Every call site already ends in
  `if (ctx->failed) return 0;`, so the unwind rides the failure path to the
  `handle` site, whose landing check clears it and takes the value.
- A `perform` no frame answers goes to the host through a route the context
  carries: the binding the run was started with, the same object the machine's
  `perform_host` consults, behind a trait the runtime crate defines and the
  CLI installs when it builds the backend. ADR 0041 §"What it costs" is the
  inventory of what that route needs -- the binding, the hermetic and
  footprint checks, and a span to hang a diagnostic on -- and each of them is a
  field on the frame or the context rather than a reason to refuse. Until it
  is built, the fixpoint refuses a performer of an operation nothing in the
  program handles, and one of every operation the run's host binding would
  answer -- the driver hands the producer that list when it builds the
  backend -- so a compiled `perform` never has to reach the host, and the
  runtime's failure for the unhandled case is the machine's own diagnostic for
  a compiler defect.
- Every `perform` records its atom on the context, and the backend hands the
  atoms across the seam after each entry for the machine to record in its
  trace: the observed row is a claim the tests make, and a handled perform is
  still a perform. Three tests in the standard library said so before the
  seam carried them.

`simulate` is not carried and neither is a `task.*` operation; a body holding
either stays the machine's until the fifth step.

## The emitter

The emitter written in Ply is the one that grows: the reference is being
retired and ADR 0042 holds the port to behaviour, so a construct the reference
refuses and the port emits is compared by the audit alone, and only the chain
entered whole (`PLY_C_EMITTER=ply-whole:<dir>`) compiles it.

- `handle`: each clause body and the `return` body become functions of their
  own, as lambdas do, closed over what they capture; the site pushes the frame,
  emits the body inline, pops, and applies the `return` closure to the body's
  value. The landing check follows the pop.
- `perform`: the arguments are evaluated and held, the effect, operation and
  resource are the unit's interned names, and the value is `rt_perform`'s.
- A clause that binds `resume` and calls it anywhere but as the tail of its
  body, or more than once, refuses the body that holds the `handle`.
- The tables a body names gain two lists: the effects it performs and the
  effects it handles. Both are in the cache encoding, and the harness reads
  them as it reads the others.

## The criterion, and where it is decided

A compiled `perform` searches the compiled handler stack. It is sound when that
stack is complete for the effect: no interpreted frame can hold a handler for
it. The seam is one-directional -- compiled code never runs an interpreted
closure, `rt_call` fails on one -- so the only interpreted frames above a
compiled perform belong to bodies the unit refused. The criterion is therefore
**per effect, over the unit's fixpoint**: a body performing `E` is taken only
while every body holding a `handle` with a clause for `E` is taken. When the
fixpoint drops a handler body for any reason, it drops every performer of `E`
with it, and the reason names the handler. That generalises ADR 0041's
`Source::stack_handled`, which asked the same question of the whole program
with every `handle` refused.

## Staging, and the oracle at each stage

1. **The runtime helpers and the tail-resumptive `handle`**, with the per-effect
   criterion in the fixpoint. Held to `--audit-backend` over the standard
   library and the examples with the chain entered whole, and to the hazard
   suite. The census of what keeps the port out of the reference's bodies is
   already empty; a census of what the tier refuses over the whole program,
   printed by the same instrument, is what ratchets from here.
2. **The zero-shot `resume` clause**, by the unwind above. `std.db.transaction`
   is the case, and its tests are the oracle.
3. **The host route.** A `perform` no frame answers reaches the binding. The
   suite's host tests under `--backend c` are the oracle, and the hermetic
   diagnostics have codes (`docs/GUIDE.md` §7.8) the tier has to raise
   verbatim.
4. **The measurement the fifth step needs.** With the three above, the tier's
   refusals over the shipped corpus are `simulate`, `task.*`, a `resume` that
   captures, and the literals the reference never carried. That list is what
   the runtime after the machine has to answer, and it is short enough to be
   answered by design rather than by porting the machine.

## What would make this wrong

- A shipped handler that resumes more than once, or after its clause returned.
  The census says there is none; a program that writes one runs on the machine
  until the fifth step, and the tier says so by name.
- A clause whose row is performed *by the clause itself* and expected to be
  answered by the frame it belongs to. §7.6's rule puts a clause's row outside
  the atoms the `handle` discharges, so the search from inside a clause starts
  below its own frame; a program relying on the other reading would be relying
  on something the machine does not do either.
- The host route turning out to need the machine after all -- a binding that
  answers by running interpreted code. The bindings in `crates/ply-host` are
  Rust; if one is found that is not, this record's third stage is where it is
  priced.
