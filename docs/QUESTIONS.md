# Questions

Decisions that want Skyler's input. Each entry says what was assumed meanwhile; an answered
entry is deleted once the code reflects it.

## Loops are now unbounded

A self call in tail position runs as a loop, so a program can now run forever, where before
every computation was bounded by the call limit or a budget. Assumed: that is the right trade
for real programs, and the guard belongs in the runner. Proposed follow-up: a per-test
wall-clock timeout in `ply test` (`--timeout`), reported as its own diagnostic. Say if you
would rather keep a step budget on loops instead.

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
