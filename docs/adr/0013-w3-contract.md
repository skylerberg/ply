# ADR 0013 — The W3 implementation contract

Status: accepted

ADR 0009 settled *why* effect-set aliases exist. This one settles how, and
settles the four things around them that a real server cannot ship without:
HTTP/1.1 framing, limits, routing, and TLS. Where 0009 and this disagree, this
wins — it was written after it, against the code. Where 0011 §8 deferred a
question to W3, this is W3 answering it.

## The rule everything else follows from

> **The trusted computing base does not grow to hold a protocol.** HTTP/1.1
> framing is a pure function from bytes to a request, so it is written in Ply,
> where `ply test` selects it exactly, a defect in it is a failing test rather
> than a line nobody checks, and every one of the framing rules below is a
> hermetic `det` test. The boundary carries bytes. It carries bytes over TLS
> too, because cryptography is the one thing here that would be reckless to
> write in Ply — and that is the only place the TCB grows in this milestone.

Four corollaries, each of which decides a section below:

1. **A protocol defect must be reachable by a test.** Request smuggling is a
   parser disagreeing with itself about where a message ends. A parser in Rust
   behind a host handler is a parser `ply test` cannot reach, and `ply hosts`
   can only print its name. Written in Ply it is ordinary code with an empty row
   (§2, §3).
2. **A bound is part of the contract, not a tuning knob.** A server without
   limits is trivially denied service by one packet, so the limits are a value
   the program passes and the parser is written so that exceeding one costs the
   bound rather than the buffer (§4).
3. **A route table is data.** Not a macro, not a registry, not a list of
   closures. Data can be inspected, derived, quantified over and specified, and
   matching over it is a pure function — which is also what keeps each
   endpoint's own row narrow and visible (§5).
4. **An alias is an abbreviation for a row, and abbreviations do not change
   meaning.** Its *name* never enters a hash. Its *expansion* enters exactly as
   the row it stands for would, because the expansion is the published upper
   bound and a published upper bound is what callers are checked against (§1).

---

## 1. Effect-set aliases

### 1.1 Syntax

```
item        := "pub"? (fnDef | typeDef | effectDef) | testDef | lawDef
             | deriveDef | effectSetDef
effectSetDef:= "effect" "set" IDENT "=" "{" setMember,* "}"
setMember   := atom | qname
row         := "{" rowMember,* ("|" IDENT)? "}" | IDENT
rowMember   := atom | qname
atom        := qname "." ("read"|"write") ("[" IDENT "]")?
```

A row member is an atom when the identifier is followed by `.`, and an
effect-set reference otherwise. One token of lookahead decides it; there is no
ambiguity and no reserved word. A whole row that is a bare `IDENT` is still a
row *variable*, as it is today — an alias is only ever written inside braces.

```ply
effect set Web = {
  db.read[users], db.read[inventory], db.write[orders],
  http.write[outbound], log.write, clock.read,
}

fn create_order(req: Request) -> Response / {Web, random.read} = ...
```

### 1.2 A member is an atom, never a whole effect

ADR 0009 §1 wrote `effect set Web = {db, http, log, clock}`, naming whole
effects. That is refused, for a reason that is not an implementation
convenience.

A whole effect is not a bounded set of atoms. `db.get[r]` is
resource-parameterized, so "every atom of `db`" is every resource label
*anywhere in the program* — and adding an unrelated table in an unrelated module
would then change the expansion of `Web`, and therefore the declared row, and
therefore the hash, of every definition annotated with it. That is precisely
ADR 0012's corollary 1: nothing outside a definition's own reachable graph may
enter its hash.

The alternative — a wildcard atom `db.read[*]` — would put a non-ground shape
into `EffectAtom`, and `EffectAtom::conflicts_with` is the predicate the
scheduler, world isolation and partial-order reduction are all built on. A
wildcard that conflicts with everything is `IO` with extra steps.

So a member is an atom, written exactly as it is written in a row, or the name
of another set. This is also the better answer for review: the resources stay
visible at the one place a reviewer reads the set, which is the defence ADR
0009's "risk worth naming" has otherwise no mechanism for.

### 1.3 Sets are module-local

An `effect set` may be used only in the module that declares it. It is not
`pub`, it cannot be imported, and `other::Web` in a row is `E0114`.

This is not timidity; it is ADR 0012 amendment A4's rule applied to the same
hazard. Gate 1 skips a file whose raw bytes are unchanged. If a set declared in
module A expanded into a row in module B, editing A's set would change B's
published row and therefore B's definition hashes — while B's bytes had not
moved and gate 1 would skip it. The result is a stored footprint that
under-reports, which is a scheduling and isolation defect that produces a
**green** result. Every dangerous defect this project has found was of that
shape.

There is a mechanism that would make the cross-module form sound —
`ImportEdge::exports` is already a gate 1 refusal condition, so an `effect set`
contributing an entry to the declaring module's exports digest would refuse the
skip. It needs a `DefKind::EffectSet`, a fourth resolution namespace, and a
hygiene rule for the effect names a substituted atom is written in, since those
are written in the *declaring* module's binders. That is a lot of surface for
sugar, in the milestone that can least afford a new silent-staleness path, and
it can be added later without moving any hash of a program that did not use it.
Recorded here so it is a decision rather than an omission.

The cost is real and stated: a service split across five route modules writes
its `effect set` five times, and the five can drift. A drifted set is an
annotation that is wider or narrower than another's — checked as an upper bound
either way, so it is a legibility cost and never a soundness one.

### 1.4 Expansion is part of parsing

`ply_syntax::parse_module` expands every row before it returns. An unexpanded
`RowExpr` does not exist outside the parser, so there is no pass ordering to get
wrong, no crate that can forget to run the expander, and no silent path where a
row is checked with its aliases ignored.

- Expansion reads **this module's own `effect set` items and nothing else**,
  which is what makes it a function of the file, which is what makes gate 1
  right (§1.3).
- The effect references inside a set are written in the same file as the rows
  they expand into, so substitution is hygienic by construction.
- Expansion runs **before** `ply_derive::expand_module`. Derived bodies carry no
  written row today; the order is pinned so that they may later.

`RowExpr` keeps the set names it was written with, in source order, as
**provenance**: `RowExpr::aliases`. It is namespace metadata, erased by
normalization, exactly as a module name and a `pub` are.

`EffectSetDef` stays in `Module::items` as `Item::EffectSet`. It declares no
name a reference can reach, generates no definition, and every consumer that
enumerates definitions is right to skip it — the same treatment `Item::Derive`
already gets, for the same reason.

### 1.5 The three properties, made exact

**An alias is annotation-only, and inference still produces the precise row.**
Expansion produces an ordinary `RowExpr`; `Checker::signature` converts it to a
`Row`; `check_upper_bound` checks the inferred row against it exactly as it
checks a written one. Nothing downstream ever sees an alias: scheduling,
isolation, partial-order reduction, `E0412` and the footprint in the cache all
operate on atoms. An `E0302` raised against an aliased signature quotes the
**expansion** in its secondary label, never the alias name, because the name is
not what the body failed to satisfy.

**An alias is checked as an upper bound exactly as a written row is.** There is
one code path and it is the existing one.

**An alias name never enters a hash — and its expansion enters exactly as the
row it stands for would.** ADR 0009 §3 said "regrouping which atoms it contains
must change no definition hash anywhere". That sentence is superseded, and the
precise claim is:

| edit | hashes that move |
| --- | --- |
| declaring an `effect set` nothing uses | none |
| renaming an `effect set` | none |
| reordering its members, or writing one twice | none |
| rewriting `/ {db.read[users], log.write}` as `/ {Web}` where `Web` expands to exactly that | **none** — the headline test |
| changing which atoms a set contains | exactly the definitions annotated with it, and their transitive dependents |

