# 20. Building and shipping

Running from source is one thing; handing someone an artifact is another. `ply
build` writes a program or a library, and `ply publish` sends a library to a
registry. This chapter covers both, plus signatures.

## A program

`ply build [path]` closes over one entry point and writes a `.plyx` file:

```console
$ ply build
   built main.main · b3:284f9a6916e2
   artifact 1 definition · 10.8 KiB · main.plyx
   binary ply 0.1.0 · 80.2 MiB
   startup none — this artifact cannot be run with `--config-schema`
```

The entry point defaults to `main` and can be named with `--entry MODULE.FN`; the
output defaults to `<entry module>.plyx` and can be named with `-o FILE`. A run of
an artifact runs no front end:

```console
$ ply run main.plyx
   program b3:284f9a6916e2 · 1 definitions
   42
```

What the artifact holds is the **transitive closure** of the entry point: the
definitions it reaches, by hash; the same definitions printed back to source
without tests, laws, comments or anything unreached; and the runnable `ply run`
loads — that source checked again, its front end's answer, and its compiled unit,
which holds the value of each `const fn` (chapter 17). An edit nothing reaches
leaves the digest unchanged.

What is reached is what each definition refers to. An effect or a type that shares
a function's name is another definition, so a program that names the effect holds
nothing the function calls. `--digest` prints just the digest:

```console
$ ply build --digest
b3:284f9a6916e2
```

and `--diff OLD.plyx` says what a rebuild would change:

```console
$ ply build --diff main.plyx
   main.main b3:284f9a6916e2 → b3:284f9a6916e2

   added      0 definitions
   changed    0 definitions
   dropped    0 definitions
   unchanged  1 definition
```

The standard library definitions an artifact draws on are **its own**, so a `ply`
that ships other ones runs it as it was built, reading none of its own. A part
that does not agree with the rest is `E0443`, and an artifact built by another
compiler or for another runtime is `E0444`: rebuild it with this `ply`.

## A library

A package whose manifest names no entry and whose own modules declare no `main` is
a **library**, and `ply build` writes a `.plyz`:

```console
$ ply build
   built libdemo 0.1.0 · b3:d86cff55b6e5
   library 1 module · 8681 bytes · libdemo.plyz
   unit 1 definition compiled
```

A `.plyz` holds every module's source, the package's `ply.pkg`, and a compiled
unit of every definition those modules declare — a library has no entry to prune
against, so nothing is left out. A consumer compiles those sources, which is why
`--verify-deps` can check a dependency's published interface against its source.
It is a package, never a program:

```console
$ ply run libdemo.plyz
Error[E0443]: `libdemo.plyz` is a library, and nothing to run
   = a `.plyz` is a package: declare it in a `ply.pkg` and depend on it
   = `ply build` writes a program's artifact as a `.plyx`
```

`ply build --config-schema MODULE.FN` ships that function too, resolved as a run
resolves it, so an artifact can be checked against a schema without its sources.

## Signing

```console
$ ply keygen release.key
   generated release.key · release.key.pub
   public bb2082006c52fc271556933363e7d9bc3549b0ee716a51e4ef012e2e50a1b3c9

$ ply build -o app.plyx --sign release.key
   signed bb2082006c52fc271556933363e7d9bc3549b0ee716a51e4ef012e2e50a1b3c9

$ ply run app.plyx --require-signer release.key.pub
   program b3:284f9a6916e2 · 1 definitions
   42
```

`ply keygen PATH` writes an Ed25519 key pair: the secret key at `PATH`, readable
by its owner alone, and the public key at `PATH.pub`, each one line naming what it
holds and 64 hex digits. It never writes over a file. The secret key is a `Secret`
from the moment it is drawn and is read back as one, so `ply build --sign` never
holds the key as a plain value.

`--sign KEY` writes `<artifact>.sig` beside the artifact. The signature is
detached, so the artifact's digest is the same whoever signs it, and signing the
same build again with another key adds a signature beside the first. What is
signed is the artifact's provenance: its kind and name, its full digest, the `ply`
that built it, the semantics version, and the commit `HEAD` named when its sources
were in a git repository.

`ply run ARTIFACT --require-signer KEY` runs an artifact only when one of the
public keys named signed that provenance for the artifact's own digest. An
artifact with no signatures, signatures for other bytes, or none by a trusted key
is `E0460`, before anything of it is loaded. `ply build --verify` builds in memory
and holds the file `-o` names, and every signature beside it, to what these sources
build, writing nothing:

```console
$ ply build --verify
   verified libdemo.plyz · b3:d86cff55b6e5 · unsigned
```

A file that is not that build, or a signature that does not hold, is `E0461`.

## The registry

