# The Ply front end's port to Ply: where it stands, and what is next

Written at the end of a long working session. `README.md` says what the spike is;
this says what the *port of the C emitter* has and has not, so the next pass
starts from the findings rather than rediscovering them.

## The two instruments, and what they say

Both are in `harness/tests/`, both run from `./run.sh`, both ratchet.

| | |
| --- | --- |
| `lower_diff.rs` | the port reaches **1179 of 1200** bodies, **1179 compared** |
| `emit_diff.rs` | 48 of 48 hand-written bodies; **every one of the 1282** shipped bodies the reference emits, each resolving to the reference's C |

`reached` and `compared` were apart for most of this work, by the record updates
the lowering excluded. They now meet: the exclusion is gone.

The emitter differential pins the reference's inlining at **zero** and passes it
explicitly. What is compared is the *emitter*; the inliner is a stage of its own
and this port does not have it.

## Nothing is switched off

Every piece written for this port is on. The runtime's field read by name, once held back
behind a `None ->` because it opened bodies that then disagreed, is the reference's own
fallback and emits wherever the shape is not in reach.

## The rules the corpus taught, all recorded at their site

Releases, in the order they were found. Each was a real difference:

1. A release counts how many times the tree reads **any** name for the object,
   not how many times one slot is read.
2. A field whose own kind is a **word** does not release its base -- and the
   generic read, which answers a word, does not either.
3. An update emits its own build, out of the base's memory, released first.
4. An update at the **tail** releases its base however often the body reads it.
5. An arm reached through a scrutinee is **still a tail**.
6. The record an update builds is the body's own.

Ownership and ordering:

- A compiled call takes **one** pass over its arguments; a builtin takes **two**;
  `map` and `filter` take one. Each is wrong in the others' place.
- An aggregate's items are held as they are evaluated; a call's are not.
- Parameters are opened at entry, in declaration order, whatever the body reads.
- The unit's tables are the **unit's**: a lambda's constants, shapes and builtins
  keep counting from what the function that builds it met.

## The disagreements

`emit_diff.rs` compares each shipped body on the C it *resolves* to -- every table placeholder
replaced by its entry -- and holds two floors and a ceiling: bodies reached, bodies agreeing,
bodies disagreeing. It no longer asserts the disagreeing names; the test prints them, and
`PLY_EMIT_DIFF_LIST=1` marks each reached body `agrees` or `differs` so two runs diff. A body
the port emits differently runs under the tier's audit like every other, so a disagreement is
a slower body or a rule not yet taken, never a silent wrong one -- the audit is the oracle.

None disagree. The ceiling is zero, and the test says to raise it only for a body that is
right by the audit and slower on purpose, with the reason named beside it.

## What is not started

The **inliner**. The differential pins inlining at zero precisely because of it,
and porting `opt.rs` is what lifts that pin.

## What changed after this was written

**`Float` and `Decimal` literals, and the rule under `Int` arithmetic.** A `Float` or `Decimal`
literal is a pooled constant the runtime holds, written in the tables as its source text (`X`
and `D` entries) for the driver to convert by the lexer's rule, and held opaque so every
operator over it is the machine's own through the runtime. Compiling those bodies found a rule
both emitters had wrong: the inline `Int` arithmetic took any two words that were not marked
opaque, and two words of a type the port cannot see may both be `Decimal`s. The inline path
now needs one operand in a register, which the checker makes an `Int` on both sides; two
unknown words go through `rt_binary`, and a negation of one through `rt_negate`. The answer
of a runtime operator takes the reference's type: a `Bool` for a comparison, opaque over an
opaque left operand, the right operand's otherwise.

**Effects, by evidence passing (ADR 0043).** The port emits `handle`, `perform` and `with cell`,
none of which the reference emits, so they compile only with the chain entered whole and are
held to the audit alone. A `handle` site pushes a runtime frame holding each clause as a
closure, runs its body inline with every failing call landing at the site -- `check` writes a
`goto` inside a handle body -- and lands through `rt_handle_land`, which pops the frame,
applies the `return` clause, passes an error on, or catches an unwind aimed at it. A
`perform` is `rt_perform` over the frames, with the effect, operation and resource named
through the field table and the operation's mode beside them. A clause that binds `resume` is
carried only when every use of the binder is a tail call. Effect names resolve program-wide
through the module's scope, with the prelude's three known by their bare names, and the
tables gain the operations a body performs and handles, as `effect#op`, for the unit's
fixpoint. A body's reads inside a clause count in the clause's own window, as a lambda's do.

