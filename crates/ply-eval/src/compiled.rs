//! Where a natively compiled body may be entered in place of evaluating one.
//!
//! The machine hands a backend a name, some scalars and a call budget, and takes
//! back at most one scalar. It hands over no arena, no stack, no handler stack,
//! no host binding, no `&mut Machine` and no way back into itself — so a backend
//! that cannot finish a call has changed nothing the program can observe. `None`
//! means "evaluate it yourself", and the machine does, from the top, with its own
//! diagnostics.
//!
//! That shape is what makes the invariants below hold by construction rather than
//! by a backend remembering them. Declining is the default for everything the
//! machine has not positively cleared:
//!
//! - **Effects, in two gates rather than one.** A backend cannot `perform`: it is
//!   handed no machine to perform into. [`admit`] additionally refuses any
//!   definition whose *published* row is non-empty ([`Gate::PublishedRow`], the
//!   same rule the constant memo reads, `memo::pure_by_published_row`) **and**
//!   any definition that can perform an atom its row does not show
//!   ([`Gate::InternalEffects`], `ply_core::DefInfo::internally_effectful`). A
//!   row answers "can an atom escape this call"; it cannot answer "can an atom
//!   be performed and discharged inside it", because discharging is precisely
//!   what takes an atom out of a row. A machine built without a `CheckOutput`
//!   reads neither fact and so enters nothing at all — which is most of this
//!   crate's own tests, and the reason the corpus tests assert an entry count
//!   rather than only a clean report.
//!
//!   > **Narrowed (R5 review, 2026-08-22): the row gate does not cover the whole
//!   > of "effects".** A definition that performs its operations and
//!   > **discharges them under its own `handle`** publishes an *empty* row —
//!   > `crates/ply-codegen-spike/tests/fixtures/hazards/effects.ply`'s
//!   > `handled` is declared `-> Int` with no row and type-checks, and both its
//!   > `footprint` and its `performed` come back empty — so this gate clears it
//!   > and the machine **offers it**. Entering it is then a real difference:
//!   > `ply-test` reports an `observed_footprint` (`report.rs`) and reads a
//!   > declared-but-unobserved atom as "a branch was not taken" (`slice.rs`),
//!   > and a native body performs nothing, so a user would be told a branch was
//!   > not taken when it was. It is **latent and not live** — the only backend
//!   > in the tree refuses `handle` at compile time (`jit.rs`) — but that is a
//!   > backend remembering the invariant, which is the property this module
//!   > claims to have engineered away. No published row can close it: `handled`
//!   > carries no fact distinguishing it from a genuinely pure definition.
//!   > `CONTRIBUTING.md` §"Things known to be broken" carries it as open.
//!
//!   > **Closed (2026-08-24), and the narrowing above was right about the row
//!   > and wrong about the remedy.** Its last two sentences are withdrawn:
//!   > *"No published row can close it: `handled` carries no fact
//!   > distinguishing it from a genuinely pure definition. `CONTRIBUTING.md`
//!   > §"Things known to be broken" carries it as open."* No published **row**
//!   > can close it and that half stands — a row is a set of atoms that escape,
//!   > and none escape. What closes it is a published fact that is not a row:
//!   > `ply_core::DefInfo::internally_effectful`, read here by
//!   > [`Gate::InternalEffects`].
//!   >
//!   > **The fact has to be transitive, and that was measured before it was
//!   > built.** The obvious form — a per-body bit for "written with `perform`
//!   > or `handle`" — closes `handled` and leaves the hole open one call away.
//!   > With `fn wrapper(x) = handled(x)`, inference publishes an empty
//!   > `footprint` *and* an empty `performed` for `wrapper`, it is written with
//!   > neither keyword, and running it records `state.read` in
//!   > [`crate::Trace`]. Every fact `wrapper` carries about its own text is a
//!   > fact a pure definition carries. `a_definition_that_only_calls_one_that_discharges_its_own_effects_is_refused_too`
//!   > and `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop`
//!   > are what hold that, the second at four hops, through a mutually
//!   > recursive pair and through a lambda.
//!   >
//!   > **One consequence the narrowing named does not follow, and is withdrawn
//!   > separately.** It read: *"`ply-test` reports an `observed_footprint`
//!   > (`report.rs`) and reads a declared-but-unobserved atom as "a branch was
//!   > not taken" (`slice.rs`), and a native body performs nothing, so a user
//!   > would be told a branch was not taken when it was."* Entering a body can
//!   > only lose atoms **discharged inside** it — an escaping atom puts the row
//!   > gate in the way one line earlier — and a discharged atom appears in no
//!   > declared row anywhere, so no *declared* atom can go missing this way.
//!   > What entering costs is an `observed_footprint` that under-reports a run,
//!   > which is a wrong answer to a user but not that one. Separately, and
//!   > measured rather than read:
//!   > `ply test <a failing fixture> --trace always --json` answers
//!   > `"causal_slice": null`, because `ply_test::SliceBuilder` is constructed
//!   > nowhere outside `ply-test/tests/bisect_audit.rs` — see
//!   > `CONTRIBUTING.md` §"Things known to be broken" item 15.
//! - **Continuations.** Nothing runs in the machine while a body runs, so no
//!   continuation can be captured beneath a native activation and no handler
//!   clause can resume into one.
//! - **Cells, regions, tasks.** No arena crosses, and neither does a
//!   `Value::Cell`, `Value::Task` or `Value::Continuation` — see [`crossable`],
//!   which carries only kinds that hold no `Value` inside them and so cannot
//!   reach one of these at any depth either.
//!
//!   > **Re-argued, not withdrawn (2026-08-31): the conclusion is the same and
//!   > the reason is no longer "childless".** The clause *"see [`crossable`],
//!   > which carries only kinds that hold no `Value` inside them and so cannot
//!   > reach one of these at any depth either"* is true of an **answer**, which
//!   > is what [`crossable`] still decides, and false of an **argument**: a
//!   > [`Value::Record`], [`Value::Ctor`], [`Value::List`] and [`Value::Map`]
//!   > all cross in now, and every one of them holds `Value`s.
//!   >
//!   > What keeps a handle out of them is [`CarriedTypes`]: an argument crosses
//!   > only when its definition's **declared parameter type** cannot
//!   > transitively reach a `Cell`, `Task`, `Secret` or function type — decided
//!   > once per program over the declared types rather than once per call over
//!   > the values — **or** when the value is an `i64`, a `bool` or an
//!   > `Arc<[u8]>`, where the old childless argument still applies unchanged.
//!   > Both are conjoined with the value's kind, which must be the one its
//!   > declared type denotes, so a definition declared `(Int) -> Int` handed a
//!   > list by `Machine::call` is refused on the kind rather than licensed by
//!   > the type.
//!   >
//!   > **Both ends now, and the second end is where this bullet stops being
//!   > structural (2026-08-31).** The block above says [`crossable`] "still
//!   > decides" the answer; it does not. `Machine::compiled_answer` reads
//!   > [`CarriedTypes::answer_crosses`], which is the same two clauses at the
//!   > other end: the declared **return** type is carried and the answer is of
//!   > the kind it denotes, **or** the answer is childless and [`crossable`]
//!   > carries it exactly as before.
//!   >
//!   > That is a genuine narrowing and it is the price of ADR 0030 §1's
//!   > collapse. An *argument* is a value the machine's own evaluation built
//!   > under a checker that accepted the program, so its interior follows its
//!   > declared type and the type test is a fact about the value. An *answer* is
//!   > built by the backend, so the same test is a fact about what the backend
//!   > was **supposed** to build. A backend that answers a `Record` of the right
//!   > kind holding a [`Value::Cell`] is believed here, and what reports it is
//!   > the independent engine — the class this module has always said it cannot
//!   > see, now one member wider. `backend::Mutation::Handle` is the ninth wrong
//!   > backend, added with this change so the limit has something standing on
//!   > it: over `examples/` and `tests/fixtures/` it changes **388** answers and
//!   > **237 of 1,127** tests report it, the first as `E0502` "`bytes_concat_all`
//!   > expects Bytes, but got Cell". 890 do not.
//!   >
//!   > What it bought, on ADR 0030's workload: `items.parse` is entered **once
//!   > per file**, entries go **306,931 -> 26**, and the share of body calls a
//!   > backend can answer goes **17.033% -> 84.014%** — from 24.1% of the
//!   > admitted set to all of it.
//!   >
//!   > This is the widening ADR 0030 §9.2 chose over a deep value walk and over
//!   > `Str`, and the reason it is a type test rather than a walk is measured
//!   > rather than argued: the walk **does not finish** on the ported Ply front
//!   > end (`crate::census`'s header). What it bought on that workload —
//!   > `spikes/ply-parser` parsing `examples/`, 13 files, 333,851 bytes:
//!   > [`Gate::ArgumentShape`]'s 100.00%-of-refusals monopoly ends and the seam
//!   > goes from admitting **294,656 of 2,414,170 body calls (12.205%)** to
//!   > **2,028,230 (84.014%)**, against ADR 0030 §6's counterfactual of
//!   > 82.855%. The residue is **98.5% [`Gate::Anonymous`]** — 380,176 lambdas
//!   > — and 5,764 `Closure` arguments, which is the whole of it: no other gate
//!   > refuses a single call on that workload.
//! - **Diagnostics.** A backend cannot raise. A body that would fail answers
//!   `None` and the machine raises its own diagnostic from its own evaluation, so
//!   the code, message, spans, labels and notes are the interpreter's by
//!   construction.
//! - **The deterministic scheduler.** The hook is off inside a `simulate` region,
//!   so every `Access` a search reads is still recorded by the interpreter.
//! - **Recursion, and the whole of the machine's one bound.** `budget` is the
//!   machine's remaining nested calls. A backend that would exceed it answers
//!   `None`, and the machine raises the same
//!   `recursion limit of 10000 nested calls exceeded` both engines answer with.
//!   Nested calls is all there is to express: a machine asked for a frame
//!   ceiling ([`crate::Machine::with_max_frames`]) enters nothing at all, so no
//!   backend is ever offered a call under a limit it was not handed.
//!
//!   > **Closed (2026-08-24). This bullet used to read "and only one of the
//!   > machine's two bounds", under a refutation an R5 review took with the real
//!   > backend, no mutation, one entry and zero declines:** *"The machine has a
//!   > **second** bound — `DEFAULT_MAX_FRAMES`, 1,000,000 pending frames,
//!   > enforced in `Machine::push` — and nothing in this signature can express
//!   > it. A compiled body pushes **one** `Frame::Call` for the whole call; the
//!   > interpreter pends one frame per pending operand as well … `hog(9000)`
//!   > gives `Err(recursion limit of 1000000 pending frames exceeded)` alone and
//!   > `Ok(1350000)` with a backend attached."* The refutation was right and the
//!   > signature was not the thing to change. A native body pends no frames, so
//!   > any frame cost charged to an entry is an estimate, and an estimate that
//!   > differs from the interpreter's exact count is itself the divergence — the
//!   > only conservative charge, `budget × the body's static pend`, declines
//!   > every recursive entry the seam exists for. So the second bound went
//!   > instead: it was a resource guard on this engine's heap that had been
//!   > phrased as a program answer, and it was sensitive to how a body's
//!   > additions were spelled rather than to what the body did.
//!   > `Machine::with_max_frames` carries the measurement.
//!
//! What is **not** structural, stated plainly: a backend that answers an `Int`
//! the definition would not have produced is a wrong answer this boundary cannot
//! detect. It is caught by `--engine both` and the differential corpus, which
//! compare the machine against an independent tree-walker, and by nothing here.
//!
//! That claim is now demonstrated rather than argued —
//! `crates/ply-codegen-spike/tests/mutations.rs` runs eight deliberately wrong
//! backends against the kernel corpus and names what caught each. **One of them
//! is still not caught by any corpus in this tree**: no corpus can exercise the
//! published-row gate, because `benches/kernel` declares no effect and the
//! differential corpus declines effectful names. Read that file's header before
//! trusting this one.
//!
//! > **Narrowed (2026-08-24).** This read "and two of them were **not** caught:
//! > a backend that ignores its budget entirely overflows the native stack
//! > before any comparison runs, and no corpus in this tree can exercise the
//! > published-row gate at all." The first half is closed: a `--mutate` run is
//! > started as a child, and a child that dies by a signal is reported as a
//! > disagreement rather than ending the run. The second half stands.
//!
//! > **Half closed, and the halves are different harnesses (2026-08-24).** The
//! > sentence above it — *"**One of them is still not caught by any corpus in
//! > this tree**: no corpus can exercise the published-row gate, because
//! > `benches/kernel` declares no effect and the differential corpus declines
//! > effectful names."* — and this block's *"The second half stands"* were
//! > written as one claim and are two.
//! >
//! > **A corpus in this tree now catches it.**
//! > `tests/fixtures/self_handled_effect.ply` declares an effect and discharges
//! > it, and `crates/ply-eval/tests/differential_corpus.rs` reads it. Measured
//! > by deleting [`Gate::InternalEffects`] and running
//! > `cargo test -p ply-eval --test differential_corpus`:
//! > `a_backend_that_answers_correctly_agrees_over_every_corpus_on_disk` and
//! > `a_definition_that_discharges_its_own_effects_is_in_the_corpus_and_is_never_entered`
//! > both fail with `observed footprint — left
//! > {self_handled_effect.tally.read[log], self_handled_effect.tally.write[log]},
//! > right {}` — the shape R5's mutation table reports for deleting the purity
//! > gate outright.
//! >
//! > **`mcts --mutate` still does not.** That harness runs over
//! > `benches/kernel`, `benches/kernel` still declares no effect
//! > (`grep -rn '^effect ' benches/kernel` is empty), and this fixture is not in
//! > it — a `.ply` under `tests/fixtures/` is read by the workspace suite and by
//! > nothing the spike runs. So the eight-mutant table in
//! > `crates/ply-codegen-spike/tests/mutations.rs` is **not** completed by this
//! > change; what changed is that `cargo test --workspace` catches the mutant
//! > the spike's own corpus cannot.
//!
//! Two more, stated because they are limits rather than guarantees. A backend's
//! panic is not caught, so a backend bug aborts the process rather than becoming
//! a silent slow path. And a run with a backend attached is a third execution
//! strategy whose results a result cache must not keep — a rule that is **not
//! enforced**, because no shipping command can install one; see
//! [`crate::Machine::set_compiled`].
//!
//! No implementation of [`Compiled`] exists in this workspace. The doubles in
//! this module's tests are what keep it exercised.
//!
//! > **Both of the last two paragraphs are withdrawn, 2026-08-28.** They read,
//! > verbatim: *"a rule that is **not enforced**, because no shipping command
//! > can install one"* and *"**No implementation of [`Compiled`] exists in this
//! > workspace.** The doubles in this module's tests are what keep it
//! > exercised."*
//! >
//! > One does now: [`crate::backend::Reference`], a backend whose compiled code
//! > is a second tree-walker over a scalar-signature fragment, installed by
//! > `ply test --backend`. It is not a code generator and ADR 0026 §4.7 promotes
//! > nothing — `Cargo.lock` still holds no cranelift — but it is a real
//! > implementor on the shipping side of this seam, and it is what makes the
//! > *accept* path reachable from a command a user runs: over `examples/` and
//! > `tests/fixtures/` it is offered 625,767 calls and enters **446,346** of
//! > them.
//! >
//! > > **Two of those clauses are withdrawn, 2026-08-31.** *"It is not a code
//! > > generator"* and *"`Cargo.lock` still holds no cranelift"* were true of
//! > > the only implementor there was. There are two now: `ply_codegen::Bodies`
//! > > is a **cranelift JIT**, in the shipping workspace, installed by
//! > > `ply test --backend cranelift`, and `Cargo.lock` holds 31 cranelift
//! > > packages. ADR 0026 §4.7 records the authorisation being exercised; ADR
//! > > 0016 §3.5's prohibition is amended there in the same change, and it still
//! > > binds `crates/ply-codegen-spike`, which remains outside the workspace and
//! > > is depended on by nothing.
//! > >
//! > > What that changes about **this file** is one sentence and it is worth
//! > > being exact: [`Compiled`] now has a second implementor whose `enter`
//! > > runs machine code, so every guarantee in the seven gates below is being
//! > > relied on by something that cannot be reasoned about by reading Ply. The
//! > > gates are unchanged and none of them was widened for it — the code
//! > > generator's own registry is **narrower** than [`crossable`], `Int | Bool`
//! > > against `Int | Bool | Bytes`, because it has no `Bytes` path.
//! >
//! > > **Re-taken after the `Bytes` widening (2026-08-30).** This read *"it is
//! > > offered 120,340 calls and enters **18,773** of them"*, and the figures
//! > > moved by more than the corpus did: measured on the same day, on the same
//! > > tree, with [`crossable`] narrowed back to `Int | Bool` and then widened,
//! > > `differential_corpus.rs`'s honest sweep goes 122,617 offered / 18,773
//! > > entered to **625,767 offered / 446,346 entered** — 23.8x the entries,
//! > > and the share of offers that are actually answered goes 15.3% -> 71.3%.
//! > > Through the shipping command, `ply test examples --engine both --backend
//! > > reference` goes from a fragment of 51 definitions entering **768** calls
//! > > to one of 153 entering **62,388**.
//! > >
//! > > **Entries rose, and that is the opposite of PR #30's result**, which is
//! > > worth stating rather than celebrating. PR #30 widened a fragment until
//! > > the interpreter stopped driving a search's loop, and its crossings
//! > > *fell* — 721 to 1 — because one entry swallowed the whole search. This
//! > > widening moves [`crossable`] and nothing else, so the fragment is still
//! > > not closed under calls and the interpreter still drives every loop: what
//! > > grows is the number of admissible **leaves**, one crossing each. Both are
//! > > coverage; only PR #30's shape is speed.
//! >
//! > And the rule is enforced, in the two stages ADR 0026 §4.6 specifies:
//! > `cache_bypassed` reads `--backend` so a backend run neither reads the
//! > store, and `ply_test::run_with` records `Record::Backend` so it writes
//! > nothing to it, with `backend_escapes` in `ply test` failing the run if a
//! > `Pass` is written by a test that entered native code anyway. Both halves
//! > were seen to fail before either was believed —
//! > `crates/ply-cli/tests/backend.rs` records which corruption produced which
//! > red.
//!
//! # What polices this seam, and what does not
//!
//! Counted rather than characterised, 2026-08-24, because
//! `CONTRIBUTING.md` §"Things known to be broken" item 13's first bullet is a
//! claim about coverage and a claim about coverage is checkable:
//!
//! | polices it | what it is |
//! | --- | --- |
//! | this module's tests | **38** tests over doubles built here: every gate in [`admit`] with a deletion recorded against it, the budget, the memo interaction, continuations, cells, regions, `Secret`, arity, and the two kinds of `Bytes` crossing |
//! | `crates/ply-eval/tests/differential_corpus.rs` | **6** tests, two hand-built backends over `examples/` and `tests/fixtures/` — an answering one and a tree-walking one |
//! | `crates/ply-codegen-spike/tests/mutations.rs` | **13** tests, eight deliberately wrong backends, each asserted to have *fired* before it is asserted to be caught |
//! | `crates/ply-codegen-spike/tests/hazards.rs`, `mcts_kernel.rs` | **25** tests over the real cranelift backend |
//! | `mcts --mutate <corruption>` | the same eight corruptions at corpus scale — 2,396 generated cases; run by hand, nothing runs it for you |
//!
//! > **Two rows added and one re-taken (2026-08-28), because the seam acquired a
//! > shipping implementor.** The `differential_corpus.rs` row read **6**; it is
//! > **14**, the eight new ones being the same eight corruptions over
//! > [`crate::backend::Reference`] at corpus scale, under `cargo test
//! > --workspace` rather than under a crate with its own toolchain. Re-counted
//! > from the run rather than by arithmetic: `cargo test -p ply-eval --test
//! > differential_corpus` reports 14 passed in 73.4s (82.7s on an earlier run;
//! > both at a load average of 8-9, so observations rather than figures), and
//! > prints what each corruption did.
//! >
//! > One cross-check fell out of it and is worth keeping. The old
//! > `backends::TreeWalker` double and the new shipping
//! > [`crate::backend::Reference`] are different code reaching the seam by
//! > different routes, and over the same corpus they enter it **18,773** times
//! > each. The test counts differ — 1,012 against 1,116 — because the old
//! > comparison is tree-walker against machine and counts a machine-only test as
//! > refused, while the new one is machine against machine-with-a-backend and
//! > has nothing to refuse.
//! >
//! > | polices it | what it is |
//! > | --- | --- |
//! > | `crates/ply-eval/tests/differential_corpus.rs` | **14** tests: the two hand-built backends, plus the eight corruptions over `Reference` on the same 1,116-test corpus |
//! > | `crates/ply-cli/tests/backend.rs` | **14** tests through `ply test --backend`, which is the shipping command. Seven of the eight configurations are caught; the eighth escapes and the file says which and why |
//!
//! > **Re-taken again the same day, for the ANSWER test (2026-08-31).** Three
//! > rows move and none is a new file. Re-counted from the runs rather than by
//! > arithmetic:
//! >
//! > | polices it | what it is |
//! > | --- | --- |
//! > | this module's tests | **51**, from 44. Seven came with the answer test, and the split is the point: three are the widening itself — a record answer crossing, an answer whose kind is not its declared return's, a closure-bearing return refused — and **four are the SUBTREE**, which is a different claim from any of the 44. An entered call used to be a leaf over scalars; `items.parse` is now entered once per file and hides 2.4 million calls, so the effects gate, the `simulate` gate, the budget and the offer count all had to be re-asked of a subtree rather than of a call |
//! > | `crates/ply-eval/tests/differential_corpus.rs` | **15**, from 14: `backend::Mutation::Handle`, the ninth wrong backend, which exists because the answer widening gave up a structural claim |
//! > | `crates/ply-cli/tests/backend.rs` | **15**, from 14, the same ninth through the shipping command. Its corpus changed too — `pair(Int) -> List<Int>` moved *inside* the fragment and `label(Int) -> String` replaced it as the definition [`Mutation::Unoffered`] bites on. Two tests failed rather than passing quietly when it moved |
//! >
//! > `cargo test -p ply-eval --lib` reports **553 passed / 0 failed / 1
//! > ignored**.
//!
//! > **The first row re-taken and one row added (2026-08-31), for the type
//! > gate.** The `this module's tests` row read **38**; it is **44** —
//! > [`Gate::ArgumentType`] brought six, one for each thing a type test has to
//! > decide that a discriminant test never had to: a closure-bearing record, a
//! > recursive type, a recursive type that reaches a closure, a type variable, a
//! > value whose kind is not its declared type's, and a world handle wearing a
//! > nominal type's shape. Re-counted from the run rather than by arithmetic:
//! > `cargo test -p ply-eval --lib compiled::` reports 44, and `--lib` alone
//! > reports **546 passed / 0 failed / 1 ignored**.
//! >
//! > | polices it | what it is |
//! > | --- | --- |
//! > | `crates/ply-eval/tests/seam_census.rs` | **1** test, and the only one that reads the type gate against a *corpus*: that it refuses something over `examples/` at all — 121,642 calls — and that the kind half refuses nothing the type half admits. Both were seen red, and by which corruption is recorded there |
//! >
//! > Measured sensitivity of the corpus-scale sweep, one run, 2026-08-28 —
//! > printed by the tests rather than asserted, because §4.7's condition names a
//! > measurement and a number that is asserted is a number nobody re-takes:
//! >
//! > | corruption | tests reporting it | answers changed |
//! > | --- | ---: | ---: |
//! > | `off-by-one` | 146 of 1,116 | 9,451 |
//! > | `inverted` | 51 | 216 |
//! > | `stale` | 259 | 501 |
//! > | `wrong-type` | 515 | 460 |
//! > | `unoffered` | 901 | 487 |
//! > | `answers=` | **0, and that is the gate** — `offered_target` is 0 | 0 |
//! > | `exceeds-budget=4` | **0 — nothing in this corpus outruns the machine's bound**, so the corruption has no decline to replace. Checked from `ply test` instead, on a corpus built to outrun it | 0 |
//!
//! > **Two rows re-taken (2026-08-24).** They read **32** and **5**. The effects
//! > gate added four tests here — three for the gate and one for the transitive
//! > closure behind it — and one to `differential_corpus.rs`. Re-counted from
//! > the runs rather than by arithmetic: `cargo test -p ply-eval --lib
//! > compiled::` reports 36 and `cargo test -p ply-eval --test
//! > differential_corpus` reports 6. The other three rows are untouched by this
//! > change and were not re-taken.
//!
//! And what does not, which is the part worth writing down:
//!
//! - ~~**`ply test --engine both` cannot install a backend at all.**~~ **Closed
//!   2026-08-28.** It read: *"`Compiled` and `set_compiled` occur **zero** times
//!   in `crates/ply-cli` — source and tests both. Every one of the **42**
//!   `set_compiled` call sites in the workspace is a test or the spike's own
//!   harness. So the shipping CLI catches **none** of the eight wrong backends,
//!   on any corpus, and `--engine both` compares the tree-walker against the
//!   machine and nothing else. A backend is reachable only from a test or from
//!   the spike's binaries."*
//!
//!   `ply test --backend <spec>` installs one, on any engine that has a compiled
//!   path — `--engine treewalk --backend ..` is refused with `E0450` rather than
//!   accepted and ignored. Under `--engine both` the backend is a **third**
//!   engine, compared against the plain machine rather than against the
//!   tree-walker, so a divergence reported is the backend's and nothing else's.
//!   `Machine::set_compiled` has exactly one production caller,
//!   `ply_test::InterpExecutor::machine_lowering`, and
//!   `crates/ply-span/tests/armed.rs`'s
//!   `a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache`
//!   fails the day a second one appears without the cache rule moving with it.
//!
//!   **The shipping CLI catches seven of the eight configurations, not eight,
//!   and the eighth is named rather than rounded up.** Ignoring the budget
//!   *entirely* over a **non-terminating** recursion is not a wrong answer: the
//!   run never comes back, and every candidate reporter is inside the process it
//!   took down. Measured — no output and no exit in 45 seconds, against 0.03s
//!   for the run that reports. `crates/ply-cli/tests/backend.rs`'s header has
//!   the table.
//!
//!   > **The count was wrong and is corrected in place (2026-08-28).** It read
//!   > *"Every one of the **five** `set_compiled` call sites in the workspace is
//!   > a test or the spike's own harness."* `grep -rn '\.set_compiled('
//!   > --include=*.rs` over the tree counts **42** across six files: this
//!   > module's own tests 27, `ply-codegen-spike/tests/hazards.rs` 5,
//!   > `ply-eval/tests/differential_corpus.rs` 3,
//!   > `ply-eval/tests/equivalence_audit.rs` 3,
//!   > `ply-codegen-spike/tests/mutations.rs` 2, and
//!   > `ply-codegen-spike/src/measure.rs` 2. `CONTRIBUTING.md`'s copy of this
//!   > bullet carries the same "five" over a parenthetical list that sums to 39
//!   > and omits `equivalence_audit.rs` altogether; it is corrected there too.
//!   > **The sentence the count sits inside is unaffected and was re-checked one
//!   > file at a time: all 42 are tests or the spike's harness.** It is
//!   > corrected anyway, because a number carried beside a true sentence is
//!   > still a number the next reader re-quotes — which is how it got here.
//! - ~~**Nothing enforces the result-cache rule.**~~ **Closed 2026-08-28.** It
//!   read: *"A run with a backend attached is a third execution strategy whose
//!   results a cache must not keep; the rule is unenforced *because it is
//!   unreachable* — see [`crate::Machine::set_compiled`]."* Both halves of ADR
//!   0026 §4.6 are armed and both were seen to fail: `cache_bypassed` reads
//!   `--backend` (delete the clause and a backend run believes 5 cached passes
//!   while entering **nothing**, which is the vacuous green this project
//!   produces most), and `ply_test::run_with` records `Record::Backend` (delete
//!   the arm and `ply test` reports three `E0505 … entered compiled code, and
//!   its pass was written to the result cache` and exits 1).
//!
//!   > **Corrected in place (2026-08-28): the gate this bullet named is
//!   > closed.** It read *"Wiring a backend into the CLI is what would make it
//!   > reachable, and it is gated on the entry-point defect (`CONTRIBUTING.md`
//!   > item 9) rather than on this seam."* Item 9 — the seam carrying one of the
//!   > machine's two resource bounds — is marked **"Fixed 2026-08-24, together
//!   > with item 10 — they were one defect"**, and the fix is recorded in this
//!   > module's own budget bullet and in `Machine::with_max_frames`. The only
//!   > gate this block named that is still open is the result-cache rule, and
//!   > `docs/adr/0026-a-reachable-backend.md` §4.6 decides how
//!   > it is armed: a source-level tripwire that fails the day `set_compiled`
//!   > acquires a production caller without `cache_bypassed` growing a way to
//!   > see it, and a `backend_escapes` diagnostic beside `cache_escapes` that
//!   > M9 owes. ADR 0026 §4.1 answers the question this bullet was waiting on:
//!   > a backend is reachable, and no backend ships before a shipping command
//!   > can police one.
//! - ~~**No corpus in the tree can exercise the published-row gate.**~~
//!   **Closed (2026-08-24).** It read: *"`benches/kernel` declares no effect, so
//!   every definition in it publishes an empty row and the gate has nothing to
//!   refuse; `ply-eval`'s differential corpus declines effectful names before
//!   the gate is reached. Closing it means a corpus that declares an effect,
//!   which does not exist yet."* It does now:
//!   `tests/fixtures/self_handled_effect.ply` declares `effect tally`, performs
//!   both its operations and discharges both under its own `handle`, so its
//!   `handled` and `wrapper` are refused by [`Gate::InternalEffects`] on the
//!   corpus path rather than in a unit test, and its `measured` publishes a row
//!   that is not empty, which is what [`Gate::PublishedRow`] reads. The row is
//!   what is asserted, not the gate: a corpus run counts declines and does not
//!   record which gate produced one. `benches/kernel` still declares no effect —
//!   checked, `grep -rn '^effect ' benches/kernel` is empty — and that half is
//!   unchanged, which is why the fixture lives in `tests/fixtures/` instead.
//! - **A wrong `Int` is not caught here by anything.** It is caught by
//!   `--engine both` and the differential corpus comparing against an
//!   independent tree-walker, which is the sentence above this section.

