# ADR 0037 — The loop is the goal; the dependency line is a second one

**Accepted as a direction and an ordering. It decides one refusal and registers
one measurement; it builds nothing.**

`docs/BOOTSTRAP-PATH.md` carried one goal with two halves fused: a verification
loop whose cost is O(the change), and a dependency line drawn where Rust's is —
a C compiler and libc, with everything that has language content above it. They
were written as one path ending in one place, and the fusion hid a conflict.
**They are two goals, they are served by different work, and in the place the
fusion put it one of them is hostile to the other.**

- **The loop.** ADR 0021's claim: Ply's verification loop is O(the change) and
  every toolchain it competes with is O(the project). That is the thesis, and it
  is what makes the language worth building for the world ADR 0021 says it is
  built for — one where inference is fast and tooling is the whole of an agent's
  wall clock.
- **The line.** Every language rests on a host it did not write. Ply's sits far
  above Rust's: the evaluator, the code generator through Cranelift, the runtime
  helpers, the driver and the host effects are all Rust.

**The loop is the goal. The line is a property the goal does not require.**

> **What this decides.** That the loop's compiler tier is in-process and stays
> in-process; that emitting C is **refused inside the loop** and kept for
> release, portability and the bootstrap chain; that the end state is therefore
> two tiers rather than one, both eventually hostable without Rust; and that
> what orders any of it is the marginal cost of one edit, whose criteria are
> registered below before the row is taken.
>
> **What it does not decide.** That C is the release target — ADR 0021's path
> already carries that as a direction and this record does not re-take it. Nor
> which loop tier replaces Cranelift: the candidate is named with its trade and
> listed rather than chosen.

## Why C is refused in the loop

A C compiler is a process, and a program is a link. Neither cost is marginal:

- **Per invocation**, a process spawn and a parse of every header the unit
  includes, paid whether or not the definition changed. Caching an object by
  content removes the compile and not the spawn.
- **Per program**, the link is whole-program. Incremental linking is weak
  everywhere it exists, and nothing makes a link proportional to an edit.

A loop built on emitted C therefore has a floor that is O(project) by
construction, and ADR 0021's whole claim is that the loop is not. **Both cannot
be true of the same tier.**

This is not only a prediction. **Lean 4 is the instance**: reference counting
with reuse from the same lineage as ADR 0034, self-hosted, emitting C, and used
at a scale that settles whether the approach works — with compile times as the
standing complaint against it. What Lean pays is what a C loop costs, paid by a
project that wanted several of the same things this one does.

### Priced and rejected

**A translation unit per definition, with an object cached by content.** The
compile becomes marginal and nothing else does: the spawn is paid per changed
definition and the link stays whole-program. Recorded so that it is not
re-derived from the appeal of the first half.

**LLVM IR in place of C, for the line rather than the loop.** Two forms, both
losing. Linking LLVM trades a Rust dependency for a larger C++ one that is
harder to build and to bind, which is the dependency this goal exists to
shrink. Emitting textual IR trades a stable interface for one that is
explicitly not stable — it moves with the release, and the opaque-pointer
transition broke the out-of-tree text emitters that existed — and it still
shells out to a binary, at which point `cc` is the simpler shell-out. **The
serious LLVM consumers link the library in process; emitting its text is rare
and is rarer the more the emitter is maintained.** C is the only one of the
three that makes the dependency smaller than the one it replaces.

## The two tiers

| tier | what it serves | what it must be |
| --- | --- | --- |
| the loop | `ply check`, `ply test`, `ply run` over an edit | in process; no spawn, no link; compile latency near zero; fast enough that the interpreter is not the fallback |
| release | `ply build`, distribution, the bootstrap chain | emitted C over libc, portable, debuggable, and buildable from source with a C compiler alone |

Cranelift is the loop tier today and it is a Rust library, so the line's goal
eventually takes it away. **What replaces it is not decided here.** The
candidate worth naming, because it collapses most of the work, is
**copy-and-patch**: stencils compiled by a C compiler at build time, and a
run-time code generator that is a copy and a relocation patch. It puts the C
dependency at build time and out of the loop, needs no register allocator, and
trades code quality down to about what an unoptimising compiler emits. Listed,
not chosen — what would decide it is the row below together with how long the
loop's tests actually run, and neither reading exists.

## The loop's own gaps

Three. The first two are holes rather than slow paths, and none of them is
mentioned by the path this record reorders.

