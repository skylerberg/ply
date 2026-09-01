# ADR 0020 — Self-hosting the front end

**Rejected for now, with two pieces of the spike kept.** Why anyone wanted this
is ADR 0021; this prices whether Ply can host its own front end and decides that
it cannot yet, and **read alone it is a rejection with nothing behind it.**

- **Rejected:** writing Ply's front end in Ply on today's interpreter. **The
  blocker is throughput, not expressiveness**, and the evidence for it has
  strengthened.
- **Accepted:** the differential-harness pattern is the right shape for pricing
  any future port, with the amendment below. Re-used, and it held.

## What has moved since, and what a reader should not trust

The *pricing* is the reason anyone reads this record. Four things around it have
been refuted or re-measured, and one of them is a claim this document itself
made.

| what moved | now |
| --- | --- |
| the premise that a recursive-descent parser recurses per element | **refuted** (ADR 0022): the reference parser drives every sequence with a loop, and `iterate` gives Ply the same shape at depth 1 |
| the throughput absolutes | **do not reproduce** — both engines several times faster than recorded; the *ratio* holds |
| the lexer-to-front-end multiplier, assumed | **measured**, for two of six phases, and it is larger |
| the premise that lexing is a tenth to a fifth of front-end time | **wrong** — it is a third after two phases |
| "a lint is the only fix" for the positional trap | **refuted** (ADRs 0024, 0025): the lint was built and failed in both directions |

**The decision is not one of the things that moved, and the case for it is
stronger than this document makes it.** The original priced one phase; two
phases now measure at a multiple three times larger, with no extrapolation.
Redoing the arithmetic on the re-taken lexer term gives a band that overlaps the
one below: **both absolute halves halved and the ratio did not move.**

## The instrument problem this started with

**The first thing found was not about lexers.** A source file carried a
modification timestamp minutes newer than every other file on the tree, and the
release binary behind every wall-clock number in the spike's write-up was built
a minute after that. **The change was an unattributed edit making a field
projection out of a uniquely-owned record *move* the field rather than clone it:
the precise operation the headline finding measures.**

So the finding was measured with an instrument that had been altered in the
place it was measuring. **That is the shape of defect this project's audits keep
finding — a result whose provenance nobody checked because the result looked
right.** It was re-taken three times on a clean binary, by two parties, and it
survived. **The lesson is worth more than the outcome: the finding was correct
and the instrument was not, and only one of those two was checked before the
work was merged.** Nothing in the loop would have caught it — CI's spike job
does not reach this spike, the workspace test command does not reach it, and a
modified working tree is invisible to every command in the contribution guide.

**And the hazard had a second, worse form.** The rule used for checking a binary
against its tree looked only at Rust sources — **and it cannot see an edit to a
stdlib module at all**, because those are embedded into the binary at compile
time, **so editing one changes what an import means and moves no `.rs` file.** A
later workstream lost a headline count to exactly that. Both are now mechanical:
the check reads the compiler's own dependency info, which lists the Ply files
beside the Rust ones, and diffs the stdlib the binary actually holds against the
files on disk.

## What the spike proved

1. **Ply can express a lexer for its own language, to the token.** Agreement is
   on spans, payloads *and* diagnostics, over the whole shipped standard library.
2. **It can lex itself**, byte-table literal and all. **The canonical
   self-hosting question for a lexer, and it was not in the spike's own test
   set.**
3. **The positional trap is real, positional, and already being paid in shipped
   code.**
4. **The type checker carried the port.** A hand port of over a thousand lines
   of Rust passed the checker first try. **That is a real result about the
   language and it is easy to overlook next to the gap list.**
5. **The divergence is honestly bounded**: the two lexers differ at exactly one
   decision point, and the three shapes are pinned by exact-dump tests rather
   than described.

## What the spike did not prove

**The agreement figure does a lot of rhetorical work and the coverage underneath
it is narrower than it sounds.**

**The error paths are a fraction of a percent of the corpus.** Every diagnostic
in the entire corpus comes from ten hand-written fixtures totalling about a
kilobyte; not one real file in the tree reaches a lexer error path. That is
disclosed — **but the disclosure and the byte count appear in different
paragraphs, and half of what a lexer does is error handling.**

