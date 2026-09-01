# ADR 0020 — Self-hosting the front end

Status: **rejected for now, with two pieces of the spike kept.**

**Why anyone wanted this:** [ADR 0021](0021-why-bootstrap.md). This document
prices whether Ply can host its own front end and decides that it cannot yet. It
does not state the goal, **and read alone it is a rejection with nothing behind
it.**

- **Rejected:** writing Ply's front end in Ply on today's interpreter. §6 prices
  it. **The blocker is throughput, not expressiveness**, and the evidence for it
  has strengthened.
- **Accepted:** the differential-harness pattern in `spikes/ply-lexer/harness` is
  the right shape for pricing any future port, with §3.2's amendment. Re-used,
  and it held: `spikes/ply-parser/harness` is built on it.
- **Decided elsewhere:** whether the fragment should grow to cover a front end —
  [ADR 0026](0026-a-reachable-backend.md).
- **Closed:** §1's shipped defect, fixed in §7 item 3.

## What has happened since, and what a reader should not trust

This document was written by re-running the spike rather than by reading its
write-up. Five later documents have moved parts of it, and **the *pricing* in §6
is the reason anyone reads this ADR. Read this before §6.**

| what moved | where | now |
| --- | --- | --- |
| §5.1's premise — that a recursive-descent parser recurses per element | ADR 0022 | **refuted.** `parser.rs` drives every sequence with a loop; `iterate` gives Ply the same shape at depth 1 |
| §6.1's throughput figures | `spikes/ply-parser` | **do not reproduce.** Both engines several times faster than recorded; the *ratio* holds |
| §6.2's multiplier, assumed | `spikes/ply-parser` | **measured**, for two of six phases |
| §6.2's premise — lexing is a tenth to a fifth of front-end time | `spikes/ply-parser` | **wrong.** It is a third after two phases |
| §6.3's ratio on `read_line`-shaped code | ADR 0026 | **`read_line` cannot cross the seam.** §6.3's arithmetic rests on a function `admit` refuses on its first line |
| §7 item 2, "a lint … is the only one" | ADRs 0024, 0025 | **refuted.** The lint was built and failed in both directions |
| §4.1's `push` mechanism | mechanism sweep | **`push` does not copy a `List`.** Sharing decides it, and sharing is decided by position |

**The decision is not one of the things that moved, and the case for it is
stronger than this document makes it.** §6.1 priced one phase; two phases now
measure at a multiple three times larger, with no extrapolation. Redoing §6.2's
own arithmetic on the re-taken lexer term gives a band that overlaps the one
below: **both absolute halves halved and the ratio did not move.**

---

## §0 What this reviewed, and the instrument problem it started with

The spike ports `crates/ply-syntax/src/lexer.rs` to Ply and compares the two
lexers token by token over a corpus of `.ply` files. Its deliverable is
`GAPS.md`.

**The first thing found was not about lexers.** `crates/ply-eval/src/frame.rs`
carried a modification timestamp minutes newer than every other source file on
the tree, and `target/release/ply` — the binary behind every wall-clock number in
`GAPS.md` — was built a minute after that. `frame.rs` holds half the
`ply_eval::rc::carry` call sites, and `carry` is the mechanism `GAPS.md` §1 rests
on. **The change was an unattributed edit making a field projection out of a
uniquely-owned record *move* the field rather than clone it: the precise
operation §1 measures.**

So §1 was measured with an instrument that had been altered in the place it was
measuring. **That is the W1/M8 shape — a result whose provenance nobody checked
because the result looked right.** It was re-taken three times on a clean binary,
by two parties, and it survived.

**The lesson is worth more than the outcome. The finding was correct and the
instrument was not, and only one of those two was checked before the work was
merged.** Nothing in this repository's loop would have caught it: CI's spike job
reaches `crates/ply-codegen-spike` and nothing in it reaches `spikes/ply-lexer`,
whose harness declares its own `[workspace]`; `cargo test --workspace` does not
reach `spikes/`; and a modified working tree is invisible to every command in
`CONTRIBUTING.md` §"The loop".

**And this hazard turned out to have a second, worse form.** The rule this
project used for checking a binary against its tree —
`find crates -name '*.rs' -newer target/release/ply` — **cannot see an edit to a
stdlib module at all**: `crates/ply-std/src/lib.rs` `include_str!`s every
`crates/ply-std/ply/*.ply` into the binary, **so editing one changes what
`import std.http` means and moves no `.rs`.** A later workstream lost a headline
count to exactly that. Both are now mechanical: `.github/binary-is-current.sh`
reads rustc's dep-info, which lists the `.ply` files beside the `.rs`, and diffs
the stdlib the binary actually holds against the files on disk.
`CONTRIBUTING.md` §"The binary is an instrument too" carries the reproduction and
the list of measurements in this ADR taken through a pre-built binary — **§1 and
§4.1 are on that list, and are not withdrawn by it.**

---

## §1 What was verified

`ply test` over the Ply lexer and the harness's own suite are green, and the
agreement corpus covers the whole shipped standard library and the largest file
in the tree, on **spans, payloads *and* diagnostics**.

**The token count was re-derived independently rather than quoted.** Dumping
every corpus file and counting record kinds sums to the spike's figure to the
token.

The code citations behind §1's mechanism were checked and hold: `rc::carry`,
`Env::take_unique_inner`'s `Rc::get_mut` refusal with its *"refuses at the first
shared link"* comment, `DEFAULT_MAX_CALLS`, and the count of `carry` call sites.

