# ADR 0015 — The W5 implementation contract

Status: accepted

ADR 0008 settled what a host handler is, 0013 built the server and 0014 put a
database behind it. This one settles what it takes to **operate** the result:
what a function records, where a credential may go, where a value comes from,
what a signal does, and what gets shipped. Where 0008, 0011, 0012, 0013 or 0014
disagree with this, this wins — it was written after them, against the code.

## The rule everything else follows from

> **A row says what a function records, and a type says where a credential
> cannot go.** Operations is the milestone where a service acquires the three
> things every service acquires — a log, a configuration, and a way to stop —
> and each of them is, in every other language, ambient: a global logger, a
> global environment, a global signal handler. Ambient is exactly what this
> language has spent eight milestones removing, because ambient means the
> runtime does not know what a computation can do. So all three are effects with
> resource-granular atoms, and the one value that must never reach any of them
> is refused by the type checker rather than by a redaction pass at the sink.

Six corollaries, each of which decides a section:

1. **A trace call is a `perform`, always, and there is no disabled path that
   skips it.** A row cannot be conditional, so a span costs what a span costs;
   what W5 buys instead is that the cost is *bounded and stated* — one perform,
   one fields map, no formatting and no clock read at the call site (§1).
2. **Containment is a claim about sinks, not about sources.** `Secret<a>` has no
   constructor pattern, no path to `String`, no derivation, no generator and no
   rendering, so every route from a credential to a log is a type error rather
   than a review item. The routes that stay open are enumerated, because a
   guarantee with an unstated hole is worse than no guarantee (§2).
3. **The environment may supply a value and may never cause a binding.** ADR
   0014 §3's rule for the database password generalises to all configuration,
   and the snapshot is frozen at bind time so that `config` is a `read` and two
   tests reading it never conflict (§3).
4. **A drain that abandons an open transaction is data loss, so teardown has one
   pinned order and it is this ADR's, not an implementation's** (§4).
5. **An artifact is a checked program, identified by a digest, and W5 ships the
   whole of it.** Incremental transfer is measured and refused; incremental
   *review* is kept, because that is the part content addressing was actually
   buying (§5).
6. **Every number an operator reads is a fact the run already holds.** Nothing
   in §6 is computed for the banner (§6).

And one more that is not a section but a hazard, because W4 found it the hard
way: **each of the three new host states is a fresh chance to couple two tests
the footprint graph believes are disjoint.** §7 is the accounting.

---

## 0. What W5 adds, in one table

| module | effect | atoms | host handler | twin |
| --- | --- | --- | --- | --- |
| `std.trace` | `trace` | `trace.write[c]` per channel | `ply_host::trace` | `Sink`, pure Ply |
| `std.config` | `config` | `config.read[k]` per namespace | `ply_host::config` | `Values`, pure Ply |
| `std.signal` | `signal` | `signal.read` | `ply_host::signal` | a cell, pure Ply |

plus a builtin type constructor `Secret`, three builtins over it, `ply build`,
and the shutdown sequence. No new external crate; `tokio` gains its `signal`
feature and nothing else.

---

## 1. Observability as an effect

### 1.1 `std.trace` — the declaration

`crates/ply-std/ply/trace.ply`, module `std.trace`, program-wide effect name
`std.trace.trace`. Ply source shipped with the compiler, for the reason
`std.net` and `std.db` are: the signature the driver binds against and the
signature the program performs are one text that cannot drift.

```ply
pub type Level = Debug | Info | Warn | Error

pub type Field =
  | FInt(Int) | FBool(Bool) | FText(String)
  | FFloat(Float) | FDecimal(Decimal) | FBytes(Bytes)
  | FJson(json::Json)

// Column name to value, a `Map` (ADR 0012 §2), so two field sets built in
// different orders are `values_equal` and a golden test over a trace line is
// stable.
pub type Fields = Map<String, Field>

// A span's identity. An ordinary record, not an opaque handle: an id is what
// correlates two lines in a log, so a program must be able to put one in a
// field, and there is nothing to protect — forging one is `E0445`, which is a
// better answer than a type that cannot be written down.
pub type Span = { id: Int, channel: String }

pub type Outcome = Ok | Failed(String) | Abandoned

pub nondet effect trace {
  write event[c](level: Level, name: String, fields: Fields) -> Unit
  write enter[c](name: String, fields: Fields)               -> Span
  write exit[c](span: Span, outcome: Outcome)                -> Unit
  write count[c](name: String, delta: Int, fields: Fields)   -> Unit
  write gauge[c](name: String, value: Decimal, fields: Fields) -> Unit
  write time[c](name: String, micros: Int, fields: Fields)   -> Unit
}
```

**`nondet` is load-bearing and it is the same sentence `std.net` and `std.db`
carry.** A production sink stamps a wall-clock timestamp and mints a span id;
neither is a function of the program's state. So a `det` test that reaches an
unhandled `trace` operation is `E0412` at compile time, whether or not `--host`
was passed, and the only way to make such a test compile is to install a
collecting handler — which is precisely the substitution this milestone exists
to make possible. The alternative, a deterministic `trace` effect, would make
that test compile and then fail at run time with `E0424`; a compile error is
strictly better and costs nothing anyone wanted.

**Six operations rather than two effects.** Metrics live on `trace` and not on
their own effect. A `metric` effect would declare operations no function can
abstract over — Ply cannot be polymorphic in an effect, which is W3 §TLS's
argument — so every handler clause set in the system would double, and
`examples/desk.ply` already writes eleven. The distinction two effects would
draw is not one any scheduling decision consults: a counter and a span are both
appends to one sink. The cost is stated rather than hidden: **a function that
only increments a counter carries a `trace.write[c]` atom**, which reads as
"this function records on channel `c`", and that is exactly true.

`gauge` takes a `Decimal` and not a `Float`, because `Float`'s `==` is not an
equivalence relation (W2) and a gauge is a number a test asserts on.

### 1.2 The resource label is a channel, and the call site writes it

`trace.event[orders](Info, "placed", fs)` performs the atom
`(trace, orders, Write)`. Resource labels are ground identifiers in the source
and the language has nothing else, so the call site is the only place the label
can come from — the same sentence ADR 0014 §2.1 writes about a table name.

A **channel** is a subsystem of the service: `http`, `db`, `orders`, `items`.
The property it buys is the one the whole design is about, stated for
observability:

```
   place_order  : (Bytes) -> Response / {std.trace.trace.write[orders],
                                         std.db.db.read[items], ...}
```

`ply check --types` now answers *which channels an endpoint records on* out of
the type, beside which tables it touches. The roadmap's "which routes write this
table" gains a sibling, "which routes record on this channel", and neither is a
comment that can go stale.

**`trace.write` — one singleton atom for everything — is refused**, and this is
the decision §1 exists to make. It would mean every definition that records
anything carries one atom, so any two tests that record anything conflict and
are serialised. `examples/desk.ply` has fifty-five tests and most of them would
be in one concurrency group. Worse, it would make the row say "records" and
nothing more, which is `db.write[db]` with a different noun — the exact thing
ADR 0014's opening rule refuses.

What that granularity costs, stated: **a channel label cannot be abstracted
over.** A helper `fn info(name, fs)` that performs on the caller's channel is
not expressible, because the label is syntax. So `std.trace` ships **no function
that performs** — only the value constructors, the twin, and the sink codec.
Every perform is written at its call site with its channel. This is the same
boilerplate `desk.ply`'s eleven handler clauses already pay, and it is what
makes a clause list a capability grant rather than a formality.

### 1.3 Span nesting, and what closes an abandoned span

A span is opened by `enter` and closed by `exit`. **The program does not
maintain the stack** — the handler does, per task — and that is not a
convenience, it is the only correct answer, because three of the four ways a
Ply computation leaves a span never run another line of it:

| how the body leaves | what runs after `enter` |
| --- | --- |
| returns normally | the program's `exit` |
| `db.rollback` (ADR 0014 §1.1) | **nothing** — the clause discards the continuation |
| raises `E0501` / `E0502` / a spent budget | **nothing** — the raise propagates past |
| the entry point ends with the span open | **nothing** |

So the rules are:

- `enter[c]` pushes onto the performing task's span stack and answers a `Span`
  whose `id` ascends from 1 **within an entry point** and is never reused within
  one. Per entry point rather than per run, and §7 is why: an id is a value that
  crosses back into the program — a `Span` is an ordinary record precisely so a
  program can put its id in a field — so a run-global counter makes a program's
  own answer a function of how much tracing a footprint-disjoint entry point did
  beside it. A `ply test --host` verdict that depended on `--jobs` is W4's
  pooled connection with a different noun. What it costs, stated: two entry
  points of one run may both hold span `1`, so a reader correlating lines across
  entry points uses the record's `seq`, which is the sink's and stays
  run-global. A service serves every request from one entry point, so nothing
  about a production log changes.
- `exit[c](s, outcome)` closes `s` **and every span the same task opened above
  it**, each of the latter with `Outcome::Abandoned`. That is the ordinary
  rollback case and it is not a warning: a discarded continuation is what a
  rollback *is*, and a record marked `Abandoned` is the useful signal.
- `exit` naming a span that is not open on this task's stack — closed already,
  never opened, or opened by another task **of the same entry point** — is
  **`E0445 SPAN_UNBALANCED`**, the program's fault, naming both tasks when there
  are two. Silently accepting it is how one request's timing lands under another
  request's span. Which of the three it is, is decided from the performing entry
  point's own table and from nothing else: E0445 is attributed and bisected like
  any other program failure, so its text is what a `--json` failure object, a
  cached failure report and a bisection carry, and a classification that
  consulted the run would put another test's `MachineId` into this test's failure
  report and change with `--jobs`.
- **Whatever is still open when an entry point ends is closed by
  `end_entry_point`**, with `Outcome::Abandoned`, and the run reports
  **`W0609 SPAN_ABANDONED`** with the count and the innermost span's name. This
  is ADR 0014 §1.3's hook doing a second job and needing no new mechanism, which
  is the strongest available evidence that the hook was the right shape.

The twin does the same thing by a different route: `drain` closes every span
still open in the `Sink` as `Abandoned`, so a hermetic test observes the same
records a bound run would write. A twin that left them open would be a twin
whose divergence from production is precisely in the failure path.

### 1.4 What a span costs when nothing is collecting

This is the question a service pays on every request, so it gets a precise
answer rather than a reassurance.

