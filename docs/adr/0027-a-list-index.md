# ADR 0027 — A list index

**Status:** accepted. It adds **one** builtin, `list_at`, and it refuses four
things: a raising index, a `list_head`, a `list_last`, and — on a gate fixed
before the measurement and missed by it — the `list_at_or` this record was
drafted to add beside it.
**Date:** 2026-08-30.
**Corrects in place:** `docs/adr/0024-ownership-as-a-checked-property.md` §10;
`docs/adr/0025-ownership-design.md` §Soundness; `docs/adr/0020-self-hosting-the-front-end.md`
§4.4; `docs/adr/0022-the-call-ceiling.md` §0; `CONTRACTS.md`'s prelude
enumeration and its builtin tables; `CONTRIBUTING.md`'s ADR count and its "where
a change is likely to bite" table; `docs/GUIDE.md` §5.4, §6.7, §6.9, §13.2,
§19.4 and §20.
**Corrects itself in place, three times:** §2 argument 2 said a raising index
would block `property`; this record then struck that and said it blocks `proved`
instead; **the strike was wrong and is itself struck** — a raising index blocks
`property` today, by raising during randomized execution rather than through
`TOTAL_BUILTINS`, and it is the `proved` cost that is inert. §7 refuses the
second builtin this record was drafted to add.
**Closes:** `spikes/ply-parser/GAPS.md` §1, ranked **first of fifteen** by the
parser spike, and the three needs it files separately (an index, a `head`, a
`last`). The spike itself is **not** ported here — see §8.
**Constrained by:** ADR 0001 (a definition's identity is its normalized
structure), ADR 0012 §A5 (which names reserve), ADR 0019 (`Value::List` is
`Arc<Vec<Value>>`), ADR 0022 (the precedent for adding one builtin and refusing
one thing), ADR 0025 (the `Vector<T>` gate this gives a term to).
**Two holes it leaves open, named rather than papered over:** an arity in
`Builtin::arity()` that is too *wide* is gated by nothing (§5 — the record first
said "gated by nothing" unqualified, which is corrected there), and this ADR's
`TOTAL_BUILTINS` entry is unarmed (§2).

---

## §1 The gap

`spikes/ply-parser/GAPS.md` §1:

> a parser *is* `tokens[pos]`, `tokens[pos + 1]`, `tokens[pos + 2]`:
> `parser.rs::kind_at` is called with 0, 1 and 2, and `at_law_start` uses all
> three in one predicate.

The list surface was `len, push, map, filter, fold, range, iterate` and nothing
else — no `nth`, no `head`, no `tail`, no index. So the Ply parser's token
buffer could not be a `List` at all. It folded `lexer.ply`'s `List<Token>` into
a `Map<Int, Token>` — the only random-access container in the language — and
every peek became a red-black descent with a `Value::cmp` at each node, plus an
`Option` to unwrap, where the reference does one bounds-checked load.

The lexer spike had ranked the same absence **tenth** of fifteen, "starts to
bite". The difference between tenth and first is not opinion: a lexer walks
forward and a parser looks around.

Four workarounds were counted in the shipped tree before this change, and each
is a hand-written index under a different name: `crates/ply-std/ply/json.ply:800
nth`, `crates/ply-std/ply/db.ply:604 nth`, `db.ply:610 last`, and the spike's own
`tok_index`. Two more accessors in the same family are hand-written over `Bytes`
for the same reason (`json.ply:167 at`, `db.ply:751 byte_at`).

## §2 The surface, and why it is not `bytes_at`'s

```ply
list_at<a>(xs: List<a>, i: Int) -> Option<a>
```

**Total**: `None` for a negative index and for one at or past the end. Pure,
monomorphic in arity, and in `TOTAL_BUILTINS`.

This record was drafted with a second builtin beside it,
`list_at_or<a>(xs: List<a>, i: Int, default: a) -> a`, and §7 is where it was
refused. The argument below for the *convention* — total rather than raising —
is the argument that survives; the argument for the *pair* is in §7 with the
number that ended it.

