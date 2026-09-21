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

## A bound host operation is answered for real inside `simulate`

On the interpreter tier, `ply test --host` answers a non-blocking bound host operation inside a
`simulate` region against the real host, once per interleaving, so a search reports a proof over
schedules built on readings that differed. The compiled tier refuses every host operation in a
seeded region (`crates/ply-codegen/src/host.rs`, `innermost_is_seeded`); the interpreter refuses
only the blocking ones (`crates/ply-eval/src/sched.rs`), which never sees an operation answered
inline. It reaches `std.config.get` and `std.signal.stopping` today, and `std.time` joins them.
Assumed: the interpreter should match the compiled tier and refuse them all, rather than
simulation growing a second notion of each. Say if you would rather a bound operation were
allowed and the proof narrowed to what it covers.
