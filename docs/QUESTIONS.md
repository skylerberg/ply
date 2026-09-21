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

## A definition too deeply nested exhausts the emitter rather than being refused

The emitter walks an expression recursively and is itself compiled, so a sufficiently nested
definition runs its native stack out while the bundle is being built. It is not a property of the
program: the depth that fails depends on the emitter's frame size and the stack the thread was
given, so the same source can compile on one build and not another. A reader in `front.ply` hit
it at depth 44 where the previous deepest definition in the tree was 28, and the failure arrived
as an exhausted stack inside `emit.emit_roots` with no name attached.

A test now measures depth over every shipped source and refuses past a proxy bound, which catches
it in a partition rather than in the fixpoint. That is a guard, not a fix. The two real options
are a depth budget the emitter checks as it walks, so it refuses by name, or an iterative walk
for the nesting-heavy node kinds, which removes the limit instead of documenting it. Assumed: the
budget, since it is contained and the message is the thing that was missing. Say if you would
rather have the walker rewritten.