The language already contains two conventions for an accessor and the choice
between them is the real decision here. `bytes_at` is the precedent and it
**raises** (`crates/ply-core/src/infer.rs`, `(Bytes, Int) -> Int`). `map_get`
answers an `Option`. Four arguments settle it, all from the tree rather than
from taste.

**1. No caller in the tree wants raising.** Every hand-written indexer already
shipped is total. `json.ply:167 at` and `db.ply:751 byte_at` are both
`if i < 0 || i >= bytes_len(src) { -1 } else { bytes_at(src, i) }` — the
language's one raising index, wrapped in a total function at its heaviest use
sites. The spike's `tok_index` goes further and *clamps*, so a peek past the end
answers the last token. A raising `list_at` would have shipped a builtin whose
first act at every use site is to be wrapped, and that wrapper is exactly the
"three calls" `json.ply:1676` complains about.

**2. `TOTAL_BUILTINS` prices it — but by less than this ADR first claimed, and
the correction is measured.** `crates/ply-prove/src/prove/lower.rs` explains why
`map_get` is in that list — *"a lookup answers an `Option` rather than raising"*
— and why `bytes_at` is not. A callee that is not total reaches
`Lowering::undefined`, which requires `false`, so the **static** tier can never
close an obligation containing one. A raising list index would therefore be
excluded from `proved` at every peek.

~~This ADR was drafted saying that a raising index would block *`property`*.
That is **wrong** and is corrected here rather than quietly dropped: `property`
is randomized execution and never reaches `lower` at all. The cost is to
`proved`, which is a real cost and a smaller one.~~

> **That correction is itself withdrawn (2026-08-30, adversarial review of this
> change), and it withdrew the true half.** The struck paragraph is quoted above
> verbatim rather than deleted, because the mistake in it is worth more than the
> sentence: it reasoned from one mechanism (`lower`) to a conclusion about a
> tier that is reached by another mechanism (execution), and so talked itself
> out of the only argument in this section that is live today.
>
> A raising index **does** block `property`, and it does so now, on the shipped
> binary. The route is not `TOTAL_BUILTINS`. It is that a `property` runs the
> term on randomized inputs, one of them is out of range, the term *raises*, and
> the obligation becomes **`unattempted`** — `W0604`, which `docs/GUIDE.md`
> §11.3 calls "a **gap**, not a weak tier: never green, never cached". The
> definition is then reported as not covered by any claim that holds.
>
> ```ply
> fn peek(xs: List<Int>, i: Int) -> Int =
>   match list_at(xs, i) { Some(v) -> v, None -> 0 }
>
> fn bpeek(b: Bytes, i: Int) -> Int = bytes_at(b, i)
>
> law "list index congruence"  forall (xs: List<Int>, i: Int) { peek(xs, i) == peek(xs, i) }
> law "bytes index congruence" forall (b: Bytes, i: Int)      { bpeek(b, i) == bpeek(b, i) }
> ```
>
> ```
>    1 not covered by a claim that holds: p.bpeek
>    ✓ property    law "list index congruence"    200 cases · 0 rejected
>    ~ unattempted law "bytes index congruence"   raised: `bytes_at` index 0 is outside a value of 0 bytes
>        shrunk to b = b"", i = 0
> ```
>
> **This argument is now gated rather than only argued.**
> `crates/ply-cli/tests/tiers.rs::a_total_index_reaches_property_where_a_raising_one_is_a_gap`
> runs the two laws above as one module and asserts both halves — `property` for
> the `list_at` arm, `Unattempted(Gap::Raised)` for the `bytes_at` control — so
> a `list_at` that quietly began raising is a red test rather than a paragraph
> nobody re-ran. It was seen to fail: replacing `Builtin::ListAt`'s arm with the
> `out_of_range` diagnostic `bytes_at` uses turns the first assertion red at
> `xs = [], i = 0`. That corruption also demonstrates argument 4 below without
> being asked to — the diagnostic it produces reads *"`list_at` index 0 is
> outside a value of 0 **bytes**"*.
>
> Guarding the raising accessor — `if i >= 0 && i < bytes_len(b) { bytes_at(b, i) } else { 0 }`
> — puts it back at `property`. So the tier cost of the raising convention is
> paid at *every unguarded peek*, and the price of avoiding it is exactly the
> wrapper argument 1 counts. **The cost to `proved` is the one that is
> currently inert** (next paragraph), and the cost to `property` is the one that
> bites; the draft had them the right way round and the correction inverted
> them. Argument 2 is therefore restored to weight — on the `property`
> mechanism, not the `TOTAL_BUILTINS` one.
>
> `CONTRACTS.md`'s host-boundary section already said the surviving thing —
> *"a raising list index would be excluded from `TOTAL_BUILTINS`, which would
> block a `property` over any function that peeks, at every peek"* — so that
> sentence was left contradicting this one for the length of this change. It is
> right about the outcome and wrong about the mechanism, and it is corrected
> there in place too.

