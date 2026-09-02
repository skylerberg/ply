# The compiled value model against Rust, on two kernels: what will be measured, and what will count as an answer

Written before either kernel exists. ADR 0035 decides the model and names this
file as its gate; `analyze.py` is the only thing that turns the raw file into a
verdict and it carries the bar, so a number cannot set it afterwards. The
protocol is ADR 0030's, as `benches/front-end-whole` took it: counterbalanced
arms with a null control, minimum user CPU over blocks, timed from outside the
process, load recorded on both sides, the binary checked current before and
after, no series spanning a rebuild, and a series taken above the load gate
reported as an observation rather than a figure.

## Question

Does compiled Ply on the value model ADR 0035 decides come within a small
factor of Rust on the two shapes a compiler's front end is made of — integer
arithmetic in a loop, and one state record threaded through a loop and updated
each step? Not "is it faster than the interpreter" — that is already known —
and not "is it as fast as a vectorised library", which is not the question.

## The kernels

| kernel | Ply | Rust |
| --- | --- | --- |
| K1, integers | `std.hash.blake3` over a fixed input of bytes `i % 251` | a scalar transliteration of `crates/ply-std/ply/hash.ply`: the same rounds, the same masks, no SIMD, no lookup tables the Ply does not have |
| K2, records | a fold over a fixed list that threads one record — counters, a list, an ordered map, bytes — and updates some fields each step | the same loop over a struct updated in place, with a `Vec` and a `BTreeMap` |

Both kernels' inputs are built inside the program, so the two arms hash and fold
identical data and no file is read: `run.sh` writes the K1 input beside
`kernels.ply` as a byte literal, with the digest the Rust bar printed, so the
Ply test asserts the digest and a transliteration that drifts fails a test
rather than skewing a comparison. The Rust kernels live in
`benches/value-model/rust`, a release binary with no dependencies that repeats
each kernel and prints its minimum time, so the bar is taken on the same machine
in the same sitting as the Ply arm and never against a figure written down
elsewhere; the Ply arm's time is each test's own duration from `--json`, one
worker, and its minimum is over the blocks. The transliteration rule for K1 is
the one a reviewer holds it to: a Rust that is faster because it is a different
algorithm is a broken bar.

## Arms

| arm | command |
| --- | --- |
| `ply` | `ply test <dir> --no-cache --jobs 1 --filter kernel: --backend cranelift` |
| `null` | the same command under a different label — the control |
| `rust` | `benches/value-model/rust/target/release/kernels` |
| `floor` | `ply test <dir> --no-cache --jobs 1 --filter kernel:nothing-matches` — the run's fixed cost |

Every arm sits in every position across three blocks. Per-kernel durations are
read from `--json` for the Ply arm and from the binary's own report for the
Rust arm; the floor is subtracted from neither, since it is reported beside them
and the Rust binary has its own, smaller, fixed cost.

## Statistic

Per kernel, the ratio of the Ply arm's minimum time to the Rust arm's minimum
time over the blocks. The null control's distance from `ply` is the resolution;
a ratio within the resolution of the bar is reported as undecided, not as a pass.

## Decision rule

`BAR = 3.0` in `analyze.py`. `modelStands = true` iff both kernels' ratios are
at or under the bar. K1 over the bar names ADR 0035's Decisions 2 and 5; K2 over
the bar names Decisions 1 and 4; both over and the model is refuted as designed.

## Baseline

The series is taken first on the fragment as it stands before any of ADR 0035
is built, so the change is read against a baseline taken by the same protocol,
and so the kernels are known to run under the backend before the model exists to
run them on.

## Predictions, registered

1. On today's fragment K1 is over the bar by an order of magnitude or more: every
   word of the hash's state is a field read through the runtime.
2. On today's fragment K2 is over the bar by more than K1: the record path pays
   the name search, the atomic count and the arena on every step.
3. After ADR 0035's sequence steps 2 and 3, K1 clears first and K2 second, in
   that order, because typed calls and unboxed locals are the whole of K1 and
   only half of K2.

## The baseline, taken

`baseline.txt`, under the gate on both sides. Prediction 1 held, by more than it
said: K1 is nearly three orders of magnitude over the bar. Prediction 2 did not:
K2 is the *closer* kernel by far, within a factor the record path's name search
and counts alone do not explain. What that says, before anything is built: the
integer path — a boxed call for every `mask32` and `rotr`, a record of four
words built and torn down per `g`, and a field read through the runtime for
each of its words — is where the model is furthest from Rust, and the record
kernel's distance is mostly the ordered map and the list, which sequence step 5
reaches last. Prediction 3's order stands.

## After sequence steps 2 and 3

`after-words.txt`, under the gate on both sides. Neither kernel clears, so
prediction 3 is not yet decided; its order held in direction — the integer
kernel halved and the record kernel moved by a fraction — with the calls typed
and the values laid out as words but every field still read by name through the
runtime. What the reading says: the integer kernel's remaining distance is the
record path inside it — a `Quad` built and read per mixing step — and not the
arithmetic, so the static half of ADR 0035's Decision 1 comes before anything
else.

## After the layouts

`after-layouts.txt`, under the gate on both sides, with field reads at known
offsets, a per-shape offset table for the rest, a bump allocator and the memo
copied out. Neither kernel clears yet. The integer kernel moved by more than
the record kernel again, so prediction 3's order still holds in direction, and
what moved it was not the field loads — those changed little on their own —
but the string compares the by-name path was paying and the allocation on
every release. What the reading says now: the integer kernel's distance is
the record built and torn down per mixing step, which no helper change
reaches; it needs the step inlined and the record kept in registers.