**Demonstrated rather than counted**, because a census of a corpus is an
argument about what that corpus *could* catch. Both diagnostic sites in one
function were replaced so the mutant raises neither. **The token stream is
byte-identical**; the mutant differs only in that it silently accepts what the
reference refuses — **the one failure a token-only comparison is built to miss,
and the reason the dump carries diagnostics at all.** Every agreement test over
real source passed. Only the hand-written fixtures failed. **So the whole corpus
of real source cannot tell the real lexer from one that never raises a
punctuation diagnostic, and the kilobyte of fixtures is the entire difference
between that mutant and a green board.** Not a caveat about breadth but **a
load-bearing dependency on ten small files**, and any future port should size
its fixtures against that rather than against the corpus.

**The float comparison cannot see the digits, and the harness said it could.**
The write-up claimed the comparison checks that the Ply lexer *extracted the same
digits*. It runs the extracted text through the reference float parser before
comparing, **so any digit string that rounds to the same value passes. What is
checked is the *value*, not the digits.** Demonstrated with two further
corruptions — appending a zero to every fraction, and truncating every fraction
to seventeen significant digits. **Both agree**, and the second is an ordinary
fixed-buffer bug. The consequence is bounded for a *lexer*, where the observable
output is the parsed value — **but it stops being bounded the moment a
self-hosted front end converts that text itself**, which a self-hosted front end
must eventually do.

**The Ply lexer cannot lex the files the harness writes to feed it.** The
harness embeds each corpus file as a byte literal, and for the largest example
that literal holds thousands of escapes, which the reference lexes into a couple
of dozen tokens and the Ply lexer cannot lex at all — the call ceiling. **So the
lexer agrees with the reference on the corpus and cannot process the files that
were written in order to ask it about them.** Both cliffs were located rather
than estimated: **the margin on real code is one to two orders of magnitude; the
margin on generated code is negative.**

## The gaps, ranked by what they cost

### The positional trap, and it is a shipped defect

**A growing container must be built in the last sub-expression of its enclosing
node or the program is quadratic.** Re-measured from a reproduction written off
the prose rather than copied, on a clean binary, **user CPU as the primary
statistic because it survives contention far better than wall clock**, against a
rule fixed before any data existed.

**The third arm is the important one.** Binding every other field to a `let`
before the append makes the growing field the last *mention* of the record **and
the program stays quadratic.** The rule is positional in the enclosing node, not
about the variable. **Two people have now written the last-mention explanation
down and both were wrong.**

**It is being paid in shipped code**, re-measured on the shipped module rather
than a copy: the standard library's JSON string encoder is quadratic in the
number of escapes, approaching a four-fold cost per doubling. **The highest-cost
item in the whole gap list, and it has nothing to do with self-hosting** — it is
quadratic behaviour in the JSON serializer, in the length of a string, **and a
served response that echoes attacker-influenced text pays it.**

### What the language cannot express

**Float construction.** There is no float parse, no float render, and the
decimal conversion cannot reach infinity, **so a Ply lexer cannot produce a
float token; it produces the literal's text and something else converts it.**
The one item that is a hole rather than a tax — **and the harness cannot check
the substitute.**

**No file IO.** No shipped effect has a file operation, so a source file reaches
a Ply program as a literal or not at all. Cheap today and absolute: **a
self-hosted front end needs either a file effect or a Rust driver that hands it
bytes.**

### Merely unfamiliar, and one that was simply wrong

The numeric bound helpers were recorded as unable to reach the bounds a lexer
decides with. **Withdrawn**: the two existing conversions answer optionally, so
neither raises and nothing needs accumulating, and run on the boundary values
they are **exactly the two bounds the reference decides with**. Checked
arithmetic does defeat the accumulate-and-watch-the-sign trick, **and that is
simply not load-bearing, because accumulating is not the only route to the
value.** **A discoverability gap, not a language gap** — and it was found by
running the builtins rather than by reading the gap list, which is also how the
first draft's overstatement of it was found, one step later.