use crate::value::{Closure, ClosureKind, Value};
use ply_core::CheckOutput;
use ply_core::ty::{SECRET, TyVar, Type};
use ply_span::Symbol;
use ply_syntax::ast::Program;
use rustc_hash::FxHashMap;

/// A source of natively compiled bodies for a program's definitions.
pub trait Compiled {
    /// Whether these bodies were compiled from `program`. Pointer identity, as
    /// [`crate::code::Lowering::describes`] is and for the same reason: a
    /// bisection builds programs whose definitions carry the names of the ones
    /// they replace (`crates/ply-eval/tests/hoist_staleness_audit.rs`).
    fn describes(&self, program: &Program) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    ///
    /// The machine checks both sides and evaluates the definition itself when
    /// either fails, so an unsound backend produces a slow program rather than a
    /// wrong one. What each side is checked against is [`crossable_argument_kind`]
    /// plus [`CarriedTypes`] on the way in and [`CarriedTypes::answer_crosses`]
    /// on the way out — in both directions, the definition's declared type and
    /// the value's kind.
    ///
    /// > **Corrected in place (2026-08-31), and it was stale on the argument
    /// > side before this change as well.** It read: *"`args` are
    /// > [`Value::Int`], [`Value::Bool`] and [`Value::Bytes`] only and the
    /// > answer must be too; the machine checks both sides and evaluates the
    /// > definition itself when either fails, so an unsound backend produces a
    /// > slow program rather than a wrong one."* Arguments stopped being those
    /// > three when [`Gate::ArgumentType`] shipped and the answer stopped being
    /// > those three when [`CarriedTypes::answer_crosses`] did. This is the
    /// > trait's own contract — the one paragraph a backend author reads before
    /// > writing anything — so it is now spelled by reference to the two tests
    /// > rather than by a list that goes stale with them.
    ///
    /// **The obligation the second widening added, stated here because this is
    /// where a backend author is.** A container answer is checked for its
    /// top-level kind and not for its contents. A backend answering a `Record`,
    /// `List`, `Map` or `Ctor` must put in it only what a value of the
    /// definition's declared return type could hold: no [`Value::Cell`],
    /// [`Value::Task`], [`Value::Continuation`], [`Value::Closure`] or
    /// [`Value::Secret`], at any depth. The machine cannot see a violation —
    /// walking the answer is O(value) per entry and does not finish on a real
    /// front end, which is the measurement `crate::census`'s header carries —
    /// and `--engine both` is what catches one.
    ///
    /// A `Bytes` is `Arc<[u8]>` and is borrowed for the length of the call —
    /// read-only it is a `(ptr, len)` pair and costs a backend nothing. A
    /// backend that wants to *keep* one clones an `Arc` and pays the refcount;
    /// a backend that wants to *produce* one needs an allocator, and one that
    /// has none simply declines, which is what a registry miss already is.
    ///
    /// `budget` is at least 1 and is the nested calls left before the machine's
    /// `max_calls`. A body that would recurse past it answers `None`; the machine
    /// then re-evaluates and raises its own `recursion limit of 10000 nested
    /// calls exceeded`, which is the guarantee `limit.rs` exists to keep in both
    /// engines.
    ///
    /// It is also the *only* bound to honour, which is what makes this one
    /// `usize` sufficient rather than merely convenient. How many frames the
    /// interpreter would have pended running this body is not a fact about the
    /// program and no answer may turn on it; a machine that was nonetheless
    /// asked for a frame ceiling declines to enter anything.
    ///
    /// The machine has committed nothing when this is called and commits nothing
    /// on `None`, so declining is free after no work or after a whole body. That
    /// holds only while this signature hands over no route back into the machine
    /// — see `Machine::compiled_answer`.
    ///
    /// A panic here is not caught. `Machine` is not `UnwindSafe` and swallowing a
    /// backend's panic would turn a loud backend bug into a silent slow path.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;
}

/// What may cross this boundary, in either direction: the two unboxed scalars
/// and [`Value::Bytes`].
///
/// The list is short on purpose, and every exclusion closes a hazard rather than
/// being conservative for its own sake.
///
/// - No [`Value::Float`]: the codegen spike has no `Float` path and lowers `+` as
///   `Int` arithmetic whatever the operands are, which ADR 0019 §5 item 4
///   records. Behind this boundary that is a decline; without it, it is a working
///   program that starts raising at a call site nobody opted into.
/// - No [`Value::Str`] and no [`Value::Decimal`]: the same lowering compares them
///   as `Int`s.
/// - No [`Value::Secret`]: a credential cannot reach a constant pool or a
///   `format!` in code the machine did not write.
/// - Nothing that can reach a [`Value::Cell`], [`Value::Task`],
///   [`Value::Continuation`] or [`Value::Closure`] **at any depth**, so no handle
///   into this run's world crosses.
///
/// This is a capability cut as much as a safety one: nothing taking or returning
/// a `List`, `Map`, `Record`, `Str` or `Float` can be entered at all.
///
/// # Why a *shallow* test, and what it costs to widen it again
///
/// This decides from the top-level discriminant alone, and for the three kinds
/// it carries that is not an approximation: an `i64`, a `bool` and an
/// `Arc<[u8]>` hold no [`Value`] at all, so "this argument cannot reach a
/// closure, a cell, a task, a continuation or a secret" follows from the
/// discriminant and needs no walk. That is the whole of the licence
/// [`internally_effectful`] takes when it argues its published fact is
/// sufficient.
///
/// It does not extend. A [`Value::List`], [`Value::Map`], [`Value::Record`] or
/// [`Value::Ctor`] holds `Value`s, so admitting one on its discriminant lets a
/// [`Value::Closure`] across one field deep and the effects gate acquires a hole
/// exactly that deep. Widening to a container therefore means a **deep** walk —
/// O(value) on every call, including the ones that go on to decline — or a test
/// over the definition's declared parameter types instead of over the values,
/// which would put the [`CheckOutput`] lookup ahead of the shape gate and
/// reverse the cost claim [`admit`] documents. Neither is done here.
/// `a_container_is_refused_on_its_discriminant_whatever_it_holds` is the
/// tripwire: add a container kind to this `matches!` and it goes red.
///
/// > **This function is now the ANSWER test only, and the paragraph above is
/// > withdrawn for the argument direction (2026-08-31).** It read, verbatim:
/// > *"Neither is done here.
/// > `a_container_is_refused_on_its_discriminant_whatever_it_holds` is the
/// > tripwire: add a container kind to this `matches!` and it goes red."*
/// >
/// > The second of the two is now done, for arguments:
/// > [`crossable_argument_kind`] plus [`CarriedTypes`], reached through
/// > [`Gate::ArgumentType`]. The tripwire named above is withdrawn with the
/// > rule it guarded and replaced by
/// > `a_closure_bearing_record_is_refused_on_its_declared_type`, which fires on
/// > the hazard rather than on the discriminant — the record crossing is the
/// > point of the change, and what must not cross is a record whose *declared
/// > type* can hold code.
/// >
/// > `crossable` itself is **unchanged**, and deliberately: it is what
/// > `Machine::compiled_answer` tests the answer with, and an answer has no
/// > declared type the machine has checked the backend against. Widening it is
/// > the *return* half of ADR 0030 §9.2 and is not taken here — the ADR's own
/// > finding is that the return type is where the collapse is (`lex(Bytes) ->
/// > Scan`, one accepted call per file, refused because `Scan` is a record), and
/// > the reason it is not taken with this change is stated where the cost is:
/// > a value-level answer test over containers is the deep walk again, on the
/// > returned value, and the parser returns its whole state record from every
/// > one of ~200,000 calls. What would replace it is a *type*-level answer test,
/// > which moves a machine-side check into a backend obligation. That is a
/// > different decision from this one and is left to be taken on its own
/// > evidence.
///
/// > **The decision above was taken the next day, and one sentence of it is
/// > wrong rather than superseded (2026-08-31).** The block above is left whole
/// > because it is the argument this change had to answer. What is **withdrawn**
/// > is its first clause — *"`crossable` itself is **unchanged**, and
/// > deliberately: it is what `Machine::compiled_answer` tests the answer
/// > with"* — and this one sentence, which is false as written:
/// >
/// > > *"an answer has no declared type the machine has checked the backend
/// > > against"*
/// >
/// > It has one: the definition's declared **return** type, published in the
/// > same [`CheckOutput`] the parameter types come from. What it does not have
/// > is a *value the machine built*, which is the real asymmetry and is not the
/// > one that sentence names. [`CarriedTypes::answer_crosses`] is the type-level
/// > answer test the last sentence asks for, taken on the evidence the ADR
/// > recorded — 79.7% of admitted calls offered and declined on the return, and
/// > `lexer.lex` declined thirteen times out of thirteen.
/// >
/// > `crossable` **is** still unchanged, and is now the *childless* clause of
/// > both tests rather than the whole of either: it is what makes each widening
/// > a strict superset of the rule before it, which is why an `Int` answer is
/// > still believed for a definition declared `-> Scan` and why two of the eight
/// > wrong backends still have something to fire on.
///
/// Which of the two, measured rather than left as a choice: the deep walk does
/// **not finish** on the ported Ply front end — the state record it would walk
/// per call transitively holds the token list — and bounded at 256 nodes it
/// reaches 18.5% of calls against the type test's 82.6%, with the whole gap
/// attributable to the budget rather than to anything a walk would have found.
/// `crate::census`'s module header carries the table and the run. The ordering
/// this doc calls a reversal is the price of the design that works, and it is a
/// per-definition precompute rather than a per-call lookup.
///
/// > **Widened to carry [`Value::Bytes`] (2026-08-30).** The first line read
/// > *"What may cross this boundary, in either direction: the unboxed scalar
/// > kinds and nothing else"*, and the last bullet read, verbatim:
/// >
/// > > *"Nothing that can reach a [`Value::Cell`], [`Value::Task`],
/// > > [`Value::Continuation`] or [`Value::Closure`], so no handle into this
/// > > run's world crosses and **no heap value is cloned across** — which is
/// > > also why the unique-ownership path `frame.rs` sets up has nothing to
/// > > lose here."*
/// >
/// > Both halves of that trailing clause are withdrawn and neither was load
/// > bearing. A `Bytes` **is** a heap value — `Arc<[u8]>`, `value.rs`, "Slicing
/// > copies" — so a backend that keeps one past the call pays a refcount, which
/// > is a cost and not a hazard. And there is no unique-ownership path in
/// > `frame.rs` to lose: the in-place branches are `Arc::get_mut` in
/// > [`crate::builtins`]'s `push` and in `value.rs`'s dismantler, and they run
/// > over `List`, `Record` and `Secret`. **None of them runs over `Bytes`**, so
/// > a `Bytes` crossing cannot push any of them onto its copying branch.
/// >
/// > Why this kind and not the others, stated so it can be argued with. ADR 0026
/// > §3 records the seam refusing `fn read_line(buf: Bytes, ..) -> Line` on its
/// > first line, and a census over the ported Ply front end
/// > (`spikes/ply-parser`) puts **353,248** `Bytes` arguments in the way of
/// > calls that clear every other gate — a lexer's arguments are `Bytes` and
/// > nothing else about the workload changes that. `Str` is the same shape and
/// > is **not** admitted, because it is one of the three kinds ADR 0019 §5
/// > item 4's defect is about and the census scores it at +0.0 pp on that same
/// > workload: the hazard is real and the return is zero.
pub(crate) fn crossable(value: &Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_) | Value::Bytes(_))
}

/// The `Value` kinds a *carried type* can denote, which is what an argument's
/// discriminant is tested against.
///
/// This is the cheap half of the argument gate and it runs first, so a call
/// carrying a `Str`, a `Float`, a `Decimal`, a `Secret`, a `Closure`, a `Cell`,
/// a `Task` or a `Continuation` is still refused on one discriminant test per
/// argument and never hashes a [`Symbol`] into [`CheckOutput::defs`]. What it
/// is **not** is a soundness argument on its own: a `Record` clears it and a
/// `Record` holds `Value`s. [`CarriedTypes`] is what decides whether this
/// definition's records may hold code, and the two are conjuncts.
///
/// On a program the checker accepted this test refuses nothing
/// [`CarriedTypes`] admits — a value whose type is `Int` is a [`Value::Int`] —
/// so it is defence in depth rather than a filter, and it is measured as such:
/// `census`'s `type_gated_shipping` counts the calls the type gate alone would
/// admit and a corpus run asserts it equals `admitted`. It bites on a call the
/// machine reaches without a type, which is what `Machine::call` is
/// (`a_secret_is_never_offered_and_never_accepted` reaches the hook through it),
/// and on a hand-built `Closure` wearing a name whose definition has another
/// signature.
pub(crate) fn crossable_argument_kind(value: &Value) -> bool {
    matches!(
        value,
        Value::Int(_)
            | Value::Bool(_)
            | Value::Bytes(_)
            | Value::List(_)
            | Value::Map(_)
            | Value::Record(_)
            | Value::Ctor { .. }
    )
}

