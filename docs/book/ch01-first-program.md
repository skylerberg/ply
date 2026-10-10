# 1. Your first Ply program

This chapter gets a program running. Along the way you will meet the three
commands you will use constantly — `ply check`, `ply test` and `ply run` — and
see what `ply new` puts on disk.

## Building Ply

Ply is built from source. You need a Rust toolchain and a C compiler, because Ply
compiles a program to C and builds it before anything runs; the compiler itself
and the standard library are not Rust.

```console
$ git clone https://github.com/skylerberg/ply
$ cd ply
$ cargo build --locked --release -p ply-launcher --bins
$ cargo pack target/release/ply
```

`ply` is a small Rust launcher with the compiler, the standard library and the
`ply` command itself appended to it. `cargo pack` does that appending: it runs the
binary it is given to ask the shipped modules what they answer, and attaches the
answers. The result is a self-contained `target/release/ply`. Put it on your
`PATH`:

```console
$ export PATH="$PWD/target/release:$PATH"
$ ply --version
ply 0.1.0
```

> **Note.** If you edit anything under `crates/ply-std/ply` or
> `crates/ply-compiler/ply`, run `cargo pack target/release/ply` again. That is the
> whole rebuild for the parts of Ply written in Ply.

## A single-file program

Create a directory and one file, `main.ply`:

```ply
// main.ply
fn main() -> Int = 40 + 2
```

Run it:

```console
$ ply run
   42
```

`ply run` evaluates the definition named `main` and prints the value it returned.
The path is optional and defaults to the current directory, so `ply run` and
`ply run .` are the same thing.

`main` may have any type. Change it to return a `String`, or a record, and `ply
run` prints that value using the same `show` used everywhere else. `main` may
also take effect — the fact that it printed nothing to the world is what let this
first program run without a host; more on that below.

## `ply check`

`ply check` is the compiler's front end: it parses, resolves names, type-checks
and infers effect rows. It runs before `ply run` or `ply test` does anything, and
you can run it by itself.

```console
$ ply check
   checked 1 module, 1 definition, 0 tests
```

That summary is the quiet path. When something is wrong, you get a diagnostic
that names a code, a place, and, when there is one, a fix:

```ply
// main.ply
fn main() -> Int = "forty-two"
```

```text
Error[E0201]: type mismatch: function body type
  --> main.ply:1:20
   | fn main() -> Int = "forty-two"
   |                    ^^^^^^^^^^^ expected `Int`, found `String`
   compilation failed (1 error)
```

`ply explain E0201` prints one line about that code, and `ply check --types`
prints the type and effect row the compiler inferred for every definition — the
most useful command in the language when something is not what you expected:

```console
$ ply check --types
   checked 1 module, 1 definition, 0 tests

   main main.ply
     main : () -> Int
```

## A package, not just a file

A lone `main.ply` is a program, but real work lives in a package: a directory of
modules with a manifest. `ply new` writes the skeleton.

```console
$ ply new spend
   created spend · ply.pkg, main.ply
   next: cd spend && ply test
$ ls spend
main.ply  ply.pkg
```

`ply new spend` writes two files. `ply.pkg` is the manifest:

```ply
// ply.pkg — the package this tree is. `ply check`, `ply test` and `ply build` read it.

import std.pkg (Manifest)

fn package() -> Manifest = {
  name: "spend",
  version: { major: 0, minor: 1, patch: 0 },
  prefix: None,
  runtime: { major: 0, minor: 1, patch: 0 },
  dependencies: [],
  entry: None,
}
```

The manifest is data, not code: a literal that every command reads on load. The
name is what a dependent package will import it as, and the version is what a
registry will publish. Both are covered in [chapter 16](ch16-packages.md).

`main.ply` is the program:

```ply
// main.ply
import std.process (process)

fn main() -> Unit / {process.out[proc]} = process.out[proc]("hello from spend")

test "the first test" { assert_eq(1 + 1, 2) }
```

Two things are worth noticing before you run it.

