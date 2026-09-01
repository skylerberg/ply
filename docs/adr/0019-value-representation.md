# ADR 0019 — What a value costs, and what to change about it

Status: **partly accepted.**

- **Accepted:** §0's two invariants, which bind every change here. §4's *refusal*
  to narrow `Value`, which needs no further measurement — the number that would
  justify it was taken and it is zero.
- **Accepted, landed, and green:** §1 and §2. Both were motivated by an
  attribution taken before this ADR was written, both had a floor in
  `ply_corpus::r4::Lever::floor` fixed before either was built, and
  `ply_corpus::r4::judge` is the rule. Neither moved a store version.
- **Not accepted:** §3. Ranked and priced, and the only change here that would
  move a stored artifact's contents. It waits on §1 and §2 being measured and on
  the histogram in §"What is assumed and not measured" item 4.
- **Not decided here:** §5. The spike re-pricing changed what ADR 0018 should
  say; this ADR records what changed and does not amend it.
- **Reported, not priced:** §6 and §7 — one allocating site and one defect found
  while integrating.

## Provenance

Every figure this ADR rests on has a command that renders it, and the commands
are named where the claim is made rather than transcribed here. Re-take rather
than quote.

**This machine is shared and its load moves by an order of magnitude.**
Allocation counts do not vary with load; wall-clock ratios do, which is why
every ratio below comes from a harness that times both sides inside one window
(`benches/README.md` §"Every ratio is taken inside one window").

**One instrument runs a pre-built binary and the rest rebuild.** Most rows come
from `cargo test -p ply-corpus --release`, and cargo does rebuild — touching an
embedded `.ply` module alone makes `ply-std` recompile, because the modules are
in the dep-info cargo reads. The exception is `w6-alloc`, which runs whatever
binary is already on disk. `CONTRIBUTING.md` §"The binary is an instrument too"
has the check to run before believing that row.

## Context

### The milestone was requested as "unboxed primitives", and the premise was false

ADR 0018 §2 stated the case in two sentences: every `Int` is a heap-allocated
`Value`, and `interp::literal` allocates on a workload doing almost no
arithmetic. The first is false and the second attributes to the wrong thing.
ADR 0018 §2 now carries the correction and points here.

`Int`, `Bool`, `Float`, `Unit` and `Decimal` are **inline enum variants** of
`Value` and building one touches no allocator; `Cell` and `Task` are inline for
the same reason, read off the type rather than measured. **There is no primitive
boxing in this evaluator to remove**, and it is a test that says so rather than
a reading:

```
cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture
```

**The second sentence is the one worth keeping, because a re-take cannot catch
it.** The frame ranking was real and the count was real; the conclusion did not
follow from either. `interp::literal` cannot allocate at all unless the literal
is a `Str` or a `Bytes` — the other arms return without touching the allocator —
so the count was not integers being boxed. And the count itself was read off a
short window, where a `Machine`'s worth of one-time literal construction divides
down into what looks exactly like a per-request slope. Fitted over two windows
the per-request part is much smaller than the single-window reading, and the two
reconcile to the digit. `CONTRIBUTING.md` §"Measure an ADR's motivating claim"
warns about precisely this in its closing paragraph, and this is R3's lesson
arriving a second time: **a ranking is not a cost, and a frame is not a type.**

Two independently written classifiers agree on the split —
`w6_alloc_sites.rs` ranks by frame and `r4_value_construction.rs` ranks by
value.

### What the milestone did instead

It took the attribution the premise should have rested on: allocations per
request, attributed to the **value being built** rather than to the frame that
built it, fitted as a slope over two windows so that per-`Machine` setup cannot
masquerade as per-request work. That measurement ranks three changes and refuses
a fourth, and this ADR is that ranking with a threshold under each.

### What is measured

Two instruments, kept separate and never mixed:

| instrument | what it answers |
| --- | --- |
| `cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture` | allocations per request attributed **by the value built**, as a slope over two windows, for `/health` over SimNet and for a pure-call routing rung; what `Value`'s 32 bytes are spent on; what a narrower `Value` would move |
| `cargo test -p ply-corpus --release --test w6_alloc_sites -- --nocapture` | the same request attributed **by frame**, which is the ranking the false premise was read off |
| `./target/release/w6-alloc --repo . --requests 200` | the request-path allocation figure `README.md` states, asserted to within 1% by `w6_report_allocations::the_readme_still_describes_this_request_path` |
| `benches/adr0018-mcts.json` | the compute-kernel half — fragment coverage, the ratio where the fragment runs, the end-to-end ratio, the Amdahl ceiling. Re-take with `mcts --dir benches/kernel --only agreement`; `benches/README.md` §"What `mcts` adds" is the command |
| `benches/w6-spike-r4.json` | the `read_line` spike, re-taken against this tree |

