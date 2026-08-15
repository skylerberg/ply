# ADR 0014 — The W4 implementation contract

Status: accepted

ADR 0008 settled what a host handler is; ADR 0013 built the server that will sit
in front of a database. This one settles the database: the `db` effect, the
transaction, the pool, the statement, the in-memory twin, and the law that says
the twin and postgres agree. Where 0008, 0011, 0012 or 0013 disagree with this,
this wins — it was written after them, against the code.

## The rule everything else follows from

> **A row says which tables.** The reason to put a database behind an effect is
> that an endpoint's declared signature names the tables it touches, and a
> driver that answers `db.write[db]` for every statement has thrown that away
> and kept only the ceremony. So the resource label is a table name, one
> statement's table set is computed rather than asserted, and every mechanism
> below exists to keep the row honest when the thing that decides it — the SQL
> text — is a runtime value rather than a piece of syntax.

Five corollaries, each of which decides a section:

1. **A statement's footprint is a function of the statement, not of the call
   site.** The call site writes one label; the statement may touch more tables
   than that. The gap is closed by making the driver *report* what it touched
   and having the machine check the report, rather than by trusting the label
   (§2).
2. **A rollback is the absence of a resumption.** M6 gave handlers a reified
   continuation; a handler clause that returns without resuming discards the
   rest of the body. ADR 0008 §7 caps a host handler at one resumption, and zero
   is inside that cap rather than an exception to it (§1).
3. **A shared connection is not forkable, and pretending otherwise is the
   silent failure.** ADR 0008 §6 already says host effects are outside the
   forkable world; a pool makes that concrete and W4 states exactly what
   isolation a host-backed test therefore has, which is less than a reader
   expects (§3).
4. **A value never becomes syntax.** Every parameter crosses the wire in the
   extended protocol's `Bind`, typed. There is no operation anywhere in W4 that
   splices a value into statement text (§4).
5. **A twin that silently diverges is worse than no twin.** So the twin refuses
   loudly what it does not model, and the agreement law is a deliverable equal
   to the driver rather than a demonstration attached to it (§5, §6).

---

## 0. `std.db` — the declaration

`crates/ply-std/ply/db.ply`, the module `std.db`, program-wide effect name
`std.db.db`. It is Ply source and it ships with the compiler, exactly as
`std.net` does and for the same reason: the signature the driver binds against
and the signature the program performs are one text that cannot drift.

```ply
pub nondet effect db {
  read  query[t](s: Stmt, ps: List<Param>)     -> Answer
  write execute[t](s: Stmt, ps: List<Param>)   -> Answer
  write returning[t](s: Stmt, ps: List<Param>) -> Answer

  write begin(level: Isolation, access: Access) -> Answer
  write commit()                                -> Answer
  write abort()                                 -> Answer
}
```

`nondet` is load-bearing and it is the same sentence `std.net` carries: a
database's contents are not a function of the program's state, so a `det` test
that reaches an unhandled `db` operation is `E0412` at compile time whether or
not `--host` was passed. The twin (§5) discharges the atoms, which is what makes
a twin-backed test `det`, cacheable and hermetic.

```ply
pub type Isolation = ReadCommitted | RepeatableRead | Serializable
pub type Access    = ReadWrite | ReadOnly

pub type Stmt = { sql: String }
pub fn stmt(sql: String) -> Stmt = { sql: sql }

pub type Param =
  | PNull
  | PInt(Int) | PBool(Bool) | PText(String) | PBytes(Bytes)
  | PFloat(Float) | PNumeric(Decimal) | PJson(json::Json)
  | PArray(List<Param>)

pub type Cell =
  | CNull
  | CInt(Int) | CBool(Bool) | CText(String) | CBytes(Bytes)
  | CFloat(Float) | CNumeric(Decimal) | CJson(json::Json)
  | CArray(List<Cell>)

// Column name to value. A `Map` (ADR 0012 §2), so a row has one canonical form,
// two rows built in different column orders are `values_equal`, and a golden
// test over a result set is stable.
pub type Row = Map<String, Cell>

// The SQLSTATE the server returned, and the object it named. **The message is
// not here.** It is postgres's prose, it moves between server versions and
// locales, and a twin that had to reproduce it would be reproducing English
// rather than behaviour. `detail` carries it for a human and nothing compares
// it — §6 is where that matters.
pub type DbError = {
  code:       String,        // SQLSTATE, five characters, e.g. "23505"
  constraint: String,        // the constraint or relation named, or ""
  detail:     String,        // for a person; never compared
}

pub type Answer =
  | Rows(List<Row>)          // `query` and `returning`
  | Count(Int)               // `execute`: rows affected
  | Failed(DbError)
```

**A SQLSTATE is a value, never a diagnostic.** A unique violation, a
foreign-key violation, a serialization failure, a connection that died
mid-statement — all of them are `Failed(e)` that the program matches on. This is
ADR 0013 §7.1's rule applied to a second peer: a server that dies because a row
already existed is not a server, and a language that turned every constraint
into a compiler-shaped failure would make the constraint unusable as a
concurrency control. The shapes that *are* diagnostics are enumerated in §8, and
every one of them is a claim the program or the run got wrong before any row
moved.

`Answer` is one type across the three data operations rather than three, because
the twin, the driver and the law all pattern-match the same shape, and a
`returning` that a schema change turned into a plain `execute` should be a
mismatch the program reads rather than a type error at a call site that did not
move.

---

## 1. Transactions as handlers

### 1.1 The shape

```ply
pub type Rollback = { reason: String, error: Option<DbError> }

pub fn transaction<a, e>(
  level: Isolation, access: Access, body: () -> a / {db.write | e}
) -> Result<a, Rollback> / {db.write | e} =
  handle {
    match db.begin(level, access) {
      Failed(err) -> Err({reason: "begin", error: Some(err)}),
      _ -> {
        let value = body();
        match db.commit() {
          Failed(err) -> Err({reason: "commit", error: Some(err)}),
          _ -> Ok(value),
        }
      },
    }
  } with {
    db.rollback(reason) -> {
      db.abort();
      Err({reason: reason, error: None})
    },
  }
```

with one more operation on the effect, which is the whole mechanism:

```ply
  write rollback(reason: String) -> Unit
```

`db.rollback` is performed anywhere inside the body, arbitrarily deep. Its
clause **does not resume**. The value of the clause is the value of the whole
`handle`, so everything the body had left to do — the rest of the function, its
callers up to the `transaction`, the statements it was about to issue — is the
continuation, and the continuation is dropped on the floor. Nothing unwinds,
nothing is caught, no frame runs an epilogue. That is what M6 bought and this is
the first place in the language where discarding a continuation is the *point*
rather than a capability.

Note what does not appear: a clause per table. `transaction` intercepts
`db.rollback` and nothing else. The data operations pass straight through to
whatever handler is below — the twin, or the host binding — and the driver
routes them onto the open scope's connection because the scope is host-side
state (§1.3). A Ply handler clause names a concrete `(operation, resource)`
pair, so a transaction that intercepted the data operations would need one
clause per table per operation and could not be a library function at all. It
does not need to: a transaction is a *scope*, and the only thing that must be
scoped in Ply is the abort.

### 1.2 Linearity, and why zero resumptions is inside the rule

ADR 0008 §7 caps a host handler's continuation at one resumption because
resuming twice performs the I/O twice. `db.rollback`'s clause resumes **zero**
times, which is `resumes = 0` and trivially satisfies `resumes <= 1`. The
enforcement in `handler::resume` refuses on `resumes > 1 && host_ops > born`,
and zero never reaches it. So rollback needs no exemption, no new predicate and
no change to the check — which is the strongest evidence available that ADR
0008 §7 was the right restriction rather than a convenient one.

The interaction that *is* new: `db.begin` is `Linearity::AtMostOnce`, so any
continuation captured before a transaction opened cannot be resumed a second
time (`E0426`). A program that wanted to replay a transaction body through a
multi-shot handler is refused, correctly — the replay would issue a second
`BEGIN` on a connection already inside one.

### 1.3 Commit, and what closes an abandoned scope

The driver keeps a **scope stack per task**, in `ply_host::db`. `begin` pushes;
`commit` and `abort` pop. Every `query` / `execute` / `returning` runs on the
innermost open scope's connection; with no open scope it acquires a connection,
runs the statement in postgres's implicit transaction, and releases it.