**There is no configuration under which a trace operation is not performed.** A
row is a claim about what a computation can do and it cannot be conditional on a
flag, so `--trace off` does not remove the perform — it binds
`ply_host::trace::discard`, a real, listed member of the trusted computing base
whose clause returns `Unit`. `ply hosts` prints it. An empty registry would be
`E0424` at the first event, which is correct and is not what "off" should mean.

So the cost of a disabled span is exactly:

1. the `Fields` map the call site built — O(fields), one allocation;
2. one `perform`: a `Stack::find_handler` walk that fails, then a host binding
   resolution, then a handler call that returns `Unit`;
3. nothing else.

And "nothing else" is the part that is designed rather than observed:

- **A call site never formats.** `event` takes a static `name` and structured
  `fields`; there is no operation anywhere in W5 that takes a pre-rendered
  message. Rendering is the sink's, and a discarding sink renders nothing.
- **A call site never reads a clock.** The timestamp is stamped by the sink from
  the host clock, so a disabled span pays no `clock.now()` — which also keeps
  `clock` out of every tracing function's row, and that is why a trace call does
  not drag `clock.read` into fifty endpoints' signatures.
- **A call site never allocates a span record.** `enter`'s `Span` is two
  scalars; the record is the sink's, and the discarding sink builds none.
- **Level filtering happens in the sink, not at the call site**, and therefore
  saves (1) nothing and (3) everything. `--trace-level warn` does not make a
  `Debug` event free; it makes it cost one perform and one map. Saying otherwise
  would be the misleading claim, because the only way to make it free is a
  conditional row.

**The number is the exit criterion, not the argument.** W1 measured the
per-request interpreter cost and W6 turns on it; W5 owes the same for a span. A
required benchmark reports the per-event and per-span cost against
`ply_host::trace::discard` and against the JSON sink, and a counting harness —
never a stopwatch — asserts that a discarded event performs zero host
operations beyond the one, allocates one `Fields` map, and calls no clock. If
that number turns out to be the reason a service cannot afford tracing, that is
a W6 finding with a measurement attached, and the fix is a cheaper perform
rather than a row that lies.

### 1.5 The collecting twin — Ply, and pure

Exactly ADR 0014 §5's shape, because it is the shape that works:

```ply
pub type Kind = KEvent | KEnter | KExit | KCount | KGauge | KTime

pub type Record = {
  seq:     Int,               // ascending from 1, the total order in the sink
  kind:    Kind,
  level:   Level,             // `Info` for a span or a metric
  channel: String,
  name:    String,
  span:    Int,               // 0 for an event outside any span
  parent:  Int,               // 0 at depth zero
  fields:  Fields,
  outcome: Outcome,           // `Ok` for anything but a `KExit`
  amount:  Int,               // `count`'s delta, `time`'s micros, else 0
  value:   Decimal,           // `gauge`'s value, else 0
}

pub type Sink = { .. }        // opaque: records, the open stack, the next id

pub fn sink() -> Sink
pub fn event_step(s: Sink, c: String, l: Level, n: String, fs: Fields) -> Sink
pub fn enter_step(s: Sink, c: String, n: String, fs: Fields) -> {sink: Sink, span: Span}
pub fn exit_step(s: Sink, sp: Span, o: Outcome) -> {sink: Sink, ok: Bool}
pub fn count_step(s: Sink, c: String, n: String, d: Int, fs: Fields) -> Sink
pub fn gauge_step(s: Sink, c: String, n: String, v: Decimal, fs: Fields) -> Sink
pub fn time_step(s: Sink, c: String, n: String, us: Int, fs: Fields) -> Sink

// Closes every still-open span as `Abandoned`, then answers the records in
// `seq` order. The one way records leave a `Sink`.
pub fn drain(s: Sink) -> List<Record>

pub fn named(rs: List<Record>, n: String) -> List<Record>
pub fn on_channel(rs: List<Record>, c: String) -> List<Record>
pub fn counter_total(rs: List<Record>, n: String) -> Int
pub fn open_spans(s: Sink) -> Int
```

`exit_step` answers `ok: false` where the bound driver answers `E0445`, because
a twin is a value and a value does not raise. The handler clause is what turns
`false` into whatever the program wants; `std.trace` does not decide it.

The consequence that matters, and it is the whole reason the twin exists: after
a `with_cell` region discharges its atoms, a **trace-collecting test's row is
empty**. It is `det`. It is cached. It is hermetic without `--host`. And a test
can assert on the exact records a request produced — which is how "what this
endpoint records" becomes a checked claim rather than a hope.

**Where the collector goes matters, and getting it wrong is §7's defect.** The
`with_cell` belongs *inside each test*, not around the suite. A collector
installed once around every test is one shared cell, and two tests asserting on
it are coupled exactly as W4's pooled connection coupled two tests the footprint
graph believed were disjoint.

---

## 2. Typed secrets

The milestone's headline deliverable. Credential leakage into logs is among the
most common real-world security failures, and this language has the machinery to
make it a compile error. This section says exactly what is made impossible and
exactly what is not.

### 2.1 The claim, stated so it can be falsified

> **No value of type `Secret<a>` can reach a trace field, a JSON document, a SQL
> parameter, an HTTP response, a diagnostic message, an assertion diff, a panic
> payload, a `--json` object, a definition hash, the front-end store or the
> result cache.**
>
> Every one of those is reached through a function, a derivation or an evaluator
> path whose parameter type `Secret<a>` does not inhabit, and the two paths that
> take *any* value — `Value::render` and `values_equal` — are closed at the
> evaluator. Nothing here is a review rule.

§2.5 is the list of what this does **not** prevent, and it is not short.

### 2.2 The mechanism

`Secret` is a **builtin type constructor of arity 1**, added to
`BUILTIN_TYPE_CONS` beside `Cell` and `Task`, and `Value::Secret(Arc<Value>)` is
a **distinct `Value` variant**. Both halves are load-bearing:

- **A distinct variant, not `Value::Ctor { name: "Secret", .. }`.** A `Ctor` is
  matchable, and `match s { Secret(plain) -> plain }` would be a one-line
  escape. `Secret` declares no constructors, so `PatternKind::Ctor` naming it is
  `E0101 UNKNOWN_NAME` at resolution, and there is no pattern that binds the
  payload. This is the single most important line in §2.
- **A builtin type constructor, not a record or an ADT in `std`.** A `std`-level
  `type Secret<a> = { value: a }` is field access away from useless, and a
  project could declare its own. `Secret` joins the names no `type` item may
  claim.

Introduction, elimination and the three total operations:

| builtin | type | what it is |
| --- | --- | --- |
| `secret_of_string` | `(String) -> Secret<String>` | the only introduction; see §2.5 (1) |
| `secret_verify` | `(Secret<String>, String) -> Bool` | constant-time over the compared bytes; **leaks one bit per call** |
| `secret_is_empty` | `(Secret<a>) -> Bool` | the check a start-up wants, and one bit |

There is **no** `secret_expose`, no `secret_len`, no `secret_map`, no
`secret_slice` and no `String` in any return type. The only way a plaintext
leaves is that a host operation is handed the whole `Secret` — see §2.4.

Evaluator behaviour, each closing a route that would otherwise be total:

- **`Value::render` is `Secret(****)`**, always, whatever the payload. This is
  what closes the assertion diff, the panic payload, the `ply run` result line,
  M5's failure JSON and every `Diagnostic` message that interpolates a value.
- **`values_equal` on two `Secret`s compares their payloads in constant time**
  and answers a `Bool`. A `Secret` is never equal to a non-`Secret`. So `==`
  works, `assert_eq` over a record holding one works, and neither prints
  anything.
- **`compare_values` on a `Secret` is `E0502 RUNTIME_ERROR`** naming
  `secret_verify`. The line between the two is not taste: equality leaks one bit
  per call, while an ordering leaks a bit of *position* per call and recovers the
  whole value in a number of calls proportional to its length. So `derive eq`
  accepts a `Secret` field and `derive ord` refuses it, and the refusal has a
  runtime backstop for the path that reaches `compare_values` without a
  derivation.
- **The backstop is one gate, not one check per builtin, and that is the
  correction the audit forced.** It was first written as a call at each map
  operation, and it was written at four of the six: `map_of_entries` and
  `map_merge` reach the tree by a different route and had none, so a `Secret`
  used as a key through either was a total ordering oracle over the plaintext —
  full recovery in comparisons proportional to its length, through a route the
  table in §2.3 lists as *closed*. It now lives under `ply_eval::map`'s single
  key gate, which every key entering, leaving or being looked up in a `Map`
  passes through and which is the only caller of `insert_mut` in that module.
  The general lesson is the one this project keeps paying for: a mitigation
  spelled once per call site is a mitigation the next call site does not have.
- **`Map<Secret<a>, v>` is `E0206 NOT_DERIVABLE`**, because a `Map` key needs
  `derivable(ord, k)` (ADR 0012). A secret as a map key would be an ordering
  oracle with a data structure attached.
- **`Secret<a>` is not quantifiable**: a `forall (s: Secret<String>)` binder is
  `E0418 UNQUANTIFIABLE_TYPE`, exactly as `Cell` and `Task` are. A generator that
  minted secrets and a shrinker that printed counterexamples is a leak by
  construction, and the code for it already exists.
- **Derivation**: `derivable(json, Secret<a>)`, `derivable(ord, Secret<a>)` and
  `derivable(row, Secret<a>)` are false — `E0206 NOT_DERIVABLE` **naming the
  field**, the same message shape a `Cell` field gets. `derivable(eq, ·)` holds.
  Asked twice, of one predicate, exactly as ADR 0012 specifies: `ply_derive::walk`
  before a body is generated, and `ply_core`'s walk over the *solved* type, so an
  alias `type Password = Secret<String>` is caught too.

### 2.3 Route by route, and what closes each