**One substantive error in this section was corrected while §7 item 3 was
taken.** It said the inner `push` in `json.ply`'s `escape_runs` is "argument 0 of
2 of `escape_runs`". It is argument 0 of 2 **of the outer `push`**;
`escape_runs` takes three arguments and the offending sub-expression is nested
inside its third. **The distinction is the whole mechanism** — the outer `push`
*is* `escape_runs`' last argument and was always fine, **which is why only one of
the two copied.** `GAPS.md` §1 states it correctly and this paraphrase did not.

### §1.1 The comparison is armed — checked with corruptions of my own

**A green agreement over a comparator that cannot go red is this project's
signature defect**, and the spike's own mutation tests are not evidence that
*mine* would be caught. Two fresh corruptions of `lexer.ply`, neither used by the
spike nor by the pre-merge check:

| corruption | axis | result |
| --- | --- | --- |
| an off-by-one in the EOF token's start | span only; kind and payload identical | **caught** |
| a keyword deleted from `is_keyword` | kind and payload | **caught** |

**A span-only corruption is the one a token-stream comparator is usually blind
to. This one is not.**

---

## §2 What the spike proved

1. **Ply can express a lexer for its own language, to the token.** Agreement is
   on spans, payloads *and* diagnostics, including the whole shipped standard
   library.
2. **It can lex itself**, byte-table literal and all. **This is the canonical
   self-hosting question for a lexer and it was not in the spike's own test set**;
   it was run here and it passes.
3. **§1's finding is real, is positional, and is already being paid in shipped
   code.** Three independent measurements agree, and the shipped instance
   reproduces on the real `std.json` module (§4.1).
4. **The type checker carried the port.** `ply check` passed first try on a
   hand port of over a thousand lines of Rust. **That is a real result about the
   language and it is easy to overlook next to the gap list.**
5. **The divergence is honestly bounded.** The two lexers differ at exactly one
   decision point — `char::is_alphabetic`/`is_whitespace` at a token's first byte
   — and the three shapes are pinned by exact-dump tests rather than described.

---

## §3 What the spike did not prove

**The agreement figure does a lot of rhetorical work and the coverage underneath
it is narrower than it sounds.**

### §3.1 The error paths are 0.15% of the corpus

**Every diagnostic in the entire corpus comes from the ten hand-written fixtures,
which together are about a kilobyte.** Not one real `.ply` file in the tree
raises a lexer diagnostic. The spike's README says the fixtures exist because no
real file reaches the error paths, **so this is disclosed — but the disclosure
and the byte count appear in different paragraphs, and half of what `lexer.rs`
does is error handling.** Float coverage is thinner still: a couple of dozen
float tokens in the whole corpus, every one short.

**Demonstrated rather than counted.** A census of a corpus is an argument about
what that corpus *could* catch, so the claim was made biting instead. Both sites
in `lexer.ply::punct` that raise a diagnostic were replaced so the mutant raises
neither. **The token stream is byte-identical**; the mutant differs only in that
it silently accepts what `ply_syntax` refuses — **the one failure a token-only
comparison is built to miss, and the reason the dump carries diagnostics at
all.**

**Every agreement test over real source passed. Only the hand-written fixtures
failed.** So the whole corpus of real source cannot tell the real lexer from one
that never raises a punctuation diagnostic, and the kilobyte of fixtures is the
entire difference between that mutant and a green board — **a ratio of hundreds
to one by weight, on half of what `lexer.rs` does.** The spike disclosed the
shape of this; what was missing is that **it is not a caveat about breadth but a
load-bearing dependency on ten small files**, and any future port should size its
fixtures against that rather than against the corpus.

`lexer.ply` was restored byte-identical afterwards, `cmp` clean and every test
green.

### §3.2 The float comparison cannot see the digits, and the README says it can

The spike's README states that the comparison checks that the Ply lexer
*"classified the literal as a float, spanned the same bytes, and extracted the
same digits."*

> **Withdrawn: "and extracted the same digits".** The harness runs the Ply
> lexer's float text through Rust's `f64` parser before comparing, **so any
> digit string that rounds to the same `f64` passes. What is checked is the
> *value*, not the digits.**

Demonstrated, not reasoned, with two further corruptions: appending a zero to
every fraction, and truncating every fraction to seventeen significant digits.
**Both agree.** The second is an ordinary fixed-buffer bug and it is invisible.

**The consequence is bounded — for a *lexer* the observable output is the `f64`
— but it stops being bounded the moment a self-hosted front end converts that
text itself**, which `GAPS.md` §5 says it must eventually. **The harness as built
cannot certify the digit extraction it is relied on to certify.**

**One thing this found that came back green.** The inf-saturation case `GAPS.md`
§3 is written around appears in no corpus file and in no fixture. It was run
here: both lexers saturate identically. **Unexplored space, now explored, no
defect.**

### §3.3 The Ply lexer cannot lex the files the harness writes to feed it

The harness embeds each corpus file in a generated `probe.ply` as a byte literal.
For the largest example that literal holds thousands of escapes. `ply_syntax`
lexes it into a couple of dozen tokens. **`lexer.ply` cannot lex it at all** —
`E0502 recursion limit of 10000 nested calls exceeded`.

**So the lexer agrees with the reference on the corpus and cannot process the
files that were written in order to ask it about them.** This is not a defect the
spike introduced — it is `GAPS.md` §6, which calls the corpus margin "a fact about
the corpus, not about the lexer". **It is worth stating in the stronger form,
because the corpus that refutes it is the spike's own scaffolding.**

