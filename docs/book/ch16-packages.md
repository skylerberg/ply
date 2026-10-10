# 16. Modules, packages and dependencies

Everything so far fits in one file. This chapter is about many files, about a
directory as a package with a name and a version, and about depending on someone
else's package without letting it depend back on you.

## Files are modules

Every `*.ply` file under the project root is a module, named by its relative path
with `/` turned into `.` and `.ply` dropped:

| file | module |
| --- | --- |
| `main.ply` | `main` |
| `store/orders.ply` | `store.orders` |
| `store/orders/place.ply` | `store.orders.place` |

Every directory name and file stem must be an identifier (`E0111`). Directories
whose name starts with `.` are not walked. `std` and `compiler` are the built-in
packages, pre-seeded for every load, so a module named under either collides with
its prefix.

Naming a **single file** (`ply check main.ply`) makes its parent the root and
loads only that file — which is why a multi-module check takes the directory.

Definitions may refer to each other and recurse across the whole program; there is
no declaration order and no forward-declaration problem.

## Imports and visibility

```ply
import store.orders                 // binds the module as `orders`
import store.orders as ord          // binds it as `ord`
import store.orders (place, cancel) // binds those names, no module binder

orders::place(...)                  // reach through a binder
```

`as` and a name list cannot be combined; write two imports. Imports precede every
item in a file.

Items are private unless `pub`, and `pub` applies to `fn`, `type`, `effect` and
`law schema` only. Values (functions, constructors, schema-declared definitions),
types, effects and module binders are **separate namespaces**, so `fn size`,
`type Size` and `effect size` coexist.

A `pub type` exports a sum's constructors with it, so another module can build
and match the values. To keep that in the declaring module, write `opaque`:

```ply
// token.ply
pub opaque type Token = | Token(String)

pub fn mint(s: String) -> Option<Token> = if s == "" { None } else { Some(Token(s)) }
pub fn text_of(t: Token) -> String = match t { Token(s) -> s }
```

Another module names the type, holds and passes its values and calls its
functions, and is refused what would make a value or take a sum apart:

```text
Error[E0107]: `Token` is a constructor of the opaque type `token.Token`
  --> bad.ply:2:21
   | fn bad() -> Token = Token("x")
   |                     ^^^^^ only `token` builds or matches one
   = `token` answers one from `mint`
   = `token` reads one with `text_of`
```

The diagnostic names the functions the module publishes for the type, which is
the pattern: an opaque type's module is its only way in.

`opaque` is not secrecy. `==`, `compare`, `digest`, `show` and `reflect` read a
whole value as they do any other, through a `key` and a `show` where the module
states them — and a `Secret` is the type nothing reads (chapter 4). A record
literal where an `opaque` record is expected is `E0220`, and a `forall` over a
type that holds an opaque value is `E0418` unless the module states a `gen`.

An alias has no values of its own to keep, so `opaque` on one is `E0221`. Unlike
`pub`, `opaque` is part of a type's hash, because it decides what a definition
that never names the type may write.

## The manifest

A project root may hold a `ply.pkg`: exactly one definition whose body is a
literal of `std.pkg`'s `Manifest`:

```ply
// ply.pkg
import std.pkg (Manifest)

fn package() -> Manifest = {
  name: "root",
  version: { major: 0, minor: 1, patch: 0 },
  prefix: None,
  runtime: { major: 0, minor: 1, patch: 0 },
  dependencies: [],
  entry: None,
}
```

- `name` is what a dependent imports it as. A package name is lower-case letters,
  digits and `_`, joined by dots.
- `version` is a semantic version. A registry publish is held to what it changes
  (chapter 20).
- `prefix` is the module root importers reach the package through; `name` when
  `None`.
- `runtime` is the toolchain the package builds with.
- `dependencies` is a list of `Dep` records (below).
- `entry` names the definition `ply build` closes over; `main` when `None`.

The body is data, not code: a literal, a constructor over literals, a record or a
list. Anything else is `E0130`, a manifest of another shape is `E0129`, and a
field that does not decode or fails validation is `E0131`. A project without
`ply.pkg` is the anonymous package.

## Dependencies

A dependency entry is a record naming where its sources come from:

```ply
import std.pkg (Manifest)

fn package() -> Manifest = {
  name: "root",
  version: { major: 0, minor: 1, patch: 0 },
  prefix: None,
  runtime: { major: 0, minor: 1, patch: 0 },
  dependencies: [
    { name: "money", min: { major: 0, minor: 0, patch: 0 }, prefix: None, source: Path("../lib") },
  ],
  entry: None,
}
```

```ply
// main.ply
import money.amount (Cents, add)

fn main() -> Int = add({ n: 1 }, { n: 2 }).n
```

A `Path("../lib")` dependency names the directory its `ply.pkg` stands in. The
dependency must hold one (`E0135`), and its modules answer to
`<prefix>.<module>`: the package `money` above is imported as `money.amount`.
`min` is a **floor**, not a pin. A `Git(url, rev)` dependency is fetched into the
project's own cache and read like any other package root; `rev` may be a commit, a
tag or a branch, and a branch means what it means the day it is fetched. A
`Registry` dependency is the package of that `name` from the registry
`PLY_REGISTRY` names, at least `min` (chapter 20).