**The `TOTAL_BUILTINS` half of argument 2 is, on lists, currently a cost of
nothing — which was found by trying to arm it, and which is what the withdrawn
correction above should have said instead of trading the live half away for it.** Removing `list_at` from `TOTAL_BUILTINS` and re-running changed no
tier in any law that could be written over it. The reason is one line wide:

```
law "plain arithmetic reaches the static tier" forall (i: Int) { i + 0 == i }
   ✓ proved      congruence · 1 steps
law "len of a literal reaches it too" forall (i: Int) { len([10, 20, 30]) == 3 }
   ✓ property    200 cases · 0 rejected
```

`len` has been in `TOTAL_BUILTINS` since W1, and a law over a `List` literal
still does not reach the static tier. **No `List`-valued term is in the
decidable fragment today**, so `TOTAL_BUILTINS` membership for `len`, `push` and
now `list_at` is correct and inert. The entry is kept — it is true, it matches
`map_get`, and it is what a later fragment with a list theory would need — but
it is recorded as an **unarmed** change, not as a gate. That is a finding about
the tree rather than about this feature. ~~It is the reason argument 2 is now
third in weight rather than first.~~ **Not any more**: the block above restores
argument 2 to weight on the `property` mechanism, which is armed and
demonstrable; only its `TOTAL_BUILTINS`/`proved` half is inert.

**3. The language is already moving the other way.** `bytes_at` is W1-era. Every
later accessor — `bytes_index_of`, `bytes_index_of_from`, `bytes_index_of_byte`,
`bytes_position`, `map_get`, `int_of_decimal` — answers an `Option`. `bytes_at`
is the precedent that exists, not the precedent that is being followed.

**4. A small tell.** `crates/ply-eval/src/builtins.rs`'s one shared
out-of-range diagnostic hardcodes *"outside a value of {len} **bytes**"*. The
code is not built to grow a second raising index.

**Against `Option` alone.** The `Option` is part of what the spike is
complaining about — "an `Option` to unwrap" — and it does not go away. `Some(v)`
is a `Value::Ctor` with a `Symbol` and an `Arc<Vec<Value>>`, so it is a clone and
an allocation per peek, which is what `map_get` allocates today. `list_at` alone
removes the descent and the `Value::cmp` chain and keeps the allocation and the
`match`.

**The case that was made for a pair, kept because §7 refuted it rather than
ignored it.** The tree has written both halves by hand and named the missing
builtin. `crates/ply-std/ply/json.ply:1676`, in the shipped standard library:

> *"`at` is three calls where a `bytes_at_or(b, i, default)` builtin would be
> one, and it is the most frequent thing the parser does. That is a constant
> factor and it is not in ADR 0012's builtin table; **it should be measured
> before it is added**."*