/// Which definitions' **declared parameter types** cannot reach a world handle,
/// decided once per program rather than once per call.
///
/// # Why a type and not a value
///
/// [`crossable`] answers "can *this value* reach a [`Value::Cell`],
/// [`Value::Task`], [`Value::Continuation`] or [`Value::Closure`]" by refusing
/// every kind that holds a `Value` at all. Widening it to containers on the
/// discriminant is unsound one field deep, and the two sound alternatives were
/// measured against each other before either was built
/// (`crate::census`'s header): a **deep value walk** does not finish on the
/// ported Ply front end — the state record it walks per call transitively holds
/// the token list — and bounded at 256 nodes reaches 18.5% of calls, against
/// this test's 82.6%, with the whole gap attributable to the budget. So the
/// question is asked of the *type*, where it is a property of a published
/// scheme and is therefore computable once.
///
/// # The rule
///
/// A type is **carried** when no value of it can transitively hold code or a
/// handle into this run's world:
///
/// - `Int`, `Bool`, `Bytes` — carried, and childless. This is [`crossable`]'s
///   list exactly: the widening is through containers and adds no leaf kind.
/// - `List<t>`, `Map<k, v>`, and a record type `{ f: t, .. }` — carried exactly
///   when every element, key and field type is.
/// - A declared sum type `T<a, ..>` — carried when **the declaration** is
///   carried (below) and every type argument at *this occurrence* is.
/// - `Float`, `Decimal`, `String`, `Unit` — **refused**, and not because they reach a
///   handle. They are ADR 0019 §5 item 4's three kinds, which the codegen spike
///   lowers as `Int`; the leaf set here is exactly [`crossable`]'s, so this
///   change widens the fragment through containers and moves no hazard.
///   `crate::census`'s ladder prices admitting them and the ADR records that
///   `Str` buys +0.0 pp on a front end.
/// - `Cell<r, t>`, `Task<t>`, `Secret<t>` — refused by name. These are the ones
///   a nominal fallback gets wrong: they are `Type::Con`s like any other, and a
///   pass that admitted "any nominal type" would admit a cell.
/// - A function type — refused. This is the closure case, and it is the whole
///   reason the record widening needs a type test rather than a discriminant.
/// - A type **variable** — refused. See "Generics" below.
/// - Anything else — refused, because a head this pass does not recognise is a
///   head it cannot argue about.
///
/// A **declaration** `type T<p, ..> = C1(f, ..) | C2(..)` is carried when every
/// field type of every constructor is carried, with an occurrence of one of
/// `T`'s own parameters counting as carried. That is sound because every
/// occurrence of `T` checks its arguments: substituting a carried argument for a
/// parameter that stood for "carried" leaves the field carried, and a
/// declaration that puts a *function type* under its own parameter — `type
/// T<a> = N(T<(Int) -> Int>)` — is refused when the occurrence inside it checks
/// its argument. So this needs no substitution and no instantiation, which is
/// what makes it a per-program table rather than a per-call one.
///
/// # Recursive types terminate, and by construction
///
/// A record alias cannot be recursive at all — `infer.rs` expands aliases and
/// answers `type alias `X` expands into itself` for a cycle — so every
/// [`Type::Record`] this pass sees is a finite tree. A **sum** type can be
/// recursive, and `type Tree<a> = Leaf | Node(a, Tree<a>)` is the ordinary
/// case rather than a corner.
///
/// The declarations are therefore solved as a **fixpoint over names** rather
/// than by walking a type into itself: every declared type starts carried, and
/// a pass that finds an uncarried field lowers it, repeated until nothing
/// moves. Lowering only ever removes, so it terminates in at most one round per
/// declaration. `carries` itself then never recurses into a declaration — it
/// reads `decls` — so the only recursion it does is over a use-site type
/// expression, which is finite. `a_recursive_type_is_decided_rather_than_walked_into_itself`
/// and `a_recursive_type_that_reaches_a_closure_is_refused` are the two sides.
///
/// # Generics: refused, not resolved at the call site
///
/// A parameter declared `List<a>` can hold a closure at some call site however
/// first-order the value in front of it happens to be, so the type alone cannot
/// clear it. The alternative is to resolve `a` from the *values* — which is the
/// deep walk this design exists to avoid, at the same cost and on the same
/// arguments. What refusing costs is measured rather than assumed: on
/// `ply test examples` the type gate reaches 84.1% against a shallow kind test's
/// 91.8%, and `Type::Var` is the whole of that gap; on the ported front end the
/// two rungs are equal to three digits, because a front end's parameters are
/// declared at concrete first-order types.
///
/// # Cost, and where the gate sits
///
/// One [`FxHashMap`] lookup per call that gets past the kind test, against the
/// zero the discriminant test cost — the ordering [`admit`] documented is
/// genuinely reversed for a container argument, and
/// `the_shape_gate_is_reached_before_the_row_is_looked_up` records what
/// survives of the old claim. The walk over declared types happens once per
/// [`CheckOutput`], behind a `OnceCell` on the machine, so a run that never
/// offers a call never pays for it.
///
/// [`Gate::ArgumentType`] is deliberately **last** rather than first, which
/// costs two lookups on the calls it refuses and is worth them. Put above the
/// row gate it refuses an unpublished name — `self.params` has no entry — and
/// so *masks* [`Gate::PublishedRow`]'s refusal of one, which is the defect this
/// module's test header spends four paragraphs on: a new gate above an old one
/// makes the old one's deletion invisible, and
/// `a_row_that_is_not_empty_and_a_row_that_is_missing_are_both_refused_by_the_row_gate`
/// and `an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate`
/// both went red on `Err(ArgumentType)` when it was tried there. Ordering by
/// cost would have bought two hash lookups on 17% of calls and paid for them in
/// a tripwire nobody would have noticed going quiet.
pub(crate) struct CarriedTypes {
    /// A declared sum type's own parameters and the field types of every one of
    /// its constructors, by program-wide type name. Record types are absent:
    /// they are aliases and are expanded before a [`Type`] exists.
    decls: FxHashMap<Symbol, Decl>,
    /// The fixpoint over [`CarriedTypes::decls`]: whether a value of that type
    /// can reach a world handle, its type arguments left to each occurrence.
    safe: FxHashMap<Symbol, bool>,
    /// Per definition, its declared signature read as [`Denotes`]. Absent means
    /// the name publishes no function type at all.
    sigs: FxHashMap<Symbol, Sig>,
}

/// One definition's declared signature, with every position answered once.
///
/// Both ends are here rather than only the parameters, and they are read by two
/// different tests at two different moments: `params` by [`Gate::ArgumentType`]
/// before a backend is called, `ret` by `Machine::compiled_answer` after it has
/// answered. Keeping them in one entry is what makes the two ends the same
/// question asked twice rather than two rules that can drift — which they did
/// between 2026-08-30 and 2026-08-31, when the parameter half moved to types and
/// the return half stayed on values.
struct Sig {
    /// One entry per declared parameter: the `Value` kind that parameter's type
    /// denotes when it is carried, and `None` when it is not.
    params: Vec<Option<Denotes>>,
    /// The same for the declared return type.
    ret: Option<Denotes>,
}

struct Decl {
    vars: Vec<TyVar>,
    fields: Vec<Type>,
}

/// The one `Value` kind a carried type denotes.
///
/// A carried type has exactly one, which is what lets the argument test be a
/// discriminant comparison rather than a walk: `Int` is a [`Value::Int`], a
/// record type is a [`Value::Record`], a declared sum type is a
/// [`Value::Ctor`]. Comparing it is what keeps an ill-typed value out of a
/// backend on a route that carries no types — `Machine::call`, which
/// `a_secret_is_never_offered_and_never_accepted` reaches the hook through, and
/// a hand-built [`Closure`] wearing a published name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Denotes {
    Int,
    Bool,
    Bytes,
    List,
    Map,
    Record,
    Ctor,
}

impl Denotes {
    fn matches(self, value: &Value) -> bool {
        match self {
            Denotes::Int => matches!(value, Value::Int(_)),
            Denotes::Bool => matches!(value, Value::Bool(_)),
            Denotes::Bytes => matches!(value, Value::Bytes(_)),
            Denotes::List => matches!(value, Value::List(_)),
            Denotes::Map => matches!(value, Value::Map(_)),
            Denotes::Record => matches!(value, Value::Record(_)),
            Denotes::Ctor => matches!(value, Value::Ctor { .. }),
        }
    }
}

impl CarriedTypes {
    /// The table for `check`, or an empty one — which admits nothing — for a
    /// machine built without a `CheckOutput`, for the reason
    /// [`Gate::PublishedRow`] refuses one: a machine that cannot read the fact
    /// has not been told it holds.
    pub(crate) fn over(check: Option<&CheckOutput>) -> CarriedTypes {
        let mut table = CarriedTypes {
            decls: FxHashMap::default(),
            safe: FxHashMap::default(),
            sigs: FxHashMap::default(),
        };
        let Some(check) = check else { return table };
        for ctor in check.ctors.values() {
            let decl = table
                .decls
                .entry(ctor.type_name.clone())
                .or_insert_with(|| Decl {
                    vars: ctor.scheme.ty_vars.clone(),
                    fields: Vec::new(),
                });
            decl.fields.extend(ctor.fields.iter().cloned());
        }
        table.safe = table.decls.keys().map(|n| (n.clone(), true)).collect();
        // Lowering only ever removes, so this settles; the bound is one round
        // per declaration and the loop asserts nothing about how many it took.
        loop {
            let lowered: Vec<Symbol> = table
                .decls
                .iter()
                .filter(|(name, decl)| {
                    table.safe[*name]
                        && !decl
                            .fields
                            .iter()
                            .all(|f| table.carries(f, Some(&decl.vars)))
                })
                .map(|(name, _)| name.clone())
                .collect();
            if lowered.is_empty() {
                break;
            }
            for name in lowered {
                table.safe.insert(name, false);
            }
        }
        let flags: Vec<(Symbol, Sig)> = check
            .defs
            .iter()
            .filter_map(|(name, def)| match &def.scheme.ty {
                Type::Fn { params, ret, .. } => Some((
                    name.clone(),
                    Sig {
                        params: params.iter().map(|t| table.denotes(t)).collect(),
                        ret: table.denotes(ret),
                    },
                )),
                _ => None,
            })
            .collect();
        table.sigs.extend(flags);
        table
    }

    /// The `Value` kind `ty` denotes, when `ty` is carried.
    fn denotes(&self, ty: &Type) -> Option<Denotes> {
        if !self.carries(ty, None) {
            return None;
        }
        match ty {
            Type::Record(_) => Some(Denotes::Record),
            Type::Con(name, _) => Some(match name.as_str() {
                "Int" => Denotes::Int,
                "Bool" => Denotes::Bool,
                "Bytes" => Denotes::Bytes,
                "List" => Denotes::List,
                "Map" => Denotes::Map,
                // `carries` cleared it and it is none of the builtin heads, so
                // it is a declared sum type and its values are constructors.
                _ => Denotes::Ctor,
            }),
            // `carries` refuses both of these, so this is unreachable rather
            // than conservative — it is spelled out so that a future kind added
            // to `carries` without an entry here is refused rather than
            // silently denoting whatever the arm above it did.
            Type::Var(_) | Type::Fn { .. } => None,
        }
    }

    /// Whether `ty` is carried. `decl_vars` is `Some` only while walking a
    /// declaration's own field types, where an occurrence of one of that
    /// declaration's parameters stands for "whatever this type is instantiated
    /// at", and every occurrence checks that instantiation itself.
    pub(crate) fn carries(&self, ty: &Type, decl_vars: Option<&[TyVar]>) -> bool {
        match ty {
            Type::Var(v) => decl_vars.is_some_and(|vars| vars.contains(v)),
            Type::Fn { .. } => false,
            Type::Record(fields) => fields.values().all(|t| self.carries(t, decl_vars)),
            Type::Con(name, args) => match name.as_str() {
                "Int" | "Bool" | "Bytes" => args.is_empty(),
                "List" | "Map" => args.iter().all(|t| self.carries(t, decl_vars)),
                // ADR 0019 §5 item 4's three, and the leaf set is deliberately
                // `crossable`'s exactly rather than one kind wider — `Unit`
                // included, which holds nothing and is refused anyway so that
                // the leaf set is the same list in both directions.
                "Float" | "Decimal" | "String" | "Unit" => false,
                // A world handle and a credential are `Type::Con`s like any
                // other. Named here because the fallback below would otherwise
                // read them as ordinary nominal types.
                "Cell" | ply_core::prelude::TASK_TYPE | SECRET => false,
                _ => match self.decls.get(name) {
                    Some(decl) => {
                        decl.vars.len() == args.len()
                            && self.safe.get(name).copied().unwrap_or(false)
                            && args.iter().all(|t| self.carries(t, decl_vars))
                    }
                    None => false,
                },
            },
        }
    }

    /// One entry per declared parameter of `name`. `None` is what a name
    /// publishing no function type gets, and an unknown name with it.
    fn params(&self, name: &Symbol) -> Option<&[Option<Denotes>]> {
        self.sigs.get(name).map(|sig| sig.params.as_slice())
    }

    /// Why [`Gate::ArgumentType`] refused this call: the head of the first thing
    /// in the first offending parameter's declared type that is not carried, or
    /// the value's kind when the parameter *is* carried and the value is not of
    /// the kind it denotes.
    ///
    /// Measurement scaffolding, called only from `crate::census`. "A gate
    /// refuses 67% of a corpus" is a number; *what* it refuses is the fact a
    /// roadmap is read off, and on `ply test examples` the two answers are
    /// different roadmaps — a leaf set one kind wider, or resolving generics at
    /// the call site, are opposite pieces of work.
    pub(crate) fn refusal(
        &self,
        check: Option<&CheckOutput>,
        name: &Symbol,
        args: &[Value],
    ) -> &'static str {
        let Some(flags) = self.params(name) else {
            return "<no signature>";
        };
        if flags.len() != args.len() {
            return "<arity>";
        }
        let declared = check
            .and_then(|c| c.defs.get(name))
            .map(|d| &d.scheme.ty)
            .and_then(|ty| match ty {
                Type::Fn { params, .. } => Some(params.as_slice()),
                _ => None,
            });
        for (i, (denotes, value)) in flags.iter().zip(args).enumerate() {
            if denotes.is_some_and(|d| d.matches(value)) || crossable(value) {
                continue;
            }
            if denotes.is_some() {
                return "<kind mismatch>";
            }
            return declared
                .and_then(|ps| ps.get(i))
                .and_then(|ty| self.blocker(ty, None))
                .unwrap_or("<unknown>");
        }
        "<none>"
    }

    /// The head of the first part of `ty` that is not carried.
    fn blocker(&self, ty: &Type, decl_vars: Option<&[TyVar]>) -> Option<&'static str> {
        match ty {
            Type::Var(v) if decl_vars.is_some_and(|vars| vars.contains(v)) => None,
            Type::Var(_) => Some("Var"),
            Type::Fn { .. } => Some("Fn"),
            Type::Record(fields) => fields.values().find_map(|t| self.blocker(t, decl_vars)),
            Type::Con(name, args) => match name.as_str() {
                "Int" | "Bool" | "Bytes" => None,
                "List" | "Map" => args.iter().find_map(|t| self.blocker(t, decl_vars)),
                "Float" => Some("Float"),
                "Decimal" => Some("Decimal"),
                "String" => Some("String"),
                "Unit" => Some("Unit"),
                "Cell" => Some("Cell"),
                ply_core::prelude::TASK_TYPE => Some("Task"),
                SECRET => Some("Secret"),
                _ => match self.decls.get(name) {
                    None => Some("<undeclared>"),
                    Some(decl) if decl.vars.len() != args.len() => Some("<type arity>"),
                    Some(decl) if !self.safe.get(name).copied().unwrap_or(false) => decl
                        .fields
                        .iter()
                        .find_map(|f| self.blocker(f, Some(&decl.vars)))
                        .or(Some("<declaration>")),
                    Some(_) => args.iter().find_map(|t| self.blocker(t, decl_vars)),
                },
            },
        }
    }

    /// Whether `args` may cross as `name`'s arguments.
    ///
    /// Per position, and the two clauses are independently sound, which is why
    /// the weaker one is allowed to rescue the stronger:
    ///
    /// - the declared parameter type is carried **and the value is of the kind
    ///   that type denotes** — no value of that type holds code or a handle,
    ///   whatever this particular value is; **or**
    /// - the value is an [`i64`], a [`bool`] or an `Arc<[u8]>` — childless, so
    ///   it reaches nothing at any depth whatever its declared type says.
    ///
    /// The kind comparison in the first clause is what makes it a claim about
    /// *this* call rather than about the program: `Machine::call` can hand a
    /// definition declared `(Int) -> Int` a [`Value::List`] holding a
    /// [`Value::Closure`], and without it the declared `Int` would license the
    /// list across. `a_value_whose_kind_is_not_its_declared_types_is_refused`
    /// is the tripwire.
    ///
    /// The second clause is what keeps this a **widening** rather than a trade.
    /// Without it a generic definition called at a scalar — `fn settle<a>(r:
    /// Result<R<a>, P>, d: a)`, and a front end is full of them — would be
    /// refused on its `Type::Var` where the value test admitted it, and the
    /// change would lose coverage in one place while gaining it in another.
    /// `a_generic_definition_called_at_a_scalar_is_still_admitted` is the
    /// tripwire; deleting the clause turns it red.
    fn args_cross(&self, name: &Symbol, args: &[Value]) -> bool {
        let Some(flags) = self.params(name) else {
            return false;
        };
        flags.len() == args.len()
            && flags.iter().zip(args).all(|(denotes, value)| {
                denotes.is_some_and(|d| d.matches(value)) || crossable(value)
            })
    }

    /// Whether `value` may cross back as `name`'s answer.
    ///
    /// The mirror of [`CarriedTypes::args_cross`] at one position, and
    /// deliberately the same two clauses in the same order:
    ///
    /// - the declared **return** type is carried and the answer is of the kind
    ///   that type denotes; **or**
    /// - the answer is an [`i64`], a [`bool`] or an `Arc<[u8]>` — childless, so
    ///   it reaches nothing at any depth whatever the declaration says.
    ///
    /// The second clause is [`crossable`] unchanged, which is what makes this a
    /// strict widening of the test `Machine::compiled_answer` used to run: every
    /// answer the old rule accepted is still accepted, including the ones whose
    /// declared return type says something else. That last part is not an
    /// oversight and it is load bearing for two of the eight wrong backends:
    /// `backend::Mutation::WrongType` answers a `Bool` where the definition
    /// returns an `Int`, and `backend::Mutation::Answers` answers an `Int` for a
    /// definition returning anything at all. Both are wrong *answers*, which is
    /// the class this boundary has always said it cannot see and `--engine both`
    /// catches; refusing them here would police a wrong answer with a kind test
    /// and would leave the corpus-scale mutations with nothing to fire on.
    ///
    /// # What this stops being able to prove, stated where the cost is
    ///
    /// For an `Int`, a `Bool` and a `Bytes` the old rule was exact: those kinds
    /// hold no [`Value`], so "nothing that can reach a [`Value::Cell`],
    /// [`Value::Task`], [`Value::Continuation`], [`Value::Closure`] or
    /// [`Value::Secret`] came back" followed from the discriminant. For a
    /// container it does not: this test reads the *top-level* kind and the
    /// declared type, and a declared type is a fact about what the **program**
    /// can build, not about what a backend actually put in the record.
    ///
    /// So a backend that answers `Record { toks: [Cell(..)] }` for a definition
    /// declared `-> Lexed` is believed here. That is a **new obligation on a
    /// backend** and it is the one thing this widening genuinely gives up; it is
    /// written into this module's header as a limit rather than argued away, and
    /// `backend::Mutation::Handle` is the ninth wrong backend that exists to say
    /// what does and does not catch it.
    ///
    /// The argument direction does not have this hole, and the asymmetry is
    /// worth naming: an *argument* is a value the machine's own evaluation built
    /// under a checker that accepted the program, so its interior follows its
    /// declared type. An *answer* is built by the backend.
    pub(crate) fn answer_crosses(&self, name: &Symbol, value: &Value) -> bool {
        self.sigs
            .get(name)
            .and_then(|sig| sig.ret)
            .is_some_and(|d| d.matches(value))
            || crossable(value)
    }

    /// Whether every position of `name`'s declared signature is carried — the
    /// registry question, asked of a definition rather than of a call.
    ///
    /// `backend::carried_signature` is this and nothing else, so a backend's
    /// fragment and this seam's two tests are the same table read three times
    /// rather than three predicates that can drift. They did drift once: between
    /// 2026-08-30 and 2026-08-31 the parameter half was a type test here and a
    /// value test there, and the registry had to spell both.
    ///
    /// A registry holding a definition the machine will refuse to hear from is
    /// not merely untidy: the body runs, the answer is thrown away and the
    /// machine evaluates it again, which is the 26.45 s against 0.04 s ADR 0026
    /// §3 measured for a declined body.
    pub(crate) fn signature_carried(&self, name: &Symbol) -> bool {
        self.sigs
            .get(name)
            .is_some_and(|sig| sig.ret.is_some() && sig.params.iter().all(Option::is_some))
    }
}

/// Which gate refused a call, named rather than collapsed into `None`.
///
/// A refusal that carries no reason is a refusal any *other* gate can satisfy,
/// and this seam has already paid for that once: [`Gate::Anonymous`] was
/// asserted by `an_anonymous_closure_is_never_offered` — whose doc said "the
/// name gate is what refuses it" — and replacing that gate with a fabricated
/// empty [`Symbol`] left the test, and all of this crate's unit tests, green,
/// because [`Gate::PublishedRow`] refuses an unknown name one line later. The
/// test named a mechanism it could not see. Every variant here has a test that
/// asserts *it*, and the table in this module's test header records the deletion
/// that was run against each one and the test that went red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Not a body this machine lowered: a tree-walker closure, a constructor or
    /// a builtin.
    NotLoweredCode,
    /// An argument whose *kind* this boundary does not carry — see
    /// [`crossable_argument_kind`]. Decided from the value alone, with no
    /// lookup.
    ///
    /// > **Narrowed (2026-08-31), when the argument test became a type test.**
    /// > This read: *"An argument this boundary does not carry — see
    /// > [`crossable`]."* It was the whole of the argument gate; it is now the
    /// > cheap half of it, and [`Gate::ArgumentType`] is the half that decides
    /// > what a container may hold.
    ArgumentShape,
    /// A declared parameter type that can reach a [`Value::Cell`],
    /// [`Value::Task`], [`Value::Continuation`], [`Value::Closure`] or
    /// [`Value::Secret`] — or a definition whose declared arity is not the one
    /// the machine is calling. See [`CarriedTypes`].
    ArgumentType,
    /// Inside a `simulate` region.
    SimulateRegion,
    /// A body with no program-wide name: a lambda.
    ///
    /// **It is a *naming* gate, not a callback gate, and the distinction is what
    /// anyone widening it will get wrong (measured 2026-08-31, ADR 0030 §10).**
    /// [`admit`] needs a name here because every gate below it is a lookup keyed
    /// by one — `memo::pure_by_published_row`, [`internally_effectful`],
    /// [`CarriedTypes::args_cross`] — and [`Compiled::enter`] is keyed by one
    /// too. A lambda publishes none of those facts and offers no key, so
    /// admitting one needs a stable per-lambda identity **and** per-lambda
    /// published facts, neither of which exists. That is also why the deletion
    /// recorded in this enum's own doc was invisible: a fabricated empty
    /// [`Symbol`] passes *this* gate and is then refused by
    /// [`Gate::PublishedRow`], which is a lookup that misses.
    ///
    /// **What it costs the SEAM on a front end is 0, and what it costs a code
    /// generator is nearly everything.** Without a backend attached it is 98.51%
    /// of refusals on ADR 0030's workload — 380,176 lambdas of 2,414,170 body
    /// calls. With
    /// `--backend reference` attached it refuses **0**, because `items.parse` is
    /// entered once per file and every lambda is inside that entry. The obstacle
    /// moved to a code generator, where it is three refusals rather than one
    /// (`jit.rs`: the higher-order builtin, the lambda expression, and the call
    /// through a local binding), and ADR 0030 §10 prices it: a backend narrowed
    /// to the definitions a callback-free code generator could compile covers
    /// 61.06% of body calls and has a ceiling of **2.074×**, against an `f` of
    /// 99.65% for one that can enter the root.
    Anonymous,
    /// The published effect row is non-empty, or there is no row to read at all.
    PublishedRow,
    /// The published row is empty and the definition performs anyway, under a
    /// `handle` of its own or of something it calls — see
    /// [`internally_effectful`].
    InternalEffects,
    /// No nested call left before the machine's own bound.
    Budget,
}