`ply_corpus::r4` holds the baseline the shares are fractions of, and it was
fixed before any lever was built. **Do not edit it to make a measurement pass**
— that is the one edit that would stop the number meaning anything.

### Which route a lever is judged on, and why the file prints two

The attribution reports `/health` over SimNet **and** a pure-call routing rung,
and they disagree on ranking. Both are printed. A lever here is judged on the
SimNet path, because it is the only one that pays for framing, the host boundary
and the response encode — which is what a served request does. The routing rung
is the one to read for the interpreter proper. §1 and §2 rank first and second on
both routes, so nothing below depends on the choice.

### What is assumed and not measured

Each is load-bearing for something below.

1. **That a free list is cheaper than the allocator here.** §1 trades a
   `malloc`/`free` pair for a bounds check, a `Vec` swap and a length reset.
   `crates/ply-eval/src/pool.rs` is the same trade for `Rc` links and it paid,
   but a `Vec` with a capacity class is not an `Rc` link.
   *Discharged, weakly:* `ply_corpus::r4::Criteria::max_time_regression` is not
   exceeded, but see §1 — **no regression detected is weaker than no
   regression**, and this harness cannot resolve the criterion at all.
2. **That the transient argument vectors are transient in the sense a pool
   needs** — released before the next call of the same arity wants one, rather
   than held live down a deep recursion.
   *Settled, and depth was not the miss.* Every buffer that reached
   `Machine::enter_code` was recycled. What was wrong is the claim that
   `enter_code` frees them all; §1 is the arithmetic.
3. **That interning a compile-time constant is unobservable.** No Ply expression
   can read an address, and `Value::builtin` — one `Closure` per builtin per
   thread since W6 — is the standing precedent. The reason to doubt it:
   `RUNTIME_VERSION` moved for the constant memo, whose note said "no value
   moves — that is the argument for doing it" and then said what moved anyway.
   The same sentence is available here and would be as wrong.
   *Settled by:* `crates/ply-eval/tests/suite/ctor_value_sharing.rs` in full for the
   constructor half, with `constant_memo.rs` unchanged. `--engine both` settles
   the literal half **and nothing about the constructor half** — see §2.
4. **That a record's field count stays small.** §3's flat layout is a linear
   scan; a B-tree is not. Nothing here measures the distribution of record
   widths in a real program.
   *Settled by:* a width histogram, which does not exist and which §3 must add
   before it is accepted.
5. **That the byte columns in the attribution mean anything.** They do not, past
   one window pair. `r4_value_construction::the_per_request_slope_is_the_same_between_the_second_and_third_window`
   prints an allocation slope that holds between window pairs and a byte slope
   that does not — `CONTRIBUTING.md` §"Things known to be broken" item 8,
   reproduced on this path. **No threshold in this ADR is stated in bytes**, and
   `ply_corpus::r4` has no byte field. A byte column is comparable only against
   another taken at the same window pair.

## 0. Two things every change below preserves

Not per-change notes. They bind §1, §2 and §3 alike.

### 0.1 `Value::Secret`'s payload stays unmatchable and unprintable

ADR 0015 §2. `Value::Secret` is a distinct variant rather than a `Ctor` because a
`Ctor` is matchable, and one `match s { Secret(p) -> p }` is a one-line escape
from every guarantee built on it.

| the move | what it would do | what catches it |
| --- | --- | --- |
| folding `Secret` into `Ctor`, or giving it a tag a pattern can name | makes the payload matchable | `crates/ply-eval/tests/suite/secrets.rs::a_secret_is_never_equal_to_its_payload` and the `E0206` refusals around it |
| a rendering path that descends into a compound before `Value::write`'s `Secret` arm sees it | prints a credential | `secrets.rs::a_secret_renders_redacted_whatever_it_holds`, `::a_nested_secret_renders_redacted`, `::a_failing_assertion_prints_no_payload` |
| a pool or intern table that keeps a `Value` after the call that carried it returned | leaves a credential in a buffer the next call reads from | `crates/ply-eval/src/argv.rs::tests::a_secret_handed_back_is_not_held_by_the_pool`, which asserts on the `Arc` count and fails for any keep-without-clear |

The third is the risk §1 introduces. It was armed against the seam before the
pool existed.

### 0.2 The store's schema fingerprint moves if the encoding of a stored type moves

