# 9. Handlers

A row is a demand: it says a function performs an effect. A **handler** is what
meets the demand. `handle { body } with { clauses }` runs `body` with a set of
answers in place, and the handle's own row is the body's row minus what the
clauses discharge, plus every clause's own row.

If a function's row says what it does to the world, a handler says what the world
does back. That is the whole idea, and it is what makes Ply tests hermetic
without any mocking.

## Clauses

A clause is `effect.op[resource](params) -> body`. A clause for a `read` or a
`write` answers the value the perform site receives:

```ply
effect log { write note[r](s: String) -> Unit }

fn greet(step: Int) -> String / {log.note[app]} = {
  log.note[app]("greet " ++ int_to_string(step));
  "hi"
}

test "a clause answers a perform" {
  handle {
    assert_eq(greet(1), "hi")
  } with {
    log.note[app](s) -> (),
  }
}
```

Clauses are tried in order, so a clause naming a specific label belongs before a
`[*]` clause that would answer it. A handler discharges an **operation** when it
has a clause for it, and a **mode atom** when it covers every operation of that
mode.

## The `return` clause

An optional `return x -> body` clause maps the handle's result:

```ply
test "a return clause maps the result" {
  let seen = handle { greet(1) } with {
    log.note[app](s) -> (),
    return v -> v ++ "!",
  };
  assert_eq(seen, "hi!")
}
```

`return` runs when the body finishes normally, not when a clause abandons it
(which is how a `raise` clause behaves — chapter 10).

## Coverage is checked

A `handle` must answer everything its body can perform on the atoms it handles,
and the diagnostic names the clause to add:

```text
Error[E0305]: this `handle` has no clause for `net.write[conn]`, which its body performs
  --> m.ply:5:12
   |   handle { talk() } with { net.send[conn](b) -> () }
   |            ^^^^^^ reaches `net.write[conn]` through `talk`
   --> m.ply:5:3
   |   handle { talk() } with { net.send[conn](b) -> () }
   |   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ handles `m.net.write[conn]` here, so the operation would run past it
   = `talk` may perform any `write` operation of `net`; a clause for each of them discharges `net.write[conn]`
   = fix: add a clause for `net.close[conn]`
```

A body that calls a function whose row names a **mode** atom may perform any
operation of that mode, so the handler must name each one. A body that calls a
function whose row names operations is judged by those.

The counterpart is `E0424`: a handler that is missing entirely, where the
operation reaches the host boundary at run time. That happens when a run has no
binding for the operation, and it is a run-time failure with a fix:

```text
   FAIL    no handler at all             0.1ms

   spend.no handler at all
     no handler for `spend.store.load[spend]`
       at spend.ply:10:8
     = wrap this in a `handle ... with { ... }` that names the operation
```

## `[*]`: answering every label

A clause's label may be `[*]`, which answers the operation on **every** resource:

```ply
effect log { write note[r](s: String) -> Unit }

fn all() -> Unit / {log.note[users], log.note[orders]} = {
  log.note[users]("a");
  log.note[orders]("b")
}

test "a [*] clause answers every label" {
  handle {
    all()
  } with {
    log.note[*](s) -> (),
  }
}
```

This is how a library serves an effect whose atoms are per-resource — a database
driver over tables, a trace sink over channels — where the resource is chosen at
the call site.

`[*t]` answers the same set of labels and **binds** the one the call site named
to `t`, which is in scope in the clause as a `String`:

```ply
effect tag { read label[r]() -> String }

fn read_tag() -> String / {tag.label[users]} = tag.label[users]()

test "a bound label is the one the call site used" {
  let seen = handle {
    read_tag()
  } with {
    tag.label[*t]() -> t,
  };
  assert_eq(seen, "users")
}
```

The label bound is the one the *call site* used, not the name of a label
parameter, so a clause answering `relay[users]("k")` binds `"users"` even when
the call site is inside a label-generic function. That is what lets a handler
check what it was asked rather than only answer it — a table check in a database
driver is a `match` over `t`.

A clause may either name a label or bind one, never both, and `[*]` on an
operation declared without a label is `E0304`.

## `[*]` in a row

A **row** may name `[*]` too. That is a licence rather than a demand: it says the
body may perform that operation on any label, and the function that states the
row answers it. It is how a library handles an effect for code it was given:

```ply
fn sink<a | e>(body: () -> a / {log.note[*] | e}) -> a / e =
  handle { body() } with { log.note[*](s) -> () }
```

```ply
test "a [*] row lets one handler serve every label" {
  sink(|| { log.note[users]("a"); log.note[orders]("b") });
  assert_eq(sink(|| 42), 42)
}
```

A body that performs nothing satisfies a `[*]` row; it may not perform an
operation the row does not name.

## `resume`: a continuation

A clause that writes `resume k` binds the continuation of the body at the
perform site. The clause then has the handle's type and may call `k` any number
of times:

```ply
effect amb { read flip[r]() -> Bool }

fn either() -> String / {amb.read[coin]} =
  if amb.flip[coin]() { "heads" } else { "tails" }

test "resume runs the body once per branch" {
  let seen = handle {
    either()
  } with {
    amb.flip[coin]() resume k -> k(true) ++ "/" ++ k(false),
  };
  assert_eq(seen, "heads/tails")
}
```

Each call to `k` re-enters the body from where it stood, with the answer passed
in. This is what makes a handler able to do more than reply: it can run the
continuation zero times (abandoning the body), once, twice, or after doing work
in between.

Without `resume`, a clause's value simply returns to the perform site. A clause
that binds `resume` and never calls it abandons the body — and that is where
`bracket` comes in, to release whatever the body held
([chapter 11](ch11-regions.md)).

`resume` is powerful and it has rules: a clause that binds `resume` is unreachable
from a task, and a continuation resumed twice across an at-most-once host
operation is `E0426`. The reference covers both (§6.6, §7, §9).

## Summary

- `handle { body } with { clauses }` answers operations. The handle's row is the
  body's minus what the clauses discharge plus the clauses' rows.
- A clause answers the operation's result type; `return` maps the body's result.
- Missing coverage is `E0305`, with the clause to add; a missing handler entirely
  is `E0424` at the host boundary.
- `[*]` answers every label; `[*t]` binds the label the call site used. A `[*]`
  row is a licence the function holding it answers.
- `resume k` binds a continuation the clause may call any number of times.

Next: failing. Ply has no exceptions, so how does a function say it can fail, and
how does a caller pick up the pieces?
