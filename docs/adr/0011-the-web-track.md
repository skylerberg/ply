# ADR 0011 — The web track

**Accepted, implemented.** This record merges the six milestone contracts that
built a service on top of ADR 0008's host boundary — the boundary itself, the
standard library and derivation, HTTP and TLS, the database, operations, and the
performance verdict that closed the track. They were separate documents
(0011–0016) written before their milestones and mostly transcribing surface the
code now owns; what is kept here is the reasoning that would otherwise be
re-derived.

Ordering note: the `Isolation::Host` variant the boundary milestone promised
does not exist and never did. A host atom is an ordinary contending atom; the
host category is computed at the CLI, because the test scheduler classifies from
a footprint alone and has no binding to ask.

---

# I. The boundary

> Every guarantee Ply has rests on the runtime knowing what a computation can
> do. A host handler is the one place that knowledge can be wrong. So the
> boundary is built so that a **wrong declaration is loud** and a **missing
> declaration is fatal**, and never the other way round.

Three corollaries, each of which decides a design question:

1. **A host binding may not change what the front end computes.** If binding a
   Rust handler moved a row, a determinism verdict or a hash, then `ply check`
   would answer differently with and without the host flag and **the cache would
   split on a command-line flag.**
2. **A host binding may not change what a green result means.** A run that
   reached the host is never written to the result cache, and a pass earned
   hermetically never satisfies a host run.
3. **When the static picture and the dynamic one disagree, the dynamic one wins
   and says so.** The declared footprint is a trusted claim; the runtime checks
   the claim at every host answer and refuses loudly on a mismatch.

## Registration is a list you can read top to bottom

A registry is *built* by a function, not assembled by attribute macros or
inventory-style global constructors. **The trusted computing base is a list in
one file**, which is the whole point of ADR 0008's enumerability argument.

## Determinism: the arrow is reversed

The naive design has the binding mark an effect `nondet`, which then flows into
the determinism check. That is wrong here, by corollary 1: the determinism
verdict decides the *result* cache key, and **a verdict that depends on whether
a flag was passed splits every cache in the system and makes `ply check`
disagree with itself.**

**So the declaration is the authority and the handler is checked against it.** A
handler registered non-deterministic requires its effect declared `nondet` in Ply
source; otherwise it is a bind-time error naming both.

The existing check then needs **no change whatsoever**. An effect declared
without `nondet` simply cannot have a real socket behind it; adding it is a
source edit, it moves the hashes it should move, and every deterministic test
that reaches the operation fails to compile — **the guarantee ADR 0008 asks for,
obtained from machinery that already exists rather than from a second path into
it.** The flag is consulted in exactly two places: bind, and the listing.
Nowhere in inference, nowhere in the cache key, nowhere in the evaluator.

## Linearity, and a deliberate over-approximation

The machine counts host operations answered under an at-most-once registration;
every continuation records that count at capture and carries a resumption
counter. A resumption is refused when it is not the first **and** an
irreversible host operation has happened since the capture.

A first resumption is always allowed. A second is allowed whenever no
irreversible host operation has happened since the capture — which is *always*
in hermetic mode. **So multi-shot continuations are entirely unaffected
everywhere they are currently used.**

**This over-approximates, deliberately.** The rule refuses a second resumption
when an at-most-once operation happened anywhere after the capture — in another
task, or in the handler clause rather than inside the continuation — even though
replaying that particular continuation would repeat nothing. The precise rule
needs a per-resumption liveness scope on the control stack: a new frame kind
interacting with capture, splice and task start, **in the one part of the system
where a defect is silent and sends a packet twice.** The conservative rule is
four lines and one counter, and its false positive is a diagnostic on a program
that is unusual to begin with. **That trade is the whole "when in doubt, refuse"
posture.**

A *repeatable* registration marks an operation whose replay changes nothing
outside the program — a clock read, a read of an immutable resource — and such
operations do not touch the counter. **A handler author choosing repeatable for
a socket write is making a false claim of exactly the kind the listing exists to
put in front of a reviewer**, which is why the flag is printed per handler and
folded into the TCB digest.

## Hermetic by default, and a distinct diagnostic for it

The registry is compiled in even under a hermetic run, so the *diagnostic* can
name the handler that would have served the operation. What is absent is the
binding.

**Reaching the boundary with nothing bound is deliberately not the
unhandled-effect error.** That error means inference should have prevented this
and did not; it is a bug-catcher. This one means inference was right, the row was
legal, and the run was configured hermetically. **The two call for opposite
responses — file a bug, versus pass the flag or write a test double — and a
consumer that cannot tell them apart will do the wrong one.**

Selection under the host flag stays exact: a test's footprint is an upper bound
on what it performs, so the tests that can reach the host are exactly those whose
footprint intersects the binding's. Those always run and are never cached.

## The footprint check, and exactly what it defends

When the machine answers a perform from the host binding it checks the performed
atom against the declared footprint of the entry point being run. A mismatch is
Ply's fault and is not bisected — the same class as an engine divergence,
because the run knows two of its own answers disagree and nothing in the
definition graph decides which was meant. It is a set lookup per host operation,
against a syscall.

**It must be armed by every command that runs an entry point**, because one
machine serves many tests per worker and a claim that outlived its entry point
would judge the next test by the last one's row. **A check nothing installs is
not a defence**: it was unarmed in `ply test` once, and a deterministic,
isolated test opened a real TCP listener and was reported green.

**What it defends is narrower than "a footprint that under-reports".** It
compares the atom the *registry* resolved against the entry point's row, so it
catches a program footprint that under-reports what the program performs, and a
binding that resolved an atom the program's footprints never enumerated. It does
**not** catch a handler that does more than its registration declared, and it
cannot: a handler is handed the atom and has no way to report a different one.
ADR 0008 states that residual explicitly rather than leaving a reader to infer a
backstop that is not there.

## The scheduler seam

The decision that makes the production scheduler small: **a Ply task cannot move
between OS threads.** A value holds non-atomic reference counts and a machine is
single-threaded by construction. So the production scheduler is *not* one task
per thread — it is the same cooperative scheduler over the same machine,
choosing by real readiness instead of by a seed, with real threads confined to
the reactor and blocking pool where no Ply value ever goes.

That is why there is **one** scheduler type with a policy, rather than a second
implementation in the host crate. **A second implementation is exactly the drift
the simulated scheduler exists to prevent**: *the signature is declared once so a
production handler and a seeded one cannot disagree* is worth nothing if the two
are separate code.

**Mutual exclusion, in three independent locks**, no one of which is
load-bearing alone. *Type level*: `task` is `nondet`, so only a hand-opted-in
nondet test can reach a production scheduler at all. *Stack level*: handler
search walks the stack innermost-first before ever consulting the host binding,
which is the handler of last resort — **so a spawn inside a simulated region
reaches the seeded scheduler always, with no special case and no ordering to get
wrong.** *Binding level*: nothing is bound without the flag.

The production region is opened **lazily**, at the first task operation that
reaches the binding. **Opening it eagerly around every entry point would make
every existing simulated region nested and refused under the host flag.**

## Two dependency choices worth keeping

A single-threaded async runtime, deliberately **not** the work-stealing one: a
work-stealing runtime is unusable here, because nothing it would steal is
`Send`. What it earns its place for is the reactor and the timer wheel. **The
blocking pool is owned rather than borrowed from the runtime's default**, so its
size is a declared, reviewable number rather than a default nobody chose — and
that number decides how many real database connections a runaway test can open.

An HTTP parser dependency was rejected here on the grounds that this milestone's
endpoint returns a fixed response: **a parser in the trusted computing base can
wait until the milestone that needs one, and then it is that milestone's
decision with that milestone's evidence.** (It was decided the other way; see
part III.)

## What the boundary does not settle

**Making a false footprint detectable.** Nothing catches a handler that does
*more* than its registration declared, and nothing in this design can.

**A handler that blocks while declaring it does not.** The structural half is
caught; the stall itself has no budget and no watchdog, and adding one would
make a diagnostic depend on wall-clock time, which is the one thing every other
verdict in this system is built to avoid.