The last row is not a concession. A `/ {..}` annotation is the *published*
signature: `Signature::published_row` is the declared row, and `DefInfo::
footprint` is what callers are inferred against. Widening a set widens the bound
every definition annotated with it publishes, so a caller checked against the
narrower bound has to be rechecked — and gate 2 only rechecks a definition whose
own hash moved. A set edit that moved no hash would leave that caller accepted
against a signature that no longer admits it, and would leave its stored
footprint under-reporting what it can now reach. That is the same argument ADR
0012 §3 makes for keeping `where` constraints in the hash, and it has the same
answer.

Since expansion happens in the parser and the row normalizer already sorts and
deduplicates atoms, the headline property falls out of machinery that exists:
`ply_hash::normalize::row` writes the same bytes for both spellings.
`BODY_ENCODING` therefore does **not** move, and a corpus with no `effect set`
hashes byte-identically to what it hashed under W2. That is a required test.

### 1.6 What an over-broad alias actually costs

ADR 0009 names the risk — "an over-broad alias used everywhere degrades
signatures back toward `IO`" — and calls it a review concern. It is more than
that, and W3 states the two mechanical costs so they can be measured rather
than worried about.

A declared row wider than the body's inferred row is legal and always has been.
An alias makes it systematic, because one set is written for a whole service and
most endpoints touch a part of it. Two things follow:

- **The conflict graph widens.** `DefInfo::footprint` is the declared row, so
  two tests reaching two endpoints that share a set contend on every atom in it,
  and the scheduler serialises tests that could have run side by side. The
  `isolated: n of m` number falls, honestly, for a reason nobody wrote down.
- **Every frame condition weakens.** DESIGN.md §7 makes the footprint the frame
  condition: an `ensures` means *this holds of the result, and every resource
  outside the footprint's writes is unchanged*. A wider footprint promises less
  about less, and `ply prove` would report the same tier over a weaker claim.

So the difference is published rather than left to be discovered.
`DefInfo::performed` carries the row inference computed for the body — always a
subset of `footprint`, equal to it for an unannotated definition — and it is
provenance: it enters no hash, no cache key, no scheduling decision, and no
determinism verdict. `ply check --types --explain` prints the difference, and
`ply prove` prints it for any definition carrying an obligation, where it is a
weakened claim rather than a scheduling cost.

Because a definition's `DefInfo` is restored from `CachedDef` when gate 1 skips
its file, `performed` and `row_aliases` are stored alongside `scheme` and
`footprint`. `ply check --types --explain` must print the same bytes for a warm
run and a cold one; anything else makes the reviewing command's output a
function of what the cache held.

### 1.7 What `--explain` prints

`ply check --types` prints the **expansion**, always, and never the alias. That
is ADR 0009 §4's "the reviewing commands should print it rather than the alias",
in its strongest form: the truth needs no flag at all.

`--explain` adds the set table and the provenance:

```
$ ply check --types --explain

   api.routes  src/api/routes.ply
     effect set Web
       = {db.read[inventory], db.read[orders], db.read[users],
          db.write[orders], http.write[outbound], log.write}
       used by 4 definitions

     table         : () -> List<Route<Endpoint>>
     endpoint_of   : (Request) -> Matched<Endpoint>
     create_order  : (Request) -> Response
                     / {db.read[inventory], db.read[orders], db.read[users],
                        db.write[orders], http.write[outbound], log.write}
       written as     / {Web}
       body performs  {db.read[inventory], db.read[users], db.write[orders],
                       log.write}
       declared, not performed: db.read[orders], http.write[outbound]
```

`table` and `endpoint_of` are pure, and their empty rows are the point of §5.

### 1.8 New diagnostic codes

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0114 | `UNKNOWN_EFFECT_SET` | a row or a set names an `effect set` this module does not declare; a qualified `m::Web`; `pub effect set` | the program's |
| E0115 | `EFFECT_SET_CYCLE` | a set contains itself, directly or through another | the program's |

Two sets with one name in one module are `E0105 DUPLICATE_DEFINITION`, because
that is what the code already means. A set name collides with nothing else: it
lives in no namespace `resolve` knows about, since expansion has erased it
before `resolve` runs, so `effect set Web` beside `type Web` is legal and means
two different things in two different positions.

E0114 carries the module-local rule in its notes, because a qualified reference
and a missing declaration have the same fix and a reader who has just written
`shared::Web` needs to be told why it cannot work rather than that it was not
found.

---

## 2. HTTP/1.1

`std.http` is Ply source, shipped by `ply-std` like `std.json` and `std.net`. It
adds nothing to `ply hosts`.

`examples/hello.ply` already argues this, and W3 is the milestone that makes the
argument load-bearing: "a parser inside a host handler joins the trusted
computing base, where nothing checks it and `ply hosts` can only print its name.
Written here it is ordinary code — its row is empty, `ply test` selects it
exactly, and a defect in it is a failing test rather than a line in the TCB."
Every rule in §3 is a `det`, cacheable, hermetic test because of that decision.

### 2.1 Representation

```ply
type Method  = Get | Head | Post | Put | Patch | Delete | Options | Other(String)
type Version = Http10 | Http11

// Field names are lowercased on parse; a name may repeat, so the value is the
// list of its field lines in order of appearance. Nothing is combined: a
// comma-joined `set-cookie` is a different document, and combining is where a
// header the program reads stops being the header that arrived.
type Headers = Map<String, List<String>>

type Request = {
  method:    Method,
  target:    String,     // the request-target exactly as received
  path:      String,     // up to the first `?`, still percent-encoded
  query:     String,     // after the first `?`, or ""
  authority: String,     // absolute-form's authority, else the `Host` field
  version:   Version,
  headers:   Headers,
  trailers:  Headers,    // never merged into `headers` — see §3.7
  body:      Bytes,      // empty when the body was streamed
}

type Response = { status: Int, headers: Headers, body: Bytes }
```

`Headers` is a `Map`, so its iteration order is ascending by name and canonical
(ADR 0012 §2) — which is what makes a response's bytes a function of its value
and a golden test over them stable.

### 2.2 The parse is two phases, and the first is pure

```ply
type Framing = NoBody | Length(Int) | Chunked

type Head = { request: Request, framing: Framing, consumed: Int,
              keep_alive: Bool, expects_continue: Bool }

// Every framing refusal is a status and a sentence, and every one of them
// closes the connection. There is no refusal that is safe to resynchronise
// from: the position of the next message is exactly what was not established.
type Refusal = { status: Int, reason: String }

type HeadResult =
  | Incomplete            // the terminator has not arrived; read more
  | Parsed(Head)
  | Refused(Refusal)

pub fn parse_head(buf: Bytes, limits: Limits) -> HeadResult
```

`parse_head`'s row is `{}`. It is a function from bytes and limits to a result,
and it is the whole of §3.

`consumed` is where the body begins in `buf`, so a caller never has to re-derive
it and a pipelined second request is `bytes_slice(buf, consumed + body_len, ..)`.

### 2.3 Bodies

Decoding is pure; only refilling the buffer performs `net`.

```ply
type BodyStep =
  // Needs more bytes. `out` is what this step produced, `consumed` is how much
  // of `buf` it absorbed.
  | Await({ state: BodyState, consumed: Int, out: Bytes })
  | Complete({ consumed: Int, out: Bytes, trailers: Headers })
  | Rejected(Refusal)

pub fn body_start(framing: Framing, limits: Limits) -> BodyState
pub fn body_step(state: BodyState, buf: Bytes) -> BodyStep
```

The buffered form, which is what a handler that wants a whole body calls:

```ply
type BodyResult =
  | Body({ bytes: Bytes, rest: Bytes, trailers: Headers })
  | BodyRefused(Refusal)

pub fn read_body(conn: Int, framing: Framing, buf: Bytes, limits: Limits)
  -> BodyResult / {net.write[conn]}
```

`rest` is the leftover, which is the next pipelined request's beginning.

A streamed response does not need a new language feature — it needs a row
variable, which the language has:

```ply
pub fn respond_chunked<s | e>(
  conn: Int, version: Version, r: Response, keep_alive: Bool, limits: Limits,
  seed: s, produce: (s) -> Option<{ chunk: Bytes, next: s }> / e
) -> Bool / {net.write[conn] | e}
```

