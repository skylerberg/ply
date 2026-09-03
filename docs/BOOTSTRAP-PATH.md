# The bootstrap path — what still stands between Ply and a compiler written in Ply

A plan, not a decision. ADR 0020 decided against self-hosting the front end on
today's interpreter and that decision stands; ADR 0021 records why the goal
exists anyway. This file is the entry for whoever continues toward it: what is
no longer a blocker, what is, the order to take the rest in, and the measurement
each step is gated on. Every claim below names the record that holds it. Figures
stay in those records and in the commands that produce them; none is restated
here, per `CONTRIBUTING.md` §"Writing a claim down".

The goal it serves is ADR 0021's: a front end whose cost is O(change) rather than
O(project), fast enough to sit inside the sub-second verification loop the
project exists to make fast. "Reasonably close to Rust" means that loop, not a
benchmark.

**That is one of two goals this file carried as one, and ADR 0037 splits them.**
The loop is the goal. A dependency line drawn where Rust's is — a C compiler and
libc — is a second one, which the loop does not require and which
§"Where this ends" holds as a direction. The split changes what is ordered
first: the loop's own gaps are not steps on the path below, they are what the
path is for, and §"The loop, which is what the path is for" carries them.

## Where it stands

**No longer a blocker, and each is checked by something in the tree:**

- **Expressiveness.** A lexer and a recursive-descent parser written in Ply agree
  with `crates/ply-syntax` on the reference corpus, tree and diagnostics, byte
  for byte; the inputs they disagree on are the ones written in syntax that
  postdates the port (`spikes/ply-parser/GAPS.md` §11R). CI runs the differential
  on every push (`.github/workflows/ci.yml`, job `spikes/ply-parser`), every
  phase of it under the compiled backend through `ply run --backend`, which is
  what keeps that job at the build's length rather than the interpreter's.
- **The call ceiling.** `iterate` gives a parser the reference's own shape —
  loops for sequences, recursion only for grammar nesting — at depth one (ADR
  0022). A raisable ceiling is refused there, with the reason.
- **The positional cost rule.** ADR 0034 is landed entire: a last use moves its
  value out of its slot (`position_invariance_g1`), a copy is bounded whatever a
  list's length and a `[x, ..rest]` pattern shares rather than copies
  (`accumulator_shape`, `list_pattern_rest`), a record update writes into a
  dying base (`record_update_reuse`), and `reuse fn` turns the cost report into
  an obligation `ply check` enforces
  (`check::tests::a_reuse_fn_is_refused_only_for_a_copy_its_own_body_causes`).
  Position decides nothing; a copy means a genuine second owner.
- **Four of the parser spike's ranked gaps.** A list index (ADR 0027), record
  update (ADR 0023), `?` (ADR 0028), and bit operators with a filesystem effect
  and a hash written in Ply (ADR 0033).

**The blocker is throughput, and it is the only one.** Lexing plus parsing in
Ply costs more than an order of magnitude what the whole six-phase Rust front
end costs on identical input in one sitting, and four of the six phases are
unwritten. `spikes/ply-parser/GAPS.md` §13 and §13R hold the series and
`spikes/ply-parser/measure-multiplier.sh` re-takes it. ADR 0021 §"The critical
path" locates the cost: interpreter dispatch dominates builtin bodies by roughly
twenty to one, so compilation removes the right half — and a fifth of executed
work is map, record and list machinery that no amount of compiling reaches.

## Why the interpreter path cannot close it

ADR 0030 measured compiled code on the front end itself and the finding is the
shape of the whole problem. Entering compiled code at the leaves pays the
boundary on every entry, and at a front end's entry rate **an infinitely fast
backend at that granularity cannot win**. The entry has to move to the root of a
subtree, and what keeps it out is the code generator: it refuses a lambda because
there is no closure representation at all, refuses the builtins that call user
code, and refuses an *enclosing* function when a callee is uncompiled instead of
emitting a trampoline — so one lambda under a root refuses the root. ADR 0018
found the earlier form of the same constraint: the fragment covered most of a
kernel's work and bought nothing until the interpreter could call compiled code
(R5).