/// Whether entering `name` natively would lose a `perform` the interpreter
/// would have run.
///
/// [`Gate::PublishedRow`] answers "can an atom **escape** this call", which is
/// what a row is. This answers the other half — "can an atom be performed and
/// **discharged** inside it" — which no row can, because discharging is exactly
/// what removes an atom from one. `ply_core::DefInfo::internally_effectful`
/// carries it, transitively over the call graph, and is `true` for anything the
/// checker did not positively clear.
///
/// A name with no entry is refused, for the reason [`Gate::PublishedRow`]
/// refuses one: a machine that cannot read the fact has not been told it holds.
///
/// # What it rests on, marked as reasoning rather than as a measurement
///
/// The published fact is reachability over the **named** call graph, and a
/// `perform` can also arrive inside a value that carries code. Every route this
/// pass knows of is closed, but by two gates rather than by one, so the
/// argument is worth writing down instead of trusting:
///
/// - A lambda that performs is syntactically inside some definition's body, and
///   the scan walks lambda bodies — so that definition is marked, and anything
///   naming it inherits the mark.
/// - A closure handed **in** as an argument is refused by [`crossable`] before
///   this gate is reached: no `Value::Closure` crosses this boundary.
/// - A closure fetched out of a cell is reached through a `Value::Cell`, which
///   [`crossable`] also refuses, and a cell cannot outlive the `with_cell` that
///   made it.
/// - A closure **inside** an argument is refused with the argument. Every kind
///   [`crossable`] carries is childless — an `i64`, a `bool`, an `Arc<[u8]>` —
///   so there is no "inside" for one to be in, and the shallow discriminant
///   test is exact for this purpose rather than merely conservative.
///
/// So a definition admitted here takes only [`Value::Int`], [`Value::Bool`] and
/// [`Value::Bytes`], none of which carry code, and the only code it can run is
/// code some definition it names contains. That is an argument, not a proof and
/// not a measurement; what **is** measured is that deleting either gate turns
/// tests red, and the deletion table in this module's test header records which.
///
/// > **Corrected in place (2026-08-30), when [`crossable`] widened.** The
/// > paragraph above read: *"So a definition admitted here takes only
/// > [`Value::Int`] and [`Value::Bool`], which carry no code"*, and the bullet
/// > list had two entries rather than three. The conclusion is unchanged and
/// > the **third bullet is what keeps it true**: this argument does not rest on
/// > the *shortness* of [`crossable`]'s list, it rests on every kind in that
/// > list being childless. A widening that admits a [`Value::List`],
/// > [`Value::Map`], [`Value::Record`] or [`Value::Ctor`] on its discriminant
/// > breaks this gate one field deep, and no amount of transitivity in
/// > `ply_core::DefInfo::internally_effectful` repairs it — the fact is about
/// > the call graph, and a closure arriving in a record is not on it. Anyone
/// > widening this further owes a deep walk or a type-level test, and owes it
/// > *here*, not in a backend.
///
/// > **The debt above is discharged and the third bullet is re-taken
/// > (2026-08-31).** The widening the paragraph warned about — a
/// > [`Value::Record`] or [`Value::Ctor`] admitted as an argument — is taken,
/// > and it is the type-level test the last sentence demanded rather than the
/// > discriminant it forbade. The bullet read: *"A closure **inside** an
/// > argument is refused with the argument. Every kind [`crossable`] carries is
/// > childless — an `i64`, a `bool`, an `Arc<[u8]>` — so there is no "inside"
/// > for one to be in, and the shallow discriminant test is exact for this
/// > purpose rather than merely conservative."*
/// >
/// > That is no longer how it is refused, and the conclusion is unchanged. A
/// > container argument now crosses, so there **is** an "inside"; what keeps a
/// > closure out of it is [`CarriedTypes`], which refuses any declared
/// > parameter type that can transitively reach a function type. The
/// > conservatism moved from the value to the type: a record whose declared
/// > type holds a closure is refused whatever the record in front of it happens
/// > to hold, which is strictly the rule the old bullet wanted and could not
/// > afford to compute on values.
/// >
/// > What it rests on that the old bullet did not: that the machine is running a
/// > program the checker accepted, so a value's kind follows its declared type.
/// > [`crossable_argument_kind`] is the second conjunct that keeps a bare
/// > [`Value::Closure`], [`Value::Cell`], [`Value::Task`],
/// > [`Value::Continuation`] or [`Value::Secret`] out on its discriminant even
/// > when a name's declared signature says otherwise, which is the case
/// > `Machine::call` can construct and a hand-built [`Closure`] can too.
fn internally_effectful(check: Option<&CheckOutput>, name: &Symbol) -> bool {
    check
        .and_then(|check| check.defs.get(name))
        .is_none_or(|def| def.internally_effectful)
}

/// The name a backend may be offered this call under and the budget to offer it
/// with, or the gate that refused the call.
///
/// Split out of `Machine::compiled_answer` so that each gate is a fact a test
/// can assert directly. The machine half of the seam is the two lines around it:
/// the backend lookup, which is what a machine with no backend fails, and the
/// [`CarriedTypes::answer_crosses`] test on the answer.
///
/// > **Corrected in place (2026-08-31).** The last clause read *"and the
/// > [`crossable`] test on the answer"*. [`crossable`] is now one of that test's
/// > two clauses rather than the whole of it. The answer test stays out of
/// > [`Gate`] for the reason the frame ceiling does: a [`Gate`] is a property of
/// > the CANDIDATE, decided before a backend is called, and an answer does not
/// > exist yet at that point.
///
/// The gates are ordered cheapest and most-discriminating first, and
/// [`Gate::ArgumentShape`] deliberately precedes the row lookup: with a backend
/// attached, a call taking a record, a list or a string is refused on one
/// discriminant test per argument and never hashes a [`Symbol`] into
/// [`CheckOutput::defs`]. That ordering is a cost claim, so
/// `the_shape_gate_is_reached_before_the_row_is_looked_up` asserts it.
///
/// > **Half withdrawn (2026-08-31), when the argument test became a type
/// > test.** The sentence above read, verbatim: *"with a backend attached, a
/// > call taking a **record**, a list or a string is refused on one discriminant
/// > test per argument and never hashes a [`Symbol`] into
/// > [`CheckOutput::defs`]"*. A record is exactly the case that no longer holds:
/// > a `Record`, `Ctor`, `List`, `Map` or `Unit` argument now clears
/// > [`Gate::ArgumentShape`] and is decided by [`Gate::ArgumentType`], which
/// > hashes the name into [`CarriedTypes`] first. The half that stands is the
/// > half `Str` and `Float` are in — a `Str`, `Float`, `Decimal`, `Secret`,
/// > `Closure`, `Cell`, `Task` or `Continuation` argument is still refused with
/// > no lookup at all, and that is what
/// > `the_shape_gate_is_reached_before_the_row_is_looked_up` now asserts,
/// > against a name no definition publishes so that a lookup would be visible.
/// >
/// > What the reversal costs is one [`FxHashMap`] lookup per call that gets past
/// > the kind test, and `crate::census`'s header registered the debt before the
/// > widening was taken: *"Whoever takes that widening owes a re-measurement of
/// > that ordering and a per-definition cache."* The cache is
/// > [`CarriedTypes`], built once per [`CheckOutput`] behind a `OnceCell` on
/// > the machine; ADR 0030 §9.2 is where the debt is recorded.
///
/// - **[`Gate::NotLoweredCode`]**: a tree-walker closure carries a program-wide
///   name (`interp.rs:118`) over a body that is a deep clone rather than a node
///   of the program, and `Interp` is the independent oracle `--engine both`
///   audits against. Routing its closures into compiled code would audit the
///   backend against itself.
/// - **[`Gate::SimulateRegion`]**: off inside a `simulate` region for the reason
///   `Machine::constant` is off there — an allocation a search depends on must
///   not be skipped, and an `Access` never recorded is an interleaving never
///   explored. `record_cell_access` and `record_alloc_access` are no-ops outside
///   one, so this single gate is the whole partial-order story: a compiled body
///   cannot fail to record what nothing is recording.
/// - **[`Gate::ArgumentType`]**: the other half of the argument test, and the
///   one that decides what a container may hold. It needs the name, which is
///   why it sits below [`Gate::Anonymous`] rather than beside
///   [`Gate::ArgumentShape`]; it also refuses a call whose argument count is not
///   the definition's declared parameter count, because the mapping from
///   arguments to declared types is positional and a mismatch has no mapping.
///   `Machine::enter_code` raises on that before the hook is reached, so the
///   arity clause is unreachable through the machine and is asserted through
///   [`admit`] directly.
/// - **[`Gate::PublishedRow`]**: necessary and not sufficient, exactly as the
///   memo's note says — an empty row still permits a definition that opens its
///   own `with_cell`. Outside a `simulate` region that allocation is
///   unobservable, which is the same argument `Machine::constant` rests on.
/// - **[`Gate::InternalEffects`]**: the other half of "effects", and the reason
///   an empty row alone was never enough. See this module's header.
/// - **[`Gate::Budget`]**: a zero budget declines, and the interpreted path
///   raises the machine's own call-limit diagnostic rather than a backend's.
pub(crate) fn admit<'a>(
    closure: &'a Closure,
    args: &[Value],
    in_simulate: bool,
    check: Option<&CheckOutput>,
    types: &CarriedTypes,
    max_calls: usize,
    calls: usize,
) -> Result<(&'a Symbol, usize), Gate> {
    admit_with(
        closure,
        args,
        in_simulate,
        check,
        Some(types),
        max_calls,
        calls,
        crossable_argument_kind,
    )
}

/// [`admit`] with the two argument tests supplied, so a census can ask what a
/// wider argument rung would admit without a second copy of the other six
/// gates.
///
/// `types` is `None` for a counterfactual rung that is decided from values
/// alone — which is what every rung of `census::LADDER` is, and what this seam
/// itself was before 2026-08-31.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admit_with<'a>(
    closure: &'a Closure,
    args: &[Value],
    in_simulate: bool,
    check: Option<&CheckOutput>,
    types: Option<&CarriedTypes>,
    max_calls: usize,
    calls: usize,
    carries: impl Fn(&Value) -> bool,
) -> Result<(&'a Symbol, usize), Gate> {
    if !matches!(closure.kind, ClosureKind::Code { .. }) {
        return Err(Gate::NotLoweredCode);
    }
    if !args.iter().all(carries) {
        return Err(Gate::ArgumentShape);
    }
    if in_simulate {
        return Err(Gate::SimulateRegion);
    }
    let name = closure.name.as_ref().ok_or(Gate::Anonymous)?;
    if !crate::memo::pure_by_published_row(check, name) {
        return Err(Gate::PublishedRow);
    }
    if internally_effectful(check, name) {
        return Err(Gate::InternalEffects);
    }
    if let Some(types) = types
        && !types.args_cross(name, args)
    {
        return Err(Gate::ArgumentType);
    }
    let budget = max_calls.checked_sub(calls).ok_or(Gate::Budget)?;
    if budget == 0 {
        return Err(Gate::Budget);
    }
    Ok((name, budget))
}

