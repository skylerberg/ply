# ADR 0039 — How Ply types its numbers

**Accepted for the language surface; the performance claim it was motivated by
is not yet taken.** It adds eight scalar types, one literal form, one
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
> to its bar. The code generator does not carry a fixed-width value yet — it
> refuses every definition that mentions one — so no ratio has moved and none is
> claimed here. That is the next record's, and §"The measurement this rests on,
> and the one it does not" says what is already known about it.

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

`rotr32` stays, as the `Int`-only spelling that predates the family. It becomes
dead surface the moment `std.hash` moves to `U32`, and should be removed in that
change rather than this one.

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

## The measurement this rests on, and the one it does not

The LLVM probe above is a control, not the gate: it says what a *good* optimiser
does with each representation, which is why it argues that Cranelift is most of
the seven. It does not say what Cranelift does with an `I32` value, because
nothing has built one yet.

**The gate is `benches/value-model/`'s, unchanged and not re-taken.** What it
will take: the code generator carrying a fixed-width value in a register of its
own width and in a record field of its own width, `std.hash` rewritten at `U32`,
and the same protocol — counterbalanced arms, a null control, the binary checked
current, `analyze.py` holding the bar at 3.0 where a number cannot set it
afterwards.

**Registered before that arm exists, so the reading cannot be chosen after it:**
if K1 clears the bar, the diagnosis in ADR 0036 §"The two listings" is confirmed
and the family is what confirmed it. If K1 moves but does not clear, the
remainder is the record layout and the loop tier, in that order, and this
record's premise was right and incomplete. **If K1 does not move at all, the
width was never the lever, the LLVM probe was measuring the optimiser rather
than the representation, and the next record is about the loop tier
(ADR 0037) rather than about types.**

## What is not built

- **The code generator refuses every definition whose type mentions a
  fixed-width type**, and refuses `rotr` and all sixteen conversions as
  builtins, so nothing inside a compiled body can reach a width its signature
  does not name. A program using the family runs on the interpreter, correctly
  and slowly. That is deliberate: a compiled body holds every integer as a
  sixty-four-bit word and would answer a `U32` addition as an `Int` one, which
  is a wrong answer rather than a slow one.
- **The prover leaves fixed-width arithmetic uninterpreted.** A literal is an
  interned constant node, so two occurrences of `5u32` are one term, but `+` at
  `U32` is not the integer `+` the linear arithmetic reasons about. It could not
  be: the operation raises where the integer one answers.
- **`spikes/ply-parser` does not know the suffix or the new prelude names.** Its
  differential is corpus-driven and no corpus input carries one, so it is green
  and will stay green until somebody writes a suffixed literal into the corpus.
  That is a latent divergence, recorded here rather than discovered later, and
  it is the same shape as the four the spike's `README.md` predicted.
- **`std.hash` is still written over masked `Int`s.** Moving it is the change
  that re-takes the gate.

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
- **If the gate does not move.** See the registered reading above; the third
  branch retires the whole premise, not just the ordering.