**Cancellation.** A pending token has no cancel path, so a task blocked on a
host operation blocks until it completes or the run ends. A real gap, deferred,
and it is still open at the end of the track.

---

# II. The standard library, `Map`, derivation and numerics

> Every new thing here is a **value or a definition like any other**. A stdlib
> definition hashes like a project one, a derived definition hashes like a
> hand-written one, and a `Map` is a value whose canonical form is a function of
> its contents. **Nothing gets a private channel into a cache key, a hash, or an
> iteration order.**

Three corollaries: nothing outside a definition's own reachable graph may enter
its hash — not the `std` prefix, not a stdlib version, not the module a `derive`
was written in; **any order a program can observe must be a function of the
values, not of history**; and a verdict may only get stronger when the evidence
does — the prover's fragment does not extend by a type arriving.

## The stdlib is source, loaded on demand

An import no project module satisfies pulls the module out of an embedded table
and repeats transitively. A program importing nothing from it loads nothing and
has hashes byte-identical to what it had before.

**A stdlib definition normalizes exactly as any other** — no marker, no version,
nothing about the module enters the bytes. Two consequences, both required
properties: **copying a stdlib source file into a project produces definitions
with the *same hashes*, sharing its cache entries** — the same sentence as
"moving a definition between modules changes no hash"; and **a compiler upgrade
that does not change a stdlib source file changes no hash and re-runs no test,
even though every byte of the compiler moved.**

**The stdlib digest is deliberately not in any cache key.** A digest in the key
would invalidate a project on an edit to a module it never imports, **which is
precisely the conservative selection this system exists to beat.** What an
upgrade needs is not correctness but *visibility*: the store records the digest
it was last written under and warns once when it differs, naming **the number of
definitions this program reaches whose hash moved** — which is often zero, and
the warning says so rather than implying work happened.

Gate 1 keys on raw file content and an embedded module has no file, so its
fingerprint is over the embedded bytes under a pseudo-path no discovered file
can produce. **Tests in a stdlib module are not selected by a project's run**,
and are run by the compiler's own suite instead — **without that rule a
project's test count changes with a compiler upgrade, for tests the project did
not write and cannot fix.** A shipped module importing anything outside the
library is an internal error, not a user error: **the user cannot have caused it
and cannot fix it, and calling it their error would send them looking in their
own tree.**

## `Map`: iteration order is the property that matters

**Ascending by value order, always, everywhere.** Not insertion order, not hash
order, not "unspecified".

This is not a nicety. A hash-ordered map would make iteration a function of a
hasher's seed and of insertion history, and four separate guarantees rest on a
value having one canonical form: a derived JSON encoding of a record containing
a map would differ run to run, so round-tripping would hold and *idempotence*
would not; equality over two maps built in different orders would compare
unequal, so **a passing test would be cached under one order and re-run red
under another**; a simulated replay would take a different branch on a fold over
keys, breaking the guarantee that a seed replays exactly; and the two engines
would disagree on a program that is entirely correct. **Every one of those is a
green result over unexplored space or a red result over correct code.** Ordered
iteration is the whole reason `Map` is a language primitive rather than a
library.

**Ordered iteration was necessary and not sufficient, and three of those four
were reachable for four milestones.** Decimal values compare by *numeric value*
so that two spellings of one number are one key, while rendering prints the
scale as stored — so a map held whichever spelling was inserted last, and
iteration, a derived encoding and a fold's branch were functions of insertion
history **through the key rather than through the order**. Two maps that compared
equal as one value served two different response bodies, and running the two
engines against each other reported nothing, because it was not an engine
disagreement. Fixed in the representation by reducing a key to one
representative per equivalence class on the way in; ADR 0019 is the write-up.

**A key type must be ordered, and "ordered" is exactly the derivability
predicate** — one implementation, no second definition to drift. Floating point
is excluded because NaN makes the order non-total: with a float key, an insert
has no well-defined position, and a total order that disagrees with `==` on its
own keys is a lookup that fails to find what it just inserted. A total ordering
helper makes the *Rust* comparison total; it does not make the *language's*
equality an equivalence relation, and the map's contract is stated in the
language's terms.

For a type parameter, well-formedness is checked at the signature with an
explicit constraint. The error is at the boundary, and the body may then assume
it.

## Derivation

**The orphan rule**: a `derive` may only name a type its own module declares.
This is the cheapest coherence available and costs nothing to have now — without
it, two modules each deriving for one type produce two *names* for one canonical
encoding, which is exactly the divergence there is no resolution layer to
prevent. With it, coherence is a local property checkable from what a module can
see.

**A generated definition takes its target type's visibility**, so a type you can
name from another module is a type you can encode, and the two cannot drift.

**Derivation composes through named types, never by inlining them.** A field of
another module's type generates a *call* to that type's codec, not a copy of its
structure. So a codec's body depends on its field codecs' *hashes*, which is
what makes a change to a nested type re-select exactly the tests that reach the
outer one — and it keeps each type's codec one definition rather than a blob
that grows with the graph.

**Expansion runs immediately after parse, before resolution and before
inference**, and generated definitions are **appended to the module's one item
list**. One list, not two: a second list is a thing every walker can forget, and
forgetting it drops a definition silently. Provenance is carried for reporting
and **erased by normalization** — a hand-written definition byte-identical to a
generated one is the same computation and must share its hash.

**A generated definition that fails to typecheck is Ply's fault.** Derivation is
total and structural, so if generation succeeded, checking must succeed; the
user did not write the body and cannot fix it. This makes the deriver's
correctness checked on every run rather than in a test suite.

Four consequences of a generated definition being an ordinary definition, each a
required property: **renaming the type re-runs no test**, because the generated
*name* moves and its body does not — field names and constructor names are what
the encoding contains; **renaming a variant re-runs its tests**, because the
wire tag changes, which is an observable protocol change. **That pair is the
sharpest available demonstration that the hash tracks meaning.** Reordering
fields counts as a change, because object order is observable. And **any change
to a deriver bumps the front-end version**: gate 1 keys on raw file content, so
a compiler upgrade that changes what the deriver emits would otherwise let a
file be skipped and a stale generated definition be reused.

**Constraints are kept by normalization, unlike specs, and the reason is
soundness rather than taste.** Adding a constraint narrows the call sites a
signature admits. If it were erased, adding one would move no hash, so a caller
already checked against the unconstrained signature would never be rechecked and
would stay accepted against a signature that no longer admits it. **Same reason
declared types and effect rows are in the hash.**

## Numerics

**A bounded decimal, not an arbitrary-precision one.** That sounds like the
weaker property and is the deciding one: an arbitrary-precision decimal has a
size that depends on the operations performed on it, **so a value inside a
deterministic replay could grow without bound, and a value that enters a hash
and a cache key needs a finite, canonical, allocation-free form. Money needs
twenty-eight significant digits, not infinity.**

A distinct normalization tag per numeric type, so the three spellings of one are
three definitions — they have three types and must not share a hash. **The
literal's scale is preserved**, so two spellings of one value are equal in value,
differently hashed, and one map key; **all three are consequences of the same
decision and all three are stated rather than smoothed over.** Positive and
negative zero are different definitions, **and a normalizer that folded them
would make two textually distinct programs one definition while division still
distinguishes them.**

**Decimal division is refused.** The one place this refuses something every
other language allows, and deliberate: an operator would have to round, **a
rounding nobody wrote down is the defect this type exists to prevent, and a
silently-rounded division is the single most common money bug.** A named
function taking a scale and a rounding mode replaces it. Addition and
subtraction are exact or they raise — **never a silent wrap and never a silent
rounding: a total that quietly lost a cent is the failure this type exists to
prevent.**

**JSON numbers hold a decimal and never a float.** This is the whole reason the
type exists: **a parser routing numbers through binary64 decodes tenths to the
nearest double and no amount of care downstream recovers the hundredth of a
cent.** The limit is stated plainly: a number outside the decimal's range is a
decode error and the document is rejected **whole**, even when the codec would
never have read that field. A real cost with a small tail; the alternative — a
string-carrying number, lossless and total — **moves a parse into every consumer
and makes two spellings of one number unequal values.**

