# ADR 0029 — Default and named arguments

**Accepted, implemented.** Follows ADR 0023 in its doctrine and departs from it
in two decisions, each argued below.

## Context

Two builtins were declared with a variable arity in the evaluator and a fixed
one in the prelude's schemes. The evaluator had a whole optional-message path
for one and a lower-bound arm for the other, **and no program the checker would
accept could reach either.** Both sides date to the initial vertical slice; they
were still there twenty-nine records later. Verified against the shipped binary:
both spellings are argument-count errors. The deployed-artifact path closes too,
because it re-typechecks on open. **So there was no route in at all.**

**How it was found is the part worth recording.** Not by a test, and not by a
compiler warning: **by someone writing the user guide who read the evaluator,
wrote down what it did, and was refused by the checker when they tried their own
example.** The arms were *covered* — one test asserts the message lands in a
note, and the differential corpus builds the other form — **but every one of
those tests constructs an AST directly and never meets the checker. That is why
nothing went red for the whole history of the language.**

## Decision 0 — Add the surface rather than delete the code

The alternative was to narrow both builtins and delete the arms. **Costed and
rejected for the assertion**: a language whose thesis is the verification loop
wants a failing assertion to carry the author's sentence, and the code to do it
was already written, tested and correct. **Accepted for the other one.**

**The larger reason is that adding the surface removes the whole class.** After
this, **no builtin has a variable arity** — every one is exactly applied — **so
the shape of defect that produced this record cannot recur in a builtin**, and
the check is an equality rather than an inequality.

## The constraint that shapes everything

> **`f(x)` must be the same definition as `f(x, <default>)`**, and `f(x, m: 1)`
> the same as both.

Otherwise adopting a default moves the caller's hash and every dependent's,
splits one value across two cache entries, and **makes "this call means what it
meant" an assertion rather than a measurement.**

## 1. Expansion runs inside resolution, not the parser

ADR 0023 and ADR 0028 both put their expansion in the parser, with the same
reason: a construct that survives into four crates is four implementations with
four chances to disagree. **That doctrine is kept — nothing downstream of
resolution knows a default exists.**

**The *location* cannot be.** Record update reads a shape out of the module in
front of it; `?` reads the enclosing function's written return type. **A default
lives in the *callee's* module, so matching a call against a signature needs the
whole program, and the parser sees one file.**

Resolution is the first point that has the whole program, **and it is still
before the driver hashes** — the pinned order is parse, resolve, hash, gate,
infer. **That is the deadline that matters.**

**It runs *inside* resolution rather than beside it**, which buys the same
guarantee the parser gives the other two passes: **no entry point can build the
name tables and skip the rewrite.** Six call sites had to be made mutable, and
each got a comment where the rewrite is a no-op.

The escape guard resolves every Ply file in the repository **plus an appended
file that uses the syntax — without which the guard would pass whether or not
the pass ran at all.**

## 2. Crossing a module boundary, where record update would not

ADR 0023 restricted itself to shapes declared in the same file and said why:
gate 1 skips a file whose bytes are unchanged, so a shape read across a boundary
would leave a stale expansion behind in a file that never moved. It priced the
restriction at about one site in ten and accepted it.

**That argument does not carry here, and the difference is not a judgment
call.** A default is part of the callee's hash, **and a spliced default is a
reference the caller now makes — so *two* of gate 1's conditions cover it, where
record update had neither. A record's field list is not a reference.** The
fingerprint's dependency set gains the callee, and the imported-module digest
covers each exported name **and its hash**, so a moved default moves that
digest.

**Which one fires was observed rather than reasoned about, and it is the
second. This section first claimed the other.** Editing only the default in a
two-module project, with the importer's bytes untouched, the explain output says
the importer was refused **by the import edge**, re-parsed and re-expanded, and
its test then fails against the *new* default rather than passing against a
stale one. **The dependency condition would have caught it too, and is the one
that survives if the digest ever stops covering hashes — but the run says
import, so this says import.**