**The port emits everything the reference emits.** The last four census items closed: a
lambda inside a lambda is one more of its owner's, its entry interned before its body and its
text written after its owner's, and it runs in a frame of its own -- fresh alias, release and
read-count tables, and no tail -- as the reference gives every lambda a frame; `iterate` over
a one-parameter lambda is the loop, emitted into the body with the lambda's parameter as the
state and a body that answers `Stop` or `Continue` in every arm written as the loop's own
assignments; a definition used as a value is a closure over its entry with nothing captured,
or a call when nullary, and a builtin used as a value goes through the runtime. Two rules
came with them: a lambda body's reads count in the lambda's own window, a capture's charged
to the object it copies and the copy itself not a read; and a fused body's counts are the
lambda's own. Of the 1282 shipped bodies the reference emits, the port emits 1282, each
resolving to the reference's C. The census has nothing left to say over that corpus.

**Every width, and the record held back.** The kind carries a width: the six below
sixty-four bits open at entry, read at a field's offset and join in an `if` in their own C
type; arithmetic is done wide and narrowed back under the overflow guard; a shift is tested
against the width; the bitwise operators, `~`, `wrap_add` and its family, and the sixteen
conversions are in place over a carried width and the runtime's call otherwise; the two
widths at sixty-four are words the reference treats as opaque. With widths in, every state
record in `std.hash` is flat, and a flat record built from locals is *held back*: its local is
declared at the top, it is built only where something asks for its word, its fields are read
from the values it was built from, an update copies from those values, and its release is
guarded on whether it was ever built. One record builder serves the literal, the update and
the `if` join, and the flat flag the port had hard-wired to zero comes from it. 1239 to 1262
reached, and nothing disagrees. The census leads with a lambda inside a lambda.

**Any callee is a value.** A call whose callee is not a name -- a field holding a function,
a call's answer -- goes through the runtime as a call of a bound variable already did: the
callee held once more, then each argument as it is evaluated, then `rt_call`. One function
serves both. 1213 to 1239 reached, every new body the reference's C exactly. The census
leads with a `Float`, `Decimal`, fixed-width or unit literal, then a lambda inside a lambda.

**An `if` joins on the kinds its arms answered.** The port guessed each arm's kind
structurally before writing either, and refused where the guesses differed, on the belief
that the reference read the checker's type for the join. It does not: it writes both arms
into buffers, joins on the kinds they *answered* -- the same kind, or a word -- and converts
each arm into the join. The port does the same now, and the structural guess is gone. 1173
to 1213 reached, every new body the reference's C exactly. The census leads with calling a
value that is not a name.

**A callback is any callee, and an inline builtin has a slow arm.** `fold` fuses over any
third argument now, as the reference does: a definition of arity two the body names directly
is called straight per element, with no closure, and anything else is held as a value once
outside the loop and called through `rt_call`. `iterate` goes through the runtime's helper
unless it is the loop the reference emits into the body -- over a one-parameter lambda --
which this port has not read yet and refuses; `map_fold` is a helper. `len`, `bytes_len` and
`bytes_at` over a value the declaration does not fix are a kind test with the header's own
field in the fast arm and the runtime's call in the slow one, which is where a value of the
wrong kind is caught; `bytes_concat_all` over a written list joins without building it and
over anything else is the call; `bytes_join` and `string_concat_all` were never inlined by the
reference and are calls. One thing was found in the reference on the way: its kind-tested arm
for `bytes_u32_le` read one byte wide. The reference was corrected to go through the runtime
for that shape, and the port does the same. 1128 to 1173 reached; `std.hash.blake3` opened
and joined the widths family. The census leads with an `if` whose arms answer different kinds.

**A closure captures a parameter the prologue did not open.** A record or list parameter is
never in the window -- it is read as its own word where the read is -- and a capture looked
only in the window, so every closure over such a parameter was refused. The capture reads
it as a variable read does. A constant named inside a lambda then showed the lambda's code
table starting empty where the reference continues the body's; it continues now, as the
lambda's other tables already did. 1096 to 1128 reached, every new body the reference's C
exactly. The census leads with a callback or inline builtin outside its fused shape.

