# ADR 0033 — The two walls: bit manipulation, and the filesystem

**Accepted.** It adds six operators (`&`, `|`, `^`, `<<`, `>>`, `>>>`, plus
unary `~`), three wrapping-arithmetic builtins, two shipped modules (a hash
written in Ply, a filesystem effect declared in Ply and bound in Rust), one flag
binding a filesystem root, and four diagnostic codes. **It refuses one thing
that would have been easier: a hash builtin.**

Constrained by ADR 0001 — a definition's identity is its normalized structure,
so **no existing hash may move** — by ADR 0008's host boundary, and by ADR 0022,
since every loop in the new modules is an `iterate`.

## Why neither wall is in the gap reports

The lexer and parser spikes are thirty entries of what Ply could not do, ranked
by cost, **and neither wall appears in either.** That is not an oversight; it is
a property of what they measured.

A lexer and a parser are pure functions from bytes to a tree. **They never
compute an identity for what they produced and they never open the file they are
given** — the lexer spike treats the second as a fixture problem, which for a
lexer spike it is. Everything after parsing has the opposite shape: hashing needs
BLAKE3 over a normalized byte serialization; the store needs to read and write a
content-addressed directory; the driver needs to enumerate files, read them,
write artifacts and answer an exit code.

**So the front-end spikes measured a *tax* and concluded expressiveness was not
the blocker. That conclusion is correct about the front end and does not extend
past it, because the next three phases meet something that is not a tax. Two of
them cannot be written at all.**

## The two claims, checked before anything was decided

Both verified against a binary rebuilt for the purpose after the staleness check
reported it out of date — **the binary is an instrument and a stale one answers
questions about the language wrongly.**

**The compiler cannot compute its own hashes.** No bitwise operator, no shift,
no unsigned integer, no wrapping arithmetic, and addition raises on overflow
rather than wrapping. The lexer's own diagnostic is the shortest proof: *Ply has
no bitwise `&`; write `&&` for logical and.* **So BLAKE3 — the function that
decides the identity of every definition, every test result and every cache
entry — is not expressible in the language whose identity it is.** Neither is any
weaker hash, any varint encoder, any bitset, any checksum, or any PRNG that is
not a linear congruence with hand-proved bounds.

**A Ply program cannot open a file.** The host listing has no filesystem
operation, no arguments, no exit code a program chooses, and no environment.
**The whole driver — the part that makes a compiler a program rather than a pure
function — has nowhere to stand.**

**Neither is a tax and neither has a workaround.** The first's workaround is
"write the hash in Rust", which is the thing a bootstrap is trying to stop doing;
the second's is "pass the source in as a byte literal", which is what the lexer
spike did and is not a compiler.

## Operators, and why not a function surface

Prefix builtins would have been cheaper — no grammar change at all — and are
refused. **A language that means to be used for general-purpose work spells a
xor the way every language its users have already learned spells it, and a hash
written in prefix calls is unreadable in exactly the place correctness matters
most.** The cost is four grammar questions and all four have answers.

`&` is free: the lexer reaches it only to raise, so the character becomes real.
`|` is already a token and stays one — its existing uses are in *type*, *item*
and *row* position, which an infix operator cannot reach.

**There is a fourth user of `|` and it is in expression position: a lambda's
parameter list.** A parameter may carry a default, a default is an expression,
and the token after that expression is the `|` that closes the list — so a
defaulted lambda parameter read the closing pipe as bit-or and swallowed the
body. The parser now carries a no-pipe flag over the parameter list, the way it
already carries one to keep a record literal out of a conditional.

**This is written down because the reasoning that missed it is the interesting
part. Enumerating where a token appears in *well-formed* source is not the same
question as where the parser can be when it meets one**: a lambda default is
refused-but-parsed, so the swallow only showed up on input that was already an
error. Nothing in the workspace suite caught it. The parser spike's differential
did, on one hand-written recovery fixture.

**`>>` is deliberately not lexed.** A generic type closing with two `>` parses
today precisely because `>` is one token; making `>>` one re-introduces the split
the ported parser carries a flag for. So the *expression* parser joins two
**adjacent** greater-thans — adjacent meaning no trivia between — into a shift,
and three into a logical shift. The type parser is untouched.

