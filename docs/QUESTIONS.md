# Questions

Decisions that want Skyler's input. Each entry says what was assumed meanwhile; an answered
entry is deleted once the code reflects it.

## Time budgets

A self call in tail position runs as a loop, so a program can run forever where before every
computation was bounded. The guard is a wall-clock budget per entry: `ply test` gives each
test 60 s, `ply prove` each evaluation 5 s, `ply run` none, all settable with `--timeout`;
past it the entry fails with `E0503`. Say if the defaults should differ, or if you would
rather bound loops by a step count.

## Row and type parameters still do not share across a recursive group

A recursive component is now checked with one set of label binders, so a definition generic over
a label may call a mutually recursive sibling. Row and type parameters are not shared the same
way: two members each get their own rigid `e` and `a`, so `std.http`'s connection loop is still
one function rather than five (`crates/ply-std/ply/http.ply`). Extending the sharing is not
mechanical, since a group whose members want *different* instantiations of a type parameter is
polymorphic recursion, which needs written signatures to stay decidable. Assumed: worth doing for
rows, where there is no such difficulty, and worth refusing clearly for types. Say whether you
want both, rows only, or neither.
