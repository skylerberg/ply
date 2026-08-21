# ADR 0019 — What a value costs, and what to change about it

Status: **partly accepted.**

- **Accepted:** §0's two invariants, which bind every change here. §4's
  *refusal* to narrow `Value`, which is a decision and needs no further
  measurement — the number that would justify it was taken and it is zero.
- **Accepted with a floor stated in code:** §1 and §2. Both are motivated by an
  attribution taken before this ADR was written, both have a threshold in
  `ply_corpus::r4::Lever::floor` that was fixed before either was built, and
  either is reverted if it lands under it. `ply_corpus::r4::judge` is the rule.
- **Not accepted:** §3. It is ranked and priced, and it is the only change here
  that would move a stored artifact's contents. It waits until §1 and §2 have
  been measured, because the ranking under it is 3.6% and the reconciliation
  cost is real.
- **Not decided here:** everything in §5. The spike re-pricing changed what ADR
  0018 should say. This ADR records what changed and does not amend ADR 0018.

## Provenance

Everything measured for this ADR was taken on **2026-08-21**:

| | |
| --- | --- |
| machine | Apple M-series, `aarch64-apple-darwin`, 10 cores, 32 GiB (`sysctl -n hw.ncpu hw.memsize`) |
| OS | macOS 15.7.3 (`sw_vers -productVersion`), Darwin 24.6.0 (`uname -r`) |
| toolchain | `rustc 1.93.1 (01f6ddf75 2026-02-11)`; the codegen spike needs `+1.94.0` |
| load | `uptime` read `6.30 4.65 5.82` when the attribution was re-taken |