Both cliffs were located rather than estimated: consecutive comment lines survive
into the thousands and escapes in one literal into the low thousands, against a
shipped corpus whose deepest trivia run is in the hundreds and whose largest
literal holds a few hundred escapes. **The margin on real code is one to two
orders of magnitude. The margin on generated code is negative.**

---

## §4 The gaps, ranked by what they cost

`GAPS.md` numbers its fifteen entries in the order it met them. **This is the
order they cost.** A bold **§N** leading an item here is `GAPS.md`'s section N.

### §4.1 The language expresses it, and slowly — and this one is a shipped defect

**§1, the positional trap.** A growing container must be built in the last
sub-expression of its enclosing node or the program is quadratic. Re-measured
from a reproduction written off the prose rather than copied, on a clean binary,
**user CPU as the primary statistic because it survives contention far better
than wall clock**, against a rule fixed before any data existed. **Accept.**

**The third arm is the important one.** Binding every other field to a `let`
before the push makes the growing field the last *mention* of the record **and
the program stays quadratic.** The rule is positional in the enclosing node, not
about the variable. **Two people have now written the last-mention explanation
down and both were wrong.**

The call-argument form confirms it is not record-specific.

**It is being paid in shipped code, and this was re-measured on the shipped
module rather than a copy.** `json::encode_string` on a string whose characters
all require escaping is quadratic in the number of escapes, approaching a
four-fold cost per doubling as the linear start-up term washes out. `GAPS.md`
measured a standalone reproduction; **the shipped module reproduces it. The
spike's figure is confirmed on better evidence than the spike had.**

**This is the highest-cost item in the whole gap list and it has nothing to do
with self-hosting.** It is quadratic behaviour in the standard library's JSON
serializer, in the length of a string, **and a served response that echoes
attacker-influenced text through `encode_string` pays it.**

### §4.2 The language cannot express it

**§3, `Float`.** There is no `float_of_string`, no `float_to_string`, no `parse`;
`float_of_decimal` cannot reach `inf`. **A Ply lexer therefore cannot produce
`TokenKind::Float(f64)`; it produces the literal's text and something else
converts it.** This is the one item in `GAPS.md` that is a hole rather than a
tax, **and §3.2 shows the harness cannot check the substitute.**

**§8, no file IO.** No shipped effect has a file operation, so a source file
reaches a Ply program as a literal or not at all. Cheap today and absolute: **a
self-hosted front end needs either a file effect or a Rust driver that hands it
bytes.**

### §4.3 Merely unfamiliar — and one of them is simply wrong

**§4, the numeric bounds.** `GAPS.md` says Ply has no `int_of_string`, that
`decimal_of_string` "tops out below both bounds", and that checked `Int`
arithmetic defeats the accumulate-and-watch-the-sign trick.

> **Withdrawn: "which is below both bounds".** `decimal_of_string` and
> `int_of_decimal` both answer an `Option`, **so neither raises and nothing needs
> to be accumulated.** Run on the six boundary values the fixtures test, they are
> **exactly the two bounds `lexer.rs` decides with**: `decimal_of_string` accepts
> the full mantissa maximum and declines one above it, which *is* the mantissa
> test, and `int_of_decimal` is the other.
>
> **The second sentence is true and this review first called it wrong.** `Int`
> arithmetic *is* checked, so accumulate-and-look-at-the-sign genuinely does
> raise before the overflow is observable. **It is simply not load-bearing,
> because accumulating is not the only route to the value.**

Consequence, stated narrowly **because the first draft of this paragraph
overstated it**: about ten lines of `lexer.ply` are replaceable by two builtin
calls that already exist. *(An earlier version of that sentence named twice as
many lines and included `strip_zeros`, which is not only a bound-check helper —
it builds the decimal mantissa payload, which has no builtin, and stays.)*

**This is a discoverability gap, not a language gap**, and it belongs in a
different bucket from §1 and §3. **It was found by running the builtins rather
than by reading the gap list — which is also how the overstatement above was
found, one step later.**

**§13, no dispatch mechanism**, and **§14, mutable state** — both recorded as
negative results, correctly: a lexer has nowhere that wants either.

### §4.4 Real taxes, correctly ranked as taxes

**§2** (no record update — **closed by ADR 0023**, which also withdrew the
"silently wrong limit rather than a type error" half; it is a type error, and the
surviving hazard is a mispairing), **§7** (no `byte_of_int`; two independent
byte-table literals exist for the identical reason), **§9** (no tuples), **§10**
(the `List` surface), **§11** (`bytes_at_or`, still **not measured**).

§12's count of `Result`-returning functions in `json.ply` was re-derived and the
spike's figure stands, with its own "close rather than exact" hedge.

**§10's enumeration has since been corrected.** It read that `List` has no index,
concat, reverse, prepend or sort, **which was an accurate reading of the builtin
table when written.** Two builtins have been added since: `iterate` (ADR 0022)
and `list_at` (ADR 0027). **"No index" is the half that is now false**; the
others stand. **ADR 0027 §8 also carries a prior against §11's `bytes_at_or`:
the equivalent *list* form was measured, missed its bar, and was refused.**

### §4.5 Five stale claims in the merged tree, all fixed while this was written

**The count in this heading moved twice, which is the point of the section.** It
read "one", then "two". **Each re-read of the spike found another.**