Accepting the restriction here would also have cost far more than one site in
ten: **the motivating case is the assertion, which every module calls.**

**What this costs, stated rather than hidden.** A default written in one module
and spliced into another is a reference the second may never have imported.
Resolution requires the caller's scope to name the module and the target to be
public — so expansion **qualifies the default's free names against the module
that wrote them and registers an implicit module binder in every scope the
default lands in**, including the declaring module's own, which does not import
itself.

**The binder is the target's dotted module name, and that is not cosmetic**: a
written binder is a single identifier and can contain no dot, **so an implicit
one can neither capture a name the file uses nor be captured by one.** The
visibility half of resolution is left in force, **which is what makes the export
rule below enforceable rather than advisory.**

## 3. A default is a pure, closed expression

Refused otherwise, before anything is spliced. Two rules, one reason — **the
expression is copied into the caller, so it must mean there what it means
here.**

- **Structurally pure and closed.** A call or a perform would run at the caller
  rather than where it was written.
- **Naming none of its own signature's parameters.** Those do not exist at a
  call site. **This rule is separate because its failure is *quiet*: a default
  mentioning a parameter that shares a name with a global would bind to the
  global and compile, meaning something nobody wrote.**

The predicate is a *sibling* of the reordering-purity predicate and not a call
to it. That one answers *may this be reordered*, and for that **every**
application is impure, because a call runs. **This asks something narrower, and
a constructor application copies fine. Refusing it would leave optional-shaped
parameters — the case that motivated the whole change — unable to state their
own absent value.**

A default on a public function mentioning a name the module does not export is
refused **once at the definition, not per call site**: the answer does not vary
by caller, and a diagnostic on the signature is one a reader can act on.

## 4. Named arguments, and the one-sentence rule

Positional arguments fill left to right; any parameter left over must be named
or have a default.

**One code is narrower than it first was, and an existing audit is why.** It was
raised for any unfilled parameter, **which quietly took over a case that
predates this feature**: under-application was an inference-time argument-count
error long before defaults existed. Reporting it here changed both the code and
the phase, **and a test written to fail when "a case changed its mind about
which failure it produces" went red on exactly that. It was right to.** So a
call with no names in it that leaves a hole is handed back exactly as written,
and inference reports what it always did. The new code is kept for **the one
shape that genuinely has no predecessor: a hole with a *named* argument in play,
where the call cannot be spelled positionally and there is nothing to hand
back.**

**Ordering is the *parser's* to enforce, because it is a property of the text.
Which names are legal needs the callee's signature** and belongs to the
expansion.

**Named arguments exist because without them a default is useful only in a
trailing position**, and the two-argument assertion would have been the only
thing this could deliver. **They are erased before hashing, so they add a
spelling and not a definition.**

Defaults on lambdas and on effect operations are refused: neither is reached by
a name a call could be matched against, **and a handler clause must bind exactly
what its operation declares.**

## 5. One builtin gets no default

Its lower bound is the **leading** parameter, so the short call would fill the
wrong one and the spelling that works is longer than the longhand. **A default
that makes every call site worse is not one worth having**, so it is narrowed
and the unreachable arm deleted. **This does not recover the short form, and
says so** rather than leaving the feature looking as though it closed both
halves of what prompted it.

## 6. A spliced default keeps its own span, and identical diagnostics are one

The first draft gave every spliced default the *call's* span, on the reasoning
that the reader is looking at the call. Measured against a wrong default with
three omitting call sites, **three of the four errors named text whose author
wrote nothing wrong.** The default is written in the callee, so a diagnostic
about it belongs there. Spans are not normalized, so this does not touch the
identity the two spellings share.

That leaves the count. A bad default is checked once where it is written and
again in every call that omitted it — the splice is a copy, so each copy fails
the same unification — **which is one plus the call-site count renderings of one
mistake, now all pointing at the same characters.**

