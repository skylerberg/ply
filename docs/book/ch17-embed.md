# 17. Files embedded at build time

A program sometimes ships with data: a word list, a fixture directory, a SQL
schema. Ply reads those files when the program is **loaded**, folds their bytes
into what a definition hashes, and can compute a value from them once and keep
it.

## `embed` and `embed_dir`

```ply
fn schema() -> Bytes = embed("schema.sql")
fn fixtures() -> List<{ name: String, bytes: Bytes }> = embed_dir("fixtures")
```

`embed("path")` is the bytes of a file. `embed_dir("path")` is every file under a
directory by its path below it (`a/b.txt`), in that order; nothing under a name
starting with `.` is read. The path is a string literal (`E0147`), read relative
to the module's own file, and the call is written out as what was read before
anything hashes or checks the module. A path that does not exist, a directory
handed to `embed`, a file handed to `embed_dir`, or a file that cannot be read is
`E0146`:

```text
Error[E0146]: `nope.txt` could not be embedded
  --> d.ply:1:19
   | fn f() -> Bytes = embed("nope.txt")
   |                   ^^^^^^^^^^^^^^^^^ `nope.txt` does not exist
   = the path is read relative to the module's own file
```

A module that declares or imports its own `embed` or `embed_dir` calls that one.

## The bytes are part of the hash

Because the bytes are written into the definition before it is hashed, a test
that reads an embedded file is re-run exactly when the file changes, and is
cached while it does not:

```console
$ ply test
   selected 1 of 1 (0 cached)
   ok      an embedded file is read at build time      0.1ms

$ ply test
   selected 0 of 1 (1 cached)
   0 failed, 0 passed, 1 cached (0.00s)

$ # add a line to words.txt
$ ply test
   selected 1 of 1 (0 cached)
   FAIL    an embedded file is read at build time      0.1ms
```

That is the whole point of embedding over reading at run time: the file is an
input the cache knows about, so a test that depends on it is neither stale nor
needlessly re-run. The same property holds for `const fn` values: the hash covers
every file they embed.

## `const fn`: a value a build computes once

A `const fn` is a definition a build evaluates once and keeps the value of:

```ply
const fn words() -> List<String> =
  filter(string_split(string_of_bytes(embed("words.txt")), "\n"), |s: String| s > "")

fn first() -> String = match words() { [w, ..] -> w, [] -> "" }
```

A `const fn` takes no parameter, binds no type, label or row, and writes no row.
Its body may raise, and performs nothing else:

```text
Error[E0156]: `bad` is `const`, and its body performs `c.log.note`
   = a build evaluates a `const fn` before the program runs, where nothing answers an effect: its body may raise, and performs nothing else
   = read a file with `embed`, and handle any other effect inside the body
```

Its answer must be data, so no function, `Cell`, `Task`, `Chan` or `Secret` at any
depth (`E0156`). `pub const fn` publishes it; `const` goes with neither
`transparent` nor `reuse`.

`ply check`, `ply run`, `ply test`, `ply prove` and `ply build` evaluate each
`const fn` the program holds before they do anything else with the load, under a
budget of 100000000 calls. One that raises is `E0157`, saying what it raised and
where; one that spends the budget, or whose value takes more than 16777216 bytes,
is `E0158`.

A call of the definition reads the kept value and enters nothing of the body, so
it costs almost nothing:

```ply
const fn big() -> List<Int> = map(range(0, 100), |i: Int| i * i)

fn computed() -> List<Int> = map(range(0, 100), |i: Int| i * i)

test "reading a kept constant is nearly free" {
  let kept = metered(|| big());
  let made = metered(|| computed());
  assert_eq(kept.value, made.value);
  assert(kept.steps < made.steps)
}
```

A program no build kept the values of — a mutant, or the mixture a bisection runs
— evaluates the body in its place, once a run. The value is the same either way,
since the body is pure.

A value is kept as it is laid out, so a table that is compact reads quickly. An
`Array` holds a word an element, and an `Int` within 63 bits or a width below 64
is held in that word; any other element is an object made when the value is read.
A map is kept in its order and read back without comparing a key, and what a value
shares is kept once. That last detail is why `const fn statuses() ->
Array<Status>` is a better shape than a list of records for a lookup table.

## A schema

A `const fn` whose type is `std.sql.Schema` is a schema: the statements of the
module that declares it, and of every module that imports that one at any depth,
are checked against it before anything runs. A module whose statements reach two
schemas is `E0471`. The checker reads the statement text a call can compute —
text joined by `++`, a call of a definition on such values, or a tagged literal —
and holds it to the call, the table the label names, and the schema
(chapter 18).

## Data that ships with a module

A file below the standard library's `ply/` directory that is not a `.ply` is data
a shipped module embeds by its path from the module. It ships in every binary, and
a load reads it whenever it pulls the module. `ply std` lists each embedded data
file with the module that embeds it, and `ply std --show std/oid/names.txt` prints
one. That is how the standard library carries the tables it needs — a package's
own data belongs beside its modules under `embed`/`embed_dir`, where a test can
read it too.

> **Try it.** Move the word list out of the file and into a `const fn` as a list
> literal. Run `ply test` and notice the difference: the literal version has no
> file to change. Then put it back and decide which is better for your case —
> the file is editable without touching code, the literal cannot drift.

## Summary

- `embed("path")` and `embed_dir("path")` read files when the module is loaded,
  relative to the module's own file (`E0146`, `E0147`).
- The bytes are part of the definition's hash, so a test reading one re-runs
  exactly when the file changes.
- `const fn` is evaluated once at build and its value kept; a call reads the value
  (`E0156`–`E0158`).
- A `const fn` of `std.sql.Schema` is a schema the statements of its importers are
  checked against.
- A shipped module's data files travel with it and are listed by `ply std`.

Next: the standard library, and how to find your way around it.