First, `main`'s type is `() -> Unit / {process.out[proc]}`. The part after the
`/` is an **effect row**: it says that calling `main` performs `process.out`
against the resource `proc`. A function's type is a complete account of what it
does to the world, and this one writes to standard output. Chapter 8 is about
that.

Second, `ply new` also wrote a test, and tests are ordinary items in the module
next to the functions they test.

## Running a program with effects

Because the generated `main` writes to the world, a plain `ply run` refuses it.
A run with no host binds only simulated handlers — the ones you write yourself —
and there is no simulated stdout.

```console
$ cd spend
$ ply run
Error[E0424]: `std.process.process.out[proc]` reached the host boundary in a hermetic run
  --> main.ply:5:43
   |
 5 | fn main() -> Unit / {process.out[proc]} = process.out[proc]("hello from spend")
   |                                           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no handler here, and no host handler is bound
   |
   = a hermetic run binds simulated handlers and refuses the real ones
   = handle `std.process.process.out[proc]` in the program, or run with `--host`
   = `ply_host::process::out` would serve this under `--host`
   raised at main.ply:5:43
```

This is not an error in the program; it is Ply telling you that the program asks
the world for something and this run did not lend it a world. Pass `--host` to
bind the real handlers:

```console
$ ply run --host
   binding host · 1 operation · b3:e8b8b5d51557
   config      0 keys
hello from spend
   shutdown    signals INT TERM · lead 0ms · drain 30000ms · second signal exits 130/143
   ()
```

The lines around the greeting are the run reporting what it bound and how it will
shut down. The `()` at the end is the value `main` returned. Splitting a program's
world into pieces the caller hands it, rather than ambient authority, is what
makes a Ply test hermetic by default; chapter 19 covers the flags that lend the
host.

> **Try it.** Change `main` to return `()` and delete the `process.out` call. Run
> it with and without `--host` and notice that a program with an empty row runs
> the same either way.

## `ply test`

```console
$ ply test
   selected 1 of 1 (0 cached)
   1 group · 10 workers
   isolated 1 of 1

   ok      main.the first test           0.0ms

   backend c · 1 of 1 offers entered · 0 declined · 1 in the fragment
   compiled 1 unit(s) in 0.0ms, after 0.0ms deciding what to compile
   0 failed, 1 passed, 0 cached (0.00s)
```

Run it again without changing anything:

```console
$ ply test
   selected 0 of 1 (1 cached)
   isolated 1 of 1

   backend c · 0 of 0 offers entered · 0 declined · 0 in the fragment
   0 failed, 0 passed, 1 cached (0.00s)
```

The second run ran no test. Ply hashed the definition and the test, found a
recorded pass for exactly that hash, and took it. Edit the test's body and it runs
again; add a comment to it and it does not, because comments are not part of what
is hashed. Chapter 13 explains what a hash covers and what a cached pass is
allowed to assume.

## Where the work goes

The first command you run in a project creates `.ply-cache/` next to the
manifest. It holds the memo store — what each module of the project answered when
it was checked — and the store, which records the tests that passed and the
obligations that were discharged. It is safe to delete; the next run rebuilds it.
Add it to `.gitignore`:

```text
.ply-cache/
```

Nothing in the cache is machine-specific, and none of it is shared between
projects unless you ask for it: `PLY_CACHE_UPSTREAM=DIR` names a directory that
several checkouts or machines share, and this run both reads passes from it and
publishes its own. Chapter 13 covers that, and what a pass found there is allowed
to count for.

## Summary

- `ply check` type-checks and infers rows. `ply test` runs tests. `ply run`
  evaluates `main`.
- A program is a directory of `.ply` modules; a package adds a `ply.pkg` manifest.
- `ply new PATH` writes a package with a manifest, a `main` and a test.
- An effect row after `/` in a type says what a function does to the world. A run
  without `--host` binds only simulated handlers.
- Builds and test passes are cached by content. `.ply-cache/` is disposable.

In the next chapter you will write a program that declares its own effect, handle
it in a test, and see why that makes tests hermetic without any mocking library.