**The obvious fix is for the checker to skip an argument it knows came from a
default. That is exactly the knowledge decision 1 keeps out of the checker**,
and buying a diagnostic with it would put a notion of defaults into the crate
this record spent its first decision keeping clear. So the fix goes where the
question is only *do these two diagnostics differ*: exact repeats are dropped,
order preserved. **Two complaints a reader cannot tell apart are not two pieces
of information.**

## The parser's stack budget, which this nearly spent

A pathological-nesting test went red, and the failure was **order-dependent**:
one shape overflowed only when another had run before it in the same test.
**That is the signature of a margin, not a runaway** — the depth bound was still
doing its job, and there was simply less headroom.

The cause is that the postfix expression parser runs once per level of a nested
expression, **so its frame is paid per level and a program's whole nesting depth
is bounded by how large that frame is.** Adding a named-argument list, a node
built in place, and a diagnostic builder in a neighbouring branch was enough to
cost the margin — **the compiler reserves stack for every branch of the match,
taken or not.**

The two builders moved into non-inlined helpers and the diagnostics became cold.
**Nothing about the language changed; a feature that adds a field to a node on
the recursion path owes this check, and this is recorded because the next one
will owe it too.**

## 7. An additive hash tag, so no existing hash moves

The normalizer writes an option tag per parameter. A parameter carrying a
default writes a **new** tag *in place of* that byte, followed by the annotation
and the default. **A parameter with no default therefore writes the byte it
wrote before this tag existed.** ADR 0023 refused to touch this stream at all,
on the grounds that a cache-format change moves every cached result everywhere;
**an additive tag is how that is avoided rather than paid.** The decoder gained
the matching slot, so a reconstructed definition round-trips to the same hash.

The default is normalized **before** the parameter names enter scope, **which is
the encoding's statement that a default is closed.**

**A spec clause does not write the owner's defaults.** An obligation is about
what the body promises; **a default changes what callers pass, not what the
promise says.**

## Consequences — the measurement

Every definition in the standard library, hashed by a binary built at the merge
base and by one built here: **the hashes that moved are exactly the definitions
containing a call to the one builtin that gained a default.** Every other
definition hashes to the byte-identical value it did before defaults existed,
and every mover is explained by the one-argument call becoming a two-argument
one — **which is the *point*: that splice is what makes the two spellings one
definition, and the price is paid once, here.**

Both versions bump. The front-end one is required by its own listed rule, since
a prelude signature moved. The runtime one is required twice over: the moved
call sites, **and a failing assertion that can now carry a note it could not
carry before.**

## What this does not do

No defaults on lambdas, effect operations or constructors. **No partial
application** — a function used as a value still has its full arity, because the
function type carries parameters, result and effects and has nowhere to put a
default. **A rule with a diagnostic, not a gap: a default is filled by matching a
call against a signature, and a call through a value has none.**

**The payoff in today's tree is small, and this would rather say so.** Four
public functions in the corpus take an optional-typed parameter, and one builtin
gains a capability. **The widest stdlib signatures are accumulator-threading
loop helpers that defaults do not help. This is a bet on ergonomics and a closed
defect class, not a change that shortens much code today.**

## The test that should have existed from the first commit

**A builtin is described in three crates that cannot see each other**: its
argument count in the evaluator, its type in the checker's prelude, and its
defaults in the syntax crate. **Nothing checked that the three agreed.** The
evaluator depends on both others and is the only place the check can live; two
small accessors exist to be read by it and by nothing else.

**It was confirmed to bite on *each half* of the original defect, by
reintroducing each separately** — one failure says the builtin has a variable
arity where every builtin is exactly applied; the other says the two tables
disagree and **whichever is larger, the extra arm is unreachable from source.**

**A test that has not been watched to fail is a test whose passing means
nothing, and this one is the whole reason to believe the next such drift will be
caught.**