**Stale claims in the merged tree, and the count moved twice.** Each re-read
found another. The agreement test opened with a claim about the corpus that was
false in both halves, **in a test four lines above one that pins the true figure
with an assertion.** The spike withdrew that claim in two places and left it
standing in a third. And the lexer's **header comment** — the first prose a
reader meets — **restates, in the voice of a measurement and with a citation,
precisely the explanation the write-up exists to withdraw**, while the corrected
statement was already in the same file sixty lines below. **So the spike carried
the withdrawn claim and its withdrawal simultaneously, with the withdrawn one
first and better placed, and three reviews read past it — because a header
comment reads as orientation rather than as a claim, and nothing in this
project's loop treats it as one.**

## The parser, which is the next component and is harder

The lexer port was function-for-function. **A parser port cannot be**, and that
is the finding rather than a list of aggravations — though **the load-bearing
half of that argument was refuted by ADR 0022.** The claim was that a
recursive-descent parser's recursion *is* the grammar, so sequence recursion
must reach the call ceiling. The reference parser drives **every** sequence with
a loop, climbs precedence iteratively, and bounds grammar nesting itself far
below the ceiling. **Only that objection falls; the throughput one does not, and
it is what the decision rests on.**

What does get worse:

- **The positional trap composes, and it is not local.** Measured: a correctly
  written callee — its append last in its own record literal — is made quadratic
  by its caller, **and the trailing sub-expression that costs it can be a
  literal constant**, because the carry rule never asks what the remaining
  sub-expression *reads*. **For a parser this means a correct combinator gives
  its caller no protection.** What gets *better* is the grouping: the copy is
  proportional to the list's current length, so many short lists are linear
  where one long one is quadratic. **A parser trades one catastrophic
  accumulator for hundreds of cheap ones plus a non-local rule.**
- **Tuples become the dominant shape.** Every parse function returns "a node and
  the next index", **the single most common type in a recursive-descent parser,
  and a record declaration each time.**
- **Error accumulation stops being free.** It cost nothing for a lexer *because
  a lexer never fails*; a parser with error recovery does. And the shape that
  forces a number literal's parse across seven functions **is evidence for the
  absence of `return`, not the absence of `?`** — each ends in a tail call
  inside a branch, because a check that fails must answer there while a check
  that passes carries on. **A self-hosted parser inherits it.**
- **Value depth is a distinct ceiling the lexer provides no evidence about.** A
  lexer's token type is flat; an AST is recursive by definition, and every
  comparison, hash and render of it is a structural walk.

## What the front end would cost

**User CPU as the primary statistic**, with wall clock beside it and the load
recorded, because the machine carried three other worktrees running test suites
throughout. Minimum of N, N stated.

Across four large files the Ply lexer's throughput varies by half again in bytes
per second and by a fifth in tokens per second, **so tokens is the right unit.**
For scale, the reference front end — lex, parse, resolve, typecheck,
effect-infer and hash the same files — is **an order of magnitude faster in
tokens per second, for six phases against one.** *(The spike reported this
slower at lower load; the re-take is faster on a busier machine, **so the
spike's figure was pessimistic, and correcting it makes the comparison worse for
self-hosting, not better.**)*

**The absolutes do not reproduce; the ratio does.** All three figures were
re-taken **in one sitting** — the originals were not, at loads ranging by a
factor of five. Both engines are two to three times faster than recorded and the
ratio between them is not, **so the headline survives its own sensitivity note
while every absolute under it is wrong by two to three times.** One cause is
identified rather than guessed: **a rendering step was timed with the lexer and
is about a fifth of the figure it produced.** **The lesson: figures taken across
sittings at wildly different loads are not comparable with each other, and this
section compared them anyway.**

**The multiplier is no longer assumed.** A parser port measures lex-plus-parse
against lex, **and this section's *premise* is the part that was wrong**:
lexing is still a third of front-end time after two phases, against the "tenth
to a fifth" borrowed from conventional compilers and built the band on. The band
itself is not refuted, and the honest statement is narrower than either
direction: at the measured two-phase multiplier, the assumed overall band
requires the four unwritten phases to cost a few times what parsing cost, which
is plausible. **Two phases of six cannot say whether the band is optimistic or
pessimistic.**