**The prover's fragment does not extend by a type arriving.** Floats are
excluded from proof entirely, because float equality is not reflexive and
congruence closure over a non-reflexive relation is unsound. **This is a
structural refusal** — lowering returns unsupported, so the certificate cannot
be constructed, **which is what makes "a tier is computed from the evidence"
true here rather than a convention someone has to remember.** Decimals may
appear only as uninterpreted terms. Generators draw the specials — NaN, both
infinities, both zeros, both extremes — **because a generator that never
produced NaN would make the property tier a lie about the type.**

## What the audits found, and the shapes they generalise to

- **An `Option` whose payload can itself encode as null** writes both cases as
  the same document and decodes both back as the absent one — accepted,
  type-checking, running, and losing the value with no error anywhere; worst as
  a map key, where a two-entry map decodes to one. Now refused. Tagging the
  encoding was the alternative and is rejected: it would change the wire format
  of every optional field, and an optional field being null is what a client
  already means by one. **The refusal is asked of one predicate in two places**,
  because the syntactic walk cannot see an alias and the solved-type check can.
- **A serializer bounded where its parser was not.** Encode was total where
  decode was partial, so a derived codec over a recursive type wrote documents
  its own parser refused, and the failure appeared at the consumer. The
  required test asked only that the codec terminate on a deep value, which it
  did *at the value level*; **the wire is where it broke, and that is what the
  required test did not look at.**
- **A map key's wire form must follow its type, not its spelling** — the alias
  defect above. The deriver now resolves the key through **this module's own**
  aliases before choosing. Only this module's: gate 1 keys on raw file content,
  so a decision that read another module's aliases would leave a stale codec
  behind when that module changed. **Expansion has to be a function of the
  file**, and a cross-module alias therefore still gets the general form — the
  price of the incremental design rather than an oversight.
- **A derived dictionary must name nothing the deriving module can supply.** Two
  instances of one mistake: a generated body wrote **bare** names, and a
  module's own items shadow the prelude. One is the reserved comparison builtin
  (ADR 0001). The other is that a selective import was taken to bind names
  bare, so a generated body wrote unqualified leaf codec names — and a module
  defining its own leaf supplied it, with no ambiguity error, **reintroducing
  silently the divergence the orphan rule exists to prevent.** Expansion now
  synthesizes an import binder and the generated body always writes through it;
  the binder is a function of the file's own imports and enters no hash.
- **"Mentions a float" is a question about a declaration, not about a written
  type.** The structural refusal was asked of a type's *written* form, so a
  single-constructor wrapper around a float answered no and the prover certified
  a reflexivity law that is false at NaN — which its own sampler finds unaided.
  The check now walks the declaration as a least fixed point, over-approximating
  a parameterised declaration, which costs completeness and never soundness. The
  derivability predicate already answered this correctly, **and the two must not
  disagree.**

---

# III. HTTP and TLS

> **The trusted computing base does not grow to hold a protocol.** HTTP/1.1
> framing is a pure function from bytes to a request, so it is written in Ply,
> where the test cache selects it exactly, a defect in it is a failing test
> rather than a line nobody checks, and every framing rule is a hermetic
> deterministic test. The boundary carries bytes. It carries bytes over TLS too,
> **because cryptography is the one thing here that would be reckless to write
> in Ply** — and that is the only place the TCB grows.

Four corollaries: **a protocol defect must be reachable by a test** — request
smuggling is a parser disagreeing with itself about where a message ends, and a
parser behind a host handler is one the test runner cannot reach; **a bound is
part of the contract, not a tuning knob**, so the limits are a value the program
passes and the parser is written so that exceeding one costs the bound rather
than the buffer; **a route table is data**, not a macro, a registry or a list of
closures; and **an alias is an abbreviation for a row, and abbreviations do not
change meaning** (ADR 0009).

## Framing is where the security bugs live

Every rule is stated as *what the server does*, and where the RFC permits a
choice this takes the refusing branch. The pattern behind all of them:

> **A message two implementations may frame differently is the definition of the
> bug.**

So a line is terminated by CRLF and by nothing else, because accepting a bare LF
is the classic desync. Obsolete line folding, whitespace before a header colon,
and a field value with a control character are all refused. **A duplicate
content-length is refused even when the values agree**, because agreement
between two field lines in *this* message says nothing about how an intermediary
in front of the server picked one. Content length and transfer encoding both
present is refused, because preferring one is correct only if every hop made the
same choice, **and smuggling is exactly the case where one did not.** A transfer
coding other than chunked is refused *after* the length question is decided,
which is the one place the order is observable — one spelling has no decidable
length, the other is framed unambiguously and is merely unimplemented, **and
accepting the second would hand the handler an undecoded body with nothing
saying so.**

**A chunk size that does not fit an integer is refused before it is
accumulated.** Sixteen hex digits is sixty-four bits and the integer is signed,
so a digit bound alone does not keep the accumulator from overflowing — **and an
overflow is a *runtime error*, not a refusal: it unwinds past the accept loop.
The bound as originally written was a whole-server kill in one unauthenticated
packet.** Leading zeros are not counted, because refusing a legal spelling of a
small number would be a second parser disagreeing about a length.

The rules are one claim: **for every input, the parse either refuses or
determines exactly one message boundary.** That is the anti-smuggling property,
and it is stated as a test over an adversarial corpus rather than as an
aspiration.

## A route table is data

The table holds segments and an **endpoint tag, not a closure**, and the reason
is instructive: a list is homogeneous, function types carry their rows, and
there is no subsumption — so every handler in one list would have to have
byte-identically the same row, **which would force every endpoint to declare the
union of the whole service and destroy exactly the per-endpoint legibility this
milestone exists to produce.**

With a tag, the program writes its own dispatch, and **that match is
exhaustiveness-checked** — so an endpoint in the table with no handler is a
compile error rather than a 500 at 3am. No framework gives that, and it costs
one `match`.

Matching has an empty row, which is the first thing the type printer says about
a service: routing is the part of a web framework that is hardest to test and
easiest to get subtly wrong, and here it is a pure function over a value.

**Empty path segments are kept, not normalized away.** A silent normalization is
a second answer to "which path is this", and two answers is how a route and an
authorization check come to disagree. **Percent-decoding happens per segment,
after splitting**, so an encoded slash decodes *inside* one segment and can
never introduce a segment boundary — the path-traversal and route-confusion
rule, and the reason the order is fixed.

## TLS is not a separate effect

It is the same network effect with one new operation for creating a listener.

The alternative is worse: a separate effect with the same operations makes every
function that touches a socket exist twice, because there is no effect
polymorphism over effect *names* — and adding one for this would be a language
feature bought to express a distinction the type system does not need. **The
framing library would fork, and two forks of a framing parser is two parsers
that disagree**, which is what part III exists to prevent.

The question is what an effect row *claims*: which resources a computation
touches and whether two computations contend. **A TLS connection and a plaintext
one are the same resource, contend the same way, and are read and written by the
same code. Encryption is a property of the transport, not of the resource**, and
putting it in the row would make every row in the service carry a fact that
decides nothing the row is used for.

**So the row does not say whether a connection is encrypted. The listener
does**, and the TCB listing prints the TLS listener as its own line with its own
handler path — so "this program can serve TLS" is a fact in the listing rather
than an inference from a row.

A third option — a TLS record layer written in Ply over a small crypto effect,
which would make the state machine testable — is refused outright. **A
hand-written TLS implementation is a security defect with a schedule.**

**The key never enters the program.** The listen operation takes a *credential
name*. Certificate bytes as a literal would put a private key into a definition's
hash and into a content-addressed store designed never to forget; a file path in
the program would put a file read inside a network operation, where the listing
discloses a socket and nothing discloses the file — **ADR 0008's unenforceable
residual, deliberately widened.** So the material is configured beside the run
and validated at bind time, before anything runs.