Four exits, and all four are specified because three of them are the ones a real
system gets wrong:

| exit | what happens |
| --- | --- |
| `commit()` | `COMMIT`. A failure — a deferred constraint, a serialization failure at commit — is `Failed(e)` and the scope is closed and rolled back |
| `db.rollback(r)` | the clause discards the continuation and calls `abort()`, which issues `ROLLBACK` |
| the body **raises** — `E0501`, `E0502`, a step-budget exhaustion | the raise propagates unchanged, past the `handle`, past `transaction`; nothing was committed. The scope is still open |
| the entry point ends with a scope open | the driver rolls it back at teardown |

That last row is the one that needs a mechanism rather than an intention.
`HostRuntime` gains

```rust
    /// Called by the machine on **every** exit path from an entry point — a
    /// value, a diagnostic, or a spent budget — before the machine resets.
    /// The driver rolls back every scope still open and releases or discards
    /// the connections holding them.
    fn end_entry_point(&self) -> Result<(), Diagnostic>;
```

and `ply_host::db`'s pool manager is the second lock: a connection returned with
a scope the driver believes is open is `ROLLBACK`ed on release, and one whose
`ROLLBACK` fails or whose session state cannot be established is **closed and
discarded rather than returned to the pool**. A connection recycled with an open
transaction is the failure that makes the *next* request read uncommitted rows
of a request that already failed, and it is invisible from either request.

`end_entry_point` failing is not the program's fault and does not change the
entry point's verdict; it is reported as a run-level warning with the
connections it discarded, and the pool refills.

### 1.4 Nesting is a savepoint

`transaction` inside a `transaction`, lexically or through a call, is a
**savepoint** rather than a refusal. `begin` on a non-empty stack issues
`SAVEPOINT ply_sp_<depth>`; `commit` issues `RELEASE SAVEPOINT ply_sp_<depth>`;
`abort` issues `ROLLBACK TO SAVEPOINT ply_sp_<depth>` followed by `RELEASE`.
Depth is bounded at `db_max_savepoints` (default 16) and exceeding it is
`Failed` with SQLSTATE `54000`, not a diagnostic — it is a program that recursed
and the program is what must stop.

Refusal was the alternative and it is worse. A nested `transaction` is what a
helper function looks like when it is called both standalone and from inside a
larger operation, which is the ordinary case rather than an exotic one; refusing
it would mean every such helper existed twice, and two copies of a write path is
the drift this milestone exists to measure.

`level` and `access` on a nested scope are **ignored**, because a savepoint has
neither. That is a silent difference between what a call site says and what
happens, so it is not silent: a nested `begin` whose `level` differs from the
open scope's is `Failed` with SQLSTATE `25001` and the message names both
levels. A nested `ReadOnly` inside a `ReadWrite` is accepted (it is a
narrowing the program may usefully write), and its statements are still
writable — postgres has no read-only savepoint — so the ADR states that the
narrowing is documentation and not enforcement, which is the only honest thing
to say about it.

### 1.5 Isolation levels

