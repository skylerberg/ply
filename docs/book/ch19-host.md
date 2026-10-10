# 19. The host boundary

An effect is answered by a handler. When no handler in the program answers it, the
operation reaches the **host boundary** — the point where the runtime would have
to do it for real. This chapter is about that boundary: why a run refuses it by
default, and how to lend a program a world on purpose.

## Hermetic by default

A run with no `--host` binds simulated handlers only. An operation nothing
answers is refused, naming the operation and the fix:

```text
Error[E0424]: `std.process.process.out[proc]` reached the host boundary in a hermetic run
  = a hermetic run binds simulated handlers and refuses the real ones
  = handle `std.process.process.out[proc]` in the program, or run with `--host`
  = `ply_host::process::out` would serve this under `--host`
```

This is the default because a test's verdict has to depend on the program, not on
the machine. A run that reads the clock, the network or a file is a run whose
answer moves when those move, so Ply makes you ask for it. Chapter 1 met this the
first time it ran `ply new`'s generated program.

## `--host` and `ply hosts`

With `--host`, the binary binds a handler for every operation the program reaches
that it can serve.

```console
$ ply hosts
   hermetic — no host handler is bound

   3 operations would bind under `--host`; run `ply hosts --host` to list them

$ ply hosts --host
   152 host handlers · 3 operations · trusted computing base

   OPERATION                   HANDLER                   DET  LINEAR        BLOCKING  SECRETS
   std.fs.fs.close[data]       ply_host::fs::close       no   at-most-once  yes       no
   std.fs.fs.open[data]        ply_host::fs::open        no   at-most-once  yes       no
   std.fs.fs.read_chunk[data]  ply_host::fs::read_chunk  no   at-most-once  yes       no
```

`ply hosts` is the list of what this binary can do on a program's behalf — its
trusted computing base — with each operation's handler, whether it is
deterministic, whether it is at-most-once or repeatable, whether it blocks, and
whether it may take a `Secret`. `ply hosts --digest` pins the list for a CI
check.

## Lending a world

The host flags are all `--host` flags; each is repeatable where it makes sense.

| flag | lends |
| --- | --- |
| `--fs NAME=PATH` | a filesystem root: `std.fs` files under `NAME` live below `PATH` |
| `--tls NAME=CERT,KEY` | a TLS credential a listener serves with |
| `--trust CERT.pem` | a certificate a client trusts, and one a listener verifies a client's against |
| `--mtls NAME` | a `--tls` credential whose listeners require a client certificate |
| `--exec NAME=PATH` | a program `process.spawn` or `process.start` may run |
| `--allow NAME` | a privileged family (below) |
| `--set KEY=VALUE` | a configuration value |
| `--config PATH` | a `KEY=VALUE` file, one pair per line |
| `--config-schema MODULE.FN` | a `ConfigSpec` the run's configuration is checked against |
| `--trace json\|text\|off`, `--trace-level LEVEL` | a trace sink and its lowest level |
| `--drain-lead-ms MS`, `--drain-ms MS` | how a stop is spread out |

A handler for an effect the program does not declare, or for a resource it never
performs, is `E0421`; two for one atom `E0422`; a determinism mismatch `E0423`.

## Filesystem roots and confinement

A resource label is a capability: what a filesystem operation may reach is named
where the run is configured, never in the program. So the same code reads one
directory under one label and another under another:

```ply
import std.fs (fs)
import std.io

fn read_file(path: String) -> Bytes
  / {fs.open[data], fs.read_chunk[data], fs.close[data], io::io.refused, io::io.closed, io::io.too_long, abort.raise}
= with_hold[data](io::reading[data](path), io::shut) { source ->
    io::read_all(source, 65536, 1000000)
  }

fn main() -> String
  / {fs.open[data], fs.read_chunk[data], fs.close[data], io::io.refused, io::io.closed, io::io.too_long, abort.raise}
= string_of_bytes(read_file("greeting.txt"))
```

```console
$ ply run --host --fs data=.
   binding host · 3 operations · b3:92064d946a9a
   config      0 keys
   "hello from a file\n"
```

Run it without the root and the failure is at the operation, with the fix:

```console
$ ply run --host
   = bind one beside the run: `--fs data=<directory>`
   = a resource label is the capability: what a filesystem operation may reach is named where the run is configured, never in the program
```

A root that does not resolve is `E0454` before anything runs; a path that leaves
its root is `E0452`; an operation on a label with no root is `E0451`. Every root
is resolved once, and the resolved path is what a confinement check is against —
which is why a symlink cannot walk out of a root.

## Configuration

`--set KEY=VALUE` is the highest precedence, then `--config` files (a later file
wins), then the environment, then defaults. `std.config` reads values as an
effect, so a program that needs configuration performs it and a test handles it:

```ply
fn port() -> Int / {config.get[server], abort.raise} = setting("PORT", 8080)
```

`--config-schema MODULE.FN` names a nullary pure function returning a `ConfigSpec`,
checked at start-up: a missing key is `E0441`, a bad value `E0442`, an undeclared
key `W0607`. With a schema, a wrong configuration is a start-up failure naming the
key rather than a `None` at a call site later.

## Capability families

`--allow NAME` lends a privileged family, and the program must declare the effect
it lends:

| name | lends |
| --- | --- |
| `machine` | load, bind, enter and call a nested program |
| `tester` | the test runner's own powers |
| `claims` | effect `prover` |
| `hosts` | effect `tcb` |
| `shipped` | the modules, version, C runtime and builtins this binary ships, and `reached` |

`machine`, `tester`, `claims` and `hosts` also lend a deterministic `hermetic_`
half of the same operations, which answers from what it is handed alone — no host,
clock, file or cache — so a test that uses one stays cached. `machine` drives
another machine, so it is granted on purpose and not by accident, which is the
point of naming a family rather than a bool.

## Caching a run that touched the host

A test that can reach a bound nondeterministic handler is cached with **what it
read and the binding it ran under**, so its pass never answers for a hermetic run.
A test that reaches only deterministic handlers — whose answers are a function of
what they are handed, as `std.password`'s hashes are — is cached like any other.

`std.signal`, `std.process` and `std.term` are bound only by `ply run --host`;
`ply test --host` withholds them (`E0424`), except that a test run binds
`process.bound`, `process.spawn`, `process.start` and the operations on a started
child, which reach only the programs `--exec` names. A test run does not get to
signal the test runner.

Outside a `simulate` region, `--host` answers the language's `task` and `clock` as
well: tasks run in turn on the thread that spawned them, and one that waits is
parked alone while the others run; `clock.now` reads the run's monotonic clock and
`clock.sleep` is a deadline on it (chapter 12).

## Stopping

At a stop the listeners are closed, so a parked `net.accept` answers `0`. That is
what `std.http`'s serve loops drain on: they accept nothing more, answer every
connection already accepted with `Connection: close` on its last response, and
close the idle ones. A connection that speaks HTTP/2 is drained with `GOAWAY`
after the streams the client opened are answered. Before the stop the server sheds
load, answering `503` with `Retry-After` to a connection that finds its queue full
or waited too long.

`--drain-lead-ms` keeps accepting after the signal so a readiness route can answer
`503`; `--drain-ms` is how long in-flight requests then have to finish. Expiry is
`W0608` and exit `3`.

> **Try it.** Take a function that reads a file and run it three ways: with a
> handler you write in a test, with `--host --fs data=DIR`, and with `--host` and
> no root. The first is how the file's contents become part of the test's inputs;
> the second is how the program ships; the third is the capability boundary doing
> its job.

## Summary

- A run without `--host` binds only simulated handlers; an operation nothing
  answers is `E0424`.
- `ply hosts` lists what the binary can bind, with determinism, linearity,
  blocking and secret permission; `--digest` pins it.
- `--fs NAME=PATH` gives a filesystem root per resource label; roots are resolved
  once and confine every path (`E0451`, `E0452`, `E0454`).
- Configuration comes from `--set`, `--config`, the environment and defaults;
  `--config-schema` checks it at start-up.
- `--allow` lends a named capability family; the deterministic `hermetic_` half
  keeps a test cached.
- A test that reached a nondeterministic handler is cached with what it read and
  the binding; a stop closes listeners and drains in-flight work.

Next: turning the program into an artifact someone else can run, and publishing a
library someone else can depend on.