**Precedence is Rust's order**, which is C's with the bitwise operators moved
above comparison where they belong. **The relative order of every existing
operator is preserved, which is the property that matters**: the numbers
renumber, no existing parse tree changes, and therefore **no existing definition
hash moves and no cached result is invalidated.** The normalizer's operator-byte
table is explicit rather than a discriminant cast, so the new operators take new
bytes and the old ones keep theirs.

### Semantics, and the one deliberate exception to checked arithmetic

The bitwise operators are defined at `Int` only, on the two's-complement
pattern; a `&` between two booleans is a type error naming both, **rather than a
silently-different short-circuit.**

**A shift count outside the word raises**, for the same reason division by zero
does: there is no answer, and inventing one — undefined behaviour, a panic, a
silent mask — is the defect the checked-arithmetic design exists to prevent.

**A left shift discards the bits shifted out and does not raise.** That is an
exception to "arithmetic is checked" and it is deliberate: **a shift is a bit
operation, not a multiplication, and a hash's mixing step is *defined* to drop
those bits.** Both an arithmetic and a logical right shift exist because `Int` is
signed and there is no unsigned type — **with only one of them, half of the
published hash and checksum algorithms cannot be transcribed.**

**Wrapping arithmetic is builtins rather than operators**, because there is no
punctuation any reader would recognise and **because the checked spelling must
stay the one that is easy to write.** Note what they are *not* needed for: BLAKE3
mixes 32-bit words, and two such values added in a 64-bit register cannot
overflow, so masking suffices there. They are for the 64-bit case, which is what
"general purpose" means.

**Two more, each a wall the first two ran into.** Hex literals, because a bit
surface without them is half a feature — a mask written in decimal is not a mask
anybody can check against a specification. It is a lexer change and nothing else,
so a program that changes base re-runs nothing; the bound is the unsigned word
and the value is reinterpreted, **which is the only reading useful to the
operators above.** And a byte constructor, because nothing turned a byte back
into bytes, so every program that had to emit a computed byte carried a
256-escape literal and sliced it — four sightings across two shipped files, and a
fifth met writing a hash's own output, **which is what makes it a wall rather
than a wart.**

## BLAKE3 is written in Ply, and there is no builtin

A hash builtin was on the table and is refused. **What that buys: the function
that decides the identity of every definition in the language becomes a
definition in the language**, which a reader can read and a test can shrink. And
it is the only honest proof that the operator surface is *enough* — **a bit
surface justified by "you could write a hash with it" and never used to write one
is a claim, not a mechanism.**

**What it costs, stated before it was measured**: two implementations of one
function that can disagree, and an interpreter running the build's hottest inner
loop. Both are real and neither is hypothetical.

**Two thresholds, fixed before the measurement existed.**

**Correctness.** Agreement byte for byte with the crate the hasher already links,
over a corpus crossing every structural boundary the algorithm has — the block,
the chunk and the tree above it — plus the published vectors. **The threshold is
zero disagreements. One disagreement and the *decision* is wrong, not the
implementation, because a second implementation of the identity function that is
*nearly* right is worse than not having one.** **Passed**, at eleven sizes, with
the published vectors holding independently, **so the two implementations are not
wrong together.**

**Throughput.** The bar is that hashing must not be worse than the phase already
known to be worst — the Ply lexer's measured rate. **Not decided, and the bar is
not moved.** The best observed figure is well short of it, **and neither series
is gate-compliant**: the machine never went below twice the load gate. **The bias
has a known direction — contention only makes a wall clock larger — so each
figure is a lower bound**, which is why the series taken while load was climbing
reads *worse* rather than confirming the first. **That is not a pass and it is
not a fail.** The run that settles it is the same series under the gate, and
until that exists this record carries a threshold it did not meet and did not
clear — **which is the state the pre-registration was written to make visible
instead of letting a number be interpreted after the fact.**

**The follow-up is named in advance and is contingent on a number rather than on
the argument being re-made**: a hash builtin, with the Ply definition kept as the
readable one and the differential kept as what holds the two together. That is
the shape ADR 0027 used, and it is the shape a *measured* reversal takes.

**Failure was expected, which is why it is written down.** One compression is
seven rounds of eight mixing functions, so a kibibyte of input is on the order of
ten thousand interpreted operations against a lexer's handful per byte. **If the
idle run lands below the bar — as the arithmetic predicts — this record does not
get to call the result acceptable after seeing it.**

## A rooted filesystem effect

Nine operations over paths, declared `nondet`.