That is a pre-registered request, from the language's own standard library, for
exactly the `_or` shape — and it is a request *to measure*, which is what §7
did and what the answer was. The pairing precedent (`string_contains` beside
`string_find`, `bytes_is_utf8` beside `string_of_bytes`) is real but is a
precedent about *safety*, not about a constant factor; neither of those pairs
was justified by a speed claim.

## §3 `list_head` and `list_last` are refused, and that is a finding

`GAPS.md` §1 files them as three separate needs. An index alone removes all
three. `head` is `list_at(xs, 0)`. `last` is `list_at(xs, len(xs) - 1)` — and
the spike's problem was never that `last` was unspellable, it was that with no
index the only spelling was a `fold`, i.e. a quadratic hidden inside a
deduplication rule. `len` on an `Arc<Vec>` is O(1), so the second spelling is
not a traversal. One primitive, three workarounds gone; two names not added.

## §4 Negative indices are absent, not counted from the end

`list_at(xs, -1)` is `None`, not the last element. A reader arriving from Python
will expect otherwise, which is why `docs/GUIDE.md` says it in three places
(§6.7, §13.2, §19.4) rather than one. The reason: counting from the end reads
well until an arithmetic slip turns an intended index negative, at which point
the program gets an element rather than the `None` that would have named the
mistake. It is not clamping either — an out-of-range index is absent, not the
nearest element, which keeps the GUIDE's existing "slices and indices are never
clamped" true of lists too.

## §5 What the engines and the compiled fragment do with it

**The two engines cannot disagree, by construction.**
`crates/ply-eval/src/builtins.rs` holds one definition per builtin that both
engines run; the machine reaches it at `machine.rs::call_builtin`, the
tree-walker at `interp.rs::call_builtin`, and neither has a private table. A
non-higher-order builtin answers `Step::Done` and nothing else, so it never
pushes a frame and never suspends — which is the whole of what the two engines
implement differently. `--engine both` over these builtins is therefore a run
beside a construction argument and **not a gate**; reporting it as a passing gate
would be this project's signature defect wearing the other hat. It was run.

**`compiled.rs::admit` never sees a builtin as a callee.** `Gate::NotLoweredCode`
is the first gate and its doc names the refusal set as "a tree-walker closure, a
constructor **or a builtin**". A *user definition* calling `list_at` internally
can be admitted, but only if its own arguments are `Int`/`Bool`: `crossable` is
`matches!(value, Value::Int(_) | Value::Bool(_))`. So on the shape that motivates
this feature — `kind_at(toks, i)`, with a `List` parameter — `Gate::ArgumentShape`
refuses the call before the row is looked up, for a reason that has nothing to do
with `list_at`. **The compiled fragment and this feature do not meet on the
workload that motivates the feature.**

> **Withdrawn 2026-08-31: they meet, and on exactly that shape.** The bolded
> sentence and the two before it are no longer true of this tree. `crossable`
> grew `Bytes` (2026-08-30) and then the argument test stopped being a value
> test at all: `compiled::Gate::ArgumentType` decides an argument from the
> definition's **declared parameter type**, so a `List<Token>` parameter is
> carried when `Token` is — which it is, being a record of `Int`s and a
> constructor.
>
> Measured on the ported front end (`spikes/ply-parser` parsing `examples/`,
> `PLY_SEAM_CENSUS=1 ply test <dir> --no-cache -j 1 --filter probe.parse`), the
> two definitions this ADR is named for are now among the most-admitted in the
> whole corpus: **`spine.tok_index` 232,041 and `spine.tok_at` 231,729 admitted
> calls**, both taking the `Ctx` record whose `toks` field is the `List<Token>`
> this feature indexes. The seam's admitted share on that workload goes from
> **12.205%** of body calls to **84.014%**.
>
> What is unchanged is the sentence's *reason* one level down: `list_at` is a
> builtin, `Gate::NotLoweredCode` still refuses it as a callee, and
> `jit.rs::admissible_builtin` still admits it only through its `_ => Ok(())`
> arm. The two features meet because the **caller** became admissible, not
> because anything about `list_at` did. ADR 0030 §6's amendment carries the
> before/after.

