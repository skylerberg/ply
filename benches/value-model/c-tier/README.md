# Would a C tier clear the bar, on the value model Ply actually compiles?

ADR 0039 ended with the integer kernel at about six times the bar under Cranelift
and seven levers that could not move it, and named two candidates for what could:
ADR 0037's tier, and destination-passing. This prices both, before either is
built.

`benches/value-model/width-probe/` asked a narrower question — what a strong
optimiser does with each *number type* — and held the representation in a plain
struct. This holds **the representation `crates/ply-codegen` really emits**: the
sixteen-byte header with its reference count, payload words after it, every `U32`
a tagged immediate, a record built per round *and* per permutation, reused from a
token when the count says it is dying and bump-allocated when it is not, with the
count taken and given back at each call. Then it compiles the whole thing with
`cc -O2`.

```
cd benches/value-model/c-tier && cc -O2 -o probe probe.c && ./probe
```

Three arms, counterbalanced across three blocks, minimum per arm, digests
asserted equal so an arm that drifts fails rather than skews.

| arm | what it is |
| --- | --- |
| `bar` | `u32` words in a `[16]`, wrapping adds, rotates. The same bar `benches/value-model/rust` is |
| `ply` | Ply's compiled value model, exactly, one tier down |
| `ply-dps` | the same with destination-passing: the caller hands the callee the memory to write into, so the callee allocates nothing, needs no reuse-or-allocate branch, and borrows *both* inputs |

## What it found

**The tier is the whole of the gap, and destination-passing is not.** On a quiet
machine the `ply` arm is about one and a half times the bar and the `ply-dps` arm
is a few percent under it — against a bar of three, and against the six times the
same representation costs under Cranelift.

So the answer to "what would clear the bar" is the tier, by a factor of about
four, and the answer to "how much is destination-passing worth" is: a few
percent, on the tier where instructions show up at all. Both were open questions
in ADR 0039 and neither needed the feature built to answer.

## What it is not

It is not a measurement of Ply. No ratio here belongs beside `analyze.py`'s: this
is C written by hand to the shape Ply's backend emits, not C emitted by it, and
the difference is exactly the engineering ADR 0040 decides to do. What it
establishes is that the shape is not the problem — which is what a tier decision
needs to know before anyone writes a code generator for it.

It also holds one thing constant that a real tier would not: the arena, the
tokens and the runtime helpers are in the same translation unit as the kernel, so
`cc` inlines them. A shipped C tier links the runtime separately unless it is
given the header, which is a reason to give it the header.