| route a credential would take | what closes it | how it fails |
| --- | --- | --- |
| `trace.event[c](.., fields)` | `Field` has no constructor over `Secret<a>`, and `Secret<String>` does not unify with `String` | `E0201 TYPE_MISMATCH` at the call site |
| `"user=" ++ pw` | `++` is `String`-only (ADR 0011 §Text) and `Secret<String>` is `Con("Secret", [String])` | `E0201` |
| `derive json for Login` with a `Secret` field | §2.2's derivation rule | `E0206`, naming the field |
| a JSON response body | as above, and `json::Json` has no `Secret` case | `E0206` / `E0201` |
| a SQL parameter | `Param` has no `PSecret`; `PText` takes `String` | `E0201` |
| an HTTP header or body | `Headers = Map<String, List<String>>`, `body: Bytes`, and `bytes_of_string` takes `String` | `E0201` |
| `panic("token " ++ pw)` | `++`, above | `E0201` |
| `assert_eq(pw, other)` failing | `Value::render` | prints `Secret(****)`, both sides |
| a `Diagnostic` from a builtin handed one | `Value::render` | `Secret(****)` |
| M5's failure JSON, the result cache, `--explain` | all render through `Value::render` | `Secret(****)` |
| a definition hash / `frontend.dat` | there is no `Lit::Secret`, no literal syntax, and no normalization tag; a `Value` never enters a hash | not expressible |
| a `law` counterexample | `E0418` at the binder | the law does not compile |
| `Map` key ordering | `derivable(ord, ·)` at the signature, and `ply_eval::map`'s key gate under **every** map operation | `E0206`, or `E0502` naming `secret_verify` if a defect in either walk got past it |
| a clause laundering one into an ordered type | §2.7: an operation's type variables are rigid where its clause is checked | `E0201` at the clause |
| a pattern match | `Secret` declares no constructors | `E0101` |
| a host handler | §2.4 | `E0439` |

### 2.4 A secret reaching the host is declared and enumerable

`HostOp` gains one field:

```rust
    /// Whether this operation may be handed a value containing a `Secret`.
    /// Printed by `ply hosts` in its own column, because a handler that
    /// receives a credential is the one place above the boundary where §2.1's
    /// claim stops being enforceable and starts being review.
    pub secrets: bool,
```

The machine checks it: a `perform` whose arguments contain a `Value::Secret`,
resolved to a registration with `secrets: false`, is **`E0439 SECRET_TO_HOST`**
before the handler is called, naming the operation and the argument position.
Ply's fault in the same sense `E0427` is — the boundary's own account of itself
disagrees with what crossed it — so it is `Status::Panicked`, `defect: true`,
and not bisected.

**In W5 no operation declares `secrets: true`**, so the column reads `no` on
every row and the check is a tripwire rather than a gate. That is stated rather
than omitted: the mechanism is landed with a user count of zero, because the
moment W6 adds an outbound HTTP client with a bearer token it will have one, and
adding the check then would mean adding it after the first operation that needed
it already shipped.

The two credentials W5's own stack holds — the TLS private key and the postgres
password — are configured *beside* the run (ADR 0013 §TLS, ADR 0014 §3), never
enter the program, and are held in `ply_cli::db::Secret`, whose `Display` and
`Debug` are `****` and whose only exit is `expose`. Nothing about them changes.
The Ply-level `Secret<a>` and the Rust-level `ply_cli::db::Secret` are two
mechanisms for two populations and neither replaces the other; they are named
alike because they make the same promise at two layers, and `ply hosts` prints
both.

### 2.5 What this does **not** prevent

Written out, because a secret that can be exfiltrated by a route the ADR did not
mention is not protected, and an unstated hole is worse than a stated one.

1. **A credential written as a source literal.** `secret_of_string("hunter2")`
   puts `"hunter2"` into a `Lit::Str`, which normalizes into the definition's
   bytes and lands in a content-addressed store designed never to forget. The
   wrapper changes nothing, because the *literal* is what entered the hash.
   `Secret` does not defend the source tree; what defends it is that a real
   credential comes from `config.secret`, and a value that came from
   configuration is in no hash because it is in no definition. **This is the
   largest hole in §2 and there is no mechanism in this ADR that closes it.**
2. **The plaintext the secret was built from.** Ply is a value language, so
   `secret_of_string(pw)` does not consume `pw`; the `String` is still in scope
   and can be traced, concatenated and returned. Containment starts where the
   `Secret` starts.
3. **`secret_verify` leaks one bit per call**, and a program that loops it over
   candidates recovers the value. The builtin is constant-time in the compared
   bytes; it is not rate-limited, and rate limiting is the program's.
4. **Timing beyond the comparison.** `secret_verify` and `==` are constant-time
   over the bytes they compare. Nothing else in the interpreter is, and what a
   program *does* with the `Bool` — a branch that takes a different number of
   evaluator steps, a trace event on one arm only — is an oracle W5 neither
   creates nor closes.
5. **A host handler that receives one.** §2.4's check says which operations may;
   what such a handler then does with it is invisible, exactly as ADR 0008 §2
   says of every other handler claim. `ply hosts` plus review is the whole
   defence, which is why the column is printed rather than derived.
6. **Memory.** There is no zeroization. A `Secret` is an `Arc<Value>`; the
   payload is not wiped on drop, the allocator may reuse the pages, and a core
   dump, a swap file or a debugger has the plaintext. Ply's evaluator copies
   values freely and a zeroizing type in it would be a promise the runtime cannot
   keep.
7. **A secret's *presence* is observable.** `secret_is_empty`, a row containing
   `config.read[credentials]`, and the start-up banner's key list all say that a
   credential exists and where it came from. That is deliberate — an operator
   must be able to tell a missing credential from a wrong one — and it is
   metadata, never the value.
8. **Length.** No builtin reports it, but `secret_verify` against candidates of
   increasing length does, in the same one-bit-per-call sense as (3).

### 2.6 Alternatives rejected

- **An effect only a redacting handler may discharge.** Handlers are checked by
  *signature*, not by identity, so "only the redacting handler" is not something
  the type system can say. It would be a convention wearing an effect's clothes,
  and replacing conventions with mechanisms is the whole point of the milestone.
- **Derivation refusal alone.** It closes the derived-JSON route and leaves
  `++`, `Field`, `Value::render` and pattern matching wide open. It is one of
  the four mechanisms above, not a design.
- **Redaction at the sink** — a regex over outgoing log lines, or a registry of
  known secret values scrubbed on write. It fails on any transformation
  (`base64(pw)`, `pw[0..8]`), it fails on a value the registry never saw, and
  the failure is silent. This is the thing W5 exists to replace.
- **An affine or linear `Secret`.** Use-once would prevent (3)'s loop, and Ply
  has no substructural types; adding one for this would be a type-system
  milestone attached to an operations milestone.
- **Leaving the ordering hole open and documenting it.** It was reachable from
  an ordinary program with no `--host`, no unsafe code and no handler declaring
  `secrets: true`, and it recovered the whole plaintext. A hole that recovers
  the value is not a hole a §2.5 entry can honestly describe; it is a hole that
  falsifies §2.1. It is closed.

### 2.7 The delivery vehicle, and a general soundness rule

The route above needed a `Secret` masquerading as an ordered type, because a
`Secret` typed honestly as a `Map` key is `E0206` at the signature. What supplied
one was **not** about secrets at all:

> A handler clause for an operation with a polymorphic return was checked against
> a **fresh** instantiation of the operation's type, never unified with the fresh
> instantiation the perform site used. For `effect vault { read fetch[k](s:
> Secret<String>) -> a }`, the clause `vault.fetch[k](s) -> s` answered a
> `Secret<String>` while the perform site had unified `a := String`, and `ply
> check --types` printed a `String` return for a function that produced a
> credential. The same shape typed a clause answering an `Int` for an `-> Int`
> caller's `String`, and failed only at run time with `E0502`.

The rule, and it is a language rule rather than a W5 one: **a handler clause is
checked with the operation's own type variables rigid.** A clause is written once
and answers every perform site there will ever be, and a row carries atoms and no
types — so a `handle` cannot see which `a` a perform three definitions deeper
picked, and a clause that chose would be handing a caller a value of a type it
never asked for. Rigid variables are what say "for every `a`", which is the
obligation a clause actually carries. `E0201` names the clause, points at the
declaration, and says why a concrete answer is not one it may give.

What that costs, stated: **an operation whose return type is a variable its
parameters do not determine can no longer be handled at all**, because no clause
can produce a `List<a>` out of nothing. That is correct — such an operation is
unsound rather than merely awkward — and it is not a hypothetical:
`examples/store.ply` declared `read all[table]() -> List<a>` and was rewritten to
declare what each of its tables holds. Its resource labels, its footprints and
the schedule read off them are unchanged. `FRONTEND_VERSION` moves for it (§9).

---

## 3. Configuration

### 3.1 `std.config` — the declaration

```ply
pub nondet effect config {
  read get[k](key: String)    -> Option<String>
  read secret[k](key: String) -> Option<Secret<String>>
}
```

Three properties, each decided rather than inherited:

- **`read`, not `write`, and therefore never a conflict.** Two tests that read
  configuration are placed in one concurrency group and run beside each other,
  which is sound only because §3.3 freezes the source at bind time. **There is
  no `config.set`**, and adding one would make the atom a write and serialise
  every test in a suite that reads a single key.
- **`nondet`.** The environment is not a function of the program's state. So a
  `det` test that reaches configuration is `E0412` at compile time and must
  supply it — which is §3.5, and which is the point.
- **The resource is a namespace the call site writes**, `config.read[database]`,
  `config.read[credentials]`. Since reads never conflict it buys no scheduling,
  and it buys the thing §1.2 buys: `ply check --types` says which definitions
  read configuration and which read *credentials*, and the second is the row a
  reviewer actually wants.

`get` answering `Option` rather than raising is ADR 0014 §0's rule applied to a
second peer: a missing key is a value the program matches on. The failure an
operator actually suffers — a service that starts and then answers wrongly
because a key was unset — is caught earlier and elsewhere, in §3.4.

### 3.2 Where values come from, and in what order

Highest wins:

| # | source | flag | shape |
| --- | --- | --- | --- |
| 1 | the command line | `--set KEY=VALUE`, repeatable | exact key |
| 2 | a file | `--config PATH`, repeatable, later files win | `KEY=VALUE` per line |
| 3 | the process environment | — | exact key, no prefix and no mangling |
| 4 | the spec's `default` | — | §3.4 |

**The file format is `KEY=VALUE`, one per line**, `#` to end of line as a
comment outside a value, blank lines ignored, no quoting, no interpolation, no
sections, no escapes, and the value is the rest of the line with surrounding
horizontal whitespace trimmed. A line without `=`, or with an empty key, or with
a key that is not `[A-Za-z_][A-Za-z0-9_.]*`, is **`E0440 CONFIG_UNAVAILABLE`**
naming the file and the line number.

TOML, YAML and JSON are refused for one reason: the effect's return type is
`Option<String>`, so a nested, typed format would carry structure this program
cannot receive, and a format richer than the type it feeds is a format whose
extra structure is silently dropped. That is the ADR 0014 §4.2 argument about a
`numeric` losing a cent, applied to a config file. The second reason is that
none of the three is in the dependency tree and a parser in a trusted computing
base is the line ADR 0013 says is worth a human's attention.