**The handshake completes lazily, on the first read or write, and never inside
accept.** A handshake inside accept means one client sending garbage takes down
the accept loop, which is a denial of service delivered by design. A failed
handshake closes that connection and nothing else, and every subsequent
operation behaves as end of stream — so the server's ordinary "the peer went
away" path handles it, which is the path it must already have. Silence would be
wrong, so failures are counted and reported; a handshake failure is not a
diagnostic, because it is not the program's fault, not Ply's fault, and not
attributable to any definition. **The protocol advertisement offers exactly
HTTP/1.1**, because a client that offers HTTP/2 must be refused at the handshake
rather than served 1.1 bytes over a connection it will parse as 2.

**The TCB digest covers the credential *names*, the provider and the library
version; it does not cover the certificate fingerprint.** A CI check that broke
on every certificate renewal is a CI check people learn to ignore, and a renewal
is an operational fact rather than a structural change. Adding or removing a
credential does move the digest.

## A deadline is an argument, not a cancellation

Cancellation was deferred at the boundary with the note that timeouts would need
it. **They do not, and the cheaper answer is also the better one.** A cancel
path on a pending token needs a token registry, a race between the cancel and
the completion, a rule for what a cancelled operation returns, and a decision
about bytes already read off the socket. A deadline on the operation needs one
socket option, inside a blocking job that already owns the socket. **The second
is one line and has no race.**

One rule, stated once: **absent is a deadline; an empty present is an ending.**
A read answering nothing means the deadline expired; a read answering empty
bytes means the peer stopped sending. A zero or negative timeout is a runtime
error — **a caller that wants no deadline passes a large one, and being made to
write the number down is the point.**

**The concurrency bound is one real thread per waiting operation**, which is the
capacity of the server and is a number a reviewer can read.

---

# IV. The database

> **A row says which tables.** The reason to put a database behind an effect is
> that an endpoint's declared signature names the tables it touches, and a
> driver that answers one generic write atom for every statement has thrown that
> away and kept only the ceremony.

Five corollaries: **a statement's footprint is a function of the statement, not
of the call site** — the call site writes one label and the statement may touch
more, and the gap is closed by making the driver *report* what it touched;
**a rollback is the absence of a resumption**, and zero resumptions is inside
ADR 0008's one-resumption cap rather than an exception to it; **a shared
connection is not forkable, and pretending otherwise is the silent failure**;
**a value never becomes syntax**; and **a twin that silently diverges is worse
than no twin.**

## Transactions are handlers, and rollback is a discarded continuation

A transaction is an ordinary library function: a `handle` over one new operation
whose clause **does not resume**. The value of the clause is the value of the
whole `handle`, so everything the body had left to do — the rest of the
function, its callers up to the transaction, the statements it was about to
issue — is the continuation, and the continuation is dropped on the floor.
Nothing unwinds, nothing is caught, no frame runs an epilogue. **This is the
first place in the language where discarding a continuation is the *point*
rather than a capability.**

Note what does not appear: a clause per table. The transaction intercepts the
rollback operation and nothing else; the data operations pass straight through
to whatever handler is below, and the driver routes them onto the open scope's
connection because the scope is host-side state. **A Ply clause names a concrete
(operation, resource) pair, so a transaction that intercepted the data
operations would need one clause per table per operation and could not be a
library function at all.** It does not need to: a transaction is a *scope*, and
the only thing that must be scoped is the abort.

Zero resumptions trivially satisfies at-most-one, so rollback needs no
exemption, no new predicate and no change to the check — **the strongest
evidence available that ADR 0008's linearity restriction was the right one
rather than a convenient one.** The interaction that *is* new: beginning a
transaction is irreversible, so a continuation captured before one cannot be
resumed a second time. A program that wanted to replay a transaction body
through a multi-shot handler is refused, correctly.

**Four exits, and three of them are what a real system gets wrong.** Commit;
rollback; **the body raises**, in which case the raise propagates unchanged past
the handler and nothing was committed while the scope is still open; and **the
entry point ends with a scope open**, which the driver rolls back at teardown.
That last needs a mechanism rather than an intention, so the runtime gains a
hook called on **every** exit path — a value, a diagnostic, or a spent budget.

The pool is the second lock: a connection returned with a scope still open is
rolled back on release, and one whose rollback fails is **closed and discarded
rather than returned**. **A connection recycled with an open transaction is the
failure that makes the *next* request read uncommitted rows of a request that
already failed, and it is invisible from either request.**

**Nesting is a savepoint, not a refusal.** A nested transaction is what a helper
function looks like when it is called both standalone and from inside a larger
operation, which is the ordinary case; refusing it would mean every such helper
existed twice, **and two copies of a write path is the drift this milestone
exists to measure.** Isolation level and access mode on a nested scope are
ignored, because a savepoint has neither — so that silent difference is made not
silent: a nested begin whose level differs from the open scope's fails naming
both. A narrowing to read-only is accepted and **is documentation, not
enforcement**, which is the only honest thing to say about it.

**Dirty reads are not offered.** The server implements that level as the next
one up, so a name in Ply's source promising them would be a name that lies, and
**this project's whole posture is that a label is a truth claim.**

**A serialization failure is not retried.** Retrying means re-running the body,
and only the program knows whether the body sent an email or charged a card
between two statements. A predicate is provided so the decision is one `if` at a
site that can see what it is repeating. A retry is a fresh call, not a second
resumption, so it is outside the linearity rule entirely.

**A read-only transaction is enforced by the server**, which is a mechanical
backstop on a row that claims to be read-only, supplied by the one component in
the stack that cannot be fooled by an annotation, and it costs nothing.

**A scope belongs to the task that opened it**, and an operation from another
task while it is open is an error. The two alternatives are both wrong: sharing
the connection is a protocol violation, and quietly acquiring a second one would
put the statement *outside* the transaction its author believed it was in.

## Footprint granularity, and the only interesting problem here

The label a call site writes is the statement's **principal table**. Transaction
control takes no resource, so its atom is a singleton — **a real scheduling
cost, stated rather than discovered**: any two tests that open transactions
conflict even when their tables are disjoint. It is also true; they contend for
the same pool, and a pool is exactly the host state that cannot be forked.
Read-only endpoints do not open transactions and keep their concurrency, which
is where most of a service's parallelism is.

**A statement may touch more tables than its label names**, and nothing in the
type system can see it, because the SQL is a string. Three answers, two refused:

- *One statement, one table.* Preserves the property perfectly and makes a join
  inexpressible, which is not a database.
- *Widen the label to a group.* The label stops being a table, the listing stops
  printing tables, and "which routes write this table" stops having an answer.
  **This is a generic write atom with more syllables.**
- **Report what was touched, and check the report.** Taken.

So a host reply carries the atoms the operation touched *beyond* the one the
registry resolved — empty for every handler whose footprint is a property of its
registration, which is every handler the earlier milestones shipped — and the
machine checks each against the entry point's declared footprint. **This is a
detector and not a preventer, and that has to be said out loud**: scheduling
happened before the run, so by the time the check fires the statement has
executed against a table the scheduler thought nobody was touching. What it buys
is that a wrong row fails loudly on its first execution instead of quietly
forever — which, given that every dangerous defect this project has found was a
green result over unexplored space, is the difference that matters.

**The preventer is the second lock and runs earlier**: the request carries the
entry point's declared footprint, so a handler that can compute its own
footprint can refuse instead of acting. The driver computes a statement's table
set at *prepare* time, once per statement text, and refuses there before a row
moves. The machine's check on what was touched then covers the case the driver's
own scan got wrong.

**This does not close ADR 0008's residual.** A handler that lies about what it
touched is exactly as invisible as one that lies about its registration. What it
closes is the case where the *honest* handler could not tell the truth — and
this driver is the first handler in the system whose footprint is not a
constant.

**The scanner refuses everything it does not recognise**, naming the offset and
the token, and never returns an empty table set. Conservative in the safe
direction by construction: a defect in the scanner is a refusal to run rather
than a footprint that under-reports. The residual is that a defect which
*mis*-recognises a construct can still under-report, which is why the scanner is
disclosed in the TCB listing and has a differential test with the database's own
query planner as the oracle — **a superset rather than an equality, because the
planner prunes, and over-reporting costs concurrency instead of correctness.**

