# ADR 0033 — The two walls: bit manipulation, and the filesystem

**Status:** accepted. It adds **six operators** (`&`, `|`, `^`, `<<`, `>>`,
`>>>`, plus unary `~`), **three builtins** (`wrap_add`, `wrap_sub`, `wrap_mul`),
**two shipped modules** (`std.hash`, written in Ply; `std.fs`, declared in Ply and
bound in Rust), **one CLI flag** (`--fs NAME=PATH`) and **four diagnostic
codes**. It refuses one thing that would have been easier: a `blake3` builtin.
**Date:** 2026-08-31.
**Corrects in place:** `docs/GUIDE.md` §3.5 (the operator table and its "Ply has
no bitwise" note), §13 (the builtin tables), §14 (the `ply std` block — counts
**and** digest), §15.2 (the trusted computing base), §17 (`ply run` flags), §18
(the code table, which is total over `codes`), §19.2 and §19.3 (both of which
list an absence this record removes); `crates/ply-syntax/src/lexer.rs`'s
`"Ply has no bitwise \`&\`"`; `CONTRIBUTING.md`'s ADR count.
**Closes:** nothing in `spikes/`, and that is §0.
**Constrained by:** ADR 0001 (a definition's identity is its normalized
structure — so no existing hash may move), ADR 0008 (the host effect boundary and
what a handler must declare), ADR 0013 §7.2 (an argument, not a cancellation —
the shape a blocking host operation takes here), ADR 0022 (`iterate` is the loop;
every loop in the modules below is one).

---

## §0 Why neither wall is in the gap reports

`spikes/ply-lexer/GAPS.md` and `spikes/ply-parser/GAPS.md` are fifteen and
fifteen entries of what Ply could not do, ranked by what each cost, and **neither
wall appears in either.** That is not an oversight in those documents. It is a
property of what they measured.

A lexer and a parser are pure functions from `Bytes` to a tree. They never
compute an identity for what they produced and they never open the file they are
given — `spikes/ply-lexer/GAPS.md` §8 is titled *"A Ply program cannot read a
file, so the lexer's input is a source literal"* and treats that as a fixture
problem, which for a lexer spike it is. Everything after parsing has the opposite
shape:

| phase | what it needs that a parser does not |
| --- | --- |
| resolve, infer, effect-infer | nothing new — this is the part the spikes generalize to |
| **hash** | BLAKE3 over the normalized byte serialization of every definition |
| **store / cache** | read and write a content-addressed directory |
| **driver** | enumerate source files, read them, write artifacts, answer an exit code |

So the front-end spikes measured a **tax** — 3.0× to write, 30× to run — and
concluded that expressiveness was not the blocker. That conclusion is correct
about the front end and does not extend past it, because the next three phases
meet something that is not a tax. Two of them cannot be written at all.

## §1 The two claims, checked before anything was decided

Both were verified on 2026-08-31 against `target/release/ply`, rebuilt after
`.github/binary-is-current.sh` reported `STALE` — per `CLAUDE.md`, the binary is
an instrument and a stale one answers questions about the language wrongly.

**Claim 1 — the compiler cannot compute its own hashes.** Ply has no bitwise
operator, no shift, no unsigned integer, no wrapping arithmetic, and `+` raises
on overflow (`E0502`) rather than wrapping. The lexer's own diagnostic is the
shortest proof:

```
$ ply check bitwise.ply
[E0001] Error: unexpected character `&`
 1 │ fn f(a: Int, b: Int) -> Int = a & b
   │                                 ╰── Ply has no bitwise `&`; write `&&` for logical and
```

`crates/ply-core/src/infer.rs::install_prelude` publishes no `bit_*`, no
`shift_*`, no `wrap_*` and no hash. So BLAKE3 — the function `DESIGN.md` §3 makes
the identity of every definition, every test result and every cache entry — is
not expressible in the language whose identity it is. Neither is any weaker hash,
any varint encoder, any bitset, any checksum, or any PRNG that is not
`(a*x + c) % m` with hand-proved bounds.

