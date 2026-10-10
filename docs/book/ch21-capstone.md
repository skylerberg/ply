# 21. A command-line tool, end to end

This chapter builds one small tool from an empty directory: `spend`, which totals
a spending file by category. It uses effects, tests, a law, a build and a
capability. It is the whole book in one program.

The program is written out in full below, one piece at a time, and the outputs
shown are its own, run with the `ply` in this repository. Type it into
`main.ply` in an empty directory as you read.

## What it does

```console
$ ply run --host --fs data=. -- spend.csv
   binding host · 5 operations · b3:5eddae0d92a8
   config      0 keys
   shutdown    signals INT TERM · lead 0ms · drain 30000ms · second signal exits 130/143
food: 2100
transport: 300
rent: 120000
other: 0

   ()
```

The file is comma-separated, one expense a line: a name, an amount in cents, and
a category.

```text
lunch,1200,food
bus,300,transport
rent,120000,rent
lunch,900,food
```

## The types

```ply
type Category = | Food | Transport | Rent | Other

type Expense = { what: String, cents: Int, category: Category }

fn categories() -> List<Category> = [Food, Transport, Rent, Other]

fn name_of(c: Category) -> String = match c {
  Food -> "food",
  Transport -> "transport",
  Rent -> "rent",
  Other -> "other",
}

fn category_of(text: String) -> Option<Category> = match text {
  "food" -> Some(Food),
  "transport" -> Some(Transport),
  "rent" -> Some(Rent),
  "other" -> Some(Other),
  _ -> None,
}
```

`Category` is a sum, so a category that is not one of the four is not
representable. `category_of` is the one place text becomes a category, and it
answers `Option`, so a file with an unknown category is a line to skip rather than
a value to trust.

## Parsing a line

```ply
/// One `what,cents,category` line of the file, or `None` for anything else.
fn parse_line(line: String) -> Option<Expense> / {abort.raise} = {
  let [what, cents, kind] = string_split(line, ",") else { None };
  let d = decimal_of_string(string_trim(cents))?;
  let n = int_of_decimal(d, Down)?;
  let category = category_of(string_trim(kind))?;
  Some({ what: string_trim(what), cents: n, category: category })
}
```

Four things happen here, and each is a moment from the book:

- `let [what, cents, kind] = ... else { None }` takes a list of exactly three
  parts apart, and `None` is the block's value when the list is another length
  (chapter 5).
- `?` on `decimal_of_string(...)` exits the function with `None` when the amount
  is not a number, because `parse_line` returns `Option` (chapter 10).
- `int_of_decimal(d, Down)` takes the amount as a whole number, so `12.50` is
  accepted as 12 rather than silently truncated by a cast.
- `category_of` is the fallible part, and it is the same `Option` machinery.

The row is `/ {abort.raise}` because `string_split` raises on an empty separator —
not here, but the type does not know that until it is checked (chapter 8).

## Totals and the report

```ply
fn total_of(expenses: List<Expense>, category: Category) -> Int =
  fold(expenses, 0, |sum: Int, e: Expense|
    if e.category == category { sum + e.cents } else { sum })

fn report(expenses: List<Expense>) -> String =
  join(
    map(categories(), |c: Category| name_of(c) ++ ": " ++ int_to_string(total_of(expenses, c))),
    "\n",
  )
```

`total_of` walks the list with `fold`; `report` maps each category to a line and
joins them with `join` from `std.string`. Both are pure, so both are trivial to
test.

## Reading the file

```ply
import std.fs (fs)
import std.io

fn read_file(path: String) -> Bytes
  / {fs.open[data], fs.read_chunk[data], fs.close[data], io::io.refused, io::io.closed, io::io.too_long, abort.raise}
= with_hold[data](io::reading[data](path), io::shut) { source ->
    io::read_all(source, 65536, 1000000)
  }

fn load(path: String) -> List<Expense>
  / {fs.open[data], fs.read_chunk[data], fs.close[data], io::io.refused, io::io.closed, io::io.too_long, abort.raise}
= {
  let lines = filter(string_split(string_of_bytes(read_file(path)), "\n"), |l: String|
    string_len(l) > 0);
  fold(lines, [], |acc: List<Expense>, line: String|
    match parse_line(line) { Some(e) -> push(acc, e), None -> acc })
}
```

`with_hold` is the bracket: `io::reading[data](path)` opens the file,
`io::shut` closes it, and the `with_hold` runs `release` however the body ends
(chapter 11). `data` is a resource **label**, and it names a capability, not a
path: which directory `data` is is decided where the run is configured, never
here (chapter 19).

`load` reads, splits on newlines, drops blank lines, parses what it can and skips
what it cannot. Skipping silently is a choice; a real tool would count the lines
it dropped and report them, which is the first thing to add.

## The entry point

```ply
import std.process (process)

fn main() -> Unit
  / {process.args[proc], process.out[proc], fs.open[data], fs.read_chunk[data], fs.close[data],
     io::io.refused, io::io.closed, io::io.too_long, abort.raise}
= match process.args[proc]() {
    [path, ..] -> process.out[proc](report(load(path)) ++ "\n"),
    _ -> process.out[proc]("usage: spend FILE\n"),
  }
```

`process.args[proc]()` is what the run passed after `--`. `main` writes through
`process.out`, so running it needs `--host`.

## Tests and laws