**Writing the scanner in Ply was considered and refused for one reason that does
not generalise**: the driver needs the answer, the driver is Rust, and a Ply
implementation would mean two scanners. Two parsers that disagree is the hazard
part III exists to prevent, and here the disagreement would be between the
footprint a test observes and the footprint the scheduler was given.

**What a scanner cannot see is asked of the database at bind time.** A trigger,
a rewrite rule or a cascading referential action makes one statement touch a
table its text never names. So the catalogue is queried before anything runs,
and any object that could reach a table outside the atom it fires under is a
start-up refusal. **Strict, and being strict is the decision**: a footprint that
under-reports corrupts scheduling and isolation silently, while a schema with a
trigger gets a refusal it can read and fix. **There is no flag to suppress it,
because a flag that turns a soundness check off is a flag whose default becomes
the one nobody uses.**

*This check is specified and was never built.* The code that would raise it does
not exist, so the guarantee is not in force, and a trigger on a production
database today produces a footprint that under-reports. **It is now an
assertion rather than a note**: the diagnostic registry test fails on any
registered code that no production source constructs, and this code is one of
two entries in its allowlist, each citing this record. The allowlist entry has
to go with the day someone builds the check.

## The pool and test isolation, stated bluntly

A pooled connection is shared state that crosses test boundaries, and host
effects are outside whatever isolation mechanism the language has. So:

- Every test that reaches the binding is counted as host, excluded from the
  isolated count, never cached, never bisected.
- **A test does not get its own database.** Its isolation is exactly two things:
  footprint conflict grouping over tables, and whatever it does inside a
  transaction it rolls back. No fork, no template database, no schema-per-test,
  no truncation between tests. (Session state is reset between borrowers, which
  closes several channels conflict grouping cannot see at all — but that is
  per *checkout*, not per test, and says nothing about table contents.)
- **Two host-backed tests whose tables are disjoint run concurrently against one
  database**, which is correct only if the footprints are honest. **This is the
  sharpest place in the system where the footprint work is load-bearing, and it
  is why a schema with a trigger is refused rather than warned about.**

A sandbox helper — a transaction that always rolls back — is shipped, with its
limits stated where it is defined: it does not isolate DDL, does not roll back a
sequence's advance (so two sandboxed tests see different ids, and a test
asserting the first id is wrong on its second run), does not isolate what a
different connection does, and cannot nest past the depth bound.

## The twin, and the agreement law

**The twin is Ply, and it is pure**: a set of functions over an opaque value.
Rows and answers are values of the same types the driver produces, so **the
"same declared signature" is structural rather than promised.** A program
installs it with an ordinary `handle` over a region-scoped cell.

The consequence that matters: after the region discharges the cell's atoms, a
twin-backed test's row is **empty**. It is deterministic. It is cached. It is
hermetic. **And it can run inside `simulate`, which is what makes a
check-then-act race between two requests on one row findable and replayable from
a seed.**

**It executes the same statement text the driver does, through its own scanner**
— which is the point, **because the scanner is where the divergences live and a
twin that took a structured operation instead would never test it.**

**Anything it does not model fails loudly with the construct named**, using the
server's own not-supported code. **It never guesses and never answers as though
it executed.** The list is written out so nobody has to discover it, and two
entries carry the weight. **Isolation**: the twin is serial, so it cannot exhibit
a phantom read, a lost update, a serialization failure or a deadlock — **the
largest thing it does not model and the one a reader is most likely to assume it
does.** And **collation**: it orders text by byte order, and any other database
collation disagrees, so the fixture database is created to match and the live
one's collation is printed, **making the divergence visible rather than latent.**

The **failed-transaction state** — after a statement fails inside a scope, every
subsequent statement fails until the scope ends — *is* modelled, **because it is
the behaviour test doubles omit most often and the one that makes a suite pass
and production fail.** And **sequences under rollback are deliberately *not*
rolled back either, because matching the surprising behaviour is the whole
job.**

### `law/host`: a law may reach the world, and says so in its declaration

A law's body must be pure, so the agreement law does not compile. **Relaxing
that silently would mean a law could touch the world without saying so, which is
the opposite of every other decision here**, so the relaxation is *declared* in
the law's own form, exactly as a nondet test declares its own.

- The **body** may carry any row; the **guard** may not, because a guard decides
  the domain and a guard that could act would be choosing which cases to be
  judged on.
- **It can never be proved**, structurally: lowering returns unsupported for a
  non-empty row, so the certificate cannot be constructed. `property` is the
  ceiling and the tier says so.
- It is **never cached**, in either direction.
- Under a hermetic run — the default — it is reported as an **unattempted gap**
  with the reason. Not skipped silently, and not green. **A law about a database
  that never ran a database, reported as passing, would be precisely the green
  result over unexplored space this project audits for.**
- The declaration is part of the law's own hash, so changing a law into a host
  law is a different claim and re-discharges.

**Both sides execute the same rendered SQL.** A structured operation handed to
the twin and SQL handed to the driver would have tested everything except the
place the bugs are. Operations are an ordinary ADT, so the existing generator
quantifies over them and the existing shrinker shrinks them — no new generator
and no new shrinking rule, which is what makes the law cheap enough to be a
required test rather than a project.

**Failures compare on code and constraint only.** The server's prose is never
compared. **This is the single most important line here: a law that compared
messages would fail on a server upgrade and would teach everyone to ignore it.**
The comparison stops at the first differing operation, because a divergence at
step three makes steps four onward meaningless.

**The law must be able to fail.** A required test *injects a known divergence* —
a fixture database created with a different collation, and separately a twin
with its failed-transaction state removed — and asserts the law finds each.
**Without those, "the agreement law passes" is a statement about the generator's
reach and nothing else.**

**A migration tool is out of scope.** No versions, no up and down, no ordering
across deploys, no diffing a live database. That is a product, orthogonal to
everything here, and a half-built one would be worse than none.

## Two amendments to the boundary

**Running both engines degrades to one on a host-backed test, silently.** The
obvious worry is wrong — the host operation is not executed twice, because the
tree-walker refuses at the first one and the machine's answer is what is
returned. **What is wrong is the reporting**: such a test gets *no* differential
audit at all, and the run says both engines regardless. So the one command whose
purpose is "two engines agree" quietly means "one engine ran" for exactly the
tests a database makes interesting, and the count of what was audited is
overstated. The fix is the same one the host count needs — declare the guarantee
inapplicable where it cannot hold, and keep the number honest.

**What a green workspace test run actually proves here.** Every test that needs
a live database is behind a conditional skip that returns *ok* rather than
failing when the dependency is absent. Both print a reason, which is honest —
and *the reason is printed to stderr of a passing test*, so a CI summary, a
quiet run, or a reader looking at an exit code sees nothing. One gate starts its
own cluster and does run on a developer machine with the database installed; the
other reads an environment variable **nothing in the repository sets**, so it
skips on a stock checkout even when the first is running and passing. CI now
sets it and fails both if a skip line was printed and if the expected count was
not reported. **The measurement is the point rather than the decoration: with
the variable unset the same command exits 0 and every one of those tests
passes.**

---

# V. Operations

> **A row says what a function records, and a type says where a credential
> cannot go.** This is the milestone where a service acquires the three things
> every service acquires — a log, a configuration, and a way to stop — and each
> of them is, in every other language, **ambient**: a global logger, a global
> environment, a global signal handler. Ambient is exactly what this language
> has spent eight milestones removing, because ambient means the runtime does
> not know what a computation can do.

## A trace call is a perform, always

**There is no configuration under which a trace operation is not performed.** A
row is a claim about what a computation can do and cannot be conditional on a
flag, so "tracing off" does not remove the perform — it binds a discarding
handler, a real listed member of the TCB whose clause returns unit. An empty
registry would be an error at the first event, which is correct and is not what
"off" should mean.