**Claim 2 — a Ply program cannot open a file.** `ply hosts examples/desk.ply
--host` lists **25 handlers · 47 operations**, across `net`, `db`, `config`,
`trace` and `signal`. There is no filesystem operation, no `argv`, no exit code a
program chooses, and no environment. The whole driver — the part that makes a
compiler a program rather than a pure function — has nowhere to stand.

**Neither is a tax and neither has a workaround.** §1's workaround is "write the
hash in Rust", which is the thing a bootstrap is trying to stop doing; §2's is
"pass the source in as a `b"..."` literal", which is what the lexer spike did and
is not a compiler.

## §2 Decision 1 — operators, and why not a function surface

`bit_and(a, b)` would have been cheaper: builtins touch `install_prelude`,
`builtins.rs` and nothing in the grammar. It is refused. A language that means to
be used for general-purpose work spells `h ^ b` the way every language its users
have already learned spells it, and a hash written in prefix calls is unreadable
in exactly the place correctness matters most. The cost of that choice is four
grammar questions, and all four have answers.

**`&` is free.** The lexer reaches `'&'` only to raise `E0001`; `&&` is a
separate token. The diagnostic goes away because the character becomes real.

**`|` is already a token and stays one.** `TokenKind::Pipe` is used by sum-type
declarations, the generic-parameter separator, and an effect row's tail — all in
*type*, *item* or *row* position, which an infix operator cannot reach. `||`
lexes first, as it does today, so `|| body` is still a nullary lambda.

**There is a fourth user of `|` and it is in expression position: a lambda's
parameter list.** A parameter may carry a default, a default is an expression,
and the token after that expression is the `|` that closes the list — so
`(|x = 1| x)` read the closing pipe as bit-or and swallowed the body. The parser
carries a `no_pipe` flag over the parameter list, the way it already carries
`no_brace` to keep a record literal out of an `if` condition.

This is written down because the reasoning that missed it is the interesting
part. Enumerating where a token appears in *well-formed* source is not the same
question as where the parser can be when it meets one: a lambda default is
`E0120`, refused but parsed anyway, so the swallow only showed up on input that
was already an error. Nothing in the workspace suite caught it. The parser
spike's differential did, on one hand-written recovery fixture
(`fixtures/35-err-named-arguments-and-updates.ply`), where recovery went from
ten items to seven.

**`^` and `~` are new characters.** Neither appears in the token set today, so
neither can collide.

**`>>` is deliberately not lexed.** `Map<Int, List<Int>>` parses today precisely
because `>` is one token and `>>` is not; making it one re-introduces the split
the ported parser carries a `gt_split` flag for. So the *expression* parser joins
**two adjacent `Gt` tokens** — adjacent meaning the second begins where the first
ends, with no trivia between — into a shift, and three into `>>>`. The type
parser is not touched, and `a > > b` stays the syntax error it is today.

### §2.1 Precedence

Rust's order, which is C's with `&`/`^`/`|` moved above comparison where they
belong:

| new | operators | was |
| --- | --- | --- |
| 1 | `\|\|` | 1 |
| 2 | `&&` | 2 |
| 3 | `==` `!=` `<` `<=` `>` `>=` | 3 |
| 4 | `\|` | — |
| 5 | `^` | — |
| 6 | `&` | — |
| 7 | `<<` `>>` `>>>` | — |
| 8 | `++` | 4 |
| 9 | `+` `-` | 5 |
| 10 | `*` `/` `%` | 6 |

**The relative order of every existing operator is preserved**, which is the
property that matters: the numbers renumber, no existing program's parse tree
changes, and therefore no existing definition hash moves and no cached test
result is invalidated. `binop_byte` (`crates/ply-hash/src/normalize.rs:133`) is
an explicit table rather than a discriminant cast, so the new operators take new
bytes — 15 and up — and the old ones keep theirs.