`version` is the *request's*, and it decides the framing. HTTP/1.0 has no chunked
transfer coding — a 1.0 client reads `5\r\nhello\r\n` as body bytes — so a 1.0
peer is sent no `Transfer-Encoding`, is told `Connection: close`, and gets the
bytes themselves with the close as the message length. `true` means the whole
response was written and the connection may be reused; `false` means it may not,
and a `false` from a chunked stream has still written the terminating chunk.
There is no path that leaves a chunked response framed and unterminated, because
a caller that reused such a connection would have its next response read as chunk
data by the client.

The producer is an ordinary function with its own row, and `respond_chunked`
threads it. A handler that streams a database cursor publishes the cursor's
atoms and nothing else — which is the same sentence as the rest of this
language.

### 2.4 The serve loop

```ply
pub fn serve_connection<e>(conn: Int, limits: Limits,
                           app: (Request) -> Response / e)
  -> Unit / {net.write[conn] | e}

pub fn serve<e>(listener: Int, limits: Limits,
                app: (Request) -> Response / e)
  -> Unit / {net.write[listener], net.write[conn] | e}
```

`app` is the program's dispatch — §5's `route` followed by the program's own
`match`. Its row is inferred and is the union of the endpoints it can reach,
which is exactly right and is what `ply check --types` prints.

**Every connection `serve` handles shares the resource label `conn`**, because a
resource label is a ground identifier in the source and W1 has nothing else.
Two connections therefore conflict. Inside one run this costs nothing — a
production region schedules on real readiness, not on footprints — and what it
does mean is that two *tests* that each serve a connection are placed in one
concurrency group. Stated rather than discovered; a program wanting per-listener
labels writes its own loop with its own labels.

### 2.5 Keep-alive

- HTTP/1.1 is persistent by default; `Connection: close` ends the connection
  after the response is written.
- HTTP/1.0 closes by default; `Connection: keep-alive` opts in.
- The connection is not reused after **any** `Refusal`, at any point.
- A response always carries exactly one framing. `encode` computes
  `Content-Length` for a buffered body; `respond_chunked` writes
  `Transfer-Encoding: chunked` to an HTTP/1.1 peer and, to an HTTP/1.0 peer —
  which has no chunked transfer coding — writes `Connection: close` and lets the
  close delimit the message, which is HTTP/1.0's own framing. A chunked response
  is **always terminated**, including when `max_stream_chunks` runs out: framed
  and unterminated on a connection the caller reuses is response smuggling, and
  it is the one shape this section has to rule out rather than assume away.
- After `limits.max_keep_alive` requests the connection is closed with
  `Connection: close` on the last response. A connection that a peer can hold
  forever by sending one byte a minute is a slow-loris; the idle timeout and
  this bound are the two halves of the answer.
- Pipelined requests are answered in order, which is by construction: one
  connection is served by one task, and the leftover buffer is carried across
  iterations.
- **If the handler did not consume the request body, the server drains it up to
  `limits.max_body` and closes past that.** An unread body is the next message's
  bytes as far as the connection is concerned, and reading the next request out
  of an unread body is a smuggle the server performs on itself.

### 2.6 Responses, and the one thing `encode` refuses

```ply
pub fn encode(method: Method, version: Version, keep_alive: Bool, r: Response)
  -> Bytes
```

- 1xx, 204 and 304 carry no body and no `Content-Length`.
- A response to `HEAD` carries the `Content-Length` the same request would have
  produced under `GET`, and no body.
- A `100 Continue` is written before the body is read when the request declared
  `Expect: 100-continue` and the head was accepted. Any other `Expect` value is
  **417**. A client that waits for a `100` it never gets is a hang, and a hang
  is the failure shape with nothing to read.
- **A header name that is not a token, or a value containing CR or LF, is
  `E0502 RUNTIME_ERROR` naming the header.** Never stripped and never escaped.
  That value came from the program — from a path segment, a database row, a
  redirect target — and silently sanitizing it turns a response-splitting
  attempt into a response the attacker partly controls and nobody noticed. A
  refusal is loud, attributable and bisectable.

---

## 3. Framing, exactly — this is where the security bugs live

Every rule below is stated as *what the server does*, and every one of them is a
required test. Where the RFC permits a choice, W3 takes the refusing branch and
says why. All of these close the connection.

### 3.1 Line terminators and the header block

1. **A line is terminated by CRLF and by nothing else.** A bare LF terminating a
   line is `400`. A bare CR anywhere inside the header block is `400`. Accepting
   a bare LF is the classic desync: two implementations disagreeing about where a
   line ended is two implementations disagreeing about where a message ended.
2. **Obsolete line folding** — a header line beginning with SP or HTAB — is
   `400`. RFC 9112 §5.2 permits replacing it with SP; refusing is safe and
   nothing has sent one since 2007.
3. **Whitespace between a field name and its colon** (`Foo : bar`) is `400`.
   RFC 9112 §5.1 makes this a MUST, and it is a MUST because it is a smuggling
   vector.
4. A field name that is not a token, or a field value containing a control
   character other than HTAB, is `400`.
5. The header block terminator is the first `CRLF CRLF`.

### 3.2 The request line

6. The method must be a token; otherwise `400`. An unrecognised token-valid
   method parses as `Other(name)` and reaches routing, which answers `405`.
7. The target must be **origin-form** (`/path?query`), **absolute-form**
   (`http://host/path`), or **asterisk-form** (`*`, and only with `OPTIONS`).
   Authority-form is `400`: this is a server, not a proxy. An asterisk with any
   other method is `400`.
7a. **SP or HTAB anywhere in the request line** beyond the two spaces that split
   it is `400`. RFC 9112 §3 is `method SP request-target SP HTTP-version` and
   §11.2 names recovering from whitespace here as a smuggling vector, because a
   recipient that splits on HTAB and one that does not disagree about what the
   target was. The line scan cannot answer this: it deliberately admits HTAB,
   which is legal inside a field *value*.
7b. **A target carrying a fragment (`#`) is `400`, in either form.** RFC 9112
   §3.2 gives the request-target no fragment component. Under absolute-form the
   fragment would otherwise *become* `Request::path` — `http://h#a/b` produced
   `path = "#a/b"`, which `std.router` splits into exactly the segments `/a/b`
   does, while an intermediary that parses the target as a URI reference sees
   `/`. Two answers to "which path is this" is the hazard §5 exists to remove.
7c. **An absolute-form target whose authority is empty, or carries a userinfo
   subcomponent (`@`), is `400`.** Rule 8 exposes the authority so a program can
   compare it against `Host`; `http://trusted.example@evil.example/x` would hand
   it `trusted.example@evil.example`, which a prefix check, a substring check or
   a log line reads in the attacker's favour. RFC 9110 §4.2.4 says to treat
   userinfo in an `http` URI from an untrusted source as an error, and every
   request-target is one.
7d. **The scheme is compared as `Bytes`.** Slicing the target into a `String` to
   read it is a remote kill: the target is checked for UTF-8 as a whole, a slice
   at a fixed offset cuts wherever that offset falls, and `string_of_bytes` is
   strict — `GET €€€ HTTP/1.1` is nine bytes cut mid-character, which is `E0502`
   unwinding out of the serve loop rather than a `400` ending one connection.
8. Under absolute-form the target's authority is the request's `authority` and
   the `Host` field is not consulted for it — RFC 9112 §3.2.2 — and **both are
   exposed as data** so a program that cares about a mismatch can say so. The
   parser does not decide that question; it refuses to hide it.
9. `HTTP/1.1` and `HTTP/1.0` are accepted. A request line with no version is
   `400` (HTTP/0.9 is not served). Any other `HTTP/x.y` is `505`.
10. A request line longer than `limits.max_request_line` is `414`.

### 3.3 `Host`

11. An HTTP/1.1 request with no `Host` field is `400`. More than one `Host`
    field is `400`. Both are RFC 9112 §3.2 MUSTs and both are routing-confusion
    vectors.