## After the drops

`after-drops.txt`, under the gate on both sides, with every binding released
at its scope's end, every tail owned, and byte concatenation native. The
integer kernel did not move; the record kernel moved, and the concatenation is
what moved it — its key is built per step — not the drops, whose in-place
updates the last-use moves had already secured on this kernel. The drops
found something else: a branch answering a local aliased it at one count,
which the fragment's cases now pin. Neither kernel clears.

## After the inlining

`after-inline.txt`, under the gate on both sides, with small callees inlined
before lowering, field-only records split into their fields, every field of a
fixed shape read inline — a scalar into a register — and the count-down
allocating nothing. The integer kernel moved by a large factor, and the
profile before this step said why in advance: half its samples were the
by-name field read the ownership marks had been routing around the static
path, and a third the work list the count-down allocated per dying record.
The record kernel moved by a fifth. Neither kernel clears. What the reading
says now: the integer kernel's records that remain are the sixteen-word
states of functions over the inliner's budget, and the record kernel's time
is in bridged keys and bytes — string compares and concatenations done by the
interpreter — which is the next step's, not this one's.

## After the strings and the list

`after-strings-and-lists.txt`, under the gate on both sides, with strings and
bytes native with room to grow, their builtins answered over the payloads,
and the list a trie with typed leaves after ADR 0034's representation gate
refused the array. The record kernel moved by about a quarter on the strings,
since its keys are bytes it builds and compares every step, and gave a few
percent back on the list, which is the bookkeeping a short list's tail
carries; the integer kernel did not move, because nothing in it is a string
or a list. Neither kernel clears. What the reading says now: the integer
kernel's distance is its state records and its callback loop, and the record
kernel's is the map — a sorted array probed by compare — and the record
update itself; the seam's census (step 7) is what says whether conversion is
anywhere in the front end's rows before those are re-taken.

## The re-take

`retake.txt`, under the gate on both sides, on the binary with every sequence
step landed. Both kernels are over the bar: the integer one by an order of
magnitude and the record one by a little. **The decision rule as registered
applies as written: both outside, and the model as designed is refuted,
which ADR 0035's opening now says.** What the rule names to revisit is in the
opening too. The seam's census over the front-end probe answered Decision 6's
question on the way: a row that keeps its tree inside compiled code costs
what a row that converts it out does, so conversion is not where any row's
time is.

## After the tokens

`after-tokens.txt`, under the gate on both sides, with the inliner's blocks
opened all the way down so a small callee's record answer is scalars in the
caller, the smallest leaves inlined at any depth, and a record dying in a body
that builds another of its width kept as that record's memory (ADR 0036,
Decision 8), and a lookup a match unwraps at once answering the value with
no constructor between, beside constructor tests as loads and byte strings
compared as bytes first (Decision 9). The integer kernel moved by more than
half, since its round is now straight-line arithmetic over one record it
rebuilds in place; the record kernel moved by a third, on the key compares
of its probe and the constructor its lookup no longer builds. The bar stands
as registered: the record kernel is inside it and the integer kernel still
outside, by a smaller factor. What the reading says now: what remains of the
integer kernel is the compiled arithmetic itself — every add checked, every
word masked, as the source writes it — and the records the source keeps alive
by shape.

## After the direct builtins

`after-direct.txt`, under the gate on both sides, with the builtins the rows
call most as direct calls from compiled code and the empty list, the empty
map and every nullary constructor made once (ADR 0036, Decision 10). The
record kernel moved by a little, on the inserts and the byte singletons its
step no longer dispatches; the integer kernel did not move, since its round
calls no builtin. The bar stands as registered and the verdict is the last
series': the record kernel inside, the integer kernel outside by the same
factor. What the reading says now is unchanged for the kernels; it is the
front end's rows that moved on this step, and `observation-6.txt` is where
they are read.

## After the rounds

`after-rounds.txt`, under the gate on both sides, with a fused loop's literal
step lowered in the loop and its `Continue` and `Stop` as jumps, a flat
record's release walking nothing and its allocation storing fields in place,
and the round's rotate as the `rotr32` builtin over literal counts the
optimizer folds (ADR 0036, Decision 11). The integer kernel moved by about a
quarter on the runtime paths and again on the arithmetic; the record kernel a
little, on the step it no longer calls. The bar stands as registered and the
verdict is the last series': the record kernel inside, the integer kernel
outside by a smaller factor. What the reading says now: what remains of the
integer kernel is the arithmetic with each add checked and the state loaded
and stored per round, and the records the source keeps alive by shape.

## After the borrows

`after-borrows.txt`, under the gate on both sides, with a parameter a body
only reads borrowed for the call: the caller keeps its hold, the callee
neither counts nor releases it, and a value the caller needs again dies where
the caller's tokens are (ADR 0036, Decision 12). The integer kernel moved by
about a seventh, on the chaining value its chunk loop no longer allocates per
block and the holds its rounds no longer take on the message; the record
kernel a little. The bar stands as registered and the verdict is the last
series': the record kernel inside, the integer kernel outside by a smaller
factor. What the reading says now: what remains of the integer kernel is the
arithmetic with each add checked and the state loaded and stored per round.
