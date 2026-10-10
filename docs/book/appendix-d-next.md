# Appendix D. Where to go next

## The other documents

- **`docs/GUIDE.md`** is the language manual: every rule, with a diagnostic code
  for each. This book teaches the same language in the order you need it and
  leaves the exhaustive edges there. When you want the precise rule — how a row is
  counted, which conversions exist, what `--shard` does — read the reference.
- **`docs/DIRECTION.md`** is what Ply is for: perfect incrementality, signatures
  that say everything, checks that are static or cached, answers for machines, and
  compute buying confidence. It says what the language is shaped around and what
  that asks of it next.
- **`README.md`** describes the repository: the crates, how to build and test, and
  how the compiler bootstrap works.

## Ask the toolchain

Three commands answer most questions, and they read the source you are looking at:

```console
$ ply doc std.json             # a module's published surface, each with its doc
$ ply doc std.list.take        # one definition: type, doc, cost, tests, laws
$ ply doc prelude              # every builtin
$ ply explain E0201            # what a code means
$ ply check --types            # what the compiler inferred for your definitions
$ ply check --types --explain  # and, for each, where its body can raise and why
$ ply defs                     # every definition with its footprint
$ ply callers some_function    # what reaches it
```

`ply doc` prints the examples that matter: the `tests` and `laws` lines name the
definitions that pin a behavior, so a function's edge cases are one command away.

## Read programs

- **`examples/`** holds small, complete programs: `clock.ply` (a `nondet` effect
  and a handler), `ledger.ply` and `report.ply` (an `opaque` type, specs, laws),
  `pipeline.ply`, `bank.ply` and `timeout.ply` (simulation, a race and a fix),
  `echo.ply` and `hello.ply` (sockets and an HTTP endpoint), `orders.ply`
  (`derive json`), `relay.ply` (a label-generic forwarder), `shout.ply` (a command
  line declared with `std.cli`), `store.ply` (a handler as a capability grant),
  and `desk.ply` (a service over PostgreSQL or its in-memory twin, with TLS,
  tracing and shutdown).
- **`tests/lang/`** is the language's own suite: one directory or file per feature,
  each a set of tests that state the rule. It is the closest thing to a
  specification of the syntax.
- **`crates/ply-std/ply/`** is the standard library in Ply. Reading one module —
  `list.ply` or `result.ply` — is the fastest way to learn the house style: small
  pure functions, a `cost` clause where it matters, tests and laws beside the
  definitions.

## Where the language is going

The compiler, the standard library, the CLI and the registry are all Ply programs
under `crates/`. The direction document's list is the answer to "what next": the
features that earn their place are the ones that make a check static or cached, a
signature say more, or a claim stronger.

If you find a rule this book states wrongly or a program that no longer runs,
that is a defect in the book and worth fixing where it is: the reference is the
authority, and the examples in `examples/` and `tests/lang/` are checked.
