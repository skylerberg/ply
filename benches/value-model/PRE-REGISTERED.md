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
