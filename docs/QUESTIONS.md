# Questions

Decisions that want Skyler's input. Each entry says what was assumed meanwhile; an answered
entry is deleted once the code reflects it.

## Time budgets

A self call in tail position runs as a loop, so a program can run forever where before every
computation was bounded. The guard is a wall-clock budget per entry: `ply test` gives each
test 60 s, `ply prove` each evaluation 5 s, `ply run` none, all settable with `--timeout`;
past it the entry fails with `E0503`. Say if the defaults should differ, or if you would
rather bound loops by a step count.

## Record shapes stay file-local

A record update now takes its shape from a call of a `fn` declared in the same file, and a
`let` without a type takes the written type of such a call, a local or an update. Shapes are
still never read across a module boundary: the rewrite runs before types exist, and a shape
read from another file would change this file's hashes without this file changing. Assumed:
that line stays where it is, and `{..other::make(), x: 1}` keeps needing a local annotation.

## Row and type parameters still do not share across a recursive group

A recursive component is now checked with one set of label binders, so a definition generic over
a label may call a mutually recursive sibling. Row and type parameters are not shared the same
way: two members each get their own rigid `e` and `a`, so `std.http`'s connection loop is still
one function rather than five (`crates/ply-std/ply/http.ply`). Extending the sharing is not
mechanical, since a group whose members want *different* instantiations of a type parameter is
polymorphic recursion, which needs written signatures to stay decidable. Assumed: worth doing for
rows, where there is no such difficulty, and worth refusing clearly for types. Say whether you
want both, rows only, or neither.

## A part of a cached answer is spelled differently by each side

The Ply compiler now reads, splits and joins a front-end answer. A *whole* dump is byte-identical
whichever side wrote it, but a *part* is not: Rust writes a part with only that file's source in
scope, which renumbers every span to module 0, while the Ply split keeps the indices the whole
had. Nothing in Ply files parts yet, so nothing is broken today, and `ply check` is the first
thing that would. Assumed: the split should relocate spans exactly as Rust's caller does, so a
part written by either side reads on both. Say if you would rather parts kept the whole's
numbering and the Rust caller stopped renumbering, which is the same fix from the other end.