/// Doubles, because nothing in this workspace implements [`Compiled`].
///
/// `rm -r crates/ply-codegen-spike` must leave a seam that is still exercised
/// rather than a `pub` API with no live caller, so every gate in
/// `Machine::compiled_answer` is asserted here against a backend that would
/// violate it. Two of them are deliberately wrong backends: one answers a value
/// this boundary refuses, one answers the wrong `Int`. The second is not caught
/// here and the test says so.
///
/// # What each gate's test is worth
///
/// A test named after a mechanism it cannot see is this crate's known defect
/// (`CONTRIBUTING.md` §"Things known to be broken" item 13), so each gate's
/// deletion was run against the whole of `cargo test -p ply-eval --lib` — 526
/// tests — and the tests that went red are recorded. Nothing below is reasoning
/// about what a deletion would do.
///
/// The 526 is that suite's size when the table was taken. The frame-ceiling
/// change (`CONTRIBUTING.md` items 9 and 10) has since added one test to it, so
/// a re-run reads 527 green and the reds below are unchanged — none of the six
/// deletions touches the ceiling gate, which lives in `Machine::compiled_answer`
/// rather than here.
///
/// > **Re-taken whole (2026-08-24), for item 11's close.** The paragraph above
/// > is left as it was written; the suite is no longer that size. Adding
/// > [`Gate::InternalEffects`] and four tests takes
/// > `cargo test -p ply-eval --lib` to **531 passed / 0 failed / 1 ignored**.
/// >
/// > A new gate below an old one can mask the old one's deletion — that is the
/// > defect this table exists to catch, and [`Gate::InternalEffects`] sits
/// > below four of the five gates above it. So every row was re-run one at a
/// > time rather than carried, in an `rsync`ed copy of the tree that no other
/// > session could write to, restored and digest-checked after the last one.
/// > Seven reproduce exactly: kind 2, shape 5, `in_simulate` 2, name-test 1,
/// > `budget == 0` 2, `saturating_sub` 0, [`internally_effectful`] 4. **One
/// > moved** — the published-row row, from five to four — and the correction
/// > under the table says which test it lost and why.
/// >
/// > The name-test row's mutation needs one line of scaffolding to compile,
/// > since [`admit`] answers a `&'a Symbol` borrowed from the closure and a
/// > fabricated one is a local: it was run as
/// > `None => Box::leak(Box::new(Symbol::new("")))`, which is the same
/// > substitution the row describes.
///
/// Re-running it: mutate, `touch` the file, and check the run actually printed
/// `Compiling ply-eval` before believing its result. Cargo fingerprints on
/// second-granular mtimes, so a script that rewrites this file and invokes
/// `cargo test` within the same second can be served the previous artifact —
/// which reports the *previous* mutation's reds under the current one's name.
/// Every row below was taken with that check enforced.
///
/// | Deletion from [`admit`] | Red |
/// |---|---|
/// | the [`ClosureKind::Code`] test | `a_body_this_machine_did_not_lower_is_refused_by_the_kind_gate`, `a_tree_walker_closure_with_a_program_wide_name_is_never_offered` |
/// | the [`crossable`] test on arguments | 5, incl. `an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate`, `a_secret_is_never_offered_and_never_accepted` |
/// | `Value::List(_)` **added** to [`crossable`] — the widening this seam must not take without a deep walk | 5, of which `a_container_is_refused_on_its_discriminant_whatever_it_holds` is the one that names the reason |
/// | the `in_simulate` test | `a_call_inside_a_simulate_region_is_refused_by_the_region_gate`, `nothing_is_offered_inside_a_simulate_region` |
/// | the name test, replaced by a fabricated empty [`Symbol`] | `an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate`, **and nothing else** |
/// | the published-row test | 4: `a_definition_that_opens_its_own_simulate_region_is_never_offered`, `a_definition_whose_published_row_is_not_empty_is_never_offered`, `a_row_that_is_not_empty_and_a_row_that_is_missing_are_both_refused_by_the_row_gate`, `an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate` — **re-taken 2026-08-31: 6**, see below |
/// | the [`internally_effectful`] test | 4: `a_definition_that_discharges_its_own_effects_is_refused_by_the_internal_effects_gate`, `a_definition_that_only_calls_one_that_discharges_its_own_effects_is_refused_too`, `nothing_that_performs_under_its_own_handler_is_offered_to_a_backend`, `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop` — **and 2 more outside this suite**, see below |
/// | the `budget == 0` test | `the_last_nested_call_is_refused_by_the_budget_gate`, `the_budget_is_the_machines_remaining_depth_and_never_reaches_zero` |
/// | `checked_sub` weakened to `saturating_sub` | **nothing — 531 still green** |
///
/// > **[`Gate::ArgumentType`]'s rows (2026-08-31).** The argument test is now
/// > two gates and the second is a *type* test, so it gets a table of its own.
/// > Every row was run one at a time against `cargo test -p ply-eval --lib
/// > compiled::` — **44 tests** — with the file `touch`ed, `Compiling ply-eval`
/// > confirmed in the output before the result was believed, the file restored
/// > from a saved copy afterwards and the digest checked. Raw logs:
/// > `/tmp/arc-typegate/red.*.log`.
/// >
/// > | Corruption | Red |
/// > |---|---|
/// > | `CarriedTypes::args_cross` stubbed to `true` — the whole gate | **6**: `a_closure_bearing_record_is_refused_on_its_declared_type`, `a_recursive_type_that_reaches_a_closure_is_refused`, `a_value_whose_kind_is_not_its_declared_types_is_refused`, `a_type_variable_parameter_is_refused_unless_the_value_is_childless`, `a_call_taking_a_non_scalar_is_never_offered`, `an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate` |
/// > | `Denotes::matches` dropped — a carried declared type licenses a value of **any** kind | **2**: `a_value_whose_kind_is_not_its_declared_types_is_refused`, `an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate` |
/// > | the childless-value clause dropped — the `crossable(value)` half of `args_cross` | **5**, re-taken when signatures became written: `a_type_variable_parameter_is_refused_unless_the_value_is_childless`, `a_bytes_crosses_in_as_an_argument_and_out_as_an_answer`, `an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate`, `an_entered_definition_that_opens_its_own_region_skips_an_allocation`, and **`a_bool_crosses_in_both_directions_and_a_float_crosses_in_neither`**, which is the new one — its `twice` is now written `(Float) -> Float`, so the `Int` control that proves the backend is live is admitted by the childless clause and by nothing else |
/// > | the declaration fixpoint replaced by a **single pass** | `a_recursive_type_that_reaches_a_closure_is_refused`, **and nothing else** |
/// > | `Type::Var` admitted | **2**: `a_type_variable_parameter_is_refused_unless_the_value_is_childless`, `a_call_taking_a_non_scalar_is_never_offered` |
/// > | a nominal type read off its **head** — no declaration walk, `Cell`/`Task`/`Secret` unnamed | **2**: `a_world_handle_typed_parameter_is_refused_though_it_is_a_nominal_type`, `a_recursive_type_that_reaches_a_closure_is_refused` |
/// > | `CarriedTypes::carries` made to recurse **into a declaration** instead of reading the fixpoint | `a_recursive_type_is_decided_rather_than_walked_into_itself` **overflows the stack** — `has overflowed its stack / fatal runtime error: stack overflow, aborting`, SIGABRT, the whole binary down |
/// >
/// > Two rows are worth reading twice. The **single pass** row is thin the way
/// > `mark_internal_effects`'s fixpoint row is thin, and for the same reason:
/// > one round settles `type Bad = BLeaf((Int) -> Int) | BNode(Bad)`, and only
/// > the mutually recursive `Ping`/`Pong` pair — where the function type is two
/// > declarations away — can tell a pass from a fixpoint. The **recursion** row
/// > is the only one in either table that does not produce a failed assertion:
/// > it takes the process down, which is what "the precompute must terminate"
/// > means when it is false.
/// >
/// > Two more were run at corpus scale rather than here, and are recorded in
/// > `crates/ply-eval/tests/seam_census.rs` beside the assertion they red:
/// > `Value::Record` removed from [`crossable_argument_kind`] (882,207 ==
/// > 859,104) and the `args_cross` stub again (1,293,678 == 1,207,996). The
/// > corruption that does **not** red anything there — dropping
/// > `Denotes::matches` — is recorded too, because it is the one a reader
/// > reaches for first.
///
/// > **The ANSWER test's rows (2026-08-31).** `Machine::compiled_answer` reads
/// > [`CarriedTypes::answer_crosses`] rather than [`crossable`], so a
/// > definition answering a record can be entered. Same protocol as above: one
/// > corruption at a time against `cargo test -p ply-eval --lib compiled::` —
/// > **51 tests** — the file `touch`ed, `Compiling ply-eval` confirmed in the
/// > output before the result was believed, the file restored from a saved copy
/// > and the digest checked. Raw logs: `/tmp/arc-return/red.*.log`.
/// >
/// > | Corruption | Red |
/// > |---|---|
/// > | [`CarriedTypes::answer_crosses`] stubbed to `true` — the whole answer test | **5**: `a_record_answer_crosses_back_under_its_declared_return_type`, `an_answer_whose_kind_is_not_its_declared_returns_is_refused_unless_it_is_childless`, `a_closure_bearing_record_return_is_refused_however_ordinary_the_record_looks`, `an_answer_this_boundary_refuses_is_declined_and_the_body_is_evaluated`, **`a_secret_is_never_offered_and_never_accepted`** |
/// > | [`Denotes::matches`] dropped from it — a carried declared *return* licenses an answer of **any** kind | **4**: the same, less the closure-bearing one |
/// > | the childless clause dropped — the `crossable(value)` half | **3**: `a_bytes_crosses_in_as_an_argument_and_out_as_an_answer`, `an_answer_whose_kind_is_not_its_declared_returns_is_refused_unless_it_is_childless`, `an_entered_definition_that_opens_its_own_region_skips_an_allocation` |
/// > | `sig.ret.is_some()` dropped from [`CarriedTypes::signature_carried`] — the registry claims a definition the machine will refuse | `a_closure_bearing_record_return_is_refused_however_ordinary_the_record_looks`, **and nothing else** |
/// > | `Reference::run`'s `inner.set_max_calls(fuel)` weakened to `fuel.max(10_000)` — a backend that overspends inside a subtree | `an_entered_subtree_is_bounded_by_the_budget_it_was_handed_and_not_by_its_entry`, **and nothing else** |
/// >
/// > The first row's fifth entry is the one worth reading twice.
/// > `a_secret_is_never_offered_and_never_accepted` is a test about a
/// > **credential**, and it goes red on a corruption of the answer test because
/// > that test is what refuses a [`Value::Secret`] coming *back*. Nothing about
/// > declared return types is involved: `Secret` is refused by the childless
/// > clause failing, which is [`crossable`] doing in 2026-08-31 what it did
/// > before. It is the clearest evidence that the widening is a superset and not
/// > a swap.
/// >
/// > A sixth corruption is recorded at corpus scale in
/// > `crates/ply-eval/tests/seam_census.rs`: `Sig::ret` filled with
/// > `Some(Denotes::Int)` instead of `table.denotes(ret)`, so the precompute and
/// > a walk over the same declared types disagree — 681,277 == 882,207, 200,930
/// > apart. And a ninth wrong backend, `backend::Mutation::Handle`, is in
/// > `crates/ply-eval/tests/differential_corpus.rs` and
/// > `crates/ply-cli/tests/backend.rs`; it is not a deletion but an addition,
/// > because what the widening gave up needed a test that did not exist.
///
/// > **The published-row row re-taken again (2026-08-31): 4 -> 6.** Two tests
/// > joined it and neither is a new claim about the row gate — they are two
/// > *other* claims that turn out to rest on it, which is what a re-take is for.
/// > `a_definition_that_calls_one_that_opens_a_simulate_region_is_never_offered`
/// > is new with the answer widening and is a **subtree** claim: an entered call
/// > now hides everything under it, so "this definition does not open a
/// > `simulate` region" has to mean "nothing it can reach opens one", and the
/// > mechanism that delivers that is the row rather than anything in this
/// > module — `sim.read` escapes, so it propagates to every caller. Under the
/// > deletion the offer list reads `["outer", "searched", "double"]` against
/// > `["double"]`. `the_shape_gate_is_reached_before_the_row_is_looked_up` is
/// > the other, and it was re-taken by the argument widening without this table
/// > being re-run against it.
///
/// The fact [`Gate::InternalEffects`] reads is computed in another crate, so it
/// gets two rows of its own. `ply_core::infer`'s `Checker::mark_internal_effects`
/// seeds a per-body bit and then propagates it over the reference graph, and
/// **the propagation is the half a reviewer is most likely to think is
/// decoration**:
///
/// | Mutation of `mark_internal_effects` | Red |
/// |---|---|
/// | the propagation deleted, leaving the per-body bit — which is the fix as it was first specified | 3: `a_definition_that_only_calls_one_that_discharges_its_own_effects_is_refused_too`, `nothing_that_performs_under_its_own_handler_is_offered_to_a_backend`, `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop` |
/// | the fixpoint replaced by a single pass over the seeds | `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop`, **and nothing else** |
///
/// The second row is the thin one, and it is thin the way the name-test row is:
/// one hop is all `wrapper` needs, so every other test in this block is
/// satisfied by a propagation that stops after one. Only a chain — four
/// wrappers, a mutually recursive pair, a call reached from a lambda — can tell
/// a single pass from a fixpoint.
///
/// > **Corrected in place (2026-08-24).** The last two rows were one row, and it
/// > read: *"| the budget test, replaced by `saturating_sub` |
/// > `the_last_nested_call_is_refused_by_the_budget_gate`,
/// > `the_budget_is_the_machines_remaining_depth_and_never_reaches_zero` |"*.
/// > That credits one mutation with another mutation's result. Re-run one at a
/// > time: deleting the `budget == 0` test turns those two red, and weakening
/// > `checked_sub` to `saturating_sub` turns **nothing** red. The second is not
/// > a hole — `saturating_sub` answers `0` where `checked_sub` answers `None`,
/// > and `0` is what the next line already refuses, so the two spellings are
/// > indistinguishable to any test. `checked_sub` is there to say in the source
/// > that a machine whose `max_calls` was lowered under a live stack is a case
/// > somebody thought about; it is a spelling, not a gate, and it is the one
/// > line in this function no test can bite.
///
/// > **The published-row row re-taken, and it went down (2026-08-24).** It read
/// > *"| the published-row test | 5, incl.
/// > `a_definition_whose_published_row_is_not_empty_is_never_offered` |"*. The
/// > five were the four now listed plus
/// > `a_machine_with_no_check_output_offers_nothing`, measured by deleting the
/// > row gate **and** stubbing [`internally_effectful`] to `false`, which is
/// > this file as it stood before the effects gate: 8 red, of which 3 are the
/// > effects gate's own new tests.
/// >
/// > Adding [`Gate::InternalEffects`] one line below the row gate **masked one
/// > of them**. A machine with no `CheckOutput` fails both gates — the row is
/// > unreadable and so is the flag — so
/// > `a_machine_with_no_check_output_offers_nothing`, which asserts a
/// > behaviour, is now satisfied by whichever gate refuses first and stays
/// > green under the row gate's deletion. That is
/// > [`an_anonymous_closure_is_never_offered`]'s defect reappearing one gate
/// > further down, and the reason this table is re-run rather than reasoned
/// > about. It is not a hole: the mechanism it stands for is asserted by
/// > `a_row_that_is_not_empty_and_a_row_that_is_missing_are_both_refused_by_the_row_gate`,
/// > which reads `Err(Gate::PublishedRow)` for a machine with no `CheckOutput`
/// > and does go red.
/// >
/// > A second test was masked and was **repaired instead of recorded**:
/// > `a_definition_whose_published_row_is_not_empty_is_never_offered` asserted
/// > only that `touch` reaches no backend, which the effects gate also
/// > guarantees. It now asserts `Err(Gate::PublishedRow)` directly and is back
/// > in the row above. Without that repair this row would read 3.
///
/// **The [`internally_effectful`] row is the only one with a corpus behind it,
/// and that is the point of it.** Every other deletion above is caught by hand-built doubles
/// over hand-built programs, which is what item 13's third bullet complained
/// about. Deleting the [`internally_effectful`] test and running
/// `cargo test -p ply-eval --test differential_corpus` also fails
/// **`a_backend_that_answers_correctly_agrees_over_every_corpus_on_disk`** and
/// **`a_definition_that_discharges_its_own_effects_is_in_the_corpus_and_is_never_entered`**
/// — 4 passed, 2 failed — on `tests/fixtures/self_handled_effect.ply`, with
/// `observed footprint — left {self_handled_effect.tally.read[log],
/// self_handled_effect.tally.write[log]}, right {}`. Measured on this tree by
/// deleting the two lines, `touch`ing this file, confirming `Compiling
/// ply-eval` in the output, and restoring.
///
/// The name-test row is the whole of item 13. Before this block that deletion was
/// caught by nothing at all: the row gate refuses an unpublished name one line
/// later, so a fabricated name produced the same *behaviour* through a
/// different mechanism, and `an_anonymous_closure_is_never_offered` — which
/// claimed the name gate by name — stayed green. One test covering one gate is
/// the thinnest row in the table, and it is thin because the gate below it
/// masks it; that is the fact the row is recording, not a gap in it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::*;
    use crate::differential::compare_answers;
    use crate::env::Env;
    use crate::limit::DEFAULT_MAX_CALLS;
    use crate::machine::Machine;
    use crate::value::{Closure, ClosureKind};
    use crate::{Interp, argv};
    use ply_core::{CheckOutput, check_program};
    use ply_span::Diagnostic;
    use ply_syntax::ast::{BinOp, Expr, ExprKind, Item, Program};
    use ply_syntax::resolve::Resolved;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    type Reply = dyn Fn(&Symbol, &[Value], usize) -> Option<Value>;

    /// One call the machine offered a backend.
    #[derive(Clone, Debug, PartialEq)]
    struct Offer {
        name: Symbol,
        args: Vec<Value>,
        budget: usize,
    }

    /// A backend that records every offer and answers by a closure the test
    /// supplies. `describes` is the pointer comparison a real backend owes.
    struct Double {
        /// Never dereferenced. A backend may not borrow the program — see the
        /// `compiled` field on `Machine` for why the field is `'static`.
        program: *const Program,
        reply: Box<Reply>,
        offers: RefCell<Vec<Offer>>,
    }

    impl Double {
        fn over(
            program: &Program,
            reply: impl Fn(&Symbol, &[Value], usize) -> Option<Value> + 'static,
        ) -> Rc<Double> {
            Rc::new(Double {
                program: std::ptr::from_ref(program),
                reply: Box::new(reply),
                offers: RefCell::new(Vec::new()),
            })
        }

        /// Declines everything and remembers what it was offered.
        fn declining(program: &Program) -> Rc<Double> {
            Double::over(program, |_, _, _| None)
        }

        /// Answers `value` for `name` and declines everything else.
        fn answering(program: &Program, name: &str, value: Value) -> Rc<Double> {
            let wanted = Symbol::new(name);
            Double::over(program, move |asked, _, _| {
                (*asked == wanted).then(|| value.clone())
            })
        }

        fn offers(&self) -> Vec<Offer> {
            self.offers.borrow().clone()
        }

        fn names(&self) -> Vec<String> {
            self.offers
                .borrow()
                .iter()
                .map(|o| o.name.as_str().to_string())
                .collect()
        }

        fn forget(&self) {
            self.offers.borrow_mut().clear();
        }
    }

    impl Compiled for Double {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
            self.offers.borrow_mut().push(Offer {
                name: name.clone(),
                args: args.to_vec(),
                budget,
            });
            (self.reply)(name, args, budget)
        }
    }

    /// A program and the check output the purity gate reads. Held together
    /// because a `Machine` borrows all three and must drop before they do.
    struct Checked {
        program: Program,
        resolved: Resolved,
        check: CheckOutput,
    }

    fn checked(items: Vec<Item>) -> Checked {
        let (program, resolved) = standalone(items);
        let check = match check_program(&program, &resolved) {
            Ok(check) => check,
            Err(ds) => panic!("the program under test does not check: {ds:#?}"),
        };
        Checked {
            program,
            resolved,
            check,
        }
    }

    impl Checked {
        fn machine(&self) -> Machine<'_> {
            Machine::new(&self.program, &self.resolved, &self.check)
        }

        fn types(&self) -> CarriedTypes {
            CarriedTypes::over(Some(&self.check))
        }
    }

    /// The same thing from source, because the argument gate is now a question
    /// about *declared types* and `crate::build`'s `fn_def` cannot write one.
    ///
    /// One anonymous module, so names stay bare and every helper above reads the
    /// same as it does over a hand-built AST.
    fn checked_source(source: &str) -> Checked {
        let mut program = ply_syntax::parse_program(vec![(
            ply_span::SourceId(0),
            ply_syntax::ast::ModuleName::anonymous(),
            source,
        )])
        .expect("the fixture must parse");
        let resolved =
            ply_syntax::resolve::resolve(&mut program).expect("the fixture must resolve");
        let check = match check_program(&program, &resolved) {
            Ok(check) => check,
            Err(ds) => panic!("the program under test does not check: {ds:#?}"),
        };
        Checked {
            program,
            resolved,
            check,
        }
    }

    /// A `Code` closure standing for `name` at its declared arity, so [`admit`]
    /// can be asked about a definition the fixture declares.
    fn named(c: &Checked, name: &str) -> Closure {
        let params: Vec<&str> = match &c.check.defs[&Symbol::new(name)].scheme.ty {
            ply_core::ty::Type::Fn { params, .. } => (0..params.len()).map(|_| "p").collect(),
            other => panic!("{name} publishes {other:?} rather than a function type"),
        };
        code_closure(Some(name), &params, int(0))
    }

    /// `Result<Value, Diagnostic>` has no `PartialEq`, and the comparison this
    /// wants is the one `differential` makes: the code, the message, every label
    /// with its span, and every note.
    fn rendered(outcome: &Result<Value, Diagnostic>) -> String {
        format!("{outcome:?}")
    }

    /// `Diagnostic` has no `PartialEq`, so a failing outcome cannot be compared
    /// with `assert_eq!`. A test that wanted a value says so here.
    #[track_caller]
    fn ok(outcome: Result<Value, Diagnostic>) -> Value {
        match outcome {
            Ok(value) => value,
            Err(d) => panic!("expected a value, got {}: {}", d.code, d.message),
        }
    }

    fn double_def() -> Item {
        fn_def_sig(
            "double",
            &[("x", tcon("Int"))],
            tcon("Int"),
            bin(BinOp::Mul, var("x"), int(2)),
        )
    }

    #[test]
    fn a_machine_with_no_backend_never_asks_and_never_counts() {
        let c = checked(vec![double_def()]);
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_counts(), (0, 0));
        assert_eq!(machine.compiled_refusals(), 0);
    }

    /// The property every other claim rests on: with a backend that answers
    /// nothing, the machine is the machine.
    #[test]
    fn a_backend_that_declines_everything_changes_nothing() {
        let items = vec![
            double_def(),
            fn_def_sig(
                "half",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Div, var("x"), int(2)),
            ),
            fn_def_sig(
                "boom",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Div, var("x"), int(0)),
            ),
            fn_def_sig(
                "table",
                &[],
                tapp("List", vec![tcon("Int")]),
                list(vec![int(1), int(2), int(3)]),
            ),
        ];
        let subjects = [
            callv("double", vec![int(21)]),
            bin(
                BinOp::Add,
                callv("double", vec![int(1)]),
                callv("half", vec![int(8)]),
            ),
            callv("boom", vec![int(1)]),
            callv("double", vec![string("not a number")]),
            bin(BinOp::Add, callv("table", vec![]), callv("table", vec![])),
        ];

        let c = checked(items);
        let baseline: Vec<String> = {
            let mut machine = c.machine();
            subjects
                .iter()
                .map(|e| rendered(&machine.eval_expr_for_test(e)))
                .collect()
        };

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        for (e, want) in subjects.iter().zip(&baseline) {
            assert_eq!(&rendered(&machine.eval_expr_for_test(e)), want);
        }
        let (entries, declines) = machine.compiled_counts();
        assert_eq!(entries, 0);
        assert!(declines > 0, "the backend was never offered a call at all");
        assert_eq!(machine.compiled_refusals(), 0);
        assert!(
            backend.offers().iter().any(|o| o.name.as_str() == "double"),
            "the backend was never offered `double`: {:?}",
            backend.offers()
        );
    }

    #[test]
    fn an_accepted_call_gets_its_name_its_arguments_and_a_budget_and_its_answer_is_used() {
        let c = checked(vec![double_def()]);
        let backend = Double::answering(&c.program, "double", Value::Int(84));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());

        // 84 rather than 42: the compiled answer was used and the body was not
        // evaluated.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(84)
        );
        assert_eq!(
            backend.offers(),
            vec![Offer {
                name: Symbol::new("double"),
                args: vec![Value::Int(21)],
                budget: DEFAULT_MAX_CALLS,
            }]
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    #[test]
    fn a_bool_crosses_in_both_directions_and_a_float_crosses_in_neither() {
        let c = checked(vec![
            fn_def_sig(
                "not",
                &[("b", tcon("Bool"))],
                tcon("Bool"),
                un(ply_syntax::ast::UnOp::Not, var("b")),
            ),
            fn_def_sig(
                "twice",
                &[("f", tcon("Float"))],
                tcon("Float"),
                bin(BinOp::Add, var("f"), var("f")),
            ),
        ]);

        let backend = Double::answering(&c.program, "not", Value::Bool(true));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("not", vec![boolean(true)]))),
            Value::Bool(true)
        );
        assert_eq!(backend.offers().len(), 1);
        assert_eq!(backend.offers()[0].args, vec![Value::Bool(true)]);
        drop(machine);

        // ADR 0019 §5 item 4: the spike's fragment accepts `Float` arithmetic and
        // fails on it at run time. A `Float` argument is refused before any
        // backend sees it, so that hole cannot reach a program.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("twice", vec![float(1.5)]))),
            Value::Float(3.0)
        );
        assert!(backend.offers().is_empty(), "a `Float` reached a backend");
        // Control: the same definition with an `Int` is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("twice", vec![int(2)]))),
            Value::Int(4)
        );
        assert_eq!(backend.names(), vec!["twice"]);
    }

    /// The boundary checks the *kind* of what comes back, in every profile. A
    /// `debug_assert!` here would leave the release half — the half a measurement
    /// runs in — unexercised.
    #[test]
    fn an_answer_this_boundary_refuses_is_declined_and_the_body_is_evaluated() {
        let c = checked(vec![double_def()]);
        for refused in [
            Value::str("a string"),
            Value::Float(1.0),
            Value::Unit,
            Value::List(Default::default()),
        ] {
            let backend = Double::answering(&c.program, "double", refused.clone());
            let mut machine = c.machine();
            machine.set_compiled(backend.clone());
            assert_eq!(
                ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
                Value::Int(42),
                "a backend answering {refused:?} was believed"
            );
            assert_eq!(machine.compiled_counts(), (0, 1));
            assert_eq!(machine.compiled_refusals(), 1);
            assert_eq!(backend.offers().len(), 1);
        }
    }

    /// Stated as a limitation, not a guarantee: the seam checks a kind and never
    /// a value. What catches a wrong `Int` is the independent engine.
    #[test]
    fn a_wrong_int_passes_the_seam_and_is_caught_only_by_the_other_engine() {
        let c = checked(vec![double_def()]);
        let backend = Double::answering(&c.program, "double", Value::Int(99));
        let mut machine = c.machine();
        machine.set_compiled(backend);
        let subject = callv("double", vec![int(21)]);
        let from_machine = machine.eval_expr_for_test(&subject);

        assert_eq!(from_machine.as_ref().ok(), Some(&Value::Int(99)));
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(
            machine.compiled_refusals(),
            0,
            "the boundary reported a violation it cannot actually see"
        );

        let mut treewalk = Interp::new(&c.program, &c.resolved, &c.check);
        let from_treewalk = treewalk.eval_expr_for_test(&subject);
        assert_eq!(from_treewalk.as_ref().ok(), Some(&Value::Int(42)));
        assert!(
            compare_answers(
                &treewalk,
                &machine,
                "the expression under test",
                &from_treewalk,
                &from_machine,
            )
            .is_some(),
            "`--engine both` did not report a backend that answered 99 for 42"
        );
    }

    /// `hoist_staleness_audit.rs`'s hazard: a bisection builds a program whose
    /// definitions carry the names of the ones they replace.
    #[test]
    fn a_backend_built_over_another_program_is_ignored() {
        let elsewhere = checked(vec![fn_def_sig(
            "double",
            &[("x", tcon("Int"))],
            tcon("Int"),
            int(1000),
        )]);
        let backend = Double::answering(&elsewhere.program, "double", Value::Int(84));

        let c = checked(vec![double_def()]);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_counts(), (0, 0));
        assert!(backend.offers().is_empty());
    }

    /// `interp.rs` mints a closure per top-level `fn` carrying the program-wide
    /// name, and one handed into a machine reaches `enter_code` through the
    /// `ClosureKind::Fn` arm. Routing those into a backend would audit the
    /// backend against itself.
    #[test]
    fn a_tree_walker_closure_with_a_program_wide_name_is_never_offered() {
        let body = bin(BinOp::Mul, var("x"), int(2));
        let call_it = callv("f", vec![int(21)]);
        let c = checked(vec![double_def()]);

        let treewalk_closure = Value::Closure(Arc::new(Closure {
            name: Some(Symbol::new("double")),
            kind: ClosureKind::Fn {
                params: vec![Symbol::new("x")],
                body: Arc::new(body),
                env: Env::empty(),
                module: 0,
            },
        }));

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let got = machine.eval_expr_in(&call_it, 0, &[(Symbol::new("f"), treewalk_closure)]);
        assert_eq!(ok(got), Value::Int(42));
        assert!(
            backend.offers().is_empty(),
            "a tree-walker closure was routed into a backend: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (0, 0));

        // Control: the machine's own `double`, under the same name the closure
        // above carries, is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A lambda is `ClosureKind::Code` with no name, and nothing anonymous
    /// reaches a backend — a backend is keyed by program-wide name and has
    /// nothing to answer for an anonymous body.
    ///
    /// > **Corrected in place (2026-08-24).** This doc used to say "so the name
    /// > gate is what refuses it". It could not see that: replacing
    /// > `closure.name.as_ref()?` with a fabricated empty `Symbol` left this
    /// > test — and every one of this crate's unit tests — green, because the
    /// > row gate refuses the fabricated name one line later. What it asserts is
    /// > the behaviour. The *mechanism* is asserted by
    /// > [`an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate`],
    /// > which is the test that goes red under that substitution.
    #[test]
    fn an_anonymous_closure_is_never_offered() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let e = bin(
            BinOp::Add,
            call(
                lam(&["x"], bin(BinOp::Mul, var("x"), int(2))),
                vec![int(21)],
            ),
            callv("double", vec![int(0)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(42));
        assert_eq!(
            backend.names(),
            vec!["double"],
            "an anonymous closure was offered to a backend"
        );
    }

    /// A `Code` closure built by hand, so [`admit`] can be asked about a body
    /// the machine would not otherwise hand it: an anonymous one, or one under a
    /// name no definition publishes.
    fn code_closure(name: Option<&str>, params: &[&str], body: Expr) -> Closure {
        Closure {
            name: name.map(Symbol::new),
            kind: ClosureKind::Code {
                params: Rc::new(params.iter().copied().map(Symbol::new).collect()),
                body: crate::code::lower(&body),
                env: Env::empty(),
                module: 0,
            },
        }
    }

    fn double_closure() -> Closure {
        code_closure(Some("double"), &["x"], bin(BinOp::Mul, var("x"), int(2)))
    }

    /// The gate chain over `c`'s program, outside a region and at full budget.
    /// The name is rendered because what a test wants to say is "offered as
    /// `double`", and `Symbol` is not what it wants to write.
    fn gate(c: &Checked, closure: &Closure, args: &[Value]) -> Result<(String, usize), Gate> {
        admit(
            closure,
            args,
            false,
            Some(&c.check),
            &CarriedTypes::over(Some(&c.check)),
            DEFAULT_MAX_CALLS,
            0,
        )
        .map(|(name, budget)| (name.as_str().to_string(), budget))
    }

    /// What every gate test below reads its refusal against: this call, on this
    /// program, clears all of them.
    fn admitted() -> Result<(String, usize), Gate> {
        Ok(("double".to_string(), DEFAULT_MAX_CALLS))
    }

    /// A tree-walker closure carries a program-wide name over a body that is a
    /// deep clone rather than a node of the program, and `Interp` is the oracle
    /// `--engine both` audits the machine against. The behaviour is
    /// [`a_tree_walker_closure_with_a_program_wide_name_is_never_offered`]; this
    /// is the gate that produces it.
    #[test]
    fn a_body_this_machine_did_not_lower_is_refused_by_the_kind_gate() {
        let c = checked(vec![double_def()]);
        let treewalk = Closure {
            name: Some(Symbol::new("double")),
            kind: ClosureKind::Fn {
                params: vec![Symbol::new("x")],
                body: Arc::new(bin(BinOp::Mul, var("x"), int(2))),
                env: Env::empty(),
                module: 0,
            },
        };
        assert_eq!(
            gate(&c, &treewalk, &[Value::Int(21)]),
            Err(Gate::NotLoweredCode)
        );
        assert_eq!(
            gate(&c, &double_closure(), &[Value::Int(21)]),
            admitted(),
            "the same name over a lowered body is refused too, so the test above says nothing"
        );
    }

    /// The kinds this boundary carries, asked of the gate rather than of a run.
    ///
    /// > **One row moved to another gate (2026-08-31).** The loop below read
    /// > `[Value::Float(1.0), Value::str("21"), Value::Unit,
    /// > Value::List(Default::default()), Value::Secret(..)]` against
    /// > `Err(Gate::ArgumentShape)` for all five. A `List` is no longer refused
    /// > on its discriminant — [`crossable_argument_kind`] carries it and
    /// > [`Gate::ArgumentType`] decides it — so it is asserted below against the
    /// > gate that now refuses it, on this fixture because `double`'s parameter
    /// > is declared `Int` and a list is not one. Moving it rather than deleting
    /// > it is the point: the *behaviour* is unchanged and only the reason is,
    /// > and a test that had been left asserting `ArgumentShape` would have gone
    /// > on passing for a gate it no longer named.
    #[test]
    fn an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        for refused in [
            Value::Float(1.0),
            Value::str("21"),
            Value::Unit,
            Value::Secret(Arc::new(Value::Int(21))),
        ] {
            assert_eq!(
                gate(&c, &subject, std::slice::from_ref(&refused)),
                Err(Gate::ArgumentShape),
                "{refused:?} was carried across the boundary"
            );
        }
        assert_eq!(
            gate(&c, &subject, &[Value::List(Default::default())]),
            Err(Gate::ArgumentType),
            "a `List` where `Int` is declared crossed the boundary"
        );
        assert_eq!(gate(&c, &subject, &[Value::Int(21)]), admitted());
        assert_eq!(gate(&c, &subject, &[Value::Bool(true)]), admitted());
        assert_eq!(
            gate(&c, &subject, &[Value::bytes(b"GET / HTTP/1.1\r\n")]),
            admitted(),
            "a `Bytes` argument is refused, and a lexer has no other kind"
        );
    }

    /// The `Bytes` widening, end to end through the machine rather than through
    /// [`admit`]: in as an argument, and out as an answer.
    ///
    /// Both halves are asserted because they are different mechanisms —
    /// [`admit`]'s argument test and `Machine::compiled_answer`'s answer test —
    /// and before 2026-08-30 both refused. ADR 0026 §3 is the reason this is a
    /// test and not a footnote: it recorded `fn read_line(buf: Bytes, ..) ->
    /// Line` being refused on `admit`'s first line, which made the `E = 1.46x`
    /// projection the M9 deferral rests on a projection about a function the
    /// seam would not enter.
    ///
    /// > **Corrected in place (2026-08-31): both mechanisms were named
    /// > [`crossable`] and neither is any more.** The sentence read *"[`admit`]'s
    /// > [`crossable`] test on `args` and `Machine::compiled_answer`'s
    /// > [`crossable`] test on the answer"*. The first is
    /// > [`crossable_argument_kind`] plus [`Gate::ArgumentType`]; the second is
    /// > [`CarriedTypes::answer_crosses`]. What this test still asserts is
    /// > unchanged, and it is worth knowing why: neither end's **type** clause
    /// > can clear `head`, so both clear on the **childless** clause, which is
    /// > [`crossable`] itself. That is what makes this test the control for both
    /// > widenings — it goes red if either one stops being a superset of the
    /// > rule it replaced, and it did go red when the childless clause was
    /// > deleted from `answer_crosses`.
    /// >
    /// > **Corrected again when signatures became written.** The sentence above
    /// > read *"`head` is declared `(Bytes) -> Bytes`"*, which no declaration in
    /// > this file ever made: `head` was built by an untyped `fn_def` and
    /// > inference published `<a>(a) -> a`. It is written at that generality
    /// > now, and it has to be — a written `Bytes` at either end is carried, so
    /// > [`Denotes::Bytes`] would match and this test would stop being the
    /// > control for the childless clause it exists to guard. The `Bytes` is in
    /// > the call rather than in the signature, which is where it always was.
    #[test]
    fn a_bytes_crosses_in_as_an_argument_and_out_as_an_answer() {
        // `head` takes whatever it is given and hands it straight back, so the
        // machine's own answer is a `Bytes` too and the comparison below is
        // between two answers of the same kind.
        let c = checked(vec![fn_def_poly(
            "head",
            &["a"],
            &[("b", tvar("a"))],
            tvar("a"),
            var("b"),
        )]);
        let call = callv("head", vec![bytes(b"GET /orders HTTP/1.1")]);

        // The control: no backend, and the machine's own answer.
        assert_eq!(
            ok(c.machine().eval_expr_for_test(&call)),
            Value::bytes(b"GET /orders HTTP/1.1")
        );

        // In. The backend declines, so the answer is still the machine's, and
        // what is asserted is what it was handed.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&call)),
            Value::bytes(b"GET /orders HTTP/1.1")
        );
        assert_eq!(
            backend.offers()[0].args,
            vec![Value::bytes(b"GET /orders HTTP/1.1")],
            "the `Bytes` did not reach the backend"
        );
        assert_eq!(machine.compiled_counts(), (0, 1));
        drop(machine);

        // Out. A `Bytes` answer is accepted rather than refused, and this is
        // the accept path: the value the program sees is the backend's.
        //
        // It is also, deliberately, a *wrong* answer — `head` would have
        // answered the argument — and the boundary takes it, which is the
        // property this module's header states plainly and `--engine both` is
        // there to catch. `an_answer_this_boundary_refuses_is_declined_and_the_body_is_evaluated`
        // is the same shape for the kinds that are still refused.
        let backend = Double::answering(&c.program, "head", Value::bytes(b"HTTP/1.1 200"));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&call)),
            Value::bytes(b"HTTP/1.1 200")
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(machine.compiled_refusals(), 0);
    }

    /// Inside a `simulate` region every cell touch and every allocation is an
    /// `Access` the search prunes on, and a body the machine did not run records
    /// none of them.
    #[test]
    fn a_call_inside_a_simulate_region_is_refused_by_the_region_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        let args = [Value::Int(21)];
        assert_eq!(
            admit(
                &subject,
                &args,
                true,
                Some(&c.check),
                &CarriedTypes::over(Some(&c.check)),
                DEFAULT_MAX_CALLS,
                0
            ),
            Err(Gate::SimulateRegion)
        );
        assert_eq!(gate(&c, &subject, &args), admitted());
    }

    /// The gate this block exists to arm, and the one hole
    /// `CONTRIBUTING.md` §"Things known to be broken" item 13 named.
    ///
    /// [`an_anonymous_closure_is_never_offered`] asserts the behaviour — nothing
    /// anonymous reaches a backend — and is satisfied by whichever gate happens
    /// to refuse first. Replacing `closure.name.as_ref()?` with a fabricated
    /// empty `Symbol` leaves it green, because the row gate refuses the
    /// fabrication downstream. The third assertion here is that fabrication,
    /// spelled out: a name no definition publishes really is refused by
    /// [`Gate::PublishedRow`], which is *why* it could stand in for the name
    /// gate without anything noticing. Under that substitution the first
    /// assertion reads `Err(Gate::PublishedRow)` and this test is what goes red.
    #[test]
    fn an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate() {
        let c = checked(vec![double_def()]);
        let anonymous = code_closure(None, &["x"], bin(BinOp::Mul, var("x"), int(2)));
        assert_eq!(
            gate(&c, &anonymous, &[Value::Int(21)]),
            Err(Gate::Anonymous)
        );
        assert_eq!(
            gate(&c, &double_closure(), &[Value::Int(21)]),
            admitted(),
            "the same body under a published name is refused too, so the refusal above is not \
             the name"
        );
        let fabricated = code_closure(Some(""), &["x"], bin(BinOp::Mul, var("x"), int(2)));
        assert_eq!(
            gate(&c, &fabricated, &[Value::Int(21)]),
            Err(Gate::PublishedRow),
            "a name the program does not publish cleared the row gate, and the substitution \
             above would now be visible to the behavioural test after all"
        );
    }

    /// The published row is the reviewable artifact, and "no row at all" is the
    /// same refusal as "a row that is not empty" — a machine built without a
    /// `CheckOutput` enters nothing, which is most of this crate's own tests.
    #[test]
    fn a_row_that_is_not_empty_and_a_row_that_is_missing_are_both_refused_by_the_row_gate() {
        let c = checked(vec![
            double_def(),
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("touch")].footprint.is_empty(),
            "the fixture is wrong: `touch` publishes an empty row"
        );
        let effectful = code_closure(Some("touch"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &effectful, &[Value::Int(1)]),
            Err(Gate::PublishedRow)
        );
        let unknown = code_closure(Some("never.declared"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &unknown, &[Value::Int(1)]),
            Err(Gate::PublishedRow)
        );
        assert_eq!(
            admit(
                &double_closure(),
                &[Value::Int(21)],
                false,
                None,
                &CarriedTypes::over(None),
                DEFAULT_MAX_CALLS,
                0
            ),
            Err(Gate::PublishedRow),
            "a machine with no `CheckOutput` cleared a definition it has no row for"
        );
        assert_eq!(gate(&c, &double_closure(), &[Value::Int(21)]), admitted());
    }

    /// `budget` is the machine's remaining nested calls, so the last one belongs
    /// to the machine: the interpreted path raises the bound both engines raise,
    /// at the machine's own span.
    ///
    /// What this test bites is the `budget == 0` refusal — deleting it turns
    /// this and `the_budget_is_the_machines_remaining_depth_and_never_reaches_zero`
    /// red. It does *not* bite the checked subtraction underneath it, and this
    /// doc no longer says it does: see the table in this module's header for why
    /// no test can.
    #[test]
    fn the_last_nested_call_is_refused_by_the_budget_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        let args = [Value::Int(21)];
        let types = CarriedTypes::over(Some(&c.check));
        let at = |max: usize, calls: usize| {
            admit(&subject, &args, false, Some(&c.check), &types, max, calls)
                .map(|(_, budget)| budget)
        };
        assert_eq!(at(8, 8), Err(Gate::Budget));
        // Not evidence for `checked_sub` over `saturating_sub`: both answer
        // `Err` here, one via `None` and one via the `budget == 0` refusal. What
        // it pins is that an over-subscribed stack is refused rather than
        // wrapping to an enormous budget.
        assert_eq!(at(8, 9), Err(Gate::Budget));
        assert_eq!(at(8, 7), Ok(1));
        assert_eq!(at(8, 0), Ok(8));
    }

    /// The ordering is a cost claim — a call taking a record, a list or a string
    /// is refused on one discriminant test per argument and never hashes a
    /// `Symbol` into `CheckOutput::defs` — and a cost claim nothing asserts is a
    /// comment. Two orderings are load-bearing and both are observable now that
    /// a refusal carries its reason.
    #[test]
    fn the_shape_gate_is_reached_before_the_row_is_looked_up() {
        let c = checked(vec![double_def()]);
        let unknown = code_closure(Some("never.declared"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &unknown, &[Value::str("21")]),
            Err(Gate::ArgumentShape),
            "the row was looked up for a call the argument shape had already refused"
        );
        let anonymous = code_closure(None, &["x"], var("x"));
        assert_eq!(
            gate(&c, &anonymous, &[Value::str("21")]),
            Err(Gate::ArgumentShape)
        );
        // Re-taken for the type gate (ADR 0030 §9.2 registered this debt): a
        // `Record` argument is NOT in the lookup-free half any more. Under a
        // name no definition publishes it now reaches the row gate, which is the
        // cost the widening pays and is asserted rather than described.
        let record = Value::Record(Arc::new(BTreeMap::new()));
        assert_eq!(
            gate(&c, &unknown, std::slice::from_ref(&record)),
            Err(Gate::PublishedRow),
            "a `Record` argument is still refused before the row is looked up, so              the cost claim this test re-takes did not actually change"
        );
        assert_eq!(
            gate(&c, &anonymous, &[record]),
            Err(Gate::Anonymous),
            "a `Record` argument under an anonymous body is still refused before              the name gate"
        );
    }

    /// The published row is the reviewable artifact, and it is what both the
    /// constant memo and this boundary read. A definition that can `perform` is
    /// refused whatever the backend claims — and a backend has no route to
    /// `perform` in any case, which is why the refusal is a correctness gate and
    /// not a courtesy.
    ///
    /// > **Strengthened (2026-08-24), because it stopped biting.** This test
    /// > used to assert only the behaviour — that `touch` reaches no backend.
    /// > [`Gate::InternalEffects`] refuses `touch` one line after
    /// > [`Gate::PublishedRow`] does, so once that gate existed, deleting the
    /// > row gate left this test green and the deletion table's row for it fell
    /// > from five reds to three. That is [`an_anonymous_closure_is_never_offered`]'s
    /// > defect exactly, one gate further down. The `gate(..)` assertion below
    /// > is what bites now: under a deleted row gate it reads
    /// > `Err(Gate::InternalEffects)`.
    #[test]
    fn a_definition_whose_published_row_is_not_empty_is_never_offered() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
            fn_def_sig(
                "bump",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(0)),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("touch")].footprint.is_empty(),
            "the fixture is wrong: `touch` publishes an empty row"
        );
        assert_eq!(
            gate(
                &c,
                &code_closure(Some("touch"), &["x"], var("x")),
                &[Value::Int(1)]
            ),
            Err(Gate::PublishedRow),
            "the row gate is not what refused a definition whose row is not empty"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // `bump` is the control: same shape, same arguments, empty row, and it
        // sits inside the same `handle` so the hook is demonstrably live there.
        let e = handle(
            bin(
                BinOp::Add,
                callv("touch", vec![int(1)]),
                callv("bump", vec![int(0)]),
            ),
            vec![clause(
                "state",
                "get",
                None,
                &["n"],
                bin(BinOp::Add, var("n"), int(1)),
            )],
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(2));
        assert_eq!(
            backend.names(),
            vec!["bump"],
            "a definition that can `perform` was offered to a backend"
        );
    }

    /// A program whose `handled` performs and discharges its own operation, and
    /// whose `wrapper` does nothing but call it.
    ///
    /// This is `crates/ply-codegen-spike/tests/fixtures/hazards/effects.ply`
    /// reduced to what the gate turns on. `bump` is the control: same shape,
    /// same arguments, empty row, and genuinely pure.
    fn self_handled() -> Checked {
        checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
            fn_def_sig(
                "handled",
                &[("x", tcon("Int"))],
                tcon("Int"),
                handle(
                    callv("touch", vec![var("x")]),
                    vec![clause(
                        "state",
                        "get",
                        None,
                        &["n"],
                        bin(BinOp::Add, var("n"), int(1)),
                    )],
                ),
            ),
            fn_def_sig(
                "wrapper",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("handled", vec![var("x")]),
            ),
            fn_def_sig(
                "bump",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(0)),
            ),
        ])
    }

    /// The gate this whole change exists for
    /// (`CONTRIBUTING.md` §"Things known to be broken" item 11).
    ///
    /// The first two assertions are what makes the third mean anything: the row
    /// gate *cannot* be what refuses `handled`, because its published row and
    /// its inferred body row are both empty and it is indistinguishable from
    /// `bump` on either. Under a deleted [`Gate::InternalEffects`] the third
    /// assertion reads `Ok(("handled", ..))`.
    #[test]
    fn a_definition_that_discharges_its_own_effects_is_refused_by_the_internal_effects_gate() {
        let c = self_handled();
        let handled = &c.check.defs[&Symbol::new("handled")];
        assert!(
            handled.footprint.is_empty() && handled.performed.is_empty(),
            "the fixture is wrong: `handled` publishes {:?} and performed {:?}, so the row gate \
             would refuse it and this test would prove nothing",
            handled.footprint,
            handled.performed
        );
        assert!(
            crate::memo::pure_by_published_row(Some(&c.check), &Symbol::new("handled")),
            "the row gate refused `handled`, so nothing below is about the effects gate"
        );

        let subject = code_closure(Some("handled"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &subject, &[Value::Int(1)]),
            Err(Gate::InternalEffects)
        );

        let control = code_closure(Some("bump"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &control, &[Value::Int(1)]),
            Ok(("bump".to_string(), DEFAULT_MAX_CALLS)),
            "a genuinely pure definition in the same program was refused too, so the refusal \
             above is not about this program"
        );
    }

    /// The half a per-body fact cannot reach, and the reason
    /// `DefInfo::internally_effectful` is transitive.
    ///
    /// `wrapper` is written with neither `perform` nor `handle`; every fact
    /// about its own text says it is pure, and its published row and inferred
    /// body row are as empty as `bump`'s. It performs `state.read` anyway,
    /// because `handled` does. A gate reading a syntactic per-body bit clears
    /// this and loses exactly the atoms the gate exists to keep.
    #[test]
    fn a_definition_that_only_calls_one_that_discharges_its_own_effects_is_refused_too() {
        let c = self_handled();
        let wrapper = &c.check.defs[&Symbol::new("wrapper")];
        assert!(
            wrapper.footprint.is_empty() && wrapper.performed.is_empty(),
            "the fixture is wrong: `wrapper` publishes a row, so the row gate would refuse it"
        );

        let subject = code_closure(Some("wrapper"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &subject, &[Value::Int(1)]),
            Err(Gate::InternalEffects)
        );

        // The atoms this refusal is protecting, measured rather than asserted
        // from the row: running `wrapper` performs, and the published row says
        // it does not.
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
            Value::Int(2)
        );
        assert_eq!(machine.trace().performs(), 1);
        assert_eq!(
            machine
                .trace()
                .footprint()
                .atoms()
                .map(|a| a.to_string())
                .collect::<Vec<_>>(),
            vec!["state.read".to_string()],
            "the engine recorded no atom, so entering `wrapper` would lose nothing and this \
             gate would be pointless"
        );
    }

    /// The same thing said about a run rather than about the gate: with a
    /// backend attached, neither the definition that handles its own operation
    /// nor the one that merely calls it is ever offered, and the atoms both of
    /// them perform are still recorded.
    #[test]
    fn nothing_that_performs_under_its_own_handler_is_offered_to_a_backend() {
        let c = self_handled();
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());

        let e = bin(
            BinOp::Add,
            bin(
                BinOp::Add,
                callv("handled", vec![int(1)]),
                callv("wrapper", vec![int(10)]),
            ),
            callv("bump", vec![int(100)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(113));
        assert_eq!(
            backend.names(),
            vec!["bump"],
            "a definition that performs under its own handler was offered to a backend"
        );
        assert_eq!(machine.trace().performs(), 2);
    }

    /// The whole partial-order story, and the reason it is one gate: inside a
    /// region every cell touch and every allocation is an `Access` the search
    /// prunes on, and a body the machine did not run records none of them.
    /// Outside one there is no trail to disturb.
    #[test]
    fn nothing_is_offered_inside_a_simulate_region() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // The control is the second half of the same expression, on the same
        // machine and the same definition: `double(1)` outside the region is
        // offered, so the silence inside it is a gate firing rather than a
        // fixture that never reached the hook.
        let e = bin(
            BinOp::Add,
            ex(ExprKind::Simulate {
                body: Box::new(callv("double", vec![int(21)])),
            }),
            callv("double", vec![int(1)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(44));
        assert_eq!(
            backend.offers(),
            vec![Offer {
                name: Symbol::new("double"),
                args: vec![Value::Int(1)],
                budget: DEFAULT_MAX_CALLS,
            }],
            "a call inside a `simulate` region reached the backend"
        );
    }

    /// The read side of the constant memo stays ahead of the hook, and the write
    /// side still goes through `Frame::Call { memo }`.
    #[test]
    fn a_nullary_constant_is_entered_once_and_memoized_afterwards() {
        let c = checked(vec![fn_def_sig("answer", &[], tcon("Int"), int(1))]);
        let backend = Double::answering(&c.program, "answer", Value::Int(7));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let e = bin(
            BinOp::Add,
            callv("answer", vec![]),
            bin(BinOp::Add, callv("answer", vec![]), callv("answer", vec![])),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(21));
        assert_eq!(
            backend.offers().len(),
            1,
            "the memo did not take over after the first compiled entry: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// `limit.rs` exists so a runaway recursion is a diagnostic in both engines.
    /// A backend is handed the machine's own remaining depth so it can decline
    /// rather than recurse natively past it; when it declines, the machine raises
    /// exactly what it raises with no backend at all.
    #[test]
    fn the_budget_is_the_machines_remaining_depth_and_never_reaches_zero() {
        let c = checked(vec![fn_def_sig(
            "down",
            &[("n", tcon("Int"))],
            tcon("Int"),
            if_(
                bin(BinOp::Eq, var("n"), int(0)),
                int(0),
                callv("down", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        )]);
        let subject = callv("down", vec![int(1_000)]);

        let baseline = rendered(&c.machine().with_max_calls(8).eval_expr_for_test(&subject));
        assert!(
            baseline.contains("recursion limit of 8 nested calls exceeded"),
            "the fixture never reached the bound: {baseline}"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine().with_max_calls(8);
        machine.set_compiled(backend.clone());
        assert_eq!(rendered(&machine.eval_expr_for_test(&subject)), baseline);

        let budgets: Vec<usize> = backend.offers().iter().map(|o| o.budget).collect();
        assert_eq!(
            budgets,
            vec![8, 7, 6, 5, 4, 3, 2, 1],
            "a backend was handed a depth the machine did not have left"
        );
    }

    /// `argv.rs` is 40.9% of ADR 0019 §1. The entered path takes the same buffer
    /// the interpreted path takes and owes the same hand-back.
    #[test]
    fn an_entered_call_returns_its_argument_vector_to_the_free_list() {
        let c = checked(vec![double_def()]);
        let subject = callv("double", vec![int(21)]);

        argv::drain_the_free_list();
        let mut interpreted = c.machine();
        assert_eq!(ok(interpreted.eval_expr_for_test(&subject)), Value::Int(42));
        let after_interpreted = argv::kept();

        argv::drain_the_free_list();
        let backend = Double::answering(&c.program, "double", Value::Int(84));
        let mut machine = c.machine();
        machine.set_compiled(backend);
        assert_eq!(ok(machine.eval_expr_for_test(&subject)), Value::Int(84));
        let after_entered = argv::kept();

        assert!(
            after_interpreted[0] > 0,
            "the fixture never used a pooled buffer at all"
        );
        assert_eq!(
            after_entered, after_interpreted,
            "the entered path left the free list in a different state than the interpreted one"
        );
    }

    /// The gates are ordered so that the argument shape is tested before the name
    /// is looked up. What is observable is the refusal; the ordering is a cost
    /// claim and is not asserted here.
    #[test]
    fn a_call_taking_a_non_scalar_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def_poly(
                "head",
                &["a"],
                &[("xs", tapp("List", vec![tvar("a")]))],
                tcon("Int"),
                callv("len", vec![var("xs")]),
            ),
            fn_def_poly(
                "width",
                &["a"],
                &[("s", tapp("List", vec![tvar("a")]))],
                tcon("Int"),
                callv("len", vec![var("s")]),
            ),
        ]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("head", vec![list(vec![int(1), int(2)])]))),
            Value::Int(2)
        );
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("width", vec![string("abcd")]))),
            Value::Int(4)
        );
        assert!(
            backend.offers().is_empty(),
            "a non-scalar argument reached a backend: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (0, 0));

        // Control: the hook is live on this machine.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A `Secret` may not cross in either direction: `value.rs` redacts it on
    /// render and `escape.rs` walks its payload deliberately, and a backend
    /// builds messages the machine never sees.
    #[test]
    fn a_secret_is_never_offered_and_never_accepted() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // Reaching `enter_code` with a `Secret` argument goes through `call`,
        // which is the only route that can carry a value the program did not
        // build. Checked rather than assumed: `escape::check` lets it through and
        // the machine's own arithmetic is what refuses it, so the value did reach
        // the hook and the argument gate is what kept it from the backend.
        let outcome = machine.call(
            "double",
            vec![Value::Secret(Arc::new(Value::str("hunter2")))],
            sp(),
        );
        let rendered = rendered(&outcome);
        assert!(
            rendered.contains("E0502") && rendered.contains("arithmetic expects Int"),
            "the fixture stopped reaching the hook: {rendered}"
        );
        assert!(!rendered.contains("hunter2"), "a credential was printed");
        assert!(
            backend.offers().is_empty(),
            "a `Secret` was handed to a backend"
        );
        backend.forget();
        // Control: the same definition with an `Int` is offered.
        assert_eq!(
            ok(machine.call("double", vec![Value::Int(21)], sp())),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
        drop(machine);

        let answering =
            Double::answering(&c.program, "double", Value::Secret(Arc::new(Value::Int(1))));
        let mut machine = c.machine();
        machine.set_compiled(answering);
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_refusals(), 1);
    }

    /// The `--engine both` comparison, taken between a machine with a backend
    /// and one without: the rendered value, the outcome field by field, the
    /// footprint, and the cell arena slot by slot.
    #[track_caller]
    fn agree_on(c: &Checked, backend: Rc<Double>, e: &Expr) {
        let mut plain = c.machine();
        let mut entered = c.machine();
        entered.set_compiled(backend);
        let left = plain.eval_expr_for_test(e);
        let right = entered.eval_expr_for_test(e);
        if let Some(d) =
            compare_answers(&plain, &entered, "the expression under test", &left, &right)
        {
            panic!("a backend changed what the machine did — {d}");
        }
    }

    /// A continuation cannot be captured beneath a native activation, because
    /// nothing runs in the machine while a body runs and the body has returned
    /// before its `Frame::Call` is even pushed. The fixture resumes twice, so a
    /// compiled entry that had left anything parked would be entered twice
    /// against one activation.
    #[test]
    fn a_multi_shot_resume_over_an_entered_call_answers_what_the_machine_answers() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "triple",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Mul, var("x"), int(3)),
            ),
        ]);
        let e = handle(
            bin(
                BinOp::Add,
                perform("state", "get", None, vec![]),
                callv("triple", vec![int(2)]),
            ),
            vec![general_clause(
                "state",
                "get",
                None,
                &[],
                "k",
                bin(
                    BinOp::Add,
                    callv("k", vec![int(1)]),
                    callv("k", vec![int(10)]),
                ),
            )],
        );

        // The backend answers exactly what the body computes, so an identical
        // result is evidence about the control flow rather than about the value.
        agree_on(
            &c,
            Double::answering(&c.program, "triple", Value::Int(6)),
            &e,
        );

        let backend = Double::answering(&c.program, "triple", Value::Int(6));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // `k(1)` gives `1 + 6`, `k(10)` gives `10 + 6`.
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(23));
        assert_eq!(
            backend.names(),
            vec!["triple", "triple"],
            "the fixture did not enter compiled code once per resumption"
        );
        assert_eq!(machine.compiled_counts(), (2, 0));
    }

    /// The other half of the same invariant: a clause that never resumes leaves
    /// the delimiter with its own value, and an entered call that already
    /// finished is not parked waiting for anything.
    #[test]
    fn a_discarded_continuation_over_an_entered_call_halts_with_the_handlers_value() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "triple",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Mul, var("x"), int(3)),
            ),
        ]);
        let e = handle(
            bin(
                BinOp::Add,
                callv("triple", vec![int(2)]),
                perform("state", "get", None, vec![]),
            ),
            vec![general_clause("state", "get", None, &[], "k", int(99))],
        );

        agree_on(
            &c,
            Double::answering(&c.program, "triple", Value::Int(6)),
            &e,
        );

        let backend = Double::answering(&c.program, "triple", Value::Int(6));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(99));
        assert_eq!(
            backend.names(),
            vec!["triple"],
            "the fixture never entered compiled code before the clause discarded"
        );
    }

    /// `differential::audit_state` compares the final arena as the ordered
    /// `(Slot, rendered value)` sequence, and an entered call must leave it
    /// alone. Here the entered definition touches no cell and its caller does.
    #[test]
    fn a_cell_touching_caller_agrees_slot_for_slot_with_an_entered_callee() {
        let c = checked(vec![fn_def_sig(
            "bump",
            &[("x", tcon("Int"))],
            tcon("Int"),
            bin(BinOp::Add, var("x"), int(1)),
        )]);
        let e = with_cell(
            "s",
            int(1),
            "c",
            block(
                vec![discard(callv(
                    "cell_set",
                    vec![var("c"), callv("bump", vec![int(8)])],
                ))],
                Some(callv("cell_get", vec![var("c")])),
            ),
        );
        agree_on(&c, Double::answering(&c.program, "bump", Value::Int(9)), &e);

        let backend = Double::answering(&c.program, "bump", Value::Int(9));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(9));
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// The one difference this boundary knowingly makes, asserted rather than
    /// assumed. `memo.rs`'s note names the case: a definition that opens its own
    /// `with_cell` publishes an empty row, so it passes the purity gate, and a
    /// compiled entry skips the allocation the interpreter makes.
    ///
    /// Unobservable to the program — the arena the two runs end with is equal
    /// slot for slot, which is what `compare_answers` checks — and observable to
    /// `w6-alloc`, which is why a `w6-alloc` figure taken with a backend attached
    /// may not be quoted from a run without one.
    #[test]
    fn an_entered_definition_that_opens_its_own_region_skips_an_allocation() {
        let c = checked(vec![fn_def_poly(
            "boxed",
            &["a"],
            &[("n", tvar("a"))],
            tvar("a"),
            with_cell("s", var("n"), "c", callv("cell_get", vec![var("c")])),
        )]);
        assert!(
            c.check.defs[&Symbol::new("boxed")].footprint.is_empty(),
            "the fixture is wrong: `boxed` does not publish an empty row"
        );
        let e = callv("boxed", vec![int(5)]);

        // Program-visible state is identical, arena included.
        agree_on(
            &c,
            Double::answering(&c.program, "boxed", Value::Int(5)),
            &e,
        );

        let mut plain = c.machine();
        assert_eq!(ok(plain.eval_expr_for_test(&e)), Value::Int(5));
        let interpreted_allocations = plain.cells().stats().allocations;

        let mut entered = c.machine();
        entered.set_compiled(Double::answering(&c.program, "boxed", Value::Int(5)));
        assert_eq!(ok(entered.eval_expr_for_test(&e)), Value::Int(5));
        assert_eq!(entered.compiled_counts(), (1, 0));

        assert!(interpreted_allocations > 0);
        assert_eq!(
            entered.cells().stats().allocations,
            interpreted_allocations - 1,
            "the accounting this test exists to pin down moved"
        );
    }

    /// A backend cannot raise — `enter` answers a `Value` or nothing — so every
    /// diagnostic a run produces is the machine's own, at the machine's own span,
    /// with the machine's own labels and notes. `rt::error`'s `Span::DUMMY` and
    /// its "in compiled code" labels are unreachable through this seam rather
    /// than mitigated on it.
    #[test]
    fn a_failure_after_an_accepted_call_is_the_machines_own_diagnostic() {
        let c = checked(vec![
            fn_def_sig(
                "safe",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(1)),
            ),
            fn_def_sig(
                "risky",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Div, int(10), var("x")),
            ),
        ]);
        let subjects = [
            bin(BinOp::Div, callv("safe", vec![int(1)]), int(0)),
            callv("risky", vec![callv("safe", vec![int(-1)])]),
            bin(
                BinOp::Add,
                callv("safe", vec![int(i64::MAX - 1)]),
                int(i64::MAX),
            ),
        ];

        let baseline: Vec<String> = {
            let mut plain = c.machine();
            subjects
                .iter()
                .map(|e| rendered(&plain.eval_expr_for_test(e)))
                .collect()
        };
        assert!(
            baseline.iter().all(|r| r.starts_with("Err(")),
            "the fixture stopped failing: {baseline:?}"
        );

        // Faithful rather than constant: what is under test is where the
        // diagnostic comes from, and a backend answering the wrong number would
        // be testing that instead.
        let backend = Double::over(&c.program, |name, args, _| match (name.as_str(), args) {
            ("safe", [Value::Int(x)]) => x.checked_add(1).map(Value::Int),
            _ => None,
        });
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        for (e, want) in subjects.iter().zip(&baseline) {
            assert_eq!(&rendered(&machine.eval_expr_for_test(e)), want);
        }
        let (entries, _) = machine.compiled_counts();
        assert_eq!(
            entries,
            subjects.len() as u64,
            "the backend was never entered, so this proves nothing about failures under it"
        );
    }

    /// The purity gate reads the published row, so a machine driven without a
    /// type-check pass has nothing to clear a definition with and the hook is
    /// inert. `Machine::for_program` is what the corpus harness, the prover's
    /// generators and most of this crate's own tests build, so this is the
    /// common case rather than a corner: found by the entry counter in
    /// `tests/differential_corpus.rs`, which was green over a seam it had never
    /// once reached.
    #[test]
    fn a_machine_with_no_check_output_offers_nothing() {
        let (program, resolved) = standalone(vec![double_def()]);
        let backend = Double::declining(&program);
        let mut machine = Machine::for_program(&program, &resolved);
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert!(backend.offers().is_empty());
        assert_eq!(machine.compiled_counts(), (0, 0));
    }

    /// A `simulate` in a *definition's* body is a different case from a call
    /// made inside a live region, and it is refused by a different gate: the row
    /// gains `sim.read`, so the purity gate takes it. Armed rather than asserted
    /// — the row is read out of the fixture before the run.
    #[test]
    fn a_definition_that_opens_its_own_simulate_region_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def_sig(
                "searched",
                &[("n", tcon("Int"))],
                tcon("Int"),
                ex(ExprKind::Simulate {
                    body: Box::new(bin(BinOp::Add, var("n"), int(1))),
                }),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("searched")].footprint.is_empty(),
            "the fixture is wrong: a `simulate` body published an empty row"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&bin(
                BinOp::Add,
                callv("searched", vec![int(1)]),
                callv("double", vec![int(0)]),
            ))),
            Value::Int(2)
        );
        assert_eq!(
            backend.names(),
            vec!["double"],
            "a definition that opens a `simulate` region was offered to a backend"
        );
    }

    #[test]
    fn crossable_admits_the_two_scalars_and_bytes_and_nothing_else() {
        assert!(crossable(&Value::Int(0)));
        assert!(crossable(&Value::Bool(false)));
        assert!(crossable(&Value::bytes(b"GET /orders HTTP/1.1")));
        assert!(
            crossable(&Value::bytes(b"")),
            "an empty `Bytes` is a `Bytes`"
        );
        for refused in [
            Value::Float(0.0),
            Value::str("s"),
            Value::Unit,
            Value::List(Default::default()),
            Value::Secret(Arc::new(Value::Int(1))),
            Value::Secret(Arc::new(Value::bytes(b"hunter2"))),
        ] {
            assert!(!crossable(&refused), "{refused:?} crossed the boundary");
        }
    }

    /// The property that makes [`crossable`]'s shallow test a sound one, asked
    /// of the containers rather than of the scalars.
    ///
    /// [`internally_effectful`]'s transitivity argument rests on nothing that
    /// carries code crossing, and it reaches that conclusion from the *kinds*
    /// [`crossable`] carries being childless. A `List`, `Map`, `Record` or
    /// `Ctor` is not: each of these holds a `Closure` one step down, and a
    /// widening that admitted them on their discriminant would hand a backend a
    /// value that can `perform` while the effects gate reported it could not.
    ///
    /// So this is a tripwire rather than a description. Add any one of these
    /// kinds to [`crossable`]'s `matches!` and this test goes red, which is the
    /// prompt to write the deep walk or the type-level test instead. **Seen to
    /// fail, 2026-08-30**: `Value::List(_)` added to the `matches!` reds
    /// **5** of this module's tests — this one first and on the `List` row,
    /// with the message below, and then
    /// `crossable_admits_the_two_scalars_and_bytes_and_nothing_else`,
    /// `an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate`,
    /// `a_call_taking_a_non_scalar_is_never_offered` and
    /// `an_answer_this_boundary_refuses_is_declined_and_the_body_is_evaluated`.
    /// Four of those five would red for *any* widening; this one is the only
    /// one that reds for the reason that matters, which is why it exists.
    ///
    /// > **Withdrawn whole and replaced (2026-08-31), because the rule it
    /// > guarded is gone.** The test was
    /// > `a_container_is_refused_on_its_discriminant_whatever_it_holds`, and it
    /// > asserted `!crossable(&holding)` for a `List`, `Record`, `Ctor` and
    /// > `Map` each holding a `Closure`, plus an empty one of each as the
    /// > control, under the doc above — which is kept verbatim because it is the
    /// > argument this change had to answer, not a sentence that turned out to
    /// > be wrong.
    /// >
    /// > A container argument now crosses; the prompt in the doc's third
    /// > paragraph was taken, and the type-level test is what took it. The
    /// > tripwire has to move with the rule: refusing a `Record` **on its
    /// > discriminant** is no longer a property this seam has, and a test
    /// > asserting it would have to be deleted rather than corrected the day the
    /// > widening landed — which is exactly the shape this project's rules
    /// > forbid. What replaces it asserts the hazard rather than the mechanism:
    /// > a record whose **declared type** can hold code must not cross, however
    /// > ordinary the record in front of it looks.
    ///
    /// The empty control the old test carried survives here and matters more
    /// than it did: a rule that walked the *value* would carry an empty
    /// closure-bearing record, because an empty one holds no closure. `Box`'s
    /// value below is a real record with a real closure in it and `EmptyBox`'s
    /// is a record of one `Int`, and both are refused for their type.
    #[test]
    fn a_closure_bearing_record_is_refused_on_its_declared_type() {
        let c = checked_source(
            "type Box = { run: (Int) -> Int, tag: Int }\n\
             type Plain = { tag: Int }\n\
             fn use_box(b: Box) -> Int = b.tag\n\
             fn use_plain(p: Plain) -> Int = p.tag\n",
        );
        let mut fields = BTreeMap::new();
        fields.insert(
            Symbol::new("run"),
            Value::Closure(Arc::new(code_closure(
                None,
                &["y"],
                bin(BinOp::Mul, var("y"), int(2)),
            ))),
        );
        fields.insert(Symbol::new("tag"), Value::Int(1));
        let holding = Value::Record(Arc::new(fields));

        assert_eq!(
            gate(&c, &named(&c, "use_box"), &[holding]),
            Err(Gate::ArgumentType),
            "a record whose declared type holds a `Closure` crossed the boundary, so \
             `internally_effectful`'s argument now has a hole one field deep"
        );

        // The same record *shape* under a declared type that cannot hold code:
        // `tag` alone. This is the control that says the refusal above is the
        // type's and not the kind's — without it, a `crossable_argument_kind`
        // that had simply kept refusing `Record` would pass the assertion above.
        let mut plain = BTreeMap::new();
        plain.insert(Symbol::new("tag"), Value::Int(1));
        assert_eq!(
            gate(
                &c,
                &named(&c, "use_plain"),
                &[Value::Record(Arc::new(plain))]
            ),
            Ok(("use_plain".to_string(), DEFAULT_MAX_CALLS)),
            "a record of `Int` did not cross, so the widening bought nothing"
        );

        // And the empty one, which is where a value walk would have differed:
        // an empty `Box` holds no closure and is refused anyway, because the
        // question asked is about the type.
        assert_eq!(
            gate(
                &c,
                &named(&c, "use_box"),
                &[Value::Record(Arc::new(BTreeMap::new()))]
            ),
            Err(Gate::ArgumentType),
            "an empty record under a closure-bearing declared type crossed"
        );
    }

    /// A declared sum type is decided from its constructors' field types, and a
    /// type that mentions itself must not make that decision recurse.
    ///
    /// Termination here is structural rather than budgeted, which is the whole
    /// difference from the deep *value* walk `crate::census` prices: the
    /// declarations are solved as a fixpoint over **names** — every declared
    /// type starts carried and a pass lowers the ones with an uncarried field,
    /// repeated until nothing moves — so `CarriedTypes::carries` never recurses
    /// into a declaration at all. It only walks a use-site type expression,
    /// which is finite because a recursive *record alias* is a compile error
    /// (`type alias `X` expands into itself`) and a recursive *sum* type is a
    /// name.
    ///
    /// `Tree` below is directly recursive and `Even`/`Odd` are mutually
    /// recursive, because one round of a fixpoint settles the first and only a
    /// real iteration settles the second.
    #[test]
    fn a_recursive_type_is_decided_rather_than_walked_into_itself() {
        let c = checked_source(
            "type Tree = | Leaf | Node(Tree, Int)\n\
             type Even = | EZero | ESucc(Odd)\n\
             type Odd = | OSucc(Even)\n\
             fn use_tree(t: Tree) -> Int = 0\n\
             fn use_even(e: Even) -> Int = 0\n",
        );
        let leaf = Value::Ctor {
            name: Symbol::new("Leaf"),
            args: Arc::new(Vec::new()),
        };
        assert_eq!(
            gate(&c, &named(&c, "use_tree"), &[leaf]),
            Ok(("use_tree".to_string(), DEFAULT_MAX_CALLS)),
            "a recursive type of carried fields was refused"
        );
        let zero = Value::Ctor {
            name: Symbol::new("EZero"),
            args: Arc::new(Vec::new()),
        };
        assert_eq!(
            gate(&c, &named(&c, "use_even"), &[zero]),
            Ok(("use_even".to_string(), DEFAULT_MAX_CALLS)),
            "a mutually recursive pair of carried types was refused"
        );
    }

    /// The other side of it: recursion must not make a type that *does* reach a
    /// closure look carried.
    ///
    /// The fixpoint's assumption is "carried until shown otherwise", so a cycle
    /// that reaches a function type has to be found by iteration rather than
    /// hidden by the cycle. `Bad` reaches one directly; `Ping`/`Pong` reach one
    /// only through the pair, which a single pass over the declarations settles
    /// the wrong way.
    #[test]
    fn a_recursive_type_that_reaches_a_closure_is_refused() {
        let c = checked_source(
            "type Bad = | BLeaf((Int) -> Int) | BNode(Bad)\n\
             type Ping = | PNil | PCons(Pong)\n\
             type Pong = | QNil | QCons(Ping, (Int) -> Int)\n\
             fn use_bad(b: Bad) -> Int = 0\n\
             fn use_ping(p: Ping) -> Int = 0\n",
        );
        for (name, ctor) in [("use_bad", "BNode"), ("use_ping", "PNil")] {
            let value = Value::Ctor {
                name: Symbol::new(ctor),
                args: Arc::new(Vec::new()),
            };
            assert_eq!(
                gate(&c, &named(&c, name), &[value]),
                Err(Gate::ArgumentType),
                "{name} took a value whose declared type reaches a closure"
            );
        }
    }

    /// Generics: refused on the type, and rescued by the value when the value is
    /// childless.
    ///
    /// A `Type::Var` can be instantiated at a closure at some call site, so the
    /// declared type cannot clear it and this gate does not try — which is the
    /// design decision `CarriedTypes`'s header argues, and the whole of the gap
    /// between the type gate and a shallow kind test on `ply test examples`
    /// (84.1% against 91.8%).
    ///
    /// The second clause of `args_cross` is why this change is a widening rather
    /// than a trade: `Int`, `Bool` and `Bytes` are childless, so a *value* of one
    /// crosses whatever its declared type says, and a generic definition called
    /// at a scalar — which the value test admitted before this change — goes on
    /// being admitted. Delete that clause and this test goes red on its first
    /// assertion.
    #[test]
    fn a_type_variable_parameter_is_refused_unless_the_value_is_childless() {
        let c = checked_source(
            "fn poly<a>(x: a, n: Int) -> Int = n\n\
             fn ints(xs: List<Int>) -> Int = len(xs)\n",
        );
        // The decision itself, asked of the type rather than of a call, because
        // the two assertions below are both satisfied by a rule that admits
        // `Type::Var` and is rescued by the kind comparison. This one is not.
        let types = c.types();
        let ply_core::ty::Type::Fn { params, .. } = &c.check.defs[&Symbol::new("poly")].scheme.ty
        else {
            panic!("poly publishes no function type");
        };
        assert!(
            matches!(params[0], ply_core::ty::Type::Var(_)),
            "the fixture stopped being generic: {:?}",
            params[0]
        );
        assert!(
            !types.carries(&params[0], None),
            "a `Type::Var` is carried, so a closure passed at that position would cross"
        );
        assert!(
            types.carries(&params[1], None),
            "the control failed: `Int` is not carried"
        );

        let poly = named(&c, "poly");
        assert_eq!(
            gate(&c, &poly, &[Value::Int(1), Value::Int(2)]),
            Ok(("poly".to_string(), DEFAULT_MAX_CALLS)),
            "a generic definition called at a scalar stopped being admitted, so \
             the type gate is a trade and not a widening"
        );
        assert_eq!(
            gate(
                &c,
                &poly,
                &[Value::List(Arc::new(vec![Value::Int(1)])), Value::Int(2)]
            ),
            Err(Gate::ArgumentType),
            "a container crossed under a `Type::Var`, which can be a closure"
        );
        // And the same container under a declared `List<Int>` does cross, so the
        // refusal above is the variable's and not the list's.
        assert_eq!(
            gate(
                &c,
                &named(&c, "ints"),
                &[Value::List(Arc::new(vec![Value::Int(1)]))]
            ),
            Ok(("ints".to_string(), DEFAULT_MAX_CALLS))
        );
    }

    /// A declared type licenses a value only when the value is of the kind that
    /// type denotes.
    ///
    /// The type gate reasons from the checker's answer, and `Machine::call` is
    /// the route that can carry a value the checker never saw — it is how
    /// `a_secret_is_never_offered_and_never_accepted` reaches the hook at all. A
    /// definition declared `(Int) -> Int` handed a `List` holding a `Closure`
    /// would otherwise be licensed across by its own declared `Int`.
    #[test]
    fn a_value_whose_kind_is_not_its_declared_types_is_refused() {
        let c = checked_source("fn twice(x: Int) -> Int = x * 2\n");
        let holding = Value::List(Arc::new(vec![Value::Closure(Arc::new(code_closure(
            None,
            &["y"],
            var("y"),
        )))]));
        assert_eq!(
            gate(&c, &named(&c, "twice"), &[holding]),
            Err(Gate::ArgumentType),
            "a `List` holding a `Closure` crossed under a declared `Int`"
        );
        assert_eq!(
            gate(&c, &named(&c, "twice"), &[Value::Int(21)]),
            Ok(("twice".to_string(), DEFAULT_MAX_CALLS))
        );
    }

    /// `Cell`, `Task` and `Secret` are `Type::Con`s with a name and arguments,
    /// exactly as `Option` is, so a rule that read "any nominal type is a record
    /// or a constructor" would carry all three — and the third is a credential
    /// while the first two are handles into this run's world.
    ///
    /// This is the corner a *value* test never had to think about, because no
    /// `Value::Cell` was ever admitted on its discriminant. A type test has to
    /// name them, and `crate::census::type_carries` had this hole until this
    /// change: its own header records the correction.
    #[test]
    fn a_world_handle_typed_parameter_is_refused_though_it_is_a_nominal_type() {
        let c = checked_source(
            "fn holds_cell(c: Cell<Int>) -> Int = 1\n\
             fn holds_secret(s: Secret<Int>) -> Int = 1\n\
             fn holds_fn(g: (Int) -> Int) -> Int = g(1)\n\
             fn holds_int(n: Int) -> Int = n\n",
        );
        let types = c.types();
        for name in ["holds_cell", "holds_secret", "holds_fn"] {
            let ty = &c.check.defs[&Symbol::new(name)].scheme.ty;
            let ply_core::ty::Type::Fn { params, .. } = ty else {
                panic!("{name} publishes no function type");
            };
            assert!(
                !types.carries(&params[0], None),
                "{name}'s declared parameter type {:?} is carried",
                params[0]
            );
        }
        let ply_core::ty::Type::Fn { params, .. } =
            &c.check.defs[&Symbol::new("holds_int")].scheme.ty
        else {
            panic!("holds_int publishes no function type");
        };
        assert!(
            types.carries(&params[0], None),
            "the control failed: `Int` is not carried, so the loop above says nothing"
        );
    }

    /// A record argument reaching a real backend through a real machine, which
    /// is what every gate assertion above is a proxy for.
    ///
    /// Before 2026-08-31 the machine offered a backend nothing at all here: the
    /// argument is a `Value::Record` and `crossable` refused it on its
    /// discriminant. ADR 0030 §1 measured what that cost — **3,236,823 `Record`
    /// arguments** refused on the ported Ply front end, against 190,703 calls
    /// admitted in total.
    #[test]
    fn a_record_argument_reaches_a_backend_through_the_machine() {
        let c = checked_source(
            "type Pair = { a: Int, b: Bytes }\n\
             fn first(p: Pair) -> Int = p.a\n\
             test \"t\" { assert(first({a: 7, b: b\"x\"}) == 7) }\n",
        );
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let call = callv(
            "first",
            vec![record(vec![("a", int(7)), ("b", bytes(b"x"))])],
        );
        assert_eq!(ok(machine.eval_expr_for_test(&call)), Value::Int(7));
        assert_eq!(backend.names(), vec!["first"]);
        assert_eq!(
            backend.offers()[0].args,
            vec![Value::Record(Arc::new(BTreeMap::from([
                (Symbol::new("a"), Value::Int(7)),
                (Symbol::new("b"), Value::bytes(b"x")),
            ])))],
            "the record the machine offered is not the one the call built"
        );
    }

    /// An arity mismatch is the machine's diagnostic, phrased from
    /// `closure.describe()`, and it stays ahead of the hook.
    #[test]
    fn an_arity_mismatch_is_the_machines_diagnostic_and_no_backend_sees_it() {
        let c = checked(vec![double_def()]);
        let subject = callv("double", vec![int(1), int(2)]);
        let baseline = rendered(&c.machine().eval_expr_for_test(&subject));

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(rendered(&machine.eval_expr_for_test(&subject)), baseline);
        assert!(
            backend.offers().is_empty(),
            "a call whose arity does not match was offered to a backend"
        );
        // Control: at the right arity the same call is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// One hop is `wrapper`. Four hops, a mutually recursive pair and a call
    /// reached only through a lambda are what separate a fixpoint from a single
    /// pass — and a single pass would satisfy every other test in this block.
    ///
    /// The `clean` chain is the same depth and genuinely pure. Without it this
    /// test is also passed by a gate that refuses everything with a call in it,
    /// which would close the seam rather than narrow it.
    #[test]
    fn the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
            fn_def_sig(
                "handled",
                &[("x", tcon("Int"))],
                tcon("Int"),
                handle(
                    callv("touch", vec![var("x")]),
                    vec![clause(
                        "state",
                        "get",
                        None,
                        &["n"],
                        bin(BinOp::Add, var("n"), int(1)),
                    )],
                ),
            ),
            fn_def_sig(
                "hop1",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("handled", vec![var("x")]),
            ),
            fn_def_sig(
                "hop2",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("hop1", vec![var("x")]),
            ),
            fn_def_sig(
                "hop3",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("hop2", vec![var("x")]),
            ),
            fn_def_sig(
                "hop4",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("hop3", vec![var("x")]),
            ),
            // Only `ping` can reach the handler; `pong` reaches it through the
            // recursion, which a propagation that stopped at a cycle would miss.
            fn_def_sig(
                "ping",
                &[("x", tcon("Int"))],
                tcon("Int"),
                if_(
                    bin(BinOp::Lt, var("x"), int(1)),
                    callv("handled", vec![var("x")]),
                    callv("pong", vec![bin(BinOp::Sub, var("x"), int(1))]),
                ),
            ),
            fn_def_sig(
                "pong",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("ping", vec![var("x")]),
            ),
            fn_def_sig(
                "via_lambda",
                &[("x", tcon("Int"))],
                tcon("Int"),
                block(
                    vec![letv("f", lam(&["y"], callv("handled", vec![var("y")])))],
                    Some(call(var("f"), vec![var("x")])),
                ),
            ),
            fn_def_sig(
                "clean1",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(1)),
            ),
            fn_def_sig(
                "clean2",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("clean1", vec![var("x")]),
            ),
            fn_def_sig(
                "clean3",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("clean2", vec![var("x")]),
            ),
            fn_def_sig(
                "clean4",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("clean3", vec![var("x")]),
            ),
        ]);

        let refused = ["hop1", "hop2", "hop3", "hop4", "ping", "pong", "via_lambda"];
        for name in refused {
            let info = &c.check.defs[&Symbol::new(name)];
            assert!(
                info.footprint.is_empty() && info.performed.is_empty(),
                "the fixture is wrong: `{name}` publishes a row, so the row gate refuses it and \
                 this says nothing about the effects gate"
            );
            let subject = code_closure(Some(name), &["x"], var("x"));
            assert_eq!(
                gate(&c, &subject, &[Value::Int(1)]),
                Err(Gate::InternalEffects),
                "`{name}` was admitted, so the propagation stopped short of it"
            );
        }
        for name in ["clean1", "clean2", "clean3", "clean4"] {
            let subject = code_closure(Some(name), &["x"], var("x"));
            assert_eq!(
                gate(&c, &subject, &[Value::Int(1)]),
                Ok((name.to_string(), DEFAULT_MAX_CALLS)),
                "`{name}` is pure at every hop and was refused anyway"
            );
        }
    }

    // ---------------------------------------------------------------------
    // The ANSWER test, 2026-08-31. Six tests, and the first three are the
    // widening while the last three are the thing the widening changes: an
    // entered call is no longer a leaf, so every gate has to hold over a
    // SUBTREE and that is a different claim.
    // ---------------------------------------------------------------------

    /// A definition answering a record is entered and its answer is used.
    ///
    /// This is ADR 0030 §1's finding closed. `lex(Bytes) -> Scan` cleared every
    /// gate on the ported front end and was declined 13 times — once per file —
    /// because `Machine::compiled_answer` tested the answer's discriminant with
    /// [`crossable`] and a `Scan` is a `Value::Record`. The registry had no body
    /// for it for the same reason. Both ends now read the declared return type.
    ///
    /// The control is the second half: the same backend answering the same
    /// record for a definition declared `-> Int` is refused, so what admits the
    /// first is the declaration and not the kind.
    #[test]
    fn a_record_answer_crosses_back_under_its_declared_return_type() {
        let c = checked_source(
            "type Scan = { at: Int, tok: Bytes }\n\
             fn scan(i: Int) -> Scan = { at: i, tok: b\"x\" }\n\
             fn count(i: Int) -> Int = i\n",
        );
        let answer = record_value(&[("at", Value::Int(7)), ("tok", Value::bytes(b"x"))]);

        let backend = Double::answering(&c.program, "scan", answer.clone());
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("scan", vec![int(1)]))),
            answer,
            "a record answer was refused under a declared return type that carries it"
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(machine.compiled_refusals(), 0);
        drop(machine);

        let backend = Double::answering(&c.program, "count", answer.clone());
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("count", vec![int(3)]))),
            Value::Int(3),
            "a record answer was believed for a definition declared `-> Int`"
        );
        assert_eq!(machine.compiled_counts(), (0, 1));
        assert_eq!(machine.compiled_refusals(), 1);
    }

    /// A carried declared return type licenses one `Value` kind, not any.
    ///
    /// The tripwire on [`Denotes`] in the answer direction, and the mirror of
    /// `a_value_whose_kind_is_not_its_declared_types_is_refused`. Delete the
    /// kind comparison from `CarriedTypes::answer_crosses` and a definition
    /// declared `-> Scan` may answer a `Value::List` holding a `Closure`, which
    /// is a route back into this run's world through a gate that reads a type.
    ///
    /// The `Int` control below is not decoration: an `Int` answer for a
    /// definition declared `-> Scan` is *still admitted*, by the childless
    /// clause, because that is exactly today's rule and two of the eight wrong
    /// backends are built on it. Widening the answer test must not quietly
    /// narrow it.
    #[test]
    fn an_answer_whose_kind_is_not_its_declared_returns_is_refused_unless_it_is_childless() {
        let c = checked_source(
            "type Scan = { at: Int, tok: Bytes }\n\
             fn scan(i: Int) -> Scan = { at: i, tok: b\"x\" }\n",
        );
        let holding = Value::List(Arc::new(vec![Value::Closure(Arc::new(code_closure(
            None,
            &["y"],
            var("y"),
        )))]));
        let types = c.types();
        let scan = Symbol::new("scan");
        assert!(
            !types.answer_crosses(&scan, &holding),
            "a `List` holding a `Closure` came back under a declared `-> Scan`"
        );
        assert!(
            types.answer_crosses(
                &scan,
                &record_value(&[("at", Value::Int(0)), ("tok", Value::bytes(b""))])
            ),
            "the record the declaration denotes was refused, so the widening bought nothing"
        );
        assert!(
            types.answer_crosses(&scan, &Value::Int(0)),
            "the childless clause was lost: `Mutation::WrongType` and `Mutation::Answers` both \
             answer an `Int` for a definition that returns something else, and refusing it here \
             would police a wrong answer with a kind test"
        );
    }

    /// A declared return type that can hold code is not answered for at all.
    ///
    /// The return half of `a_closure_bearing_record_is_refused_on_its_declared_type`,
    /// and it is enforced twice over: `CarriedTypes::signature_carried` keeps the
    /// definition out of a backend's registry, and `answer_crosses` refuses the
    /// record if a backend answers one anyway. The second is what this asserts,
    /// because the first is a backend's choice and this seam does not get to
    /// depend on one.
    #[test]
    fn a_closure_bearing_record_return_is_refused_however_ordinary_the_record_looks() {
        let c = checked_source(
            "type Box = { run: (Int) -> Int, tag: Int }\n\
             type Plain = { tag: Int }\n\
             fn make_box(n: Int) -> Box = { run: |y: Int| y, tag: n }\n\
             fn make_plain(n: Int) -> Plain = { tag: n }\n",
        );
        let types = c.types();
        // A record with no closure in it, under a declared type that can hold
        // one. A value walk would carry this; the question asked is about the
        // type, so it is refused.
        let innocent = record_value(&[("tag", Value::Int(1))]);
        assert!(
            !types.answer_crosses(&Symbol::new("make_box"), &innocent),
            "a record came back under a declared return type that can hold a `Closure`"
        );
        assert!(
            types.answer_crosses(&Symbol::new("make_plain"), &innocent),
            "the control failed: a record of `Int` was refused too"
        );
        assert!(
            !types.signature_carried(&Symbol::new("make_box")),
            "a backend's registry would hold a definition the machine will not hear from"
        );
        assert!(types.signature_carried(&Symbol::new("make_plain")));
    }

    /// Entering a call now hides its whole subtree, and the effects gate has to
    /// hold over the subtree rather than over the entry.
    ///
    /// Before the answer test read declared types, a definition returning a
    /// record could not be entered, so an entered body was a leaf-ish thing over
    /// scalars. `items.parse` is now entered **once per file** and every call it
    /// makes runs inside that entry, where the machine sees nothing: no
    /// `perform` is recorded, no `Access` reaches a scheduler, no cell touch is
    /// counted. So "this definition performs nothing" has to mean "nothing this
    /// definition can reach performs anything".
    ///
    /// It does, and it did before this change — `DefInfo::internally_effectful`
    /// is a fixpoint over the call graph and
    /// `the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop`
    /// holds it at four hops, through a mutually recursive pair and through a
    /// lambda. What is new is that the *consequence* of that fixpoint being
    /// wrong grew from one call to a program. This test is the same claim asked
    /// the way the widening makes it matter: the machine offers the outer
    /// definition, and if it were entered the inner `perform` would be lost.
    #[test]
    fn an_entered_subtree_is_refused_for_an_effect_two_hops_down_that_it_would_hide() {
        let c = self_handled();
        // `wrapper` calls `handled`, which discharges `state.get` under its own
        // handler. Running it performs; its published row says it does not.
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
            Value::Int(2)
        );
        assert_eq!(
            machine.trace().performs(),
            1,
            "the fixture is wrong: nothing was performed, so hiding the subtree would cost \
             nothing"
        );
        drop(machine);

        // And the machine offers it to nobody, so the subtree is never hidden.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
            Value::Int(2)
        );
        assert!(
            !backend.names().iter().any(|n| n == "wrapper"),
            "a definition whose subtree performs was offered: {:?}",
            backend.names()
        );
        assert_eq!(
            machine.trace().performs(),
            1,
            "the atoms the interpreter records were lost"
        );
    }

    /// The same claim for the deterministic scheduler, and this one is a gate
    /// away from where a reader looks for it.
    ///
    /// [`Gate::SimulateRegion`] reads the **machine's** state — it refuses a
    /// call made *inside* a live `simulate` region — and says nothing about a
    /// definition that opens one. For a definition that opens its own, the row
    /// gate is what refuses it (`a_definition_that_opens_its_own_simulate_region_is_never_offered`,
    /// which reads `sim.read` out of the fixture's footprint first). Over a
    /// subtree the question is the two-hop one: does a definition that merely
    /// *calls* one that opens a `simulate` region get entered, hiding every
    /// `Access` the search depends on?
    ///
    /// It does not, and the mechanism is the row rather than anything in this
    /// module: `sim.read` is an escaping atom, so it propagates to every caller
    /// that does not discharge it. The footprint is read out of the fixture
    /// before the run, so a change that made `simulate` publish nothing turns
    /// this red rather than making it vacuous.
    #[test]
    fn a_definition_that_calls_one_that_opens_a_simulate_region_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def_sig(
                "searched",
                &[("n", tcon("Int"))],
                tcon("Int"),
                ex(ExprKind::Simulate {
                    body: Box::new(bin(BinOp::Add, var("n"), int(1))),
                }),
            ),
            fn_def_sig(
                "outer",
                &[("n", tcon("Int"))],
                tcon("Int"),
                callv("searched", vec![var("n")]),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("outer")].footprint.is_empty(),
            "the fixture is wrong: a definition two hops from a `simulate` published an empty \
             row, so the row gate would clear it and the subtree would be hidden"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&bin(
                BinOp::Add,
                callv("outer", vec![int(1)]),
                callv("double", vec![int(0)]),
            ))),
            Value::Int(2)
        );
        assert_eq!(
            backend.names(),
            vec!["double"],
            "a definition that reaches a `simulate` region two hops down was offered"
        );
    }

    /// The budget bounds the whole entered subtree, not the entry.
    ///
    /// `budget` is the machine's remaining nested calls and is handed over once.
    /// While an entered body was a leaf that mattered little; now one entry can
    /// swallow a recursion of any depth, and if the bound were charged per
    /// *entry* rather than per *nested call inside it* a native run would answer
    /// where the machine raises.
    ///
    /// Run against the real [`crate::backend::Reference`] rather than a double,
    /// because the claim is about how a backend spends the number it is handed —
    /// a double that ignored `budget` would pass any assertion a double could
    /// make. The two arms must produce the *same diagnostic*, which is what
    /// `limit.rs` exists to keep true of both engines.
    #[test]
    fn an_entered_subtree_is_bounded_by_the_budget_it_was_handed_and_not_by_its_entry() {
        let c = checked_source(
            "fn down(n: Int) -> Int = if n <= 0 { 0 } else { down(n - 1) + 1 }\n\
             fn top(n: Int) -> Int = down(n)\n",
        );
        let call = callv("top", vec![int(400)]);

        let bare = Machine::new(&c.program, &c.resolved, &c.check).with_max_calls(50);
        let mut bare = bare;
        let without = bare.eval_expr_for_test(&call);
        assert!(
            rendered(&without).contains("recursion limit of 50 nested calls exceeded"),
            "the fixture is wrong: the machine did not reach its own bound: {}",
            rendered(&without)
        );
        drop(bare);

        let fragment = crate::backend::Fragment::over(&c.program, &c.resolved, &c.check);
        assert!(
            fragment.holds(&Symbol::new("top")) && fragment.holds(&Symbol::new("down")),
            "the fixture is wrong: the backend has no body for the recursion under test"
        );
        let mut backed = Machine::new(&c.program, &c.resolved, &c.check).with_max_calls(50);
        backed.set_compiled(fragment.attach(&crate::backend::Spec::honest()));
        let with = backed.eval_expr_for_test(&call);
        assert_eq!(
            rendered(&with),
            rendered(&without),
            "an entered subtree outran the machine's bound and answered where the machine raises"
        );
        assert_eq!(
            backed.compiled_counts().0,
            0,
            "the backend answered a call whose subtree cannot fit the budget"
        );

        // The control: the same program under a budget the recursion fits, so
        // the refusal above is the bound's and not the fixture's.
        let mut ok_run = Machine::new(&c.program, &c.resolved, &c.check);
        ok_run.set_compiled(fragment.attach(&crate::backend::Spec::honest()));
        assert_eq!(
            ok(ok_run.eval_expr_for_test(&call)),
            Value::Int(400),
            "the recursion does not fit the default bound either, so nothing above is about the \
             budget"
        );
        assert!(ok_run.compiled_counts().0 > 0);
    }

    /// What a collapse actually is, at unit scale: the machine offers the entry
    /// and never sees anything under it.
    ///
    /// The measurement this stands in for is on the ported front end
    /// (`spikes/ply-parser`, 13 files, 333,851 bytes): entries fall from
    /// **306,931 to 26** while the share of body calls a backend can answer
    /// rises from **17.03% to 84.01%**, because `items.parse` is entered once
    /// per file and its ~2.4 million inner calls run inside that entry. A
    /// falling entry count is the win and not a regression — it is PR #30's
    /// shape, where a fragment widened until one crossing swallowed a whole
    /// search and crossings went 721 to 1.
    ///
    /// Asserted here rather than only measured there, because a number in a
    /// report is not a tripwire.
    #[test]
    fn an_entered_call_hides_its_subtree_and_the_machine_offers_none_of_it() {
        let c = checked_source(
            "type Scan = { at: Int }\n\
             fn leaf(i: Int) -> Int = i + 1\n\
             fn middle(i: Int) -> Int = leaf(i) + leaf(i)\n\
             fn outer(i: Int) -> Scan = { at: middle(i) }\n",
        );
        // Declining, so the machine evaluates everything itself: this is the
        // set of calls the seam is offered when nothing is entered.
        let declining = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(declining.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("outer", vec![int(1)]))),
            record_value(&[("at", Value::Int(4))])
        );
        assert_eq!(
            declining.names(),
            vec!["outer", "middle", "leaf", "leaf"],
            "the fixture is wrong: the subtree this entry would hide is not offered without it"
        );
        drop(machine);

        // Answering, and the same expression offers exactly one call.
        let answering =
            Double::answering(&c.program, "outer", record_value(&[("at", Value::Int(4))]));
        let mut machine = c.machine();
        machine.set_compiled(answering.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("outer", vec![int(1)]))),
            record_value(&[("at", Value::Int(4))])
        );
        assert_eq!(
            answering.names(),
            vec!["outer"],
            "the entry did not swallow its subtree"
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// A record `Value` from a list of fields, which no helper in
    /// [`crate::build`] answers because that module builds `Expr`s.
    fn record_value(fields: &[(&str, Value)]) -> Value {
        let mut map = BTreeMap::new();
        for (name, value) in fields {
            map.insert(Symbol::new(name), value.clone());
        }
        Value::Record(Arc::new(map))
    }

    /// A `Float` in flight is what `crossable` refuses; this is the same
    /// statement about the answer rather than the argument, and it is separate
    /// because the two are separate gates.
    fn float(f: f64) -> Expr {
        ex(ExprKind::Lit(ply_syntax::ast::Lit::Float(f)))
    }
}