What replaces the projection is a measurement: **Ply lex-plus-parse over the
identical files costs thirty times what six Rust phases cost, with no
extrapolation.**

**That is the answer to whether this is acceptable for the loop it is meant to
make fast, and it is not close.** The verification loop this work exists to
accelerate is sub-second, **and a self-hosted front end at today's interpreter
speed would make it the slowest thing in the build by two orders of magnitude.**
The incrementality argument does not rescue it either: the warm path is already
an order of magnitude under the cold one, **and a self-hosted front end would
have to be cached by machinery that is itself in Rust.**

## Does the compiled fragment close this?

**Not obviously, and the reason prices that work.**

The natural objection is that a lexer is *builtin-bound* — its inner loop is
byte operations, which compiled code dispatches through the identical
interpreter builtin body, so compiled code would get native arithmetic and
control flow and pay interpreter price for every byte operation.

**The objection was named as the highest-value decision-relevant measurement in
the document and it cost six seconds to take, so it was taken.** A sampling
profile against the release binary running the Ply lexer over four distinct
slices — distinct so a pure-function memo cannot collapse them — attributing by
walking the call graph and charging each subtree to its outermost matching
frame. Two independent windows agree: **samples under builtin bodies are a
twentieth of the samples under evaluation. Dispatch dominates builtin bodies by
roughly twenty to one.** By leaf sample the machine's own step and dispatch is
the largest share, reference-counting traffic next, then the continuation stack,
and every builtin body together is about a percent.

**So the objection does not hold: a lexer is *not* builtin-bound.** Its cost is
the interpreter's per-step protocol and the refcount churn around it, **which is
precisely the half compiled code removes.** That makes the fragment the right
lever for a front end on this evidence, and it makes open-coded byte primitives
a second-order concern rather than a prerequisite.

**Three things this does not license.** The fragment cannot take the *loop* —
higher-order builtins are refused, and the lexer's whole scan is a fold — **so
the top-level functions are refused outright, and what the fragment could accept
is the per-token work beneath them: one entry per token, not one per file.** The
per-entry arena cost is now a small constant rather than a cliff, after the
carry-over fix, **which is the difference between *probably fatal* and *probably
fine, go and check*.** And **the builtin share is a share of the interpreter's
time, not a predicted speedup** — removing dispatch for the compiled fraction
does not make that fraction free.

Two interactions recorded rather than resolved: cell operations are refused by
the fragment, **so the cell-based lexer priced as the alternative to the fold is
*excluded from the fragment*. The two routes to making a Ply lexer fast are
mutually exclusive today.**

*The speedup figure this section originally applied is not a number about this
seam at all:* the function it was measured on takes bytes, the seam then carried
only integers and booleans, and the harness path is labelled in its own source
as a direct native call *outside any machine*. **The conclusion is unaffected in
direction and its magnitude should not be quoted** (ADR 0026).

## Decision

**Do not write Ply's front end in Ply on today's interpreter.** Not because Ply
cannot express one — it expressed a lexer that agrees with the reference across
the whole corpus and lexes itself, and it has since expressed a parser with zero
disagreements — **but because the pricing puts it two orders of magnitude above
the loop it is meant to make fast, and the fragment cannot be assumed to close
that.**

Ranked, what would change the answer:

1. **The fragment's actual throughput at one entry per token**, which is the
   entry pattern a front end would produce. *Its precondition moved*: the seam's
   argument gate refused a lexer's per-token functions outright, so the entry
   rate was zero, and a backend is now reachable from a shipping command.
