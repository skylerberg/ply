# Questions

Decisions that want Skyler's input. Each entry says what was assumed meanwhile; an answered
entry is deleted once the code reflects it.

## Time budgets

A self call in tail position runs as a loop, so a program can run forever where before every
computation was bounded. The guard is a wall-clock budget per entry: `ply test` gives each
test 60 s, `ply prove` each evaluation 5 s, `ply run` none, all settable with `--timeout`;
past it the entry fails with `E0503`. Say if the defaults should differ, or if you would
rather bound loops by a step count.

## Tail calls across definitions

Only a call of the enclosing function loops. A tail call of another function still nests.
Making those loop needs either the C compiler's sibling-call optimisation (not available under
`tcc` or `-O0`) or compiling a module's mutually recursive group as one C function. Assumed:
worth doing, as a later item, by the second route.

## Handler completeness stops at function values

A `handle` is now checked against the operations its body performs, following calls of named
definitions. An atom reached through a function value (a parameter, a field, a closure held in a
`let`) is not judged, because a row names atoms, not operations. Making that precise means rows
that name operations (`net.send[conn]`), a change to the row syntax, the printer, the hash and
the frames. Assumed: worth doing as its own item, listed in `docs/DIRECTION.md`.

## Libraries against more than one resource label

`std.http`'s client runs under `[conn]` because a definition cannot be generic over a label:
`send_all` is written `/ {net.write[conn]}` and a socket is bound to the label it is first used
under. The direction item wants `fn send_all<[l]>(c: Int, ...) / {net.write[l]}`, a label
variable instantiated at each call and printed, hashed and scheduled like a named label. It
interacts with rows that name operations, so it is planned after that item. Say if you would
rather have labels passed as values, or the standard library duplicated per label.