### 3.4 `Content-Length`

12. **A `Content-Length` field line appearing more than once is `400`, even when
    every value agrees.** RFC 9112 §6.3 permits treating identical values as
    one. W3 refuses, because agreement here is agreement between two field lines
    in *this* message and says nothing about how an intermediary in front of the
    server picked one. A duplicate is a message two implementations may frame
    differently, which is the definition of the bug.
13. A `Content-Length` value that is not one or more ASCII digits is `400`. No
    sign, no internal whitespace, no `0x`, no non-ASCII digits, and a value with
    a comma in it is a list and is covered by rule 12.
14. A `Content-Length` greater than `limits.max_body` is `413`, decided before
    a single body byte is read.

### 3.5 `Transfer-Encoding`

15. **`Content-Length` and `Transfer-Encoding` both present is `400`.** RFC 9112
    §6.1 lets a server reject; W3 rejects. The alternative — preferring
    `Transfer-Encoding` and ignoring `Content-Length` — is correct only if every
    hop in front of the server made the same choice, and CL.TE/TE.CL smuggling is
    exactly the case where one did not. No body is read.
16. `Transfer-Encoding` whose final coding is not `chunked` is `400`: the
    message length cannot be determined.
17. A transfer coding other than `chunked` — `gzip`, `deflate`, `compress`,
    `identity` — is `501`, **including when it appears beside `chunked`**.
    Rule 16 is decided first, which is the one place the order is observable:
    `chunked, gzip` has no decidable length and is `400`, while `gzip, chunked`
    is framed unambiguously and is `501`. Accepting the second would hand the
    handler undecoded `gzip` as `Request::body` with nothing saying so, and would
    leave an intermediary that honours the coding and a server that ignores it
    disagreeing about what the body was.
18. `Content-Encoding` is end to end and is **not** a transfer coding. It is
    passed through untouched. W3 decodes nothing.

### 3.6 Chunked bodies

19. A chunk-size line is hex digits, optionally followed by `;` and chunk
    extensions, terminated by CRLF. A size that is empty, contains a non-hex
    character, or is longer than 16 hex digits is `400`. **A size that does not
    fit in an `Int` is `413`, decided before it is accumulated.** Sixteen hex
    digits is sixty-four bits and `Int` is signed, so the digit bound alone does
    not keep the accumulator from overflowing — and an integer overflow is
    `E0502 RUNTIME_ERROR`, which is not a `Refusal`: it unwinds out of
    `body_step`, out of `serve_connection` and past the accept loop. The bound as
    originally written was therefore a whole-server kill in one unauthenticated
    packet. Leading zeros are not counted, because `0000000000000005` is a legal
    spelling of five and refusing it would be a second parser disagreeing about a
    length.
20. A chunk size greater than `limits.max_chunk_size` is `413`.
21. A running total of chunk data greater than `limits.max_body` is `413`.
22. A chunk-size line longer than `limits.max_chunk_line` is `400`, **and the
    line is not buffered past the bound** — the scan carries the bound as
    `bytes_scan`'s `max`.
23. Chunk extensions are parsed for framing and discarded. They are bounded by
    rule 22 and by nothing else, and nothing reads them.
24. A chunk's data must be followed by CRLF. Anything else is `400`.
25. The terminating chunk is `0` followed by CRLF, then the trailer section,
    then CRLF.

### 3.7 Trailers

26. The trailer section is parsed under the same rules as the header block, with
    `limits.max_trailer_bytes`, and is exposed as `Request::trailers`.
27. **Trailers are never merged into `headers`.** A trailer named
    `content-length`, `transfer-encoding`, `host` or `authorization` changes
    nothing about framing, routing or authorization. Merging them is a documented
    smuggling and privilege-escalation route, and the merge is not an
    optimisation anybody asked for.

### 3.8 Sizes

28. A header block — every field line after the request line, up to and
    including the terminator — longer than `limits.max_header_bytes` is `431`.
29. More than `limits.max_header_count` field lines is `431`.
30. Neither is decided by buffering the whole block first: the read loop stops
    accumulating at `max_request_line + max_header_bytes` and answers.

### 3.9 Timeouts

31. The whole head must arrive within `limits.header_timeout_ms`, measured from
    the first byte of the request. Expiry is `408` and close.
32. The whole body must arrive within `limits.body_timeout_ms`. Expiry is `408`
    and close.

**A deadline is enforced by dividing it, because Ply has no clock and W3 does not
add one.** `net.recv` takes a per-operation deadline, which `ply_host::tcp::recv`
turns into one `set_read_timeout` per syscall; passing the whole timeout to each
read restarts it on every byte, so the only real bound is the read count — 2048
reads x 5000 ms is 2.8 hours on one socket, and `serve` is a sequential accept
loop, so that is the whole server. So each read carries `timeout / max_reads()`
and the budget is the number of slices: the whole message costs at most its
deadline, whatever packet boundaries the peer chose. A slice that expires with no
bytes is an ordinary read and not a refusal — the refusal is the budget running
out — and a peer sending bytes as fast as the deadline requires never notices,
because what a slice costs is granularity rather than patience. Rule 33's wait is
one read carrying the whole `idle_timeout_ms`, since nothing is being assembled
yet.
33. On a kept-alive connection, `limits.idle_timeout_ms` between the previous
    response and the next request's first byte closes the connection with **no
    response** — there is no request to answer.
34. A write that does not complete within `limits.write_timeout_ms` closes the
    connection.

### 3.10 The property that ties them together

The framing rules are one claim: **for every input, `parse_head` either refuses
or determines exactly one message boundary.** That is the anti-smuggling
property and it is stated as a test rather than as an aspiration — a generated
corpus of adversarial heads, each parsed, with the assertion that no accepted
head admits two body lengths and no accepted head's `consumed` disagrees with a
reference framing table.

---

## 4. Limits

```ply
type Limits = {
  max_request_line:  Int,   //   8192
  max_header_bytes:  Int,   //  65536
  max_header_count:  Int,   //    100
  max_body:          Int,   // 1048576
  max_chunk_size:    Int,   // 1048576
  max_chunk_line:    Int,   //   4096
  max_trailer_bytes: Int,   //   8192
  max_keep_alive:    Int,   //    100
  max_stream_chunks: Int,   //   2048
  header_timeout_ms: Int,   //   5000
  body_timeout_ms:   Int,   //  30000
  idle_timeout_ms:   Int,   //   5000
  write_timeout_ms:  Int,   //  30000
}

pub fn default_limits() -> Limits
```

A record, because Ply has no top-level values and because a limit set is data
like a route table: a test can quantify over it, `derive json for Limits` works,
and a service can print the bounds it is running under. There is no global, no
environment variable and no flag, so two runs of one program cannot differ in
what they refuse.

A limit that is zero or negative is `E0502 RUNTIME_ERROR` at `serve`, naming the
field. A server with `max_body: 0` accepts no request with a body and that is a
configuration nobody meant.

`max_stream_chunks` is the one bound on a *write*: the most chunks one
`respond_chunked` produces. It lives here rather than in a constant because this
record is where a program's bounds are readable, and it is a bound at all because
a server that streams without one streams forever. Exhausting it terminates the
message and answers `false`.

> **Corrected by ADR 0022 (2026-08-27).** The second reason given here was
> *"and it is a bound at all because every loop in Ply is a tail call charged
> against the evaluator's nested-call budget"*. That was true of `stream_chunks`
> and it made this field's largest usable value a fact about
> `ply_eval::limit::DEFAULT_MAX_CALLS` rather than about HTTP —
> `max_stream_chunks: 50000` raised `recursion limit of 10000 nested calls
> exceeded`. `stream_chunks` and `stream_raw` now drive their loops with
> `iterate`, which is depth 1 however long it runs, so the field bounds only
> what it says it bounds. The sentence is **not** true of the whole language
> either: `map`, `filter`, `fold`, `map_fold`, `bytes_position` and `iterate`
> are all depth 1. It remains true of `serve` and `connection_loop` in the same
> file, which ADR 0022 records as out of its scope.

