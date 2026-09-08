# ADR 0042 — One code generator, and the order self-hosting goes in

**Accepted as a retirement and an ordering.** The Cranelift tier is removed:
`--backend c` is the only code generator, the emitter's own fixpoint is what
decides the compiled set, and the seam has one provider. The record also fixes
the order the self-hosting work goes in from here and the oracle each stage is
held to, because the tier was retired *for* that order and the reasons are the
same reasons.

> **What this decides.** That the shipping binary carries one code generator,
> emitted C, and that the analysis which decides what compiles is that
> generator's refusals rather than a dry run of another one. That the port of
> the emitter into Ply is held to **behaviour** — the suite under
> `--backend c`, the hazard tests, the backend differential — and not to the
> byte-exact form of the Rust emitter's output. That the Ply front end, lowering
> and emitter are entered from Rust as one chain, source bytes in and C text
> out, so that no Rust code ever reads a Ply value and no value bridge is
> written. And that `handle` in the compiled tier is re-priced against evidence
> passing before anything else on the evaluator is started.
>
> **What it does not decide.** What the runtime looks like once the machine is
> gone; that is the record after the effects design. Whether the inliner is
> ported or replaced by the C compiler's; the elision it enables is ADR 0040's
> finding and stands until the port has a measurement of its own.

## What the tier was for, and what it had become

ADR 0037 made Cranelift the loop's tier because a C compiler was priced at sixty
times its per-definition cost, and ADR 0040 kept the C tier "off the loop's
path" for the same reason. Both readings were taken before the emitted tier
kept anything between runs. Since then the emitted body is cached per
definition, the built unit is cached per program and shared across workers, and
the `development` profile compiles a unit with `tcc` or `cc -O0` in a fraction
of the time the readings priced. The premise moved and the placement did not.

Measured on 2026-09-07 on the machine in `docs/ONBOARDING.md` §Provenance,
running the self-hosted front end's own 156 tests through `ply test` with the
result cache off:

| engine | one run |
| --- | --- |
| no backend | 1.4s |
| `c`, unit cached | 1.6s |
| `c`, after adding one definition | 3.6s |
| `cranelift` | 5.6s, and 25s of CPU |

Cranelift recompiled the whole unit in every worker on every run, because a
JIT's code lives in the process that made it and nothing serialised it. The C
tier reads its unit back. So the in-process tier was the slower one in the loop
it was kept for, by the measurement the placement rested on.

Three things it cost that the readings never priced:

- **Every node three times.** ADR 0041 found that a construct entered the
  compiled set only if `Jit::refusals` accepted it, whatever tier emitted it, so
  `with cell` had to land in the interpreter, in Cranelift and in the C emitter
  together, and anything below it would have to. That is the tax this
  repository's goal of retiring implementations exists to stop paying.
- **It cannot be self-hosted.** Cranelift is a Rust library. An emitter written
  in Ply can produce C text; it cannot drive a Rust code generator without the
  Rust it exists to remove. The line ADR 0037 draws — a C compiler and libc,
  with everything above it in the language — has no place for it.
- **It was where CI's time went.** The longest job, `test cli`, spent most of
  its test time in four codegen tests that compile a corpus under both tiers,
  more than the whole of ply-cli's own suite. Those tests still run; they now
  compile one unit and read it back.

## What retiring it changed in the analysis

`Jit::refusals` was a dry run of the Cranelift function builder over every
candidate body, and `closure` iterated it to a fixpoint. The C emitter already
ran the same fixpoint of its own — emit everything, drop what refused, go round
again — and then discarded what the first analysis had decided. There are now
not two of these. `ply_codegen::closure` builds the unit over every function,
and the compiled set is what survived; the unit it leaves in the cache is keyed
on that same offered set, so every worker reads back the pre-flight's unit
rather than emitting one. The cached unit records its refusals, so a warm run
reports what a cold one did.

The compiled set can only have grown: a body the C emitter takes that Cranelift
refused was refused before and is compiled now. The suite under `--backend c`
and `--audit-backend` over `examples/` is the instrument that says whether any
such body is wrong, and it was green when this was written.

**What the hazard suite found the first time it ran against this tier.** The
hazard tests had only ever run against the in-process tier; pointing them at
the emitted one found four things, each fixed in the same change:

- A bitwise not over a fixed width emitted a cast with no type in it. The other
  tier had refused the node first, so the path had never been reached.