**Environment keys are exact.** No `PLY_` prefix, no upper-casing, no `.` to `_`
translation. A key is one string in one namespace across all four sources, so
"which key was that" has one answer. `PLY_DB_URL` and `PLY_DB_PASSWORD` (ADR
0014 §3) keep their names and are read by `ply-cli`, not through this effect;
they are the run's configuration of a *binding* and §3.3 is why that is a
different thing.

### 3.3 The environment is read exactly once

`ply_host::config` reads `std::env::vars()`, the `--config` files and the
`--set` arguments **at bind time**, into one immutable `BTreeMap`, and never
consults the process environment again. `config.get` is a lookup in that map.

This is one line of implementation and it is the whole of §3's soundness:

- it is what makes `config.read[k]` honestly a **read** — the source cannot
  change under a run, so two readers cannot disagree and the conflict graph is
  right;
- it is what stops a test's `setenv` from being seen by another test, which is
  §7's second entry and is the pooled-connection defect in a new costume;
- it is what makes a run reproducible: the snapshot is a fact of the run, it is
  printed (§6), and its non-secret half is in `ply hosts --json`.

**Configuration may supply a value and may never cause a binding.** ADR 0011's
rule stands untouched: a reviewer reads `--host` in the command or the run
reached nothing. Without `--host` no configuration source is opened at all,
whatever the environment holds, and `config.get` is `E0424` naming
`ply_host::config::get`.

### 3.4 Start-up, not first request — and `--config-schema`

A service that starts, serves two hundred requests and answers wrongly because
`DESK_API_KEY` was unset is the failure mode this section exists to prevent.
ADR 0014 §7 solved the same problem for a database by checking the schema at
bind time; W5 does the identical thing for configuration.

```ply
pub type Shape = SText | SInt | SBool | SSecret
pub type Key = {
  name: String, shape: Shape, required: Bool, default: Option<String>,
}
pub type ConfigSpec = { keys: List<Key> }
```

`--config-schema <module>.<fn>` names a nullary function returning a
`ConfigSpec`. At bind time the run materialises it, resolves every key against
§3.2's sources, and:

- a `required` key nothing supplies is **`E0441 CONFIG_MISSING`**, naming the
  key, its shape, and the four places it looked;
- a resolved value that is not of its `Shape` — `SInt` that is not an `Int`,
  `SBool` that is not `true`/`false` — is **`E0442 CONFIG_INVALID`**, naming the
  key and the source that supplied it, **and never the value when the shape is
  `SSecret`**;
- an explicit key — from `--set` or `--config`, never from the environment —
  that the spec does not declare is **`W0607 CONFIG_UNDECLARED`**. Only the
  explicit sources, because an environment is full of names that have nothing to
  do with this program, while a `--set` is something a person typed on purpose
  and a typo in one is the classic silent deploy failure.

`--config-schema` is optional. Without it, a missing key surfaces as `None` at
the call site — later, per key, and still the program's to handle.

A `SSecret` key resolves to a `Secret<String>` and `config.get` **will not
return it**: `get` on a key the spec declares `SSecret` answers `None`, and
`secret` on a key declared anything else answers `None` too. Without that, the
type-level guarantee would be one call site away from a `String`. Without a
spec, `get` and `secret` both answer whatever the sources hold, and the ADR says
plainly that **§2's containment for configured values is only as strong as the
spec**: a run with no `--config-schema` can read a password with `config.get`
and get a `String`.

### 3.5 How a test supplies configuration hermetically

The same way it supplies a database: by handling the effect.

```ply
pub type Values = { plain: Map<String, String>, secret: Map<String, String> }

pub fn values(plain: List<{key: String, value: String}>,
              secret: List<{key: String, value: String}>) -> Values
pub fn get_step(v: Values, key: String)    -> Option<String>
pub fn secret_step(v: Values, key: String) -> Option<Secret<String>>
```

```ply
handle { place_order(body) } with {
  config.get[database](k)      -> config::get_step(fixture(), k),
  config.secret[credentials](k) -> config::secret_step(fixture(), k),
}
```

After the handle, the row is empty; the test is `det`, cached and hermetic. A
test's fixture secret is a source literal by nature, which is §2.5 (1) and is
harmless — a fixture credential is not a credential.

### 3.6 Startup versus per request, and the line that decides it

**Configuration is read at start-up and is a value thereafter.** A program
performs `config.*` in its entry point, builds a record, and passes it as an
ordinary argument to everything below. No definition below the entry point
carries a `config` atom in a well-built service, and `ply check --types` is how
that is checked rather than asserted.

There is no live reload. A configuration that can change mid-run is a
nondeterminism that would have to be in every reader's row, and two requests in
one run that saw different values is a class of bug with no repro.

The line between what may be configured and what may not is worth stating,
because getting it wrong is how a service acquires behaviour no test covers:

- **May be configured**: the identity and credentials of the peers a run talks
  to, the address it listens on, and the deployment's own name.