The seam is wider than it was, and what it still refuses is specific.
`crates/ply-eval/src/compiled.rs` carries a value whose declared type is built
from `Int`, `Bool`, `Bytes`, `String` and `Unit` — lists, maps, records and
declared types of those included, since ADR 0030's widening — and a backend
answers one value with no arena, handler stack or route back in. What cannot
cross: a function, a type variable, `Float`, `Decimal`, and the cell, task and
secret kinds (`CarriedTypes::blocker`). Separately, the *registry* of what the
machine may enter is every compiled function whose signature is carried
(step 3's row decided it); `PLY_CODEGEN_REGISTER=narrow` keeps ADR 0030's
scalar-signature arm as the measurement knob.

## The path, in order

Each step names its gate. Take them in this order because each later step's
measurement is confounded until the earlier one has moved.

1. **The front-end row exists, and it is the ordering.** ADR 0026 named the
   bootstrap front end as a workload class with no row; ADR 0030 then took it —
   the parser spike's modules parsing the example files as byte literals,
   through `ply test --backend`, against a null control, arms rotated. Every
   backend arm was slower than no backend, by nearly ten times the control,
   because the entries are leaf islands and a boundary crossing costs more than
   the machine's own dispatch. So the first step is not a measurement but the
   two that follow, and the row is re-taken after each root lands, with ADR
   0030's protocol and its bar: beat the unbacked arm by more than the null
   control, on an idle machine, pre-registered (`CONTRIBUTING.md` §"Gate on an
   idle machine").
2. **The code generator's roots — landed, except the gate.** String
   concatenation and nested patterns were already lowered when this was
   re-censused; the callback family was the whole of the rest, and it is
   lowered as one piece: a lambda is a compiled function taking its captures as
   leading arguments, built into a `ClosureKind::Native` value that lives only
   inside the entry that made it (the seam carries no function); a named
   function, a constructor and a builtin used as values are closures over
   nothing; a call through a local or through an expression is `rt_call`;
   `map`, `filter`, `fold`, `map_fold` and `iterate` are loops in the runtime
   that call the callback back; and the bitwise operators are lowered with the interpreter's
   shift-count refusal. A trampoline for an uncompiled callee was not needed:
   the cascade was the callbacks' blast radius, and the parser spike's census
   now refuses only effects, `Decimal` and `Float` literals, a `handle`,
   `secret_of_string` and a refutable `let` pattern
   (`parser_census::the_census_over_the_parser_spike` pins that no lambda,
   callback, value-call or `let` over a record is refused — the last was what
   kept the whole checker out of the unit, through two functions that bind a
   tuple). With every carried
   signature registered, the examples and the spike's own tests agree with the
   backend attached under `--audit-backend`. What remains is the gate, and it
   cannot move until step 3: the narrow registry enters the same leaves it
   entered before.
3. **The registry is wide — landed; the seam's remaining kinds are not.**
   Every function the fragment compiles is registered and the seam admits each
   call by its carried types; `PLY_CODEGEN_REGISTER=narrow` keeps ADR 0030's
   scalar-signature arm. Decided on ADR 0030's row re-taken under
   `benches/front-end/PRE-REGISTERED.md`: with the wide registry the whole
   parse of each example file runs inside one native entry and beats no backend
   by ten times the null control's resolution, while the narrow registry loses
   as before. Three series (`observation-*.txt`) say the same, and all three
   are recorded as observations rather than figures: the protocol's after-load
   gate reads the one-minute average, and a series of ten-worker test runs
   lifts that past 4 on an otherwise idle machine before it ends — the third
   series started at 3.2 and finished at 4.9 with nothing else running. The
   gate as pre-registered measures the series' own workers, so what stands as
   the load evidence is the load *before* and the null control's resolution,
   which is tight in every series. The backend now refuses an
   answer holding a closure, cell, task or secret itself, so no registry width
   leaks one. `String` and `Unit` now cross, with the wrong-backend mutations
   that police them and the differential corpus asserting the widening is
   reached. Still to admit: a type variable (the spike's generic `comma_list`)
   — listed, not built: with every phase entered at its root (step 7) a
   generic leaf is reached inside compiled code and never through the seam, so
   the kind buys nothing for this workload until a program's *root* is generic.
   A value of unknown type can only be admitted by walking it for a handle,
   which is the O(value) cost per call the type gate exists to avoid, so what
   it needs is the instantiation at the call site — a design step with its own
   row rather than a widening of the table. What the row
   also says: the compiled parse is a fifth faster than the interpreted one,
   not five times, so the next lever is what compiled code does with values
   (step 4), not what it lowers.
4. **What compiled code does with values — landed, at parity.**
   `front_end_alloc_sites` (in `ply-codegen-tests`) attributes one parse's
   allocations by site under both engines, the way `w6_alloc_sites` attributes
   a request's. It first read the compiled parse at several times the
   interpreted one, because the arena handed every value to a helper as a
   shared handle and none of the last-use ownership ADR 0034 gave the machine
   reached compiled code. Now it does: a borrowed local is duplicated only
   where it is consumed, a helper takes every argument, a field read at its
   base's last use or its own moves out in place, an exact-shape update writes
   in place, and a pattern binds by moving out of a value nothing else holds.
   Two more things the interpreter did and compiled code did not were found
   by the same census: pure nullary definitions are memoised (`rt_constant`,
   with `ply_eval::memo`'s rule), and constructor payloads no longer drain the
   argument pool. The census pins the compiled arm within one percent of the
   interpreter's allocations; what is left is mostly the arena's teardown.
   Two traps, since each would otherwise be re-found. The counts are conserved
   whichever way the pool is fed — a buffer that becomes a payload was
   allocated by some earlier miss — so a warm-up parse hides nothing and a
   pool change never moves the total. And a caller that passes a record and
   then reads a field of it in a later argument, as the spike's state setters
   do, hands the callee a shared record whose update copies under both
   engines; argument order cannot be changed under effects, so that is the
   spike's to hoist into a `let`. What the row must say next is time, and the
   census no longer says where it goes.
5. **The language tax the spike priced.** In `spikes/ply-parser/GAPS.md`'s
   order: tuples (§3 — **landed** as sugar over positional records, `(a, b)` is
   `{_0: a, _1: b}` in a type, a value and a pattern), `const` (§5 — the value of a nullary pure definition is
   already memoised at run time by `ply-eval::memo`, so what remains is the
   spelling), `?` inside lambdas (§2, `E0118` — **landed**: a lambda may write
   `-> T` before a block body and `?` reads it; an `iterate` step answers `Iter`
   and can never carry one), `?` as a `let`'s value inside a branch (`E0119` —
   **stays**: lifting the conditional whole with its success wrapped would keep
   what runs unchanged, but it is a second expansion shape, and ADR 0028 §4
   chose refusal over hoisting for exactly that reason; re-take that decision
   before building it, and the spike converted every guard without it),
   keywords reserved in the field namespace (§6 — **landed** on both sides: a
   keyword names a field wherever a field is named, and only the punned forms,
   which bind a variable too, are refused), an expression-position `unreachable` (§8 — the
   expression exists: `panic` is typed `String -> a`, and what the spike wanted
   beyond it was a placeholder *visible in the differential*, which is the
   spike's choice rather than a gap), and §9's small pieces (`min` and `max`
   **landed** as integer builtins; `saturating_sub` is `max(a - b, 0)`; the rest
   of §9 is spelling the language does not owe). Float construction is ADR 0020's
   one absolute hole. Each is an ordinary language
   change under ADR 0001's rule that no existing hash may move, and each moves
   `docs/GUIDE.md` in the same change.
6. **The other four phases, behind the differential.** Resolve, inference,
   effect inference and hashing, each ported the way the parser was: a reference
   dumper on the Rust side, a corpus, and mutations that prove the comparison
   can go red (`spikes/ply-parser/arm-*.sh`). **Resolve is ported**
   (`spikes/ply-parser/resolve.ply`, `GAPS.md` §15): the tables, the load
   order, the diagnostics and the whole defaults pass agree with the reference
   over the standard library, every example with it, every program the
   reference's own tests build and a hand-written bundle of the error paths;
   `arm-resolve.sh` arms it. The one tax it met is worth knowing before the
   next port: record update needs the base's field list declared in the same
   file (ADR 0029's parse-time expansion), so a rewrite of an imported AST node
   spells every field. **The three parse-time rewrites are ported too**
   (`rewrite.ply`, `GAPS.md` §16): the checker reads the expanded tree, so the
   effect-set, record-update and try-operator passes that §11R.D left in Rust
   now run in Ply and agree with `parse_recovering` over everything the parser
   differential reads; `arm-rewrite.sh` arms them. **Inference with rows is
   ported** (`tycore.ply`, `infer.ply`, `GAPS.md` §17): the checker's published
   output — every scheme, footprint, constraint set, test, law, effect and
   constructor, or its diagnostics — agrees with `check_program` over the
   standard library, every example, a hand-written bundle and every one of the
   reference checker's own test inputs; `arm-infer.sh` arms it. **The deriver
   is ported** (`derive.ply`, `GAPS.md` §18): the source every derivation
   generates agrees byte for byte with `ply-derive` over a hand-written bundle,
   every example and the standard library, the checker expands before it
   resolves, and `arm-derive.sh` arms it. **The restored path is ported**:
   `check_program_with` publishes a known group from its interfaces as the
   driver's cache hands them in, and a program checked from what its own
   first check published publishes the same thing on both sides, over the
   standard library and every bundle. **Hashing is ported** (`hash.ply`,
   `GAPS.md` §19): every hash `ply-hash` publishes — definitions, declarations,
   tests, laws, own-form and spec keys — and the reference graph beside them
   agree over the standard library, every example, every bundle and the
   hasher's own mined inputs, with BLAKE3 from `std.hash` over the same bytes;
   `arm-hash.sh` arms it. The one surface it needed, `bits_of_float`, landed
   first. Every phase step 6 names is now behind a differential. The syntax
   the parser spike predated went in before any of them, so the parser's own
   differential was green before anything was built on it: the bit operators
   (with the shift join and the lambda-parameter pipe guard) and keyword
   fields are in, the corpus is re-mined from the reference's tests, and the
   agreement tests pass over all of it. Re-mine whenever the reference grows
   syntax; the harness's diagnostics pin moves with the corpus and says so.
7. **The driver.** Incremental caching, the content-addressed store and the
   gates are Rust, and ADR 0020 notes a self-hosted front end would be cached by
   machinery it does not own. Whether the front end lives behind the Rust driver
   or the driver is ported too is a decision for when phases exist to drive.
   They exist now, and what step 6 established bears on the decision: the
   ported phases publish exactly what the driver caches on — the restored
   `Known` interface is checked from on both sides, and every hash the store is
   keyed by is reproduced bit for bit — so a Rust driver can drive a Ply front
   end through the interfaces it already has, with no cache format moving. What
   was not established was the cost, and the row that decides is the whole
   front end — parse, expansion, resolve, check, hash — over the standard
   library and an example, under the backend and without it, with step 3's
   protocol (`benches/front-end-whole/PRE-REGISTERED.md`, its observation
   beside it). **The row is taken, and the decision is: the driver stays
   Rust.** Under the interpreter hashing is the largest phase and checking the
   next, together most of the whole; parsing is a distant third and the
   resolver, with derive expansion, is small. Under the backend every phase
   falls by several times and each row's root call is entered whole, nothing
   declined — the checker included, once the fragment lowered the `let` over
   a tuple that had kept its roots out of the unit (the first series saw the
   checker barely move, and the pre-registration records why that confirmed
   its prediction for the wrong reason). The whole front end is still well
   over an order of magnitude from the Rust front end over the same files,
   which is not the small factor the rule asked for, so the driver is not the
   lever. What is: the runtime's cost per value on the callback path. A
   profile of the compiled check row
   (`benches/front-end-whole/profile-check-wide.txt`) puts its time under the
   runtime's callback loops — `fold`, `map`, `iterate` calling the compiled
   step back — and the larger part of that in the value traffic each step
   causes: dropping and draining the values a step gives up, allocating,
   the argument pool, cloning, field reads; the compiled frames themselves
   are the minority. So the lever is that path — what a callback step pays
   to receive, update and release a carried state — and the hasher is the
   same lever seen on an integer kernel, not a separate one: step 9 is the
   decision that moves it. The series is an observation and not a figure by
   ADR 0030's gate: quiet before, the load lifted past four after by the
   series' own four workers, as the pre-registration said it would. **Re-taken
   with ADR 0035's sequence landed** (`observation-3.txt`, the same gate
   reading): every phase under the backend fell by several times again —
   hashing by an order of magnitude, parsing and checking by several — the
   whole front end under the backend is a small fraction of what it was, and
   the interpreted row did not move, as it should not have. The decision
   stands: the driver stays Rust.
8. **Repair the oracles as they are needed.** The lexer spike's harness did not
   compile past the tokens ADR 0028 and ADR 0033 added, and its lexer knew
   neither them nor hex literals; both are repaired, the differential is green
   over the corpus and the standard library, and CI runs it (`lexer-spike`).
   `CONTRIBUTING.md` item 18 is closed: the codegen spike's agreement corpus
   was red because its boundary handed the leaf kinds the machine's seam
   admits to bodies compiled over `Int` and `Bool`, and green under `cargo
   test` because no test ran the command; the boundary checks the kind and the
   suite runs the command. A bootstrap is verified with exactly these
   instruments, and a green result over an instrument that runs nothing is the
   defect class this project names as its most expensive.
9. **The compiled value model.** ADR 0035 decides it: the representation
   compiled code runs on is the interpreter's — name-keyed records searched on
   every read, atomic counts, a radix trie, an arena handle for every argument
   — and no builtin carved out for a hot function changes that, so ADR 0033's
   hash builtin is retired and the hash is kept as a kernel instead. The model
   is a second one for compiled code only, with the interpreter's value kept
   whole as the oracle and the seam converting at an entry's root: layouts
   fixed from the checker's types, scalars unboxed wherever the type is known,
   reference counts without atomics, and reuse where a value is unique, which
   is the layout ADR 0034 asked for and did not get. **The gate is two kernels
   against Rust** — BLAKE3 in Ply against a scalar transliteration, and a
   threaded state record against a struct updated in place — within a factor
   registered in `benches/value-model/PRE-REGISTERED.md` before either exists,
   with a baseline taken on today's fragment first; the front-end row re-taken
   on the model is the outcome measure. Its sequence is in the record, and the
   first five steps are landed: the kernels and their baseline; direct and
   typed calls; the words themselves — records, constructors, lists, maps
   and closures as counted objects over a bump allocator, fields read at the
   offsets the checker's types fix, the memo copied out; the drops, with
   every binding released at its scope's end and every tail owned; and the
   inlining, with small callees inlined before lowering, field-only records
   split into their fields and scalar fields read into registers. The series
   after them (`benches/value-model/after-inline.txt`) reads both kernels
   still over the bar — the integer one much closer than it was, the record
   one a few times Rust — and the front end under the backend at roughly a
   fifth of its cost before the record. Strings and bytes are native since,
   and the list is a trie with typed leaves after ADR 0034's representation
   gate refused the array (`benches/value-model/after-strings-and-lists.txt`
   is the series after both), and the seam's census is in the run's report.
   **The sequence is complete and the gate is not met**
   (`benches/value-model/retake.txt`): both kernels are over the bar, the
   integer one by an order of magnitude and the record one by a little, and
   ADR 0035's own rule says the model as designed is refuted. What it names
   to revisit — builtins and callbacks that stay calls, a map copied per
   insert, an update that does not reuse the cell it releases — is where the
   next record starts, and the front-end row re-taken on the same binary
   (step 7) is the outcome it is read against. ADR 0036 is that record's
   first pass: the update's copy path by offset, the builtins the checker can
   type as loads, the callbacks as loops in the body, memory reused within an
   entry, the seam's memo, the map as a tree, tests as roots, and a dying
   record as the next one's memory, a lookup a match unwraps answering the
   value, the hottest builtins as direct calls over values made once, a
   literal step as the loop's own body, the round's rotate as one
   instruction, and a parameter a body only reads borrowed for the call; its
   series are `benches/value-model/after-borrows.txt` and
   `benches/front-end-whole/observation-8.txt`. **The record kernel is inside
   the bar and the integer kernel is not**, and what keeps it out is now
   measured rather than listed: `benches/value-model/k1-where.sh` prices the
   checked adds at nothing, the masks at a few percent and the records at a
   fifth, and finds a round that spills four hundred times because every word
   is a sixty-four-bit tagged value and thirty-two of them are live at once.
   Ply has one integer type, so nothing in the source or the checker can say a
   value is thirty-two bits; ADR 0036 carries what that means.
10. **Emit C, for release; inside the loop it is priced and not chosen.** Where
    the path ends, decided as a direction and gated on step 9: the eventual host
    is a C compiler and libc, with the compiler and its runtime written in Ply,
    which is the line Rust itself holds above LLVM and libc. It is a direction
    and not a step to start because emitting C onto today's representation
    would move a slow model to a different host, and step 9 is what decides
    whether the model competes. Whether the loop's tier is the same C over
    smaller units or a second code generator is ADR 0037's question, and
    `benches/c-floor/` prices what C charges: constants per changed definition
    and per run, not the exponent that question was first argued from.
    §"Where this ends" below is the order once step 9 clears.

## The loop, which is what the path is for

ADR 0037 records the split; this is what it means for the order of work. **The
loop's gaps are not steps on the path above.** They are not gated on the
bootstrap, they pay off before any of it lands, and the path is worth ordering
only if the loop it serves is the thing ADR 0021 claims.

What the interpreter's loop has. The front end is cached by content —
`DefHash -> interface`, `crates/ply-store/src/frontend.rs` — and a test is
selected against the definition set it last passed under, which
`ply_store::PassRecord` holds as the test's own hash together with every
function and declaration in its closure, so a test re-runs only when something
it reaches moved. Selecting nothing after a rename is an invariant the suite
asserts rather than a heuristic:
`crates/ply-cli/tests/suite/cli.rs renaming_a_definition_re_runs_nothing`. And
it is measured: `ply-corpus bench` applies a rename, a leaf edit and a hub edit
and times nine phases after each, and `ply-corpus sweep` takes that at each of
several sizes.

What the compiled loop does not have, in the order to take them:

- **Both caches, bypassed for one cache's reason — landed.** `ply test
  --backend` used to open a scratch store, so the front end loaded whole *and*
  every test ran, though the stated reason covered only the second. A result now
  names the engine that earned it (`ply_test::Engine`), so a backed run selects
  against what backed runs proved and neither engine reads the other's; the
  front-end cache is read whatever executes; and a backend that is wrong on
  purpose still gets no store, since a run that skipped a test is not evidence.
  `crates/ply-cli/tests/suite/cli.rs` holds both halves, and
  `armed.rs::a_shipping_command_that_installs_a_backend_must_also_bypass_the_cache`
  fails if a new route to a backend forgets either.
- **The row under `--backend`, fitted — taken.** `benches/marginal-change/`
  is it: `ply-corpus bench --backend`, the five edit scenarios at three sizes in
  a ratio of four, fitted step by step, with a `compile` phase of its own and an
  arm that reads what the real command pays. What it found, and what reordered
  the two items below: a rename costs nothing under either engine and a leaf
  edit is flat under the interpreter, but **the cost that dominates a small edit
  is the invocation**. A warm run that rechecks nothing still pays a front end
  proportional to the project — hashing every definition to establish that none
  moved, restoring every interface, writing them back — and under the backend
  the compile is about half of that again.
- **A warm process**, which is what the row chose — `ply test --watch`, started
  and not finished. It holds the store, the checked front end and the compiled
  unit across iterations, so an iteration where nothing moved pays a stat per
  file. An iteration where something moved still pays the whole front end,
  because the driver is one-shot. The phase report says there is no single term
  to fix: hashing is about a quarter of a warm run that rechecks nothing,
  writing back a fifth, and parsing, restoring, resolving, checking, assembling
  and reading divide the rest. **ADR 0038 measured what drives that and closed
  it.** The answer was not project size: an edit costs what the tests that must
  run *reach*, and those modules were re-derived although only their bodies were
  wanted. A module needed only to run now keeps its tree and is restored like any
  other unchanged file. An edit in a warm process no longer grows with the
  project, and no longer depends on whether the project has tests that run every
  time.
- **A compiled-code cache**, which the row demoted. `crates/ply-codegen`
  persists nothing across runs: no `DefHash -> code`, and `cranelift-jit` rather
  than `cranelift-object`, so there is no object output for a cache to hold, and
  `Cranelift::over` builds the whole unit once as a pre-flight and again for
  every worker that attaches. A warm process removes that cost without
  serialising anything, so this is only needed if the warm process is not
  enough.

The row is taken and it re-ordered these itself. A lever's share of a
whole-project run says nothing about its share of one edit, and every row the
path above is ordered on measures the former — which is how the invocation's own
cost went unnoticed until something measured an edit.

## Where this ends: two tiers, over a C compiler and libc

Every language rests on a host it did not write. Rust's is LLVM, libc and the
kernel, and everything with language content — the front end, the middle, the
standard library — sits above that line in Rust. Ply's line today is drawn much
higher: the evaluator, the code generator through Cranelift, the runtime helpers
compiled code calls, the driver and the host effects are all Rust. The path ends
when that line is where Rust's is: a Ply compiler, written in Ply, emitting C
whose only external dependencies are a C compiler and the C library, over a
runtime written in Ply or a thin C shim.

**It ends in two tiers, and whether they share a code generator is open**
(ADR 0037). Emitted C is the release tier: `ply build`, distribution, and a
bootstrap chain anyone can follow with a C compiler alone. The loop's tier is
whatever makes an edit compile O(change) definitions and a run load what its
selected tests reach, at a per-definition constant the loop affords. Cranelift
is that tier today and is a Rust library, so this goal takes it away
eventually; ADR 0037 lists what could replace it — the same C over
per-definition objects, a C compiler linked in process, copy-and-patch — with
the trade each makes, and chooses none until the rows it registers are read.

The route is visible already, which is why it can be written down before it is
started. The runtime surface compiled code depends on is an enumerated table —
the helpers in `crates/ply-codegen/src/rt.rs` a compiled body calls for field
reads, constructor tests, list operations, callbacks and failure — and that
table is the host ABI in all but name; the host effects are likewise an explicit
listing. A switch of host is a re-implementation behind two existing seams, not a
redesign. The order, each stage held to the Rust one by a differential as every
port in this tree has been:

1. **The value model competes** (step 9). Nothing below is worth starting until
   it does, because every later stage inherits the representation.
2. **The front end is within a small factor of Rust under that model** — the
   front-end row's own decision rule, re-taken.
3. **A code generator in Ply that emits C**, over the same runtime ABI, with
   the Rust code generator as the oracle: the same program compiled both ways
   answers the same, over the differential corpus. C rather than machine code
   because a C compiler is portable, debuggable and the route most self-hosted
   languages took at this stage; Cranelift is a Rust library, so a Ply backend
   over it would still link Rust. **This is the release tier.** Whether the
   loop's tier is the same generator over smaller units or a second one is
   ADR 0037's open question.
4. **A runtime over libc** behind the same ABI: values as step 9 laid them out,
   counts, reuse, strings, the ordered map, and the host effects over the C
   library. The open design question is effects with captured continuations —
   a CPS transform in the code generator or stack switching in the runtime —
   and the deterministic simulation scheduler must be reproduced exactly; that
   gets its own record before it is built.
5. **The driver last.** It needs only host effects the listing already has,
   and it is the part whose cost the plan has never found to matter.

What stays Rust until each stage lands is stated by the stage; nothing is
removed from the Rust side until its Ply replacement agrees with it over the
corpus, which is the rule that let every phase of the front end land without a
regression.

## What would make this plan wrong

- **If root entry, once reachable, still loses to no backend.** ADR 0030's
  per-entry cost — a registry lookup, a context, a clone and an arena push per
  argument, two post-conditions, a clone out — is then the ceiling, and the seam
  has to change shape before any construct is worth lowering.
- **If the container share stays the ceiling after steps 2 and 3.** Then step 4
  is the milestone and the generator's constructs were the cheap half.
- **If the inference speedup ADR 0021 bets on does not arrive.** Then tooling
  stays a minority of the loop, O(project) stays affordable, and the bootstrap is
  over-engineering; ADR 0021 says so in its own falsifiers and the second payoff
  path — the project simply growing — is slower.
- **If a Rust-side tool makes the conventional loop O(change).** Nothing has
  tried, and it would remove the motive entirely.
- **If step 9's kernels clear and the front-end row does not move.** Then the
  cost is at the seam or in dispatch rather than in the values, and ADR 0035's
  census is what says which; the C target waits either way.
- **If the row under `--backend` finds an edit flat in project size.** Then
  the reading of the code in §"The loop, which is what the path is for" is
  wrong somewhere, and ADR 0037's re-ordering should be given back once the
  row says where.
- **If no loop tier can be built without Rust at a latency the loop affords.**
  Then the two goals are one after all, the release tier is the only tier, and
  what gives is either the sub-second claim or the dependency line — ADR 0037
  does not say which.
- **If the compiled model cannot come within its factor of Rust on either
  kernel.** Then the bootstrap's front end cannot sit inside the loop ADR 0021
  exists for, on any host, and what to change is the model's decisions the
  record names for that kernel, not the host.

## What this is not

Not a decision to self-host. ADR 0020's decision stands until a re-take of
ADR 0030's row clears its bar, and this file should be corrected in place when
it does: replace the sentence, do not add a block saying the sentence moved.
And §"Where this ends" is a direction with an order, not a commitment to a
date: each stage is gated on the one before it, and the first gate is step 9's.
And it is not a plan for the loop: ADR 0037 carries that, and orders the two
against each other there rather than here.