The harness's agreement test opened with *"the corpus above is every `.ply` file
in the tree and **all of it is ASCII** — checked, and the reason this test
exists."* **Both halves are false.** The corpus is a third of the `.ply` files
outside `spikes/`, and it holds over a thousand non-ASCII bytes — a figure a test
four lines below pins with an `assert_eq!`, **in a test carrying two correction
blocks of its own that say so.** The spike withdrew this claim in its README and
in that test's doc comment **and left it standing in the doc comment one test
above.**

`GAPS.md` §5 called one module "the largest `.ply` file in the tree". It is the
file with the most **tokens** and the second largest by **bytes**. The claim §5
rests on holds either way.

**Three more, all in `lexer.ply`, none found by the spike's own review or by the
first pass of this one:** a header comment naming the wrong accumulator data
structure, and two counts off by one.

**The first is worth more than the other two together, and it is the strongest
single illustration in this review of why the repository keeps finding this
defect.** It is `lexer.ply`'s **header comment** — the first prose a reader of
the file meets — and it does not merely name the wrong data structure. **It
restates, in the voice of a measurement and with a citation to `GAPS.md` §1,
precisely the explanation that `GAPS.md` §1 exists to withdraw.** The corrected
statement was already in the same file sixty lines below. **So the spike carried
the withdrawn claim and its withdrawal simultaneously, with the withdrawn one
first and better placed, and three reviews read past it — because a header
comment reads as orientation rather than as a claim, and nothing in this
project's loop treats it as one.**

Correcting the agreement test also closed a hole this review had left open: the
uncompared files include **the only file outside the spike that raises a lexer
diagnostic.** It was compared by hand and the two lexers agree on it exactly.
**That is a real narrowing of §3.1's gap: the error-path evidence is now ten
fixtures *and* the one real file in the tree that reaches an error path.**

---

## §5 The parser, which is the next component and is harder

The lexer port was function-for-function. **A parser port cannot be**, and that
is the finding of this section rather than a list of aggravations.

### §5.1 The gap that changes category

> **Its premise is withdrawn by ADR 0022.** The two load-bearing sentences were
> that a recursive-descent parser *"recurses once per element unless it is
> folded"* and that *"the lexer's fold-over-a-range escape hatch does not
> generalise, because a recursive-descent parser's recursion **is** the
> grammar."*
>
> **Neither holds of the reference implementation this repository ships.**
> `crates/ply-syntax/src/parser.rs` drives **every** sequence with a loop, with
> one shared `comma_list` called from a dozen-plus sites covering argument lists,
> literals, parameters and pattern arguments. It even climbs precedence
> iteratively, recursing only for the right operand, **so its depth is bounded by
> the binding powers it declares rather than by operand count.**
>
> It reserves recursion for grammar nesting **and bounds that itself** at
> `MAX_DEPTH`, far below the call ceiling and far above the corpus maximum this
> section measures. **Grammar nesting in this design cannot reach the ceiling —
> the parser refuses first.** ADR 0022 also adds `iterate`, an early-terminating
> loop that is depth 1 on both engines, **so the escape hatch generalises further
> than "fold over a range": it no longer has to run to a conservative bound.**
>
> **What is not withdrawn:** §6's throughput finding, which is this ADR's actual
> reason for deciding against self-hosting today. §5.1 was a second, independent
> objection; **only that one falls.**

**§5, the 10,000-call ceiling, stops being a tax and becomes an architecture
constraint.** The lexer escaped it with a fold over an eagerly materialised range
— **a loop driven by the machine's step protocol, so it nests nothing.** That
works because lexing is a *flat* state machine: one step per byte, no nesting.

Two depths are bounded by the ceiling. **Grammar nesting** is comfortable: the
deepest bracket nesting measured across the real files is well under twenty.
**Sequence recursion is the problem** — a parser consuming *N* items recurses
once per item unless it is folded, the largest example is tens of thousands of
tokens, **and there is no flag.**

### §5.2 Gaps that get worse