**The bound must cost the bound.** Every scan in the head parser passes a
`Limits`-derived `max` to `bytes_scan` / `bytes_scan_until`, and the read loop
searches for the header terminator with `bytes_index_of_from` starting three
bytes before the previous end rather than from zero. W2 removed the property
that a request's cost is proportional to its bytes; a parser that re-scans the
accumulated buffer on every read would restore it as O(n²), quietly, and the
test for it is the counting harness rather than a stopwatch.

`bytes_concat` in a read loop is the same hazard in the other direction: N reads
concatenated pairwise is O(total²). W3 adds one builtin for it:

| builtin | type | notes |
| --- | --- | --- |
| `bytes_concat_all` | `(List<Bytes>) -> Bytes` | one allocation, O(total), the empty list is `b""` |

**Cheap slicing of a shared `Bytes` is deliberately not in W3.** ADR 0011 §8
deferred the question here, and the answer is no: with `bytes_concat_all` the
read loop is O(total) once and the head/body split is one copy, so the
representation change buys a constant factor in a place nothing has measured,
at the price of changing `Value`'s one enum and every path that matches on it.
W6 has the measurement that would justify it.

---

## 5. Routing

### 5.1 The table is data, and it holds no handlers

```ply
type Segment =
  | Literal(String)
  | Param(String)      // captures one segment under this name
  | Rest(String)       // captures every remaining segment, joined by "/"

type Route<a> = { method: Method, path: List<Segment>, endpoint: a }

type Matched<a> =
  | NotFound
  | MethodNotAllowed(List<Method>)   // sorted, deduplicated — the `Allow` field
  | Found({ endpoint: a, params: Map<String, String> })

pub fn route<a>(table: List<Route<a>>, method: Method, path: String) -> Matched<a>
```

`route`'s row is `{}`. That is the first thing `ply check --types` prints about
a service and it is worth more than it looks: routing is the part of a web
framework that is hardest to test and easiest to get subtly wrong, and here it
is a pure function over a value, selected exactly by the test cache and
quantifiable in an M8 `law`.

**The endpoint is a tag, not a closure.** A table of closures cannot work and
the reason is instructive: `List` is homogeneous, function types carry their
rows, and Ply has no subsumption, so every handler in one list would have to
have byte-identically the same row — which would force every endpoint to declare
the union of the whole service and destroy exactly the per-endpoint legibility
this milestone exists to produce.

With a tag, the program writes its own dispatch:

```ply
type Endpoint = ListOrders | GetOrder | CreateOrder | Health

fn dispatch(e: Endpoint, req: Request, params: Map<String, String>) -> Response =
  match e {
    ListOrders  -> list_orders(req),
    GetOrder    -> get_order(req, params),
    CreateOrder -> create_order(req),
    Health      -> health(),
  }
```

and that `match` is **exhaustiveness-checked** (`E0205`), so an endpoint in the
table with no handler is a compile error rather than a 500 at 3am. No framework
gives that, and it costs one `match`.

### 5.2 Matching

- The path is the request's `path` — the target up to the first `?`.
- It is split on `/`. A leading `/` produces no empty first segment. **Empty
  segments are kept**: `/orders/` has a trailing empty segment and `/a//b` has an
  empty middle one, and neither is normalized away. A silent normalization is a
  second answer to "which path is this", and two answers is how a route and an
  authorization check come to disagree. `normalize_path` is available for
  programs that want one, and it is a call the program makes.
- **Percent-decoding happens per segment, after splitting.** So `%2F` decodes to
  a `/` *inside* one segment and can never introduce a segment boundary. This is
  the path-traversal and route-confusion rule and it is the reason the order is
  stated rather than left to an implementation.
- **Decoding costs the segment's length and never its square, and never
  recurses per escape.** `route` reaches `percent_decode` before it has decided
  anything, so this is on the path of every request: an accumulator grown with
  `push` in a **non-final** sub-expression made k escapes cost O(k²), measured at
  125.8 ms for a 7,681-byte path of escapes against 0.1 ms for the same-length
  plain path, at a length the default `max_request_line` admits. §4's rule is
  not only about the head parser.

  > **Corrected (mechanism sweep, 2026-08-28): the measurement stands, the
  > parenthetical does not.** This read *"an accumulator built with `push` —
  > **which copies a `List`** — made k escapes cost O(k²)"*. The 125.8 ms against
  > 0.1 ms is re-affirmed, and so is everything §4 draws from it; only the cause
  > is withdrawn. `push` does not copy a `List`. It grows one **in place** when
  > the caller is its last owner — that is what `Arc::get_mut` decides in
  > `crates/ply-eval/src/builtins.rs` — and copies the whole array only when
  > something else still holds a reference. What put the old `percent_decode` in
  > the copying branch was **position**: `rc::carry`
  > (`crates/ply-eval/src/rc.rs:98`) hands a pending frame a live clone of the
  > scope whenever any sub-expression of the enclosing node remains, and never
  > asks what those remaining sub-expressions read — so an accumulator grown
  > anywhere but last is at two owners by the time `push` looks, and the
  > sub-expression that costs the copy can be a literal constant. The rule
  > composes across call boundaries: a correctly written callee is made quadratic
  > by its caller. `spikes/ply-lexer/GAPS.md` §1 is the rule; ADR 0020 §5.2 is
  > the measurement of the composition.
  >
  > The difference is not pedantry, because *avoid `push`* is not a fix anyone
  > can apply: `push` is the language's only list primitive and
  > `crates/ply-std/ply/trace.ply`'s own `cons` is written out of it. The remedy
  > that landed here removed the *accumulator* — one native split, one call per
  > escape, one allocation for the join. Where an accumulator is the right shape,
  > the fix is to build it in the last sub-expression of its enclosing node.
- An invalid percent escape — `%` not followed by two hex digits — leaves the
  bytes as written rather than raising; a segment is a `String` and the request
  already parsed.
- Matching is byte-exact and case-sensitive. Method names are the uppercase
  tokens RFC 9110 defines.
- `Rest` matches zero or more remaining segments and **must be the last
  segment** of its route.

### 5.3 Precedence

Two routes may both match. The winner is decided **segment by segment, left to
right**: at the first position where the two patterns differ in kind,
`Literal` beats `Param` beats `Rest`. A route with fewer segments never beats
one with more at a position it does not have.

If two routes are still tied — the same method and the same pattern — the
**earlier entry in the list wins**, and that is a defect rather than a feature:

```ply
pub fn conflicts<a>(table: List<Route<a>>) -> List<{ first: Int, second: Int }>
pub fn well_formed<a>(table: List<Route<a>>) -> List<{ index: Int, reason: String }>
```

`conflicts` reports every tie; `well_formed` reports a `Rest` that is not last,
a duplicate `Param` name in one route, and an empty pattern. Both are pure, both
are data, and a service writes

```ply
test "the route table is unambiguous" {
  assert_eq(conflicts(table()), []);
  assert_eq(well_formed(table()), [])
}
```

which is a `det`, cached test over the shape of the table. That is what "the
route table as ordinary data" is *for*: the property a framework enforces with a
macro is here a value a test asserts about.

### 5.4 404 and 405

- No route's path pattern matches, under any method: `NotFound`. The program
  answers `404` with whatever body it likes.
- At least one route's path pattern matches but none with this method:
  `MethodNotAllowed(ms)`, where `ms` is the sorted, deduplicated list of methods
  that do match. The `405` response **must** carry `Allow: <those methods>` —
  RFC 9110 §15.5.6 makes it a MUST, and `std.http.method_not_allowed(ms)` builds
  it so a program cannot forget. That function has to exist for the sentence to
  mean anything: while it did not, the guarantee was a convention the one example
  happened to follow.
- `OPTIONS *` is not routed; `serve` answers it with the methods the table
  declares anywhere, or the program may handle it itself.

---

## 6. TLS

### 6.1 TLS is not a separate effect

