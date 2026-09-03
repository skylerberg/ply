# ADR 0037 — The loop is the goal; the dependency line is a second one

**Accepted as a split and an ordering. It decides that two goals were carried
as one and which comes first, registers the row that orders the work under it,
and takes one row it needed — `benches/c-floor/`, which prices what a C
toolchain charges per definition on this machine. It refuses nothing.**

`docs/BOOTSTRAP-PATH.md` carried one goal with two halves fused: a verification
loop whose cost is O(the change), and a dependency line drawn where Rust's is —
a C compiler and libc, with everything that has language content above it. They
were written as one path ending in one place, and they are served by different
work.

- **The loop.** ADR 0021's claim: Ply's verification loop is O(the change) and
  every toolchain it competes with is O(the project). That is the thesis, and it
  is what makes the language worth building for the world ADR 0021 says it is
  built for — one where inference is fast and tooling is the whole of an agent's
  wall clock.
- **The line.** Every language rests on a host it did not write. Ply's sits far
  above Rust's: the evaluator, the code generator through Cranelift, the runtime
  helpers, the driver and the host effects are all Rust.

**The loop is the goal. The line is a property the goal does not require** —
and the loop has the property today in one tier and not in the other.

> **What this decides.** That the compiled loop is O(project) at every stage a
> backend run makes, by three mechanisms named below, and that the first of
> them is a decision about what a cached pass claims rather than a cache to
> build. That the loop's tier is whatever compiles O(change) definitions per
> edit and loads what a run's selected tests reach, at a per-definition
> constant the loop affords — and not, once such a cache exists, whatever
> compiles fastest. That emitted C inside the loop is a constant-factor
> question rather than an exponent one, measured by `benches/c-floor/`, which
> refuses one C shape — an image per definition, whose load cost grows faster
> than the reach — and leaves the other open. And that the row which orders
> all of it is `ply-corpus bench`'s edit scenarios across the size ladder,
> fitted, with the backend arm it lacks, whose criteria are registered below
> before the reading.
>
> **What it does not decide.** That C is the release target — ADR 0021's path
> already carries that as a direction and this record does not re-take it. Nor
> which loop tier replaces Cranelift: the candidates are listed with the trade
> each makes, and what would choose among them is named.

## The loop today, checked

The interpreter's loop has the property. The front end is cached by content
(`crates/ply-store/src/frontend.rs`), a test is selected against the definition
set it last passed under (`ply_store::PassRecord`), and `ply-corpus bench`
prices an edit — `cold`, `warm`, `rename`, `edit-leaf` and `edit-hub`, nine
phases each — at every size `ply-corpus sweep` is given. `README.md` §"The
loop" is one size of that row.

The compiled loop did not have it, and every stage of a backend run was
O(project), for three reasons that read in `crates/ply-cli/src/commands/test.rs`
and `crates/ply-codegen/src/backend.rs`. The first is fixed; the other two
stand:

- **The caches were bypassed — both of them, for one cache's reason. Fixed.**
  `cache_bypassed` was true whenever `--backend` was given, so the run opened a
  scratch store, and the runner refused to record a test that entered compiled
  code. The reason given was about results only: a stored `Pass` is a claim
  about what the authoritative engine did. But one store holds both caches, so
  the same flag also cleared `incremental`, and a backed run re-parsed,
  re-resolved and re-checked the whole program — work that is the same whichever
  engine executes afterwards. Both halves are decided rather than fused now: a
  result names the engine that earned it (`ply_test::Engine`), so a backed run
  selects against what backed runs proved and neither engine reads the other's;
  and the front-end cache is read whatever executes. What still gets no store at
  all is a backend that is **wrong on purpose**, since a run that skipped a test
  is not evidence. The cost is a cold first run per engine, which this record's
  own falsifier named as the acceptable outcome.
- **The unit is whole and compiled per worker.** `Cranelift::over` closes the
  unit over every function the fragment compiles and builds it once as a
  pre-flight; `Provider::attach` builds it again for every worker. A run
  compiles the whole unit once more than it has workers.
- **Nothing persists and every process is cold.** `crates/ply-codegen` depends
  on `cranelift-jit` and not `cranelift-object`, so there is no object output a
  cache could hold and no `DefHash -> code` exists; there is no `watch`, daemon
  or server in `Command`, so every invocation starts from disk.

So the compiled loop's exponent is not a question the row needs to answer; the
code answers it. What the row prices is the constants — how much of an edit
under the backend is the front end, how much is tests re-run, how much is
compile — which decides whether the pass decision, the per-definition cache or
the warm process pays first.

## What the loop's tier must be

