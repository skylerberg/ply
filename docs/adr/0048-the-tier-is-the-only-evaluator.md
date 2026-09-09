# ADR 0048 — The tier is the only evaluator

**Accepted; supersedes ADR 0047.** ADR 0047 kept a fast in-process
interpreter as a second front end beside the compiled tier, the interpreter
running the bodies it could and *declining* the rest — multi-shot `resume`,
`simulate`, a region's tasks — to the tier. Building it surfaced the flaw the
record did not: **the interpreter is a second Rust implementation of the
evaluation semantics, and a second implementation diverges.** It diverged in
practice — the interpreter raised an internal error (`E0505`) on `with_cell`
combined with nested higher-order builtins where the tier answered cleanly,
and the combined audit missed it only because the corpus held no program of
that shape. A partial evaluator that declines does not remove the divergence
risk; it adds a decline seam on top of it. And the interpreter is *Rust* — the
very thing the self-hosting programme (ADR 0042) exists to retire.

> **What this decides.** There is one evaluator of the language, and it is the
> compiled tier. `ply test`, `ply run` and program execution run on it and
> only on it. There is no in-process interpreter of the language, no decline
> seam, no `--audit-backend` pairing of an interpreter against a compiler, and
> no `interp`/`combined`/`reference` backends. The CEK machine
> (`machine.rs`, `frame.rs`) is deleted; the shared runtime it drove
> (`cont.rs`, `window.rs`, `slots.rs`, the heap, regions, host, scheduler and
> the ADR 0044 stacks) stays, because the tier's own runtime is built on it.
>
> **The one exception.** A narrow **pure** evaluator survives, used only where
> there is no compiled body to run: the prover's `law` bodies over generated
> binders, a const the tooling reads, and — the reason it cannot be dropped —
> the **function values higher-order property testing synthesizes at test
> time**, which a compile-ahead tier can never apply. It is
> `ply_eval::interp::Pure`, the first-order language with local `with_cell` and
> tail-resumptive `handle`/`perform` and nothing else. It is not a second
> evaluator of the language; it evaluates expressions the tier never compiled.

## Why not keep the interpreter

Because the goals it served are better served without it. It existed for a
fast test loop with no `cc`. But it bought that speed by being a second
implementation, and the programme's fourth goal is *no diverging
implementations*. The cost it avoided — `cc` per program — is recovered
instead by making the tier's object cache amortize across builds: the cache is
keyed on the binary's own stamp today, so every CI build is cold; content-
addressing it on the program and the runtime digest makes an unchanged program
a cache hit. That is an engineering problem with a known fix, and it does not
cost a second evaluator that can be wrong.

## Why not make the interpreter complete

A complete in-process evaluator needs a reified continuation to run multi-shot
`resume` and a region's scheduler — which is exactly `machine.rs`, the CEK we
deleted. Completing the interpreter means resurrecting the second continuation
model ADR 0044/0047 set out to remove. The tier already has one continuation
model, native and compiled; a second, interpreted, is the divergence again.

## What would make this wrong

* A program shape the pure prover applier is asked to evaluate that is not
  pure and first-order — a law that performs an external effect, a generated
  value that is not first-order. `Pure` refuses these by construction rather
  than guessing; the refusal is a diagnostic, not a silent wrong answer.
* The cache staying cold. If the object cache is not made to amortize, the
  test loop pays `cc` per program and the fast loop the interpreter bought is
  lost with nothing recovering it. That is the load-bearing follow-up, not an
  optional one.
* The port emitter differential — the reference C emitter against the Ply one
  — is now the code generator's only oracle, since the machine it used to be
  paired against is gone. It must stay, and its corpus must be wide enough to
  catch a generator that is wrong on a shape the served examples do not reach.