Three rules make the boundaries real:

- Each dependency grants only its own prefix. Reaching a package the manifest does
  not declare is `E0132`; two packages granting one prefix is `E0133`, as is a
  module of the root package squatting on a dependency's prefix.
- A bare import is always **its own package's**. Inside a dependency, `import fmt`
  names *its* `fmt`, never the importing package's: a package cannot reach back
  into what imports it.
- A command acts on the **root package**, the one whose tree it was given. A
  dependency's `main` is no entry point; `ply test` runs the root package's tests
  and never a dependency's, which are that package's own to run.

That last rule is what keeps a dependency's tests, proofs and promises its own:
its definitions are read by their written rows and specifications, and its body is
its own run's to check (chapter 14).

## Resolution and the lockfile

Resolution is **minimal version selection**. Two manifests may ask different
floors of one package — the highest floor wins — and a dependency below its
importer's floor is `E0136`. One version of a package serves a whole closure, so a
package of one name at two places is `E0137`, naming both requesters and both
paths, while a package two others both depend on is an ordinary diamond and
resolves to the one version they agree on. The order the manifests are read in
changes nothing.

`ply build` records what it resolved in `ply.lock`, beside `ply.pkg`:

```console
$ ply resolve
   resolved 1 package · ply.lock
     money 0.1.0 · b3:3ff9b52d6953bd9a493e1589ccbfca55cba6150796d7ccb84c620b7edd69e411
```

The lock lists every dependency's name and version, sorted by name, with the
BLAKE3 digest of the modules it contributed and what they embed. A path dependency
**inside the checkout** the project is in is recorded without a digest, as Cargo's
lock does: the commit that holds the project holds its sources, so an edit to it
leaves the dependent's lock as it was. A git or registry dependency, a path one
outside the checkout, and everything a project outside a checkout has keep their
digest.

A build verifies the lock before it writes an artifact: a dependency whose digest
no longer matches its sources is `E0138`, and a lock this `ply` cannot read is
`E0139`. `ply resolve` pins what is on disk now and is how a change to a
dependency is accepted, deliberately.

`ply why NAME` says how a package got here:

```console
$ ply why money
   root -> money
     0.1.0 · b3:3ff9b52d6953bd9a493e1589ccbfca55cba6150796d7ccb84c620b7edd69e411
```

## Many packages at once

- `ply vendor` copies the closure into `vendor/`, one directory per package plus
  an index, whole — its `ply.pkg`, its modules and the data it ships, but not the
  repository a fetch came from. A vendored checkout builds with no cache, no
  network and no git.
- `--workspace` (`ply check`, `ply test`, `ply prove`) runs the command for every
  package the path reaches by a **path** dependency, each as its own root with its
  own cache, dependencies first. A fetched or vendored dependency is never one.

```console
$ ply check . --workspace
   /private/tmp/b45/lib
   checked 1 module, 1 definition, 0 tests

   .
   checked 2 modules, 2 definitions, 0 tests
```

## The cache

`.ply-cache/` at the root holds the memo store — what each module of the project
answered when it was checked — the store (tests' passes and baselines, discharged
obligations, review baselines), and the git and registry dependencies that were
fetched. `vendor/` holds the ones `ply vendor` copied. It is safe to delete; add
it to `.gitignore`.

`PLY_CACHE_UPSTREAM=DIR` names a second cache shared between checkouts and
machines — a directory on any storage they all reach. The passes and discharged
obligations found there count here, and this run's are published there
(`PLY_CACHE_UPSTREAM_READONLY=1` reads only). Entries are keyed by content and by
the shape of what is stored, so nothing machine-specific is shared, and
`--no-cache` ignores it.

> **Try it.** Make a second package under a sibling directory, depend on it by
> path, and call one of its `pub` functions. Then delete the dependency's
> `ply.pkg` and read `E0135`; then move a module inside it and read the import
> error, which tells you what a module is a file.

## Summary

- A file is a module named by its path; a single-file check loads only that file.
- `import a.b (x)` binds names, `import a.b as c` renames the binder, and
  `a::b` qualifies through one. `pub` exports; `opaque` keeps building and
  matching to the declaring module.
- `ply.pkg` is a literal `Manifest`: name, version, prefix, runtime,
  dependencies, entry.
- A `Dep` names a `Path`, a `Git(url, rev)` or the `Registry`, with a version
  floor. A dependency's prefix is the only way in, and a package cannot reach its
  importer.
- Resolution is minimal version selection; `ply.lock` pins it, `ply resolve`
  writes it, and `vendor/`, `why` and `--workspace` are the working tools.
- `.ply-cache/` is disposable; `PLY_CACHE_UPSTREAM` shares passes across
  checkouts.

Next: data that ships with the code, and values a build computes once.