2. **Making the positional trap visible.** It is already in the standard
   library, **so it pays for itself with no self-hosting at all** — and the
   precondition is *not local*, which rules out a coding convention, because
   **there is no local property an author can check.** *"A lint is not the
   convenient fix here; it is the only one" is withdrawn.* Everything above it
   survives, **but "the only one" was reached by eliminating the alternatives
   rather than by trying the remedy, and the remedy was tried and failed** (ADR
   0024). **The rest of the menu is worse, not better**: an explain line shows
   the property only to someone who already suspects it and runs a tool with a
   flag — **a diagnostic for the reader who least needs one** — and under the
   authorship model this record is written inside, where most Ply is written by
   agents that cannot see a refcount and read a signature instead, **a property
   visible only behind a flag is not visible at all.** What survives across the
   two later records is **this item's title, not its remedy.**
3. **Fixing the shipped quadratic. Taken**, counted in-process on the shipped
   module: whole-accumulator copies per encode went from one per escape to
   **zero** at every size measured. **The fix is the machine engine's, and this
   item did not say so.** Every figure was taken on the machine and stated
   without naming one; **the audit that catches one engine drifting from the
   other compares *answers*, and a divergence in *cost* passes it in silence.**
   On the tree-walker the function is **still quadratic after the fix, and no
   spelling of it is not** — that engine runs no reference counting at all, so
   the accumulator is at two owners at every append and **position cannot
   help.** All three of the survey's fixes are engine-conditional. Disclosed
   rather than fixed, because reuse on the tree-walker needs the pass the
   machine gets at lowering, and that engine is retired in principle anyway —
   with an assertion so the disclosure cannot go stale in silence, **naming the
   documents to correct on the day it fails.** **The depth was checked, not
   assumed**: the largest input the encoder completes is the same integer before
   and after, **so of the three shapes only this one is both linear and no
   deeper.**
4. **A loop, or a raisable call ceiling. Delivered and refused respectively**
   (ADR 0022).
5. **Float construction.** The only absolute hole, and the smallest in impact.

**Keep** the differential harness. It is the right shape and it is armed on
every axis but one — kind, payload, span, dropped token and dropped diagnostic
all go red under mutation — **and the one exception is the float digits.** Amend
it so the digits are compared as digits. **Do not keep** the implication that
the corpus size is broad coverage.

## What would make this wrong

- **The multiplier was assumed.** Writing the parser is the only thing that
  settles it, **and this record recommends against writing it — so the number
  that would refute it is one it declines to take.** A real weakness, and why
  the first follow-up is a profile rather than a port. *(A parser was
  subsequently written and it settled two phases of six.)*
- **The comparison baseline is not like-for-like, and the two errors push
  opposite ways.** Counting only the examples' tokens while the run also
  resolves the standard library **understates** the reference's throughput;
  crediting it with six phases where the Ply side does one **overstates** the
  gap. **Neither was separated.**
- **Every wall-clock figure was taken on a shared machine.** User CPU is
  reported precisely because it is the robust half; **if user CPU is itself
  distorted by cache contention at this load — not checked — the absolutes
  move.** The *shape* results do not depend on it.
- **The profile is a sampling profile of one workload on one input.** Symbol
  attribution in a release build can be distorted by inlining — **a builtin body
  inlined into the machine's step would be charged to dispatch.** The leaf
  histogram corroborates rather than assumes; **a counter-based attribution
  would settle it and none exists.**

## Provenance, and two things it cost

The statistic was pre-registered before any measurement existed and written down
while the binary was still building: minimum of N, load recorded before and
after every series, **prefer any deterministic counter over wall clock if one
exists.** No run was discarded after the fact.

**A deterministic counter existed, and nothing outside the evaluator crate could
read it.** The reference-counting statistics had counted updates against in-place
updates since the pass was written, read by three test files and with **no CLI
surface at all** — so "no counter exists" was true of the command and false of
the codebase, **and the consequence is the one the throughput section carries: a
document that needed a count timed something instead, and its absolutes did not
reproduce.** They are reported now, and **in-place counts are null on the
tree-walker rather than zero**, because that engine runs no reference counting
and **a zero there reads as a fact about the program when it is a fact about the
engine.**

**One measurement was discarded and re-taken.** The first attempt at the
shipped-module series used the wrong member spelling, so every run failed to
compile **and the harness timed the failure and reported it as a row.** The
harness now refuses to record a run whose output does not report success.
**Recorded because it is the same defect this document is about: a green number
over a program that never ran.**