It is the same `net` effect, with one new operation for creating a listener.

The alternative was considered and is worse. A separate `tls` effect with the
same five operations makes every function that touches a socket exist twice,
because Ply cannot abstract over two effects that declare the same operations —
there is no effect polymorphism over effect *names*, and adding one for this
would be a language feature bought to express a distinction the type system does
not need. `std.http` would fork, and two forks of a framing parser is two
parsers that disagree, which §2 exists to prevent.

The question is what an effect row *claims*. It claims which resources a
computation touches and whether two computations contend. A TLS connection and a
plaintext one are the same resource, contend the same way, and are read and
written by the same code. Encryption is a property of the transport, not of the
resource, and putting it in the row would make every row in the service carry a
fact that decides nothing the row is used for.

**So the row does not say whether a connection is encrypted. The listener
does**, and it says so in the one place a reviewer looks:

```ply
pub nondet effect net {
  write listen[s](port: Int) -> Int
  write listen_tls[s](port: Int, credential: String) -> Int
  ...
}
```

`ply hosts` prints `net.listen_tls` as its own line with its own handler path,
so "this program can serve TLS" is a fact in the trusted computing base listing
rather than an inference from a row.

A third option — a TLS record layer written in Ply over a small `crypto` effect,
which would make the state machine testable — is refused outright. A
hand-written TLS implementation is a security defect with a schedule, and
nothing about this project's thesis is improved by owning one.

### 6.2 The key never enters the program

`listen_tls` takes a **credential name**, not certificate bytes and not a file
path.

- Certificate bytes as a `Bytes` literal would put a private key into a
  definition's hash and into a content-addressed store that is designed never to
  forget.
- A file path in the program would put a file read inside a `net` operation,
  where `ply hosts` discloses a socket and nothing discloses the file — which is
  ADR 0008 §2's unenforceable residual, deliberately widened.

So the material is configured beside the run and named from it:

```
$ ply run service.ply --host --tls api=certs/api.pem,certs/api.key
```

`--tls NAME=CERT,KEY` is repeatable. PEM: a certificate chain, leaf first, and a
private key in PKCS#8, PKCS#1 or SEC1. Everything is loaded and validated at
**bind time, before anything runs**:

- a file that cannot be read, a PEM that does not parse, a file with no
  certificate or no private key, or a key that does not match the leaf
  certificate, is `E0430 TLS_CREDENTIAL_INVALID`, naming the file;
- `net.listen_tls` naming a credential the binding does not hold is `E0429
  TLS_CREDENTIAL_UNKNOWN` at the perform site, listing the credentials that were
  configured;
- under `ply test` without `--host`, `net.listen_tls` is `E0424` like every
  other host operation, naming `ply_host::tls::listen`.

### 6.3 What the handshake does, and what its failure looks like

**The handshake is completed lazily, on the first `recv` or `send`, and never
inside `accept`.** A handshake inside `accept` means one client sending garbage
takes down the accept loop, which is a denial of service delivered by design.

A handshake that fails — an unsupported version, a bad ALPN, a client that
disconnects mid-flight, a client that sends something that is not TLS —
**closes that connection and nothing else**. Every subsequent operation on the
handle behaves as end of stream: `recv` answers `Some(b"")` and `send` answers
`Some(0)`. The server's ordinary "the peer went away" path handles it, which is
the path it must already have, and the accept loop survives.

Silence would be wrong, so the failure is counted and named: the run's `--host`
summary reports refused handshakes with their reasons, and `--json` carries the
counts. A handshake failure is not a Ply diagnostic — it is not the program's
fault, not Ply's fault, and not attributable to any definition.

**ALPN offers exactly `http/1.1`.** A client that offers only `h2` is refused at
the handshake rather than served HTTP/1.1 bytes over a connection it will parse
as HTTP/2. This is not a nicety: every browser offers `h2` first, and a server
that negotiates it and then speaks 1.1 produces a connection error the client
reports as the server being broken.

### 6.4 What `ply hosts` must disclose

The TCB now contains a TLS stack, and a listing that hides it is the failure
ADR 0008 §2 exists to prevent.

```
$ ply hosts --host
   7 host handlers · 8 operations · trusted computing base

   OPERATION                 ATOM                  HANDLER                    DET  LINEAR         BLOCKING
   net.accept[listener]      net.write[listener]   ply_host::tcp::accept      no   at-most-once   yes
   net.close[conn]           net.write[conn]       ply_host::tcp::close       no   at-most-once   no
   net.listen[listener]      net.write[listener]   ply_host::tcp::listen      no   at-most-once   no
   net.listen_tls[listener]  net.write[listener]   ply_host::tls::listen      no   at-most-once   no
   net.recv[conn]            net.write[conn]       ply_host::tcp::recv        no   at-most-once   yes
   net.send[conn]            net.write[conn]       ply_host::tcp::send        no   at-most-once   yes
   task.spawn                task.write            ply_host::task::scheduler  no   repeatable     no

   transport
   tls        rustls 0.23.43 · provider ring · TLS 1.3, TLS 1.2 · alpn http/1.1

   credentials
   api        sha256:9f2c1a4e8b03…  2 certificates

   digest: b3:4f19c0a8e2d3
```

`net.recv` and `net.send` serve both transports, and the listing says
`ply_host::tcp::recv` because that is the handler the registry resolves; what
routes a particular socket through rustls is which listener accepted it. The
`transport` block is what makes that visible, and it is why the block exists
rather than being left implicit in a handler path.

**The digest covers the credential names, the provider and the library version;
it does not cover the certificate fingerprint.** A CI check that broke on every
certificate renewal would be a CI check people learn to ignore, and a renewal is
an operational fact rather than a structural change to the TCB. Adding or
removing a credential does move the digest, because that is a structural change.
The fingerprint is printed and is in `--json`.

---

## 7. `net`, amended

Two things W1 left are now correctness problems for a server, and both are
changes to the declaration in `std.net`.

### 7.1 A peer's misbehaviour is not the program's error

Today an I/O error becomes a `Diagnostic` and ends the run. A server that dies
because a client reset a connection is not a server. So:

- Errors that are **the program's** fault stay diagnostics: an unknown handle, a
  handle used under two resource labels, a port outside `1..=65535`, a
  non-positive read bound, a non-positive timeout, an empty `send` payload.
- Errors that are **the peer's** — reset, broken pipe, a failed TLS handshake,
  an aborted connection — are ordinary outcomes: end of stream.
- `accept` answers `0` when the listener is finished; handles ascend from 1 and
  are never reused, so `0` is never a live socket and no new convention is
  needed. A transient accept error is retried inside the handler.

### 7.2 A deadline is an argument, not a cancellation

ADR 0011 deferred cancellation and said "W3's timeouts need it". They do not,
and the reason is worth stating because the cheaper answer is also the better
one.

A cancel path on `Pending` needs a token registry, a race between the cancel and
the completion, a rule for what a cancelled operation returns, and a decision
about what happens to bytes already read off the socket. A deadline on the
operation needs one `setsockopt`, inside a blocking job that already owns the
socket for its duration. The second is one line and has no race.

```ply
pub nondet effect net {
  write listen[s](port: Int) -> Int
  write listen_tls[s](port: Int, credential: String) -> Int
  write accept[s](listener: Int) -> Int
  write recv[s](conn: Int, max: Int, timeout_ms: Int) -> Option<Bytes>
  write send[s](conn: Int, payload: Bytes, timeout_ms: Int) -> Option<Int>
  write close[s](socket: Int) -> Unit
}
```

One rule, stated once: **`None` is a deadline; an empty `Some` is an ending.**

| answer | means |
| --- | --- |
| `recv` → `None` | the deadline expired with no bytes |
| `recv` → `Some(b"")` | the peer has stopped sending |
| `recv` → `Some(bs)` | those bytes, possibly fewer than `max` |
| `send` → `None` | the deadline expired |
| `send` → `Some(0)` | the peer is gone |
| `send` → `Some(n)` | `n` bytes were written |

