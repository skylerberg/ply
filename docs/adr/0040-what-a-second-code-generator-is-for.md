# ADR 0040 — What a second code generator is for

**Accepted as a build and a placement. Ply has a second code generator; it does
not go in the loop, and it does not clear ADR 0035's bar either.** What it does
is answer the question ADR 0039 left open, and the answer is not the one that
record predicted.

ADR 0037 listed the candidates for a tier and named what would choose among
them. This builds the strongest one — emitted C, one `cc`, one `dlopen` — and
runs it against the same gate, the same kernels and the same interpreter.

> **What this decides.** That `--backend c` exists and is checked: it emits the
> same lowered `Code` the Cranelift tier lowers, and it is held to the
> interpreter's answers, to the other tier's, and to BLAKE3's published vectors.
> That it is **not** the loop's tier — ADR 0037 priced the toolchain per
> definition and this build confirms it whole. That it **is** the tier the
> emitted code is read through, which is not a small thing: every defect fixed
> in the week this took was found by reading its output.
>
> That the two remaining candidates in ADR 0037's table — libtcc, copy-and-patch
> — are not built next, because this build removes the reason to. The thing a
> second code generator was going to buy is *better code*, and it does buy it,
> and better code is not what the bar is waiting on.
>
> **What it does not decide.** That C is the release target; ADR 0021 and
> ADR 0037 carry that direction and this record does not re-take it. That
> Cranelift is replaced; nothing here displaces it.

## What was built

`crates/ply-codegen/src/c/` — an emitter over the same `Code`, a prelude that
mirrors `heap.rs`'s layouts, a table of the runtime helpers a body may call, and
a loader. A unit is one C translation unit, one `cc -O2 -fPIC -shared` and one
`dlopen`; the runtime hands the loaded image its helper addresses through
`ply_bind`. The admitted set is a fixpoint exactly as the other tier's is: a
body is taken when it emits *and* everything it calls is taken, so a compiled
set cannot call out of itself.

It is checked three ways, because a second code generator is a second chance to
be wrong. `the_tier_answers_what_the_interpreter_answers` and
`the_two_tiers_agree` run the same definitions through both engines; `std.hash`'s
published vectors pass under `--backend c`; and the refusals are themselves
pinned, so a construct the tier cannot get right is declined rather than emitted
—
`a_width_the_tier_cannot_carry_is_refused_rather_than_answered_wrongly`.

## The code it emits, read against the bar

`round` is BLAKE3's inner body and the one ADR 0036 and ADR 0039 both measured.
Taken from the loaded images with `otool`, per call:

| | Rust bar | C tier | Cranelift |
| --- | --- | --- | --- |
| instructions | 139 | **321** | 706 |
| calls | 0 | **0** | 0 |
| `add` | 48 | 51 | |
| `ror` | 32 | 32 | |
| `eor` | 32 | 32 | |
| `lsr` | 0 | **32** | |
| loads | 17 | 49 | |
| stores | 9 | 21 | |

**The arithmetic is at parity.** BLAKE3 wants 32 rotates, 32 xors and about 48
adds per round, and the C tier emits exactly those, within three instructions of
what `rustc` emits for the same eight `g` calls. Whatever else is true, this
tier is not losing on arithmetic and neither is its register allocator.

The excess is 182 instructions and it is itemised in the table. Thirty-two
`lsr` are the tag coming off, once per field read. The extra thirty-two loads
and twelve stores are the record going out to memory and coming back. There is
nothing else — no masks, no checks, no spill traffic that the bar does not also
have.

**So the cost of Ply's value model, on the body that dominates the integer
kernel, is 182 instructions per round: 131% on top of the arithmetic, and all of
it the tag and the box.**

## Which corrects ADR 0039

ADR 0039 concluded that *"the binding constraint is Cranelift's register
allocator over a body with thirty-two live values"* and that *"the
representation was never what stood between this kernel and its bar."* Seven levers were tried and all seven ended at the allocator, which made
the conclusion look forced.

It was the wrong conclusion, and this tier is what shows it. Clang's register
allocator is not Cranelift's: handed the same body it emits 321 instructions
where Cranelift emits 706, less than half. If the allocator were the binding
constraint, halving its output would move the kernel. **It does not move the
kernel** — §"What the gate said, this time" below. The 2.2× on the body buys
nothing at the gate.

What ADR 0039 got right is that the levers it tried were exhausted. What it got
wrong is the inference from that: the levers were being pulled on a body that is
about a third of the kernel, and the constraint is not in that body's register
pressure but in its interface — the tag on every word crossing it and the heap
record it is read from and written to.

The LLVM probe that record leaned on measured the same third, in a loop where a
sixteen-field record stays in L1 and is never allocated. It is a good instrument
for what it measures and it was read as saying more than it did.

This is the failure mode `CONTRIBUTING.md` §"Measure an ADR's motivating claim
before accepting the ADR" already has a worked example of, and it is the same
shape: ADR 0017 reasoned its way to the world being the allocation cost, and the
allocations were never in the world. Here three records reasoned their way to
the code generator being the kernel's cost, and two thirds of the kernel is in
bodies none of them profiled. Both times the instrument that would have settled
it — a profile, over the whole workload rather than the suspected part — was
cheap and came late.

## Where the kernel's time actually is

This tier can be profiled, which is the finding under the finding. The bodies
carry symbols, so `sample` attributes time to `ply_h_round` and `ply_h_compress`
by name; ADR 0036 recorded that a sampling profiler cannot see through
Cranelift's frames at all. Over the integer kernel:

| | share |
| --- | --- |
| `round` | about a third |
| `compress`, excluding the rounds it calls | about a third |
| `block_words` | about an eighth |
| allocation and release | about a sixth |

**`compress`'s own work is as large as all seven of its rounds.** It is not
arithmetic: it is nine sixteen-field records built and read per compression —
the initial state, six permutations, the final xor. And `block_words` spends
1080 instructions turning sixty-four bytes into sixteen words, because
`bytes_at` is the only bytes primitive Ply has and each byte costs two bounds
tests, a load, a shift and an or. The bar does the same work in sixteen loads.

Neither of those is a register-allocation problem, neither was measured by the
probes, and together they are two thirds of the kernel.

## The seam is a cliff, and a partial tier falls off it

The record kernel is the sharper result. Under Cranelift `k2` is inside the bar.
Under this tier it takes **about ninety seconds** — not slower than the other
tier, slower than *no tier at all*, by more than two orders of magnitude.

Nothing in the emitted code explains it and the profile says so plainly: the
time is in `Heap::to_value_counted` and `Heap::to_word`. `fold` is a builtin
that calls user code, which this tier does not carry, so the fold runs
interpreted and calls the compiled `step` two hundred thousand times. Each
crossing deep-converts the whole `State` — including a `List` that grows to two
hundred thousand elements. The crossing is O(the value) and it happens O(n)
times.

**A partial tier is not a slower whole one.** The seam offers a compiled entry
per call and can see neither that the caller is interpreted nor that the
argument is large, so compiling a callee whose caller was refused is a trap laid
by the refusal. This is ADR 0038's subject and this is the sharpest instance of
it yet measured. It is not new — it reads the same at this tier's first commit
— and it is the first thing any second tier has to answer, before code quality
is worth discussing.

## What it costs the loop

One unit of thirty-nine definitions is about four hundred milliseconds of `cc`,
and a run compiles a unit per worker plus a pre-flight. ADR 0037 read
`benches/c-floor/` as putting a C toolchain around sixty times Cranelift's
per-definition cost and predicted that a C tier inside the loop *"buys its
single code generator at a price the loop can feel"*. It does. Nothing measured
here argues with that record; this one only confirms it at the scale of a whole
unit rather than a floor.

## The decision

**Keep the tier, off the loop's path.** `--backend c` is an instrument and a
release-shaped experiment, not the default and not the loop's. It earns its
place on three things that are not speed: it is a second implementation the
first is checked against, it is the only one whose output a disassembler and a
profiler can read, and it is the shape ADR 0021's dependency line eventually
wants.

**Do not build libtcc or copy-and-patch next.** ADR 0037 left them open pending
what would choose among them. This chooses: all three differ in how fast they
compile and what they depend on, and *none* of them changes the tag, the box,
the seam or the bytes primitive — which is where every measurement in this
record says the kernel's time is. A fourth code generator would be a fourth way
to emit 321 instructions where 139 will do.

## What would clear the bar, in the order the measurements put them

Not code generation. In descending size, each with the measurement that sizes
it:

1. **A word-wide read on `Bytes`.** `block_words` is 1080 instructions for what
   the bar does in sixteen loads, and the reason is that Ply's only primitive is
   `bytes_at`. This is a language surface question — Rust has `from_le_bytes`,
   Go has `binary.LittleEndian.Uint32` — and it is the cheapest of the three.
2. **Local aggregates that are not heap records.** `compress`'s own work equals
   its seven rounds, and all of it is building and reading records that never
   escape. ADR 0034 has the in-place update; what this wants is the step past it
   — an escaping analysis and scalar replacement, so a record whose every use is
   a field read is never a record.
3. **The tag off the words inside a body.** 32 `lsr` per round, measured above.
   ADR 0039 ablated this inside the Cranelift tier and got a twentieth back
   because the allocator took the rest; that result is about that allocator, and
   the ablation should be re-taken here, where the arithmetic is already at
   parity and there is nothing else in the way.

The first two are language and representation decisions and the third is a
consequence of them. None is a code generator.

## What the gate said, this time

Registered before the arm existed: the C tier would be accepted as the loop's
if it cleared the bar the other tier does not, and rejected for the loop if its
per-definition cost stood while its kernel did not improve.

`benches/value-model/`, ADR 0030's protocol, three counterbalanced blocks with a
null control, load gate held:

| kernel | Cranelift | C tier | bar |
| --- | --- | --- | --- |
| k1 (BLAKE3) | 5.79 | see `raw.txt` | 3.0 |
| k2 (records) | 1.86 | far outside, and §"The seam is a cliff" is why | 3.0 |

**The integer kernel does not clear the bar on either tier, and the C tier is
not the closer of the two.** Halving the instructions in the body that dominates
it moved it by nothing worth reporting, which is the single most useful thing
this record contains: it rules out the whole class of answer that ADR 0036,
ADR 0039 and ADR 0037 were all reaching for, and it points at three that are
not code generators.

## What would make this wrong

- **If `block_words` and `compress`'s records were fixed and the kernel still
  did not move.** Then the decomposition above is wrong about where the time is,
  and the profile that produced it should be distrusted before anything else.
- **If the seam's crossing cost were removed** — a compiled caller for every
  compiled callee, or a crossing that does not deep-convert — and `k2` under
  this tier came inside the bar. Then the tier's placement is a question again,
  because the only thing keeping it out of the loop would be `cc`'s wall clock,
  which the other candidates in ADR 0037's table exist to lower.
- **If a body were found where clang's output is not at parity on arithmetic.**
  The claim here is narrow — one body, measured — and BLAKE3's round is
  unusually friendly to a register allocator. A body with more live values or
  worse locality could put the allocator back in the frame.