The requirement is on the edit, not on the compiler. An edit must compile the
definitions whose hash moved, and a run must load the code its selected tests
reach; both are O(change) in ADR 0021's sense, because selection already bounds
the reach. Once a cache keyed by `DefHash` holds compiled code, the compiler's
latency is paid per changed definition and its code quality is paid on every
test that runs — so a tier that compiles in microseconds and runs the suite
several times slower loses to one that compiles in milliseconds and runs it
fast, as soon as the suite runs longer than the compile. Compile latency near
zero is the requirement only while every run compiles everything, which is the
state this record exists to end.

Two consequences. The tier's compile speed matters up to the budget and not past
it; past it, its code quality decides. And loading by reach is itself a link —
`cranelift-jit` is an in-process linker, as every JIT is — so "no link" is not a
property any tier can have. What a tier can have is a link proportional to the
reach rather than to the program.

## Emitted C inside the loop: priced, and the cost is not where the argument put it

The argument against C in a loop is that a compiler invocation is a process and
a program is a link, so a C loop has an O(project) floor by construction. It is
the argument anyone reaches for, it is why this record's question exists, and
`benches/c-floor/` takes it on this machine — on units shaped like a compiled
Ply body over the runtime ABI, criteria in `PRE-REGISTERED.md` and the series in
`observation-2.txt`. **Both halves are wrong about where the cost is, and the
row found a third cost neither half names.** Written down so that the argument
is not made again from the appeal of its first half.

- **A process per changed definition is real and is a constant.** At `-O0` the
  spawn and the header are most of it and the code generation is the smaller
  part; optimising adds a fraction. It is tens of milliseconds per changed
  definition, and it does not grow with the project.
- **The whole-program link is not the exponent.** Over sixteen times the
  objects, the link costs under twice the time: it is dominated by a fixed cost
  and its marginal cost per object is microseconds. A link over everything a
  run reaches, at the largest size measured, is about a tenth of a second.
  "The link is whole-program, therefore O(project)" is true as asymptotics and
  false as a description of the loop's budget at the sizes this project
  measures.
- **Loading is per image, not per definition.** One library's first load costs
  the same whether it holds a few hundred definitions or a few thousand, and a
  later load of the same file is a fraction of a millisecond.
- **And the cost the argument missed: images.** Loading one image per definition
  costs *more* than N times loading one, superlinearly in N, and it holds
  whether each image binds its runtime symbols two-level or by flat lookup. At
  the largest per-image size measured, a warm load of one image per definition
  costs seconds where the same definitions in one library cost a fraction of a
  millisecond. That is a property of the loader, not of C, and it is the real
  floor under the design the first half of the argument makes attractive:
  compile each definition to its own image and load what you reach. **That
  design is refused, on this reading rather than on the link.**

**So the C shape that could sit in a loop is the opposite of the attractive
one.** An object cached per definition and compiled only when its hash
moves; one link over the objects the run reaches; one image loaded. Its floor is
a constant per run — a link and a load, each about a tenth of a second at the
largest size measured — plus the compile of what actually changed. Constants,
measured, not an exponent.

Whether that constant fits inside the loop is not this row's to say, because the
budget is not written down. The marginal-change row below is what sets it, and
what it must compare the constant against is what the same edit costs with no
process at all — which is what an in-process tier gives and what nothing has
measured for Cranelift, since the unit is compiled whole.

**Lean 4 is the precedent for the split, not evidence against C.** Checked
against Lake's source (`LeanLibConfig.lean` and `Facets.lean`, read
2026-09-02): a bare `lake build` of a library builds its `leanArts` facet — the
`.olean`, `.ilean` and emitted `.c` — and does not compile that C;
`precompileModules` defaults to false, and objects are built for an executable
or for a module the elaborator is asked to load natively. Lean's editing loop is
elaboration over its own artifacts, and its C is a release tier, which is the
shape this record proposes. The one Lean loop that does pass through C is the
compiler's own, rebuilt through the stage chain — the analogue of ADR 0021's
instance, Ply's own loop being a Rust loop, and not evidence about C in a user's
loop.

## Candidates for the loop's tier, listed and not chosen

| tier | depends on | per changed definition | per run | code quality | what it costs the tree |
| --- | --- | --- | --- | --- | --- |
| Cranelift over a `DefHash -> code` cache | a Rust library, until the line's goal removes it | a Cranelift compile — hundreds of microseconds, `benches/marginal-change/` | none; the code is already in the process | Cranelift's | a per-definition unit and a way to hold code across runs; keeps Rust |
| emitted C, an object per definition, linked and loaded once per run | a C compiler at run time | a process and a compile | a link and a load — `benches/c-floor/` | the C compiler's, at the level chosen | one code generator serving both tiers |
| emitted C, an image per definition, loaded by reach | a C compiler at run time | a process, a compile and a link | superlinear in the images loaded — **the row above refuses this shape** | the C compiler's | one code generator, and a loader cost that grows faster than the reach |
| a C compiler linked in process (libtcc is the instance) | a C library at run time | a compile, no process | unmeasured | unoptimised | one code generator; a second C toolchain to bind |
| copy-and-patch | clang at build time — the stencils need a calling convention that passes every register through, and CPython's JIT builds with a pinned LLVM for that reason | a copy and a relocation patch | none | about an unoptimising compiler's | a second code generator in Ply, with its own differential |