**`nondet` is load-bearing, exactly as it is for the network.** A file read is
not a function of the program's state, so a deterministic test that reaches one
fails to compile until a handler discharges it — **and the handler that
discharges it is an in-memory twin, which is the whole thesis applied to a
filesystem.** The declaration is the authority and the handler is what gets
refused.

**A resource label is a root, and the root is the capability.** The label is
bound to a directory beside the run, the way a TLS credential is and for the same
reason: **a path inside the program would put a filesystem location into a
definition's hash and into a store designed never to forget.**

Three consequences, and the third is the one that pays.

**An operation naming a label with no bound root is refused *at the perform*,
naming the label and the flag that would bind it.** Two earlier-firing designs
were tried and refused: making the bind refuse the atom puts filesystem knowledge
into the evaluator's generic resolution, and registering a handler per bound root
gets the timing for free but makes an unbound label resolve to *no handler at
all* — **which reports the hermetic-run error and names a twin rather than naming
the flag.** The TLS case is the same sentence one capability along. **The cost is
real**: a build that reads one root and writes another learns about the second
when it first writes, and the listing is the instrument that answers earlier.

**A path that escapes its root — `..` above it, an absolute path, or a symlink
resolving outside — is refused before the syscall**, with the root and the
resolved target both named.

**Two roots that do not overlap do not conflict**, so tests over different roots
run concurrently and two readers of one root run concurrently while a writer
serializes against both. **That is the readers-writers rule the scheduler has had
since footprints existed, applied to directories for free.**

**Reads are reads, and that differs from the network deliberately.** A socket
read *consumes* from a kernel buffer, so two readers of one socket race; two
readers of one file do not.

**What is refused, each because the compiler driver this exists for does not need
it and each is cheaper to add later than to remove**: no file handles and no
streaming — a read is whole-file, refused past a bound the run sets; no recursive
walk; no permissions; no watching; no standard streams; and no argument vector.

**Rename is within one root**, which is what makes a cache write atomic. Across
two roots it would be a copy with a different failure mode, **and it would need
two labels in one atom, which the effect row cannot say.**

**The twin ships with the declaration**, as pure functions over a map, so every
project's in-memory filesystem is the same one, checked against the same
signature. The database twin is the precedent; this one is a hundred lines rather
than two thousand **because a filesystem has no query language.**

## What this costs, by surface

Every checklist item, and none optional: the lexer's tokens and its now-false
diagnostic; the parser's operator table and the adjacent-`>` join; the AST's new
variants; the normalizer's new bytes, **with no existing byte moving**;
inference; three builtins **and the completeness checks that would otherwise
miss them by omission**; the evaluator, including which shift counts raise; the
prover, where a new operator must be lowered **or explicitly unsupported, so an
obligation over one reports the weaker tier rather than a wrong proof**; four
diagnostic codes, each with a constant, a registry row **and a production
construction site**; a host handler with every declared flag, **and the listing's
digest moves**; two standard-library modules, **whose counts and digest both
move**; and nine sections of the guide.

**The code generator refuses the new operators rather than lowering them, and its
matches are exhaustive with no wildcard on purpose** — a new variant fails to
compile there rather than defaulting to something. **A left shift discards where
every other arithmetic operator raises, and a shift count out of range raises
where the machine instruction of the same name masks it, so they are not the same
operator. A backend that answered where the evaluator raises is a divergence no
differential covers, because a backend is not a second evaluator.**

**Three methods route a pending token, not one.** A facility wired into two of
them submits jobs nothing can resolve — which the run path, having one task and
blocking rather than parking, met as an internal error on the first filesystem
operation any program performed. **Found by running the thing, not by reading
it.**

## What would make this wrong

- **Correctness fails.** A Ply hash that disagrees on any input in the corpus
  refutes the decision, and the builtin returns.
- **Throughput fails and the follow-up is not taken.** Recording a number below
  the bar and calling it acceptable is the defect the contribution rules count
  seven of.
- **A rooted filesystem is the wrong capability granularity** — if real programs
  bind one root at the top because splitting them is tedious, then the label
  carries no information and the escape check is ceremony. **The check is whether
  anything in the tree binds more than one root and means it.**
- **The operators move a hash.** Any pinned digest that moves for a program
  containing none of the new tokens refutes the renumbering.
- **`nondet` on the filesystem turns out to be too strong.** If every test that
  touches a file ends up opting out of determinism rather than using the twin,
  the effect is buying nothing and costing a keyword.