`ply_store::schema_fingerprint` is digested over *encoded exemplars*, so an
encoder that starts writing a field differently moves it even when no type
declaration changed. `schema::tests::the_stored_schema_is_pinned` is what fails
and says to bump `FRONTEND_VERSION`.

**Checked, because it decides how much of this ADR is a cache-format change:**
`ply-store` does not depend on `ply-eval` at all, and `ply-hash`'s normalizer
normalizes the AST, not values. **`Value` is not a stored type.** So none of §1,
§2 or §3 moves the schema fingerprint by changing `Value`'s shape.

What *is* stored is `Value::render`'s **output**: an assertion failure's note is
built from it and cached as `Outcome::Fail { message }`. So:

- A change that moves a rendered byte is a **`RUNTIME_VERSION`** bump, not a
  schema one — a cached `Fail` message would otherwise describe a run this
  evaluator no longer produces.
- A change that moves a stored *type's* encoding is a **`FRONTEND_VERSION`** bump
  and the pin test names it. §3 is the only change here that goes near one, and
  it does not: a record's `Value` layout is not `Type`.

Every change below states which it claims, and the claim is checkable by running
the pin test.

## 1. Recycle the call-argument vector

**The number.** The largest single line in the profile, on both routes. A few
of the vectors survive the call as `Ctor.args`; the rest are filled, handed to
the callee, emptied into its scope and freed. The steady state is a
`malloc`/`free` pair per call.

**The representation.** Not a change to `Value`. A thread-local free list of
`Vec<Value>` in four capacity classes, arity 1 through 4, which covers the large
majority of the vectors by arity. `crates/ply-eval/src/pool.rs` is the same
mechanism for `Rc` links, bounded for the same reason, and its module note is the
model. The seam is `crates/ply-eval/src/argv.rs`:

```rust
pub(crate) fn take(arity: usize) -> Vec<Value>;
pub(crate) fn give(args: Vec<Value>);
```

wired at the two sites the attribution names — `Frame::AppCallee`'s `done:` field
and `Machine::enter_code` after the arguments are bound into scope. **This change
is those two function bodies.**

**Where the reasoning had a hole, and it is the finding.** The specification
said `enter_code` frees every non-retained buffer. It does not, because it skipped
a callee kind: `Machine::enter_closure` sends a `ClosureKind::Builtin` to
`Machine::call_builtin`, and `ply_eval::builtins::call` takes its `Vec<Value>`
**by value** and consumes it — so that buffer is freed inside a function with no
way to hand it back, whatever the seam does. The vectors split four ways, and the
split is what the free list can and cannot reach:

| | what it is |
| --- | --- |
| **recycled by the free list** | taken at `Frame::AppCallee`, given back at `enter_code`. This is what §1 removes |
| retained as `Ctor.args` | `enter_closure`'s `ClosureKind::Ctor` arm keeps the buffer; there is nothing to give back |
| wider than the four classes | arity 5 and up, left to the allocator by construction |
| **freed but never given back** | a callee that is not `enter_code` — overwhelmingly `builtins::call` |

Re-take it: the `r4_value_construction` command prints that split under
`-- /health: the argument vectors the free list did not serve, by arity --`, and
`a_warm_ply_call_takes_its_argument_vector_from_the_free_list` is the controlled
experiment underneath — a warm 1-argument Ply call in a loop body allocates
nothing per iteration and a 1-argument *builtin* call still allocates.

