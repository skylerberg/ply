# 18. The standard library

The standard library is a built-in package: it is an implicit dependency of every
package, needs no declaration, and ships inside `ply`. It has 142 modules, and
this chapter is a map of them plus the commands that tell you what a definition
does.

## Finding what you need

```console
$ ply std
   142 modules · 12499 definitions · shipped with this compiler

   MODULE                DEFINITIONS  TESTS  BYTES   DATA    SUMMARY
   std.cli               237          51     129938  0       A command line as a declaration: one `Spec` says what a program's commands, flags and positionals are, and the parser, ...
   std.json              183          59     82056   0       JSON: the value, a parser, a serializer, and the codecs `derive json for T` builds on.
   ...
```

`ply std` lists every module with its summary and how many definitions, tests and
data bytes it carries. `ply std --show std.json` prints a module's source, and
`ply std --digest` prints the digest over all of it.

`ply doc` answers the question you actually have most often:

```console
$ ply doc std.list.take
   pub fn take<a>(xs: List<a>, n: Int) -> List<a>
      The first `n` elements. A non-positive `n` is the empty list, and a list shorter than `n` is
      itself.
      cost len(xs)

   at <std>/list.ply:47:5
   touches {}
   hash 7a90c327471d5ece728614a17bb9a8f23d0d9d8b210feb24fc0b0c2f125537257
   tests "take and drop are the two halves, and the edges are total"
   laws "what take takes and drop drops make the list", ...
```

A `ply doc NAME` answers with the definition's signature and parameter names, its
doc comment, its `returns` and specification clauses, where it is, its hash, what
it touches (its footprint) and the tests and laws that name it. For a module it
lists what the module publishes, each with its doc. `ply doc std.json` documents
the whole module; `ply doc std.fs.write_chunk` documents one operation of an
effect; `ply doc prelude` lists the builtins.

The library's own tests and obligations are not a project's. `ply test`, `ply
prove` and `ply review` skip them unless you pass `--std`, which adds those of
every module `ply std` lists, whichever of them the project imports.

## What a shipped module costs

A load reads of a shipped module only what a module importing it can reach: none
of its tests or laws, nor a private function or type or an `import` that only they
reach, which it neither parses nor checks. `ply std --show` lists the source, but
importing `std.bigint` does not make your build read the module's tests.

That is why the `BYTES` column above is not the whole cost of a module. What you
pay for is what you reach plus what it imports.

## A tour

| area | modules |
| --- | --- |
| text | `std.string`, `std.text`, `std.char`, `std.unicode`, `std.segment`, `std.width`, `std.fmt`, `std.parse`, `std.regex` (+ the `re"..."` literal), `std.glob` |
| data formats | `std.json`, `std.bin`, `std.cbor`, `std.msgpack`, `std.yaml`, `std.toml`, `std.ini`, `std.csv`, `std.xml`, `std.html`, `std.protobuf`, `std.asn1`, `std.multipart` |
| encodings | `std.hex`, `std.base32`, `std.base64`, `std.ascii85`, `std.radix`, `std.punycode`, `std.idna` |
| collections | `std.list`, `std.map`, `std.set`, `std.deque`, `std.heap`, `std.psq`, `std.seq`, `std.bitset`, `std.graph`, `std.cache`, `std.stats` |
| numbers | `std.math`, `std.bigint`, `std.bigdecimal`, `std.rational`, `std.complex`, `std.float`, `std.decimal` |
| time | `std.time`, `std.date`, `std.duration`, `std.tz`, `std.strftime`, and the `datetime"..."`, `timestamp"..."` and `timeofday"..."` literals |
| files and IO | `std.fs`, `std.io`, `std.path`, `std.tar`, `std.zip`, `std.kv`, `std.sqlite`, `std.gzip`, `std.brotli`, `std.zstd`, `std.coding` |
| network | `std.net`, `std.http`, `std.http2`, `std.websocket`, `std.udp`, `std.dns`, `std.resolver`, `std.ip`, `std.url`, `std.cookie`, `std.cors`, `std.static`, `std.router`, `std.mime` |
| mail | `std.email`, `std.smtp`, `std.sasl` |
| concurrency | `std.task`, `std.chan`, `std.parallel`, `std.random` |
| the machine | `std.process`, `std.os`, `std.signal`, `std.term`, `std.config` |
| crypto | `std.crypto`, `std.hash`, `std.hash.legacy`, `std.password`, `std.ed25519`, `std.signed`, `std.x509`, `std.pem`, `std.jwt`, `std.totp`, `std.checksum` |
| data | `std.db`, `std.pg`, `std.sql`, `std.sqlite` |
| observability | `std.trace`, `std.telemetry`, `std.otlp`, `std.prometheus`, `std.traceparent` |
| identifiers | `std.uuid`, `std.oid`, `std.semver` |
| tooling | `std.cli`, `std.sh`, `std.pkg`, `std.syntax`, `std.checker`, `std.gen`, `std.laws`, `std.cases`, `std.snapshot` |
| values | `std.show`, `std.value`, `std.option`, `std.result` |