`accept` takes no timeout: a server accepts until it is shut down, and graceful
shutdown is W5. `timeout_ms <= 0` is `E0502 RUNTIME_ERROR` — a caller that wants
no deadline passes a large one, and being made to write the number down is the
point. `send` of an empty payload is `RUNTIME_ERROR`, which is what keeps
`Some(0)` unambiguous.

`std.net` gains `send_all(conn, payload, timeout_ms) -> Bool`, which loops
correctly and answers `false` on a deadline or a dead peer. Programs call it;
the raw `send` is for programs that are counting bytes.

These signature changes move `std.net`'s hashes and the hashes of everything
reaching them. That is correct: the signature changed, and selection is exact
about it.

**The concurrency bound is unchanged and is stated again**:
`MAX_BLOCKING_OPERATIONS` is 64, one real thread per waiting operation, so a run
can have 64 socket operations in flight. That is the capacity of the W3 server
and it is a number a reviewer can read. Raising it, or moving to a reactor, is
W5/W6 with a measurement.

---

## 8. Versions

| constant | to | why |
| --- | --- | --- |
| `RUNTIME_VERSION` | `0.9.0` | `bytes_concat_all` is a new builtin, and the `net` handlers answer `Option` where they answered a bare value |
| `FRONTEND_VERSION` | `0.11.0` | `RowExpr::aliases`, `Item::EffectSet`, expansion inside the parser, and `DefInfo` / `KnownDef` / `CachedDef` gaining `performed` and `row_aliases` |
| `BODY_ENCODING` | **6, unchanged** | an alias expands to atoms the row encoder already writes, sorted and deduplicated; the alias name is erased |
| `PROVER_VERSION` | **0.4.0, unchanged** | no new generators and no change to what a discharge samples |

`BODY_ENCODING` not moving is a required test, not an observation: a corpus
carrying no `effect set` must hash byte-identically to what it hashed under W2.

---

## 9. Workspace

```toml
rustls = { version = "0.23.43", default-features = false, features = ["ring", "std", "tls12"] }
rustls-pemfile = "2.2.0"
rcgen = "0.14.9"      # dev-dependency of ply-host only
```

`ply-host` gains the first two; `rcgen` is a dev-dependency, for generating a
self-signed certificate in the suite so a TLS test needs no fixture on disk and
no network.

Notes on each, because a dependency in the TCB is a review obligation:

- **rustls 0.23.43**, the latest stable 0.23. `cargo search` surfaces
  `0.24.0-dev.1`, which is a pre-release and must not be used.
- **`ring` rather than `aws-lc-rs`.** `aws-lc-rs` is rustls's default and needs a
  C toolchain and cmake on some platforms; `ring` needs neither. The provider is
  **installed explicitly** rather than taken from a default feature, so the
  single line that decides it is the line `ply hosts` names. Switching is that
  line and a string in the transport block.
- **`tokio-rustls` is not used**, though the milestone brief allows it. W1's
  sockets are blocking `std::net` on a pool owned by `ply-host` — a deliberate
  decision, since nothing a work-stealing runtime would steal is `Send` — so
  `rustls::StreamOwned` over the existing blocking stream is the fit, and
  `tokio-rustls` would require an async socket layer that nothing else in this
  workspace has.
- Related and worth fixing while here: **`tokio` is a declared dependency of
  `ply-host` that no code uses.** ADR 0011 justified it as the reactor and timer
  wheel, and the implementation went a different way. Remove it or use it; a
  dependency in a trusted computing base that nothing calls is a line a reviewer
  spends attention on for nothing.

`ply-std` gains two modules and no dependency. `ply-eval` gains one builtin and
no dependency.

---

## 10. New diagnostic codes

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0114 | `UNKNOWN_EFFECT_SET` | a row or a set names an `effect set` this module does not declare; a qualified set reference; `pub effect set` | the program's |
| E0115 | `EFFECT_SET_CYCLE` | a set contains itself, directly or through another | the program's |
| E0429 | `TLS_CREDENTIAL_UNKNOWN` | `net.listen_tls` named a credential the binding does not hold | the run's configuration |
| E0430 | `TLS_CREDENTIAL_INVALID` | a `--tls` credential that does not load: unreadable, malformed PEM, no certificate, no key, or a key that does not match the leaf | the run's configuration |

**Nothing in §2, §3, §4 or §5 has a diagnostic code**, and that is the point of
writing the protocol in Ply. A malformed request is a `400`, which is a
`Response` value the program returns — data, testable, with no compiler
involvement at all.

---

## 11. Required tests

The ones whose absence would let W3 ship broken rather than merely incomplete.

**Effect sets**

1. A definition written `/ {Web}` and one written with `Web`'s expansion have
   **identical** `DefHash`es.
2. Renaming a set, reordering its members, or declaring an unused one changes no
   hash and selects zero tests.
3. Changing which atoms a set contains moves exactly the hashes of the
   definitions annotated with it and their transitive dependents, and selects
   exactly the tests reaching them.
4. A corpus with no `effect set` hashes byte-identically to W2 — `BODY_ENCODING`
   did not move.
5. A body performing an atom outside the expansion is `E0302` whose secondary
   label quotes the **expansion**, not the alias name.
6. A set naming itself, directly or through another, is `E0115` naming the cycle
   in order. A row naming an undeclared set, a qualified `m::Web`, and
   `pub effect set` are all `E0114`. Two sets with one name are `E0105`.
7. `ply check --types` prints the expansion and never the alias.
   `--types --explain` prints the set table, the alias a row was written with,
   the body's inferred row, and the declared-but-not-performed difference — and
   its **bytes are identical** whether gate 1 parsed the file or skipped it.
8. Two definitions annotated with one over-broad set are placed in one
   concurrency group, and `--explain` names the atoms that put them there.

**HTTP framing** — every one a hermetic `det` test over a pure function

9. `Content-Length` and `Transfer-Encoding` both present: `400`, connection
   closed, no body read.
10. Two `Content-Length` field lines: `400`, **including when the values agree**.
    `Content-Length: 5, 5`: `400`.
11. `Content-Length` values `+5`, ` 5`, `5x`, `0x5`, and a non-ASCII digit: each
    `400`. A value over `max_body`: `413` before a body byte is read.
12. `Transfer-Encoding: chunked, gzip`: `400`. `Transfer-Encoding: gzip`: `501`.
    `Transfer-Encoding: gzip, chunked`: `501` — the framing is unambiguous and
    the coding is still one this server cannot decode.
13. A chunk size that is empty, non-hex, over 16 digits, or over
    `max_chunk_size`: `400` / `413`. A chunk not followed by CRLF: `400`. Chunk
    data summing over `max_body`: `413`. A size at or above
    `8000000000000000`, which is sixteen legal hex digits and does not fit in an
    `Int`: `413` and **not** an integer overflow, which would end the run.
13a. A request line holding an HTAB, a target holding a `#`, and an absolute-form
    target whose authority holds an `@` or is empty: each `400`. A non-origin-form
    target whose bytes are multi-byte UTF-8: `400`, and never a `string_of_bytes`
    on a slice cut mid-character.
14. A chunk-size line over `max_chunk_line` is `400` **and the counting harness
    shows no scan examined more than the bound**.
15. Trailers are exposed separately and never merged: a trailer
    `content-length: 0` changes no framing decision, and a trailer
    `authorization:` never appears in `headers`.
16. `Foo : bar`, an obs-fold line, a bare LF line terminator, and a bare CR in a
    value: each `400`.
17. HTTP/1.1 with no `Host`, and with two `Host` fields: each `400`.
18. Header block over `max_header_bytes`: `431`. Over `max_header_count`: `431`.
    Request line over `max_request_line`: `414`. Each without buffering past the
    bound.
19. `HTTP/2.0` in the request line: `505`. No version: `400`. A non-token method:
    `400`. Authority-form target: `400`. `*` with `GET`: `400`.
20. Pipelined requests are answered in order and the leftover buffer is carried.
21. A handler that ignores the body: the server drains to `max_body` and closes
    past it, and the next request is never framed out of an unread body.
