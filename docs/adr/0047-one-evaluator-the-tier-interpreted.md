# ADR 0047 — One evaluator: the tier, interpreted

**Accepted; the build is a ratchet, and this records where it starts.** ADR
0045 planned to delete the CEK machine with its consumers switched to a
facade over the compiled tier. Measuring the switch found the obstacle that
record did not: the machine is not only the production evaluator's
predecessor, it is the *fast* evaluator the Rust test suite runs on. A tiny
program is `0.02s` on the machine and a `~5s` cold C compile on the tier,
and the tier's object cache is keyed on the binary's own stamp, so every CI
build is a cold cache. Routing the suite's ~240 programs through the tier
would add minutes to every CI run — against the whole point of the
programme. So the fifth stage's "facade over the tier alone" is wrong as
written: deleting the machine and keeping only the compiling tier trades one
stated goal (one implementation) against another (a fast loop).

> **What this decides.** That there is one evaluator, and it is the tier —
> but the tier gains a second front end. The compiled front end lowers a
> body to C and runs it; the **interpreted** front end walks the same
> lowered `Code` over the same runtime `Ctx`, calling the same `rt_*`
> helpers, with no C compiler in the path. One lowering
> (`ply_eval::code`), one runtime (the heap, regions, host, scheduler and
> the ADR 0044 stacks that carry `resume` and `simulate`), one continuation
> model, one emitter written in Ply. The CEK machine — `machine.rs`,
> `cont.rs`, `window.rs`, `frame.rs`, `slots.rs` and the `reference`
> backend that nests a machine — is deleted once the interpreter reaches
> it, and `ply-eval` keeps only the runtime the two front ends share.
>
> **What it does not decide.** The compiled front end stays exactly as it
> is; this adds a front end, it does not change the tier's C. The reference
> emitter in Rust and its byte-exact differential against the port stay as
> the code generator's oracle, independent of which front end runs a body.

## Why interpret the tier's lowering rather than keep the machine

The machine and the tier already share the runtime ADR 0044 built: the same
`Value` model behind the heap, the same regions, the same host boundary,
the same scheduler and search, the same native stacks for off-tail and
multi-shot `resume`. What the machine has of its own is a *second* way to
hold a continuation — `cont.rs`'s explicit continuation objects and
`window.rs`/`frame.rs`/`slots.rs`'s activation windows — beside the tier's
stacks. Two continuation models is the divergence the programme set out to
remove, and it is the part that has to be re-derived every time an effect's
semantics move.

The interpreter removes the second model rather than the second evaluator.
It is a Rust transcription of what `spikes/ply-parser/emit.ply` emits, node
for node, but producing `Word`s in the runtime's heap through the `rt_*`
helpers instead of C text: a `Binary` does what the emitted arithmetic
does, a `Perform` calls `rt_perform`, a `Handle` pushes a frame with
`rt_handle_push` or opens a detached one with `rt_handle_detached`, a
`Simulate` calls `rt_simulate`. Because it drives the same runtime by the
same calls the compiled code makes, it answers what the compiled code
answers by construction, and where it does not, the audit says so.

## How it is built: the port's method, and the port's oracle

The interpreter is a `Provider` in `crates/ply-codegen`, beside `Unit` (the
compiling tier) and where `ply-eval`'s `Fragment` (the nested-machine
`reference` backend) is today. It implements the `Compiled` seam: a body it
reaches is entered and answered; a node it has not yet transcribed makes the
body **decline**, exactly as the emitter port refuses a form it has not
reached. `--audit-backend` already pairs a backend against the machine and
checks the answer and the simulation record agree per test; run with the
interpreter attached, it is the coverage ratchet and the oracle at once —
every body the interpreter enters is checked against the machine, and the
count of bodies it enters may only rise. `tests/lang`, the three corpora and
the served examples are the acceptance set, run interpreted as they are run
compiled.

When the interpreter enters every body the machine does — the ratchet's
ceiling — the machine becomes the thing with no caller. Then, in one change:
the CEK modules and the `reference`/`Fragment` backend go, the `reference`
backend name becomes the interpreter, `--audit-backend` retires (its two
engines were the machine and a backend; now both engines are the tier's two
front ends and the differential of record is the emitter's), and `ply-eval`
is the runtime the two front ends share. The consumers — the harness, the
CLI, the prover, the corpus — hold an engine that is one of the two front
ends and never construct a `Machine`.

## What would make this wrong

* An interpreter that drifts from the compiled front end on a body both
  reach. The audit is total over what the interpreter enters, so a drift is
  a failure, not a silent fork; the risk is a body the interpreter declines
  and the audit therefore never compares, which the ratchet's rising floor
  is there to squeeze.
* A per-node cost that makes the interpreter slower than the machine it
  replaces. The machine's `0.02s` is the bar; the interpreter walks the
  same tree the machine's lowering produced, over the same runtime, so the
  cost is a constant factor on the walk, not a new algorithm. It is
  measured on the same programs before the machine is deleted.
* A runtime that turns out not to be shared after all — a helper the
  compiled code calls that the interpreter cannot, or the reverse. `rt.rs`
  is the surface both use; a helper that only one can call is the seam to
  fix before the deletion, not after.