Where a definition *is* admitted and happens to contain a `list_at`,
`crates/ply-codegen-spike/src/jit.rs::admissible_builtin` admits it through its
`_ => Ok(())` arm and it runs through `rt.rs::rt_builtin` →
`ply_eval::builtins::call` with `Span::DUMMY`. Two notes. The spike already
carries `rt.rs::rt_list_at` for list *patterns*, so the same operation now exists
by two routes, one span-carrying and one not. And because `list_at` is total,
**no diagnostic can originate from it inside a compiled body at all** — which is
the `Span::DUMMY` weakness `bytes_at` has under the spike today, and which a
raising list index would have widened. That is a fourth argument for the total
form and it only shows up if you read `rt_builtin`.

**A gate this change shipped vacuous, and the review that found it.**
`costs.rs::result_owner` gains `Builtin::ListAt => Owner::Blocked(Cause::Element)`,
because the fallback is `_ => Owner::Fresh` and without the arm `ply review`
claims an in-place append onto an element the list still holds — a wrong claim,
which is the only kind of thing `costs.rs` can get wrong.
`ownership_checker_armed::an_append_onto_an_indexed_element_is_flagged_and_the_counters_confirm_it`
was written to arm it and **did not**: its program wrote the append as
`len(push(row, i)) + touch(...)`, which the *position* rule flags on its own, so
the verdict was `Copies` with the reason *"the scope binding `row` is still held
by an enclosing frame"* both with the arm and without it, and deleting the arm
left the test green. The program now puts the `push` in a helper's tail, where
the position rule has nothing to say, and the test asserts that the reason
**names `list_at`** — a right answer reached by the wrong rule being precisely
what the first version could not tell apart. Two corruptions were then seen to
go red: deleting the arm (verdict flips to `Reuses` while the run copies 60
times) and keeping it under `map_get`'s cause with a reason that does not name
the index.

**A hole this change closed.** `Builtin::all()` had no completeness check. Four
tests iterate it — the reachability pin, the two callback pins and the arity
check — so a variant *missing* from `all()` is never named and therefore never
checked by any of them. Deleting `Builtin::ListAt` from `all()` was run against
the reachability test on the assumption it would go red; it stayed **green**.
`builtins::tests::builtin_all_is_complete_and_lists_each_name_once` now pins the
whole name list, and the corruption was then seen to fail against it.

**A hole this change did not open and did not close.** `Builtin::arity()` is a
second hand-maintained table beside the prelude scheme, and ~~**nothing in the
tree gates its value**~~ — **the hole is one-directional, and this record
overstated it (corrected 2026-08-30 on review, which ran the other
corruption).** `builtins::call` reads `b.arity()` on every call
(`builtins.rs:558`; `region_kind.rs:1086` and `value.rs:169` read it too), so an
arity *narrower* than the truth is caught loudly: giving `ListAt` `(1, 1)`
reddens five of the six tests in `crates/ply-eval/tests/list_builtins.rs`, at
run time, under the tree-walker. What nothing gates is an arity that is too
*wide*, because no well-typed call can reach the extra slot to meet it. That is
exactly the drift the paragraph goes on to describe — `assert` and `range` are
both `(1, 2)` over schemes of 1 and 2 arguments, i.e. both too wide — so the
hole is real, it is the shape the tree has already fallen into twice, and it is
half the size this record claimed. As drafted: giving `ListAt` an arity of
`(2, 3)` leaves
`every_builtin_checks_its_argument_count` green — that test asserts the declared
arity is *enforced*, not that it is *right* — and leaves the `ply-core` scheme
test green too, because the scheme is a different table. The two have already
drifted: `assert` is `(1, 2)` and `range` is `(1, 2)` in `Builtin::arity()`
while their schemes take 1 and 2 arguments, so `assert(c, "msg")` and `range(5)`
are both **`E0202`** and the second leg of each arity is unreachable from any
well-typed program. That is why a general "arity agrees with the scheme" test
cannot simply be written, and it is recorded in `CONTRIBUTING.md`'s "where a
change is likely to bite" table rather than fixed here.