**Every pattern the reference tests, this port tests.** A pattern's test can now emit the
reads it needs -- `pat_test` answers the state beside the condition -- so a constructor's
argument with a test of its own is read into a local under the test built so far, as the
reference does; a record pattern is `rt_record_fits_p` and one `rt_record_has_p` per field
through the body's field table; a list pattern is `rt_list_fits_p` and `rt_list_at_p`, with a
refutable `..rest` refused on both sides; a `Bool` or a fixed-width literal is a word
comparison. Binding follows the same shapes. 1005 to 1096 reached, every new body the
reference's C exactly. The census now leads with a capture the port has not bound, then a
callback or inline builtin outside its fused shape, then an `if` whose arms answer different
kinds.

**A field's type travels, and an update's copy reads the base's rank.** A field read over a
shape the port cannot see is the runtime's read by name through the body's field table, as the
reference's is, so the `None` arm of `field_at` is an emission rather than a refusal. That
alone opened bodies that then disagreed, and the resolved-C comparison found why: a lambda
handed back its constants and shapes but not the fields it interned; a record type carried
each field's *kind* and not its type, so a read of a read was generic; a parameterised alias
did not substitute its arguments, so `Ran<http::Response>` hid `response.status`. `FieldTy`
carries `ty` now, `Alias` carries its parameters and `written_kind` binds them where the
argument was written. And the last one was wrong C rather than a slow body: the lowering
recognises `{headers: acc, count: count, next: l.next}` as an update of `l`, and the port
read the copy at `next`'s rank in the *new* record, which is `l`'s own rank only when the
two shapes agree. `std.http.field_line` read `stop` for `next`. The copy reads at the base's
rank now, by name when the base's shape is unknown, as the reference does. 842 to 1005
reached, 991 the reference's C exactly.

**The release family closed, and it was the tail flag.** Nine bodies differed on a release the
reference makes and the port did not, and the four guards were already the same on both
sides. What differed was position: the port threads "nothing is emitted after this" as a flag
on its state, and two places dropped it -- a block emitted its tail expression with the flag
its statements had cleared, and a `match` emitted its second arm with the flag its first
arm's body had cleared. Both restore the node's own answer now. The once-per-binding guard
is keyed on the object the local holds rather than on the slot, as the reference keys it,
so a name each arm binds afresh releases in each arm. 991 to 1000 of 1005.

**`&&` and `||`.** The port emits the short-circuit operators as the reference does, the
right operand inside the branch: 789 to 842. The three bodies that opened differ on the same
release rule as the others in the named gaps -- seven now, one family, the next pass.

**The constant table.** A pure nullary definition answering a word is asked of
`rt_constant` as the reference asks it; purity is the checker's word, through
`infer.pure_definitions`, so the port runs its checker over the program before it emits.
The body's code table (`Em.lambdas`) holds the constants' entries beside the lambdas', and a
closure names its code by the entry's place in it. A `let` keeps its value's whole type now,
as the reference does, and a type of another module is a word. That was 623 to 789. What it
opened, three bodies that differ on the port's release rules and on declared widths, is in
the named gaps.

**A refusal carries its reason now.** `Em.refused` holds the first reason a body was refused;
a refusal poisons the state and emission runs out, so no signature changed. `emit_fn_why`
reads the reason, `emit_refusals_all` lists every refused body of a program, and the census
test in `harness/tests/emit_diff.rs` aggregates them by reason over the bodies the reference
emits. Read that census before choosing what to build next: as this was written it put a
nullary call of a definition first (the constant table wants the purity the checker
publishes), then a field read over a shape the port cannot see, then a pattern it cannot
test, then an operator it does not emit, then a parameter with no written type.

`std.json.float_json` is gone from the gaps: a lambda's emission restarted the body's
constant and shape tables instead of continuing them, which the tables differential
below is what caught.