So the cost of a disabled span is exactly: the fields map the call site built,
one perform, and nothing else. **And "nothing else" is designed rather than
observed.** A call site never formats — there is no operation that takes a
pre-rendered message, and rendering is the sink's. **A call site never reads a
clock**; the timestamp is stamped by the sink, which also keeps the clock atom
out of every tracing function's row, **and that is why a trace call does not
drag a clock read into fifty endpoints' signatures.** A call site never
allocates a span record. **And level filtering happens in the sink, not at the
call site** — so a level filter does not make a debug event free; it makes it
cost one perform and one map. **Saying otherwise would be the misleading claim,
because the only way to make it free is a row that lies.**

If that number turns out to be the reason a service cannot afford tracing, the
fix is a cheaper perform rather than a row that lies.

## Typed secrets

> **No value of type `Secret<a>` can reach a trace field, a JSON document, a SQL
> parameter, an HTTP response, a diagnostic message, an assertion diff, a panic
> payload, a definition hash, the store or the result cache.**
>
> Every one of those is reached through a function, a derivation or an evaluator
> path whose parameter type `Secret<a>` does not inhabit, and the two paths that
> take *any* value — rendering and equality — are closed at the evaluator.
> **Nothing here is a review rule.**

Two halves of the mechanism are load-bearing. **A distinct value variant, not a
constructor.** A constructor is matchable, and one pattern match would be a
one-line escape; `Secret` declares no constructors, so a pattern naming it does
not resolve, and **there is no pattern that binds the payload.** And **a builtin
type constructor, not a record or an ADT in the standard library**: a
record-shaped secret is one field access away from useless, and a project could
declare its own.

**A secret reaching the host is declared and enumerable.** A host operation
declares whether it may be handed one, and the machine refuses before calling a
handler that did not. It ships with a user count of zero — the column reads no
on every row and the check is a tripwire — **which is stated rather than
omitted: the mechanism lands empty because the moment an outbound client with a
bearer token arrives it will have a user, and adding the check then would mean
adding it after the first operation that needed it already shipped.**

### What this does *not* prevent

Written out, **because a secret that can be exfiltrated by a route the design
did not mention is not protected, and an unstated hole is worse than a stated
one.**

1. **A credential written as a source literal.** The literal normalizes into the
   definition's bytes and lands in a store designed never to forget. The wrapper
   changes nothing, because the *literal* is what entered the hash. What defends
   the source tree is that a real credential comes from configuration, and a
   configured value is in no hash because it is in no definition. **This is the
   largest hole and nothing here closes it.**
2. **The plaintext it was built from.** Ply is a value language, so wrapping does
   not consume the string. Containment starts where the secret starts.
3. **Verification leaks one bit per call**, and a program that loops it over
   candidates recovers the value. Constant-time in the compared bytes, not
   rate-limited; rate limiting is the program's.
4. **Timing beyond the comparison.** Nothing else in the interpreter is
   constant-time, and what a program *does* with the answer — a branch, a trace
   event on one arm only — is an oracle this neither creates nor closes.
5. **A host handler that receives one.** What it then does is invisible, exactly
   as ADR 0008 says of every handler claim.
6. **Memory.** No zeroization. The payload is not wiped on drop, the allocator
   may reuse the pages, and a core dump or a debugger has the plaintext. The
   evaluator copies values freely and a zeroizing type in it would be a promise
   the runtime cannot keep.
7. **A secret's *presence* is observable** — deliberately, because an operator
   must be able to tell a missing credential from a wrong one. Metadata, never
   the value.
8. **Length**, in the same one-bit-per-call sense as (3).

### Alternatives rejected

**An effect only a redacting handler may discharge.** Handlers are checked by
*signature*, not by identity, so "only the redacting handler" is not something
the type system can say. **A convention wearing an effect's clothes**, and
replacing conventions with mechanisms is the whole point.

**Derivation refusal alone.** Closes one route and leaves concatenation, trace
fields, rendering and pattern matching wide open.

**Redaction at the sink** — a regex over outgoing log lines, or a registry of
known secret values scrubbed on write. It fails on any transformation, it fails
on a value the registry never saw, and **the failure is silent. This is the
thing being replaced.**

**An affine or linear secret.** Use-once would prevent the guessing loop, and
adding substructural types for this would be a type-system milestone attached to
an operations milestone.

**Leaving the ordering hole open and documenting it.** It was reachable from an
ordinary program with no flags and no unsafe code, and it recovered the whole
plaintext. **A hole that recovers the value is not a hole a caveat can honestly
describe; it is a hole that falsifies the claim.**

### The general soundness rule the secret hole delivered

The route needed a secret masquerading as an ordered type, and what supplied one
was not about secrets at all: **a handler clause for an operation with a
polymorphic return was checked against a fresh instantiation, never unified with
the one the perform site used.** So a clause could answer a credential where the
call site had unified the variable with a string, and the type printer showed a
string return for a function that produced a credential. The same shape typed a
clause answering an integer for a string caller, failing only at run time.

> **A handler clause is checked with the operation's own type variables rigid.**

A clause is written once and answers every perform site there will ever be, and
a row carries atoms and no types — so a `handle` cannot see which instantiation
a perform three definitions deeper picked, and a clause that chose would be
handing a caller a value of a type it never asked for. **Rigid variables are
what say "for every `a`", which is the obligation a clause actually carries.**

What that costs, stated: **an operation whose return type is a variable its
parameters do not determine can no longer be handled at all**, because no clause
can produce a value of an unknown type out of nothing. That is correct — such an
operation is unsound rather than merely awkward — and it is not hypothetical: a
shipped example declared one and was rewritten to declare what each of its
tables holds.

## Configuration is read exactly once

The environment, the config files and the command-line overrides are read **at
bind time** into one immutable map, and the process environment is never
consulted again. One line of implementation, and the whole of this section's
soundness: it is what makes a config read honestly a **read**, since the source
cannot change under a run so two readers cannot disagree and the conflict graph
is right; it is what stops one test's environment mutation being seen by
another, **which is the pooled-connection defect in a new costume**; and it is
what makes a run reproducible.

**Configuration may supply a value and may never cause a binding.** Without the
host flag no source is opened at all, whatever the environment holds. A reviewer
reads the flag in the command, or the run reached nothing.

**Start-up, not first request.** A service that starts, serves two hundred
requests and answers wrongly because a key was unset is the failure this
prevents. A named schema function is materialised at bind time and every key
resolved against it, so a missing required key or a wrong shape is a refusal
that names the key and the four places it looked — **and never the value when
the shape is a secret**. An *explicit* key the schema does not declare is a
warning; only the explicit sources, **because an environment is full of names
that have nothing to do with this program, while a typed override is something a
person wrote on purpose and a typo in one is the classic silent deploy
failure.**

A secret-shaped key resolves to a secret and the ordinary getter **will not
return it**; without that, the type-level guarantee would be one call site away
from a string. **And containment for configured values is only as strong as the
schema**: a run with no schema can read a password as an ordinary string.

## Graceful shutdown

A signal sets an atomic flag and nothing else, which is the only thing a signal
handler is allowed to do. The accept operation then answers "finished", so an
existing sequential accept loop stops accepting, finishes the connection it is
on and returns — **and not one line of it changes**, which is the direct
consequence of the earlier decision that accept answers rather than raises when
the listener is finished.

**Teardown order is pinned, because three of the four steps are
ordering-sensitive and a wrong order is a data-loss bug rather than a mess.**
Roll back every open transaction — **never commit**, because a commit at a
deadline commits a half-finished body, and the only thing that knows whether a
body finished is the body. Close every open span as abandoned, *after* the
rollback so a span can record it. Flush the sink *before* the pool closes, so a
trace naming a rolled-back transaction is written before the connection that
rolled it back is gone. Then close the pool, closing connections rather than
returning them.

**The teardown is bounded by the drain budget, and that is what makes it a bound
at all.** Every waiting step waits for at most the budget it is handed, and the
waiting steps *share* it rather than each getting it. It was not, and the
failure was the quiet kind: the steps were bounded by the *database's* own
deadlines, so a request blocked on a row lock raised the drain warning on time
and then held the process open until the statement timeout fired. **For a
rolling restart that is the difference between a bounded and an unbounded stop.**
A rollback that cannot finish inside the budget is answered by closing the
connection, which is what makes the server abandon the statement holding the
locks the rest of the restart is waiting on.

