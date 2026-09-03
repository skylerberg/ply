# ADR 0040 — What a second code generator is for

**Accepted as a build and a placement. Ply has a second code generator; it does
not go in the loop, and on it the integer kernel reaches ADR 0035's bar without
clearing it — the gate's word is *undecided*, twice.** The shipped tier is at
5.18 and cannot follow, for a reason this record measures. What the tier mostly
did was answer the question ADR 0039 left open, and the answer is not the one
that record predicted.

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
constraint, halving its output would halve the kernel's gap. It does not: the
gate moves from 5.79 to 4.69, a fifth, against a bar of three. The 2.2× on the
body buys a fifth at the gate, and §"What the gate said, this time" is the
reading.

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

## What the tier was for: the levers it made visible, and what they were worth

Everything below was found by reading this tier's output — the emitted C, or the
disassembly of the image it loads — and each is a form of *not building what
nothing looks at*. None is a code generator improvement.

| lever | what it removes | k1 on this tier |
| --- | --- | --- |
| a field read answered from the register the record was built from | the load and the tag | nothing, alone |
| the rounds folded into the compression that calls them | the call, and it is what puts a record's build and its reads in one body | with the above, **4.69 → 3.9** |
| `bytes_u32_le`, four bytes in one read | 128 branches and 131 comparisons per block, bounds-testing bytes one at a time | nothing on this tier; **5.79 → 4.8 on the other** |
| a flat record nothing reads is never built | the allocation, sixteen tags, sixteen stores, and one dismantling | **3.9 → 3.4** |
| `Stop` and `Continue` written into the loop rather than built | a constructor built and taken apart per block | **3.4 → 3.3** |

**The first two are one lever.** Forwarding alone measures as noise, because
`round` reads its parameters rather than records it built; folding alone leaves
the reads going to memory. Together they are a fifth. This is the kind of thing
a table of independent levers cannot show, and ADR 0039's table is a table of
independent levers.

**The bytes read is the one that moved the other tier**, and it is the only
lever here that is a language change rather than an emitter change — which is
also why it is the only one both tiers get.

## The seam is a cliff, and a partial tier falls off it

The record kernel is the sharper result. Under Cranelift `k2` is inside the bar.
Under this tier it takes **about ninety seconds**: 10398 times the bar, against
the other tier's 1.86. That is not slower than the other tier, it is slower than
*no tier at all*, by more than two orders of magnitude.

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

## What a second code generator costs, which is not nothing

It is checked against the first, and that is the reason to have one. It also has
to *be* checked, and the checking is not free: this tier ran for its whole first
week against the interpreter's answers on its own unit tests and against BLAKE3's
published vectors, and was wrong in three ways that neither could see.

- **`None` was a closure.** A constructor named as a value is two things by
  arity — `None` is a singleton, `Some` is a function value — and the tier asked
  for the function in both cases. Every body that answered `None` answered with a
  closure where a variant belonged.
- **Every worker wrote the same file.** The build directory was keyed on the
  process id, and a run compiles the unit once per worker plus a pre-flight, in
  one process. The losers of the race declined every call and the run went
  quietly on with the interpreter — green, and over nothing.
- **A record built in a loop was built once.** Records are held back and built on
  demand under a guard; inside a loop the guard handed back the object the first
  iteration built. `std.hash`'s vectors passed throughout, because they hash one
  and two chunks and the gate hashes sixty-four.

**All three were found in one afternoon by running `--backend c --audit-backend`
over `examples/`, which the other tier had been doing since it existed.** None
of them needed a new idea, only the check that was already written for the other
tier and not pointed at this one. That is the real cost of a second code
generator, and it is a cost in *discipline* rather than in code: the first tier's
harness has to be aimed at the second on the day it lands, or the second is a
tier nobody is checking.

The two steps are in CI now, and the second is the shape that broke: sixty-four
chunks, audited against the machine.

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

## What would clear the bar, now that two of the three have been taken

The three this record first named were a word-wide `Bytes` read, local
aggregates that are not heap records, and the tag off the words inside a body.
The first is `bytes_u32_le` and the second is the elision above; the third came
with the second, since a record that is never built is never tagged. Together
they are 5.79 to 3.14, and the bar is 3.

What is left is one thing, and it is the one the elision rule excludes:

**A record whose fields are themselves records.** Only a record of immediates is
held back, because only such a record holds no counts and so cannot be wrong
about them. The kernel's chunk loop carries `{cv, i, out}` — the compression's
sixteen-word answer, the eight-word chaining value taken from it, and an integer
— so it is built, and being built it forces the two it holds. Those three
objects, allocated and dismantled per 64-byte block, are what the profile has
left: **memory management is about two fifths of the kernel and the emitted code
is the rest.**

Extending the rule to the counted case is ADR 0034's `reset` token generalised —
a Perceus question rather than a code generation one — and it is worth roughly
what is between 3.14 and the bar, twice over.

**For the shipped tier the answer is different**, and this record does not have
it. Cranelift cannot take the inlining the elision needs; either it learns the
elision in its own record handling, where the inlining is not the enabler, or
the tier that clears the bar is not the tier that ships.

## Six more levers, and the instrument that cannot see them

With the gate reading *undecided* the obvious move is to find another few
percent. Six were tried, each measured by running two binaries alternately so
that background load falls on both, and reading the **minimum** of a dozen runs
rather than the mean. Five moved nothing; the sixth is the one worth keeping.

| lever | effect |
| --- | --- |
| `cc -O3`, and other optimisation levels | none |
| deciding a block's padding once rather than once per word, so the padding arm is not inlined sixteen times | none on this tier; a little on the other. It needs the row below to not *lose* by it |
| joining an `if` whose arms are records field by field, so elision survives a branch | neutral alone: nothing in the kernel needs it until the row above exists |
| carrying the loop's state as three locals instead of a record | **not worth it.** It removes an allocation and a walk per block and costs register pressure across a 2,400-instruction body; the two cancel, and it needs ownership work the tier does not have |
| dropping `-fno-strict-aliasing`, on the theory that alias analysis was blocking store forwarding | none, so that was not what blocked it |
| **not** eliding records past a width, on the theory that Rust's bar keeps its message in memory and reloads it | **1.3× worse.** The ablation that shows the elision is doing the work, and the theory was wrong |

**And then the instrument.** Run the same binary against *itself* through that
harness and the two arms differ by about five percent. The distance from 3.14 to
3.0 is smaller than that. This is not a fact about the kernel: it is why the gate
says *undecided (within the resolution of the bar)* rather than yes or no, and no
amount of measuring on this machine will turn it into either.

**What would give it room.** Ply's folded per-block body is about 2,400
instructions against the bar's ~1,100, and the arithmetic in each is the same
865. But the *time* ratio is larger than the instruction ratio, so the extra
instructions are also cheaper-per-instruction than the bar's -- which is what a
body with thirty-two live values on a machine with thirty-one registers looks
like. Rust's bar answers that by keeping sixteen words in registers and reloading
the other sixteen from memory each round. Ply has no way to say that, and the row
above shows that the crude version of saying it -- refusing to elide the wide
record -- is much worse than not saying it at all.

So the next thing is not another lever. It is either a body that does not need
thirty-two values live at once, or a way to tell the tier which sixteen to keep.

## What the gate said, this time

Registered before the arm existed: the C tier would be accepted as the loop's
if it cleared the bar the other tier does not, and rejected for the loop if its
per-definition cost stood while its kernel did not improve.

`benches/value-model/`, ADR 0030's protocol, three counterbalanced blocks with a
null control, load gate held:

| kernel | Cranelift | C tier | bar |
| --- | --- | --- | --- |
| k1 (BLAKE3) | 5.18 | **3.14 — undecided** | 3.0 |
| k2 (records) | 1.84 | 10060 | 3.0 |

**The integer kernel reaches the bar on the C tier and does not clear it.** The
gate's own word is *undecided (within the resolution of the bar)*, and it said
the same on the run before at 3.00, so this is a kernel sitting on its bar rather
than one that has passed it. `raw-c-tier.txt` is the series.

Two things about that number are worth more than the number.

**The first is where it came from.** 5.79 to 3.14 is not a code generator getting
better at emitting the same program; it is four changes that stop the program
being emitted — §"What the tier was for" — of which one is a language addition
and three are refusals to build what nothing reads. The one that moved *both*
tiers is the language addition.

**The second is that the other tier cannot follow.** Cranelift is at 5.18 and
the elisions above are unavailable to it, because they need the whole-body
inlining its register allocator cannot hold. The shipped tier is the one that
does not clear, and closing that is either a different allocator or the same
elisions taught to Cranelift's own record handling.

The k2 column is not a slow tier; it is §"The seam is a cliff" in one number.

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