- **§1 gets worse in two ways and better in one, and the first draft of this
  bullet had it wrong.** It asserted "much worse … one such site per grammar
  production" with **no measurement behind it.** The direction is right and the
  magnitude was guessed. Measured:

  | where the *caller* puts a correctly-written callee | |
  | --- | --- |
  | the whole body — nothing follows it | linear |
  | argument 0 of 2, followed by a variable | **quadratic** |
  | argument 0 of 2, followed by a **constant** | **quadratic** |
  | argument 1 of 2 — nothing follows it | linear |

  **The rule composes, and it is not local.** The callee is written correctly in
  all four rows — its `push` is the last sub-expression of its own record literal
  — **and the caller destroys it anyway.** The third row is the sharp one:
  `carry` is *"if remaining, clone the env"* and **never asks what the remaining
  sub-expression reads**, so a literal `0` sitting after the call is enough.
  **For a parser this means a correct combinator gives its caller no protection,
  and the thing that removes the protection can be a constant.**

  **What gets better is the grouping.** The copy is O(the list's current length),
  so the cost is O(k·m) for k pushes over lists of length m. Pushing into one
  list is quadratic; **pushing into lists of ten is linear.** A lexer accumulates
  one list of every token in the file — the worst case. A parser accumulates
  mostly short lists: a block's statements, a call's arguments, a match's arms.
  **So per site a parser is far cheaper, and the honest statement is that a
  parser trades one catastrophic accumulator for hundreds of cheap ones plus a
  non-local rule** — except for whatever is module-wide, which is the lexer's
  shape again.
- **§9 (no tuples) becomes the dominant shape.** Every parse function returns "a
  node and the next index". **That is the single most common type in a
  recursive-descent parser and it is a record declaration each time.**
- **§12 stops being free.** `GAPS.md` §12 records that error accumulation cost
  nothing *because a lexer never fails*. **A parser with error recovery does
  fail**, and `json.ply` is the preview: nearly half its functions return
  `Result`, with hand-written `decode_map`/`decode_and_then`.

  *(An earlier version of this bullet cited "no `?`" and "one number literal
  split across seven functions purely to bind an `Ok`". `?` exists as of ADR
  0027 and `json.ply` uses it. **And the seven-function chain has no `Ok` bind in
  it** — checked function by function: each ends in a tail call inside a branch,
  because a check that fails must answer `Err` *there* while a check that passes
  carries on, **and Ply has no early `return` with which to write that in one
  function.** `?` collapses none of it. **What the chain is evidence for is the
  absence of `return`, not the absence of `?`** — which matters here, because a
  self-hosted parser inherits it.)*
- **§10 (the `List` surface)** starts to bite: list patterns, argument lists and
  match arms want index, `nth` and reverse.

### §5.3 A gap a lexer cannot hit

**Value depth.** `MAX_VALUE_DEPTH` bounds how deep a value may nest before a
*structural walk* over it refuses. The lexer's token type is flat, so nothing in
the spike touches this. **An AST is recursive by definition, and every
comparison, hash and render of it is a structural walk.** The bound is generous
relative to the measured nesting, but **it is a distinct ceiling from the call
ceiling and the lexer provides no evidence about it.**

---

## §6 What the front end would cost, and whether that is acceptable

### §6.1 Measured

**User CPU seconds as the primary statistic**, with wall clock beside it and the
load recorded, because this machine carried three other agent worktrees running
test suites throughout. Minimum of N, N stated.

Across four large files the Ply lexer's throughput in KB/s varies by half again
and **its throughput in tokens/s varies by a fifth, so tokens is the right
unit.** `GAPS.md`'s figure survives the re-take; it was a wall-clock figure under
load and is somewhat pessimistic against the load-corrected one. **The
`bytes_len` control re-takes at essentially zero, confirming that essentially all
of it is the Ply lexer running.**

For scale, `ply check examples/` — lex, parse, resolve, typecheck, effect-infer
and content-hash the same files and their modules — is **an order of magnitude
faster in tokens per second, for six phases against one.** *(`GAPS.md` reports
this slower, at lower load. The re-take is faster on a busier machine: **the
spike's figure was pessimistic, and correcting it makes the comparison worse for
self-hosting, not better.**)*

**That is roughly a dozen times slower for the first phase of six.**

> **The absolutes here do not reproduce; the ratio does.** All three figures were
> re-taken **in one sitting** — this section's were not, and §9 records load
> ranging by a factor of five across the series. **Both engines are two to three
> times faster than recorded and the ratio between them is not**, so the headline
> survives its own sensitivity note while every absolute under it is wrong by two
> to three times.
>
> **One cause is identified rather than guessed:** the `dump` step this section
> timed *with* the lexer is about a fifth of the figure it produced, **so the Ply
> lexer was charged with a rendering step worth about a fifth of its own cost.**
>
> Nothing in §6.2 or §7 turns on the absolutes — they are used to form a ratio,
> and the ratio held. **The lesson is §9's own, arriving from outside: figures
> taken across sittings at wildly different loads are not comparable with each
> other, and this section compared them anyway.**

### §6.2 Assumed

To get from a lexer to a front end needs a multiplier, and there is no way to
measure one without writing the rest. **This is an assumption and it is labelled
as one.** In conventional compilers lexing is a tenth to a fifth of front-end
time, which puts a full Ply front end at five to ten times the lexer's cost.
**If anything that is optimistic for Ply specifically, because the phases after
lexing are the ones that build records and lists — which is where §1's tax and
§9's absence land.**

At that rate a cold `ply check examples/` becomes tens of seconds. **Roughly two
orders of magnitude.**

> **The multiplier is no longer assumed. It is measured for two phases of six,
> and this section's *premise* is the part that was wrong.**
> `spikes/ply-parser` ports the parser and measures lex+parse against lex — **and
> lexing is still a third of front-end time after two phases**, against the
> "tenth to a fifth" this section borrowed from conventional compilers and built
> its band on.
>
> **The band itself is not refuted**, and the honest statement is narrower than
> either direction: at the measured two-phase multiplier, the assumed overall
> band requires the four unwritten phases to cost a few times what parsing cost,
> **which is plausible.** Two phases of six cannot say whether the band is
> optimistic or pessimistic, **and the spike declines to.**
>
> **What replaces the projection is a measurement.** Ply lex+parse over the
> identical files `ply check` reads costs **thirty times** what six Rust phases
> cost, with no extrapolation — and redoing this section's own arithmetic on the
> re-taken lexer term halves both absolute halves and leaves the ratio unmoved.
> **So the sentence that follows is more true than when it was written, on
> better evidence, and it is the one thing here that did not need correcting.**

**That is the answer to whether this is acceptable for the loop it is meant to
make fast, and it is not close.** The verification loop this work exists to
accelerate is currently sub-second, **and a self-hosted front end at today's
interpreter speed would make it the slowest thing in the build by two orders of
magnitude.** **The incrementality argument does not rescue it either:** the warm
path is already an order of magnitude under the cold one, **and a self-hosted
front end would have to be cached by machinery that is itself in Rust.**

### §6.3 The compiled fragment does not obviously close this, and the reason is specific

The natural objection is that the fragment being built in parallel makes this
moot. **It does not, and the reason is worth stating precisely because it prices
that work.**

The spike's headline ratio is measured on `read_line`-shaped kernel code, and it
is **the most conservative expression the file supports** — interpreter *best*
against spike *worst*, minimised across its inputs. Re-derived from the file
rather than quoted.

> **`read_line` cannot cross the seam (ADR 0026).** `read_line` takes `Bytes`;
> `crossable` is `Int | Bool`, and `admit`'s first line refuses on argument
> shape. The spike's own `measure.rs` labels its path *"a direct native call,
> outside any machine"*.
>
> **So the number is real and it is not a number about this seam:** the
> arithmetic below applies a speedup measured outside the machine to a function
> the machine's own gate refuses on its first line. **It is not that the transfer
> is unmeasured — for this shape it is *unmeasurable* without widening
> `crossable`.** The conclusion is unaffected in direction and **its magnitude
> should not be quoted.**

Even at the full measured speedup, applying it to §6.2's projected front end
still leaves a cold `ply check` several times worse than today's. **So the
fragment does not make a self-hosted front end competitive on the cold path.**

**And the speedup should not be assumed to transfer at all.** I first wrote that
the fragment lowers no `Bytes` builtins and would refuse a lexer outright. **That
is wrong and is withdrawn:** there is a generic path — `admissible_builtin`
refuses only higher-order builtins, the cell operations and `secret_of_string`;
every other builtin, `bytes_at` and `bytes_scan` included, is admitted and
dispatched through one runtime helper.

**What that helper does is the point:** it calls `ply_eval::builtins::call` —
**the identical interpreter builtin body.** So compiled code gets native
arithmetic, native control flow and no per-call frame protocol, **and pays
interpreter price for every byte operation.** A lexer's inner loop is
`bytes_at`, `bytes_scan`, `bytes_slice`, `bytes_concat_all` — builtin dispatch,
not arithmetic — **which is exactly the half the fragment does not accelerate.**

**The transfer was unmeasured. It has now been measured, and the answer is the
one that favours the fragment.** The profile was taken rather than left as a
recommendation, **because it was named as the highest-value decision-relevant
measurement in the document and it cost six seconds.**

`/usr/bin/sample` against the release binary running `lexer.ply` over four
distinct slices of a large example — distinct so a pure-function memo cannot
collapse them — attributing by walking the call graph and charging each subtree
to its outermost matching frame. **Two independent windows agree: samples under
builtin bodies are a twentieth of the samples under evaluation.**

**Dispatch dominates builtin bodies by roughly twenty to one.** By leaf sample,
the machine's own step and dispatch is the largest share, reference-counting and
`Drop` traffic next, then the continuation stack — **and every builtin body
together is about a percent.** `memcmp`/`memcpy`, where a byte-scanning
workload's "real work" lives, is a couple of percent.

**So the objection this section was written to answer does not hold: a lexer is
*not* builtin-bound.** Its cost is the interpreter's per-step protocol and the
refcount churn around it, **which is precisely the half the fragment removes.**
That makes the fragment the right lever for a front end on this evidence, and it
makes open-coded `Bytes` primitives a second-order concern rather than a
prerequisite.

**Three things this does not license, and the first is the one that bites.**

- **The fragment cannot take the loop.** `admissible_builtin` refuses every
  higher-order builtin, and the lexer's whole scan is a `fold` while `dump` is
  two `map`s. **So the top-level functions are refused outright, and what the
  fragment could accept is the per-token work beneath them. That means one entry
  per token, not one per file.**
- **The per-entry arena cost is now a small constant rather than a cliff.** This
  bullet first cited a carry-over where **every entry cost O(the previous entry's
  peak arena)** — a two-orders-of-magnitude multiplier — **which was fixed the
  same day.** `Ctx::end` clears the arena at the end of the entry that filled it,
  so an entry pays for its own work and its successor pays for nothing. Checked
  here at the mechanism rather than the ratio: `begin` is a single emptiness
  check, and `end` costs a few nanoseconds per slot the entry itself used, with
  the shrink amortized. Arithmetic puts a token-granularity front end's arena
  cost **under a percent** of current lexing time. **Still unmeasured for this
  workload and still worth measuring, but it is now an open question about a
  small constant rather than a cliff — the difference between *probably fatal*
  and *probably fine, go and check*.**
- **The builtin share is a share of the interpreter's time, not a predicted
  speedup.** Removing dispatch for the compiled fraction does not make the
  compiled fraction free, **and the only measured speedup was taken on a workload
  with no such entry pattern.**

Two further interactions, recorded rather than resolved: `admissible_builtin`
refuses the cell operations, **so the cell-based lexer `GAPS.md` §14 priced as
the alternative to the fold is *excluded from the fragment*. The two routes to
making a Ply lexer fast are mutually exclusive today.**

---

## §7 Decision

**Do not write Ply's front end in Ply on today's interpreter.** Not because Ply
cannot express one — it expressed a lexer that agrees with the reference across
the whole corpus and lexes itself — **but because §6 prices it two orders of
magnitude above the loop it is meant to make fast, and §6.3 shows the fragment
cannot be assumed to close that.**

**Still the decision, on better evidence.** The original figure was a projection
from an assumed multiplier. `spikes/ply-parser` wrote the parser and measured it:
**two phases against six, no extrapolation.** Ply now also *expresses* a parser,
with **zero disagreements** against `ply_syntax` over a large corpus, so the "not
because Ply cannot express one" clause covers two phases rather than one. **The
rejection was right and it is the throughput objection, alone, that carries it.**

Ranked, what would change the answer:

1. ~~**An attribution run splitting the Ply lexer's time between builtin bodies
   and dispatch.**~~ **Taken** (§6.3): builtin bodies are a twentieth of
   evaluation; dispatch and refcount traffic are the rest. **The fragment is the
   lever, not a distraction.** What replaces it at the top of this list is **the
   fragment's actual throughput at one entry per token**, which is the entry
   pattern a front end would produce.

   > **Overtaken by ADR 0026.** Two things have to happen before that measurement
   > means anything. **A lexer's arguments are `Bytes`, and `crossable` is
   > `Int | Bool`** — the same gate that refuses `read_line` refuses a lexer's
   > per-token functions, **so today the entry rate is not "one per token", it is
   > zero.** And a backend is now reachable from a shipping command, which is
   > what makes such a measurement takeable at all. **The item stands as the
   > right question and its precondition moved.**
2. **Making §1 visible.** A lint, a `--explain` line, anything that says *this
   `push` will copy*. §4.1 shows the trap is already in the standard library, **so
   it pays for itself with no self-hosting at all.** And §5.2 shows the
   precondition is **not local**: a correctly written function is made quadratic
   by its caller, **and by a caller whose offending sub-expression can be a
   constant.** That rules out a coding convention, because **there is no local
   property an author can check.**

   > **The lint was built and it was refuted.** *"A lint is not the convenient
   > fix here; it is the only one"* is withdrawn. Everything above it survives —
   > **but "the only one" was reached by eliminating the alternatives rather than
   > by trying the remedy, and the remedy was tried and failed.** A field-order
   > lint written for exactly this rule **fires** on a shape that copies nothing
   > and is **silent** on one that is fully quadratic — a false negative on the
   > exact shape it existed for. ADR 0024 records the trial: **a lint is a
   > partial oracle over a dynamic property, so a better lint is not what was
   > missing.**
   >
   > **The rest of this item's menu is worse, not better.** A `--explain` line
   > shows the property only to someone who already suspects it and runs a tool
   > with a flag — **a diagnostic for the reader who least needs one.** Under the
   > authorship model this ADR is written inside, where most Ply is written by
   > agents that cannot see a refcount and read a signature instead, **a property
   > visible only behind a flag is not visible at all.**
   >
   > **And ADR 0025 then declined ADR 0024's mechanism**, on a measurement. What
   > survives across both is **this item's title, not its remedy**: §1 must be
   > visible somewhere an author cannot miss it. `ply check --costs` and a
   > per-site oracle are where that landed.
3. ~~**Fixing `escape_runs`.**~~ **Taken.** `escape_runs` performs one `push` per
   escape, in last-argument position. Counted in-process with
   `ply_eval::rc::stats()` **on the shipped module, not on a copy**:
   whole-accumulator copies per encode went from one per escape to **zero** at
   every size measured.

   > **The fix is the machine engine's, and this item did not say so.** Every
   > figure in it was taken on the **machine** engine and stated without naming
   > one. Ply ships two, `--engine both` is the audit that catches one drifting
   > from the other, **and it compares *answers*. A divergence in *cost* passes
   > it in silence, which is exactly what this is.**
   >
   > On `--engine treewalk` `escape_runs` is **still quadratic after the fix, and
   > no spelling of it is not.** The tree-walker runs no reference counting at
   > all: it evaluates the AST, which carries no `Own` — that field is on the
   > lowered node only, and lowering is the step this engine does not have — and
   > it has no `take_unique` and no `carry` call site, **so a pending "frame" is
   > a native stack frame holding the caller's scope by shared reference for the
   > whole of every subexpression.** The accumulator is therefore at two owners
   > at every `push`, and **position cannot help.**
   >
   > **All three of the survey's fixes are engine-conditional**, not just this
   > one. What the fix buys on the tree-walker is a halved constant.
   >
   > **Disclosed rather than fixed, and the reason is in `CONTRACTS.md`.** Reuse
   > on the tree-walker needs the Perceus pass the machine gets at lowering,
   > which a walker holding `&Env` on the native stack has no way to express —
   > **and §"Deleted with the machine" retires that engine outright.** The
   > disclosure carries an assertion so it cannot go stale in silence:
   > `stdlib_accumulator_cost.rs::all_three_fixes_are_the_machine_engines_only`
   > pins one copy per element on the tree-walker at all three sites, **and names
   > the documents to correct on the day it fails.**

   **The depth was checked, not assumed.** The largest input `encode_string`
   completes under the call budget is **the same integer before and after**, by
   bisection. `GAPS.md` §1's refusal of the split shape is confirmed here on the
   shipped module for the first time: built that way, the same bisection gives
   **half the ceiling. So of the three shapes, only this one is both linear and
   no deeper.**

   Behaviour is byte-identical over a corpus covering all 256 code points singly
   and in one string, every named escape form, escapes first, last and adjacent,
   and a large escape string, on both engines. The gate asserts a **count** and so
   needs no deferred row; **it was seen red on the shipped defect before the fix
   existed, and red again on a deliberate revert.**

   **Item 2 is still the general answer.** This fixes one instance; a survey with
   the same counter found and fixed two more of the same shape — a trace sink's
   `append` **on a serving path** and a router's table builder. A module with
   dozens of `push` sites was **not** measured and is the obvious next place to
   point the counter.

   > **Both of those were fixed and neither was gated at first.** The sentence
   > that stood in for a test leaned on one that **asserts nothing**: it prints a
   > figure and returns, **and would have passed with both fixes reverted.**
   > `stdlib_accumulator_cost.rs` now covers all three sites with counts, **each
   > bound armed against a revert of its own literal** — demonstrated rather than
   > assumed.
4. ~~**A loop, or a raisable call ceiling.**~~ **The loop is delivered and the
   raisable ceiling is refused, by ADR 0022.** `iterate` is an early-terminating
   loop that is depth 1 on both engines. **A bare `--max-calls` flag is refused
   because results are cached and shipping code writes only `Pass`: raising the
   bound is monotone and safe, and lowering it silently returns a `Pass` for a
   program that would now raise.** And §5.1's premise, which is what put this
   item on the list, does not hold.
5. **`Float` construction** (§4.2). **The only absolute hole, and the smallest of
   the five in impact**, because §3.2 shows the text-passing substitute works.

**Keep** the differential harness. It is the right shape and it is armed on every
axis but one — kind, payload, span, dropped token and dropped diagnostic all go
red under mutation (§1.1) — **and the one exception is the float digits, which
pass two deliberate corruptions (§3.2).** Amend it so the digits are compared as
digits.

**Do not keep** the implication that the corpus size is broad coverage. **It is
0.15% error paths and two dozen float tokens (§3.1).**

---

## §8 What would make this wrong

- **§6.2's multiplier is assumed, not measured.** If a Ply parser and typechecker
  cost only twice the lexer the conclusion weakens without reversing; if they
  cost twenty times it gets worse. **Writing the parser is the only thing that
  settles it, and this ADR recommends against writing it — so the number that
  would refute this ADR is one it declines to take.** That is a real weakness and
  it is why §7's first item is a profile rather than a port.
- **`ply check examples/` is not a like-for-like baseline, and the two errors
  push opposite ways.** Counting only the examples' tokens while the run also
  resolves the standard library **understates** the Rust front end's throughput,
  so §6.1's ratio is a floor. Against that, crediting the Rust side with six
  phases where the Ply side does one **overstates** the gap for the phase
  actually compared. **Neither was separated here.**
- **Every wall-clock figure was taken on a machine shared with three other
  worktrees.** User CPU is reported precisely because it is the robust half; **if
  user CPU is itself distorted by cache contention at this load — not checked —
  the absolute figures move.** The *shape* results in §4.1 do not depend on it.
- ~~**§6.3's conclusion rests on reading the runtime helper, not on profiling
  it.**~~ **Closed by measurement**, and it resolved against the reading.
- **The profile is a sampling profile of one workload on one input.** Symbol
  attribution in a release build can be distorted by inlining — **a builtin body
  inlined into `Machine::step` would be charged to dispatch.** The leaf histogram
  corroborates rather than assumes, **but a counter-based attribution would
  settle it and none exists.**
- **§6.1's ratio divides two numbers taken at different loads.** Both are user
  CPU and the gap is an order of magnitude, so the conclusion is not sensitive to
  it, **but the ratio is not a clean single-sitting figure.**

---

## §9 Provenance

Machine shared throughout with three other agent worktrees, two of them running
`cargo test --workspace`. **1-minute load average recorded with every series and
ranging by a factor of five.**

Two files in the reviewed tree were written by another party **during** this
review. `lexer.ply` was re-checked afterwards — unchanged, three content windows
byte-identical to the ones read before the move, tests and agreement still green
— so §1 stands. **It is recorded because §0 is about exactly this and a second
instance should not go unwritten.**

Statistic pre-registered before any measurement existed, written down while the
binary was still building, and reproduced here so a reader does not have to take
it on trust:

> Minimum of N runs; N = 5 where a run is under 2 s, N = 3 otherwise. Minimum,
> because on a loaded machine the minimum is the closest estimate of the unloaded
> time and no run is discarded after the fact. Load recorded immediately before
> and after every series. **Prefer any deterministic counter over wall clock if
> one exists.**
>
> Decision rule for §1's shape claim: **accept** if, over two consecutive
> doublings, the non-final column is ≥ 3.0× per doubling at both and the final
> column is ≤ 2.5× at both. Chosen because the prediction is 4× against 2× and
> load noise multiplies both columns roughly equally, **so the ratio is the
> load-robust statistic.**

No run was discarded after the fact.

> **A deterministic counter existed, and nothing outside `ply-eval` could read
> it.** `ply_eval::rc::Stats` had counted updates against in-place updates, and
> takes attempted against takes moved, since the reference-counting pass was
> written. It was read by three test files and had **no CLI surface at all**, so
> "no counter exists" was true of the command and false of the codebase — **and
> the consequence is the one §6.1 now carries: a document that needed a count
> timed something instead, and its absolutes did not reproduce.**
>
> `ply run --json` now reports them. **In-place counts are `null` on the
> tree-walker rather than zero**, because that engine runs no reference counting
> at all **and a zero there reads as a fact about the program when it is a fact
> about the engine.** Under `--engine both` the whole object is `null`: the two
> engines do not count the same thing, **and their sum is a figure about
> neither.**

**One measurement in this document was discarded and re-taken.** The first
attempt at §4.1's shipped-module series used the wrong member spelling, so every
run failed to compile **and the harness timed the failure and reported it as a
row.** The harness now refuses to record a run whose output does not carry
`"ok": true`. **Recorded because it is the same defect this ADR is about: a green
number over a program that never ran.**