## §6 No `DefHash` moves. Three versions bump anyway, and the reason is sharper

*Verified, not argued.* `crates/ply-hash/src/normalize.rs::value_ref` resolves a
bare name against the `ProgramIndex`; the prelude is not in it, so a builtin call
is `free_ref` — `tag::FREE` plus the name string — and
`ply-hash`'s `builtins_and_unknown_names_are_not_dependencies` pins exactly that.
Nothing in the tree names either symbol at the Ply level. **`ply hash --json`
over `examples/`, `crates/ply-std/ply/` and the parser spike's modules, taken
with a pre-change binary and again with a post-change one, is byte-identical
across 2,386 definitions.**

**And that is precisely why the versions must bump. The hazard is not a hash that
moved; it is a hash that did not.**

```ply
fn f(xs: List<Int>) -> Option<Int> = list_at(xs, 0)
```

hashes to the same bytes before and after — `tag::FREE "list_at"` either way
— and means two different things: `E0101 UNKNOWN_NAME` before, a value after. A
cached interface, fingerprint or `Pass` under that hash is a claim about the old
meaning. This is ADR 0022's own argument, quoted there as *"a cached `Pass`
written before `iterate` existed is a claim about a program in which the name
meant nothing"*, applied to a change that adds no type and so has no `E0105`
story of its own.

- `FRONTEND_VERSION` 0.16.0 → **0.17.0**: one bare name gains a stored type.
- `RUNTIME_VERSION` 0.12.0 → **0.13.0**: the result cache, same argument.
- `PROVER_VERSION` 0.5.0 → **0.6.0**, and **here this ADR diverges from ADR 0022
  rather than following it.** 0022 declined the prover bump with an argument. I
  take it with a different one: `list_at` joins `TOTAL_BUILTINS`, which
  `ply-store/src/lib.rs` says explicitly to bump for — *"any change to the
  fragment, to a rule's meaning"*. No *existing* obligation can change its answer,
  for 0022's reason (an obligation mentioning the name did not check at all
  before), so this re-attempts obligations and re-runs no test. It is a bump for
  the rule's meaning, not for any obligation's answer, and it is recorded here as
  a decision so that nobody later discovers it as a side effect.

**The name is not reserved.** ADR 0001 §"Resolution order" as amended by ADR 0012
§A5 reserves exactly three (`cell_get`, `cell_set`, `compare_values`), each for a
reason this does not have. So a module's own `fn list_at` shadows the builtin —
which keeps `json.ply` and `db.ply` compiling untouched, and is also why the name
is not `nth`: those two modules each already define one, and a bare `nth` would
have been *legally shadowed* into invisibility in exactly the two files that most
wanted it.

## §7 What the measurement said, and the builtin it refused

Pre-registered in `/tmp/ply-list-index/PREREGISTRATION.md` before any number
existed and outside this repository, with the statistic, the run count, five
numbered predictions and the decision rule. The outcome is appended to that same
file below its own line, every run printed and none discarded.

Two rules were fixed before the number. **`list_at` ships regardless** — the
workarounds §1 counts are code, not time, and no measurement can un-need them.
**`list_at_or` ships if and only if it is at least 1.5× faster per peek than
`list_at`**, at 14,742 tokens, on min user CPU.

### The rig, and the two defects it had to be repaired for

