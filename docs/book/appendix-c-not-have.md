# Appendix C. What Ply does not have

Absences are design decisions here, not gaps. Each one buys something.

- **No loops, `break` or `return`.** A tail call is a loop, and the checker proves
  a recursion ends; `iterate` is the loop with a budget. The absence is what lets
  the checker read every recursion, which is what lets a function's type say it
  may not return (chapter 6).
- **No mutable variables.** State lives in a cell inside a region, and the type
  says which region. That is what makes `parallel` able to prove two computations
  cannot interfere (chapters 11 and 12).
- **No exceptions.** Failure is an effect, in the row, and `try` and `?` are its
  syntax. A row is therefore a complete account of how a call can fail
  (chapter 10).
- **No typeclasses, implicits or method syntax.** Constraints are `where` clauses
  on a parameter, and `derive` writes the dictionary as a plain value you can see
  and pass (chapters 7 and 15).
- **No `unsafe` and no FFI.** What a program reaches outside itself is an effect a
  handler answers, and the builtins are the only functions the runtime implements.
  That is what makes the host boundary a list (chapters 12 and 19).
- **No modules-as-values and no first-class effects.** An effect is a name in a
  row, and a handler is written where it stands.
- **No tracing garbage collector.** A cell, a task, a channel and a hold are
  branded by a region that frees them; a reference cycle is never freed
  (`W0610`).
- **No shared mutable state between OS threads.** A `parallel` block's branches
  run on threads of the runtime's own; a task never moves between threads.

Some sharp edges are worth knowing before they surprise you:

- `x.f(y)` where `x` is a bare variable is an **effect perform**, not a field
  call. Parenthesize: `(x.f)(y)` (chapter 5).
- An operation no `handle` names is found only when it reaches the host boundary
  at run time (`E0424`), unless its effect is `nondet` in a deterministic test
  (`E0412`).
- A record update needs the base's type to be known where it stands.
- `bytes_at`, `bytes_u32_le`, `string_slice`, `string_find`, `list_set`,
  `array_get` and `array_set` **raise** where `list_at` and `array_at` answer
  `None`.
- `Int` arithmetic is checked and overflows end the run, which is why a law over
  `Int` usually needs a guard (chapter 14).
- A `let` is a statement, not an expression, so a body that binds needs a block
  (chapter 5).

The reference lists the same absences and sharp edges with their section numbers
(§18).
