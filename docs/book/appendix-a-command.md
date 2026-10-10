# Appendix A. The `ply` command

The command surface, by what you are trying to do. Every command takes `--json`
and then prints exactly one JSON object on stdout, and `--color
auto|always|never` is global. The reference (§16) has each command's flags in
full, and `<command> --help` is authoritative.

## Building and running

| command | does |
| --- | --- |
| `ply new PATH` | writes a package: `ply.pkg`, a first module and a test; `--lib` writes a library instead |
| `ply check [path]` | parse, resolve, type-check, infer rows; `--types`, `--costs`, `--explain` |
| `ply test [path]` | select, schedule and run the tests |
| `ply run [path] [-- ARGS]` | evaluate `main`, or run a built `.plyx` |
| `ply prove [path]` | discharge every obligation and report each tier |
| `ply review [path]` | what changed since `--accept`, and whether its proofs still hold |
| `ply build [path]` | write a deployable artifact: `--entry`, `-o`, `--digest`, `--diff`, `--sign`, `--verify` |
| `ply hosts [path]` | list the host handlers this binary can bind; `--host` lists them as bound; `--digest` |

## Reading a program

| command | does |
| --- | --- |
| `ply doc NAME [path]` | a definition or builtin: signature, doc, place, hash, footprint, the tests and laws that name it; a module lists what it publishes; `prelude` lists the builtins |
| `ply defs [path]` | every definition with its place, hash, signature and footprint |
| `ply hash [path]` | the content hash of every definition; `--deps` adds references and the transitive closure |
| `ply show NAME [path]` | one `fn` or `type` as its file holds it |
| `ply callers DEF [path]` | what mentions a definition, directly and through calls |
| `ply explain CODE` | one line on a diagnostic code; `--all` lists every code |
| `ply std` | the modules that ship with this compiler, with their summaries and digest; `--show [NAME]` prints a module's source or a data file |

## Editing

| command | does |
| --- | --- |
| `ply fmt [paths]` | rewrite `.ply` files in the canonical layout; `--check` writes nothing and exits 1 naming what would change |
| `ply replace NAME [path]` | rewrite one `fn` or `type` from `--with FILE` or stdin, touching no other byte; refuses (`E0128`) if the program would not check or another definition would move |

## Packages

| command | does |
| --- | --- |
| `ply resolve [path]` | write `ply.lock` from the closure; the one command that fetches registry dependencies |
| `ply vendor [path]` | copy the closure into `vendor/`, so the project builds with no cache or network |
| `ply why NAME [path]` | why a package is in the closure, and what it resolved to |
| `ply publish [path]` | upload this library's `.plyz` and its interface to `PLY_REGISTRY` |
| `ply yank NAME VERSION` | mark a published version yanked |
| `ply attest NAME VERSION` | run a published version's tests, claims and promises; `--sign KEY` sends the answer |
| `ply contracts NAME FROM TO` | what moved between two published versions, and the bump it needs |

## Keys and internals

| command | does |
| --- | --- |
| `ply keygen PATH` | an Ed25519 key pair: the secret key at `PATH`, the public key at `PATH.pub` |
| `ply cache clear\|stats\|compact [path]` | discard the store and memo store / report what they hold / reclaim space |
| `ply cache inspect <DEF> [path]` | what a load answers for one definition |
| `ply bootstrap <path>` | write the builder or `ply` itself as a runnable (the repository's own use) |

## Exit codes

| exit | meaning |
| --- | --- |
| 0 | success |
| 1 | a test failed, or `main` raised |
| 2 | the program did not run: bad path, syntax or type error |
| 3 | the drain deadline expired with requests in flight |
| 4 | `ply test --kept`: what earlier runs kept does not answer the run |
| *n* | `process.exit[p](n)` under `ply run --host`: the program's own, 0 to 125 |
| 141 | the reader of stdout or stderr went away, as when output is piped into `head` |

Flag groups repeat across commands: *simulation* (§9 of the reference), *host*
(`--host` and the flags of §14), *prove* (`--prove-cases`, `--prove-roots`,
`--prove-budget`, `--shrink-budget`, `--prove-steps`), *trace* (`--trace`,
`--trace-level`), *drain* (`--drain-ms`, `--drain-lead-ms`).
