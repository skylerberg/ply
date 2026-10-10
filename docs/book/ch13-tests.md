# 13. Tests

A test is an item in a module, next to the functions it tests. Ply runs the tests
that changed and takes the rest from a record of earlier passes, so a large suite
stays fast as it grows. This chapter is about writing tests, what the cache is
allowed to assume, and the knobs for running many of them.

## Writing a test

```ply
test "a total counts only the category asked for" {
  handle {
    assert_eq(total(Food), 1200)
  } with {
    store.load[spend]() -> [{ what: "lunch", cents: 1200, category: Food }],
  }
}
```

A test is `test "label" { body }`. It is keyed `<module>.<label>`, so two tests
in one module may not share a label (`E0105`), and its body must be `Unit`: a test
that ends on a value, such as a comparison missing its `assert`, is `E0201`.

`assert(cond)` and `assert_eq(actual, expected)` fail with `E0501`;
`assert_eq` reports both values and their first difference. Both take an optional
message: `assert(cond, Some("why"))`.

A test may live in any module, including one that is not the entry point, and it
need not be declared or registered anywhere. `ply test` finds every one.

## Tests over cases

A test may range over a table, and is then one test per element — a **case**:

```ply
type Case = { input: String, expected: Int }

fn cases() -> List<Case> =
  [{ input: "", expected: 0 }, { input: "abc", expected: 3 }]

test "{c.input} is {c.expected} long" for c: Case in cases() {
  assert_eq(string_len(c.input), c.expected)
}
```

```console
$ ply test
   ok      t. is 0 long                                 0.3ms
   ok      t.abc is 3 long                              0.5ms
```

`for <name>: <Type> in <table>` stands between the label and the body. The table
is a `List<Type>`, usually a call of a definition that builds it, often from an
`embed_dir` (chapter 17). The name is bound in the body and in the label, and a
label is read as an interpolated string is, so its `{...}` holes make the case's
name.

Each case is a test of its own for selection: a case added to the table runs
alone, a reordered table or reworded label runs nothing, an edit to the body runs
every case, and an edit to what the table is built from runs the cases whose
values it changed. A case's type must be hashable (`derivable(hash, ·)` read
through no `key`), so it holds no `Float`, function, `Cell`, `Task`, `Chan` or
`Secret` (`E0206`).

## What a test costs

`metered(f)` answers `f()` with what it cost, in resources the runtime counts
rather than time: `steps`, `allocations`, `bytes`, and `performs` (each atom with
its count):

```ply
test "metered counts the calls a body makes" {
  let m = metered(|| sum_to(10, 0));
  assert_eq(m.value, 55);
  assert(m.steps > 0)
}
```

A cost is the same however often it is read and under either `--profile`, because
it is counted, not measured. A cost law (chapter 14) states how `steps` grows
with a size instead of pinning a number.

## Snapshots

A test may hold a rendering against a file the package stores, so it performs
nothing, is cached, and runs again when the stored file changes:

```ply
import std.snapshot
import std.snapshot (Stored)

fn stored() -> Stored = { dir: "snapshots", files: embed_dir("snapshots") }

test "an order renders as stored" {
  snapshot::check(stored(), "order.txt", snapshot::render(order()))
}
```

`check` fails with the unified diff from the stored text to the rendering, and
never writes a file. Storing is `snapshot::accept`, run by a person with the
directory lent through `--fs`; `ply doc std.snapshot` has the rest.

## Determinism

A test whose row, after handling, still holds a `nondet` atom is `E0412`, because
its verdict would depend on the clock or the network:

```text
Error[E0412]: nondeterministic effect in a deterministic test
  --> t.ply:3:34
   | test "unhandled nondet" { assert(tick() >= 0) }
   |                                  ^^^^^^ reaches `t.clock.read`, and `t.clock` is declared `nondet`
   = handle it here, e.g. `handle <body> with { clock.now() -> <value> }`
```

The two fixes are to handle the effect, or to declare the test `test/nondet`. A
`test/nondet` is cached like any other test — what it read of the clock or the
network is no part of its pass — but it says that the nondeterminism is intended.
It still needs the effect answered at run time, so in practice it is almost
always paired with a `handle`.

## What a cached pass means

A definition's hash covers its normalized form: names, comments, formatting,
imports, `pub`, specs and test labels are erased, and references are replaced by
their referent's hash. A test runs exactly when neither its hash nor the code it
compiles to has a recorded pass that still stands. So a rename or a comment edit
runs nothing:

