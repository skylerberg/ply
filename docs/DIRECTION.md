# Direction

Ply is a language for programs that agents write, test and ship, in a world where inference
is 10 to 100 times cheaper than today. Writing code is then no longer the bottleneck.
Verification, the edit-test loop, context cost and many agents sharing one codebase are.

A feature earns its place by one of these. Saving keystrokes does not count.

- **The signature says everything.** What a definition touches, needs and promises is in its
  type, so nothing has to read its body.
- **Every check is static or cached.** A failure found at run time that the compiler could
  have found costs a whole loop iteration.
- **Answers are for machines.** Every command answers in JSON; every diagnostic names its
  code, its place and, where one exists, its fix.
- **Compute buys confidence.** More budget on a test, proof or search yields a stronger claim,
  never just a slower run.
- **One implementation, O(change) everywhere.** Nothing is redone that an edit did not reach,
  including in the compiler's own loop. New code is Ply; Rust is ported as work reaches it.

## Work

- Tail calls across definitions, so mutual recursion is a loop too.
- Rows that name operations, so a signature says `net.send[conn]` rather than `net.write[conn]`
  and a `handle` over a function value is judged too.
- `ply fmt`, from the printer `ply build` already has.
- Structured fixes on diagnostics: a machine-applicable suggestion and candidates in the JSON.
- Replace a definition by name, once `ply fmt` exists to print it back.
- Induction over lists in the prover, which needs a cons term in its term language.
- Mutation testing and per-definition coverage over the hash index.
- The compiler's own loop: stage-1 emission cached by source digest and incremental per
  definition.
- Reach: an HTTP client over `net.connect`, and a TLS `connect`; libraries usable against more
  than one resource label; packages with dependencies pinned by content hash.

## Open

- Loops and mutable variables, dispatch and method syntax: stay out unless generated code's
  error rate says otherwise.
- The module system's shape: manifests, hash-pinned dependencies, the standard library as a
  package.