Seven probes per size, each a generated project holding the same six spike
modules so module typechecking is identical and cancels: a control, the lex, the
`Map` build, a driver with no peek in it, and one arm per peek implementation
(`map_get`, `list_at`, `list_at_or`, and the recursive `nth` that `json.ply` and
`db.ply` already ship).

The pre-registration said *"the sweep driver is identical in all four `K` arms …
so the driver cancels in `K − B`."* **It does not** — `K − B` is driver *plus*
peek — and the first sitting is kept in the outcomes file as the record of that.
Worse, subtracting across arms whose *builds* differ produced an incoherent
answer: a red-black descent measuring **cheaper** than an array load. The
instrument was repaired, in an amendment written before the second sitting's
first run and changing no bar: each arm is measured at **R = 1** and **R = 21**
sweeps, so a peek is a **within-arm** difference, `(T21 − T1) / (20 × ntok)`, in
which every byte of the program that is not the sweep is identical and cancels
exactly.

### The numbers

Min user CPU, µs per peek, spreads 1–3% at the registered size, one
`/usr/bin/time -p` tick = 0.034 µs there:

| ntok | `map_get` | `list_at` | `list_at_or` |
| ---: | ---: | ---: | ---: |
| **14,742** | **1.696** | **1.662** | **1.323** |
| 128,000 | 1.777 | 1.621 | 1.516 |

| | registered | measured at 14,742 | |
| --- | --- | ---: | --- |
| P1 | `list_at_or` ≥ 2× `map_get` | 1.28× | miss |
| P2 | `list_at` ≥ 1.3× `map_get` | **1.02×** | miss |
| P3 | `list_at_or` ≥ 1.5× `list_at` | **1.26×** | miss — **the gate** |
| P5 | `Knth` cannot complete the sweep | DNF at 90 s | hit; mechanism wrong |

**What the tick actually resolves, added on review.** The pre-registration's
sitting-2 note says one tick is *"10× smaller than the smallest difference
claimed below"*. That is true of the **gate** and not of the headline. A derived
per-peek figure is a difference of four readings each quantized to 0.01 s, so a
comparison *between two arms* rests on eight of them: worst case ±4 ticks.

| at 14,742 | difference | ±4 ticks | resolved? |
| --- | ---: | ---: | --- |
| P3, `list_at` − `list_at_or` (the gate) | 0.339 µs | 0.136 µs | yes, 2.5:1 |
| P2, `map_get` − `list_at` (the headline) | **0.034 µs** | 0.136 µs | **no** |

At 128,000 tokens one tick is 0.0039 µs, ±4 ticks is 0.016 µs, and the
`map_get` − `list_at` difference is 0.156 µs — resolved 10:1, at **1.10×**. So
the honest statement of the headline is that the two containers are within about
a **tenth** of each other, measured where the instrument can see it, and
indistinguishable at the registered size. "The same to within 2%" is a point
estimate quoted to a precision this rig does not have, and the 2% comes from the
*less* resolved of the two sizes. Nothing about the conclusion changes — a tenth
is still not the cost `GAPS.md` §1 described — and nothing about the `list_at_or`
gate changes either: 1.26× ± 0.1 is below 1.5× at 14,742 and 1.07× at 128,000 is
below it with room to spare. The claim that moves is the *precision*, not the
verdict.

### `list_at_or` is refused

1.26× against a 1.5× bar, and 1.07× at 128,000. The bar was fixed before the
number and is honoured against it. A peek in this evaluator is ~1.7 µs and is
**almost entirely interpreter dispatch**; the `Some` allocation and the `match`
that `_or` removes are 0.34 µs of that — real, and not a second name's worth.
`json.ply:1676` asked for this to be *measured* before it was added; it was, and
the answer is no.

### The headline is a withdrawal, and it was registered as one in advance

