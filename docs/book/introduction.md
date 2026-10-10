# Introduction

Ply is a general-purpose, statically typed functional language. It has no loops,
no mutable variables, no classes, no exceptions and no method syntax. It makes up
for those absences with two ideas:

- **A function's type says what it does to the world.** If a function reads the
  clock, the fact is in its signature. If it can fail, the fact is in its
  signature. A caller never has to read a body to know.
- **Nothing is recomputed that an edit did not reach.** Every definition is
  content-addressed, so the compiler, the test runner and the build cache can tell
  what a change touched — not guess from a file's timestamp. You pay for what you
  changed, not for the size of the project.

This book teaches you to write Ply. It is a guide, not a reference: it walks
through the language in the order you will need it, with a small program that grows
across the chapters and runs at every step. Every complete program in this book was
compiled and run with the `ply` in this repository.

## Who this book is for

You can program already — in Rust, Go, Python, TypeScript, Haskell or anything
else. You do not need to know a functional language, and you do not need to know
Ply.

## How this book is organized

**Getting started** (chapters 1–2) gets a program running and shows the two
things that make Ply feel different: effects in the signature, and tests that
run only when they need to.

**The language** (chapters 3–12) is the core. Values and types, records and
sums, functions and pattern matching, collections, generics, effects, handlers,
errors, regions and concurrency.

**Confidence** (chapters 13–15) covers what Ply gives you besides the program:
tests, specifications and proof, and derivation.

**Programs** (chapters 16–20) is about everything that is not a single file:
modules and packages, files embedded at build time, the standard library, the host
boundary where effects meet the real world, and building and shipping.

**The capstone** (chapter 21) builds one real command-line tool from an empty
directory.

The **appendices** list the command surface, the diagnostic codes, what Ply
deliberately does not have, and where to look things up.

## The reference

[`docs/GUIDE.md`](../GUIDE.md) is the language manual. It states every rule and
names a diagnostic code for each one; this book teaches the same language in a
different order and leaves the exhaustive edge cases to it. When you want the
precise rule for something — how a row is counted, which conversions exist, what
`--shard` does — read the reference, or ask the toolchain:

```console
$ ply doc std.json
$ ply doc std.json.parse
$ ply explain E0201
$ ply check --types
```

`ply doc` reads the doc comments of the standard library and the builtins.
`ply explain CODE` prints one line about a diagnostic. `ply check --types` prints
what the compiler inferred for every definition in your program. Those three
commands answer most questions without opening a file.

## Conventions

A code block with a `//` comment on the first line names the file it goes in:

```ply
// hello.ply
fn main() -> Int = 42
```

A `console` block is a shell command; the `$` is the prompt, not something you
type.

```console
$ ply run
   42
```

Output is shown exactly as `ply` prints it, including the leading spaces it uses
to align its summaries.

Diagnostics are shown as the compiler prints them:

```text
Error[E0201]: type mismatch: function body type
  --> main.ply:1:20
   | fn main() -> Int = "forty-two"
   |                    ^^^^^^^^^^^ expected `Int`, found `String`
   compilation failed (1 error)
```

## A note on the examples

The chapters share one small program: a spending tracker that starts as a few
records and grows a store effect, tests, a JSON codec and a command-line entry
point. You are encouraged to type it in as you go rather than copy it. Short
snippets that illustrate one rule are shown on their own.

> **Try it.** After each chapter, change something in the chapter's program and
> run `ply check` and `ply test`. Ply's diagnostics point at a place and name a
> code; `ply explain CODE` tells you what it means. Building the habit of reading
> the diagnostic rather than the source is most of what makes Ply pleasant to
> use.