This machine is shared and its load moved between 3 and 47 while the numbers in
`benches/adr0018-mcts.json` were taken. Allocation counts do not vary with load;
the wall-clock ratios in §5 all come from a harness that times both sides inside
one window for that reason (`benches/README.md` §"Every ratio is taken inside
one window").

## Context

### The milestone was requested as "unboxed primitives", and the premise was false

The request that opened this milestone was to unbox primitives, and ADR 0018 §2
states the case for it in two sentences:

> **The gap.** Every `Int` is a heap-allocated `Value`. `interp::literal`
> allocates 111 times per request on a workload doing almost no arithmetic.

The first sentence is false and the second attributes to the wrong thing. Both
are left standing in ADR 0018, and this is the correction. `CONTRIBUTING.md`
§"Correct, do not delete" wants the correction *beside* the original, which
means a block in ADR 0018 — §5 item 1 is that obligation, deliberately not
discharged here, and until it is, a reader who opens ADR 0018 §2 first will read
the false claim with nothing pointing at this file.

`Int`, `Bool`, `Float`, `Unit` and `Decimal` are **inline enum variants** of
`Value` (`crates/ply-eval/src/value.rs:50-104`) and building one touches no
allocator. Measured, printed by name:

```
cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture
```

> ```
> -- what one Value costs to build --
>   Value::Int       0 allocations
>   Value::Bool      0 allocations
>   Value::Float     0 allocations
>   Value::Unit      0 allocations
>   Value::Decimal   0 allocations
> ```

`Cell` and `Task` are inline for the same reason and are not in that table:
`Cell(Slot)` is two `u32`s (`crates/ply-eval/src/arena.rs:163-166`) and
`Task(TaskId)` is one (`crates/ply-eval/src/sim.rs:141`). Neither is measured
above; both are read off the type.

`size_of::<Value>()` is **32 bytes**, printed by the same test under `-- what
the 32 bytes are spent on --`. **There is no primitive boxing in this evaluator
to remove.** How long that has been true is not checked here and is not the
point; what is checked is that it is true of this tree, by a test that runs.

The second sentence — `interp::literal` at 111 allocations per request — is a
frame ranking read off a **20-request window**, and `interp::literal` **cannot
allocate at all** unless the literal is a `Str` or a `Bytes`
(`crates/ply-eval/src/interp.rs:999-1009`; `Int`, `Bool`, `Float`, `Decimal` and
`Unit` return without touching the allocator). So the count was real, the frame
was real, and the conclusion drawn from it — that integers are boxed — did not
follow from either.

The window is the rest of it, and the two figures reconcile exactly. Re-taken
with `cargo test -p ply-corpus --release --test w6_alloc_sites -- --nocapture`,
that frame reads **111.2 at 20 requests** and fits to **65.0 per request plus
925 once per `Machine`** — and 65.0 + 925/20 = 111.25. So 111 is a per-request
slope with a `Machine`'s worth of one-time literal construction folded into it,
which is the exact defect `CONTRIBUTING.md` §"Measure an ADR's motivating claim"
warns about in its closing paragraph: *one-time work divided by twenty looks
exactly like that*. The slope is 65.0.

Two independently written classifiers agree on it — `w6_alloc_sites.rs` ranks by
frame and `r4_value_construction.rs` ranks by value, and both print 65.0 and
925.

This is R3's lesson arriving a second time and `CONTRIBUTING.md` §"Measure an
ADR's motivating claim before accepting the ADR" is where it is written down.
A ranking is not a cost, and a frame is not a type.

### What the milestone did instead

It took the attribution the premise should have rested on: allocations per
request, attributed to the **value being built** rather than to the frame that
built it, fitted as a slope over two windows so that per-`Machine` setup cannot
masquerade as per-request work. That measurement ranks three changes and refuses
a fourth, and this ADR is that ranking with a threshold under each.

### What is measured

Every figure below has a command that renders it. None is quoted from another
document.

| Figure | Source |
| --- | --- |
| 911.5 allocations per `/health` request over SimNet, + 34,465 once per `Machine` | `cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture`, the `fit:` line of the `/health` section |
| 1,083.9 allocations at a 200-request window on the same path | same run, `200 requests:` line. Reconciles with the slope: 911.5 + 34,465/200 = 1,083.8 |
| 1,082 allocations per `/health`, the figure `README.md:363` states | `./target/release/w6-alloc --repo . --requests 200`; asserted to within 1% by `crates/ply-corpus/tests/w6_report_allocations.rs::the_readme_still_describes_this_request_path` |
| `interp::literal` at 111.2 allocations at a 20-request window, fitting to 65.0 per request + 925 once per `Machine` | `cargo test -p ply-corpus --release --test w6_alloc_sites -- --nocapture`, the whole-request rung and the two-window fit under it |
| 372.4 argument vectors per request, 40.9% | same run, `Vec<Value> — call arguments` |
| at most 31.0 of those retained as `Ctor.args`; 341.4 transient, 37.5% | same run, the closing line of the `-- /health: the argument vector, by arity --` table. `enter_closure`'s `ClosureKind::Ctor` arm (`crates/ply-eval/src/machine.rs:1786`) is the only path that keeps one |
| arity 1–4 covers 349.4 of 372.4 = 93.8% | same table |
| 65.0 literal `Str`/`Bytes` constructions per request, 7.1% | same run, `Value::Str\|Bytes — literal, rebuilt per evaluation` |
| 110.0 values per request rebuilt from a compile-time constant, 12.1% | same run, closing summary. 65.0 literals + 21.0 nullary constructor mentions + 24.0 constructor-closure mentions |
| 33.0 `Record` B-tree nodes per request, 3.6%, at 544 bytes each | same run, `Value::Record — B-tree node` |
| `size_of::<Value>()` = 32; 24 with `Ctor` boxed; 40 with `Ctor.args` as `Arc<[Value]>` | same run, `-- what the 32 bytes are spent on --` |
| narrowing `Value` to 24 bytes saves 7,085 bytes per request and **zero** allocations | same run, `-- what a narrower Value would move --` |
| `Value::Record(1 field)` costs 2 allocations, `[40, 544]` | same run, `the_shape_of_every_value_variant_is_measured` |
| 22 of 34 kernel functions and 386 of 745 lowered nodes inside the codegen fragment | `benches/adr0018-mcts.json`, `accepted_functions` / `accepted_nodes` |
| 81.0% of the kernel's executed work inside the fragment | same file, `fragment_share_measured` |
| 52.58× [50.19–53.52] where the fragment runs | same file, `pure_compute` |
| 0.998× [0.979–1.007] end to end, against a floor of 1.000× [0.994–1.009] | same file, `rungs[3]` and `harness_floor` |
| ceiling 4.86× measured, 5.26× at an infinitely fast fragment | same file, `ceiling_at_measured_ratio` / `ceiling_at_infinite_ratio` |
| 28.34 µs per `ucb` call, 1.35 µs without its square root and logarithm; `ucb` is 62.6% of the kernel | same file, `micros_per_ucb`, `micros_per_ucb_without_sqrt`, `attribution[0].share_of_request` |
| the `read_line` spike at 11.68×, the same conservative minimum ADR 0018 states as 11.67× | `benches/w6-spike-r4.json` — `min(interpreter_best_micros / spike_worst_micros)` over its five inputs, which is not a field in the file. 11.67× over `benches/w6-spike.json` by the same expression, so the half did not move |

The command that writes the second block is at `benches/README.md` §"What
`mcts` adds"; the two are separate instruments and are not mixed anywhere below.

### Which route a lever is judged on, and why the file prints two

The attribution reports `/health` over SimNet **and** a pure-call routing rung,
and they disagree on ranking. Both are printed. A lever here is judged on the
SimNet path, because it is the only one that pays for framing, the host boundary
and the response encode — which is what a served request does. The routing rung
is the one to read for the interpreter proper; on it, argument vectors are
**245.0 of 496.0 allocations, 49.4%**, so §1 ranks first on both routes and
nothing below depends on the choice.

### What is assumed and not measured

Each of these is load-bearing for something below. None is measured today.

1. **That a free list is cheaper than the allocator here.** §1 trades a
   `malloc`/`free` pair for a bounds check, a `Vec` swap and a length reset.
   `crates/ply-eval/src/pool.rs` is the same trade for `Rc` links and it paid,
   but a `Vec` with a capacity class is not an `Rc` link.
   *Settled by:* `ply_corpus::r4::Criteria::max_time_regression` against the
   served-request timing, run before and after §1. ADR 0018 §2 flagged exactly
   this and it is still open.
2. **That the 341.4 transient argument vectors are transient in the sense a
   pool needs** — released before the next call of the same arity wants one,
   rather than held live down a deep recursion. The attribution counts them; it
   does not measure the depth at which they overlap. A pool that misses is a
   pool that allocates anyway.
   *Settled by:* the allocation count after §1, against `Lever::floor`.
3. **That interning a compile-time constant is unobservable.** The argument is
   that no Ply expression can read an address, and `Value::builtin`
   (`value.rs:200`) is the standing precedent — one `Closure` per builtin per
   thread since W6. But `RUNTIME_VERSION` went to `0.11.2` for the constant memo
   and the note on it (`crates/ply-store/src/lib.rs:79-82`) says "No value moves
   — that is the argument for doing it" and then says what moved anyway: the
   calls pending under a second reference, and with them what `E0502` fires on.
   The same sentence is available for this change and would be as wrong.
   *Settled by:* `--engine both` over every corpus on disk
   (`crates/ply-eval/tests/differential_corpus.rs::the_two_engines_agree_on_every_corpus_on_disk`),
   plus `crates/ply-eval/tests/constant_memo.rs` unchanged.
4. **That a record's field count stays small.** §3's flat layout is a linear
   scan; a B-tree is not. Nothing in this repository measures the distribution
   of record widths in a real program.
   *Settled by:* a width histogram, which does not exist and which §3 must add
   before it is accepted.
5. **That the byte columns in the attribution mean anything.** They do not, past
   one window pair. `r4_value_construction::the_per_request_slope_is_the_same_between_the_second_and_third_window`
   prints an allocation slope holding to 1.0% between (20, 200) and (200, 400)
   and a byte slope moving 95.3% — `CONTRIBUTING.md` §"Things known to be
   broken" item 8, reproduced on this path. **No threshold in this ADR is stated
   in bytes**, and `ply_corpus::r4` has no byte field.

## 0. Two things every change below preserves

These are not per-change notes. They bind §1, §2 and §3 alike.

### 0.1 `Value::Secret`'s payload stays unmatchable and unprintable

ADR 0015 §2, and `value.rs:92-103` says why it is a distinct variant rather than
a `Ctor`: a `Ctor` is matchable, and one `match s { Secret(p) -> p }` is a
one-line escape from every guarantee built on it.

Three specific ways a change here could break it, and what catches each:

| the move | what it would do | what catches it |
| --- | --- | --- |
| folding `Secret` into `Ctor` to save the variant, or giving it a tag a pattern can name | makes the payload matchable | `crates/ply-eval/tests/secrets.rs::a_secret_is_never_equal_to_its_payload` and the `E0206` refusals around it |
| a new rendering path that descends into a compound before `Value::write`'s `Secret` arm sees it | prints a credential | `secrets.rs::a_secret_renders_redacted_whatever_it_holds`, `::a_nested_secret_renders_redacted`, `::a_failing_assertion_prints_no_payload` |
| a pool or an intern table that keeps a `Value` after the call that carried it returned | leaves a credential in a buffer the next call reads from | `crates/ply-eval/src/argv.rs::tests::a_secret_handed_back_is_not_held_by_the_pool`, which asserts on the `Arc` count and fails for any keep-without-clear |

The third is new with this ADR and is the one §1 introduces the risk of. It is
armed today, against the seam, before the pool exists.

### 0.2 The store's schema fingerprint moves if the encoding of a stored type moves

`ply_store::schema_fingerprint` (`crates/ply-store/src/schema.rs:265`) is
digested over *encoded exemplars*, so an encoder that starts writing a field
differently moves it even when no type declaration changed;
`schema.rs:385 the_stored_schema_is_pinned` is what fails and says to bump
`FRONTEND_VERSION`.

**Checked, because it decides how much of this ADR is a cache-format change:**
`ply-store` does not depend on `ply-eval` at all — `crates/ply-store/Cargo.toml`
lists `ply-span`, `ply-syntax`, `ply-core`, `ply-hash` and no evaluator — and
`Value` occurs in `crates/ply-store/src/lib.rs` exactly once, in a comment.
`crates/ply-hash/src/normalize.rs` normalizes the AST, not values. **`Value` is
not a stored type.** So none of §1, §2 or §3 moves the schema fingerprint by
changing `Value`'s shape.

What *is* stored is `Value::render`'s **output**: an assertion failure's note is
built by `ply_eval::builtins::assert_failure` (`builtins.rs:1493-1506`) from
`Value::render`, and it is cached as `Outcome::Fail { message }`
(`ply-store/src/lib.rs:361-367`). So:

- A change that moves a rendered byte is a **`RUNTIME_VERSION`** bump, not a
  schema one — a cached `Fail` message would otherwise describe a run this
  evaluator no longer produces.
- A change that moves a stored *type's* encoding is a **`FRONTEND_VERSION`**
  bump and the pin test names it. §3 is the only change here that goes near
  one, and it does not: a record's `Value` layout is not `Type`.

Every change below states which of these it claims, and the claim is checkable
by running the pin test.

## 1. Recycle the call-argument vector — 372.4 per request, 40.9%

> **Corrected in place (R4 build agent, 2026-08-21). The denominator under this
> section was wrong, and it is the second unmeasured premise this ADR has been
> caught carrying.**
>
> This section said, and the paragraph below still says verbatim so the next
> reader can see it: *"At most 31.0 of the 372.4 survive the call as `Ctor.args`;
> the other **341.4 — 37.5% of the whole request** — are filled, handed to the
> callee, emptied into its scope and freed"*, and *"the remaining 341.4 = 37.5%
> of the request are freed by `enter_code`, which binds
> `for (p, v) in params.iter().zip(args)` and drops the Vec"*.
>
> **They are not.** `enter_code` frees **178.0** of them. The reasoning skipped a
> callee kind: `Machine::enter_closure` sends a `ClosureKind::Builtin` to
> `Machine::call_builtin`, not to `enter_code`, and `ply_eval::builtins::call`
> takes its `Vec<Value>` **by value** and consumes it — so that buffer is freed
> inside a function that has no way to hand it back, whatever the seam does. The
> ADR's own list of what the seam does not cover names `Step::Apply`'s arguments
> *out of* `builtins::call` and misses the arguments going *into* it.
>
> The lever was built exactly as specified — the two function bodies, four
> capacity classes, arity 1 through 4 — and A/B'd against the same tree with only
> those two bodies swapped, twice on two instruments. The 372.4 splits four ways,
> and the four sum to 372.4 to the digit:
>
> | | per request | what it is |
> | --- | --- | --- |
> | **recycled by the free list** | **178.0** | taken at `Frame::AppCallee`, given back at `enter_code`. This is what §1 removes. |
> | retained as `Ctor.args` | 31.0 | `enter_closure`'s `ClosureKind::Ctor` arm keeps the buffer; there is nothing to give back |
> | wider than the four classes | 23.0 | arity 5, 6, 7 and 10, left to the allocator by construction |
> | **freed but never given back** | **140.4** | a callee that is not `enter_code` — overwhelmingly `builtins::call` |
>
> Re-take it: `cargo test -p ply-corpus --release --test r4_value_construction
> -- --nocapture` prints that four-way split under
> `-- /health: the argument vectors the free list did not serve, by arity --`,
> and `a_warm_ply_call_takes_its_argument_vector_from_the_free_list` is the
> controlled experiment underneath it — a 1-argument Ply call adds **0.00**
> allocations of 32 bytes per iteration and a 1-argument *builtin* call still
> adds **+1.00**, in one run, on one loop shape.
>
> **What that does to the bar.** The share this section places under the lever is
> 178.0/911.5 = **19.53%**, not 37.5%. `ply_corpus::r4::Lever::floor` was fixed
> before the lever was built and is **20%**, derived as "a little over half" of
> the wrong 37.5%. So `ply_corpus::r4::judge` returns `Verdict::Short` on a lever
> that removed everything the mechanism could reach. **That is a documentation
> defect, not a weak lever, and the floor has deliberately not been edited** —
> editing a pre-registered threshold to make a measurement pass is the one edit
> that would stop the number meaning anything (`CONTRIBUTING.md` §"Measure an
> ADR's motivating claim before accepting the ADR"). `crates/ply-corpus/src/r4.rs`
> is likewise untouched, so `no_levers_floor_is_above_what_the_attribution_places_under_it`
> still passes against the pre-registered numbers; a re-derived floor is a
> decision for whoever amends this ADR, and the number to derive it from is
> 19.53%.
>
> **The 140.4 is the next lever and it is larger than §3.** Recovering the buffer
> a builtin consumes means `builtins::call` taking `&mut Vec<Value>` or draining
> rather than owning, across every arm of a ~100-arm match. That is a change to a
> function signature, not to two function bodies, and it is not claimed here.
>
> Everything below this block is as it was written before the lever was built.

**The number.** The largest single line in the profile, on both routes: 372.4
allocations per request on `/health` over SimNet (40.9% of 911.5) and 245.0 on
the routing rung (49.4% of 496.0). At most 31.0 of the 372.4 survive the call as
`Ctor.args`; the other **341.4 — 37.5% of the whole request** — are filled,
handed to the callee, emptied into its scope and freed. The steady state is a
`malloc`/`free` pair per call.

**The representation.** Not a change to `Value`. A thread-local free list of
`Vec<Value>` in four capacity classes — 32, 64, 96 and 128 bytes, arity 1
through 4 — which covers 349.4 of the 372.4 vectors, **93.8%**, measured by
arity in the same run. `crates/ply-eval/src/pool.rs` is the same mechanism for
`Rc` links, bounded at `KEEP` for the same reason, and its module note is the
model for this one's.

The seam is landed: `crates/ply-eval/src/argv.rs` with

```rust
pub(crate) fn take(arity: usize) -> Vec<Value>;
pub(crate) fn give(args: Vec<Value>);
```

wired at the two sites the attribution names — `Frame::AppCallee`'s
`done:` field (`crates/ply-eval/src/frame.rs:111`) and `Machine::enter_code`
after the arguments are bound into scope (`machine.rs:1830-1833`). Their bodies
are `Vec::with_capacity` and `drop`, unchanged from what was there. **This
change is those two bodies.**

The seam was re-measured after landing and the attribution is unchanged to the
digit — 911.5 allocations per request, 372.4 argument vectors, on the same
command — so the baseline every threshold below is a fraction of was taken
against the tree the build agent starts from, not against the one before it.

Vectors the seam does not cover, and which are therefore not in the 341.4 a pool
can take: the zero-arity path, which allocates nothing already
(`frame.rs:104`); `Step::Apply`'s arguments out of `builtins::call`; and the
continuation-resumption path. Each is a separate site and none is claimed here.

**What it costs.** A bounds check, a class index and a length reset on every
call, against a `malloc`/`free` pair. Assumption 1 above: unmeasured, and the
thing most likely to sink this.

**What it must not break.**

| | |
| --- | --- |
| a pooled vector may not hold a `Value` | it would keep a `Cell` past the region that would reclaim it, defeat `Arc::get_mut` in `value.rs`'s dismantler, and park a `Secret` in a reused buffer |
| a vector handed out may not be non-empty | the callee pushes; a residue shifts every argument |
| the list may not cross a thread | a `Value` is thread-confined (`value.rs`'s note on `RcK`) |
| the vector retained as `Ctor.args` may not also be in the list | two owners of one buffer |
| a release during thread-local teardown may not abort | `pool.rs` takes `try_with` throughout for exactly this |

**The tests that catch it breaking.**

- `crates/ply-eval/src/argv.rs::tests::a_secret_handed_back_is_not_held_by_the_pool`
  — the §0.1 obligation, armed now.
- `crates/ply-eval/src/argv.rs::tests::a_vector_given_back_full_does_not_come_out_of_take_full`
  — the residue.
- The `link_reuse.rs` pattern, which is what a pool's own evidence looks like
  here: `a_warm_frame_push_allocates_nothing` is the assertion to copy, and
  `the_pools_upper_bound_is_stated_in_bytes` is the bound to copy.
- `crates/ply-eval/tests/region_reclamation_audit.rs` and
  `crates/ply-eval/tests/use_after_free_audit.rs` in full: if a pooled vector
  holds a `Cell`, `a_slot_reclaimed_late_reads_nothing_rather_than_the_next_regions_value`
  is where it surfaces.
- `crates/ply-eval/tests/differential_corpus.rs::the_two_engines_agree_on_every_corpus_on_disk`.

**Versioning.** No stored type moves; no rendered byte moves. No
`FRONTEND_VERSION` bump, no `RUNTIME_VERSION` bump. That claim is checked by
`ply-store`'s pin test staying green without being touched.

**The bar.** `ply_corpus::r4::Lever::ArgumentVectors` — attributed share
341.4/911.5 = 37.5%, floor **20%** of the request. Under it, the mechanism fired
on something other than what was counted, and the next step is another
attribution rather than §2.

> **The attribution above is wrong and the block at the head of this section is
> the correction.** The share is 19.53%, the floor is 20%, and the mechanism
> fired on exactly what it could reach. The attribution the floor asks for was
> taken and is the four-way split in that block.

## 2. Build a compile-time constant's `Value` once — 110.0 per request, 12.1%

**The number.** 110.0 allocations per request are values rebuilt from something
the compiler already knows: 65.0 literal `Str`/`Bytes` constructions (7.1%),
21.0 nullary constructor mentions (2.3%) and 24.0 constructor-closure mentions
(2.6%). On the routing rung the same three total 65.0 of 496.0 — 13.1% — so this
ranks second on both routes.

This is R3's pattern: runtime work for a static value. A literal is a
compile-time constant whose `Value` could be built once at lowering, and an
`Arc` clone is a refcount bump — measured at **0 allocations**, printed as
`clone of a Value::List (a refcount bump) 0 allocation(s)` in the same run.

**The representation.** Three sites, two mechanisms, no new type:

1. **Literals.** `NodeKind::Lit(Lit)` (`crates/ply-eval/src/code.rs:44`) becomes
   `NodeKind::Lit(Lit, Value)`, built once at `code.rs:353` where the node is
   lowered, by the existing `interp::literal`. `interp::literal(&Lit) -> Value`
   keeps its signature and its callers; `machine.rs:848` stops calling it per
   evaluation and clones the node's value instead. **`Lit` stays in the
   variant**, and the reason is not the machine: `crates/ply-codegen-spike/src/jit.rs`
   dispatches on it to choose a Cranelift type (`:595-597`) and lowers it
   (`:656`). That crate is outside the workspace and nothing in `cargo test
   --workspace` compiles it, and it has already bit-rotted once from exactly
   this — `code::Stmt::Expr` becoming a struct variant, `CONTRIBUTING.md`
   §"Things known to be broken" item 1. It builds again today under `+1.94.0`,
   which is what made ADR 0018 §1's measurement possible at all, so a build
   agent that widens `NodeKind::Lit` and does not build the spike will break the
   only instrument this project has for pricing codegen.
2. **Nullary constructors and constructor closures.**
   `interp::ctor_value(name: &Symbol, arity: usize) -> Value`
   (`crates/ply-eval/src/interp.rs:1056`) keeps its signature; its body becomes
   a thread-local cache. `Value::builtin` (`value.rs:200`) is the shape to copy
   verbatim, `try_with` and all, and its doc comment already contains the
   argument for why sharing is invisible.

The tree-walker is not changed. Both engines must still agree, and that is the
check rather than a symmetry requirement.

**What it costs.** A `Value` per literal node held for the program's life
instead of built per evaluation — the same bytes `NodeKind::Lit`'s owned `Lit`
already holds, moved rather than added — and a thread-local lookup on a
constructor mention.

**What it must not break.**

| | |
| --- | --- |
| a shared `Str` may not become observable as shared | no Ply operation reads an address; `Value::cmp` answers `Equal` for any two closures, which is why `Value::builtin` was already safe |
| the constant memo's semantics | `RUNTIME_VERSION` 0.11.2 exists because remembering a nullary definition moved what `E0502` fires on. Interning a *value* is not that, and the difference has to be shown, not asserted |
| a simulation's recorded accesses | `Machine::constant` refuses the memo inside a `simulate` region because an allocation is an `Access::Alloc` the search depends on (`machine.rs:1858`). Constructing a `Ctor` is not an `Access` — but if any interning touches a cell, the same rule applies |
| `--engine both` | the tree-walker keeps building per evaluation; the two must still produce the same value |

**The tests that catch it breaking.**

- `crates/ply-eval/tests/constant_memo.rs` in full, unchanged — in particular
  `a_nullary_pure_definition_is_evaluated_once_and_both_engines_agree` and
  `a_constant_built_behind_a_handler_is_remembered_with_its_value_intact`.
- `crates/ply-eval/tests/differential_corpus.rs::the_two_engines_agree_on_every_corpus_on_disk`
  and `::the_two_engines_agree_on_examples`.
- `crates/ply-eval/tests/resumption_semantics_audit.rs::two_resumptions_thread_one_world_rather_than_snapshotting_per_branch`
  — the two-resumption cell reads 2, and a shared constant reached through a
  resumed continuation is where that would stop being true.
- `crates/ply-eval/tests/secrets.rs` in full: a `Secret` is never built from a
  literal today, and an intern table that admitted one would be a credential
  with program lifetime.
- `crates/ply-eval/tests/map_order.rs::the_iteration_order_is_pinned` and
  `::a_second_process_iterates_in_the_same_order` — a shared key is still a key.

**Versioning.** No stored type moves. No rendered byte moves — the same `Value`
renders the same way whether it was built once or a thousand times. So no bump,
and the same check applies: the pin test stays green untouched. If a build agent
finds it needs a `RUNTIME_VERSION` bump, that is a signal something *is*
observable, and it should stop rather than bump.

**The bar.** `ply_corpus::r4::Lever::ConstantValues` — attributed share
110.0/911.5 = 12.1%, floor **7%**.

## 3. A record's fields in one allocation — 33.0 per request, 3.6%

**Not accepted.** Ranked, priced and stated here so that the next reader has the
number; it waits on §1 and §2 landing and on the histogram in assumption 4.

**The number.** `Value::Record(Arc<BTreeMap<Symbol, Value>>)` costs **2
allocations for a one-field record — `[40, 544]`** — because a `BTreeMap`
allocates a whole 544-byte node for the first field. On the request path that
is 33.0 B-tree nodes per request, 3.6%, and 17,952 bytes — the largest byte line
among `Value`'s payloads, though §"What is assumed" item 5 is why no decision
here rests on that.

**The representation.** `Arc<[(Symbol, Value)]>`, sorted by `Symbol`, built
once at construction. `Symbol` is `Arc<str>` with a derived `Ord`
(`crates/ply-span/src/lib.rs:13`), so that sort is lexicographic and does not
depend on intern order — checked, because a field order that varied run to run
would break four things at once and none of them loudly. One allocation, no per-node overhead, field lookup a linear
scan over a contiguous slice.

`Value` stays 32 bytes wide, and that is reasoned from the component table the
same run prints rather than measured: an `Arc<[T]>` is a fat pointer at 16 bytes
against the thin `Arc<BTreeMap>`'s 8, and the widest variant is `Ctor` at 24 —
`Symbol` 16 plus `Arc<Vec<Value>>` 8. 16 is under 24, so the enum's width is
still set by `Ctor`. **Re-take `size_of::<Value>()` before believing that**;
`the_shape_of_every_value_variant_is_measured` prints it.

**What it costs.** Linear field lookup, and a rebuild-on-update where a
`BTreeMap` shares structure. Assumption 4: nobody has measured record width in a
real program, and this change is wrong at 50 fields.

**What it must not break.** `Value::cmp`'s `Record` arm iterates `x.iter()` and
`values_equal`'s compares `x.keys().ne(y.keys())` first — both depend on
**ascending `Symbol` order**, and both would silently answer wrongly over an
unsorted slice rather than fail. `Value::write`'s `Record` arm renders in the
same order, and that output is stored. So the sort is load-bearing at three
places and is the whole risk of this change.

**The tests that catch it breaking.** `crates/ply-eval/tests/map_order.rs`
(a record inside a map key), `crates/ply-eval/tests/secrets.rs::a_failing_assertion_prints_no_payload`
(a record holding a `Secret` renders), the derivation audits under
`crates/ply-cli/tests/derivation_determinism_audit.rs`, and — for the ordering
itself — a new test asserting two records built by different field orders are
one value, which is `map_order.rs::two_insertion_orders_build_one_value` for
records and does not exist.

**Versioning.** This is the one change here that reaches a **stored artifact's
contents**: `Value::render`'s record output is cached in `Outcome::Fail.message`.
If field order moved, that is a `RUNTIME_VERSION` bump. It does not move if the
slice is sorted, and the point of the paragraph above is that "does not move" is
an obligation on the implementation rather than a property of the type.

**The bar.** `ply_corpus::r4::Lever::RecordLayout` — attributed share
33.0/911.5 = 3.6%, floor **2%**.

## 4. Rejected: narrowing `Value`

The milestone's name points here, so the refusal is stated with its number
rather than by omission.

`size_of::<Value>()` is 32 bytes. The same run prints what those 32 bytes are
spent on and what the alternatives would cost:

> ```
>   Value today                                      32
>   with Ctor behind one Arc<(Symbol, Vec<Value>)>   24
>   with Ctor.args as Arc<[Value]>                   40
> ```

`Ctor { name: Symbol, args: Arc<Vec<Value>> }` at 16 + 8 bytes is the widest
variant and the only thing holding `Value` at 32. Boxing it together reaches 24.

**What 24 bytes would buy, measured:** 885.6 `Value`-wide slots per request live
in argument vectors, 28,338 bytes at 32 bytes each; narrowing to 24 saves
**7,085 bytes per request and zero allocations.**

**What it would cost:** an allocation on every applied constructor — 31.0 per
request today — to box the name and the args together. So the change trades a
byte count that §"What is assumed" item 5 says cannot be compared across windows
for an allocation count that can, in the wrong direction.

The other direction is worse: `Arc<[Value]>` for `Ctor.args` removes one
indirection and **widens `Value` to 40 bytes**, and the same run prices its
construction — `Arc<[Value]>::from(an owned Vec)` is **2 allocations `[64, 80]`**
against `Arc::new` on the vector the caller already filled at 1. On the path
where it matters — `enter_closure`'s `Ctor` arm takes ownership of the argument
vector — it is strictly worse.

**Rejected**, and this needs no further measurement: the figure that would
justify it was taken and it is zero allocations.

## 5. What the spike re-pricing changed, and what ADR 0018 now needs

ADR 0018 §1 asked for one measurement before anything was built: re-price the
codegen spike against a compute kernel. It landed as `benches/adr0018-mcts.json`
and `benches/README.md` §"What `mcts` adds". **It does not amend ADR 0018 and
neither does this ADR.** What follows is the list of what an amendment owes.

**The premise held on shape.** 22 of 34 kernel functions and 386 of 745 lowered
nodes are inside the fragment, and **81.0% of the kernel's executed work** is.
ADR 0018 was not wrong that a compute kernel is mostly arithmetic — the
corresponding share for an HTTP request is the 2–5% ADR 0016 states, which is
**not re-taken here**: the spike did not compile against this tree until R4, and
`benches/w6-spike-r4.json` is the first re-take of the `read_line` half
(`benches/README.md` §"And there are now two spike halves" carries the command).

**What the fragment refuses is the roadmap, and it is one thing.** Ranked by the
lowered nodes it takes with it (`benches/adr0018-mcts.json`, `refusals_ranked`):
a **field access**, 7 functions and 253 nodes; a list pattern in a `match`, 2
and 71; unary `-`, 2 and 25; a list literal, 1 and 10. One construct is 71% of
the refused nodes.

**The conclusion it drew from that does not hold.** End to end on the whole
kernel the hybrid is **0.998× [0.979–1.007]** against a floor of 1.000×
[0.994–1.009] — nothing. The reason is structural and is the finding: **the
interpreter cannot call compiled code.** A function the fragment accepts whose
callers it refuses is compiled and never entered; the compiled code reaches
three functions and every `ucb`, `isqrt` and `rollout` under them runs in the
machine. The boundary is not the problem — 102 crossings cost 9.9 µs, 0.017% of
the run.

**And the ceiling is not what ADR 0018 assumes.** Amdahl over the two measured
numbers — the 81.0% share and the 52.58× — gives **4.86×** for a backend that
could be entered from interpreted code, **5.26×** at an
infinitely fast fragment — neither the number ADR 0018 carries for the spike nor
the 52.58× [50.19–53.52] the fragment shows where it does run.

ADR 0018's spike figure is re-taken here rather than quoted, because R4 produced
the first re-take that was possible: the same conservative
interpreter-best ÷ spike-worst minimum is **11.68×** over
`benches/w6-spike-r4.json`'s five inputs and **11.67×** over the older
`benches/w6-spike.json`, so the `read_line` half did not move. The point is that
neither number is the ceiling for a kernel; 4.86× is.

So an amendment to ADR 0018 owes, at minimum:

1. **§2's two motivating sentences corrected in place**, per §"Context" above.
   `CONTRIBUTING.md` §"Correct, do not delete" — keep the original beside the
   measurement.
2. **§2's success criterion replaced.** It reads "a `w6_alloc_sites` re-run
   showing `interp::literal` gone from the top sites". `interp::literal` is
   7.1% of the request. Removing all of it leaves a slope of 911.5 − 65.0 =
   846.5 and an intercept of 34,465 − 925 = 33,540, so **1,014** at the
   200-request window the README's sentence is taken at — and the profile's top
   line unmoved. The argument vector is the top line; see §1.
3. **A new blocker inserted above §2:** a backend the interpreter cannot enter
   buys nothing whatever the representation is. That is a prerequisite for §2,
   §3 and §5 of ADR 0018, and it is not in that document at all.
4. **§2's "and `Float`" priced or withdrawn.** The fragment has no `Float` path,
   compiles `a + b` as `Int` arithmetic whatever the operands are, and fails at
   run time — `crates/ply-codegen-spike/tests/mcts_kernel.rs::the_fragment_accepts_float_arithmetic_and_then_fails_on_it_at_run_time`.
   A census counting such a function as compiled counts one that cannot run.
5. **A lever ADR 0018 does not list, which outranks most of the ones it does.**
   Ply ships no `sqrt` and no `ln` for any numeric type, so the kernel's `ucb`
   computes its own square root by Newton's method over an `ilog2` logarithm:
   **28.34 µs a call against 1.35 µs with those two removed.** `ucb` is 62.6%
   of the kernel's time, so that is **≈2.5× on the whole kernel** — Amdahl over
   `micros_per_ucb`, `micros_per_ucb_without_sqrt` and `attribution[0].share_of_request`,
   three fields of one file — from two prelude builtins and no compiler work.
6. **A safety gap recorded.** Compiled code carries no equivalent of
   `ply_eval::limit`'s bound on nested calls: the same call is a diagnostic in
   the machine and `SIGABRT` in the fragment.

Item 5 is why the sequencing below does not put §2 of ADR 0018 first, and item 3
is why it does not put codegen anywhere.

## The criteria, in code

`crates/ply-corpus/src/r4.rs`. It fixes, before any of this is built: the window
pair a slope may be fitted from, the baseline the shares are fractions of, the
share the attribution places under each lever, the floor each must clear, the
wall-clock regression none may exceed, and the divergence count that reverts one
whatever it saved. `ply_corpus::r4::judge` is the rule and
`no_levers_floor_is_above_what_the_attribution_places_under_it` is the test that
a floor cannot be quietly raised above what was ever counted.

This is `ply_corpus::w6::Criteria::default()`'s pattern and it is here for
`CONTRIBUTING.md`'s reason: a threshold a measurement supplies is a threshold
the measurement cannot fail.

## Sequencing

1. **§1, the argument vector.** Largest line on both routes, the seam is landed,
   and it touches no stored artifact and no `Value` variant.
2. **Re-take the attribution.** Not "run §2 next": if §1 lands under its floor,
   the answer is another attribution, because a mechanism that fired on
   something other than what was counted invalidates the ranking under it too.
3. **§2, compile-time constants.** Independent of §1 and could run in parallel;
   sequenced after only so that each lever's number is attributable to it.
4. **The width histogram in assumption 4**, then decide §3.
5. **Amend ADR 0018** with §5's six items. Its §1 is discharged and its
   ordering is not.

§4 is closed and is not sequenced.

## What would make this ADR wrong

- **If §1 lands and the request's allocation count does not move by 20%.** Then
  the 341.4 transient argument vectors are not transient in the sense a pool
  needs (assumption 2), the attribution's largest line is not a lever, and the
  right response is another attribution — not §2, and not a wider pool.
- **If §1 saves allocations and loses wall clock.** Then assumption 1 is false,
  the trade is real, and every allocation-count target in this ADR — including
  `README.md`'s sentence and the whole `w6` ladder's boxing lever — is measuring
  a proxy that has stopped tracking the thing anyone cares about.
- **If §2 requires a `RUNTIME_VERSION` bump.** Then interning a compile-time
  constant *is* observable, assumption 3 is false, and `Value::builtin` — which
  has been doing exactly this since W6 — is a latent defect rather than a
  precedent.
- **If a build agent has to widen `Value` past 32 bytes to land any of this.**
  Every argument in §4 was made at 32, and 885.6 `Value`-wide slots per request
  live in argument vectors alone. A change that saves 341.4 allocations and
  widens every slot has not been priced by anything here.
- **If §1's seam turns out not to be where the vectors are made.** It is wired
  at `frame.rs:111` on the strength of one controlled experiment —
  `r4_value_construction::a_warm_ply_call_takes_its_argument_vector_from_the_free_list`
  (which read `a_call_allocates_one_argument_vector_of_32_bytes_per_argument`
  until the free list landed and it fired; §1's correction block says why)
  adds one 1-argument call to a loop and watches one 32-byte allocation appear
  under `frame::dispatch`. That licenses reading `frame::dispatch` at a multiple
  of 32 as an argument vector, and nothing else does. If a pool at that seam
  moves fewer allocations than the arity table predicts, the rule table is what
  to doubt first, not the pool.
- **If the two routes' rankings diverge after a lever lands.** They agree on §1
  and §2 today. If they stop agreeing, one of the two harnesses is measuring
  something the other is not, and no verdict may be read off either until that
  is explained.