- The rule that keeps a credential out of the fragment's value arena lived only
  in the in-process tier's admissibility check. It is the emitter's now.
- **Compiled recursion could run off the native stack before the machine's
  budget stopped it.** The budget is a count of nested calls; an unoptimising C
  compiler gives every temporary its own slot, so ten thousand frames of a
  small body outrun a worker thread's two megabytes with no diagnostic at all.
  Two things close it. The prologue now refuses when the thread's stack is
  nearly out, failing the entry the way running out of fuel does, so the seam
  declines and the machine answers with its own bound; and a refused dive
  declines every deeper offer until the machine unwinds above it, since the
  machine re-offers each call it evaluates and a refused dive repeated once per
  level is the recursion squared. Beside that, the worker pool's threads get a
  stack sized for the budget, so the budget is the bound that fires and the
  floor is the backstop.
- A comparison or arithmetic over a `Float` or `Decimal` was refused at
  compile time, and the refusal cascaded through most of `std.db`. The
  in-process tier compiled the same bodies and failed every entry into them at
  run time, which the census could not see. The emitter now reaches the
  machine's own operator through the runtime for an operand whose type it does
  not fix, so those bodies compile and answer what the machine answers.

## The order from here, and the oracle at each step

The goal is a language whose compiler is written in itself, on a runtime that
is a C compiler and libc away from the machine, with one implementation of its
semantics. Each step below retires Rust or removes a duplicate, and each lands
behind an instrument that already exists.

1. **This record.** One code generator.
2. **The Ply emitter as the C backend's producer**, behind a flag, ratcheted on
   the suite passing under it rather than on bytes agreeing with `c/emit.rs`.
   **Built:** `PLY_C_EMITTER=ply:<dir>` installs it, `c/producer.rs` is the
   seam, and it answers a body as the pair the reference does -- the C and the
   tables it names -- in the cache's own encoding, so a body the port answers
   and a body the reference answers are kept and resolved alike. The
   differential compares that pair now, not the text alone.
   The byte-exact differential was the right instrument for every front-end
   stage because a syntax tree's canonical dump is a semantic form; C text is
   not, and the six "release rules" the byte-exact chase found were heuristics
   of one emitter reverse-engineered from its output. Byte-exactness stays for
   the one place it belongs: the fixpoint of the Ply emitter compiled by itself.
3. **The chain entered whole.** The front end, the lowering and the emitter are
   all Ply; entered once from the driver with source bytes in and C text out,
   nothing in Rust reads a Ply value, and the bridge `ply bootstrap` said was
   missing is not needed. The dumps remain what the differentials compare.
4. **Effects by evidence passing.** ADR 0041 declined `handle` because carrying
   it meant every effectful function becoming a state machine over a heap
   frame. That is the price of the naive design. The design Ply's own reference
   counting came from carries multi-shot handlers to C by passing the handler
   as evidence, calling a tail-resumptive clause as a plain function, and only
   on a clause that actually captures unwinding the callers into heap fragments
   that can be resumed any number of times. The shipped corpus has over a
   hundred `handle` sites and one clause that binds `resume`, a zero-shot
   rollback. The tier becomes total, which is the only thing that lets the next
   step be a deletion.
5. **Delete the machine.** The scheduler, regions, limits and continuations
   move into the runtime under the total tier; the CEK machine is not ported.
   An interpreter rewritten in Ply would keep two implementations of the
   semantics forever, checked against each other by the seam this record is
   working to remove.
6. **Delete the Rust front end**, since nothing reads the syntax tree.

The loop that is O(the change) for the *language itself* falls out of step 6
rather than being built: once the compiler's tests are Ply tests, content
addressing re-runs only what an edit touched. A Rust test suite cannot have that
property.

## What would make this wrong

- **If a body the C emitter takes and Cranelift refused is wrong.** The
  compiled set grew; `--audit-backend` over the corpora is the check, and a
  disagreement there is this record's, not the emitter's.
- **If the loop needs an in-process tier after all.** The remaining O(project)
  term in a backed edit is one unit's `cc`. ADR 0037 already picked the shape
  that removes it — an object per definition, one link over the reach — and it
  is emitted C either way. If that shape cannot be made cheap enough, the
  answer is `libtcc` in process, which is still one code generator, not a
  second one.
- **If evidence passing prices out.** Step 4 is a design record before it is
  code; if a handler that captures turns out to be common rather than rare in
  real programs, the cost lands on every effectful call and step 5 is re-read.
