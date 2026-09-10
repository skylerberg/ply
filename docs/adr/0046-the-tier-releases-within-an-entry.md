# ADR 0046 — The tier releases within an entry

**Accepted and built.** ADR 0045 §"What the fixpoint measured" found that the
compiled tier never let go of a count within an entry: a temporary passed to
a builtin, an old record, a matched constructor and a callee's parameters
all stayed until `Ctx::end` recycled the arena whole. Six small programs run
tier-only leaked one object per iteration, the emitter emitting its own
sources peaked past a CI runner's memory, and a served program's `main`
would have leaked for as long as it served. That record named precise
release as the fifth stage's prerequisite and as the next record. This is
it.

> **What this decides.** That the emitter written in Ply transcribes the
> machine's ownership rule into the C it emits — a value is moved out of its
> binding at the read the lowering marked its last use, cloned at any other
> read, taken by whatever consumes it, and released at the close of the
> block that declared it — so that a compiled entry ends holding only its
> answer. That the reference emitter in Rust is *not* changed: the port
> keeps a second mode that emits the reference's text, and that is the text
> `emit_diff.rs` still compares body for body, so the lowering and the
> structure keep their byte-exact oracle while release is checked by the
> audits. That where the reference's evaluation order differs from the
> machine's, release follows the machine.
>
> **What it does not decide.** A failure path still leaks until the entry
> ends: a body that raises returns 0 through every frame, or jumps to a
> `handle`'s landing, releasing nothing on the way. That is bounded per
> failure and the arena still recycles at the entry's end, and it is
> recorded here rather than fixed because a failure is not a loop.

## The rule, as the emitter carries it

The machine's model, which `crates/ply-eval/src/rc.rs` marks and
`machine.rs` acts on: a window's slots own their values; a read marked
`Owned` takes the slot's value, any other read clones it; every value on the
operand stack is owned and every consumer takes what it is given; the
window is truncated when the activation ends. The C has no window, so the
emitter keeps the ledger itself, in `Em`:

* **`made`** is every C local holding one count, with the block depth it was
  declared at. A runtime helper's answer, a compiled call's answer, a
  constructed record, list, constructor or closure, a parameter, a `let`,
  a pattern binder, a join local and a fused loop's per-iteration state are
  all holders. A rename of a holder takes its count over — `Word t9 = t8;
  t8 = 0;` — and the source is nulled so that no later release finds it.
* **`bound`** is the subset a slot names. A bound holder is spent only by a
  read marked `Owned`, which moves it out under a temporary's name and
  nulls the slot's local; a `Borrowed` read is the name itself, and whoever
  consumes it holds it once more first, as before.
* **`pinned`** is what a fused loop's body may read but never move: the
  body is emitted once and runs once per iteration, so a mark that says
  "last use" is about the text. The loop's own state is renamed per
  iteration and is the iteration's holder.
* **A block's close** releases every holder declared at its depth or deeper
  and forgets them; a `break` releases from the loop's depth without
  forgetting what the enclosing blocks still hold on the paths that stay.
  An `if` or `match` arm releases its own before the join, and the join
  keeps the union of what either arm may still hold: the path that spent a
  holder left it null, so releasing it there is nothing. The right operand
  of `&&` and `||` is an arm in this sense too — the first cut forgot what
  it moved, and a lambda whose short-circuit skipped the branch leaked its
  captures on every call.
* **A helper that reads** is handed its arguments as they are held and
  releases nothing; `rt_equal` is the one the port calls that way, and
  every other helper it hands a value to takes it, which `rt.rs` says on
  each. The first cut handed equality its operands as taken, and every
  string sliced to compare leaked.
* **The epilogue** takes the answer's count first — spent from a temporary,
  added to a name still held elsewhere — and releases everything else,
  parameters included. A callee therefore takes its arguments and answers
  an owned word, which is the convention the runtime's helpers already had.

Three places the reference's text and the machine's order disagree, and
release takes the machine's side:

* **Argument lists evaluated in two passes.** The reference evaluates every
  argument of a builtin and only then holds each; between the first read
  and its hold, a later argument's read may be the binding's last use and
  move it. `cell_set(outbox, push(cell_get(outbox), payload))` did exactly
  that. Under release a borrowed value is held once more at the read, as
  the machine clones at the read.
