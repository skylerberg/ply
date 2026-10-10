# Appendix B. Diagnostic codes

Every diagnostic `ply` writes names a code, and `ply explain CODE` prints its one
line. `ply explain --all` lists the whole table, and the reference (§17) holds it
with a sentence each.

```console
$ ply explain E0201
$ ply explain --all
```

## Reading a code

The first digit after the letter says which part of the language you are in:

| range | about |
| --- | --- |
| `E00xx` | tokens and files: an unexpected token, an unterminated string, a doc comment that documents nothing |
| `E01xx` | names, imports and packages: unknown names and types, private and `opaque` names, manifests, dependencies, the registry |
| `E02xx` | types: mismatch, arity, exhaustiveness, derivation, keys, literals |
| `E03xx` | effects, rows and labels: an effect the row does not permit, a `handle` that leaves an operation, label and row parameter mistakes |
| `E04xx` | programs and runs: regions, tasks, hosts, specifications, artifacts, configuration, SQL |
| `E05xx` | failures at run time: assertion failed, runtime error, step budget, Ply's own invariant |
| `W06xx` | warnings, never a fault in your program |

## The first ones you will meet

| code | means |
| --- | --- |
| `E0001` | unexpected token — usually a missing `;`, a `let` where an expression was expected, or a stray character |
| `E0101` | unknown name, including no `main` to run |
| `E0106` | unknown module |
| `E0107` | private name, or a constructor of an `opaque` type outside its module |
| `E0118` | `?` with no written `Result`/`Option` to exit through |
| `E0119` | `?` where its early exit would change what runs |
| `E0126` | a top-level `fn` missing a parameter or return type |
| `E0201` | type mismatch |
| `E0205` | a `match` that does not cover every case |
| `E0206` | not derivable, including an unordered `Map` key |
| `E0302` | effect not permitted by the written row — the one to read a fix from |
| `E0305` | a `handle` missing a clause for an operation its body performs |
| `E0412` | a `nondet` effect in a deterministic test |
| `E0424` | an operation reached the host boundary with nothing bound |
| `E0446` | a value outlives its region |
| `E0501` | assertion failed |
| `E0502` | a runtime error: `panic`, an unanswered raise, division by zero, a bad index, a spent budget |
| `E0503` | a spent step budget |
| `W0611` | a definition nothing reaches; a leading `_` in its name keeps it quiet |

## Reading a diagnostic

A diagnostic has a heading, one block per label, and notes. The heading is the
severity, the code and the message; a block points at a place in a file and runs
carets under the span; a `  = ` line is a note.

```text
Error[E0201]: type mismatch: function body type
  --> main.ply:1:20
   | fn main() -> Int = "forty-two"
   |                    ^^^^^^^^^^^ expected `Int`, found `String`
   compilation failed (1 error)
```

Two things are worth looking for in every diagnostic:

- **A `fix:` line.** Many diagnostics carry the literal edit: the row to write,
  the clause to add, the field to remove.
- **A second label.** `E0302` and `E0305` point at both the place the effect is
  performed and the signature or `handle` that has to cover it.

Under `--json`, a diagnostic that knows its remedy carries `fixes`, each a title
and a list of edits with byte ranges, so a tool can apply them. An edit applied as
written leaves a program the diagnostic no longer holds for.