A library is published to a registry and depended on from it. A registry is a
directory of files behind an HTTP server:

```text
GET  /<name>/index.json                          every version of <name>, newest last
GET  /<name>/<version>/package.plyz              the library's `.plyz`
GET  /<name>/<version>/package.plyz.b3           its digest
GET  /<name>/<version>/interface/<semantics>     the interface the publisher cut
GET  /<name>/<version>/attestation/<semantics>   what an attester found
PUT  /<name>/<version>                           publish
POST /<name>/<version>/yank                      mark yanked
```

`ply publish [path]` builds the library and sends it to the registry
`PLY_REGISTRY` names, under the token `PLY_REGISTRY_TOKEN` holds. Only a library
whose dependencies are all `Registry` ones is published, since whoever depends on
it resolves them from the registry alone. The registry recomputes the digest from
the body and refuses a mismatch, refuses other bytes under a version it already
lists — a published version never changes, and the fix is a new version — and
refuses an archive whose manifest is not the package and version it was sent as.

After the archive, `ply publish` sends the package's **interface**: what each of
its modules shows a module that imports it — each name's signature, rows and
counts, and what hashing fixed of it — framed with the **semantics version** of
the `ply` that cut it. The semantics version names what a definition's hashes, its
checked rows and a claim's verdict mean, and moves only when one of them does, so
a `ply` that changes nothing they mean reads an interface another cut.
`--verify-deps` (`check`, `test`, `prove`) refuses a dependency whose interface
does not re-derive from its source (`E0149`).

`ply yank NAME VERSION` marks a version yanked: a new resolution passes it over
and a lock that pins it keeps it. Nothing is ever deleted.

A registry with an attester vouches for what it serves. `ply attest NAME VERSION`
lays the published version out as a project of its own, fetched and checked as a
resolve fetches it, and runs `ply test` and `ply prove` over it; with `--sign KEY`
the answer is signed and sent back. `ply resolve` fetches each dependency's
attestation and believes it only when a key `PLY_ATTESTERS` names signed it for
the archive the lock pins.

## Version discipline

`ply publish` compares the contract of every public definition — its signature and
specifications, and its body when it is `transparent`; a type's or effect's whole
declaration — with those of the highest unyanked version published below it. A
patch moves no contract, a minor only adds definitions, and a major may change or
remove any. A version that bumps less than its changes need is `E0150`, naming
what moved and the least version that says so, before anything is sent. 0.x
versions follow the same places. `ply contracts NAME FROM TO` lists what moved
between two published versions.

The point is that a `ply.lock` in someone else's checkout keeps meaning what it
meant: a version whose bytes change is refused (`E0142`), a version whose contract
moved needs the bump that says so, and an artifact built by another semantics is
refused (`E0444`).

## Environment

| variable | for |
| --- | --- |
| `PLY_REGISTRY` | the registry base URL |
| `PLY_REGISTRY_TOKEN` | the bearer token a publish or yank sends |
| `PLY_ATTESTERS` | public key files an attestation is believed against |
| `PLY_TRUST` | PEM certificates the `ply` command's own HTTPS connections accept |

`PLY_REGISTRY` is one base URL, `https://host[:port][/prefix]`; the client verifies
the server against the built-in roots and the certificates `PLY_TRUST` names, so a
registry under a private CA is reached by pointing `PLY_TRUST` at the CA's
certificate. `http://` is accepted only for a registry on this machine, because a
publish carries a token.

The registry is itself a Ply program — `crates/ply-registry/ply` — and the
reference (§15.1) documents how to run one: a store directory, a port, a TLS
credential, a `token.<name>` per package in a configuration file.

> **Try it.** Build the spending tracker with `--sign`, copy the `.plyx` and its
> `.sig` somewhere else, and run it with `--require-signer`. Then change one
> definition that nothing reaches and rebuild: the digest is unchanged, so the old
> signature still verifies. Change one that the entry point does reach and watch
> the digest move.

## Summary

- `ply build` writes a `.plyx` for a program's entry point or a `.plyz` for a
  library; `--entry`, `-o`, `--digest` and `--diff` shape the output.
- The artifact holds the closure's definitions, their source, a compiled unit and
  each `const fn`'s value; the digest covers the closure and the entry.
- `ply keygen` writes an Ed25519 pair; `--sign` writes a detached signature over
  the artifact's provenance; `--require-signer` refuses to run without a trusted
  one; `--verify` compares without writing.
- `ply publish` sends a library's `.plyz` and its interface; `resolve`, `yank` and
  `attest` are the rest of the registry protocol; a version's bytes never change.
- A version's bump is checked against its contract changes (`E0150`), and the
  semantics version says what hashes and verdicts mean.

Next: one real tool, built end to end.