**There is no cancellation, and the honest answer is not a good one.** A task
still running at the deadline is not cancelled, not unwound, and not handed a
503; the process tears down and exits, and the client sees a connection closed
with no response. Three things follow: the drain budget should exceed the
program's own body and write timeouts, which the run cannot check because the
limits are a Ply value it never sees, so it is documented and printed in the
start-up banner where the two numbers can be compared by eye; **a drain that
expires exits with its own code**, so a deployment that sees it knows it lost
requests and one that sees success knows it did not — **that distinction is the
whole product of this section**; and **a rolled-back transaction is not a lost
request in the dangerous sense** — the client got no answer and the database has
no partial write, which is the outcome a retry can fix. **The outcome a retry
cannot fix is a committed half-transaction, and the ordering above is what makes
that unreachable.**

## Deployment over the content-addressed store

Ply knows exactly which definitions changed, so a deploy *could* ship only
those. **It ships a whole-program artifact and no incremental transfer**, and
here is the reasoning rather than the conclusion.

A deploy must ship the binary, because the program is interpreted and every
guarantee is the runtime's. The version constants have moved in most milestones,
so **the binary is the part that actually changes, and it is orders of magnitude
larger than the definitions. Shipping only the changed definitions optimises the
small side of a ratio nobody measured** — so the required test prints both
numbers, because a decision of this shape should be re-openable against a
measurement rather than against a paragraph.

What incremental transfer would additionally need, none of which exists: an
agent on the target, an authenticated channel, a negotiation, a rollback story,
an atomic switch, and a garbage-collection policy. **That is a product, and a
half-built one would be worse than none.**

What content addressing *is* worth here, and costs almost nothing because the
store already does it, is **identity and verification.**

**An artifact is the transitive closure of its roots**, which are the entry
point *and the start-up definitions the build names*. That second half was
learned the hard way: a schema function is a nullary definition nothing in
`main` calls, so it was not in the closure, so the deployed artifact could not
be run with a schema flag — at which point the missing-key error could never
fire, the schema check could never fire, and **the getter on the deployed
artifact could hand back the API key as an ordinary string, which is the one
thing the secret work exists to prevent.** The conflict was resolved in favour
of the guarantee. A name that resolves to nothing is refused at *build* time,
where the person who can fix it is holding the source tree.

**Tests, laws and specs are not in it.** They are in no root's closure — a test
is a definition nothing calls — so a deployed artifact carries no fixture data,
no seed corpus and no host law that would try to reach a database. **That falls
out of the closure rather than being filtered, which is the better kind of
property.**

**A target verifies everything, and each check answers a different question**:
every body against its own key, **which makes a corrupted transfer a
per-definition refusal rather than a plausible wrong program**; every reference
resolving inside the artifact, which catches a closure computed wrong; the
header's versions against the running binary, **under its own code and not the
corruption one, because the responses are opposite — rebuild the artifact,
versus re-transfer it**; and a digest over everything, which is the line a
deployment pins.

**Two builds of one source tree produce byte-identical artifacts**, on any
machine, from a warm or a cold cache. Bodies are normalized, sections are
sorted, nothing carries a timestamp. **Reproducible builds falling out of
content addressing rather than being engineered.**

The build's *diff* is the part worth keeping: a set difference over two hash
sets plus the reverse closure the graph already computes, answering "what is
actually going out" in the language's own terms. **The incremental story
delivered as *information* rather than as transport.**

What this costs, stated: **a deployed artifact has no spans**, so a diagnostic
raised in production carries no source location unless it was built with sources
— and that puts the program's source text in whatever receives the artifact, a
disclosure decision, which is why it is a flag, is off, and is covered by the
digest. **No target-side inventory**, because nothing on the target answers.
**No signing**: the digest establishes identity, not authenticity, and a
signature needs a key, a trust root and a revocation story.

---

# VI. Where the time goes, and the verdict on a code generator

> A decision made after the numbers arrive is a decision fitted to them. So the
> **criteria are pinned first**, the **measurement is by substitution** so that
> no in-machine timer has to be trusted, and the **honest ceiling is stated
> plainly** — because a flattering number that a reader later discovers is
> flattering costs more than the truth would have.

This milestone's deliverable is a number and a verdict, and the only way a
verdict of that shape is worth anything is if the criteria are fixed before the
numbers exist. The criteria sections were written before any measurement was
taken and are placed first, **in that order deliberately: a reader who wants to
check that the bar was not moved reads the bar first.**

## Measurement is by substitution, not by instrumentation

> Every rung runs **the same program**. Two measurements differ in exactly
> **one** thing underneath it, in the **same arena**, in the **same run**. The
> layer is their difference. **Nothing is timed from inside the machine, because
> a timer inside the machine is a claim about where a boundary is, and the
> boundary is what is being measured.**

Three consequences, all load-bearing. **A rung needs two numbers, not one**, and
refuses to be built from one of them — a ladder of cumulative totals silently
assumes each rung's baseline is the rung below, which is true in one arena and
false across a seam. **A negative layer is a result**: it means the substitution
did not isolate what it claimed to, and the report says so rather than deciding
from it. **A clamp to zero would turn a broken measurement into a plausible
one.** And a rung names the route it was taken on, because two rungs on two
routes have a difference that is a route change as well as a layer.

**The residue is printed.** The total is *measured*, not summed; the rungs are
checked against it and the difference is printed on its own line. **Every
profile-shaped table in the industry either omits this or folds it into the
nearest plausible layer; both are ways of claiming an attribution the
measurement did not earn.** A *positive* residue is credited to nobody, which
makes the interpreter's share a lower bound — deliberately conservative in the
direction a code generator's case rests on. A *negative* one is the layers
summing to more than the request, so leaving it uncredited inflates the
numerator instead, and it is charged back.

The measurement discipline that survives the milestone: release profile, because
a debug measurement of an interpreter is a measurement of assertions; best of N
with N reported, because the quantity of interest is the cost of the work and
everything a run adds is additive noise; the request head length printed on
every table, because a load number quoted without it says nothing; and **the
service under measurement is the one that shipped, rewritten only in the ways
the harness already rewrites it, not a copy that can drift.**

## The criteria, and why they are in code

Four criteria, all of which must hold: the interpreter's **share** of a request
clears a bar; the spike's **speedup** and its projection clear theirs; **nothing
cheaper** — every alternative lever is *priced*, and the projected gain is at
least twice the best alternative's; and **correctness**.

The thresholds are justified by what a permanent second execution path costs,
and **that right-hand side is written down because "is it worth it" is a
comparison**: a dependency of a different order from anything in the tree; **a
second execution path, permanently**, turning one differential pair into three
— *or the guarantee weakens silently, which is the failure mode every audited
milestone has produced and none has produced as a crash*; **a cache key**, since
a cached pass is a claim about what the evaluator did — **and an opt-in code
generator is an opt-in cache**, the shape already refused for the host flag on
the grounds that the checker must not disagree with itself; **determinism**,
because a backend that reassociates arithmetic or reorders argument evaluation
breaks seeded replay and tier honesty *silently*; and **maintenance forever**,
since every new builtin and every value variant lands twice.

**The thresholds live in code, not in prose.** The report the measuring runs
produce carries **no** criteria field and **no** verdict field, **so there is no
path from a measurement to the bar it is about to clear.** The same structural
argument the tier contract makes: **a label that can be asserted will eventually
be asserted wrongly, and a verdict that can be written into a file will
eventually be written into a file.**

**An unpriced alternative defers on its own, independently of every number** —
the earlier precedent stated as a rule: a code generator was predicted to be the
second lever, and the byte-builtins milestone beat the prediction by attacking
the algorithm instead.

Two outcomes are *undecided* rather than *defer*, **because they call for the
opposite response — take the measurement rather than accept the answer**: a
missing rung, and a spike that is not evidence.