That is not all 142. `ply std` is the list, and it is worth skimming once: most
of what you would otherwise write a helper for already exists, with tests and
laws.

## Two conventions to know

**The prelude has the basics; the modules have the rest.** `std.list`, `std.map`,
`std.set`, `std.option` and `std.result` re-export the prelude's builtins under
the module's name and add what is missing, so you can write either
`map_get(m, key)` or, with `import std.map`, `map::get(m, key)`. The module
versions are ordinary `pub fn`s with docs, costs and laws:

```ply
import std.list
import std.math (sum)
import std.string (join, words)
import std.result

fn half(n: Int) -> Result<Int, String> =
  if n % 2 == 0 { Ok(n / 2) } else { Err("odd") }

test "the standard library fills in the prelude" {
  assert_eq(list::reverse([1, 2, 3]), [3, 2, 1]);
  assert_eq(list::take([1, 2, 3, 4], 2), [1, 2]);
  assert_eq(sum([1.5, 2.5]), 4.0);
  assert_eq(join(words("a b c"), "-"), "a-b-c")
}

test "result helpers" {
  assert_eq(result::result_map(half(4), |n: Int| n + 1), Ok(3));
  assert_eq(result::result_unwrap_or(half(3), 0), 0);
  assert_eq(result::result_ok(half(3)), None)
}
```

**A module that writes a literal imports what it needs implicitly.** `uuid"..."`
brings in `std.uuid`, `#[...]` brings in `std.set`, `f"..."` brings in
`std.show`, and `datetime"..."`, `re"..."`, `cidr"..."` and the rest each bring in
their module under a name no source can write. That is why a set literal works
without an import while `set::of_list` needs one.

## Reading a module's tests

The library's tests and laws are the best documentation of edge cases, and `ply
std --show` prints them:

```console
$ ply std --show std.list | grep -n 'fn take'
```

or read `ply doc std.list.take`, whose `tests` and `laws` lines name the
definitions that pin its behavior. A function whose doc says "the first `n`
elements" and whose test says "a non-positive `n` is the empty list" has answered
the question you were about to ask.

> **Try it.** Pick the helper you would write next — splitting a string, reading a
> file, encoding hex — and ask `ply doc` first. Search `ply std` for a module that
> sounds right, then read its `pub` definitions. The odds are good you will not
> write the helper.

## Summary

- The standard library is `std`, an implicit dependency: 142 modules, listed by
  `ply std`, documented by `ply doc`.
- A load reads only the shipped modules the program reaches, and of those only
  the definitions and names it uses.
- `--std` includes the library's own tests in `ply test`, `ply prove` and
  `ply review`.
- `std.list`, `std.map`, `std.set`, `std.option` and `std.result` re-export the
  prelude and add the rest; a literal such as `#[...]`, `re"..."` or
  `datetime"..."` imports its module implicitly.

Next: the boundary where an effect stops being handled by your program and starts
being answered by the machine.