* **A fused `fold`'s seed and step.** The reference builds the closure
  before it evaluates the seed; the machine evaluates the seed first, so a
  capture the closure takes at its last use is null when the seed reads it.
  Under release the seed comes first.
* **A `handle`'s inline body.** A failing call jumps to the landing label
  past the declarations after it, and a release at the function's end would
  read locals never initialised. Under release the body is a block of its
  own, released at its close and out of scope at the label.

And two sites the reference's ownership had wrong in a way the arena hid: a
`with_cell` initial value handed to `rt_cell`, which takes it, without being
held once more; and a record update whose base has no shape in reach, whose
copies the runtime answers held once more and the new record takes, which
were left on the release list too. The heap's dead-object check found the
second; the first is fixed under release and left as it was in the
reference's text. The emitter's own sources were the workload that found
the last two rules above: with them, the entry that emits those sources
recycled nothing and ended with forty million byte strings live.

## What checked it

The oracle is the audit that pairs every test with the machine, run with
the emitter in release mode producing the unit:

```sh
PLY_C_EMITTER=ply:spikes/ply-parser ./target/release/ply test examples --backend c --audit-backend --no-cache
PLY_C_EMITTER=ply:spikes/ply-parser ./target/release/ply test crates/ply-std/ply --backend c --audit-backend --no-cache
PLY_TIER_ONLY=1 PLY_C_EMITTER=ply:spikes/ply-parser ./target/release/ply test examples --backend c --no-cache
PLY_TIER_ONLY=1 PLY_C_EMITTER=ply:spikes/ply-parser ./target/release/ply test crates/ply-std/ply --backend c --no-cache
```

All four are green. `crates/ply-codegen/src/heap.rs` refuses to read a word
whose object died, and under the audit that refusal is what turned each
ownership defect into a named failure rather than a wrong answer; it
caught the double release above and the two-pass hazard. The ratchet
`spikes/ply-parser/harness/tests/emit_diff.rs` compares the port's
reference mode, which is unchanged, and the port's own tests pass under both
engines.

The measurement is `PLY_C_PHASES=1`, which prints what an entry allocated,
recycled and left live, by kind:

```sh
PLY_C_PHASES=1 PLY_TIER_ONLY=1 PLY_C_EMITTER=ply:spikes/ply-parser ./target/release/ply run --backend c f.ply
```

For the fold over fifty thousand records that ADR 0045 measured, the entry
went from 51618 objects allocated and 50001 records live at the end to 1618
allocated and none live. Seven loop programs of two hundred thousand
iterations — bytes concatenated and dropped, a record rebuilt, a constructor
matched, a list pushed then folded, a record updated in place, a fold over a
compiled step, an update chain — each recycle within a few objects of what
they allocate and leave a fixed baseline live, which is the unit's own
tables and not the loop's.

The entry that emits the emitter's own sources is the workload the gates
were waiting on. Through the refreshed bundle the port's own tests run
tier-only in under a gigabyte, the bootstrap fixpoint test runs in under a
gigabyte and under a minute, and the port's own-sources ratchet in under
three, so all three run in CI again. The harness's differentials run the
port with the tier attached, and `run.sh` now names the bundle as the
producer for them: with the reference producing, the port ran as C that
released nothing, over its own sources, which is the memory the runner
could not hold. One thing stood between the fixpoint
and a runner even so: a debug build's heap never reuses a dead block, so
that a read of one finds the marker, and that entry allocates two hundred
million objects. `heap::reuse_by_default` turns reuse on for a process, the
fixpoint test calls it, and the audits — release builds, and the check's
real oracle — do not.

## What would make this wrong

* A read the port emits in an order the machine does not. Every move rests
  on the lowering's mark being about the machine's order, and the port's
  order is the reference's except where this record says otherwise. A new
  fusion or a reordered site is a place to look first.
* A helper that reads its argument where the emitter believes it takes it,
  or the reverse. `rt.rs` says which on each helper, and `secure` and
  `owned` assume it.
* A path out of a block that is neither its close nor `break` nor a failure.
  There is none today.