The port emits a **whole program** now (`emit_all_program`): every module parsed, rewritten,
its derives expanded and then resolved together, so a call's default arguments are filled
and a callee's declared type is in reach across modules -- which is what the reference's
emitter sees. That closed `std.http.check_limits` (a default argument's constant) and
opened the **test roots**: a test is a nullary body named `test#<n>`, and the port emits
them as the reference does. With an expression statement's value discarded and two `Int`
literals under an operator folded as the reference folds them, the port went from 487 to
605 shipped bodies. The census in `emit_diff.rs` says what keeps it out of the rest: field
reads over shapes it cannot see, lambdas, and updates, in that order.

The port is the C tier's **producer** now, behind `PLY_C_EMITTER=ply:<dir>`
(`crates/ply-codegen/src/c/producer.rs`). `emit_fn_full` answers the text and the
tables a body names, `emit_bodies_in` frames every body of a module by length,
and the tier reads that where it reaches and emits the rest itself. The third
emitter differential holds the tables to the reference's wherever the text agrees.

The port reads **opaque** operands now: a written `Float` or `Decimal` is `TyOpaque`, a
builtin over an opaque argument answers opaque, opacity survives a `let`, and an operator
over one goes through `rt_binary_p` -- the machine's own operator -- as the reference does.
An `if` whose one arm is opaque joins at a word; every other join of differing kinds is
still refused. What is deliberately *not* kept across a `let` is the rest of a value's
type: the reference keeps it whole, and keeping a record type here reaches two more bodies
(`std.hash.round`, `std.http.method_not_allowed`) that then differ on declared `U32` field
widths and on an update's release -- the next two shapes to run down.

ADR 0042 moved the emitter's oracle. The byte-exact differential above stays as
an instrument, but the port is no longer held to agreeing with `c/emit.rs`'s
text: it is held to the suite passing under it as the C backend's producer, and
to the fixpoint of compiling itself. The two switched-off pieces and the named
gaps are therefore things to *make work*, not things to make byte-identical, and
the release rules above are the reference emitter's heuristics rather than
requirements. Read that record before the sections above.

The port emits `simulate`. The node's body is a nullary lambda over the region's captures,
handed to `rt_simulate_p` as a closure word; the site notes the eight operations a region
answers as handled, so the fixpoint takes a `perform` of `task`, `clock` or `random` whose
nearest region is in the unit, and `task` joined the prelude effects the port resolves. The
reference emits none of it, so the ratchet is unchanged; the instrument for this shape is the
audit with every simulated test's seeds paired against the machine, green over both corpora.

A `handle` with a clause that binds `resume` and calls it off the tail is no longer refused:
the body becomes a nullary closure over every slot it reads that the enclosing scope binds, a
parameter included, at the same slot numbers so nothing in it is renumbered, handed to
`rt_handle_detached_p` with the clause table, in which the resume column says `2` for such a
clause. The runtime resumes such a body as often as the clause asks. The port compiles `secret_of_string`, as the reference now does: a
credential is a bridged value inside an entry, and the runtime's memo is what keeps one from
outliving it.

The port emits itself. Its own sources joined both differentials: the lowering one, where nine
disagreements were two bugs in the update recognition, a record literal not recognised as an
update of a local it projects from and an update's base resolved on a stack that was thrown
away, which lost a lambda its capture; and the resolved-C one, where the port's own bodies are a
ratchet of their own with floors and not yet an emptiness. The audit of the port's own tests
under the tier is green. `spikes/ply-parser/bootstrap/` is the C the port emitted for itself,
compressed, with the unit's record and the digest of the sources it came from; the CLI builds
the producer from it rather than with the reference, `PLY_C_BOOTSTRAP=off` asks for the
reference again, and `PLY_C_BOOTSTRAP_REFRESH=1 cargo test -p ply-codegen-tests --test bootstrap`
rewrites the bundle from the fixpoint's own emission. The fixpoint test is the check: the emitter
built from the bundle emits, for its sources, C that builds an emitter that emits the same C.

The port's output is now built as lines joined once per body and frames joined once per dump,
which the measurement in ADR 0045 §"What the fixpoint measured" showed was not where the
memory went: the tier released nothing within an entry. ADR 0046 built the release: `emit.ply`
carries the machine's ownership rule under `Em.release`, on for everything the producer emits
and off for `emit_bodies_reference`, which is the text `emit_diff.rs` still compares against
the reference. The three places release departs from the reference's order, and the two the
reference had wrong, are in that record.

## Beyond the port

`docs/adr/0041-effects-in-a-compiled-tier.md` carries the effects work: `with
cell` ships in both tiers, `perform`'s whole-program criterion is built
(`Source::stack_handled`), and answering a `perform` needs a decision about the
seam rather than more code -- `Compiled::enter` takes `&self` with the machine
already borrowed.

The evaluator is not started, and the thing to settle before it is what oracle it
is held to. Every error found here was found by a byte-exact one.