**`spikes/ply-parser/GAPS.md` §1's *cost* claim does not hold.** P2 is **1.02×**:
a `map_get` peek and a `list_at` peek cost the same to within 2% at 14,742
tokens and within 10% at 128,000. The `Map<Int, Token>` the parser spike was
forced into was **not** materially more expensive than the index it wanted, and
neither arm's per-peek cost grows with n (1.03× and 1.05× over an 8.7× increase),
so the O(log n) descent is invisible against dispatch — which withdraws the
*mechanism* half of §1 as well.

The pre-registration §4 registered this outcome in advance: if P1 failed, the
cost claim is withdrawn with a number beside it and **the feature ships on §1's
code cost** — four hand-written workarounds, a container the spike did not want,
and two extra fields threaded through ~93 functions. That is the case for
`list_at`, and it is a smaller case than the one that motivated it. It is stated
that way here rather than quietly rescoped.

**`Knth`, and a mechanism I got wrong.** P5 predicted the hand-written `nth`
would exceed the 10,000-call budget and answer a diagnostic. It does not finish
at all — at 14,742 tokens or at 2,000 — inside a 90 s cap, and it completes at
541 tokens in 1.59 s against 0.09 s for every other arm. `[x, ..rest]` allocates
the tail at every step, so one `nth(xs, i)` is O(n·i) rather than O(i) and the
sweep is O(n³). The prediction's outcome was right and its reason was wrong, and
that is a sharper argument for `list_at` than the timing was: the workaround the
standard library ships is not slow, it is asymptotically unusable.

## §8 What this deliberately does not do

- **The spike is not ported.** `spikes/ply-parser/` is off-limits to this change
  by instruction and porting it is a separate one. So the tree gains a builtin
  with **zero call sites**, which is the shape ADR 0025's ill-posed criterion
  warns about, and it is filed as explicit follow-up rather than smuggled in
  here: `json::nth`, `db::nth`, `db::last` and the spike's `Map<Int, Token>` are
  the four call sites waiting.
- **No `bytes_at_or`.** `json.ply:1676` asks for it and §7 has now answered the
  question it asked — a defaulting form saves the `Some` and the `match`, which
  on a `List` is 0.34 µs of a 1.66 µs peek. That is a strong prior that
  `bytes_at_or` would not clear a 1.5× bar either, and it is a prior rather than
  a measurement: `bytes_at` *raises*, so its total wrapper is three calls and
  not one, which is a bigger saving than the one measured here. Somebody should
  take it under its own registered gate.
- **No `List` representation change.** ADR 0025's `Vector<T>` gate is untouched;
  §Soundness there is corrected in place to say that the gate now has an index
  cost to be a cost *of*.

## §9 What would make this ADR wrong

- **The measurement did not support the motivation, and §7 is that.** Both of
  the things this bullet was drafted to warn about happened. If a later reader
  finds a workload where the `Map` *is* materially more expensive, §7's numbers
  are the ones to argue with — they are one machine, one interpreter, one load
  band, and they price a peek rather than a parser.
- **If the evaluator's dispatch cost falls.** §7's whole finding is that a peek
  is ~1.7 µs of dispatch and ~0 of container access. A compiled backend
  (ADR 0026) changes the denominator, and the `list_at_or` refused here becomes
  worth re-opening the moment dispatch stops dominating — under the same 1.5×
  bar, re-measured, not re-argued.
- **If ADR 0025's fallback lands.** A chunked structurally-shared vector makes
  `list_at` O(log₃₂ n) where it is O(1) today. §7 says that matters less than it
  looks — a red-black descent over 128,000 entries is already invisible against
  dispatch, so a 32-way one certainly is — but it is not the O(1) a reader might
  have been promised, which is why the GUIDE says "constant time on today's
  representation" and cross-references ADR 0025 rather than saying O(1).
- **If the code cost §1 counts turns out to be small.** That is now the *whole*
  case for this builtin, §7 having removed the time one. It rests on four
  hand-written indexers in the shipped tree and on a spike that could not use a
  `List` as a token buffer at all; if a later reader thinks those were cheap,
  this ADR is what to argue with.