What would choose: the per-run and per-definition constants against the loop's
budget; the code quality against how long the suite runs, which the
marginal-change row separates only if test length is varied; and one code
generator against two, since the release tier is C either way and every code
generator in Ply is a differential to maintain.

**Both constants are now read, and they are two orders of magnitude apart.**
`benches/marginal-change/` reports what Cranelift charges per definition it
compiles, at three project sizes; `benches/c-floor/` reports what a C toolchain
charges for one changed definition, dominated by the process rather than by the
compiling. Cranelift is the cheaper by roughly sixty times, and neither reading
is an estimate. That does not settle the table on its own — an in-process tier
still has to be hostable without Rust, which is the whole of the line's goal —
but it does say that a C tier inside the loop buys its single code generator at
a price the loop can feel, and that the shape which pays it least is the one
`benches/c-floor/` already picked out: an object cached per definition, one link
over the reach, one image loaded.

The same row says the unit is compiled once per worker plus a pre-flight, so a
run pays that per-definition cost as many times as it has workers and one more.
Sharing one compiled unit across workers is the next bounded win under a
backend, and it is blocked on `Unit` holding an `Rc`; nothing has attempted it.

**LLVM in place of C, for the line rather than the loop — priced and rejected.**
Two forms, both losing. Linking LLVM trades a Rust dependency for a larger C++
one that is harder to build and to bind, which is the dependency this goal
exists to shrink. Emitting textual IR trades a stable interface for one that
moves with the release — the opaque-pointer transition broke the out-of-tree
text emitters that existed — and it still shells out to a binary, at which
point `cc` is the simpler shell-out. C is the only one of the three that makes
the dependency smaller than the one it replaces.

## The row, registered before it is taken

**Half built.** `ply-corpus bench` applies the edits and times the phases,
`ply-corpus sweep` takes it at each size, and `benches/run.sh` is a ladder of
six sizes. What is missing: a `--backend` option on `bench`, which calls
`ply_test::run` in-process and installs no backend, though
`ply_cli::commands::common::build_backend` is public and is the seam the
command itself uses; a fit across the sizes, since `sweep` prints one report
per size and nothing reads the slope; and the process start `bench` does not
pay, since it runs the phases in-process. The criteria, written before the arm
exists:

**Question.** What does one edit cost, and does it grow with the size of the
project — under the interpreter, and under the backend?

**Arms.** `bench`'s five scenarios, each under no backend and under
`--backend cranelift`: `warm` is the run's fixed cost, `cold` the O(project)
bound on the same project, `rename` the edit that moves no hash, `edit-leaf`
and `edit-hub` the two reaches.

**Sizes.** Three from the ladder in a ratio of four, so a slope is told from a
constant; two points fit any line.

**Statistic.** `bench`'s own — minimum total over repeats, phases beside it —
under ADR 0030's gate: load read before and after, the binary checked current,
no series spanning a rebuild.

**Control.** `cold` on the same project. The Rust tree is one program at one
size and cannot be fitted, so it is not in the row.

**Decision rule.** Under the interpreter, `rename` and `edit-leaf` move less
across the sizes than `warm` does, and `cold` moves in proportion to the size.
Under the backend the same test, and it is expected to fail — the code above
says the compile does not shrink with the edit — and the row's reading is
*which phase* carries the growth, because that is what orders the two items
left. Growth in compile means the per-definition cache; a fixed cost that
dominates it means the warm process. The front end and the tests re-run are no
longer candidates: item 1 took them out, and the row is what shows whether it
did.

That rule is CONTRIBUTING §"Measure an ADR's motivating claim before accepting
the ADR" applied to the claim this whole project rests on, which has been
load-bearing for every bootstrap decision in the tree and has been read as a
slope for neither tier.

## The order

1. **Done.** A result names the engine that earned it, the front-end cache is
   read whatever executes, and a backend that is wrong on purpose still gets no
   store. `one_engines_pass_is_never_another_engines` and
   `a_second_backed_run_selects_nothing` in `crates/ply-cli/tests/suite/cli.rs`
   hold the two halves, and `armed.rs` fails if a new route to a backend
   forgets either.
2. **Done, and it decided item 3.** `benches/marginal-change/` is the row:
   `ply-corpus bench --backend`, five edits at three sizes, fitted step by step.
   What it says is in its `observation-1.txt`, and the short form is below.