```console
$ ply test
   selected 1 of 4 (3 cached)
   ok      t.metered counts the calls a body makes      0.1ms
   0 failed, 1 passed, 3 cached (0.00s)

$ # reword the test's label and run again
$ ply test
   selected 0 of 4 (4 cached)
   0 failed, 0 passed, 4 cached (0.00s)
```

A pass is filed with what its run read of the world: each file a handler read
under a root, each shipped module it asked for, and every `ply` it started in
turn. It stands while each of those still answers as it did, and otherwise the
test runs again. A read of something the test wrote first is not an input, and
neither is the clock, the network, or what a run keeps for the next. That is why
`--no-cache` exists: to bypass both the result and the memo store.

A pass found in a shared `PLY_CACHE_UPSTREAM` counts too. Entries are keyed by
content and by the shape of what is stored, so nothing machine-specific is shared;
a pass is believed only by a `ply` whose runtime is the same and whose relevant
definitions hash as they did.

Selecting a subset:

- `--filter SUBSTRING` matches `<module>.<label>`; repeat it to run everything any
  of them matches.
- `--shard K/N` runs part `K` of `N`, parted by a hash of each key, so a test is
  in the same part every run.
- `--explain` says why each test was selected and names, for a changed one, the
  read that moved.

## Running many tests

Tests whose **footprints** do not conflict run concurrently, on one thread per
core. A test whose effects are all discharged in a region conflicts with nothing.
A raise is in no footprint, since the run answers it as the test's failure.

```console
$ ply test
   selected 4 of 4 (0 cached)
   1 group · 10 workers
   isolated 3 of 4 · 1 test can contend
```

A test that retains a footprint atom (the `test/nondet` above, for instance) is
the one that "can contend" and runs in the contended group. `--jobs N`/`-j` deals
tests into `N` lanes instead.

Two bounds apply to each test:

- `--steps N` is the calls it may make (default a billion; `0` is no bound). A test
  past it fails with `E0503`, which is a program error and is recorded as one,
  because the count is a property of the program.
- `--timeout MS` is the wall clock it may take (default 60000; `0` is no clock).
  It is not a verdict: a test past it is *abandoned* (`W0612`), reported apart from
  the failures, recorded nowhere, and run again next time. A run with an abandoned
  test is not a success.

A failing deterministic test that has passed before is bisected over the
definitions that changed to name a culprit. `--bisect auto|always|never` and
`--bisect-budget N` control this, and `--json` prints each failure's diagnostic,
suspects, culprit and a replay command. `--watch` re-runs on every `.ply` change.

## Beyond the basics

- `--coverage` reports, from the hash closure alone, which tests reach each
  definition and which definitions no test reaches.
- `--mutate [DEF]` changes one definition one operator or literal at a time and
  runs the tests that reach it. A mutant every one of them passes is a survivor,
  reported with its place and the tests that let it through, and fails the run.
- `--std` includes the standard library's own tests; `--workspace` runs the
  command for every package a path dependency reaches.
- `--profile development|release` picks the C toolchain. Compiled code is the only
  evaluator, so every command compiles; `PLY_C_CACHE`, `PLY_C_STAGE` and the rest
  of the backend's variables are listed in the reference (§8.6) and in the help.

> **Try it.** Add a test that would fail, run `ply test --json`, and read the
> `suspects` and `culprit` fields. Then fix the body and run again — the pass is
> filed under the new hash, and the next run takes it back without checking.

## Summary

- `test "label" { body }`, keyed `<module>.<label>`, may live in any module and
  must return `Unit`.
- `for x: T in table` makes one case per element, each selected as its own test.
- `metered` counts steps, allocations, bytes and performs. Snapshots compare a
  rendering against a stored file.
- A `nondet` atom needs a handler (`E0412`); `test/nondet` declares the
  nondeterminism intended.
- A pass is cached under hashes and the world it read. Renames and comment edits
  run nothing; `--filter`, `--shard` and `--no-cache` control selection.
- Tests run concurrently by footprint; `--steps` fails a test, `--timeout`
  abandons it.
- `--coverage`, `--mutate`, `--std` and `--workspace` extend a run.

Next: stating what a function promises, and letting the prover try to hold you to
it.