```ply
test "a line parses only when it is one" {
  assert_eq(parse_line("lunch,1200,food"), Some({ what: "lunch", cents: 1200, category: Food }));
  assert_eq(parse_line("  bus , 300 , transport "), Some({ what: "bus", cents: 300, category: Transport }));
  assert_eq(parse_line("nonsense"), None);
  assert_eq(parse_line("x,y,z"), None);
  assert_eq(parse_line("x,10,teleport"), None)
}
```

```ply
law "the category names are distinct"
  forall (a: Category, b: Category) where a != b { name_of(a) != name_of(b) }

law "one expense counts toward its own category and no other"
  forall (e: Expense, c: Category) {
    total_of([e], c) == if e.category == c { e.cents } else { 0 }
  }
```

```console
$ ply test
   ok      main.a line parses only when it is one      0.1ms
   1 passed

$ ply prove
   2 obligations · 1 proved · 1 property · 0 example   (0.02s)

   ✓ proved      law "the category names are distinct"    propositional · case analysis over main.Category (4 arms) · congruence · injectivity · 1 unfolding · 62 steps
   ✓ property    law "one expense counts toward its own category and no other" 200 cases · 0 rejected
```

The first law is `proved` — four constructors, so the checker splits on all of
them. The second is `property`: `fold` over a list is outside the decidable
fragment, so the prover draws 200 expenses and finds the claim true. Both are
green, and the reports say why, which is the point.

The test needs no handler and no host: `parse_line` reads nothing. A test of
`load` or `main` would, and would answer `fs` and `process.out` with a `handle`
of its own (chapters 2 and 9) or run under `--host`.

## Checking and running it

```console
$ ply check --types
   checked 15 modules, 17 definitions, 1 test

   main main.ply
     categories  : () -> List<main.Category>
     name_of     : (main.Category) -> String
     category_of : (String) -> Option<main.Category>
     parse_line  : (String) -> Option<{category: main.Category, cents: Int, what: String}>
                   / {abort.raise bounded}
     total_of    : (List<{...}>, main.Category) -> Int
     report      : (List<{...}>) -> String
     read_file   : (String) -> Bytes
                   / {abort.raise scaling, std.fs.fs.close[data] scaling, ...}
     load        : (String) -> List<{...}>
                   / {abort.raise scaling, std.fs.fs.close[data] scaling, ...}
     main        : () -> Unit
                   / {abort.raise scaling, ..., std.process.process.args[proc] bounded,
                      std.process.process.out[proc] bounded}
```

Read the counts. `parse_line` raises a **bounded** number of times: it is one
call, at most. `read_file`'s `fs` atoms are **scaling**: a file is read in as many
chunks as its size needs. `report` is `(List<Expense>) -> String` with nothing
after the arrow — a pure function, and the checker says so.

```console
$ ply run --host --fs data=. -- spend.csv
   binding host · 5 operations · b3:5eddae0d92a8
   config      0 keys
   shutdown    signals INT TERM · lead 0ms · drain 30000ms · second signal exits 130/143
food: 2100
transport: 300
rent: 120000
other: 0

   ()
```

Run it without the root and the capability boundary answers at the operation, not
at start-up:

```console
$ ply run --host -- spend.csv
   = bind one beside the run: `--fs data=<directory>`
```

## Building it

```console
$ ply build
   built main.main · b3:c54be1bbdabc
   artifact 39 definitions · 158.1 KiB · main.plyx
   binary ply 0.1.0 · 80.2 MiB
   startup none — this artifact cannot be run with `--config-schema`

$ ply run main.plyx --host --fs data=. -- spend.csv
   binding host · 5 operations · b3:5eddae0d92a8
   config      0 keys
   program b3:c54be1bbdabc · 39 definitions
   shutdown    signals INT TERM · lead 0ms · drain 30000ms · second signal exits 130/143
food: 2100
transport: 300
rent: 120000
other: 0
```

Thirty-nine definitions, because the closure carries the filesystem and IO
definitions `load` reaches. The artifact runs no front end; the run starts by
loading a compiled unit.

## What comes next

Three things would turn this into a tool someone would use, and each is a chapter
of this book:

- **A declared command line.** `std.cli` (chapter 18) turns a `Spec` into the
  parser, the `--help`, the completions and the manual page, so none of them can
  drift from another. `examples/shout.ply` in this repository is a worked example.
- **Configuration instead of a positional path.** `config.get` plus
  `--config-schema` (chapter 19) makes a missing setting a start-up failure that
  names the key.
- **A real error for a bad line.** `load` currently skips what it cannot parse.
  Returning a `Result` with the line number, or counting the skips and reporting
  them on stderr, is the difference between a script and a tool.

The pieces to do all three are the ones you have already used.

## Summary

- The program combines sums and records, `?` and `Option`, `fold` and `map`, a
  held resource, a capability label, a test and two laws.
- `ply check --types` tells you what it inferred, including how often each effect
  happens.
- `ply test` needs no handler for pure code; `ply prove` reports each law's tier
  and why.
- `ply run --host --fs NAME=DIR` binds a filesystem root to a label, and
  `ply build` closes it into an artifact.

You have reached the end of the guide. `docs/GUIDE.md` is the reference for every
rule this book taught in passing, and `ply doc`, `ply explain` and
`ply check --types` are the tools for the questions it left out.