- **May not**: anything the program's behaviour is *specified* in terms of —
  `http::Limits`, the route table, business rules, retry policy. Those are what
  tests assert on, and a value that differs per environment is a value no test
  covers. W3 already said this about `Limits` ("no global, no environment
  variable and no flag, so two runs of one program cannot differ in what they
  refuse") and W5 does not weaken it.

Nothing enforces that line, and W5 does not pretend otherwise — but it makes it
*visible*, which is the enforcement this language actually offers: a definition
that reads configuration says so in its row, so the configured part of a service
and the specified part are distinguishable in `ply check --types`, and a `det`
test that handles no `config` at all is a proof that what it covered was the
second kind.

---

## 4. Graceful shutdown

### 4.1 `std.signal` — the declaration

```ply
pub nondet effect signal {
  read stopping()    -> Bool
  read deadline_ms() -> Int    // milliseconds left in the drain; -1 when not draining
}
```

No resource parameter, so the atom is the singleton `signal.read`. Two readers
never conflict, which is right: nothing a program does to this effect changes
it.

`deadline_ms` exists so a handler can decide to shed rather than to start work
it cannot finish — answer `503` on a request that arrived with four hundred
milliseconds left. Without it the only choice is to begin and be cut off.

### 4.2 What a signal does, in order

`SIGINT` or `SIGTERM`, delivered to `ply run --host`. `ply_host::signal`
registers with `tokio::signal` on the reactor thread the db driver already owns
(ADR 0014 §3.1); the handler sets an atomic flag and nothing else, which is the
only thing a signal handler is allowed to do.

| phase | what happens | bound by |
| --- | --- | --- |
| 0 | the flag is set. `signal.stopping()` answers `true` from the next perform | — |
| 1 | **lead**: accept keeps running so a readiness route can answer `503` and a load balancer can remove the instance | `--drain-lead-ms`, default `0` |
| 2 | **stop accepting**: every `net.accept[s]` answers `0`, and the listening sockets are closed so the kernel stops queueing | immediate |
| 2′ | **catch-up**: a socket table attached *after* phase 2 ran is stopped the moment it arrives | immediate |
| 3 | **drain**: in-flight connections finish their current request. Keep-alive is not offered — `http::encode` writes `Connection: close` because `serve_connection` was told `keep_alive: false` | `--drain-ms`, default `30000` |
| 4 | **teardown**: `end_entry_point`, then the sink flush, then the pool close — §4.4's order | — |
| 5 | **exit**: `0` if the drain completed, `3` if the deadline expired | — |

**Phase 2′ is not a refinement, it is what makes phase 2 true.** `ply run --host`
registers the signal handlers before it loads the TLS material, opens the pool —
including a real connect probe bounded by `--db-connect-ms` — binds the registry
and verifies the schema, and only then hands the coordinator the socket table. A
`SIGTERM` in that window is the ordinary shape of a rolling restart, or of a
failed readiness probe against an instance that is still coming up. Without the
catch-up, phase 2 found no `Accepting`, closed nothing, and never set the flag
that makes `net.accept` answer `0` — while `signal.stopping()` answered `true`,
so a readiness route shed and the load balancer took the instance out, and the
listener stayed open and kept serving for the whole of `--drain-ms`, after which
the run reported `W0608 DRAIN_INCOMPLETE` and exited `3`. A shutdown that was
going to be clean was reported as one that dropped requests, and requests that
should have been refused were served. `attach_net` holds the socket slot across
the read *and* the flag test, and phase 2 sets the flag while holding the same
slot, so either phase 2 saw the table or the table sees phase 2 — there is no
interleaving in which neither happens.

**The numbers phase 2 records are written before the run can observe the stop.**
`stop_accepting` is the instant `net.accept` starts answering `0`, which is what
ends a sequential accept loop, so a machine thread can be through the drain, the
teardown and the banner in a couple of milliseconds. The state lock is therefore
taken *around* `stop_accepting` and released before the wake dialling, which the
banner reports nothing about — otherwise §6's rule reads as satisfied while the
banner prints `0 listener(s) closed · 0 connection(s) in flight · 0
transaction(s) open` for a run that had one of each, which is exactly what it did
on `examples/desk.ply` with no special timing.

A **second** identical signal at any phase is an immediate exit with code `130`
(`SIGINT`) or `143` (`SIGTERM`), after printing one line naming what was
abandoned. A second signal means a person has decided to stop waiting, and a
process that ignores it is a process people learn to `kill -9`, which abandons
the transaction rollback in §4.4 and is strictly worse.

`SIGTERM` does not exist on Windows; `tokio::signal::ctrl_c` is what binds there,
and `ply hosts --host` prints which signals the run is listening for so the
difference is a fact rather than a surprise.

### 4.3 `desk.ply` drains with no source change, and why that is the design

`examples/desk.ply`'s `serve` is a sequential accept loop that exits on
`accept` answering `0` (ADR 0013). Phase 2 makes `accept` answer `0`. So the
existing program stops accepting, finishes the connection it is on, returns from
`serve`, closes the listener and returns a count — **and not one line of it
changes**. That is the exit criterion, and it is the direct consequence of ADR
0013's decision that `accept` answers `0` when the listener is finished rather
than raising.

Its in-flight count at the signal is exactly one, so it cannot lose a request.
A service that spawns a task per connection has N, and §4.5 is what that costs.

### 4.4 Teardown order, and the transaction that must not be abandoned

Pinned, because three of the four steps are ordering-sensitive and a wrong order
is a data-loss bug rather than a mess:

1. **`end_entry_point`** (ADR 0014 §1.3), across drivers in this order:
   1. **db** — every open transaction scope is `ROLLBACK`ed. **Never
      committed.** A commit at a deadline commits a half-finished body, and the
      only thing that knows whether a body finished is the body.
   2. **trace** — every open span is closed `Abandoned` (§1.3), so the last
      records a dying request produced are the ones that say what it was doing.
      After db, so that a span can record the rollback.
2. **flush the sink** — the JSON writer flushes and `fsync`s nothing (stderr is
   unbuffered at line granularity by construction; the flush is of the run's own
   buffer). Before the pool closes, so that a trace naming a rolled-back
   transaction is written before the connection that rolled it back is gone.
3. **close the pool** — connections are closed rather than returned. A
   connection whose `ROLLBACK` failed is closed and discarded, which is ADR 0014
   §1.3's existing rule and needs no new case.
4. **exit.**

**The teardown is bounded by `--drain-ms`, and that is what makes it a bound at
all.** Every step of 1–3 that waits on a peer waits for at most the budget it is
handed, and the two waiting steps *share* the budget rather than each getting
it. A run that was signalled gets whatever is left of its deadline, floored at
one second so that a `ROLLBACK` on a healthy connection can still complete after
a drain that expired; a run that ended on its own gets the whole of
`--drain-ms`, because there is no deadline it is already past. So signal-to-exit
is `lead + drain + 1s + ε` and an operator can compute it.

It was not, and the failure was the quiet kind. The steps were bounded by the
*database's* own deadlines — `--db-statement-ms` plus `--db-connect-ms` — so a
request blocked on a row lock raised `W0608` on time and then held the process
open until the statement timeout fired: `--drain-ms 1000 --db-statement-ms 8000`
exited **5955ms** after the signal, and `--drain-ms 3000` with the default
30-second statement timeout exited **19470ms** after it. For a rolling restart
that is the difference between a bounded and an unbounded stop.

A rollback that cannot finish inside the budget is answered by **closing the
connection**, which is what makes the server abandon the statement holding the
locks the rest of the restart is waiting on — ADR 0014 §1.3's existing rule, not
a new case. The discarded connection is counted and reported. Nothing about the
transaction outcome changes: it is rolled back, never committed.

Any failure in 1–3 is **`W0606 HOST_TEARDOWN`** — the existing code, doing the
job it was introduced for — naming the driver and what it could not hand back.
It does not change the exit code, because a service that shut down uncleanly
still shut down and the operator needs the distinction between that and a drain
that did not finish.

`HostRuntime` gains the process-level hook, distinct from the per-entry-point
one:

```rust
    /// Called once, after the last entry point, before the process exits. Runs
    /// the pinned order above and answers what it managed. Never called from a
    /// signal handler — the handler sets a flag, and this runs on the machine's
    /// thread.
    ///
    /// `deadline` is a **bound and not a hint**: a step that waits on a peer
    /// waits for at most this long and then discards the connection.
    fn shutdown(&self, deadline: Duration) -> Shutdown;
```

### 4.5 A request still running at the deadline

The honest answer, and it is not a good one:

**W5 has no cancellation.** ADR 0011 deferred it, ADR 0013 §7.2 argued deadlines
made it unnecessary for socket operations, and W5 does not add it. So a task
still running at the drain deadline is not cancelled, not unwound, and not
handed a 503. The process performs §4.4's teardown and exits, and the client
sees a **connection closed with no response**, or a truncated one if bytes were
already written.

Three things follow, all stated rather than implied:

- `--drain-ms` should exceed the program's own `body_timeout_ms +
  write_timeout_ms`, which for `http::default_limits()` is 60 seconds against a
  default `--drain-ms` of 30000. The run cannot check that — `Limits` is a Ply
  value the run never sees (W3, deliberately) — so it is documented and it is in
  the start-up banner, where the two numbers can be compared by eye.
- A drain that expires reports **`W0608 DRAIN_INCOMPLETE`** naming the number of
  connections abandoned, the number of transactions rolled back at teardown, and
  the elapsed time, and exits `3`. A deployment that sees code `3` knows it lost
  requests; a deployment that sees `0` knows it did not. That distinction is the
  whole product of §4.
- **A rolled-back transaction is not a lost request in the dangerous sense.**
  The client got no answer and the database has no partial write, which is the
  outcome a retry can fix. The outcome a retry cannot fix is a committed
  half-transaction, and §4.4's ordering is what makes that unreachable.

### 4.6 The scheduler must notice

`Policy::Host` runs the lowest-numbered ready task and calls
`HostRuntime::park` when none is ready (ADR 0011 §Scheduler seam). An idle
service has no ready task and is parked in `park`. If `park` waits only on
outstanding tokens, **a service with no traffic never observes the signal** and
`ctrl-C` does nothing until the next request arrives. That is a defect worth
naming before anyone writes it.

So `HostRuntime` gains:

```rust
    /// Whether a stop has been requested. `park` returns when this becomes
    /// true even with no token outstanding, and the deadlock check (`E0414`)
    /// consults it so that a park which woke on a stop is not counted as
    /// fruitless.
    fn stopping(&self) -> bool;
```

`ply-eval` reads it in exactly two places — the park loop and the deadlock
check — and nowhere else. It is not consulted by inference, by a cache key or by
`Isolation`.

### 4.7 `signal` does not bind under `ply test`

**With or without `--host`.** A test that could be ended by the suite's own
`ctrl-C`, or that observes a stop another test requested, is a test whose verdict
depends on the terminal. `ply test --host` binds `trace`, `config`, `db` and
`net`; it binds no signal handler, and reaching `signal.stopping()` under it is
`E0424` whose message says so and names the twin.

This is a deliberate asymmetry with `config`, and the reason is the difference
between the two states: a frozen configuration snapshot is read-only and cannot
couple two tests, while a stop flag set once ends every test after it. §7 is the
accounting for both.

---

## 5. Deployment over the content-addressed store

### 5.1 The honest answer first

Ply knows exactly which definitions changed, so a deploy *could* ship only
those. **W5 ships a whole-program artifact and no incremental transfer**, and
here is the reasoning rather than the conclusion.

A deploy must ship a `ply` binary, because the program is interpreted and every
guarantee is the runtime's. `RUNTIME_VERSION`, `FRONTEND_VERSION` and
`BODY_ENCODING` have each moved in most milestones, so the binary is the part
that actually changes, and it is three orders of magnitude larger than the
definitions. Shipping only the changed definitions optimises the small side of a
ratio nobody measured. **The required test prints both numbers** — the artifact's
bytes and the binary's — and the exit criterion carries them, because a decision
of this shape should be re-openable against a measurement rather than against
this paragraph.

What incremental transfer would additionally need, none of which exists: an
agent on the target, an authenticated channel, a negotiation, a rollback story,
an atomic switch, and a garbage-collection policy for definitions the target no
longer runs. That is a product, and a half-built one would be worse than none —
ADR 0014 §7's sentence about migrations, for the same reason.

What content addressing *is* worth here, and costs almost nothing because the
store already does it, is **identity and verification**. That is what §5.2 ships.

### 5.2 `ply build` and the `.plyx` artifact

```
ply build [PATH] [-o FILE] [--entry NAME] [--config-schema NAME] [--db-schema NAME]
          [--sources] [--digest] [--json]
ply build --diff OLD.plyx [PATH]
ply run FILE.plyx --host [...]
```

An artifact is **the transitive closure of its roots**, and nothing else. The
roots are the entry point and the **start-up definitions the build names** —
`--config-schema` and `--db-schema`, spelled exactly as `ply run` spells them:

```
ply build examples/desk.ply --config-schema desk.config --db-schema desk.schema
```

This paragraph used to say "one entry point, and nothing else", and that was in
direct conflict with §3.4 — a schema is a nullary function nothing in `main`
calls, so it was not in the closure, so `ply run desk.plyx --host
--config-schema desk.config` was `E0440` and the artifact served only with both
flags dropped. At which point `E0441 CONFIG_MISSING` could never fire on the
deployed form, `E0435` schema verification could never fire either, and — since
§3.4 says that without a spec `config.get` returns whatever the sources hold —
`config.get` on the deployed artifact could hand back the API key as an ordinary
`String`, which is the one thing §2 exists to prevent. The conflict was resolved
against the deploy story; it is resolved here in favour of the guarantee.

A schema is start-up code — it runs before the entry point does, exactly as the
entry point runs — rather than an exception to the rule, and everything the
closure bought is unchanged: tests, laws and specs are in no root's closure and
still fall out rather than being filtered. `ply build` prints the roots it
shipped, and prints `startup none` when there are none, because an artifact that
cannot be run with `--config-schema` is an artifact that cannot refuse to start
on a missing credential and that is the thing a deploy pipeline has to be able
to see. A name that resolves to nothing is refused at **build** time, where the
person who can fix it is holding the source tree.

What each root contributes:

- definition bodies, in `ply_hash::body`'s encoding, keyed by `DefHash` — the
  same bytes the store already holds, so `ply build` is a copy and not a second
  encoder;
- the namespace needed to resolve them;
- the entry point's name and hash;
- optionally the `SourceMap`, under `--sources`;
- versions and digests in the header.

**Tests, laws and specs are not in it.** They are in no root's closure — a
`test` is a definition nothing calls — so a deployed artifact carries no fixture
data, no seed corpus and no `law/host` that would try to reach a database. That
falls out of the closure rather than being filtered, which is the better kind of
property, and adding start-up roots does not weaken it: a schema is a definition
the run calls, which is the whole of what makes it a root.

**The header carries the entry point's hash and not the roots'.** A start-up
root is resolved by *name* on the deployed run, exactly as it is on a source
run, so what the artifact owes is that the name is in `NAMES` and the body is in
`BODIES` — which the closure already gives. The digest covers both sections, so
"was this built with its schema" is answerable from the digest alone, the same
sentence `--sources` earns.

```
header   0  magic        8    b"PLYPROG1"
         8  format       u32  ARTIFACT_FORMAT = 1
        12  flags        u32  bit 0: sources embedded
        16  frontend     32   blake3(FRONTEND_VERSION)
        48  runtime      32   blake3(RUNTIME_VERSION)
        80  body_enc     u32  BODY_ENCODING
        84  std          32   ply_std::digest()
       116  entry        32   the entry point's DefHash
       148  digest       32   §5.3
       180  sections     u32
       184  reserved     u32  0
       188  descriptors  sections × { kind u32, count u32, offset u64, bytes u64 }
            payloads
```

| kind | section | record | sorted by |
| --- | --- | --- | --- |
| 1 | `BODIES` | `{ hash [32], len u32, bytes }` | hash |
| 2 | `NAMES` | `{ name_off u32, name_len u32, hash [32] }` | name bytes |
| 3 | `STRINGS` | the name blob | — |
| 4 | `SOURCES` | present iff flag bit 0 | path |

### 5.3 What a target verifies

Everything, and each check answers a different question:

1. **Every body against its own key.** `blake3(bytes)` is the key for a
   single-definition component and `blake3(component ‖ index_le_u32)` for a
   member of one — ADR 0003's rule, and `ply_store::put_body` already refuses a
   body that fails it. A mismatch is **`E0443 ARTIFACT_INVALID`** naming the
   hash and the offset. This is what makes a corrupted transfer a per-definition
   refusal rather than a plausible wrong program.
2. **Every reference resolves inside the artifact.** A body naming a hash the
   `BODIES` section does not hold is `E0443`: the closure was computed wrong or
   the file was truncated.
3. **The header's versions against the running binary.** A mismatch in
   `FRONTEND_VERSION`, `RUNTIME_VERSION` or `BODY_ENCODING` is **`E0444
   ARTIFACT_VERSION`**, naming both sides. Its own code and not `E0443` because
   the responses are opposite: rebuild the artifact, versus re-transfer it.
   `ply_std::digest()` mismatching is `W0605 STDLIB_CHANGED`, not an error — a
   stdlib definition is content-addressed like any other, so a differing digest
   over modules the program never imported is a fact and not a fault.
4. **The digest over everything after it.** BLAKE3, domain-tagged
   `b"ply.program.1"`, over the header bytes from `sections` onward and every
   section payload in section order. `ply build --digest` prints
   `b3:` plus twelve hex characters and nothing else, which is the line a
   deployment pins — the same shape `ply hosts --digest` and `ply std --digest`
   already have.

**Two builds of one source tree produce byte-identical artifacts**, on any
machine, in any directory, from a warm or a cold cache. Bodies are normalized
(no names, no spans, no paths), sections are hash-sorted or name-sorted, and
nothing carries a timestamp. That is reproducible builds falling out of content
addressing rather than being engineered, and it is a required test run twice
from two roots.

### 5.4 `ply build --diff` — the part worth keeping

```
$ ply build --diff dist/desk-1.4.plyx
   desk.run   b3:7c02e9a41b6d → b3:91af0c33d7e2

   added      3 definitions   desk.restock_item, desk.restock_row, desk.restocked
   changed    2 definitions   desk.place_order, desk.recorded
   dropped    0
   unchanged  412

   reached by a changed definition: 7 endpoints
     place_order   cancel_order   list_items   featured   get_item   receipt   app
```

This is the deploy's review artifact, and it is the incremental story delivered
as *information* rather than as transport. It costs a set difference over two
hash sets plus the reverse closure the graph already computes, it answers "what
is actually going out" in the language's own terms, and it is the same sentence
`ply review --changed` makes about specifications.

### 5.5 What this costs, stated

- **A deployed artifact has no spans.** Normalization erases them, so a
  diagnostic raised in production carries `Span::DUMMY` and a synthesized
  definition name (`d_<hash12>`, ADR 0003 §Decoding). You cannot get a
  source-located failure out of a running service unless it was built
  `--sources`, and `--sources` puts the program's source text in whatever
  receives the artifact — a disclosure decision, which is why it is a flag and
  is off, and why the flag is covered by the digest so that "was this built with
  sources" is answerable from the digest alone.
- **No target-side inventory.** Nothing tells a sender what a target already
  has, because nothing on the target answers. `ply build --diff` compares two
  *artifacts*, which is a sender-side operation on two files.
- **No signing.** The digest establishes identity, not authenticity. A signature
  needs a key, a trust root and a revocation story, and W5 has none of those.
  Distribution integrity is the transport's, exactly as it is today for the
  `ply` binary itself.

---

## 6. What an operator sees

### 6.1 Health and readiness are routes the program writes

W5 adds **no** health effect and **no** built-in endpoint. A route table is
ordinary data (ADR 0013), so a framework-supplied `/healthz` would be a route
that is not in `table()` — a second answer to "what does this service serve",
and two answers is how a route and an authorization check come to disagree.

What W5 supplies is the two facts a readiness route cannot otherwise compute,
and the distinction:

- **Liveness** — the process is up and its scheduler is not wedged. Answerable
  with an empty row. `desk.ply`'s `health()` is already exactly this, and its
  row being `{}` is the proof that it cannot be made to fail by a database
  outage.
- **Readiness** — this instance should receive traffic. Two conditions:
  `!signal.stopping()`, and the peers are reachable. The second is one
  `db.query[t](stmt("select 1"), [])` and the program writes it, so a readiness
  route's row says exactly what it checks:

```ply
pub fn ready() -> http::Response / {signal.read, db.read[items]}
```

That row is the answer to "what does readiness actually verify", and it is
inferred rather than documented. A readiness route whose row is `{}` is a
readiness route that checks nothing, and `ply check --types` says so.

Named because it is the mistake: a readiness probe that reaches the database on
every poll is a load generator with a two-second period. The poll interval is
the operator's; what W5 owes is that the check is one statement on a pooled
connection and that its cost is visible in the row.

### 6.2 Structured output

`ply_host::trace::json` writes **one JSON object per line to stderr**:

```json
{"ts":1755230417331942,"level":"info","channel":"orders","kind":"exit","name":"place_order","span":41,"parent":12,"outcome":"ok","micros":8213,"fields":{"customer":"ada","total":"41.75"}}
```

Five rules, each of which is a required test:

- **stderr, never stdout.** Every `ply` command's `--json` owns stdout and emits
  exactly one document; a trace line interleaved into it destroys the document.
- **The program's fields are nested under `fields`**, always, even when empty.
  A field named `level`, `ts` or `span` therefore cannot shadow the envelope,
  and a program cannot forge a level by naming a field.
- **`ts` is epoch microseconds, an integer.** Not RFC 3339. Ply has no time type
  (ADR 0014 §4.2) and a `timestamptz` is already stored as `int8` microseconds by
  a program's own schema, so this is the same representation; and a calendar
  formatter in a trusted computing base is a dependency and a locale bug for a
  field every consumer re-formats anyway. `--trace text` renders `+412.3ms` from
  the run's start, which needs no calendar either.
- **A `Secret` cannot appear**, because a `Field` cannot hold one (§2.3). There
  is no redaction pass in the JSON writer, deliberately: a redaction pass is what
  W5 is replacing, and having one would invite someone to rely on it.
- **The line is a single write**, so two tasks cannot interleave one line.

`--trace <json|text|off>` selects the sink; `--trace-level <debug|info|warn|error>`
filters in the sink (§1.4). `off` is `ply_host::trace::discard`, a listed
handler, not an empty registry.

### 6.3 What `ply` prints when a service starts

```
$ ply run examples/desk.ply --host --db postgres://desk@localhost/desk \
      --tls desk=certs/desk.pem,certs/desk.key --config-schema desk.config
   desk.run · ply 0.13.0 · program b3:91af0c33d7e2
   hosts       12 handlers · 19 operations · digest b3:4f19c0a8e2d3
   database    PostgreSQL 18.3 · desk · collation C · pool 8 · schema desk.schema verified
   config      6 keys · 4 environment · 1 --set · 1 default · 2 secrets (values not shown)
   trace       json → stderr · level info · channels db, http, items, orders
   shutdown    signals INT TERM · lead 0ms · drain 30000ms
   listening   0.0.0.0:8137 · tls desk · http/1.1
```

Every line is a fact the run already holds: the digests are computed anyway, the
pool and schema lines are ADR 0014 §11's `ply hosts` block, the channel list is
the resolved resource labels of the bound `trace` registration, and the config
line is §3.3's snapshot counted by source. **Nothing is computed for the
banner.** Secret values are absent, secret *keys* and their sources are in
`ply hosts --json` because an operator debugging "it used the wrong credential"
needs to know which source won.

`--json` on `ply run` emits the same as one object before the entry point starts,
and the run's trace lines follow on stderr.

### 6.4 What it prints when it stops

```
   ^C
   stopping    drain 30000ms · 1 connection in flight · 0 transactions open
   drained     1 connection · 0 abandoned · 412ms
   database    8 connections closed · 0 rolled back at teardown
   trace       1284 events · 96 spans · 0 abandoned · flushed
   desk.run    exit 0 · served 10429 requests · 4m12s
```

and the failure shape, which is the one that matters:

```
   stopping    drain 30000ms · 14 connections in flight · 3 transactions open
   warning[W0608]: the drain deadline expired
     = 6 connections abandoned with no response written
     = 3 transactions rolled back at teardown; nothing was committed
     = raise `--drain-ms` above the program's body_timeout_ms + write_timeout_ms
   desk.run    exit 3 · served 10429 requests · 4m42s
```

Exit `3` (`EXIT_DRAIN_INCOMPLETE`) rather than `0`, because a deployment must be
able to tell a clean stop from one that dropped requests, and because a rolling
restart that reports success while losing six requests per instance is the
failure this section exists to make visible.

### 6.5 `ply hosts`

Three additions, and the reasoning is ADR 0014 §11's unchanged: a fact the rows
cannot carry and a reviewer must not have to derive.

```
$ ply hosts --host
   12 host handlers · 19 operations · trusted computing base

   OPERATION              ATOM                  HANDLER                    DET  LINEAR        BLOCKING  SECRET
   config.get[database]   config.read[database] ply_host::config::get      no   repeatable    no        no
   config.secret[creds]   config.read[creds]    ply_host::config::secret   no   repeatable    no        no
   signal.stopping        signal.read           ply_host::signal::stopping no   repeatable    no        no
   trace.enter[orders]    trace.write[orders]   ply_host::trace::enter     no   at-most-once  no        no
   trace.event[orders]    trace.write[orders]   ply_host::trace::event     no   at-most-once  no        no
   ...

   configuration
   sources    --set 1 · --config 0 files · environment 217 · defaults 1
   schema     desk.config · 6 keys · 6 resolved · 2 secret
   keys       DESK_PORT=8137 (env) · DESK_API_KEY=**** (env) · DESK_REGION=eu (--set) · ...

   observability
   sink       ply_host::trace::json → stderr · level info
   channels   db http items orders
   spans      per-task stack · closed at end_entry_point

   shutdown
   signals    INT TERM · lead 0ms · drain 30000ms · second signal exits 130/143

   digest: b3:4f19c0a8e2d3
```

- `SECRET` is a new column on every row (§2.4), and it is in the digest: a
  handler that starts accepting credentials is a structural change to the
  trusted computing base and CI should break on it.
- The **`keys` line prints values for non-secret keys and `****` for secret
  ones**, with the winning source beside each. That is the answer to "it
  connected to the wrong thing", and it is the reason §3.2's precedence is worth
  writing down.
- The digest covers the operation rows including `SECRET`, the config *schema*
  function's name and its key names and shapes, the sink's handler path, the
  channel list, and the shutdown knobs. It does **not** cover resolved config
  *values*, the environment's variable count, or the server version — the ADR
  0014 §11 rule, that a CI check which breaks on a deployment's own
  configuration is a CI check people learn to ignore.

`trace.*` is `Linearity::AtMostOnce`: replaying a continuation across an event
writes the event twice, and a duplicated span in a log is a wrong answer about
what happened. `config.*` and `signal.*` are `Repeatable` — reading a frozen map
twice is the definition of harmless.

---

## 7. The three new shared states, and W4's lesson

W4 found that a pooled connection coupled two tests the footprint graph believed
were disjoint, and that the scheduler could not prevent it. W5 adds three more
pieces of shared host state and each is a fresh chance to repeat that defect, so
each gets an explicit account rather than a hope.

| state | how it could couple two tests | what W5 does about it |
| --- | --- | --- |
| **the trace sink** | one process-wide sink; a test asserting on collected records sees another's | the atom is `trace.write[c]` — a **write**, per channel — so two tests recording on one channel conflict and are serialised by the existing conflict graph. A test that installs the twin per test discharges the atom entirely and is coupled to nothing. **The defect to avoid** is one `with_cell` around the whole suite, which is the pooled connection in a new costume |
| **the sink's span-id counter** | one counter per driver; a program's own `Span.id`, and its `E0445` text, move with what a footprint-disjoint entry point traced | the counter and `E0445`'s classification are **per `MachineId`**. The channel argument above covers the *records* and does not cover the counter, which is shared across channels — that gap was found by audit after this section was written, and it produced verdicts that differed between `--jobs 1` and `--jobs 8` and were not stable across runs at `--jobs 8`. The rule the fix follows is the one W4 should have followed: give the shared state the identity that should have scoped it, rather than serialising the tests that expose it |
| **the config snapshot** | a test's `setenv` seen by another; a value read twice differing | §3.3: the environment is read **once**, at bind time, into an immutable map, and never consulted again. That is what makes `config.read[k]` honestly a read, which is what makes two readers non-conflicting, which is what makes the conflict graph right. One line of implementation carrying three properties |
| **the stop flag** | a stop requested once ends every test after it, and the suite's own `ctrl-C` decides verdicts | §4.7: `signal` **does not bind under `ply test`**, with or without `--host`. `E0424` names the twin |

And the sentence that has to be repeated because it is the one a reader
forgets: **ADR 0008 §6 makes footprint conflict grouping the only isolation a
host-backed test has**, so each of these is exactly as isolated as its
registration's mode and resource, and nothing checks either. `ply hosts` and
review are the whole of the defence. W5's registrations are `trace.write[c]`
(write, per channel), `config.read[k]` (read, per namespace) and nothing at all
for `signal`, and those three choices are the substance of this table.

Every test that reaches any of them is `Isolation::Host`: counted separately,
excluded from `isolated: n of m`, never cached, never bisected. W5 adds no case
to that machinery.

---

## 8. New diagnostic codes

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0439 | `SECRET_TO_HOST` | a host operation whose registration says `secrets: false` was handed a value containing a `Secret` | **Ply's** |
| E0440 | `CONFIG_UNAVAILABLE` | `--config` names a file that is unreadable, or a line that is not `KEY=VALUE`; `--set` that is not `KEY=VALUE` | the run's configuration |
| E0441 | `CONFIG_MISSING` | a `--config-schema` key marked `required` that no source supplies | the run's configuration |
| E0442 | `CONFIG_INVALID` | a resolved value that does not satisfy its declared `Shape` | the run's configuration |
| E0443 | `ARTIFACT_INVALID` | a `.plyx` whose header, digest, section table, body hash or reference closure does not verify | the run's configuration |
| E0444 | `ARTIFACT_VERSION` | a `.plyx` built under a different `FRONTEND_VERSION`, `RUNTIME_VERSION` or `BODY_ENCODING` | the run's configuration |
| E0445 | `SPAN_UNBALANCED` | `trace.exit` naming a span that is not open on the performing task's stack | the program's |
| W0607 | `CONFIG_UNDECLARED` | a `--set` or `--config` key the schema does not declare | the run's configuration |
| W0608 | `DRAIN_INCOMPLETE` | the drain deadline expired with connections still in flight | the run's configuration |
| W0609 | `SPAN_ABANDONED` | spans were still open when an entry point ended | the program's |

**Reserved.** E0439 joins `E0427`'s row — the machine's own verdict about the
boundary, `Status::Panicked`, `Failure::defect` true, never bisected. E0440,
E0441 and E0442 are raised by `HostRegistry::bind` before anything runs, like
E0421–E0423 and E0431/E0435/E0438, and join `RESERVED_CODES` for the same
reason. E0443 and E0444 are raised by the artifact loader before any binding
exists, and join it too.

**E0445 does not.** It is a refusal the trace driver is the only component in a
position to compute — which task holds which span — and reserving it would have
`attribute` rewrite the driver's own diagnosis to `E0502` and send a reader
looking for a defect in Ply. This is ADR 0014 §8's rule unchanged, and E0445
belongs with E0432–E0434, E0436 and E0437.

E0445 and W0609 are attributed and bisected like any other program failure.
W0608 and W0606 are run-level and change no verdict.

`RESERVED_CODES` grows from 18 to 24.

---

## 9. Versions

| constant | to | why |
| --- | --- | --- |
| `RUNTIME_VERSION` | `0.11.1` | `Value::Secret`, three builtins, `render` and `values_equal` and `compare_values` changing behaviour on a variant, `HostRuntime::shutdown` and `stopping`, `HostOp::secrets`, `end_entry_point` closing spans. A cached `Pass` is a claim about what the evaluator did. `0.11.1` completes it: the map key gate refuses a `Secret` where `map_of_entries` and `map_merge` used to order one, and a span id is minted per entry point rather than per run |
| `FRONTEND_VERSION` | `0.14.0` | `Secret` in `BUILTIN_TYPE_CONS`, so a project's `type Secret` becomes reserved; `derivable(json/ord/row, Secret<a>)` false and `derivable(eq, ·)` true, which changes what `E0206` fires on; `E0418` for a `Secret` binder. `0.14.0` adds §2.7: an operation's type variables are rigid where its handler clause is checked, so a clause answering a concrete type for an operation declared `-> a` is `E0201` where it used to be accepted, and a cached interface written before that is an interface for a program this front end refuses |
| `BODY_ENCODING` | **stays at `7`** | no new normalization tag. A `Secret<String>` in a declared signature is `Type::Con(Symbol, Vec<Type>)`, which already encodes by name |
| `PROVER_VERSION` | **stays at `0.5.0`** | no existing obligation's discharge can change: `Secret` is a new type, so no law could have mentioned it, and the only new prover behaviour is a refusal at a binder that could not previously be written |
| `FRONTEND_FORMAT` | unchanged | the store's shapes do not move |
| `ARTIFACT_FORMAT` | `1` | new |

**`BODY_ENCODING` staying is a required test, not an observation**: the whole W4
corpus normalizes byte-for-byte identically under W5, and the front-end cache
written by a W4 binary is discarded on the `FRONTEND_VERSION` bump while the
**result cache is untouched**, so no test re-runs for a reason other than a
source edit.

---

## 10. Workspace

```toml
tokio = { version = "1.53.1", features = ["rt", "net", "time", "sync", "signal"] }
```

**One feature, and no new crate.** That is the whole dependency change in W5, and
it is worth saying why each candidate was refused:

- **`signal-hook`** — `tokio` is already in `ply-host` and already owns a
  current-thread reactor on one OS thread (ADR 0014 §3.1). `tokio::signal`
  registers there. A second signal mechanism in a trusted computing base is two
  things that can both claim `SIGTERM`.
- **`tracing` / `tracing-subscriber` / `opentelemetry`** — the sink is fifty
  lines of JSON writing, the effect is the interface, and a subscriber
  ecosystem's whole value is the ambient dispatch this milestone exists to
  remove. Adopting one would mean two notions of a span, one of which is not in
  any row.
- **`toml` / `serde_yaml` / `figment`** — §3.2. The format is `KEY=VALUE`
  because the effect returns `Option<String>`.
- **`chrono` / `time` / `jiff`** — §6.2. `ts` is epoch microseconds and the run
  computes it from `SystemTime` in two lines.
- **`zeroize`** — §2.5 (6). It would zero one `Arc<str>` while the evaluator
  copies values freely, which is a promise the runtime cannot keep and a badge
  it should not wear.

`ply-std` gains three modules (`std.trace`, `std.config`, `std.signal`) and no
dependency. `ply-host` gains `trace.rs`, `config.rs`, `signal.rs`. `ply-cli`
gains `commands/build.rs` and `artifact.rs`. `ply-eval` gains a `Value` variant,
three builtins and two `HostRuntime` methods, and **no dependency**.

---

## 11. Changes to `examples/desk.ply`

The example is the milestone's evidence, so its changes are part of the
contract:

- **`effect log` is deleted** and its two call sites become
  `trace.event[orders]` and `trace.event[items]`. `effect set Desk` loses
  `log.write` and gains `trace.write[orders]` and `trace.write[items]`. This
  moves the hashes of every definition annotated with `Desk` and everything
  reaching them, which is correct — the signature changed and selection is exact
  about it — and the required test asserts that the definitions *not* reaching
  `trace` are untouched.
- **A readiness route** `ready()` with the row `{signal.read, db.read[items]}`,
  and `health()` unchanged with the row `{}`. Two routes, two rows, and the
  difference between them is §6.1.
- **An API-key check** on `POST /orders`, reading `config.secret[credentials]`
  at start-up and comparing with `secret_verify`. This is the only place in the
  repository where a `Secret<a>` is used end to end, and it is what test 14
  below is written against.
- **`run_memory` gains six `trace` clauses, two `config` clauses and one
  `signal` clause** over three region-scoped cells, and stays hermetic: its row
  is still `{net.write[conn], net.write[listener]}` and its tests are still
  `det`, cached and runnable without `--host`.
- **Not changed**: `serve`, `serve_connection`, the framing, the route table, or
  any endpoint's behaviour. §4.3 is the claim and it is checked by test 20.

---

## 12. Required tests

The ones whose absence would let W5 ship broken rather than merely incomplete.

**Observability**

1. A definition performing `trace.event[orders]` publishes `trace.write[orders]`
   and one performing `trace.event[items]` publishes `trace.write[items]`;
   `ply check --types` prints both with no flag, and the two do **not** conflict
   in the concurrency graph while two on one channel do.
2. A `det` test reaching an unhandled `trace` operation is `E0412` at compile
   time, with `--host` and without it.
3. A twin-backed tracing test's row is empty, it is `det`, it is cached, and it
   runs without `--host`; its second run is a cache hit.
4. A `db.rollback` inside a span leaves that span `Abandoned` in `drain`'s
   output, with every record before it intact and the enclosing span closed `Ok`.
5. A raise inside a span leaves it `Abandoned`; the entry point's verdict is the
   raise, and `W0609` names the innermost span.
6. `trace.exit` naming a span that is closed, never opened, or opened by another
   task is `E0445` naming both tasks in the third case.
7. Two tasks interleaving `enter`/`exit` under one channel produce correctly
   nested `parent` links, verified against a reference computed from the record
   list.
8. **The cost property**, by a counting harness and never a stopwatch: N events
   under `ply_host::trace::discard` perform exactly N host operations, allocate
   exactly N `Fields` maps, call `clock.now()` **zero** times, and format zero
   strings. The same under `--trace-level warn` for `Debug` events.
9. A published benchmark reports the per-event and per-span cost under
   `discard`, under `json`, and under the twin.

**Secrets — the headline, and every route in §2.3**

10. `derive json for` a record with a `Secret<String>` field is `E0206` naming
    the field; so are `derive row` and `derive ord`; `derive eq` succeeds.
11. `Secret<String> ++ String`, a `Secret` in a `Field`, in a `Param`, in
    `bytes_of_string`, in `panic`, and as a `Map` key are each a compile error,
    one test per route, asserting the code.
12. `match s { Secret(x) -> x }` is `E0101`; there is no pattern that binds the
    payload.
13. A failing `assert_eq` over a record holding a `Secret` prints
    `Secret(****)` on both sides, and the same bytes appear in `--json`, in the
    result cache's failure report, and in `ply test --explain`.
14. End to end on `desk.ply`: a request with the right key is accepted and one
    with the wrong key is refused; the whole run's stderr, its `--json`, its
    `.ply-cache` directory and its `frontend.dat` are searched for the
    credential's bytes and it appears in **none** of them.
15. `forall (s: Secret<String>)` is `E0418`.
16. `compare_values` on a `Secret` is `E0502` naming `secret_verify`; `==` on
    two equal secrets is `true` and on a `Secret` and a `String` is a type error.
17. A host operation registered `secrets: false` handed a `Secret` is `E0439`,
    `Status::Panicked`, not bisected; the `SECRET` column appears in
    `ply hosts` and changing it alone moves the digest.
18. `secret_verify` compares in constant time: a harness over mismatches at
    increasing positions shows no monotone step count.

**Configuration**

19. Precedence: one key supplied by all four sources resolves to `--set`;
    removing sources in order walks it down to the default.
20. The environment is read **once**: a `setenv` performed between two
    `config.get` calls in one run does not change the second's answer.
21. `config.read[k]` never conflicts: two tests reading configuration are in one
    concurrency group and run concurrently.
22. `--config` with an unreadable file, a line without `=`, an empty key and a
    non-identifier key are each `E0440` naming the file and line.
23. A `required` key nothing supplies is `E0441` at bind time, naming the key and
    the four sources; a `SInt` key holding `"eight"` is `E0442`, and a
    `SSecret` key that is malformed is `E0442` **without printing the value**.
24. A `--set` key the schema does not declare is `W0607`; an *environment* key
    the schema does not declare is **not**.
25. `config.get` on a key the schema declares `SSecret` answers `None`, and
    `config.secret` on a non-secret key answers `None`.
26. A test supplying configuration by handling `config.*` is `det`, cached and
    hermetic; the same test without `--host` and without the handler is `E0424`
    naming `ply_host::config::get`.

**Shutdown**

27. `desk.ply` drains with **no source change**: a signal during a request lets
    that request complete, `accept` answers `0`, `serve` returns, the listener
    closes and the exit code is `0`.
28. A transaction open at the deadline is **rolled back and never committed**,
    asserted against `pg_stat_activity` and against the table's contents, not
    against the driver's bookkeeping.
29. Teardown order is the pinned one: a trace record naming the rollback is
    written before the pool closes, asserted from the captured stderr.
30. A second signal exits `130`/`143` immediately, after printing what was
    abandoned.
31. A drain that expires reports `W0608` with the abandoned count and exits `3`;
    one that completes exits `0`.
32. An **idle** service with no traffic and no outstanding token observes a
    signal and exits: `park` returns on `stopping()`, and the deadlock check does
    not report `E0414`.
33. `signal.stopping()` under `ply test --host` is `E0424` naming the twin;
    under `ply run --host` it works.
34. `--drain-lead-ms 2000` keeps accepting for two seconds after the signal while
    `signal.stopping()` already answers `true`, so a readiness route can answer
    `503` before accept stops.

**Deployment**

35. `ply build` twice from two different absolute roots, one cold cache and one
    warm, produces **byte-identical** artifacts and the same digest.
36. `ply run desk.plyx --host` serves the same responses as
    `ply run desk.ply --host`, byte for byte, over the full route table.
37. A flipped bit in one body is `E0443` naming that definition's hash and
    offset; a truncated file is `E0443`; a body referring to a hash the artifact
    lacks is `E0443`.
38. An artifact built under a different `BODY_ENCODING` is `E0444` and not
    `E0443`; a differing `ply_std::digest()` is `W0605` and the run proceeds.
39. An artifact contains no `test`, no `law` and no `Span` other than
    `Span::DUMMY`, asserted by walking the decoded definitions; `--sources`
    changes the digest.
40. `ply build --diff` over an artifact pair reports added, changed, dropped and
    unchanged counts that agree with `ply hash` over the two trees, and the
    reached-endpoint list agrees with the reverse closure.
41. The artifact's size and the `ply` binary's size are both printed, so §5.1's
    decision has a number attached.

**What an operator sees**

42. A trace line goes to stderr and `ply run --json`'s document on stdout parses
    with trace output interleaved.
43. A program field named `level`, `ts`, `span` or `channel` appears under
    `fields` and does not shadow the envelope.
44. `--trace off` binds `ply_host::trace::discard`, `ply hosts` lists it, and
    reaching `trace` with an **empty** registry is `E0424` rather than silence.
45. The start-up banner's every number matches the corresponding
    `ply hosts --json` field; the shutdown banner's counts match the run's own
    totals.
46. `ply hosts --digest` changes when the `SECRET` column, a config key name, a
    config shape, the sink path, the channel list or a shutdown knob changes, and
    does **not** change when a resolved config value, the environment's size or
    the server version changes.

**Everything W5 must not regress**

47. Renaming a top-level function selects zero tests; moving a definition between
    modules changes no hash — on a corpus with `trace`, `config`, `signal` and
    `Secret` rows.
48. Incremental and `--no-incremental` agree byte-for-byte across the full
    mutation sequence, with `Secret`-typed signatures and channel-label edits
    added.
49. `E0412` still fires for an unsimulated nondeterministic effect in a `det`
    test; `ply test` is hermetic without `--host` and says so.
50. Bisection names the correct culprit on a W5 corpus; `--engine both` reports
    no `E0503`, and a `Secret` round-trips identically on both engines.
51. A seeded simulation replays exactly; a check-then-act race between two
    requests is found against the twin, reported with a seed and replayed —
    with tracing installed, so the twin's `Sink` is inside the forked world and
    two interleavings do not share records.
52. `ply prove` reports honest tiers and `ply hosts` lists the TCB, on the W5
    corpus, with the `configuration`, `observability` and `shutdown` blocks
    present under `--host`.
53. An effect-set alias and its explicit expansion hash identically, on a corpus
    whose alias contains `trace` and `config` atoms.
54. `Store::open` at 10,000 definitions stays under 5 ms.
55. **No definition's normalized bytes moved** across the W5 change, over the
    whole W4 corpus — `BODY_ENCODING` stays at `7` and this is what proves it.

Plus one `tests/fixtures/` entry per new code, as every milestone owes.

---

## Not in W5

- **Metrics backends.** No Prometheus exposition, no OTLP, no StatsD, no
  push gateway, no histogram bucketing. `trace.count`, `gauge` and `time` are
  records in a sink, and turning them into a time series is a consumer's job.
- **Log shipping.** No file rotation, no syslog, no network sink, no batching.
  One JSON object per line on stderr, and the process supervisor owns the rest.
- **Orchestration and autoscaling.** No container image, no manifest, no
  service discovery, no rolling-restart controller, no replica count.
- **Distributed tracing propagation.** No W3C trace context, no `traceparent`
  parsing, no span export. W3 has no HTTP client, so there is no outbound
  context to propagate; an inbound `traceparent` is a header, which is data, and
  a program that wants it reads it and passes it as a field.
- **Sampling.** The sink drops by level and by nothing else. A sampler is a
  policy, and a policy that silently discards is the thing this project audits
  for.
- **Cancellation.** Still. §4.5 states exactly what that costs at the drain
  deadline, and it is the largest gap in the milestone for the second time.
- **Live configuration reload.** §3.6.
- **Incremental deploy transport.** §5.1, with the measurement that would
  re-open it.
- **Artifact signing.** §5.5.
- **Zeroization, and any memory-level guarantee about a `Secret`.** §2.5 (6).
- **A `Secret` that survives concatenation, transformation or partial
  disclosure.** There is no `secret_map`, no `secret_concat` and no
  `secret_slice`, deliberately: every one of them would need to see the
  plaintext, and a function that sees the plaintext is a function that can
  return it.
- **Rate limiting, backpressure and load shedding.** ADR 0014 §3.2 said W5 owns
  backpressure and W5 does not: `E0437 DB_POOL_EXHAUSTED` is still a diagnostic
  rather than a shed request. Turning it into a `503` needs a policy about which
  requests to refuse and a way to refuse one without ending the run, and neither
  is here. **This is a promise W4 made that W5 is breaking, and it is stated
  rather than quietly dropped.**