22. `Connection: close` on 1.1, plain 1.0, and the `max_keep_alive`th request
    each close; a 1.1 request with no `Connection` is reused.
23. A response header value containing CR or LF is `E0502` at `encode`, naming
    the header — never sanitized.
24. 204, 304 and 1xx carry no body and no `Content-Length`; a `HEAD` response
    carries `Content-Length` and no body; `Expect: 100-continue` gets a `100`
    before the body is read, and any other `Expect` gets `417`.
24a. A streamed response whose producer outlasts `max_stream_chunks` still ends
    with its terminating chunk, and answers `false` so the connection is not
    reused. A streamed response to an HTTP/1.0 peer carries no
    `Transfer-Encoding` and says `Connection: close`.
25. **The anti-smuggling property**: over a generated corpus of adversarial
    heads, every accepted head admits exactly one body length and its `consumed`
    agrees with a reference table; nothing is accepted with two framings
    available.
26. **The cost property**: the W2 head-length sweep is re-run against
    `std.http.parse_head` over heads grown to 8 KB of fields the parser never
    reads, and the time is flat in the head's length.
    *Enforced by `the_cost_of_a_head_is_flat_in_the_length_of_a_field_it_does_not_read`
    (`crates/ply-corpus/tests/http_cost.rs:143`), which sweeps pad lengths
    `0, 64, 256, 1024, 4096, 8192` and asserts on the ratio rather than on
    absolute microseconds. A second sweep in the same file covers the
    field-**count** direction, which this entry does not mention and should:
    growing fields must cost linearly, not quadratically.*
26a. **The same property for routing**: four times the percent-escapes in one
    path cost about four times the time and not sixteen, and a path of escapes
    longer than the interpreter's nested-call limit returns a value rather than
    ending the run.
    *First clause enforced by `routing_a_path_of_escapes_costs_its_length_and_not_its_square`
    (`crates/ply-cli/tests/w3_http_audit.rs:694`), comparing `escapes(11)`
    against `escapes(13)`. Note the assertion is `four <= one * 9.0`, not "about
    four": the test's own doc explains the slack — quadratic would be 16x and
    9x is chosen so a contended machine cannot redden it while a reintroduced
    copying accumulator cannot green it. That is a defensible threshold, and it
    is looser than this line reads.*
    **Second clause not demonstrated.** `ply_eval::DEFAULT_MAX_CALLS` is
    `10_000` (`crates/ply-eval/src/limit.rs:23`). The largest escape path any
    test builds is `escapes(13)` = `3 · 2^13` = 24,576 bytes, i.e. 8,192
    escapes — under the limit, so it does not exercise the case. The nearest
    thing is `"a path with two thousand segments is answered"`
    (`crates/ply-cli/tests/routing_audit.rs:322`), which is 2,000 *segments*,
    also under the limit and not escapes. So nothing checks that a path of
    escapes past 10,000 returns a value rather than ending the run with a
    recursion-limit diagnostic. Whichever way that case actually behaves, it is
    unmeasured.

**Routing**

27. `route`, `conflicts` and `well_formed` publish `{}` in `ply check --types`.
28. Literal beats parameter beats wildcard, decided left to right; a tie goes to
    the earlier entry and `conflicts` reports it.
29. A path matching no route is `NotFound`; one matching under another method is
    `MethodNotAllowed` with sorted, deduplicated methods, and the built `405`
    carries `Allow`.
30. `%2F` in a segment decodes after splitting and never introduces a boundary.
31. `/orders` and `/orders/` are different paths and both route.
32. A `Rest` that is not last, a repeated `Param` name, and an empty pattern are
    each reported by `well_formed`.
33. `derive json for Route<Endpoint>` works and a table round-trips.
34. A `match` over `Endpoint` missing an arm is `E0205` — the table and the
    dispatch cannot drift.

**TLS**

35. `net.listen_tls` with an unconfigured credential is `E0429` listing the ones
    configured; without `--host` it is `E0424` naming `ply_host::tls::listen`.
36. A malformed PEM, a missing key, and a key that does not match the leaf are
    each `E0430` **before anything runs**.
37. A request over TLS against an rcgen-generated certificate returns the same
    bytes as the plaintext path, with **no change to the service's source**.
38. A client offering only `h2` is refused at handshake; one offering `http/1.1`
    succeeds.
39. A failed handshake makes `recv` answer `Some(b"")`, leaves the accept loop
    running, and is counted in the run summary.
40. `ply hosts --host` lists `net.listen_tls`, the transport block naming rustls
    and its provider, and the credentials by name and fingerprint. `--digest` is
    stable across a certificate rotation and moves when a credential is added or
    removed.

**`net`**

41. `recv` answering `None` on a deadline is distinguishable from `Some(b"")` at
    end of stream, under the scripted twin.
42. `timeout_ms <= 0` and an empty `send` payload are each `RUNTIME_ERROR`;
    a reset peer is end of stream and not a diagnostic.
43. `send_all` answers `false` rather than looping when the peer is gone.
44. A header timeout is `408` and close; a body timeout is `408` and close; an
    idle timeout closes with no response written. And the deadline is the
    *message's*: the sum of every timeout the server asks a `recv` for over one
    dribbling head is no more than `header_timeout_ms`, and over one dribbling
    body no more than `body_timeout_ms`. A peer sending one byte per read is
    answered `408` rather than held for hours.

**Everything W3 must not regress**

45. Renaming a top-level function selects zero tests; moving a definition
    between modules changes no hash — on a corpus with effect sets, a route
    table and a TLS listener.
46. Incremental and `--no-incremental` agree byte-for-byte across the full
    mutation sequence, with `effect set` edits added to it.
47. `--engine both` reports no `E0503` on the W3 corpus.
48. `E0412` still fires for an unsimulated nondeterministic effect in a `det`
    test; `ply test` is hermetic without `--host` and says so.
49. `Store::open` at 10,000 definitions stays under 5 ms.
50. `ply prove` reports honest tiers and `ply hosts` lists the TCB, on the W3
    corpus.

Plus one `tests/fixtures/` entry per new code.

---

## Not in W3

- **A database.** W4. `db.*` in this document's examples is an illustration of a
  row, not a shipped effect.
- **Authentication, authorization, sessions and cookies.** A cookie parser is
  ordinary Ply and a program can write one; shipping an authentication framework
  before there is a database or a secret type (W5) would be shipping a shape
  nothing can implement correctly yet.
- **HTTP/2 and HTTP/3.** ALPN advertises `http/1.1` and only `http/1.1`, which
  is the honest form of not having them.
- **A template language.** JSON is the payload W2 built for and the one W3
  serves.
- **Compression.** No `gzip`, `deflate` or `br`, in either direction.
  `Content-Encoding` is passed through untouched.
- **mTLS, SNI-based certificate selection, session resumption, and OCSP.** One
  credential per listener.
- **`Upgrade`, WebSockets and `CONNECT`.** Authority-form targets are `400`.
- **Cross-module `effect set`s.** §1.3 states the mechanism the sound version
  needs.
- **Effect sets over row variables** — `effect set Handler<e> = {db.read[users],
  log.write | e}`. ADR 0009 already deferred it and W3 does not need it.

  *(Written `{db, log | e}` here until the W6 documentation audit. That is the
  whole-effect form §1.2 of this same document refuses, so the example spelled
  the deferred feature in syntax the milestone had just made illegal. Corrected
  to atoms, which is how ADR 0009's own "Not in this ADR" writes it.)*
- **Cheap slicing of a shared `Bytes`.** ADR 0011 §8 deferred the question to
  W3; §4 answers it, and the answer is no, with the measurement W6 would need to
  change it.
- **Cancellation of a `Pending` token.** §7.2 explains why deadlines removed the
  need rather than deferring it again. A host operation with no deadline still
  blocks until it completes or the run ends.
- **Graceful shutdown and connection draining.** W5.
- **More than 64 host operations in flight.** §7.2.