`ReadCommitted` (postgres's default), `RepeatableRead`, `Serializable`. Set with
`SET TRANSACTION ISOLATION LEVEL` at `BEGIN`.

`ReadUncommitted` is **not offered**. Postgres implements it as read committed,
so a name in Ply's source that promised dirty reads would be a name that lies,
and this project's whole posture is that a label is a truth claim.

A serialization failure (`40001`) or a deadlock (`40P01`) is `Failed(e)` and
**W4 does not retry**. Retrying means re-running the body, and only the program
knows whether the body sent an email, charged a card or wrote a file between two
statements. `std.db` provides

```ply
pub fn is_retryable(e: DbError) -> Bool   // 40001, 40P01
```

so the decision is one `if` at a site that can see what it is repeating. A
retry is a fresh call to `transaction`, not a second resumption, so it is
outside §1.2's rule entirely and needs no exemption from it.

`access = ReadOnly` issues `SET TRANSACTION READ ONLY`, and a write inside it is
`Failed` with `25006` **from the server**. That is a mechanical backstop on a
row that claims to be read-only, supplied by the one component in the stack that
cannot be fooled by an annotation, and it costs nothing.

### 1.6 A transaction and a task

A scope belongs to the task that opened it. A `db` operation performed by a task
that is not the scope's owner, while that scope is open, is **`E0436
DB_TRANSACTION_SCOPE`**. The two answers it prevents are both wrong: sharing the
connection is a protocol violation (a postgres connection carries one
conversation), and quietly acquiring a second connection would put the statement
*outside* the transaction its author believed it was in.

`HostRequest` therefore gains the performing task:

```rust
    /// The task that performed this operation. `None` outside a scheduler
    /// region, which is one identity rather than an absence of one.
    pub task: Option<TaskId>,
```

The complementary refusal already exists: a host operation inside a `simulate`
region, or in the prefix or suffix around one, is `E0425` (ADR 0008,
CONTRACTS "Host boundary"). So a transaction is never explored by DPOR against a
real database. It is explored against the twin, which is pure Ply — and that is
where the roadmap's "concurrent request races become findable" is actually
delivered, because a seeded search over two requests hitting one row needs a
store it can fork and postgres is not one.

---

## 2. Footprint granularity

### 2.1 The label is a table, and the call site writes it

`db.query[items](s, ps)` performs the atom `(db, items, Read)`;
`db.execute[items](...)` and `db.returning[items](...)` perform
`(db, items, Write)`. Resource labels are ground identifiers in the source and
the language has nothing else, so the call site is the only place the label can
come from. The label a call site writes is the statement's **principal table** —
the relation the program considers the statement to be about, and the one
`ply hosts` prints a row for.

The transaction control operations take **no** resource, so their atom is the
singleton `db.write`. That is a real scheduling cost, stated rather than
discovered: every definition that opens a transaction carries `db.write`, so any
two tests that open transactions conflict and are serialised even when their
tables are disjoint. It is also true — they contend for the same pool, and a
pool is exactly the host state ADR 0008 §6 says cannot be forked. Read-only
endpoints do not open transactions and keep their concurrency, which is where
most of a service's parallelism is. A program that wants finer granularity
writes its own `handle` over `db.begin` with its own labels, exactly as ADR 0013
§2.4 says about `conn`.

### 2.2 A statement may touch more tables than its label names

This is the hole, and it is the only interesting problem in the milestone.
`select … from orders join items …` performed as `db.query[orders]` records one
atom and touches two tables. Nothing in the type system can see it, because the
SQL is a `String` and Ply has no compile-time evaluation.

Three answers were considered and two are refused:

- **One statement, one table** — refuse a statement whose table set is not
  exactly its label. Preserves the property perfectly and makes a join
  inexpressible, which is not a database.
- **Widen the label to a group** — `db.read[orders_and_items]`. The label stops
  being a table, `ply hosts` stops printing tables, and "which routes write this
  table" stops having an answer. This is `db.write[db]` with more syllables.
- **Report what was touched, and check the report.** Taken.

### 2.3 The driver reports its footprint, and the machine checks it

ADR 0008 §2 says a handler "is handed that atom and has no way to report a
different one", and calls the resulting blindness the trust the boundary is
bought with. W4 narrows that, for the specific case it introduces — an operation
whose true footprint is a function of a runtime value:

```rust
/// A completed host operation.
pub struct HostReply {
    pub value: Value,
    /// Every atom this operation touched **beyond** the one the registry
    /// resolved. Empty for every handler whose footprint is a property of its
    /// registration, which is every handler W1 and W3 shipped.
    pub touched: Footprint,
}

impl HostReply {
    /// The W1/W3 shape: a value, nothing touched beyond the resolved atom.
    pub fn value(value: Value) -> HostReply;
}

pub enum HostAnswer { Reply(HostReply), Pending(Pending) }

pub trait HostRuntime {
    fn poll(&self, p: &Pending) -> Result<Option<HostReply>, Diagnostic>;
    fn park(&self) -> Result<(), Diagnostic>;
    fn block_on(&self, p: Pending) -> Result<HostReply, Diagnostic>;
}
```

The machine, on every host answer, checks **each** atom of `touched` against the
entry point's declared footprint and unions the whole set into `HostUse`. An
atom outside it is `E0434 DB_FOOTPRINT_UNDECLARED`, the *program's* fault,
attributed and bisected like any other program failure — as distinct from
`E0427`, which stays what it is: the registry-resolved atom disagreeing with the
row, which is Ply's fault and is not bisected.

**`E0434` is a detector and not a preventer, and that has to be said out loud.**
Scheduling happened before the run, from the declared footprint, so by the time
the check fires the statement has executed against a table the scheduler thought
nobody was touching. What it buys is that a wrong row fails loudly on its first
execution instead of quietly forever — which, given that every dangerous defect
this project has found was a green result over unexplored space, is the
difference that matters.

The preventer is the second lock, and it runs earlier:

```rust
    /// The declared footprint of the entry point that reached this operation,
    /// so a handler that can compute its own footprint can refuse instead of
    /// acting.
    pub declared: &'a Footprint,
```

on `HostRequest`. The driver computes a statement's table set at **prepare**
time — once per statement text, cached — and refuses `E0434` there, before a row
moves, when a table is missing from `declared`. The machine's check on `touched`
then covers the case the driver's own scan got wrong.

This does not close ADR 0008 §2. A handler that lies about `touched` is exactly
as invisible as a handler that lies about its registration. What it closes is
the case where the *honest* handler could not tell the truth, and W4's driver is
the first handler in the system whose footprint is not a constant.

### 2.4 Where the table set comes from

`ply_host::db::scan` — a bounded scanner over the statement text, in Rust,
inside the trusted computing base. It recognises exactly the statement shapes W4
admits and **refuses everything else**: an unrecognised construct is `E0432
DB_STATEMENT_REFUSED` naming the byte offset and the token, never an empty table
set. Conservative in the safe direction by construction — a defect in the
scanner is a refusal to run rather than a footprint that under-reports — with
one residual, that a defect which *mis*-recognises a construct can still
under-report, which is why the scanner is disclosed in `ply hosts` and has the
differential test below.

Writing it in Ply was considered, and it is what ADR 0013 did with HTTP framing
for reasons that mostly apply here too. It is refused for one that does not: the
driver needs the answer, the driver is Rust, and a Ply implementation would mean
two scanners. Two parsers that disagree is the hazard ADR 0013 §2 exists to
prevent, and here the disagreement would be between the footprint a test
observes and the footprint the scheduler was given.

Recognised: `SELECT` (with `FROM`, `JOIN`, `USING`, set operations, and `WITH`
whose CTEs are resolved to their own sources), `INSERT INTO … [RETURNING]`,
`UPDATE … [FROM] … [RETURNING]`, `DELETE FROM … [USING] … [RETURNING]`, and
`VALUES`. Refused: anything else at all, including `DO`, `CALL`, `COPY`,
`CREATE`/`ALTER`/`DROP` outside `create_schema` (§7), `LOCK`, `LISTEN`,
`NOTIFY`, `SET`, a subquery in a position the scanner does not model, and any
statement containing a `;` outside a string literal or a dollar-quoted body.

**The differential test is the evidence, and postgres is the oracle.** Over a
generated corpus of statements against a fixture schema, `scan`'s table set must
be a **superset** of the relations `EXPLAIN (GENERIC_PLAN, FORMAT JSON)` reports
for the same statement. A superset rather than an equality, because the planner
prunes: a partition or a constant-false branch that never executes is a table
`EXPLAIN` omits and `scan` names, and over-reporting is the direction that costs
concurrency instead of correctness.

### 2.5 What the scanner cannot see, and what the bind does about it

A trigger, a rewrite rule, or a foreign key with `ON DELETE CASCADE` /
`ON UPDATE CASCADE` / `SET NULL` / `SET DEFAULT` makes one statement touch a
table its text never names. No scanner can see that, because it is not in the
statement.

So the *database* is asked, at bind time. `HostRegistry::bind` for the db driver
queries `pg_trigger`, `pg_rewrite` and `pg_constraint` over every table any of
the program's `db` atoms names, and any object that could reach a table outside
the atom it fires under is **`E0438 DB_UNMODELLED_SIDE_EFFECT`**, before
anything runs, naming the trigger or the constraint and the table it reaches.

This is strict, and being strict is the decision. A footprint that under-reports
corrupts scheduling and isolation silently; a service whose schema has a trigger
gets a start-up refusal it can read and fix. There is no flag to suppress it,
because a flag that turns a soundness check off is a flag whose default becomes
the one nobody uses.

### 2.6 What `ply check --types` prints

Unchanged, and that is the exit criterion. `examples/desk.ply`'s rows are
`desk.store.read[items]` today; behind postgres they are `std.db.db.read[items]`
and every endpoint's row still names its tables. Swapping `run`'s clause set for
a postgres handler changes `run` and no other definition, which is the claim
`desk.ply`'s own comment has been making since W3.

---

## 3. The connection pool

### 3.1 What it is made of

`deadpool-postgres` over `tokio-postgres`, driven by a **current-thread** tokio
runtime on one OS thread owned by `ply_host::db::Reactor`. This is where ADR
0013 §9's open item lands: tokio has been a declared dependency of `ply-host`
that nothing used, and W4 uses it, because `tokio-postgres` is an async client
and reimplementing the wire protocol to avoid a runtime would be growing the TCB
to hold a protocol for no reason ADR 0013's rule endorses.

No Ply value crosses to that thread. A `Value` is not `Send` and the boundary is
the same one W1 drew: the reactor speaks parameters and rows in postgres's own
types, and the conversion to and from `Value` happens on the machine's thread
inside `call` and `poll`.

Every `db` operation is `blocking: true` and answers `HostAnswer::Pending`. A
`blocking: true` handler that answered a value inline is `E0428` (ADR 0011 §7);
the db driver has no path that does.

### 3.2 Acquisition, release, exhaustion, timeouts

| knob | default | what it bounds |
| --- | --- | --- |
| `--db-pool N` | 8 | connections in the pool |
| `--db-acquire-ms` | 5000 | waiting for a connection |
| `--db-statement-ms` | 30000 | server-side `statement_timeout` |
| `--db-idle-txn-ms` | 30000 | server-side `idle_in_transaction_session_timeout` |
| `--db-connect-ms` | 5000 | establishing a connection |

The last two are set with `SET` on every connection at checkout, and they are
not optional. A statement with no server-side timeout holds a pool slot until
the server restarts, and an idle transaction holds locks the rest of the service
is waiting on; both turn one slow query into a service outage, and the whole
point of ADR 0013 §4's "a bound is part of the contract, not a tuning knob" is
that a bound nobody chose is a bound set to infinity.

- **Acquisition** happens at `begin` for a transaction, and per statement
  otherwise. A statement inside a scope reuses the scope's connection and never
  waits.
- **Release** happens at `commit` / `abort`, at `end_entry_point` (§1.3), and
  immediately after a scope-less statement.
- **Exhaustion** — no connection within `--db-acquire-ms` — is **`E0437
  DB_POOL_EXHAUSTED`**, a diagnostic and not a `Failed`. It names the pool size,
  the number checked out, and the operation that waited. It is the run's
  configuration at fault rather than the program's: the program asked for a
  connection and the run said how many exist. A `Failed` here was the
  alternative and is refused, because a `Failed` is a value a program is
  invited to swallow, and a swallowed pool exhaustion is a service that returns
  wrong answers under exactly the load that produced it. W5 owns backpressure
  and is where this becomes a shed request rather than a stop.
- **A connection whose transaction was abandoned** is §1.3: rolled back on
  release, and closed and discarded if the rollback fails.
- **Connect failure** at bind time is `E0431 DB_NOT_CONFIGURED`. Connect failure
  *during* a run is `Failed(e)` with SQLSTATE `08006`, because a database that
  restarted is a peer that went away and ADR 0013 §7.1 already decided what
  those are.

### 3.3 The pool and W1's scheduler

The two bounds do not compose the way a reader expects, so both are stated.

`MAX_BLOCKING_OPERATIONS` is 64 — W1's blocking pool, one real thread per
waiting socket operation. The db reactor is **not** on it: a `db` operation
dispatches to the reactor thread and answers `Pending` immediately, so an
outstanding query costs a pending token and no blocking-pool thread. The two
capacities are independent, and a service can have 64 socket operations and 8
queries in flight at once.

A task parked on a `Pending` leaves the enabled set. When nothing is enabled the
scheduler calls `HostRuntime::park`, which waits for any outstanding token —
socket or query — and the existing deadlock check (`E0414`) covers the case
where none can resolve. The pool acquire deadline is what keeps that check
honest: without it, a pool smaller than the number of open scopes parks every
task forever with nothing to report, and `E0437` turns a hang into a sentence.

### 3.4 The pool and world isolation, stated bluntly

A pooled connection is shared state that crosses test boundaries. ADR 0008 §6
says host effects are outside the forkable world, so:

- Every test that reaches the db binding is `Isolation::Host`, counted
  separately, excluded from `isolated: n of m`, never cached, and never
  bisected. All of that exists already and W4 adds no case to it.
- **W4 does not give a test its own database.** A test's isolation is exactly
  two things: footprint conflict grouping over tables, and whatever the test
  does inside a transaction it rolls back. There is no fork, no template
  database, no schema-per-test and no truncation between tests.
- Two host-backed tests whose tables are disjoint therefore run *concurrently
  against one database*, which is correct only if §2's footprints are honest.
  This is the sharpest place in the system where §2 is load-bearing, and it is
  why §2.5 refuses a schema with a trigger rather than warning about one.

`std.db` ships the one thing that helps:

```ply
// A transaction that always rolls back, whatever the body did. The body's value
// comes out; its writes do not.
pub fn sandbox<a, e>(body: () -> a / {db.write | e}) -> a / {db.write | e}
```

and its limits are stated where it is defined: it does not isolate DDL, does not
roll back a sequence's advance (postgres's sequences are non-transactional, so
two sandboxed tests see different ids and a test asserting `id == 1` is wrong on
its second run), does not isolate what a *different* connection does, and cannot
nest below the depth bound in §1.4.

---

## 4. Prepared statements and parameters

### 4.1 A value never becomes syntax

Every `db` data operation takes `(Stmt, List<Param>)`. The driver issues
`Parse` / `Bind` / `Execute` — the extended query protocol — so parameters cross
as typed binary values in a `Bind` message and are never part of the statement
text. There is no operation in W4 that interpolates, formats, escapes or quotes
a value into SQL, and none will be added; a program cannot express one because
no such function exists to call.

**What that claim does and does not cover, exactly.** It covers every value: a
`String` in a `PText` cannot terminate a literal, because it is not in a
literal. It does **not** cover a program that builds its own statement text by
`++`, because `stmt` takes a `String` and Ply has no way to demand a literal.
Two mechanical defences narrow that and neither is a proof:

- **One statement per `Stmt`.** A `;` outside a string literal or a dollar-quoted
  body is `E0432`, which removes stacked statements — the payload class that
  turns an injected fragment into a `DROP`.
- **§2.4's scanner refuses what it cannot account for**, so an injected fragment
  that changes the statement's shape is usually a refusal rather than an
  execution.

The honest summary, which belongs in the ADR rather than in a README: values are
structurally safe; statement text is a program's own to get right, and W4 makes
a dynamic one loud rather than impossible.

### 4.2 The type mapping

Pinned. A parameter or a result column outside this table is `E0432` at prepare
time, naming the postgres type and the column.

| Ply | postgres (parameter) | postgres (result) |
| --- | --- | --- |
| `Int` | `int8` | `int2`, `int4`, `int8` |
| `Bool` | `bool` | `bool` |
| `String` | `text` | `text`, `varchar`, `bpchar`, `name`, `uuid` |
| `Bytes` | `bytea` | `bytea` |
| `Float` | `float8` | `float4`, `float8` |
| `Decimal` | `numeric` | `numeric` |
| `Json` (`std.json`) | `jsonb` | `json`, `jsonb` |
| `List<a>` | `a[]`, one dimension | `a[]`, one dimension |
| `Option<a>` | `a` or `NULL` | a nullable column of `a` |
| `Unit` | not a parameter | not a column |

Rules at the edges, each of which is a place a driver quietly loses data:

- **`numeric` beyond scale 28, or with more than 96 bits of mantissa, is a
  decode failure** naming the column, never a rounding. This is W2 §4's whole
  argument — a total that quietly lost a cent — applied to the wire.
  `numeric` `NaN` and `±Infinity` are decode failures too: `Decimal` has no
  representation for them and substituting zero is the silent-wrong-answer
  shape.
- **`int2` and `int4` widen to `Int` losslessly; `Int` narrows to nothing.** A
  parameter is always sent as `int8` and postgres performs the assignment cast,
  so an `Int` that does not fit an `int4` column is `Failed` with `22003` from
  the server rather than a truncation in the driver.
- **A one-dimensional array only.** A multi-dimensional array, or an array with
  a `NULL` element, is a decode failure naming the column. `PArray` whose
  elements are not all one non-null constructor is `E0432`; `PArray([])` is
  legal and takes its element type from the parameter description.
- **`Option<Option<a>>` is refused** wherever it appears, exactly as ADR 0012
  amendment A1 refuses it for `json` and for the same reason: two values with
  one wire form.
- **No date, time, timestamp or interval type.** Ply has no time type, and a
  column of one is `E0432`. A `timestamptz` is stored as `int8` microseconds
  since the Unix epoch and a `date` as `int4` days, by the program's own schema,
  and the value comes from `clock.now()` **as a parameter** — which is better
  than `now()` in the statement, because it puts the nondeterminism in the row
  where `E0412` can see it instead of hiding it inside a string. `now()`,
  `current_timestamp` and `random()` in statement text are `E0432`, naming this
  paragraph. This is a real gap in what W4 can talk to and it is stated rather
  than worked around.
- **A result description with two columns of one name is `E0433`.** A `Row` is a
  `Map`, so `select a.id, b.id` would silently keep one of them.

### 4.3 Preparation and caching

A statement is prepared per connection, keyed by its text, in an LRU of
`--db-statement-cache` entries (default 256). Preparation is where the result
description arrives, so it is where §2.4's scan, §4.2's type check, §4.4's codec
check and §2.3's footprint refusal all happen — once per statement per
connection, never per execution.

`DEALLOCATE` is never issued; an evicted entry's server-side statement is closed
by the protocol's `Close` message. `DISCARD ALL` is never issued either, because
it would drop the cache the pool exists to amortise; connection reset is
§1.3's rollback and nothing more.

A prepare that postgres refuses — a syntax error, an unknown relation, an
unknown column — is **`E0433 DB_PREPARE_FAILED`** and not a `Failed`. It is the
program's fault, it is the same shape every time, and it will never succeed on a
retry, so making it a value would invite a program to loop on it. A statement
that prepares and then fails at execution is `Failed`, because that one depends
on the data.

### 4.4 `derive row`

ADR 0010 named `row` as a deriver and ADR 0012 §3 deferred it to W4 "with the
`Row` type it is a codec over". It lands here.

```ply
pub type RowError = { column: String, expected: String, found: String }

pub type RowCodec<a> = {
  columns: List<String>,
  decode:  (Row) -> Result<a, RowError>,
  params:  (a) -> List<Param>,      // in `columns` order
}
```

`derive row for Item` generates `fn item_row() -> RowCodec<Item>`, under ADR
0012 §3's naming, orphan, visibility, expansion-point, hashing and
`E0505`-on-generated-body rules unchanged. Everything true of `json` is true of
`row`.

What `row` walks, which is narrower than what `json` walks:

- The target must be a **record**. An ADT is `E0206 NOT_DERIVABLE` naming the
  type: a row is flat and a sum has no columns.
- A field whose type is a scalar leaf (`Int`, `Bool`, `String`, `Bytes`,
  `Float`, `Decimal`) is a column of that type.
- A field of type `Option<leaf>` is a nullable column. `Option<Option<a>>` is
  `E0206`.
- A field of type `List<leaf>` is a one-dimensional array column.
- **A field that is none of those, but is `derivable(json, ·)`, is a `jsonb`
  column** encoded through that type's json codec. This is what lets
  `examples/desk.ply`'s `Order` — which has `lines: List<Line>` and a
  `state: State` — derive at all, and it is the point where a reader should
  notice that W4 has no opinion about normalization: a program that wants
  `order_lines` as its own table writes two codecs and two statements, and
  `derive row` will not do a join for it.
- Anything else — a function, a `Cell`, a `Task`, a `Map` with a non-`String`
  key — is `E0206` naming the field, exactly as `json` reports it.

The column name is the **field name, unchanged**. No case mangling, no prefix,
no pluralisation. A column named differently from its field is a hand-written
codec, and a rule that guessed would be a rule that guesses wrong once and
silently.

`RowCodec::columns` is why it is in the record: at prepare time the driver
checks the statement's result description against the codec's column list, so a
`select` missing a column the codec needs is `E0433` **before the first row
arrives** rather than a decode failure per row afterwards.

Constraints are `where derivable(row, a)`, checked at the signature exactly as
ADR 0012 §3 specifies, with the deriver tag added to `tag::CONSTRAINT`'s pinned
enumeration.

---

## 5. The in-memory twin

### 5.1 It is Ply, and it is pure

The twin is `std.db`'s memory engine: a set of **pure functions over a `MemDb`
value**, with no effects at all.

```ply
pub type MemDb = { .. }                      // opaque: tables, sequences, scope stack

pub fn open(s: Schema) -> MemDb
pub fn step(d: MemDb, s: Stmt, ps: List<Param>) -> { db: MemDb, out: Answer }
pub fn begin_step(d: MemDb, level: Isolation, access: Access) -> { db: MemDb, out: Answer }
pub fn commit_step(d: MemDb) -> { db: MemDb, out: Answer }
pub fn abort_step(d: MemDb) -> { db: MemDb, out: Answer }
```

Rows are `Map<String, Cell>` and answers are `Answer`, so the twin and the
driver produce values of one type by construction — ADR 0008 §5's "the same
declared signature", made structural rather than promised.

A program installs it with an ordinary `handle` over a region-scoped cell, which
is `examples/desk.ply`'s existing shape with the clause bodies changed:

```ply
with_cell[store](db::open(schema())) { c ->
  handle { serve(...) } with {
    db.query[items](s, ps)     -> db::run(c, s, ps),
    db.execute[items](s, ps)   -> db::run(c, s, ps),
    db.query[orders](s, ps)    -> db::run(c, s, ps),
    db.execute[orders](s, ps)  -> db::run(c, s, ps),
    db.begin(level, access)    -> db::run_begin(c, level, access),
    db.commit()                -> db::run_commit(c),
    db.abort()                 -> db::run_abort(c),
    db.rollback(reason)        -> (),
  }
}
```

`db::run(c, s, ps)` is the two-line cell wrapper `std.db` ships. The boilerplate
is proportional to tables times operations and it is real; it is also the same
boilerplate `desk.ply` already writes, and a handler clause naming a concrete
resource is what makes the discharge visible at the resource granularity the
whole design is about.

The consequence that matters: after `with_cell` discharges the cell's atoms, a
twin-backed test's row is **empty**. It is `det`. It is cached. It is hermetic
without `--host`. And it can run inside `simulate`, which is what makes a
check-then-act race between two requests on one row findable and replayable from
a seed.

### 5.2 What the twin models

It executes the same `Stmt` text the driver does, through its own scanner —
which is the point, because the scanner is where the divergences live and a twin
that took a structured operation instead would never test it.

- **Tables and columns** from a `Schema` value (§7), with declared types and
  nullability.
- **Rows**, in insertion order, with `Cell` values under §4.2's type mapping.
- **`SELECT`** with a `WHERE` over comparisons, `AND` / `OR` / `NOT`, `IS NULL`,
  `IN`, `BETWEEN` and `LIKE`; `ORDER BY` over one or more columns with
  `ASC`/`DESC` and `NULLS FIRST`/`NULLS LAST`; `LIMIT` and `OFFSET`;
  `count(*)`.
- **`INSERT`** with `VALUES` and `RETURNING`, including a `DEFAULT nextval`
  column.
- **`UPDATE … SET … WHERE … [RETURNING]`** and
  **`DELETE FROM … WHERE … [RETURNING]`**.
- **Constraints**: `NOT NULL` (`23502`), `PRIMARY KEY` and `UNIQUE` (`23505`),
  `FOREIGN KEY` existence with `NO ACTION` (`23503`), and `CHECK` over the same
  expression grammar the `WHERE` uses (`23514`). Each carries the constraint's
  name in `DbError::constraint`, because that is what a program branches on.
- **Transactions**: `BEGIN`, `COMMIT`, `ROLLBACK`, and savepoints at §1.4's
  depths, over a stack of snapshots. A `MemDb` is persistent (`Map` and `List`
  share structure), so a snapshot is a pointer copy and a rollback is dropping
  one.
- **The failed-transaction state.** After a statement fails inside a scope,
  every subsequent statement in that scope is `Failed` with `25P02` until the
  scope ends or a savepoint below the failure is rolled back to. This is the
  postgres behaviour test doubles omit most often, it is the one that makes a
  suite pass and production fail, and it is required.
- **Type errors** the server would raise: `22P02` for a text parameter in an
  integer column, `22003` for an `Int` outside an `int4` column's range.

### 5.3 What the twin does not model — and how it says so

Anything outside §5.2 makes `step` answer `Failed({code: "0A000", constraint:
"", detail: "<the construct>"})` — `feature_not_supported`, the SQLSTATE
postgres itself uses. **It never guesses and never answers as though it
executed.** A test that exercises an unmodelled statement fails, loudly, with
the construct named, on the twin, hermetically, in the run that introduced it.

Named, so that nobody has to discover them:

- **Joins, subqueries, `GROUP BY`, `HAVING`, window functions, CTEs, set
  operations, and every aggregate but `count(*)`.** A `WHERE` is not a query
  planner and W4 does not ship one.
- **Views, triggers, rules, `ON CONFLICT`, generated columns, and partial or
  expression indexes.**
- **Isolation.** The twin is serial: one connection, one scope stack, no
  concurrency. It cannot exhibit a phantom read, a lost update, a serialization
  failure or a deadlock, so `RepeatableRead` and `Serializable` are accepted and
  behave as a serial execution. §6's law therefore quantifies over sequential
  operation sequences and claims nothing about concurrent ones. This is the
  largest thing the twin does not model and it is the one a reader is most
  likely to assume it does.
- **Collation.** The twin orders `String` by W2's `Value` order, which is byte
  order, which is `C`. Under any other database collation `ORDER BY` on text
  disagrees. §6's fixture database is created `LC_COLLATE=C LC_CTYPE=C
  ENCODING=UTF8` and `ply hosts --host` prints the live database's collation, so
  the divergence is visible rather than latent.
- **`numeric` beyond `Decimal`'s 96 bits and scale 28**, `float4` rounding,
  and every locale-dependent function.
- **Sequences under rollback.** Postgres does not roll back `nextval`; the twin
  does not either, deliberately, because matching the surprising behaviour is
  the whole job.

---

## 6. The agreement law

This is the milestone's headline deliverable and it is what the other six
sections are in service of.

### 6.1 `law/host` — a law may reach the world, and says so in its declaration

An M8 law body's row must be a subset of `{sim.read}` or it is `E0417`
(DESIGN.md §7). The agreement law's body performs `db` atoms on one side, so as
the language stands it does not compile. Relaxing `E0417` silently would mean a
law could touch the world without saying so, which is the opposite of every
other decision here, so the relaxation is **declared**, exactly as `test/nondet`
declares a test's:

```ply
law/host "the memory engine agrees with postgres"
  forall (ops: List<Op>) where well_formed(fixture(), ops) {
    replay_memory(fixture(), ops) == replay_live(ops)
  }
```

- A `law/host`'s **body** may carry any row. Its **guard** may not: a `where`
  stays pure under `E0417` unchanged, because a guard decides the domain and a
  guard that could act would be choosing which cases to be judged on.
- A `law/host` **can never be `proved`.** Structurally, not by convention: the
  prover's lowering returns "unsupported" for a body whose row is non-empty, so
  the certificate cannot be constructed. `property` is its ceiling and the tier
  says so.
- A `law/host` is **never cached**, in either direction, exactly as a
  host-backed test is not.
- Under a hermetic run — which is `ply prove`'s default — a `law/host` is
  reported **`W0604 OBLIGATION_NOT_DISCHARGED`**, `unattempted`, with the reason
  "reaches the host; run `ply prove --host`". It is not skipped silently and it
  is not green. A law about a database that never ran a database, reported as
  passing, would be precisely the "green result over unexplored space" this
  project audits for.
- `law/host` is part of the law's own hash — `LawDef::host`, written after
  `tag::LAW` exactly as `TestDef::nondet` is written after `tag::TEST`. A law
  that changes from `law` to `law/host` is a different claim and re-discharges.
- A `law` (without `/host`) whose body carries a non-`{sim.read}` row is
  `E0417`, with the message amended to name `law/host` as the fix.

### 6.2 What is generated, and how the two sides are driven

```ply
pub type Op =
  | Insert({ table: String, values: List<Param> })
  | Update({ table: String, column: String, to: Param, where_col: String, eq: Param })
  | Delete({ table: String, where_col: String, eq: Param })
  | Select({ table: String, order_by: String, limit: Int })
  | Count({ table: String })
  | Begin(Isolation) | Commit | Abort | Savepoint | Release | RollbackTo

pub fn render(fx: Schema, op: Op) -> { stmt: Stmt, params: List<Param> }
```

`Op` is an ordinary ADT, so M8's existing generator quantifies over it and M8's
existing shrinker shrinks it — no new generator, no new shrinking rule, which is
what makes the law cheap enough to be a required test rather than a project.

**Both sides execute the rendered SQL.** `render` is called once per op and its
output goes to the twin and to postgres unchanged, so the twin's scanner is on
the tested path. A structured op handed to the twin and SQL handed to the driver
would have tested everything except the place the bugs are.

`well_formed(fixture, ops)` is a **pure** guard that rejects sequences the law
has no opinion about: an unbalanced scope stack, a savepoint below depth zero, a
parameter whose type does not match its column, and a nesting depth over §1.4's
bound. A guard that admitted them would make the law a claim about error
messages.

Each op yields an `Answer`, and the two sides are compared as `List<Answer>`:

- `Rows` compares as a list of `Row`s **in the order returned**, because
  `ORDER BY` is in the generator and an unordered `SELECT` is `ORDER BY` on the
  primary key by construction of `render`. Postgres does not promise an order
  without one, and a law that compared unordered results would be flaky by
  design.
- `Failed` compares on **`code` and `constraint` only**. `detail` is postgres's
  prose and is never compared. This is the single most important line in §6: a
  law that compared messages would fail on a server upgrade and would teach
  everyone to ignore it.
- The comparison stops at the **first** differing index and reports it, because
  a divergence at op 3 makes ops 4..n meaningless.

### 6.3 What a counterexample looks like

```
error[E0419]: law "the memory engine agrees with postgres" was refuted
  ┌─ examples/agreement.ply:44:1
44│ law/host "the memory engine agrees with postgres"
  │ ^^^^^^^^ refuted after 118 cases; shrunk from 37 ops to 3

  ops:
    Begin(ReadCommitted)
    Insert({table: "part", values: [PText("a"), PNumeric(1.00m)]})
    Select({table: "part", order_by: "sku", limit: 10})

  first difference at op 3:
    memory   Rows([{price: CNumeric(1.00), sku: CText("a")}])
    postgres Rows([{price: CNumeric(1.0000), sku: CText("a")}])

  = the twin preserved the literal's scale; the column is `numeric(10,4)` and
    postgres returned the column's scale
  = replay: ply prove --host --law "the memory engine agrees with postgres" --case 118
  = tier: property (118 cases, shrunk) — a `law/host` can never be `proved`
```

Three things are required of that output and each has been wrong somewhere
before: the op list is **Ply source a reader can paste into a test**; the seed or
case index replays it exactly; and the tier line says why the ceiling is
`property` rather than leaving a reader to infer that a green `property` was the
best available.

### 6.4 The law must be able to fail

A law that cannot fail is decoration. So a required test **injects a known
divergence** and asserts the law finds it: the fixture database is created with
a non-`C` collation, and the law must report a refutation on an `ORDER BY` over
text — with the shrunk counterexample being two rows and one select. A second
injection removes the twin's `25P02` failed-transaction state and asserts the
law finds that.

Without those, "the agreement law passes" is a statement about the generator's
reach and nothing else.

### 6.5 Where it lives

`examples/agreement.ply`: a two-table fixture (`part`, `bin`, with a primary
key, a unique constraint, a foreign key, a not-null and a check) whose only
purpose is this law, plus the hermetic `det` tests of `render` and
`well_formed`. It is **not** in `std.db`, because `ply test --std` and
`ply prove --std` must not need a database to pass.

---

## 7. Migrations

**A migration tool is out of scope.** No versions, no up and down, no ordering
across deploys, no diffing a live database into a change script. That is a
product, it is orthogonal to everything this milestone is about, and a
half-built one would be worse than none.

A **schema is a value**, which is the part W4 does need — the twin has to be
built from something, and the law's fixture has to exist.

```ply
pub type ColumnType = TInt | TBool | TText | TBytes | TFloat | TNumeric(Int, Int) | TJson
                    | TArray(ColumnType)

pub type Default = DNone | DSequence(String) | DLiteral(Param)

pub type Column = { name: String, ty: ColumnType, nullable: Bool, default: Default }

pub type ForeignKey = { name: String, columns: List<String>,
                        references: String, refers_to: List<String> }

pub type Check = { name: String, expr: String }

pub type Table = {
  name: String, columns: List<Column>,
  primary_key: List<String>, unique: List<{name: String, columns: List<String>}>,
  foreign_keys: List<ForeignKey>, checks: List<Check>,
}

pub type Schema = { tables: List<Table> }

pub fn create_schema(s: Schema) -> List<Stmt>     // pure: CREATE TABLE text
pub fn drop_schema(s: Schema) -> List<Stmt>
```

How a schema comes to exist, in each of the three places it has to:

1. **The twin**: `open(schema())`. Nothing else is involved and nothing touches
   a disk.
2. **A test or the law's fixture, against real postgres**: the harness executes
   `create_schema(schema())` against a database it created. `create_schema`'s
   output is the one DDL §2.4's scanner accepts, and it accepts it only from
   this path — a `CREATE TABLE` in a `db.execute[t]` call is `E0432` like any
   other unrecognised statement, because DDL inside a request handler is not a
   thing W4 has an opinion about how to schedule.
3. **A production database**: it already exists, and W4's job is to check that
   it is the one the program describes. `--db-schema <module>.<fn>` names a
   nullary function returning a `Schema`; at bind time the driver materialises
   it, reads `information_schema` and `pg_constraint`, and reports **`E0435
   DB_SCHEMA_MISMATCH`** for every difference — a missing table, a missing
   column, a type that does not match §4.2's mapping, a nullability that
   disagrees, a missing constraint — before anything runs.

That third point is most of what a migration tool is actually bought for: the
guarantee that the code and the database agree, checked at start-up rather than
discovered at the first request. W4 delivers that without owning the tool that
changes the database, and says so.

`--db-schema` is optional. Without it, a mismatch surfaces at prepare time as
`E0433` — later, per statement, and still loud.

---

## 8. New diagnostic codes

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0431 | `DB_NOT_CONFIGURED` | `--host` binds the db driver and no `--db` URL was given, or it is malformed, or the server is unreachable at bind time | the run's configuration |
| E0432 | `DB_STATEMENT_REFUSED` | statement text W4 refuses: more than one statement; a construct §2.4's scanner cannot account for; a parameter or result type outside §4.2; `now()` / `random()` in the text | the program's |
| E0433 | `DB_PREPARE_FAILED` | postgres refused to prepare — syntax, unknown relation, unknown column — or the result description has a duplicate column name or lacks a column the codec requires | the program's |
| E0434 | `DB_FOOTPRINT_UNDECLARED` | a statement touches a table outside the entry point's declared footprint: at prepare, from `HostRequest::declared`; at answer, from `HostReply::touched` | the program's |
| E0435 | `DB_SCHEMA_MISMATCH` | the live database differs from the `Schema` the run named | the run's configuration |
| E0436 | `DB_TRANSACTION_SCOPE` | a `db` operation from a task that does not own the open scope | the program's |
| E0437 | `DB_POOL_EXHAUSTED` | no connection became available within `--db-acquire-ms` | the run's configuration |
| E0438 | `DB_UNMODELLED_SIDE_EFFECT` | a trigger, rule, or cascading referential action reaching a table outside the atom it fires under | the run's configuration |

E0431, E0435 and E0438 are raised by `HostRegistry::bind`, before anything runs,
like E0421–E0423.

E0432, E0433, E0434, E0436 and E0437 join E0424's row: `Failure::defect` is
`false`, they are attributed like any other failure, and bisection is skipped
when the run reached the host to produce them (E0434 at answer time and E0437
always; E0432, E0433, E0434-at-prepare and E0436 refuse before a statement
executes, so those are ordinary).

**Only the three bind-time codes join `RESERVED_CODES`** — E0431, E0435, E0438.
The other five are refusals the driver is the only component in a position to
compute: a statement's table set, a result description, the task holding a
scope, a pool's occupancy. Reserving those would have `attribute` rewrite the
driver's own diagnosis to `E0502` and send the reader looking for a defect in
Ply, which is the exact failure the reserved set exists to prevent in the other
direction. The rule is unchanged and it is the second group they do not belong
to: they are not verdicts about the machine's state. `attribute` still stamps
each with the handler path, which is what makes them attributable.

E0434 is the one that is raised from **two** places — the driver at prepare time
and the machine at answer time — and it is unreserved for the first, which is
why the second must be the machine's own check rather than a rewrite of a
handler's word.

`E0417`'s message is amended to name `law/host`. No other existing code changes
meaning.

---

## 9. Versions

| constant | to | why |
| --- | --- | --- |
| `RUNTIME_VERSION` | `0.10.0` | `HostAnswer`, `HostReply` and `HostRuntime` changed shape; the machine checks `touched` and calls `end_entry_point`; a cached `Pass` is a claim about what the evaluator did |
| `FRONTEND_VERSION` | `0.12.0` | a new deriver (`row`), `LawDef::host`, and the `law/host` grammar. ADR 0012 §3's rule: any change to a deriver bumps this |
| `BODY_ENCODING` | `7` | `law_def` writes a host flag after its tag, as `test_def` writes `nondet` |
| `PROVER_VERSION` | `0.5.0` | `law/host` is a new discharge mode with a new ceiling and a new unattempted reason |

**`BODY_ENCODING` moving is a one-time cost with a bounded blast radius, and the
required test pins the boundary**: every law's hash moves once and re-discharges
once; **no non-law definition's normalized bytes change**, on the whole W3
corpus, asserted byte-for-byte. A milestone that moved definition hashes for a
law's sake would have got the layering wrong.

---

## 10. Workspace

```toml
tokio-postgres    = { version = "0.7.18", features = ["runtime"] }
postgres-protocol = "0.6.12"
deadpool-postgres = "0.14.1"
tokio             = { version = "1.53.1", features = ["rt", "net", "time", "sync"] }
rust_decimal      = { version = "1.42.1", features = ["db-tokio-postgres"] }
```

`ply-host` gains the first three and uses the fourth, which resolves ADR 0013
§9's open item: tokio has been declared and unused since W1, and a dependency in
a trusted computing base that nothing calls is a line a reviewer spends
attention on for nothing. It is called now.

Notes, because a dependency in the TCB is a review obligation:

- **`rt` and not `rt-multi-thread`.** One current-thread runtime on one OS
  thread owned by `ply_host::db::Reactor`. A work-stealing runtime buys nothing
  here — every connection's future is independent and the pool bounds the
  concurrency — and it would make the thread count a number nobody chose.
- **`deadpool-postgres` rather than `bb8` or a hand-rolled pool.** It is the
  smallest of the three, its recycling hook is where §1.3's rollback-on-release
  lives, and its size is a declared number rather than a default.
- **`postgres-protocol`** is taken directly for `numeric` and array
  encode/decode, where `tokio-postgres`'s `ToSql`/`FromSql` would otherwise
  require a Rust type per Ply type. `rust_decimal`'s `db-tokio-postgres` feature
  supplies the `numeric` conversion and is used rather than reimplemented.
- **No `sqlx`, no `diesel`, no `sea-orm`.** W4 ships no query builder and no
  ORM; a crate whose whole value is one would be a large dependency for a
  feature the milestone refuses.
- **TLS to postgres is not configured in W4.** `--db` accepts `sslmode=disable`
  and `sslmode=prefer` only, and `require` or above is `E0431` naming this
  paragraph. Wiring rustls into `tokio-postgres` is a small change and a real
  TCB decision, and it belongs beside W5's secrets rather than here where it
  would be an untested line.

`ply-std` gains one module and no dependency. `ply-eval` gains no dependency.

---

## 11. `ply hosts`

The TCB now contains a postgres driver and a SQL scanner, and a listing that
hides either is the failure ADR 0008 §2 exists to prevent.

```
$ ply hosts --host
   9 host handlers · 14 operations · trusted computing base

   OPERATION                 ATOM                  HANDLER                     DET  LINEAR         BLOCKING
   db.begin                  db.write              ply_host::db::begin         no   at-most-once   yes
   db.abort                  db.write              ply_host::db::abort         no   at-most-once   yes
   db.commit                 db.write              ply_host::db::commit        no   at-most-once   yes
   db.execute[items]         db.write[items]       ply_host::db::execute       no   at-most-once   yes
   db.execute[orders]        db.write[orders]      ply_host::db::execute       no   at-most-once   yes
   db.query[items]           db.read[items]        ply_host::db::query         no   at-most-once   yes
   db.query[orders]          db.read[orders]       ply_host::db::query         no   at-most-once   yes
   db.returning[orders]      db.write[orders]      ply_host::db::returning     no   at-most-once   yes
   net.accept[listener]      net.write[listener]   ply_host::tcp::accept       no   at-most-once   yes
   ...

   database
   server     PostgreSQL 18.3 · database desk · collation C · encoding UTF8
   pool       8 connections · acquire 5000ms · statement 30000ms · idle-txn 30000ms
   scanner    ply_host::db::scan · select insert update delete values with
   schema     desk.schema · 2 tables · 11 columns · verified

   digest: b3:7c02e9a41b6d
```

The `database` block exists for the same reason W3's `transport` block does: a
fact the rows cannot carry and a reviewer must not have to derive. The
**collation is printed** because §5.3 makes it the twin's largest silent
divergence, and the **scanner is printed** because it is a parser in the TCB and
ADR 0013's whole rule is that those are the lines worth a human's attention.

`db.rollback` does **not** appear: it is handled in Ply by `transaction` and
never reaches the binding. If it appears, something has bound it and that is a
defect.

The digest covers the operation rows, the pool numbers, the scanner's accepted
statement set and the schema function's name. It does **not** cover the server
version or the database name, by the same argument W3 makes about a certificate
fingerprint: a CI check that broke on a minor server upgrade is a CI check
people learn to ignore. Both are printed and both are in `--json`.

---

## 12. Amendments to W1

Two things W1 left are correctness problems once a statement is an insert.

### 12.1 `--engine both` degrades to one engine on a host-backed test, silently

The obvious worry is wrong and the real one is quieter, so both are stated.

`Engines::Both` runs a test under the tree-walker and the machine and compares.
It does **not** execute a host operation twice: `Interp` holds the binding
"only in order to *refuse* at it", so the tree-walker's arm ends at the first
host operation with `err_machine_only_host` (`E0504`), `execute_directly`
returns the machine's answer, and the insert happens once. That was checked
against the code rather than assumed, and W4 changes nothing about it.

What is wrong is the reporting. A host-backed test under `--engine both` gets
**no differential audit at all** — the tree-walker refused before it computed
anything to compare — and the run says `--engine both` regardless. So the one
command whose purpose is "two engines agree" quietly means "one engine ran" for
exactly the tests a database makes interesting, and the count of what was
audited is overstated by however many of them there are.

`ply test --explain` therefore reports such a test as `engine: machine (host,
not audited)`, the summary line carries `audited: n of m`, and `--json` carries
the same. This is `Isolation::Host` and `Skipped::Host`'s argument — declare the
guarantee inapplicable where it cannot hold, and keep the number honest —
applied to the one place W1 left it implicit.

### 12.2 `end_entry_point` is called on every exit

§1.3's hook has no value unless it is called on the diagnostic and
budget-exhaustion paths as well as the value path. `InterpExecutor` calls it
from one place per machine path, beside `arm_footprint_check`, and a required
test asserts an entry point that raised inside a transaction left no open scope
on the connection it used.

---

## 13. Required tests

The ones whose absence would let W4 ship broken rather than merely incomplete.

**Transactions**

1. A `db.rollback` deep inside a transaction body discards the continuation:
   the statements after it never execute, the value of `transaction` is `Err`,
   and postgres shows no row.
2. A committed transaction's rows are visible to a later statement; an aborted
   one's are not.
3. A body that raises `E0502` propagates the raise, commits nothing, and leaves
   no open scope — asserted against `pg_stat_activity`, not against the driver's
   own bookkeeping.
4. A nested `transaction` is a savepoint: the inner rollback discards the
   inner's writes and keeps the outer's; the outer commit persists them.
5. A nested `begin` with a different `Isolation` is `Failed` with `25001` naming
   both levels.
6. A `ReadOnly` transaction that writes is `Failed` with `25006` **from the
   server**, not from a check in the driver.
7. A serialization failure under `Serializable` is `Failed` with `40001`,
   `is_retryable` is true, and a program-written retry succeeds.
8. A continuation captured before `db.begin` and resumed twice is `E0426`, and
   `BEGIN` was issued exactly once.
9. A connection whose scope was abandoned is rolled back before it is reused,
   and one whose rollback fails is discarded rather than returned to the pool.
10. A `db` operation from a task that does not own the open scope is `E0436`.

**Footprints**

11. `db.query[items]` publishes `db.read[items]` and `db.execute[items]`
    publishes `db.write[items]`, printed by `ply check --types` with no flag.
12. A join across `orders` and `items` performed as `db.query[orders]` in a
    definition declaring only `{db.read[orders]}` is `E0434` **at prepare**,
    before any row is read; and the same statement with `{db.read[orders],
    db.read[items]}` declared runs and records both atoms in `HostUse`.
13. `HostReply::touched` reaches the machine's check: a handler answering an
    undeclared atom is `E0434` and the atom is in the reported footprint.
14. `scan`'s table set is a **superset** of `EXPLAIN (GENERIC_PLAN, FORMAT
    JSON)`'s relation set over a generated statement corpus, with no exception.
15. Every refused construct in §2.4 is `E0432` naming the offset; a statement
    with a `;` outside a literal is `E0432`; a `;` inside a literal and inside a
    dollar-quoted body is not.
16. A schema with a trigger, a rule, or an `ON DELETE CASCADE` reaching a table
    outside its atom is `E0438` at bind time, naming the object.
17. Two tests over disjoint tables are placed in different concurrency groups
    and run concurrently against one database; two over a shared table with one
    writer are not.

**Statements and parameters**

18. A `PText` containing `'; drop table part; --` inserts that string and
    changes no schema; the same bytes in `Stmt::sql` are `E0432`.
19. Every row of §4.2's mapping round-trips, in both directions, including an
    empty `PArray`, a `NULL` in an `Option` column, a `jsonb` document, and a
    `Decimal` at scale 28.
20. A `numeric` of scale 29, a `numeric` `NaN`, a two-dimensional array, an
    array with a `NULL` element, and a `timestamptz` column are each a named
    refusal and never a silently coerced value.
21. A duplicate result column name is `E0433`; a codec naming a column the
    statement does not return is `E0433` before the first row.
22. `derive row for Order` produces `order_row`, `lines` and `state` are `jsonb`
    columns, and a round-trip through a real table is identity.
23. `derive row` for an ADT, and for a record with a function field, is `E0206`
    naming the type and the field respectively.
24. Renaming the type re-runs no test; renaming a field re-runs exactly the
    tests reaching it — ADR 0012 test 16, for `row`.
25. The prepared-statement cache is used: N executions of one statement issue
    one `Parse`, asserted by a counting harness against the protocol, not by
    timing.

**The pool**

26. `--db-pool 1` with two concurrent transactions is `E0437` after
    `--db-acquire-ms`, naming the size and the operation — not a hang and not a
    deadlock report.
27. `statement_timeout` and `idle_in_transaction_session_timeout` are set on
    every connection at checkout, asserted by reading `current_setting` through
    the same connection.
28. A database restarted mid-run gives `Failed` with `08006` and the next
    request succeeds against a fresh connection.
29. A `db` operation in flight costs a pending token and **no**
    `MAX_BLOCKING_OPERATIONS` slot; 64 socket operations and 8 queries are in
    flight simultaneously.

**The twin**

30. Every clause of §5.2 has a `det`, hermetic, cached test, including the
    `25P02` failed-transaction state and the sequence that does not roll back.
31. Every construct in §5.3 answers `Failed` with `0A000` naming the construct,
    and never a result.
32. A twin-backed test's row is empty, it is `det`, it is cached, and it runs
    without `--host`.
33. `examples/desk.ply`'s test suite passes against the twin and against
    postgres **with no source change to any endpoint** — the exit criterion.
34. A check-then-act race between two requests on one row is found by
    `simulate` against the twin, reported with a seed, and replayed exactly.

**The law**

35. The agreement law is discharged as `property` with its case count under
    `ply prove --host`, and is `W0604 unattempted` with a reason without it.
36. A `law/host` is never `proved`, including a trivially true one, and the
    differential prover audit covers it.
37. A `law` whose body has a non-`{sim.read}` row is `E0417` naming `law/host`;
    a `law/host` whose **guard** has a non-empty row is still `E0417`.
38. **The law finds an injected divergence**: a non-`C` collation fixture is
    refuted on an `ORDER BY` over text, and a twin with the `25P02` state
    removed is refuted, each shrunk to a minimal op list.
39. A counterexample prints the ops as pasteable Ply source, replays from its
    case index, and compares `code` and `constraint` while never comparing
    `detail`.
40. Changing `law` to `law/host` changes the law's hash and no definition hash.

**Schema**

41. `create_schema` output executes against real postgres and produces a
    database that `--db-schema` verifies; a dropped column, a changed type and a
    changed nullability are each `E0435` naming the difference.
42. A `CREATE TABLE` passed to `db.execute` is `E0432`.

**Everything W4 must not regress**

43. Renaming a top-level function selects zero tests; moving a definition
    between modules changes no hash — on a corpus with `db` rows, `derive row`
    and a `law/host`.
44. Incremental and `--no-incremental` agree byte-for-byte across the full
    mutation sequence, with `derive row` and `law/host` edits added.
45. `--engine both` reports no `E0503` on the W4 corpus; a host-backed test
    performs its statements **exactly once** across both arms, and is reported
    `host, not audited` with the `audited: n of m` count excluding it (§12.1).
46. `E0412` still fires for an unsimulated nondeterministic effect in a `det`
    test; `ply test` is hermetic without `--host` and says so.
47. An effect-set alias and its explicit expansion hash identically, on a corpus
    whose alias contains `db` atoms.
48. `Store::open` at 10,000 definitions stays under 5 ms.
49. `ply prove` reports honest tiers and `ply hosts` lists the TCB, on the W4
    corpus, with the `database` block present under `--host`.
50. **No non-law definition's normalized bytes moved** across the
    `BODY_ENCODING` bump, over the whole W3 corpus.

Plus one `tests/fixtures/` entry per new code, as every milestone owes.

---

## Not in W4

- **Query building and ORMs.** No builder, no fluent API, no entity mapping, no
  lazy loading, no identity map. A statement is text and a row is a `Map`.
- **`LISTEN` / `NOTIFY`**, and every other connection-level asynchronous
  message. A notification arrives outside any perform, which means it arrives
  outside any row, and there is nothing in the effect system for it to be.
- **Replication, logical decoding, and read replicas.** A replica is a second
  resource with different consistency, and W4 has no vocabulary for that.
- **`COPY`**, in either direction.
- **Cursors and streaming result sets.** `query` materialises its rows. A result
  set larger than memory is a real limit and W6 is where a measurement would
  justify the machinery.
- **Migrations as a tool.** §7 states what exists instead.
- **Joins and aggregates in the twin.** §5.3 names them, and the twin refuses
  them rather than approximating them.
- **Isolation-level phenomena in the twin**, and therefore in the law. §6's
  claim is over sequential operation sequences only.
- **A date, time, timestamp or interval type.** §4.2 states the workaround and
  it is a workaround.
- **TLS to postgres.** §10.
- **Automatic retry of a serialization failure.** §1.5.
- **A test database per test.** §3.4. Footprint conflict grouping and `sandbox`
  are the whole of the isolation, and they are less than a reader expects.