3. **A warm process**, which the row chose over the `DefHash -> code` cache: the
   cost that dominates a small edit is not the compile alone but the whole fixed
   cost of an invocation, and a warm process removes both while a code cache
   removes one. **Started, not finished.** `ply test --watch` is a process that
   stays: it holds the store, the checked front end and the compiled unit across
   iterations. A save that changed no byte — the common case, since a save is
   what wakes the loop — costs a read of the files whose timestamps moved and no
   front end at all, which `watch_reruns_on_a_save_and_keeps_the_front_end_it_already_had`
   asserts and was seen to fail. An iteration where something *did* move still pays the front
   end in full, because `crates/ply-cli/src/driver.rs` is one-shot: it works over
   the whole program whatever changed.

   **And the phase report says there is no single term to fix.** On a warm run
   that rechecks nothing, hashing is about a quarter, writing back a fifth, and
   parsing, restoring, resolving, checking, assembling the modules and reading
   the files divide the rest — every one of them proportional to the project.
   Assembling was in no phase at all until this record's branch gave it one: gate
   1 runs to a fixed point and copies every parsed module each round, so a cost
   proportional to the program was invisible in a report that accounted for
   everything else.

   That shape is the finding. A loop cannot be made O(change) by making hashing
   incremental, or by any other single lever, because no single lever is where
   the time is. What it needs is for the front end to be *held* and updated,
   which is what a resumable driver means, and that is the last O(project) term
   in a warm loop. **ADR 0038 is that record**: it fixes what correctness means
   for a resumable front end and what the row must show, before a branch exists
   to judge.
4. Choose the loop's tier from the table above. **The comparison it was waiting
   on is taken**: `benches/marginal-change/` reads Cranelift's per-definition
   cost and `benches/c-floor/` reads C's, and they are two orders of magnitude
   apart. What is still open is not a number but the line's goal — whether an
   in-process tier can be hosted without Rust — and that is a design question
   rather than a measurement. The re-take against a *per-definition* unit is
   conditional on item 3: a warm process holds compiled code in memory and never
   serialises it, so a per-definition unit is only needed if the warm process
   turns out not to be enough.

## What the row said

`benches/marginal-change/observation-1.txt`, taken above the load gate at the
end and therefore an observation. The counts in it are deterministic and carry
most of the argument.

- **A rename costs nothing, under either engine.** Its marginal cost never rises
  above the measurement's own resolution at any size. That is ADR 0021's claim
  holding where it is sharpest, and under the compiled backend it holds only
  because item 1 landed.
- **A leaf edit is flat under the interpreter** across a project sixteen times
  larger, and `cold` is proportional. The two arms that had to separate, did.
- **And the cost that dominates a small edit is neither of those. It is the
  invocation.** A warm `ply test` that rechecks *nothing* — the report says
  `rechecked 0` at every size — still pays a front end proportional to the
  project: over the step where the reading is above the noise floor, four times
  the project costs four times the time. It goes on hashing every definition to
  establish that none moved, restoring every interface, and writing them back.
- **Under the backend, compile is about half of that warm run** and does not
  shrink with the edit, because the unit closes over every function the fragment
  compiles and nothing holds it across runs.

So the ordering the row decides is: **the warm process, not the code cache.**
The code cache removes the compile and leaves the front end's fixed cost; the
warm process removes both, because what makes an invocation cost the project is
that it starts knowing nothing. This record's own falsifier — *if a warm process
removes the backend's cost by itself, persisting compiled code is never needed* —
is the live hypothesis rather than a hedge.

## What would make this wrong

- **If the row finds an edit under the backend flat in project size.** Then the
  code reading above is wrong somewhere, and the ordering should go back to
  what it found once the row says where.
- **If a warm process removes the backend's cost by itself.** Then persisting
  compiled code is never needed and the daemon is the whole of that work rather
  than half of it. This is the live hypothesis rather than a hedge, and what
  would settle it is the resumable driver: until an iteration stops rebuilding
  the whole program's tables, a warm process cannot be read as evidence about
  what holding compiled code is worth.
- **If the loop tier's code quality decides the loop rather than its latency.**
  A tier that compiles instantly and runs the tests several times slower loses
  to one that compiles slowly and runs them fast, once the tests run long
  enough; which regime the loop is in is a property of the suite, not of the
  compiler. The row above does not separate the two, and a row that varied test
  length would.
- **If the C floor is different where the loop runs than here.**
  `benches/c-floor/` is one machine, one loader and one toolchain, and the cost
  that decided the shape — many images being superlinear to load — is the
  loader's. Another platform is a re-take, not an inference, and a loader
  without that property would put the per-definition-image shape back on the
  table.
- ~~**If a cached pass under a backend cannot be given a meaning the
  evaluator's cache can share.**~~ **This fired.** It could not, and the
  outcome this named is the one taken: each engine keeps its own namespace, so
  O(change) holds per engine rather than across them, at the cost of a cold
  first run per engine.