- **Compiled code does not persist.** The front end is cached by content —
  `DefHash -> interface`, `crates/ply-store/src/frontend.rs` — and a test is
  selected against the definition set it last passed under, which
  `ply_store::PassRecord` holds as the test's own hash plus every function and
  declaration in its closure by hash. Both are already O(change).
  `crates/ply-codegen` persists nothing: no `DefHash -> code`, and it depends on
  `cranelift-jit` and not `cranelift-object`, so there is no object output for a
  cache to hold. **Every `--backend` run compiles the whole reachable program
  again**, and a `.plyx` carries definitions rather than code, so a built
  artifact does not change that. The backend is the one stage of the loop still
  O(project) on every invocation, and it is the stage the loop now depends on
  for its speed.
- **Every invocation is cold.** There is no `watch`, no daemon and no server in
  `Command`; a run starts a process, reads the caches from disk and exits. A
  warm process holds interfaces, compiled code and selection in memory and
  sidesteps serialising compiled code at all, which is the harder half of the
  gap above.
- **Nothing measures an edit in the loop.** Every row under `benches/`
  measures work proportional to the project — parse the standard library, check
  everything, hash everything — and where those pre-registrations say "warm"
  they mean *the cache was warmed so that it would not vary*. `ply-corpus w5`
  times one edit, on the deploy path and at one size. **No row prices the
  marginal change in the loop**, which is the only quantity ADR 0021's claim is
  about.

## The row, registered before it is taken

**Not built, but not from nothing.** The reading does not exist and the harness
is a composition of two that do: `ply-corpus sweep` generates a project at each
of several sizes and benchmarks whole-project phases over it, and `ply-corpus
w5` times a rebuild after a one-leaf edit at one size on the deploy path.
Neither applies an edit across two sizes, and neither runs under `--backend`.
This section is the criteria, written first so that a number cannot set the bar
it is about to clear.

**Question.** What does one edit cost, end to end, and does that cost grow with
the size of the project?

**Arms.** Each is an edit applied to a checked-out tree, followed by `ply test`
to green:

| arm | the edit |
| --- | --- |
| `leaf` | a body-only change to a definition nothing else depends on |
| `hub` | a body-only change to a definition much of the suite reaches |
| `rename` | a rename, which changes no hash |
| `signature` | a change to a declared type, which moves every dependent hash |
| `cold` | the same tree with the caches discarded — the O(project) bound |

Against the control the thesis is actually about: **the same five edits made to
the Rust tree**, `cargo nextest` to green, which is the O(project) toolchain
ADR 0021 names as the competitor.

**Statistic.** Minimum user CPU to green over blocks, under ADR 0030's protocol
— counterbalanced arms, a null control, the load gate read before and after, the
binary checked current on both sides, no series spanning a rebuild.

**Decision rule, and it is a slope rather than a threshold.** The claim is about
an exponent, so no single project size can test it: **take every arm at two
project sizes and fit.** `leaf` and `rename` must be flat in project size;
`cold` must not be. If `leaf` grows with the project then the loop is O(project)
whatever its constant, and whatever causes that growth — today, the backend's
missing cache — is what to fix before anything else in the path is worth
ordering. `hub` and `signature` are read for where the knee falls, not against a
bar.

That rule is CONTRIBUTING §"Measure an ADR's motivating claim before accepting
the ADR" applied to the claim this whole project rests on. Note what skipping it
has cost so far: ADR 0021's claim has been load-bearing for every bootstrap
decision in the tree and **has never been measured as a slope.**

## What would make this wrong

- **If `leaf` is already flat and already small.** Then the loop is what it
  claims, the two holes above are theoretical, and the ordering this record
  changes should go back to the one it found.
- **If a warm process removes the backend's cost by itself.** Then persisting
  compiled code is never needed and the daemon is the whole of that work rather
  than half of it.
- **If the loop tier's code quality decides the loop rather than its latency.**
  A tier that compiles instantly and runs the tests several times slower loses
  to one that compiles slowly and runs them fast, once the tests run long
  enough; which regime the loop is in is a property of the suite, not of the
  compiler. The row above does not separate the two, and a row that varied test
  length would.
- **If C's floor is smaller than this record assumes.** The spawn and the link
  are asserted here from how C toolchains work and are **not measured on this
  tree**: one timing of `cc` over a generated unit, and one link of the whole
  program, would settle it. Per CONTRIBUTING §"Say how it was checked, or say it
  was not": not checked.
