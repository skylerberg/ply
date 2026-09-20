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

## Libraries against more than one resource label

`std.http`'s client runs under `[conn]` because a definition cannot be generic over a label:
`send_all` is written `/ {net.write[conn]}` and a socket is bound to the label it is first used
under. The direction item wants `fn send_all<[l]>(c: Int, ...) / {net.write[l]}`, a label
variable instantiated at each call and printed, hashed and scheduled like a named label. It
interacts with rows that name operations, so it is planned after that item. Say if you would
rather have labels passed as values, or the standard library duplicated per label.

## Porting the CLI to Ply

The CLI's own shell (arguments, JSON answers, exit codes) is small. `std.process` gives a
program its arguments, stdout, stderr and the exit code; what the port still lacks is a way
to spawn the C compiler. What the CLI mostly does is drive Rust subsystems: the evaluator
and compiled backend, the test scheduler and bisector, the prover, the store and the hosts.
Assumed order, if this is wanted: the commands that are pure over the compiler's answers
(`check`, `defs`, `doc`, `explain`, `fmt`, `hash`) as Ply programs behind the same command
surface; then one subsystem at a time, the prover and the store first as the most
self-contained. Say whether this should come before rows that name operations and label
abstraction, which are the items left in `docs/DIRECTION.md`.
