# ADR 0039 — How Ply types its numbers

**Accepted for the language surface. The performance claim it was motivated by
has now been taken and it did not clear: the integer kernel moved from about
seven times the Rust bar to about six, against a bar of three.** §"What the gate
said" is the reading, registered before the arm existed and unchanged since. It adds eight scalar types, one literal form, one
diagnostic code, seventeen prelude names and one normalizer tag. It refuses
three things: a modular type whose `+` wraps, arbitrary bit widths, and a
literal that is a value of more than one type.

`Int` does not move. It is sixty-four bits, signed and checked, and it stays the
type a program counts and indexes with.

> **What this decides.** That Ply has a fixed-width integer family — `U8`,
> `U16`, `U32`, `U64`, `I8`, `I16`, `I32`, `I64` — whose arithmetic is checked
> exactly as `Int`'s is, whose members never widen into one another implicitly,
> and whose literals carry their type as a suffix the way a `Decimal`'s already
> does. That the wrapping surface stays the three builtins plus `rotr`, at every
> width rather than at sixty-four bits only.
>
> **What it does not decide.** That the family closes the integer kernel's gap
> to its bar. It does not, and §"What the gate said" is the measurement rather
> than an expectation.

## What made this necessary, measured

`benches/value-model/`'s gate has the record kernel inside the bar and the
integer kernel — BLAKE3 written in Ply — at about seven times the Rust bar,
against a bar of three. ADR 0036 named three suspects and `k1-where.sh` priced
all three: the checked adds are worth nothing, the masks a few percent, the
records a fifth. What it found instead is in ADR 0036 §"The two listings, side
by side": Rust's `round` is 138 instructions with two stack operations, Ply's
carries 330 masks, 142 tag-tested field loads and 524 stack operations, and
**one fact explains most of the difference — Rust's words are `u32` in `w`
registers and Ply's are `Int`, a sixty-four-bit tagged word.**

Nothing in the source, the checker or the code generator could say that a value
is thirty-two bits wide. That is the gap this record closes, and it is a gap in
the *language* before it is one in the backend.

**What the family is not a fix for.** A probe written for this decision
transliterates the same BLAKE3 four ways and compiles all four with LLVM,
holding the algorithm, the sixteen-field state record and the memory traffic
constant and varying only the number type: `u32` words; `i64` words masked after
every add; the same with every add checked; and the same again with every word
tagged `(v << 1) | 1` in the record, which is Ply's compiled model exactly.
**Under LLVM the whole of Ply's representation costs about a fifth over the
`u32` bar, not seven times** — the optimiser deletes every mask, cancels nearly
every tag round-trip, and proves away all but four of forty-eight overflow
checks, landing at 201 instructions against the bar's 141.