## The spike

**The selection rule is applied to the measured stage table rather than chosen
in advance**: among the pure functions on the request path, the one with the
highest per-request cost **whose entire body is inside the compilable fragment**
— because a spike that compiles half a function and calls back into the
interpreter for the rest **is measuring a trampoline.**

What it may not do: **be a synthetic benchmark**; **be kept because it works**,
since an advance schedules a milestone with its own record and not a promotion
of a spike; and **report a ratio whose two sides did different work.**

*The prohibition on a compiled backend in general is withdrawn, and cheap
deletion is over — the deletion clause's stated reason was falsified by a later
milestone leaving a seam in the evaluator that survives it. What replaces cheap
deletion as the protection is that **a backend must be policeable before it is
fast** (ADR 0026).*

## The verdict, and what the audits found

**Keep deferring.** The first take deferred on the nothing-cheaper criterion
alone. The re-take defers on three, and **the share fell not because the
interpreter got faster in the ways a code generator would have made it faster,
but because a cheaper lever landed and took interpreter time out of the
request** — which the record itself had predicted as the most likely way the
number would move.

**One of the levers landed rather than being priced**, and priced the way the
list requires — a source substitution, both variants served alternately by the
same binary, byte-identical responses asserted before anything was timed — **it
is worth more on a trivial route than the first take projected a whole code
generator would be worth end to end.** One memoized definition: no new execution
path, no cache key that splits on a flag, no third pair to police. **It also
took the case apart from the inside: the share it removed was interpreter share.**

**And a projection may not clear a gate.** The projection applies one function's
speedup to the whole interpreter share, which assumes a backend as fast on
everything the interpreter does. Three measured things say that is generous:
that function's own end-to-end value is nearly nothing — a small single-digit
percentage, so compiling it *perfectly* buys almost what compiling it at the
spike's ratio buys; the fragment reaches well under half the functions, and what
it refuses is what endpoints and codecs are built from; and **coverage is not
linear, it is a cliff** — the whole speedup collapses the moment two callees
stay in the interpreter. **The honest reading is *an upper bound with three
measured reasons to doubt it*, not a forecast.**

### What two audits found

Three blockers were three faces of one thing: **the numbers had stopped
describing the tree they shipped in, and nothing in the repository could have
noticed.**

**The tree moved under the file.** The constant memo landed after the first
take, so the route five of the nine rungs are taken on stopped doing most of
what the ladder said it did. **The proof that this is a tree difference and not
a rig difference needs no clock**: the two takes' allocation counts for one
request differ by nearly an order of magnitude, counted by a global allocator,
on any machine. Two guards now stand where nothing did — one re-takes the cheap
half of the ladder and compares against the shipped file, one does the same in
allocations, **which do not move with a machine and so cannot be argued away as
load.** And **the command now writes the file**: it used to emit a differently
shaped document that was assembled around by hand, so both staleness guards told
a contributor to *re-take the ladder* and **following that instruction would
have deleted the evidence the verdict was decided against.**

**The nothing-cheaper criterion was decided against a field of the file it was
judging.** It was implemented as "no entry in the alternatives array I was
handed is unpriced" — and that array is a field of the same file the ladder
comes from, an **empty** one satisfied it vacuously, and the audit said nothing.
**Two lines of script over the shipped ladder turned *keep deferring* into
*advance*, cleanly, with no audit finding.** The fix moves the check out of
reach of the run being judged: the roster of levers is in code, so **a file that
says nothing about a lever prices nothing**; and a price needs **evidence** — a
sentence saying what the ratio is between — because a boolean is something
somebody can type, and what is wanted is a *measured* speedup, **including 1.00×,
which is a result, and which is why the fix is a citation rather than a floor on
the ratio.**

**Four places the tables claimed more than the measurement had.** A negative
residue was printed and ignored, and the first take decided on one while calling
its share a lower bound two sentences after saying why it was not. One number
per rung and no spread, on differences of about one percent, with re-runs of the
same harness flipping the sign — and **three clean re-runs put the share on both
sides of its bar**, so a share whose band falls on both sides of a bar is now
*undecided*, because that ladder answers whichever run was taken. The floor did
different work from the total — the headline multiple divided a database-backed
TLS route by a floor replaying a trivial plaintext response, **so one rung alone
was a third of the numerator and none of the denominator.** And the accept loop
the run used was the one the method excluded, which the disclosure section did
not list among its departures.

That last one is worth its shape: **the obvious remedy was to default to the
pinned loop, and taking that measurement is what found the reason the run had
been right for without saying it.** A spawning accept loop opens a production
region for the life of the server, and the constant memo refuses to fire inside
any open region — so a spawning service memoizes nothing while the in-process
rungs memoize, and a ladder read off the pinned loop would divide a memo-inert
denominator into a memo-active numerator. **So the fix is disclosure rather than
a different default**: both loops are swept on every run, one is read off, the
other is labelled.

**What did not change: the criteria.** The same thresholds, no bar moved in
either direction. The numbers under them were re-taken, and where the machinery
changed it changed to make a measurement **harder to overstate rather than
easier to clear.**

## The honest account, and where this is not competitive

The report owes six things, and the audit names any that are missing *above* the
tables: the accumulated stack in one table with the floor, the residue and the
measured total; what a reader gets today, with the Rust floor beside it so the
multiple is visible rather than inferable; **where this is genuinely not
competitive, with none of the candidates allowed to go unmentioned because it is
unflattering**; the verdict with the criteria restated and the numbers plugged
in; **provenance — machine, profile, date, repeats, head length, versions,
because a table without it is a rumour**; and **what was not measured, because
an empty list is itself an audit finding.**

The candidates, all of which survived: **one machine is one core**, because a
value holds non-atomic reference counts and a task cannot move between OS
threads; a request costs tens of times the syscalls under it, **and that
multiple must be read like for like**; **a service whose accept loop spawns
memoizes nothing**; the in-memory twin is slower than the database it stands in
for, because it parses its SQL in Ply on every call; running both engines costs
more than two runs; the request path allocates far more times than it writes
bytes; there is no cancellation, no backpressure and no load shedding — **no
number; the absence is the statement**; and the trace sink is quadratic.

**That last row is worth its own paragraph, because the cost was right and the
cause was not.** It was blamed on the list append, **and the only remedy that
implies is *avoid append*, which no one can act on**, since it is the language's
sole list primitive. Append grows a list **in place** when the caller is its last
owner, and what decides that is **position**: a pending frame is handed a live
clone of the scope whenever any sub-expression of the enclosing node remains, and
never asks what those sub-expressions read. The sink wrote the growing field
first of three. **The real fix is one line of field order** — and on the machine
engine only, because the tree-walker runs no reference counting at all, **which
makes this a limit of one engine rather than of the library.**

And the tone, since it is a decision rather than a description. The right
sentence to be able to write is *"a Ply service serves N requests per second on
one core, which is M times what the same thing costs in Rust, and here is where
the M goes"*. **The wrong one is any sentence whose numerator was chosen after
the numbers arrived.**

## What the ladder cannot decide

The decision procedure structurally requires a served HTTP request with a
socket, a TLS record layer and a database round trip on the path: it refuses a
ladder missing any of its nine rungs. **That refusal is correct for what the
ladder is** — a share taken over a partial stack is a share of the wrong
denominator. **The defect is in reading the output of a nine-rung HTTP
instrument as an answer about a language**, and ADR 0026 is where that is
argued and where the ladder's authority over the backend question is withdrawn.

Its finding is unamended and is a sufficient reason on its own: **do not put a
code generator in front of this HTTP stack.**

---

## What the track left open

Cancellation, unresolved from the first milestone through the last. Backpressure
and load shedding, promised in one milestone's prose and broken explicitly in
another's exclusions. The dispatch question ADR 0010 deferred, whose deciding
measurement the track was supposed to produce and did not. The schema-object
check specified for the database and never built. And six of the seven cheaper
performance levers, unpriced — **which is, on the criteria's own terms, reason
enough to keep deferring a code generator without reading any share at all.**
