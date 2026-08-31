# ADR 0029 — Default and named arguments

Status: accepted — implemented in W4.
Date: 2026-08-30
Closes: the `assert`/`range` divergence recorded in
`crates/ply-cli/tests/failure_classification_audit.rs`, which documented half of
it in a comment and resolved neither half.
Constrained by: ADR 0001 (a definition's identity is its normalized structure),
ADR 0002 (the driver hashes before it infers, and gate 1 skips a file on its
content hash alone), **ADR 0023 (record update)**, which this follows in its
doctrine and departs from in two decisions, each argued below.

## Context

`Builtin::arity()` gave `assert` and `range` `(1, 2)`. `infer.rs` typed them
`(Bool) -> Unit` and `(Int, Int) -> List<Int>`. The evaluator had a whole
`assert_failure(message: Option<&Value>)` path for the message and a
`match args.len()` for the lower bound, and **no program the checker would
accept could reach either**. Both sides date to `0a66ed5`, the initial vertical
slice; they were still there 29 ADRs later.

Verified rather than assumed, against the shipped binary:

```
$ ply check t1
[E0202] Error: this function takes 2 arguments, but 1 was supplied
 1 │ fn xs() -> List<Int> = range(5)

[E0202] Error: this function takes 1 argument, but 2 were supplied
 3 │ test "message" { assert(no(), "the thing was not true") }
```

The `.plyx` path closes too — `artifact.rs` re-typechecks on open — so there was
no route in at all.

**How it was found is the part worth recording.** Not by a test, and not by a
compiler warning: by someone writing `docs/GUIDE.md` who read the evaluator,
wrote down what it did, and was refused by the checker when they tried their own
example. The arms were *covered* — `tests.rs` asserts the assert message lands
in a note, and `differential.rs`'s corpus builds `range(6)` — but every one of
those tests constructs an AST directly and never meets the checker. That is why
nothing went red for the whole history of the language.

## Decision 0 — Add the surface rather than delete the code

The alternative was to narrow both builtins and delete the arms. That was
costed and rejected for `assert`: a language whose thesis is the verification
loop wants a failing assertion to carry the author's sentence, and the code to
do it was already written, tested, and correct. It was *accepted* for `range`
(§Decision 5).

The larger reason is that adding the surface removes the whole class. After this
change **no builtin has a variable arity** — `Builtin::arity()` has `min == max`
for every variant — so the shape of defect that produced this ADR cannot recur
in a builtin, and `every_builtin_agrees_on_its_arity_everywhere` checks it as an
equality rather than an inequality.

## The constraint that shapes everything below

ADR 0023's, and ADR 0028's, unchanged:

> **`f(x)` must be the same definition as `f(x, <default>)`**, and
> `f(x, m: 1)` the same as both.

Otherwise adopting a default moves the caller's `DefHash` and every dependent's,
splits one value across two cache entries, and makes "this call means what it
meant" an assertion rather than a measurement. Asserted by
`ply-syntax:tests:an_omitted_argument_is_filled_with_the_default`, and measured
under §Consequences.

## Decision 1 — Expansion runs inside `resolve`, not the parser

ADR 0023 §"Decision 1" and ADR 0028 §"Decision 1" both put their expansion in
`parse_module`, and gave the same reason: a construct that survives into
`ply-hash`, `ply-core`, `ply-eval` and `ply-prove` is four implementations with
four chances to disagree. That doctrine is kept — nothing downstream of
`resolve` knows a default exists.

The *location* cannot be. Record update reads a shape out of the module in front
of it; `?` reads the enclosing `fn`'s written return type. A default lives in
the **callee's** module, so matching a call against a signature needs the whole
program, and `parse_module` sees one file.

`resolve` is the first point that has the whole program, and it is still before
the driver hashes — ADR 0002 pins the order `parse → resolve → hash → gate 2 →
infer`. That is the deadline that matters.

It runs *inside* `resolve`, which now takes `&mut Program`, rather than beside
it. That is what buys the same guarantee `parse_module` gives the other two
passes: **no entry point can build the name tables and skip the rewrite.** Six
call sites across `ply-cli`, `ply-hash`, `ply-test` and `ply-corpus` had to be
made mutable, and each got a comment where the rewrite is a no-op (a
reconstructed program's calls were positional and fully applied before they were
ever encoded).

`no_named_argument_survives_resolve_anywhere_in_the_tree` parses and resolves
every `.ply` in the repository, plus an appended file that actually uses the
syntax — without which the guard would pass whether or not the pass ran at all —
and asserts none survives.

## Decision 2 — Crossing a module boundary, where record update would not

ADR 0023 §"Decision 4" restricted record update to shapes declared in the same
file, and said why: gate 1 skips a file whose bytes are unchanged, so a shape
read across a boundary would leave a stale expansion behind in a file that never
moved. It priced the restriction at about one site in ten and accepted it.

**That argument does not carry here, and the difference is not a judgment
call.** A default is part of the callee's `DefHash`, and a spliced default is a
reference the caller now makes — so *two* of gate 1's conditions cover it, where
record update had neither. A record's field list is not a reference.

- `fingerprint.deps` holds every free name the file references with the hash it
  had, and a spliced default puts the callee there.
- `fingerprint.imports` holds a digest per imported module, and `exports_digest`
  covers each exported **name and its hash** — so a moved default moves the
  digest of the module that declares it.

**Which one fires was observed rather than reasoned about, and it is the second.
This section first claimed `deps`.** Editing only the default in a two-module
project, with the importer's bytes untouched:

```
$ ply check . --explain
   front end
     checked   src/main.ply import `src.palette` changed
     checked   src/palette.ply content changed
     rechecked src.main.wall
```

`src/main.ply` is refused by the import edge, re-parsed, and re-expanded; its
test then fails against the *new* default rather than passing against a stale
one. The `deps` condition would have caught it too, and is the one that survives
if the digest ever stops covering hashes — but the run says `import`, so this
says `import`.

Accepting the restriction here would also have cost far more than one site in
ten. The motivating case is `assert`, which every module calls.

**What this costs, stated rather than hidden.** A default written in module `A`
and spliced into module `B` is a reference `B` may never have imported.
`Resolved::lookup` requires the caller's scope to name the module and requires
the target to be `pub`. So expansion qualifies the default's free names against
the module that wrote them and registers an implicit module binder in every
scope the default lands in — including `A`'s own, which does not import itself
and which the checker types the default in.

The binder is the target's **dotted module name**. That is not cosmetic: a
written binder is a single identifier and can contain no `.`, so an implicit one
can neither capture a name the file uses nor be captured by one. The visibility
half of `lookup` is left in force, which is what makes the `pub` rule below
enforceable rather than advisory.

## Decision 3 — A default is a pure, closed expression

`E0121` refuses anything else, in `defaults::admissible`, before anything is
spliced. Two rules, one reason — *the expression is copied into the caller, so
it must mean there what it means here*:

- **Structurally pure and closed**, by `ast::is_default_expr`. A call or a
  `perform` would run at the caller rather than where it was written.
- **Naming none of its own signature's parameters.** Those do not exist at a
  call site. This rule is separate because its failure is *quiet*: a default
  mentioning a parameter that shares a name with a global would bind to the
  global and compile, meaning something nobody wrote.

`is_default_expr` is a sibling of `ast::is_pure` and not a call to it. `is_pure`
answers *may this be reordered*, and for that every `ExprKind::App` is impure —
a call runs. This asks something narrower, and `Some(0)` copies fine. Refusing
it would leave `Option`-shaped parameters — the case that motivated the whole
change — unable to state their own absent value.

`E0122` refuses a default on a `pub fn` that mentions a name the module does not
export. Checked once at the definition, not per call site: the answer does not
vary by caller, and a diagnostic on the signature is one a reader can act on.

The checker adds what this pass cannot: the default's *type*, unified with its
parameter's, in `check_fn_body` — deliberately **outside** the scope the
parameters are bound in.

## Decision 4 — Named arguments, and the one-sentence rule

Positional arguments fill parameters left to right; any parameter left over must
be named or have a default. `E0123` for a name that is not a parameter or is
given twice, `E0124` for a positional after a named one.

**`E0125` is narrower than it first was, and an existing audit is why.** It was
raised for any unfilled parameter, which quietly took over a case that predates
this feature: `f(1)` where `f` takes two is under-application, and it was
`E0202` from *inference* long before defaults existed. Reporting it here changed
both the code and the phase, and
`ply-eval:equivalence_audit:every_builtins_failure_mode` — written to fail when
"a case changed its mind about which failure it produces" — went red on exactly
that. It was right to.

So a call with no names in it that leaves a hole is handed back exactly as
written, and inference reports `E0202` in the words it always used. `E0125` is
kept for the one shape that genuinely has no predecessor: a hole with a *named*
argument in play, where the call cannot be spelled positionally and there is
nothing to hand back.

| written | reported |
| --- | --- |
| `f(1)`, `f` takes two | `E0202`, from inference, unchanged |
| `f(b: 2)`, `a` unfilled | `E0125`, from this pass |

Ordering is the *parser's* to enforce, because it is a property of the text.
Which names are legal needs the callee's signature and is `defaults::expand`'s.

Named arguments exist because without them a default is useful only in a
trailing position, and the two-argument `assert` would have been the only thing
this ADR could deliver. They are erased before hashing, so they add a spelling
and not a definition.

`E0120` refuses a default on a lambda and on an effect operation: neither is
reached by a name a call could be matched against, and a handler clause must
bind exactly what its operation declares.

## Decision 5 — `range` gets no default

Its lower bound is the **leading** parameter, so `range(5)` would fill `lo` and
leave `hi` empty. The spelling that works is `range(hi: 5)`, which is longer
than `range(0, 5)`.

A default that makes every call site worse is not one worth having, so `range`
is narrowed to `(2, 2)` and the one-argument arm is deleted. **This ADR does not
recover `range(5)`, and says so** rather than leaving the feature looking as
though it closed both halves of what prompted it.

## Decision 6 — A spliced default keeps its own span, and identical diagnostics are one

The first draft gave every spliced default the *call's* span, on the reasoning
that the reader is looking at the call. Measured against a wrong default with
three omitting call sites, that produced:

```
4 errors
  1 × type mismatch: parameter default   → at the default
  3 × type mismatch: argument type       → at src/main.ply:3, :4, :5
```

Three of the four named text whose author wrote nothing wrong. The default is
written in the callee, so a diagnostic about it belongs there; the span is now
the one it was written at. Spans are not normalized, so this does not touch the
identity `f(x)` and `f(x, d)` share.

That leaves the count. A bad default is checked once where it is written and
again in every call that omitted it — the splice is a copy, so each copy fails
the same unification — which is `1 + <call sites>` renderings of one mistake,
now all pointing at the same characters.

The obvious fix is for the checker to skip an argument it knows came from a
default. **That is exactly the knowledge Decision 1 keeps out of `ply-core`**,
and buying a diagnostic with it would put a notion of defaults into the crate
this ADR spent its first decision keeping clear. So the fix goes where the
question is only *do these two diagnostics differ*: `check_program_with` now
drops exact repeats — same code, severity, message, and every label's span and
text — preserving order. Two complaints a reader cannot tell apart are not two
pieces of information.

```
2 errors   (independent of how many call sites omit the argument)
```

## The parser's stack budget, which this nearly spent

`pathological_nesting_is_a_diagnostic_rather_than_a_stack_overflow` went red on
`[[[[…20,000…]]]]`, and the failure was **order-dependent**: the bracket case
overflowed only when the parenthesis case had run before it in the same test.
That is the signature of a margin, not a runaway — `MAX_DEPTH` was still doing
its job, and there was simply less headroom than there had been.

The cause is that `postfix_expr` runs once per level of a nested expression, so
its frame is paid per level and a program's whole nesting depth is bounded by
how large that frame is. Adding a named-argument list to the call branch, an
`ExprKind::App` built in place, and a `Diagnostic` builder in the `perform`
branch was enough to cost the margin — the compiler reserves stack for every
branch of the `match`, taken or not.

`f(..)` and `e.op[r](..)` now build in `#[inline(never)]` helpers, and the two
new diagnostics in `call_args` and `perform_on` are `#[cold]`. Nothing about the
language changed; a feature that adds a field to a node on the recursion path
owes this check, and this ADR records it because the next one will owe it too.

## Decision 7 — An additive hash tag, so no existing hash moves

`normalize.rs`'s `fn_def` writes `opt(p.ty)` per parameter, which emits
`tag::NONE` or `tag::SOME`. A parameter carrying a default writes a **new**
tag, `PARAM_DEFAULT`, *in place of* that byte, followed by the annotation and
the default.

A parameter with no default therefore writes the byte it wrote before this tag
existed. ADR 0023 refused to touch this stream at all, on the grounds that "a
cache-format change moves every cached result everywhere"; an additive tag is
how that is avoided rather than paid. `body.rs`'s decoder gained the matching
`param_slot`, so a reconstructed definition round-trips to the same hash.

The default is normalized **before** the parameter names reach `self.values`,
which is the encoding's statement that a default is closed.

A spec clause does **not** write the owner's defaults. An obligation is about
what the body promises; a default changes what callers pass, not what the
promise says.

## Consequences — the measurement

Every definition in `crates/ply-std/ply`, hashed by a binary built at this
branch's merge base and by one built here:

| | |
| --- | --- |
| definitions compared | 1211 |
| hashes that moved | **49** |
| of those, containing an `assert(` call | **49** |

So the 1162 definitions that do not call `assert` hash to the byte-identical
value they hashed to before defaults existed, and every mover is explained by
`assert(c)` becoming `assert(c, None)` — which is the *point*: that splice is
what makes the two spellings one definition, and the price is paid once, here.

`RUNTIME_VERSION` goes to `0.14.0` and `FRONTEND_VERSION` to `0.18.0`. The
second is required by its own listed rule — `assert`'s scheme is a prelude
signature. The first is required twice over: the moved call sites, and a failing
`assert` that can now carry a note it could not carry before.

## What this does not do

No defaults on lambdas, effect operations or constructors; each refuses with a
diagnostic rather than being ignored. No partial application — a function used
as a value still has its full arity, because `Type::Fn` carries `params`, `ret`
and `effects` and has nowhere to put a default. That is a rule with a
diagnostic, not a gap: a default is filled by matching a call against a
signature, and a call through a value has none.

**The payoff in today's tree is small, and this ADR would rather say so.**
Counting candidate sites the way ADR 0023 did: four public functions in the
corpus take an `Option`-typed parameter, and one builtin gains a capability. The
widest stdlib signatures — `http.ply`'s six- and seven-parameter functions — are
accumulator-threading loop helpers that defaults do not help. This is a bet on
ergonomics and a closed defect class, not a change that shortens much code
today.

## The test that should have existed from `0a66ed5`

`ply_eval::builtins::tests::every_builtin_agrees_on_its_arity_everywhere`.

A builtin is described in three crates that cannot see each other: its argument
count in `ply-eval`, its type in `ply-core`'s prelude, and its defaults in
`ply-syntax`. Nothing checked that the three agreed. `ply-eval` depends on both
others and is the only place the check can live; `ply_core::prelude_arity` and
`ply_syntax::defaults::builtin_shape` exist to be read by it and by nothing
else.

It was confirmed to bite on **each half** of the original defect, by
reintroducing each separately:

```
$ # Builtin::Assert => (1, 2)
assertion `left == right` failed: `assert` has a variable arity; every builtin
is exactly applied, and a call that leaves an argument out is filled by
`ply_syntax::defaults` before anything here sees it

$ # ("assert", mono(vec![Type::bool()], Type::unit()))
assertion `left == right` failed: `assert` takes 2 arguments here and 1 in the
prelude's scheme. Whichever is larger, the extra arm is unreachable from source.
```

A test that has not been watched to fail is a test whose passing means nothing,
and this one is the whole reason to believe the next such drift will be caught.