### §2.2 Semantics, and the one deliberate exception to checked arithmetic

`&`, `|`, `^` and unary `~` operate on the two's-complement bit pattern of `Int`
and are defined at `Int` only. `Bool` has `&&`, `||` and `!`; a `&` between two
`Bool`s is `E0201` naming both, rather than a silently-different short-circuit.

A shift count outside `0..=63` **raises** `E0502`, for the same reason `/` by
zero does: there is no answer, and inventing one (C's undefined behaviour, Rust's
`shl` panic, Java's silent mask by 63) is the defect the checked-arithmetic
design exists to prevent.

`<<` **discards the bits shifted out** and does not raise on overflow. That is an
exception to §5.1's "arithmetic is checked" and it is deliberate: a shift is a
bit operation, not a multiplication, and a hash's mixing step is *defined* to
drop those bits. `>>` is arithmetic (sign-propagating); `>>>` is logical
(zero-filling). Both are in the language because `Int` is signed and there is no
`UInt` — with only one of them, half of the published hash and checksum
algorithms cannot be transcribed.

`wrap_add`, `wrap_sub` and `wrap_mul` are builtins rather than operators, because
there is no punctuation for them that any reader would recognise and because the
checked spelling must stay the one that is easy to write. Note what they are
*not* needed for: BLAKE3 mixes 32-bit words, and two values below 2^32 added in
`i64` cannot overflow, so masked arithmetic suffices there. They are for the
64-bit case — xxhash, SipHash, an LCG — which is what "general purpose" means.

### §2.3 Two more, each a wall the first two ran into

**Hex literals: `0xFF`, `0xdead_beef`.** A bit surface without them is half a
feature — `mask32(x) = x & 4294967295` is not a mask anybody can check against a
specification, and BLAKE3's IV is eight constants that exist in hexadecimal
everywhere they are published. It is a lexer change and nothing else: the token
is `Lit::Int`, so `0xFF` and `255` are one literal, one definition and one hash,
and a program that changes base re-runs nothing. The bound is `u64` and the
value is reinterpreted, so `0xFFFF_FFFF_FFFF_FFFF` is `-1` — the bit pattern the
literal names, which is the only reading useful to the operators above. A
decimal literal keeps `i64`'s range and its own diagnostic.

**`byte_of_int(n: Int) -> Bytes`.** `bytes_at` answers an `Int` and nothing
turned one back, so every program that had to emit a byte it computed carried a
256-escape `b"\x00\x01…"` literal and sliced it. `spikes/ply-lexer/GAPS.md` §7
counts four sightings of that workaround across two shipped files; §3 below met
it a fifth time, writing a hash's own output, which is what makes it a wall
rather than a wart. Out of range raises rather than masking, for the reason a
shift count does: a silent `& 0xFF` writes a byte nobody chose.

## §3 Decision 2 — BLAKE3 is written in Ply, and there is no builtin

A `bytes_blake3` builtin was on the table and is refused. `std.hash.blake3` is an
ordinary Ply definition in `crates/ply-std/ply/hash.ply`.

**What that buys.** The function that decides the identity of every definition in
the language becomes a definition in the language, which a reader can read and a
test can shrink. And it is the only honest proof that §2's surface is *enough*:
a bit surface justified by "you could write a hash with it" and never used to
write one is a claim, not a mechanism.

**What it costs, stated before it is measured.** Two implementations of one
function that can disagree, and an interpreter running the build's hottest inner
loop. Both are real and neither is hypothetical.

**The criteria, fixed here before the measurement exists** — `CONTRIBUTING.md`
§"Measure an ADR's motivating claim before accepting the ADR" and §"An ADR", and
they are written as thresholds a number cannot move after the fact:

- **T1, correctness.** `std.hash.blake3` agrees byte for byte with the `blake3`
  crate `ply-hash` already links, over a corpus that crosses every structural
  boundary in the algorithm: 0, 1, 63, 64, 65, 1023, 1024, 1025, 2048, 4096 and
  10,000 bytes, plus the published test vectors. **The threshold is zero
  disagreements.** One disagreement and this decision is wrong — not the
  implementation, the decision — because a second implementation of the identity
  function that is *nearly* right is worse than not having one.
- **T2, throughput.** `spikes/ply-lexer/GAPS.md` §15 measured the Ply lexer at
  **85 KB/s** and concluded that a self-hosted front end at that speed "would be
  the slowest thing in the build". The bar is that hashing must not be worse than
  the phase that is already the worst: **≥ 85 KB/s, measured on the same machine
  under `CONTRIBUTING.md`'s load gate.**

### §3.1 What the two thresholds produced

**T1 — passed, zero disagreements.** `crates/ply-eval/tests/blake3_differential.rs`
hashes the same input with `std.hash.blake3` and with the `blake3` crate at
0, 1, 63, 64, 65, 1023, 1024, 1025, 2048, 3072 and 4096 bytes — every structural
boundary the algorithm has: the block, the chunk, and the tree above it. All
eleven agree byte for byte, and the published vectors for the empty and one-byte
inputs hold independently, so the two implementations are not wrong together.
The decision in §3 stands on the evidence it named in advance.

**T2 — not decided, and the bar is not moved.** The figure is 65,536 bytes
hashed, minus a baseline that builds the same input and does not hash it:

| series | load (1-min, start → end) | runs | min | throughput |
| --- | --- | --- | --- | --- |
| A | 10.5 → 9.4 | 5 | 1.26 s (baseline 0.04 s) | **53.7 KB/s** |
| B | 10.4 → 15.2 | 15 | 2.07 s (baseline 0.09 s) | **33.1 KB/s** |

Instrument: `target/release/ply`, `.github/binary-is-current.sh` printing
`current` before the series.

**Neither series is gate-compliant and the verdict is therefore withheld rather
than assumed.** `CONTRIBUTING.md` §"Gate on an idle machine" sets 4.0 at the
start and 6.0 at the end; this machine did not go below 9 at any point in the
session that produced these numbers, and ranged to 77 while a build and the test
suite ran. The bias has a known direction — contention only makes a wall clock
larger — so **each figure is a lower bound on throughput**, which is why series
B, taken while the load was climbing, reads *worse* than series A rather than
confirming it.

So the honest reading is: the best observed figure is **53.7 KB/s against a bar
of 85 KB/s**, and the shortfall (1.58×) is inside what the gap between load 10
and an idle machine could account for. **That is not a pass and it is not a
fail.** The run that settles it is series A repeated under the gate, and until
that exists this ADR carries a threshold it did not meet and did not clear —
which is the state the pre-registration was written to make visible instead of
letting a number be interpreted after the fact.

**What follows if the idle run lands below the bar** is what §3 already named:
a `bytes_blake3` builtin, with `std.hash.blake3` kept as the readable definition
and `blake3_differential.rs` kept as what holds the two together. Nothing about
that follow-up is contingent on the argument being re-made — it is contingent on
a number.

**T2 was expected to fail, and that is why it is written down.** One BLAKE3
compression is 7 rounds of 8 mixing functions of about 12 operations, so a
kibibyte of input is on the order of 10⁴ interpreted operations; the lexer spends
a handful per byte. If the measured figure lands below the bar — as the arithmetic
predicts — this ADR does not get to call the result acceptable after seeing it.
The follow-up it names in advance is exactly the option refused above: a
`bytes_blake3` builtin, with `std.hash.blake3` kept as the readable definition
and a differential test asserting the two agree. That is the shape ADR 0027 used
for `list_at` and it is the shape a *measured* reversal of this decision takes.

## §4 Decision 3 — `std.fs`, a rooted `nondet` effect

```ply
pub nondet effect fs {
  read  read_file[r](path: String) -> Option<Bytes>
  read  list_dir[r](path: String) -> Option<List<String>>
  read  exists[r](path: String) -> Bool
  read  file_size[r](path: String) -> Option<Int>
  read  modified_ms[r](path: String) -> Option<Int>
  write write_file[r](path: String, body: Bytes) -> Bool
  write create_dir[r](path: String) -> Bool
  write remove[r](path: String) -> Bool
  write rename[r](from: String, to: String) -> Bool
}
```

**`nondet` is load-bearing, exactly as it is in `std.net`.** A file read is not a
function of the program's state, so a `det` test that reaches one is `E0412` at
compile time until a handler discharges it — and the handler that discharges it is
an in-memory twin, which is the whole thesis (`DESIGN.md`, the "Setup" row) applied
to a filesystem. The declaration is the authority and the handler is what gets
refused: a handler bound over an effect declared without `nondet` is `E0423`.

**A resource label is a root, and the root is the capability.** `fs.read_file[src]`
performs `(fs, src, Read)`. The label is bound to a directory beside the run —
`ply run build.ply --host --fs src=./crates --fs out=./target` — the way `--tls
NAME=CERT,KEY` binds a credential and for the same reason: a path inside the
program would put a filesystem location into a definition's hash and into a store
designed never to forget. Three consequences, and the third is the one that pays:

1. an operation naming a label with no bound root is **`E0451`** **at the
   perform**, naming the label and the flag that would bind it.

   Two earlier-firing designs were tried and refused. Making `bind` refuse the
   atom puts filesystem knowledge in `ply-eval`'s generic resolution; registering
   `HostResource::Only` per bound root gets the timing for free but makes an
   unbound label resolve to *no handler at all*, which reports `E0424` and names
   a twin rather than naming the flag. `net.listen_tls` under a missing `--tls`
   is `E0429` at the perform for the same reason, and this is that sentence one
   capability along. The cost is real: a build that reads one root and writes
   another learns about the second when it first writes. `ply hosts` is the
   instrument that answers earlier, and it prints every bound root.
2. a path that escapes its root — `..` above it, an absolute path, or a symlink
   resolving outside — is **`E0452`**, refused before the syscall, with the root
   and the resolved target both named;
3. **two roots that do not overlap do not conflict**, so `EffectAtom::conflicts_with`
   already lets tests over `src` and tests over `out` run concurrently, and two
   readers of one root run concurrently while a writer serializes against both.
   That is the readers-writers rule the scheduler has had since M4, applied to
   directories for free.

**Modes are read and write in the ordinary sense**, and `read_file` is a `read`
where `net.recv` is a `write`. The difference is real rather than an
inconsistency: a socket read *consumes* from a kernel buffer, so two readers of
one socket race; two readers of one file do not.

**What v1 refuses**, each because the compiler driver this exists for does not
need it and each is cheaper to add later than to remove: no file handles and no
streaming (a read is whole-file — `E0453` when a file exceeds a bound the run
sets), no recursive walk (`list_dir` answers one directory; recurse in Ply), no
permissions or modes, no watching, no `stdin`/`stdout` (that is a separate
question and not this one), and no `argv`.

**`rename` is within one root**, which is what makes a cache write atomic:
write to a temporary name under `out`, then rename into place. Across two roots
it would be a copy with a different failure mode, and it would need two labels in
one atom, which the effect row cannot say.

**The twin ships with the declaration.** `std.fs` also publishes pure functions
over a `Map<String, Bytes>` — `mem_read`, `mem_write`, `mem_list`, `mem_remove` —
so that a test's handler is three lines and every project's in-memory filesystem
is the same one, checked against the same signature. `std.db`'s twin is the
precedent; this one is a hundred lines rather than two thousand because a
filesystem has no query language.

## §5 What this costs, by surface

Each row is a checklist item from `CONTRIBUTING.md` §"Adding things" or from
`CLAUDE.md`'s guide table, and none of them is optional:

| surface | what moves |
| --- | --- |
| `lexer.rs` | `&`, `^`, `~` tokens; the `E0001` message that says `&` does not exist |
| `parser.rs` | `bin_op` gains six rows; the expression parser joins adjacent `Gt` |
| `ast.rs` | six `BinOp` variants, one `UnOp` variant |
| `normalize.rs` | `binop_byte` rows 15+; **no existing byte moves** |
| `infer.rs` | typing for the new operators; three builtins in `install_prelude` |
| `builtins.rs` | three `Builtin` variants — and `Builtin::all()`, the pinned name list, and `arity()`, all four of which are checked by omission |
| the evaluator | `semantics.rs`, `machine.rs`, `code.rs`/`compiled.rs`, `costs.rs` — including which shift counts raise |
| `ply-prove` | new operators lowered, or explicitly unsupported so an obligation over one reports `property` rather than a wrong `proved` |
| `ply-codegen` | every `BinOp`/`UnOp` match, and they are **exhaustive with no wildcard on purpose** — a new variant fails to compile there rather than defaulting to something. The operators are **refused**, not lowered: `<<` discards where every other arithmetic operator raises, and a shift count outside `0..=63` raises where Cranelift's `ishl` masks it, so the machine instruction of the same name is not the same operator. A backend that answered where the evaluator raises is a divergence no differential covers, because a backend is not a second evaluator |
| `ply-span` | four codes, each with a `codes` constant **and** a registry row **and** a production construction site |
| `ply-host` | a new handler module, registered, appearing in `ply hosts --host` with footprint, determinism, linearity, blocking and secrets — and the digest moves |
| the composed runtime | **three** methods route a pending token, not one: `poll`, `park` **and** `block_on`. A facility wired into two of them submits jobs nothing can resolve, and `ply run` — which blocks rather than parking, having one task — met it as `E0505` on the first `fs` operation any program performed. Found by running the thing, not by reading it |
| `ply-cli` | `--fs NAME=PATH`, repeatable, refused without `--host` the way `--tls` is |
| `crates/ply-std/ply/` | two new modules; `ply std`'s counts **and** digest both move |
| `docs/GUIDE.md` | §3.5, §5.1, §13, §14, §15.2, §17, §18, §19.2, §19.3 |

## §6 What would make this ADR wrong

- **T1 fails.** A Ply BLAKE3 that disagrees with the crate on any input in the
  corpus refutes §3, and the builtin returns.
- **T2 fails and the follow-up is not taken.** Recording a number below the bar
  and calling it acceptable is the defect `CONTRIBUTING.md` §"The one rule"
  counts seven of.
- **A rooted filesystem turns out to be the wrong capability granularity** — if
  real programs bind one root at `/` because splitting them is tedious, then the
  label carries no information and `E0452` is ceremony. The check is whether
  anything in the tree binds more than one root and means it.
- **The operators move a hash.** Any pinned digest that moves for a program
  containing none of the new tokens refutes §2.1, and the renumbering is wrong.
- **`nondet` on `fs` turns out to be too strong.** If every test that touches a
  file ends up `test/nondet` rather than handled by the twin, the effect is
  buying nothing and costing a keyword.

## §7 Provenance

Machine and load gate: `docs/ONBOARDING.md` §Provenance and `CONTRIBUTING.md`
§"Gate on an idle machine before measuring, not after". Instrument:
`target/release/ply`, with `.github/binary-is-current.sh` printing `current`
immediately before and after every series. §1's two claims were checked against a
binary rebuilt for the purpose; the diagnostics quoted are transcribed from runs
rather than sketched. T1 and T2 are stated here **before** any implementation
exists, and the numbers they gate go in §3 beside them.