So the seven is not the representation's; most of it is what Cranelift emits
from that representation, and Cranelift has no demanded-bits pass and is chosen
for compile latency rather than code quality (ADR 0037's tier table). **A width
in the type is how you get good code out of a backend that does not optimise:
it hands the code generator `add w`, `ror w`, `eor w` with nothing left to
infer.** It is also the only route that changes *layout*, since no analysis can
make two adjacent words one `ldp` when the record does not store them that way.
Both of those are arguments, not measurements, and they are why the gate above
is left open rather than claimed.

**The expressiveness case stands on its own and is older.** ADR 0033 recorded
that Ply could not express its own hash, added six operators and three wrapping
builtins to get within reach, and left the wall it worked around in place: no
unsigned type, so `>>` and `>>>` both exist to cover what one unsigned shift
would; hex literals defined "as a bit pattern" because there was nothing else
for them to be; `bytes_at` answering an `Int` for a byte. This record retires
that workaround list.

## The decisions

### `Int` is not a member of the family

It is the default and it stays exactly what it was. Every language surveyed that
competes on compiled speed has a fixed-width family, and every one of them that
also has an ergonomic default — OCaml's `int`, Koka's `int`, Lean's `Nat`,
Haskell's `Int` — keeps the two apart. The one you reach for is not the one you
write a hash in. Use a fixed width where the *algorithm* is written in one;
everything else counts and indexes.

### Arithmetic is checked at every width

`+`, `-`, `*`, `/` and `%` raise `E0502` on a `U8` exactly as they do on an
`Int`. **Nothing wraps silently at any width.**

The measurement is what makes this free: `k1-where.sh` emitted a build with no
overflow check anywhere and the ratio moved within its own spread. So the speed
argument buys no licence to wrap, and wrapping silently is the thing `Int`
refuses on principle.

The field splits and the split is not about performance. Swift, Roc, Nim and
Ada's range types check; Go, Java, C#, Julia, OCaml, Haskell, Lean 4 and Koka
wrap. Ply was already in the checked camp and stays there.

**Rejected: a modular type, Ada's `mod 2**32`.** It is the one tradition where
wrapping is a property of the type rather than of the operator, which gets both
the ergonomics and the honesty — `a + b + mx` reads like the published
algorithm — and it is the strongest argument against what this record decided.
It is refused *for now* and not on the merits: it makes GUIDE §5.1's
"arithmetic is checked" a per-type claim, every reader would have to know which
family they are in, and **wrapping can be added later while it cannot be taken
away.** If the wrapping builtins prove unreadable where it matters, this is the
first thing to revisit.

### A literal's type is its spelling

`255u8`, `0x6A09_E667u32`. The suffix is one of `u8 u16 u32 u64 i8 i16 i32 i64`;
an unsuffixed integer literal is an `Int` and stays one.

**This is the decision §5.2 already took for `Decimal`, extended rather than
revisited.** `1` is not a `Decimal` in Ply — `1m` is — and by the same rule `1`
is not a `U32`. There is no defaulting and no literal that is a value of two
types.

**Rejected: a literal polymorphic over the nine integer types, defaulting to
`Int`.** It was built first, and it is what Rust, Swift and Haskell do. Two
things sank it, the second decisively:

- The default is a tiebreak taken inside the compiler, which is exactly what
  §5.2 refuses for an operand type. The argument that a literal's type never
  reaches a published signature is true and is a *weaker* reason than the one it
  is asked to overturn.
- **The value would have had to come from a side table the cache cannot carry.**
  A literal's type is inference's answer, and the evaluator builds a literal's
  value from the syntax alone. Keying that answer by span breaks under
  incremental checking twice over: a definition restored from a cached interface
  is never walked, so it has no entries at all, and a cached definition's hash is
  span-independent, so entries taken under one layout are wrong after the text
  above it moves. Keying by traversal order instead makes the checker's walk and
  the lowering's walk a coupling nothing checks. A suffix puts the answer where
  the lexer can read it, and every one of those problems disappears.

The cost is real: `fn f(x: U16) -> U16 = x + 1` is an error and `x + 1u16` is
not. **And one value is unspellable** — the smallest of each signed type, since
`-128i8` is a negation of `128i8` and `128i8` is not an `I8`. Write
`i8_of_int(-128)`. `Int` has had the identical limit at `i64::MIN` since the
beginning, so this is a wart the family inherited rather than one it introduced.

### Nothing widens implicitly, and two widths meet through `Int`

Sixteen conversions, eight each way. `u32_of_int` and its siblings **raise**
when the value is not one of that type's, for the reason `byte_of_int` does: a
value silently reduced is one nobody chose. `n & 0xFF` first is how a program
says it meant the truncation. `int_of_u32` and its siblings are total but for
`int_of_u64`, which raises past the largest `Int`.

There is no `u32_of_u8`. Fifty-six conversions between widths would be a table
nobody reads; `u32_of_int(int_of_u8(b))` is two calls and is verbose on purpose,
because every narrowing in a program should be one call a reader can find.

### Bit operators answer their operands' type; a shift's count is an `Int`

`~0u8` is `255u8`, not `-1`. A shift's bound is the *word's* width, so a count of
32 shifts an `Int` and raises on a `U32` — and the count itself is an `Int`
whatever the word is, because a count is not a word.

### The wrapping surface is four builtins, polymorphic over the family

`wrap_add`, `wrap_sub`, `wrap_mul` and `rotr`, at every integer type, wrapping
at the operand's own width. Their published type is `forall a. (a, a) -> a`,
which alone would take two `String`s; the obligation that `a` is an integer type
is attached at each call and discharged with every other integer-type obligation,
so a wrong operand is a diagnostic naming the type rather than an unsolved
variable.

`rotr32` stays, as the `Int`-only spelling that predates the family, and it now
has no user in any `.ply` file — `std.hash` turns on `rotr` at `U32`. It is kept
rather than removed for one reason: `benches/value-model/k1-where.sh` is a
recorded measurement whose source must keep compiling, and a measurement that no
longer runs is worth less than a builtin nobody calls. It is dead surface, it is
recorded here as dead surface, and `docs/dead-surfaces-report.md` is where a
sweep would find it.

## Refused: arbitrary bit widths

Zig's `u0..u65535` is a better rule than eight names and it rests on a backend
Ply does not have. Written down because it is the obvious next question.

1. **Cranelift has exactly `I8 I16 I32 I64 I128`** (checked in
   `cranelift-codegen-0.132.3`, `src/ir/types.rs`), and signedness lives in the
   *operation* rather than the type. LLVM gives Zig `iN` and a legaliser; Ply
   would have to write the legaliser, and what it emits for a `u12` is a mask
   after every operation — **the exact code this record exists to delete**,
   reintroduced for widths nobody asked for. The C tier is no better: `_BitInt`
   is C23 and unevenly supported.
2. **Several passes here are total over a closed set of types and are checkable
   tables** — GUIDE §18, `ply-derive`'s `Shape`, the prover's domains, the
   normalizer's tags. Eight members are a table a reader verifies; 65,536 is a
   function, and §"The one rule" is about claims a reader can check.
3. **Ply cannot cash the precision.** Zig's narrow widths earn their keep in
   `packed struct` bit-packing and in range proof. Ply has no packed layout — a
   `U3` field would occupy the byte a `U8` does — so an odd width would buy only
   a narrower *range*.
4. **And Ply already has a better tool for range.** `requires x <= 4000` says
   what you mean and `ply-prove` discharges it; `u12` says `0..4095` and only
   approximately. Wuffs, the one language that really solves this, spells it
   `u32[0..12]` — a refinement on a machine width, not a machine width per
   bound. A second, weaker range mechanism beside the one the language
   advertises is not an improvement.

**The eight are the same design restricted to the widths the machine has**, and
nothing forecloses generalising the rule later. What would reopen it: packed
layouts, or a wire format that needs sub-byte fields.

## What the family bought the prover, unasked

`U8` and `I8` have 256 inhabitants and `U16` and `I16` have 65,536, so
`cardinality` answers for every width below sixty-four. `MIN_PROPERTY_CASES` and
`ENUMERATION_BOUND` do the rest: **`forall (b: U8)` is discharged by covering
every byte** rather than by sampling. That is a stronger obligation than the same
property at `Int` can ever have, and it came free with the type.

## What the gate said

**Registered before the arm existed:** if K1 clears the bar, ADR 0036's
diagnosis is confirmed and the family is what confirmed it. If K1 moves but does
not clear, the remainder is the record layout and the loop tier, in that order,
and this record's premise was right and incomplete. If K1 does not move at all,
the width was never the lever.

**It moved and did not clear: about seven times the bar before, about six after,
against a bar of three** (`benches/value-model/after-widths.txt`, by
`PRE-REGISTERED.md`'s protocol with `analyze.py` holding the bar where a number
cannot set it afterwards). So the second branch, and the middle sentence of it
is the finding: **right about the cause, wrong about the size.**

**The width did everything a width can do**, which the two listings side by side
say exactly (ADR 0036 §"The two listings"): the 330 masks per `round` are gone
entirely, the rotate is `extr w, w, #16`, the adds are `add w` and none is
checked, the field loads carry no tag test, and the compiled body went from about
1750 instructions to 706 — of which about half is one execution, because the body
holds the round twice, once for the path that reuses the dying state record and
once for the path that allocates.

**And the LLVM probe predicted the size, which is the part worth keeping.** It
put Ply's whole representation — masked, checked and tagged — at about a fifth
over the `u32` bar. A fifth is what removing the representation was worth. The
probe was not a curiosity beside the decision; it was the decision's most
accurate instrument, and it said before any of this was built that the seven was
mostly the code generator.

### What is left, measured rather than listed

Three levers were tried on the remainder and all three are recorded so the next
reader does not spend the day:

| lever | effect |
| --- | --- |
| a wider inline budget, so `round` folds into `compress` | **worse: 9.17 against 6.01.** ADR 0036 measured this over `Int` and found four times worse; the width does not change the conclusion, and `compress` goes to 76KB |
| borrowing the two state records instead of reusing the dying one | **worse: 6.45 against 6.01.** The bump allocation the reuse saves costs more than the reference counting and the doubled body it pays |
| storing a width raw in a flat record, so the tag round-trip goes | **a twentieth, and the reason is the fourth time the same wall appears.** Ablated: the 64 untags and 32 tags per `round` do go, and 96 instructions out buys 33, because the loads rise from 102 to 113 and the stack traffic from 114 to 136. Taking the shift away lengthens the live range it was shortening, and the allocator spills the difference. (The ablation also segfaults, since a generic path reads a record's word without its shape's field types — which is what a shipped version would have to teach.) |

**Four levers, and every one of them ends at the same wall.** Inline the round
and the allocator spills; borrow the records and the allocation it saved costs
more; untag the fields and the allocator takes back two thirds of what was
removed; and ADR 0036 already recorded a fifth, sinking a field read to its use,
with the same result in the same words — *spills went up, time unchanged*. The
binding constraint is Cranelift's register allocator over a body with
thirty-two live values, and it is not something a type can move.

**So the bar is not reachable from the type system, and the probe says what it
is reachable from.** `width-probe`'s `i64t` arm is Ply's compiled representation
exactly — tagged words, masked and checked, in a memory-resident sixteen-field
record — and under LLVM it is **1.21 times the bar**, inside a bar of three with
room to spare. The representation was never what stood between this kernel and
its bar. The code generator is, and
`docs/adr/0037-the-loop-is-the-goal.md` is where that is decided.

**What this record leaves the tier is a kernel it can actually clear.** `U32`
arithmetic in `w` registers is what a C compiler wants to be handed; the masks
and narrow-and-widens this deleted are the ones it would otherwise have had to
prove away. The next lever inside *this* tier, if one is wanted before the tier
changes, is destination-passing — letting a caller hand `round` the memory to
write into, which is the only shape that gets both the borrow's freedom from
reference counting and the reuse's freedom from allocation, and which Koka and
Lean both have for exactly this reason.

## What is not built

- **`U64` and `I64` are outside the compiled fragment.** Every other width fits
  the sixty-three bits an immediate carries, so boxing one is a shift and an or
  with no branch and no allocator and unboxing needs no tag test — the property
  the whole lowering rests on. A `U64` past `2^62` does not have it, so carrying
  those two means a heap object of their own kind and an unboxing that tests for
  it. They check, they evaluate, they do not compile.
- **A signature mentioning a width does not cross the seam**, in either
  direction. Compiled code holds a width as the tagged immediate an `Int` is
  held as, so a value crossing would be read as an `Int` where a `U32` was
  declared. The bodies still compile and still call each other directly, which
  is what lets `std.hash`'s internals work at `U32` while `blake3` itself is
  entered whole — but it costs the differential its per-function coverage of
  `compress`, `round` and `g`, which are now checked only through `blake3`.
  Closing it means a type-directed conversion at the seam, which the layout
  already has the information for (`Ty::Record` knows each field's `Ty`) and
  which nothing needs yet.
- **The prover leaves fixed-width arithmetic uninterpreted.** A literal is an
  interned constant node, so two occurrences of `5u32` are one term, but `+` at
  `U32` is not the integer `+` the linear arithmetic reasons about. It could not
  be: the operation raises where the integer one answers.
- **`spikes/ply-parser` does not know the suffix or the new prelude names.** Its
  differential is corpus-driven and no corpus input carries one, so it is green
  and will stay green until somebody writes a suffixed literal into the corpus.
  That is a latent divergence, recorded here rather than discovered later, and
  it is the same shape as the four the spike's `README.md` predicted.
- **The state records still live in memory across a round**, seven of them per
  compress, because inlining them away costs more than it saves under this tier.
  That is the remainder, and it is ADR 0037's.

## What would make this wrong

- **If a modular type turns out to be what the hash wants.** The wrapping
  builtins are a bet that `wrap_add(wrap_add(a, b), mx)` reads well enough. If
  `std.hash` at `U32` reads worse than it does today over masked `Int`s, the
  bet lost and Ada's shape is the answer.
- **If the suffix proves unbearable in ordinary code.** The evidence would be
  fixed-width types being avoided where they belong, or `u32_of_int` appearing
  where a literal should have. Polymorphic literals could be restored — but only
  behind a cache-safe way to carry inference's answer to the evaluator, which is
  the thing that sank them the first time.
- **If packing, rather than merely untagging, turns out to be worth more.** The
  ablation above removed the tag and kept eight bytes a field. Packing to four
  would halve the record's footprint and could pair its loads, and cache
  behaviour is in no instruction count. It is the last thing the type enables
  that has not been measured — though the ablation's result, that the allocator
  takes back two thirds of anything removed, is not encouraging.
- **If ADR 0037's tier lands and the kernel still does not clear.** Then the
  memory traffic was not the remainder either, and the next reading has to come
  from a profile of the compiled frames rather than from a listing — which is the
  instrument `benches/value-model/k1-where.sh` exists to be, and which has not
  been re-taken since the width landed.
