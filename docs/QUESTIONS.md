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

## Retiring `--backend wrong:<mutation>`

That flag corrupts answers at the `Compiled::enter` seam, which no production path enters any
more: a test is entered whole, so every test under it fails as "no body" and its own suite
passes for the wrong reason. `ply test --mutate` makes the same claim honestly at the source.
Assumed: retire the flag, its `Mutant` wrapper and its suite in a follow-up, unless you want
the seam kept for something else.