**`judge` answers `Verdict::Short` on a lever that removed everything the
mechanism could reach, and the floor was deliberately not edited.** The floor was
derived as "a little over half" of a share this document had wrong; the share the
lever can actually reach is under it. **That is a documentation defect, not a
weak lever.** Editing a pre-registered threshold to make a measurement pass is
the one edit that would stop the number meaning anything
(`CONTRIBUTING.md` §"Measure an ADR's motivating claim before accepting the
ADR"), so `crates/ply-corpus/src/r4.rs` is untouched and
`no_levers_floor_is_above_what_the_attribution_places_under_it` still passes
against the pre-registered numbers. Re-deriving the floor is a decision for
whoever amends this ADR.

**Assumption 1 is discharged, and "no regression detected" is weaker than "no
regression".** The instrument is paired windows of `ply-corpus serve --repo .
--no-load --repeats 5 --ladder-requests 8000`, running the two binaries back to
back so each pair is one window, alternating which goes first so first-position
bias cancels, and reading the `answer` rung — the rung with no socket in it.
Windows are kept or dropped by the load average sampled at the *start* of the
window, on a threshold fixed before the data was taken.

Three things are worth carrying forward from that, and none of them is a ratio:

- **The one pre-registered run is underpowered and does not resolve the
  criterion.** Pooling it with earlier runs under its own threshold does clear
  the bar, but that applies a filter chosen with some of the data already seen,
  so it is a supporting cut and not the result.
- **The sign test leans the wrong way for this change** — the pool is slower in
  more windows than it is faster, at a magnitude of one print step of the
  harness. Not a result, and not nothing. A consistent sub-1% cost is exactly
  what a bounds check, a class index and a length reset would look like, and this
  instrument cannot separate it from rounding. **Resolving it needs a timer with
  more digits than the ladder prints, not more windows.**
- **A percent-scale criterion cannot be resolved on a loaded machine at all** —
  windows taken under high load spread by a factor of three.

**The buffer a builtin consumes is the next lever and it is larger than §3.**
Recovering it means `builtins::call` taking `&mut Vec<Value>` or draining rather
than owning, across every arm of a ~100-arm match. That is a change to a function
signature, not to two function bodies, and it is not claimed here.

**What it must not break.**

| | |
| --- | --- |
| a pooled vector may not hold a `Value` | it would keep a `Cell` past the region that would reclaim it, defeat `Arc::get_mut` in `value.rs`'s dismantler, and park a `Secret` in a reused buffer |
| a vector handed out may not be non-empty | the callee pushes; a residue shifts every argument |
| the list may not cross a thread | a `Value` is thread-confined (`value.rs`'s note on `RcK`) |
| the vector retained as `Ctor.args` may not also be in the list | two owners of one buffer |
| a release during thread-local teardown may not abort | `pool.rs` takes `try_with` throughout for exactly this |

**The tests that catch it breaking.**
`argv.rs::tests::a_secret_handed_back_is_not_held_by_the_pool` (the §0.1
obligation), `::a_vector_given_back_full_does_not_come_out_of_take_full` (the
residue), and `region_reclamation_audit.rs` and `use_after_free_audit.rs` in
full.

**`--engine both` cannot see the case this lever is most likely to break.** Every
`resume k` clause is `E0504 MACHINE_ONLY_CLAUSE` in the tree-walker, so the
differential harness records the tree-walker's *refusal* and compares no value at
all for a program that resumes a continuation — and multi-shot resumption is
exactly where one `Frame::AppArgs` becomes two, each finishing a buffer taken
from the free list. `value_semantics_audit.rs` audits it instead, with a
continuation captured *inside an argument list* and resumed twice, and asserts
the blindness itself so a tree-walker that later grows the capability turns that
justification into a failure rather than leaving it stale.

**Versioning.** No stored type moves; no rendered byte moves. No bump. Checked by
`ply-store`'s pin test staying green without being touched.

**The bar.** `ply_corpus::r4::Lever::ArgumentVectors`.

## 2. Build a compile-time constant's `Value` once

**The number.** Second on both routes: values rebuilt every evaluation from
something the compiler already knows — literal `Str`/`Bytes` constructions,
nullary constructor mentions, and constructor-closure mentions.

This is R3's pattern: runtime work for a static value. A literal is a
compile-time constant whose `Value` could be built once at lowering, and an
`Arc` clone is a refcount bump — measured at zero allocations by the same run.

**The representation.** Three sites, two mechanisms, no new type:

1. **Literals.** `NodeKind::Lit(Lit)` becomes `NodeKind::Lit(Lit, Value)`, built
   once where the node is lowered, by the existing `interp::literal`, which keeps
   its signature and its callers; the machine clones the node's value instead of
   calling it per evaluation. **`Lit` stays in the variant**, and the reason is
   not the machine: `crates/ply-codegen-spike` dispatches on it to choose a
   Cranelift type. That crate is outside the workspace, nothing in
   `cargo test --workspace` compiles it, and it has already bit-rotted from
   exactly this kind of widening (`CONTRIBUTING.md` §"Things known to be broken"
   item 1). **A build agent that widens `NodeKind::Lit` and does not build the
   spike breaks the only instrument this project has for pricing codegen.**
2. **Nullary constructors and constructor closures.** `interp::ctor_value` keeps
   its signature; its body becomes a thread-local cache. `Value::builtin` is the
   shape to copy verbatim, `try_with` and all, and its doc comment already
   carries the argument for why sharing is invisible.

**The mitigation this section specified was structurally incapable of firing,
and that is the finding.** It said the tree-walker is unchanged and `--engine
both` is therefore the check. For item 2 that is false: the memo lives inside
`interp::ctor_value`, and the **tree-walker's** constructor arm calls it too. So
both engines answer a constructor mention **from one memo** and comparing them
compares a value against itself. Item 1 is the half where the sentence holds and
is the control — the tree-walker builds a fresh `Arc<str>` per evaluation while
the machine clones the node's value, so a literal divergence *would* be visible.
Measured, not reasoned, by
`value_semantics_audit.rs::both_engines_answer_a_constructor_mention_from_one_memo_and_a_literal_from_two`,
which asserts `Arc::ptr_eq` across the two engines in both directions.

This is the failure class `CONTRIBUTING.md` §"The one rule" lists twice (M8, W5):
a mitigation named in a document that cannot fire. **What actually audits the
memo is `crates/ply-eval/tests/suite/ctor_value_sharing.rs`**, whose note on `on_both`
says why it can — *"`ctor_value` is shared by the two, so a cache in it is a
change to both"* — and which checks the properties that survive sharing: the
shared value matches the arm a fresh one matched, is `values_equal` to a fresh
one, and holds nothing a closing region could reclaim.

**What it costs.** A `Value` per literal node held for the program's life instead
of built per evaluation — the same bytes `NodeKind::Lit`'s owned `Lit` already
holds, moved rather than added — and a thread-local lookup on a constructor
mention.

**What it must not break.**

| | |
| --- | --- |
| a shared `Str` may not become observable as shared | no Ply operation reads an address; `Value::cmp` answers `Equal` for any two closures, which is why `Value::builtin` was already safe |
| the constant memo's semantics | `RUNTIME_VERSION` moved once already because remembering a nullary definition moved what `E0502` fires on. Interning a *value* is not that, and the difference has to be shown, not asserted |
| a simulation's recorded accesses | `Machine::constant` refuses the memo inside a `simulate` region because an allocation is an `Access::Alloc` the search depends on. Constructing a `Ctor` is not an `Access` — but if any interning touches a cell, the same rule applies |
| `--engine both` | **item 1 only.** For item 2 both engines read one memo; `ctor_value_sharing.rs` is the evidence |

**The tests that catch it breaking.** `ctor_value_sharing.rs` in full is the file
that audits item 2, with `constant_memo.rs` unchanged;
`differential_corpus.rs`'s agreement tests are independent evidence for item 1
and item 2's literals only. Three more mark edges: `secrets.rs` in full, because
an intern table admitting a `Secret` would be a credential with program
lifetime; `map_order.rs`, because a shared key is still a key; and
`resumption_semantics_audit.rs`, because a shared constant reached through a
resumed continuation is where one-world threading would stop being true.

**Versioning.** No stored type moves. No rendered byte moves — the same `Value`
renders the same way whether it was built once or a thousand times. So no bump.
**If a build agent finds it needs a `RUNTIME_VERSION` bump, that is a signal
something *is* observable, and it should stop rather than bump.**

**The bar.** `ply_corpus::r4::Lever::ConstantValues`.

## 3. A record's fields in one allocation

**Not accepted.** Ranked and priced so the next reader has the number; it waits
on §1 and §2 landing and on the histogram in §"What is assumed and not measured"
item 4.

**The number.** `Value::Record(Arc<BTreeMap<Symbol, Value>>)` costs **two
allocations for a one-field record**, because a `BTreeMap` allocates a whole node
for the first field. On the request path that is the largest byte line among
`Value`'s payloads — though item 5 is why no decision here rests on a byte
column.

**The representation.** `Arc<[(Symbol, Value)]>`, sorted by `Symbol`, built once
at construction. `Symbol` is `Arc<str>` with a derived `Ord`, so that sort is
lexicographic and **does not depend on intern order** — checked, because a field
order that varied run to run would break four things at once and none of them
loudly. One allocation, no per-node overhead, field lookup a linear scan over a
contiguous slice.

`Value` stays 32 bytes wide, and that is *reasoned* from the component table
rather than measured: an `Arc<[T]>` is a fat pointer against the thin
`Arc<BTreeMap>`'s thin one, and the widest variant is still `Ctor`.
**Re-take `size_of::<Value>()` before believing that**;
`the_shape_of_every_value_variant_is_measured` prints it.

**What it costs.** Linear field lookup, and a rebuild-on-update where a
`BTreeMap` shares structure. Assumption 4: nobody has measured record width in a
real program, and this change is wrong at fifty fields.

**What it must not break.** `Value::cmp`'s `Record` arm iterates in order and
`values_equal` compares key sequences first — both depend on **ascending
`Symbol` order**, and both would silently answer wrongly over an unsorted slice
rather than fail. `Value::write` renders in the same order, and that output is
stored. **The sort is load-bearing at three places and is the whole risk of this
change.**

**The tests that catch it breaking.** `map_order.rs` (a record inside a map key),
`secrets.rs::a_failing_assertion_prints_no_payload` (a record holding a `Secret`
renders), the derivation audits under
`crates/ply-cli/tests/suite/derivation_determinism_audit.rs`, and — for the ordering
itself — a test that two records built by different field orders are one value,
which does not exist.

**Versioning.** The one change here that reaches a **stored artifact's
contents**: `Value::render`'s record output is cached in `Outcome::Fail.message`.
If field order moved, that is a `RUNTIME_VERSION` bump. It does not move if the
slice is sorted — and "does not move" is an obligation on the implementation
rather than a property of the type.

**The bar.** `ply_corpus::r4::Lever::RecordLayout`.

## 4. Rejected: narrowing `Value`

The milestone's name points here, so the refusal is stated rather than left to
omission.

`Ctor { name: Symbol, args: Arc<Vec<Value>> }` is the widest variant and the only
thing holding `Value` at 32 bytes. Boxing it together reaches 24.

**What 24 bytes would buy, measured:** a few kilobytes per request off a byte
column §"What is assumed and not measured" item 5 says cannot be compared across
windows, and **zero allocations**.

**What it would cost:** an allocation on every applied constructor, to box the
name and the args together. So the change trades an incomparable byte count for
a comparable allocation count, in the wrong direction.

The other direction is worse: `Arc<[Value]>` for `Ctor.args` removes one
indirection and **widens `Value` to 40 bytes**, and the same run prices its
construction at two allocations against `Arc::new` on the vector the caller
already filled at one. On the path where it matters — `enter_closure`'s `Ctor`
arm takes ownership of the argument vector — it is strictly worse.

**Rejected, and this needs no further measurement: the figure that would justify
it was taken and it is zero.**

## 5. What the spike re-pricing changed, and what ADR 0018 now needs

ADR 0018 §1 asked for one measurement before anything was built: re-price the
codegen spike against a compute kernel. It landed as `benches/adr0018-mcts.json`.
**Neither that file nor this ADR amends ADR 0018.** What follows is what an
amendment owes.

**The premise held on shape.** Most of the kernel's functions and lowered nodes
are inside the fragment, and most of its executed work is. ADR 0018 was not wrong
that a compute kernel is mostly arithmetic — the corresponding share for an HTTP
request is the much smaller one ADR 0016 states, and that share is **not re-taken
here**: the spike did not compile against this tree until R4.

**What the fragment refuses is the roadmap — but the refusal census does not read
as a work list, and that is the trap.** `refusals_ranked` is a **first-refusal**
census: one construct named per refused function. The rows are therefore **not
additive and not independent**. Admitting the top-ranked construct moved the
census by almost nothing and left what executes unchanged, because the rows are a
single closure that arrives on the last item — and a fifth construct that appears
in no row at all had to be lowered too. All of them are in now: the fragment
takes every function and every node of the kernel, with no refusals and no
disagreements. ADR 0018 §0 carries the per-item table.

**The conclusion ADR 0018 drew does not hold, and the reason is structural.**
End to end on the whole kernel the hybrid was worth nothing, because **the
interpreter could not call compiled code.** A function the fragment accepts whose
callers it refuses is compiled and never entered. The boundary was never the
problem — the crossings cost a rounding error of the run.

**And the ceiling is not what ADR 0018 assumes.** Amdahl over the fragment's
share and the ratio where it runs gives a whole-kernel ceiling far below the
in-fragment ratio, and below it again at an infinitely fast fragment. Neither the
spike's `read_line` figure nor the in-fragment ratio is the ceiling for a kernel.

So an amendment to ADR 0018 still owes three things. Items 1, 2 and 6 of the
original list are discharged; the numbering is kept so citations resolve.

3. **A new blocker inserted above §2:** a backend the interpreter cannot enter
   buys nothing whatever the representation is. That is a prerequisite for §2, §3
   and §5 of ADR 0018 and is not in that document at all.
4. **§2's "and `Float`" priced or withdrawn.** The fragment has no `Float` path,
   compiles `a + b` as `Int` arithmetic whatever the operands are, and fails at
   run time —
   `crates/ply-codegen-spike/tests/mcts_kernel.rs::the_fragment_accepts_float_arithmetic_and_then_fails_on_it_at_run_time`.
   **A census counting such a function as compiled counts one that cannot run.**
5. **A lever ADR 0018 does not list, which outranks most of the ones it does.**
   Ply ships no `sqrt` and no `ln` for any numeric type, so the kernel's `ucb`
   computes its own square root by Newton's method over an `ilog2` logarithm —
   and `ucb` dominates the kernel. Removing those two costs almost all of the
   call. **Two prelude builtins and no compiler work**, from three fields of
   `benches/adr0018-mcts.json`.

Item 5 is why the sequencing below does not put §2 of ADR 0018 first, and item 3
is why it does not put codegen anywhere.

## 6. Found while integrating, and deliberately not priced here

One site, recorded rather than fixed, because a lever with no attribution and no
pre-registered floor is what this ADR exists to refuse.

**`ply_std::is_reserved` builds a `String` on every call.** It is
`name == ROOT || name.starts_with(&format!("{ROOT}."))`, and
`ply_eval::host::registration_names` asks it once per host-operation resolution.
`is_pseudo_path` has the same shape and is not on this path. The removal is three
lines with no semantic content — `strip_prefix` in place of `format!` — so it is
almost certainly worth taking. Not taken here, for two reasons, and the second is
the real one:

1. It is not a value-representation change, so nothing in `ply_corpus::r4` places
   a share under it or sets a floor for it, and `judge` cannot answer.
2. Landing it would move the freshly re-taken request-path figure and break the
   per-lever decomposition beside it, which was A/B'd one change at a time against
   this tree. And §"Sequencing" step 2 is explicit that when a lever lands under
   its floor the next step is *another attribution* rather than the next lever.
   §1 did land under its floor, and that applies to a lever found by accident as
   much as to a planned one.

**How it surfaced is worth more than the site, and it is a trap in the
instrument.** `r4_value_construction`'s rule table attributes an allocation by
(deepest `ply_*` frame, size) over a three-frame window. In release this
allocation is **inlined** into `Machine::perform_host` and lands in the
host-boundary bucket; in debug it is a frame of its own whose callers symbolize
to a bare crate name, so the window ends earlier and the same tree is attributed
two ways **by profile**. The residue passed in the profile the harness was
developed in and failed in the one `cargo test --workspace` runs. Naming the
function in the rule table restores the module note's own invariant — *"both
spellings must land in the same bucket"*. **Anything else added to that table
should be checked in both profiles before it is believed.**

## 7. Found while auditing, and fixed: a `Map` key was a function of insertion history

**Not a lever, not ranked, and not an R4 regression** — it predates R4 and
neither §1 nor §2 touches it. It is here because it is a *value representation*
defect, this is the value-representation ADR, and the consequence was recorded
nowhere while three comments in the tree asserted its opposite.

**The defect, and it is a conjunction of two deliberate decisions.** `Value::cmp`
and `values_equal` compare a `Decimal` **by numeric value**, so that `1.50m` and
`1.5m` are one map key and so a `Decimal` may appear in a `proved` obligation as
an uninterpreted term. `Value::write` prints **the scale as stored**, because the
scale is a digit count the value carries. Together they made a `Map`'s *keys* a
function of insertion history: `map::insert` replaced an equal key's key as well
as its value, so whichever spelling was written last is the one `map_keys`
answered with.

Three written claims said that could not happen, and each is now enforced rather
than asserted:

- `ply_eval::value`, the `Map` type note — *"a hash-ordered map makes `map_keys`
  a function of a hasher's seed and of insertion history, and four separate
  guarantees rest on a value having one canonical form"*. That is the stated
  reason `Map` is a search tree; the failure it names was present anyway, through
  the key rather than through the order.
- `ply_eval::value`, on `Value::cmp` — *"`map_keys` is a function of the values
  and of nothing else"*.
- `ply_core::infer`, on `map_fold` — *"a fold over a map is a function of the
  map's contents rather than of how it was built"*.

ADR 0012 §"Iteration order is the property that matters" and `CONTRACTS.md`
§`Map` state the same guarantee and were false the same way; both now point here.

**What it cost, end to end.** Two maps that `assert_eq` as one value served two
different response bodies. `std.json`'s number writer goes through
`decimal_to_string`, so a `derive json` over a record holding a
`Map<Decimal, String>` wrote one key or the other depending only on the order two
inserts ran in — **with `--engine both` reporting no divergence, because this was
never an engine disagreement. It was the language.** The blast radius was wider
than `Map<Decimal, _>`: `Map<{sku: String, price: Decimal}, Int>` typechecks, so a
record-keyed map carried it into a compound key.
`derivation_determinism_audit::a_decimal_keyed_map_encodes_one_body_whichever_spelling_was_written_last`
runs the program under `ply test` and again under `--engine both`; **take the
body from the test, not from a sentence.**

**The fix, and where it goes.** In the representation, not in either deliberate
decision: `ply_eval::value::canonical_key` reduces a key to the one
representative of its equivalence class on the way in, and
`ply_eval::value::insert_key` is the single site every `Map` insert now passes
through. **Adding a second site re-opens the defect.** For a `Decimal` the
canonical member is `Decimal::normalize` — minimal scale is unique per numeric
value, so it is a canonical form rather than merely a smaller one. Every position
`Value::cmp` descends into is walked, because a `Decimal` anywhere under a key is
a distinction the order cannot see. A `Secret` is **not** descended into: it is
refused as a key before this runs, it cannot be under one, and a path that
rebuilt a credential's payload is what ADR 0015 §2 exists to prevent. The scan
does not allocate, and the request path is unmoved.

`1.50m == 1.5m` still holds, and a `Decimal` that is not a map key still renders
every digit it was written with — both asserted, so a "fix" that rounded the
scale away everywhere would fail.

**Versioning.** A stored artifact's contents **do** move for a program that
renders a `Map` with a `Decimal` key, but only from one of two spellings to the
canonical one, and only for a value that had no single spelling before. No stored
*type*'s encoding moves, so no `FRONTEND_VERSION` bump. Whether that is worth a
`RUNTIME_VERSION` bump is judged here explicitly rather than by omission: **it is
not taken**, because the values whose rendering moves are exactly the ones whose
cached message described a run that was not reproducible in the first place, and
a cache entry keyed by a hash that did not move still describes the same
*verdict*. A reader who disagrees should bump it; the argument is here to be
disagreed with.

**The tests.** `map_order.rs` carries verbatim what it asserted from W2 until
this change, plus the compound-key half. `value_semantics_audit.rs` holds the
three tests that found it, each rewritten to assert the fix with the old
expectation quoted in its note, and arms the ADR 0015 §2 bound against the new
path on the `Arc` pointer.

## The criteria, in code

`crates/ply-corpus/src/r4.rs`. It fixes, before any of this is built: the window
pair a slope may be fitted from, the baseline the shares are fractions of, the
share the attribution places under each lever, the floor each must clear, the
wall-clock regression none may exceed, and the divergence count that reverts one
whatever it saved. `ply_corpus::r4::judge` is the rule and
`no_levers_floor_is_above_what_the_attribution_places_under_it` is the test that
a floor cannot be quietly raised above what was ever counted.

This is `ply_corpus::w6::Criteria::default()`'s pattern, and it is here for
`CONTRIBUTING.md`'s reason: **a threshold a measurement supplies is a threshold
the measurement cannot fail.**

## Sequencing

1. **§1, the argument vector.** Largest line on both routes, and it touches no
   stored artifact and no `Value` variant.
2. **Re-take the attribution.** Not "run §2 next": if §1 lands under its floor,
   the answer is another attribution, because a mechanism that fired on something
   other than what was counted invalidates the ranking under it too.
3. **§2, compile-time constants.** Independent of §1; sequenced after only so
   each lever's number is attributable to it.
4. **The width histogram in assumption 4**, then decide §3.
5. **Amend ADR 0018** with §5's items. Its §1 is discharged and its ordering is
   not.

§4 is closed and is not sequenced.

## What would make this ADR wrong

- **If §1 lands and the request's allocation count does not move by its floor.**
  Then the transient argument vectors are not transient in the sense a pool needs
  (assumption 2), the attribution's largest line is not a lever, and the right
  response is another attribution — not §2, and not a wider pool.
- **If §1 saves allocations and loses wall clock.** Then assumption 1 is false,
  the trade is real, and every allocation-count target in this ADR — including
  `README.md`'s sentence and the whole `w6` ladder's boxing lever — is measuring
  a proxy that has stopped tracking the thing anyone cares about.
- **If §2 requires a `RUNTIME_VERSION` bump.** Then interning a compile-time
  constant *is* observable, assumption 3 is false, and `Value::builtin` — which
  has been doing exactly this since W6 — is a latent defect rather than a
  precedent.
- **If a build agent has to widen `Value` past 32 bytes to land any of this.**
  Every argument in §4 was made at 32, and most of a request's `Value`-wide slots
  live in argument vectors alone. A change that saves allocations and widens every
  slot has not been priced by anything here.
- **If §1's seam turns out not to be where the vectors are made.** It is wired on
  the strength of one controlled experiment, which adds one 1-argument call to a
  loop and watches one argument-vector-sized allocation appear under
  `frame::dispatch`. **That licenses reading `frame::dispatch` at a multiple of
  the slot size as an argument vector, and nothing else does.** If a pool at that
  seam moves fewer allocations than the arity table predicts, the rule table is
  what to doubt first, not the pool.
- **If the two routes' rankings diverge after a lever lands.** They agree on §1
  and §2 today. If they stop agreeing, one of the two harnesses is measuring
  something the other is not, and no verdict may be read off either until that is
  explained.
