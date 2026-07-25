# Crate contracts

These public APIs are fixed. Crates are implemented concurrently against them,
so a signature here is a promise other crates have already been written to call.
Add anything you like *beyond* these; do not change what is written here. If a
contract is genuinely unworkable, implement the closest thing that compiles and
say so in your report rather than silently diverging.

`ply-span` and the pinned module `ply-core::ty` are already written and tested.
Read them before starting — they answer most questions. `ply-syntax::ast` is no
longer pinned; the Modules section below is the record of what changed in it.

---

## Modules

`docs/adr/0001-modules.md` has the reasoning; this section is the contract.
**Where it disagrees with a per-crate section below, this section wins** — the
module system was landed after them.

### The rule everything else follows from

The namespace is metadata over hashes. A cross-module reference normalizes to
the referent's `DefHash` exactly as a same-module reference does. Module names,
`import` declarations and `pub` are **erased by normalization**. Therefore
renaming a definition changes no hash, and *moving a definition from one module
to another changes no hash anywhere either*. Both are required tests.

Two identical definitions in different modules have the same hash and share one
cache entry. That is correct, not a collision to break.

### Names

One file is one module; the name is derived from the path relative to the
project root, minus extension, separators to dots: `src/store/orders.ply` is
`store.orders`. Every path segment must be an identifier, else
`INVALID_MODULE_PATH`.

**Every `Symbol` that names a definition, type, effect or constructor in any
public API is the program-wide name** — `store.orders.place`, not `place`. This
covers `CheckOutput`'s four maps, `HashOutput`'s `defs` / `deps` / `closure`,
`Failure::suspects`, and `NameRef::name` in the incremental front end. A `.`
cannot occur in an identifier, so a qualified name never collides with a name
written in source. Where a struct also carries `simple_name`, that is the name
as the source wrote it.

Tests are keyed `<module>.<label>` (`TestInfo::key`), which is what keeps two
identically-labelled tests in different modules distinct.

`EffectAtom::effect` holds the **qualified** effect name, so two modules each
declaring an `effect db` do not contend. Resource labels are **not** qualified:
`[users]` in two modules is one resource, because conflict is a claim about the
world rather than about a file.

### Syntax

```
file      := importDecl* item*
importDecl:= "import" modulePath ("as" IDENT | "(" IDENT,* ")")?
item      := "pub"? (fnDef | typeDef | effectDef) | testDef
qname     := (IDENT "::")? IDENT
```

Imports precede every item. `as` and a name list are mutually exclusive. A
selective import binds **no** module binder. `pub` on a `type` exports its
constructors with it. `test` cannot be `pub`.

References are qualified with `::`, never `.`: `orders::place(x)`,
`store::db.get[users](k)`, `orders::Order`, `orders::Placed(x)`. With `.` a
qualified call is token-identical to a `perform` and to field access, and the
parser has no scope information to tell them apart — see the ADR.

### Resolution order — implement exactly this

Bare name, first match wins:

1. local binders, innermost outward — they always win;
2. the current module's own items **plus** every selectively-imported name, one
   flat table per namespace;
3. prelude builtins.

A collision within step 2 is an error, never silent shadowing: two local items
are `DUPLICATE_DEFINITION`, two imports are `DUPLICATE_IMPORT`, an import versus
a local item is `AMBIGUOUS_IMPORT`.

Qualified name `m::x` — no ordering, no shadowing, never ambiguous:

1. `m` must be a module binder imported in **this file**, else `UNKNOWN_MODULE`;
2. `x` must exist in `m` (`UNKNOWN_NAME`, noting the module) and be `pub`
   (`PRIVATE_NAME`, pointing at the declaration).

Namespaces are values (functions and constructors together), types, and effects.
Module binders are a fourth namespace reachable only through `::`, so a local
variable named `orders` does not hide the module binder `orders`.

Module-level import cycles are `MODULE_CYCLE`, naming the cycle in order.
Definition-level mutual recursion **within** a module is unaffected and still
goes through the existing SCC path.

### New diagnostic codes

Added to `ply_span::codes`; existing numbers are unchanged.

| code | constant | when |
| --- | --- | --- |
| E0106 | `UNKNOWN_MODULE` | no module imported under that binder |
| E0107 | `PRIVATE_NAME` | the name exists but is not `pub` |
| E0108 | `AMBIGUOUS_IMPORT` | an imported name collides with a local item |
| E0109 | `MODULE_CYCLE` | modules import each other |
| E0110 | `DUPLICATE_IMPORT` | two imports bind the same name |
| E0111 | `INVALID_MODULE_PATH` | a path segment is not an identifier |

### AST — landed in `ply-syntax::ast`

```rust
pub struct ModuleName(Symbol);                 // "store.orders"
impl ModuleName {
    pub fn anonymous() -> ModuleName;          // the empty name; snippets only
    pub fn is_anonymous(&self) -> bool;
    pub fn from_relative_path(path: &Path) -> Result<ModuleName, Diagnostic>;
    pub fn from_dotted(name: impl AsRef<str>) -> ModuleName;
    pub fn as_symbol(&self) -> &Symbol;
    pub fn as_str(&self) -> &str;
    pub fn segments(&self) -> impl Iterator<Item = &str>;
    pub fn default_binder(&self) -> Symbol;    // last segment
    pub fn qualify(&self, name: &Symbol) -> Symbol;
}

pub struct QName { pub module: Option<Ident>, pub name: Ident, pub span: Span }
impl QName {
    pub fn bare(name: Ident) -> QName;
    pub fn qualified(module: Ident, name: Ident) -> QName;
    pub fn is_bare(&self) -> bool;
    pub fn symbol(&self) -> &Symbol;           // the simple name
}
impl From<Ident> for QName;
impl Display for QName;                        // "orders::place"

pub enum Visibility { Private, Public }        // Default = Private

pub struct Program { pub modules: Vec<Module> }
impl Program {
    pub fn single(module: Module) -> Program;
    pub fn find(&self, name: &ModuleName) -> Option<&Module>;
    pub fn index_of(&self, name: &ModuleName) -> Option<usize>;
}

pub struct Module {
    pub name: ModuleName,
    pub source: SourceId,
    pub imports: Vec<ImportDecl>,
    pub items: Vec<Item>,
}

pub struct ImportDecl { pub path: Vec<Ident>, pub kind: ImportKind, pub span: Span }
pub enum ImportKind { Module, Alias(Ident), Names(Vec<Ident>) }
impl ImportDecl {
    pub fn module_name(&self) -> ModuleName;
    pub fn path_span(&self) -> Span;
    pub fn binder(&self) -> Option<Symbol>;    // None for a selective import
    pub fn binder_span(&self) -> Span;
}

impl Item {
    pub fn name(&self) -> Option<&Ident>;      // None for a test
    pub fn visibility(&self) -> Visibility;
}
```

`vis: Visibility` was added to `FnDef`, `TypeDef` and `EffectDef`. These fields
changed from `Ident` to `QName`: `ExprKind::Var`, `ExprKind::Perform::effect`,
`HandleClause::effect`, `AtomExpr::effect`, `TypeExpr::Con::name`,
`PatternKind::Ctor::name`. `TypeExpr::Var` stays an `Ident` — a type parameter
is bound by the enclosing `<..>`, never by a module.

`lexer::is_ident` is public; `Kw::Pub`, `Kw::Import` and `TokenKind::ColonColon`
exist.

### Resolution — `ply-syntax::resolve`

Purely syntactic, needs no types, and is the **single** place the module
namespace lives. Inference, hashing and evaluation all consume it rather than
each re-deriving what a name means.

```rust
pub enum Namespace { Value, Type, Effect }
pub struct Declared { pub qualified: Symbol, pub vis: Visibility, pub span: Span }
pub struct Declarations {                      // per module, keyed by simple name
    pub values: IndexMap<Symbol, Declared>,
    pub types: IndexMap<Symbol, Declared>,
    pub effects: IndexMap<Symbol, Declared>,
}
pub struct Binding { pub qualified: Symbol, pub owner: usize, pub span: Span }
pub struct Scope {
    pub module: ModuleName,
    pub modules: IndexMap<Symbol, (usize, Span)>,   // binder -> module index
    pub values: IndexMap<Symbol, Binding>,
    pub types: IndexMap<Symbol, Binding>,
    pub effects: IndexMap<Symbol, Binding>,
}
pub struct Resolved {
    pub scopes: Vec<Scope>,                    // parallel to Program::modules
    pub declarations: Vec<Declarations>,       // parallel to Program::modules
    pub index: IndexMap<Symbol, usize>,        // module name -> module index
    pub order: Vec<usize>,                     // dependency-first; acyclic
}
impl Resolved {
    pub fn lookup(&self, module: usize, ns: Namespace, q: &QName)
        -> Result<&Binding, Diagnostic>;
}
pub fn resolve(program: &Program) -> Result<Resolved, Vec<Diagnostic>>;
```

`resolve` reports unknown modules, cycles, duplicate import bindings,
import/local collisions, and selective imports of a missing or private name.
Reference-site failures are **not** found there — bodies are walked by the
consumer, which calls `lookup` and reports what it returns. `lookup`'s caller
must have already ruled out a local binder for a bare name.

### Changed signatures

```rust
// ply-core — `check_module` is kept as the single-module convenience wrapper.
pub fn check_program(program: &Program, resolved: &Resolved)
    -> Result<CheckOutput, Vec<Diagnostic>>;

// ply-hash
pub fn hash_program(program: &Program, resolved: &Resolved, check: &CheckOutput)
    -> Result<HashOutput, Vec<Diagnostic>>;

// ply-eval
impl<'a> Interp<'a> {
    pub fn new(program: &'a Program, resolved: &'a Resolved, check: &'a CheckOutput) -> Self;
}

// ply-test
pub fn run(selection: &Selection, program: &Program, resolved: &Resolved,
           check: &CheckOutput, hashes: &HashOutput, store: &mut Store) -> RunReport;
```

`CheckOutput` gained `modules: IndexMap<Symbol, ModuleInfo>`. `DefInfo`,
`EffectInfo` and `CtorInfo` gained `module: ModuleName` and
`simple_name: Symbol`, with `name` holding the program-wide name and equal to
the map key. `TestInfo` gained `module` and `key`.

### `ply test <dir>` no longer concatenates

Every `*.ply` under the root is its own module, and a name in one file is
invisible in another until exported and imported. This replaces the old
behaviour outright.

`examples/` needs **no changes to work**: `clock.ply`, `ledger.ply` and
`store.ply` are independent, so they become the modules `clock`, `ledger` and
`store` and check exactly as before. No module header is needed and `pub` is
optional on a definition nobody imports. `examples/clock.ply`'s `nondet effect
clock` inside module `clock` is not a collision under `::`.

Required so imports are exercised end to end: one new example that imports from
another module, which means adding `pub` to the handful of items it names. The
`pub` edits must change no hash — that is test 4 below, observed on real code.

`ply run` needs a rule for `main`, since there is no longer exactly one. A file
argument means that file's module. A directory means exactly one module may
declare `main`; if several do, report the candidates rather than picking by load
order.

### Required tests

1. Moving a definition between modules changes **no** hash in the program.
2. Renaming a definition referenced from another module changes no hash.
3. Renaming a module — renaming its file — changes no hash.
4. Adding or removing `pub`, adding, removing or reordering `import`s, and
   `as`-renaming an import all change no hash.
5. Two modules each declaring `effect db` produce non-conflicting atoms; a
   `[users]` label shared across two modules produces conflicting ones.
6. Two structurally identical definitions in different modules hash identically.
7. `a -> b -> a` is `MODULE_CYCLE` naming both modules; self-import likewise.
8. One `tests/fixtures/` entry per new code.
9. A local binder named `orders` does not hide the module binder `orders`.
10. A selectively-imported name colliding with a local definition is
    `AMBIGUOUS_IMPORT`, and qualifying the reference fixes it.
11. `ply test examples/` still selects zero tests after a top-level rename.

---

## ply-syntax

```rust
pub mod ast;      // written; do not modify
pub mod lexer;
pub mod parser;
pub mod resolve;

pub fn parse(source: SourceId, text: &str) -> Result<ast::Module, Vec<Diagnostic>>;
pub fn parse_module(source: SourceId, name: ModuleName, text: &str)
    -> Result<ast::Module, Vec<Diagnostic>>;
pub fn parse_program<'a>(inputs: impl IntoIterator<Item = (SourceId, ModuleName, &'a str)>)
    -> Result<ast::Program, Vec<Diagnostic>>;
```

`parse` is `parse_module` under `ModuleName::anonymous()`. `parse_many` is gone;
see the Modules section for what replaced it and why.

### Grammar

```
file      := importDecl* item*
importDecl:= "import" modulePath ("as" IDENT | "(" IDENT,* ")")?
modulePath:= IDENT ("." IDENT)*
item      := "pub"? (fnDef | typeDef | effectDef) | testDef
qname     := (IDENT "::")? IDENT
fnDef     := "fn" IDENT generics? "(" params ")" ("->" type)? ("/" row)? ("=" expr | block)
generics  := "<" IDENT,* ("|" IDENT,*)? ">"        // types, then effect vars
typeDef   := "type" IDENT ("<" IDENT,* ">")? "=" (type | variants)
variants  := "|"? variant ("|" variant)*
variant   := IDENT ("(" type,* ")")?
effectDef := "nondet"? "effect" IDENT "{" opDef* "}"
opDef     := ("read"|"write") IDENT ("[" IDENT "]")? "(" type,* ")" "->" type
testDef   := "test" ("/" "nondet")? STRING block

row       := "{" atom,* ("|" IDENT)? "}" | IDENT
atom      := IDENT "." ("read"|"write") ("[" IDENT "]")?

expr      := ... standard precedence, listed below
block     := "{" stmt* expr? "}"
stmt      := "let" pattern (":" type)? "=" expr ";" | expr ";"

perform   := IDENT "." IDENT ("[" IDENT "]")? "(" expr,* ")"
handle    := "handle" expr "with" "{" hClause* retClause? "}"
hClause   := IDENT "." IDENT ("[" IDENT "]")? "(" IDENT,* ")" "->" expr ","?
retClause := "return" IDENT "->" expr ","?
withCell  := "with_cell" "[" IDENT "]" "(" expr ")" "{" IDENT "->" expr "}"
```

Precedence, loosest to tightest: `||`, `&&`, comparison (`== != < <= > >=`),
`++` (string concat), `+ -`, `* / %`, unary `- !`, application / field access /
indexing. `if`, `match`, `handle`, `with_cell`, lambda (`|x, y| expr`) are
primary expressions.

Comments are `//` to end of line. Integers are decimal, optionally `_`
separated. Strings are double-quoted with `\n \t \\ \" \r \0` escapes.

Recover from errors where cheap — report several parse errors per run rather
than stopping at the first. Every node must carry a real span.

---

## ply-core

```rust
pub mod ty;       // written; do not modify
pub mod env;
pub mod unify;
pub mod infer;

pub struct EffectInfo {
    pub name: Symbol,
    pub nondet: bool,
    pub ops: IndexMap<Symbol, OpInfo>,
}
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
}

pub struct DefInfo {
    pub name: Symbol,
    pub scheme: Scheme,
    /// Closed after solving. Empty for a pure function.
    pub footprint: Footprint,
}

pub struct TestInfo {
    pub name: String,
    pub index: usize,          // position among tests in the module
    pub nondet: bool,
    pub footprint: Footprint,
}

pub struct CheckOutput {
    pub defs: IndexMap<Symbol, DefInfo>,
    pub tests: Vec<TestInfo>,
    pub effects: IndexMap<Symbol, EffectInfo>,
    pub ctors: IndexMap<Symbol, CtorInfo>,   // variant name -> arity, owning type
}

pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>>;
```

Requirements:

- Hindley–Milner with let-polymorphism over `Type`, extended so every expression
  also yields a `Row`.
- Row unification per DESIGN.md §1: `{A|ρ1} ~ {B|ρ2}` solves
  `ρ1 := (B\A) ∪ ρ3`, `ρ2 := (A\B) ∪ ρ3` with ρ3 fresh, plus an occurs check.
- The `handle` rule from DESIGN.md §2:
  `(row(body) \ handled) ∪ ⋃ row(clause_i)`.
- `with_cell[r](init) { c -> body }` binds `c : Cell<T>` and discharges
  `cell.read[r]` / `cell.write[r]` from the body's row. `cell_get` / `cell_set`
  are prelude builtins that perform those atoms.
- A `/ {...}` annotation is an upper bound: inference must produce a subset, and
  the annotation becomes the published signature. Violation is `EFFECT_NOT_PERMITTED`.
- After solving, a definition's row must be closed. A surviving row variable in a
  top-level signature that was not generalized is `UNBOUND_ROW_VAR`.
- A `test` that is not `nondet` whose final footprint contains an atom belonging
  to an effect declared `nondet` is `NONDET_IN_DET_TEST` (code `E0412`). The
  message must name the offending atom and point at the performing expression.

Use the codes in `ply_span::codes`. Report as many independent errors as you can
per run.

---

## ply-hash

```rust
pub mod normalize;
pub mod graph;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct DefHash(pub [u8; 32]);

impl DefHash {
    pub fn to_hex(&self) -> String;          // 64 chars
    pub fn short(&self) -> String;           // first 12 hex chars, for display
}
impl std::fmt::Display for DefHash;          // == short()

pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,                 // parallel to CheckOutput::tests
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

pub fn hash_module(module: &Module, check: &CheckOutput)
    -> Result<HashOutput, Vec<Diagnostic>>;
```

Normalization, per DESIGN.md §3 — this is the crate's whole point, get it exact:

- Local binders become de Bruijn **levels**; a local's *name* never enters the hash.
- A reference to another top-level definition contributes **that definition's
  hash**, never its name.
- Names, spans, comments, and formatting are erased. Declared type and effect
  annotations are kept (they are part of the published signature).
- Serialize deterministically: postorder, every variable-length field
  length-prefixed, every node tagged with a distinct discriminant byte so that
  no two differently-shaped trees can collide.
- `hash = blake3(bytes)`.
- Mutually recursive definitions form an SCC (Tarjan). Hash the component with
  self-references replaced by component-local indices, then each member is
  `blake3(component_hash ‖ index_le_u32)`. Order members by their position in the
  source so the component hash is stable.
- A `test`'s hash covers its body, so it transitively covers everything it calls.

These properties must hold, and there must be a test for each:

1. Renaming a top-level definition changes **no** hash in the program.
2. Renaming a local parameter or `let` binding changes no hash.
3. Reformatting, recommenting, and reordering top-level items change no hash.
4. Editing a function body changes its hash and every transitive dependent's.
5. Two structurally identical definitions with different names hash identically.
6. Swapping two fields, two arguments, or two match arms changes the hash.

---

## ply-store

```rust
pub struct Store { .. }

#[derive(Clone, Serialize, Deserialize)]
pub enum Outcome {
    Pass,
    Fail { message: String, diagnostic: Option<Diagnostic> },
}

impl Store {
    /// Opens/creates `<root>/.ply-cache`.
    pub fn open(root: &Path) -> anyhow::Result<Store>;
    pub fn get(&self, hash: DefHash) -> Option<Outcome>;
    pub fn put(&mut self, hash: DefHash, outcome: Outcome);
    pub fn flush(&mut self) -> anyhow::Result<()>;
    pub fn clear(&mut self) -> anyhow::Result<()>;
    pub fn len(&self) -> usize;
}
```

The cache key is `(RUNTIME_VERSION, DefHash)`; bumping `RUNTIME_VERSION` (a
`pub const &str` in this crate) invalidates everything. JSON on disk is fine —
this is not the bottleneck and being able to read the cache by hand is worth
more than speed. Writes must be atomic (temp file + rename) so an interrupted
run cannot corrupt the cache. A corrupt or unreadable cache must degrade to an
empty cache with a warning, never a crash.

---

## Incremental front end

`ply-store` carries a second cache, versioned and invalidated independently of
the result cache, so that a definition is typechecked and hashed once rather
than on every invocation. `docs/adr/0002-incremental-frontend.md` has the
reasoning and the invalidation table; this section is the contract.

```rust
pub const FRONTEND_VERSION: &str;

/// BLAKE3 over raw bytes. Deliberately not `DefHash`, which is over a
/// normalized definition — the two gates below key on different things.
pub struct ContentHash(pub [u8; 32]);
impl ContentHash {
    pub fn of(bytes: &[u8]) -> ContentHash;
    pub fn to_hex(&self) -> String;      // 64 chars
    pub fn short(&self) -> String;       // 12
    pub fn from_hex(s: &str) -> Option<ContentHash>;
}

pub struct SourceFingerprint {
    pub content_hash: ContentHash,       // of the file's RAW BYTES
    pub imports: Vec<ImportEdge>,
    pub deps: Vec<NameRef>,              // free names, and what each resolved to
    pub defs: Vec<DefEntry>,
    pub tests: Vec<CachedTest>,
}

pub struct DefEntry {
    pub name: Symbol,
    pub hash: DefHash,
    pub span: FileSpan,
    pub kind: DefKind,                   // Fn | Type | Effect
    pub members: Vec<Member>,            // variants of a type, operations of an effect
}
pub struct Member    { pub name: Symbol, pub span: FileSpan }
pub struct NameRef   { pub name: Symbol, pub hash: DefHash }
pub struct ImportEdge { pub module: Symbol, pub exports: ContentHash }
pub struct FileSpan  { pub start: u32, pub end: u32 }
pub struct CachedTest {
    pub name: String, pub hash: DefHash, pub nondet: bool,
    pub footprint: Footprint, pub span: FileSpan, pub name_span: FileSpan,
}

pub struct CachedDef  { pub scheme: Scheme, pub footprint: Footprint, pub names: Vec<NameRef> }
pub struct CachedDecl { pub body: DeclBody, pub names: Vec<NameRef> }
pub enum DeclBody {
    Type   { arity: usize, ctors: Vec<CachedCtor> },
    Effect { nondet: bool, ops: Vec<CachedOp> },
}
pub struct CachedCtor { pub fields: Vec<Type>, pub scheme: Scheme }
pub struct CachedOp {
    pub mode: Mode, pub resource_param: bool,
    pub params: Vec<Type>, pub ret: Type,
}

impl Store {
    pub fn source(&self, path: &Path) -> Option<&SourceFingerprint>;
    pub fn put_source(&mut self, path: &Path, f: SourceFingerprint) -> bool;
    pub fn forget_source(&mut self, path: &Path) -> bool;
    pub fn source_paths(&self) -> Vec<PathBuf>;
    pub fn cached_def(&self, hash: DefHash) -> Option<&CachedDef>;
    pub fn put_def(&mut self, hash: DefHash, def: CachedDef);
    pub fn cached_decl(&self, hash: DefHash) -> Option<&CachedDecl>;
    pub fn put_decl(&mut self, hash: DefHash, decl: CachedDecl);
    pub fn prune(&mut self, keep: &[PathBuf]) -> Pruned;
    pub fn sources_len(&self) -> usize;
    pub fn defs_len(&self) -> usize;
    pub fn decls_len(&self) -> usize;
    pub fn frontend_is_empty(&self) -> bool;
    pub fn frontend_path(&self) -> &Path;
    pub fn root(&self) -> &Path;
}

pub fn exports_digest(exports: &[NameRef]) -> ContentHash;
pub fn witness_holds(names: &[NameRef], resolve: impl FnMut(&Symbol) -> Option<DefHash>) -> bool;
```

### The two gates

**Gate 1 — file level.** Skip parsing a file entirely when its raw content hash
is unchanged *and* every entry of its `deps` still resolves to the same
`DefHash`. Under modules, each `ImportEdge`'s `exports` digest matching is the
cheap conservative pre-check. Its definitions' types and footprints then come
from `cached_def` / `cached_decl`, and its tests from the fingerprint.

**Gate 2 — definition level.** Inside a file that did change, recheck only those
definitions whose `DefHash` is absent from the store. A reference contributes
the referent's hash, so this one condition covers both "its own normalized form
changed" and "a dependency's hash changed".

The keys differ for a reason that must not be optimized away: **a `DefHash`
cannot be computed without parsing**, so gate 1 has to key on raw file content,
while gate 2 keys on `DefHash`. Gate 1 is conservative about formatting; gate 2
is exact.

### Requirements

- Typechecking a definition takes its dependencies' types from the store rather
  than from rechecking them. This is the compile-once mechanism and the one
  place a defect is silent rather than slow.
- A cached interface is only usable while its `names` witness holds — every
  recorded name must still denote the recorded hash. A `DefHash` erases names
  but a `Scheme` is written in them, so the hash alone does not determine the
  interface.
- Schemes must be **canonicalized** — quantified `TyVar`/`RowVar`s renumbered
  from zero in a deterministic traversal order — before being stored or
  compared. `generalize` otherwise emits whatever the run's global counter
  produced, and checking a subset yields alpha-equivalent but textually
  different schemes.
- Spans are persisted as `FileSpan`, file-relative, and rebased onto the run's
  `SourceId`. A `SourceId` is an index into the run's `SourceMap`; adding or
  removing a file shifts every later one.
- Never write a fingerprint for a file that failed to parse or typecheck.
- Nothing consults mtime. Content hashes only.
- `prune` may only be called after a run that discovered every `.ply` file under
  the store root.
- `Store::clear` discards both caches. `FRONTEND_VERSION` and `RUNTIME_VERSION`
  invalidate their own cache and not the other's. Bump `FRONTEND_VERSION` for
  any change to normalization, inference, the representation of `Scheme` or
  `Footprint`, or the prelude's signatures.
- Evaluation still needs an AST. A file gate 1 skipped is parsed on demand when
  a test in it must actually run; if that parse yields `DefHash`es other than
  the ones the fingerprint recorded, fall back to the full path with a warning
  rather than evaluating against a stale interface.

### Soundness — mandatory test

For every corpus, the incremental path must produce **byte-identical**
`DefHash`es, `Scheme`s and `Footprint`s to a full from-scratch check.

`ply check` and `ply test` take `--no-incremental`, which neither reads nor
writes the front-end cache. The equivalence test runs both paths over each
example corpus and compares all three maps in full, across a sequence of
mutations — edit a body, rename a definition, add a file, delete a file, move a
definition between files, reformat — not just the cold case. Comparison is by
equality, never by alpha-equivalence: weakening it would hide exactly the defect
the test exists to catch.

`--no-cache` continues to mean "prove everything again" and disables both
caches.

### Reporting

`ply check --explain` reports per file whether it was parsed or skipped and why
a skip was refused (`content changed`, `dependency <name> changed`,
`import <module> changed`, `no fingerprint`), and per definition whether it was
loaded from cache or rechecked. `ply test --explain` prints the same block for
its front-end phase, before the existing selection and scheduling output.
`ply cache stats` reports both caches' sizes.

---

## ply-eval

```rust
pub enum Value {
    Int(i64), Bool(bool), Str(Arc<str>), Unit,
    List(Vector<Value>),                    // persistent or Arc<Vec<_>>; cheap clone
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor { name: Symbol, args: Arc<Vec<Value>> },
    Closure(Arc<Closure>),
    Cell(Rc<RefCell<Value>>),
}

pub struct Interp<'a> { .. }

impl<'a> Interp<'a> {
    pub fn new(module: &'a Module, check: &'a CheckOutput) -> Self;
    pub fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic>;
    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic>;
}
```

- Tree-walking, environment-passing. Correctness first; speed is M9's problem.
- Effect handlers are a stack of frames. `Perform` walks the stack outward to the
  first frame with a matching `(effect, op, resource)` clause, evaluates that
  clause's body in the handler's captured environment, and returns its value
  directly to the perform site — tail-resumptive, no continuation capture.
- An unhandled `Perform` at runtime is `UNHANDLED_EFFECT`. Inference should have
  ruled this out; it is a bug-catcher, not a user-facing path.
- Recursion depth must be bounded and produce a diagnostic, not a stack overflow.
- Prelude builtins: `assert`, `assert_eq`, `len`, `push`, `map`, `filter`, `fold`,
  `range`, `int_to_string`, `string_concat`, `cell_get`, `cell_set`, `panic`.
  A failing `assert`/`assert_eq` is `ASSERTION_FAILED` with a structured
  expected/actual message.
- An `Interp` must be usable from a worker thread. If `Value` holds `Rc`, keep
  each interpreter and every value it produces confined to one thread and do not
  implement `Send` for it — the scheduler hands each worker its own `Interp`.

---

## ply-test

```rust
pub struct Selection {
    pub total: usize,
    pub cached: Vec<(usize, Outcome)>,     // test index -> cached outcome
    pub to_run: Vec<usize>,
    /// Concurrency groups over `to_run`; every pair within a group has
    /// non-conflicting footprints.
    pub groups: Vec<Vec<usize>>,
}

pub fn select(check: &CheckOutput, hashes: &HashOutput, store: &Store) -> Selection;
pub fn run(selection: &Selection, module: &Module, check: &CheckOutput,
           hashes: &HashOutput, store: &mut Store) -> RunReport;

pub struct RunReport {
    pub passed: usize, pub failed: usize, pub cached: usize,
    pub failures: Vec<Failure>,
    pub duration: Duration,
}
pub struct Failure {
    pub name: String,
    pub diagnostic: Diagnostic,
    /// Definitions in this test's closure whose hash is not in the store —
    /// the suspects for this failure.
    pub suspects: Vec<Symbol>,
}
```

- Selection: a test runs iff its `DefHash` is absent from the store. `nondet`
  tests always run and are never written to the store.
- Grouping: build a conflict graph over the selected tests' footprints and colour
  it greedily, largest-footprint-first. Tests within a group run concurrently via
  `rayon`; groups run in sequence.
- A panic in one test must be caught and reported as a failure, not abort the run.
- Only write `Pass` to the store. A failure must not be cached — a red test has
  to re-run every time until it goes green.

---

## ply-cli

```
ply check [PATH]            typecheck; print inferred signatures with `--types`
ply test  [PATH]            the main event
ply run   [PATH]            evaluate `main`
ply hash  [PATH]            definition hashes, `--deps` to show the graph
ply cache clear|stats
```

`ply test` flags: `--json`, `--explain` (why each test was selected or skipped,
and how groups were formed), `--no-cache`, `--filter <substring>`, `--jobs <n>`.

Default `PATH` is `.`; a directory means every `*.ply` under it, sorted, parsed
as one module. Exit `0` on success, `1` on test failure, `2` on a compile error.
Human output goes to stdout; `--json` emits one JSON object to stdout and nothing
else.

Target output shape:

```
$ ply test
   selected 3 of 47 (44 cached)
   3 groups · 10 workers

   ✓ active_users excludes inactive          1.2ms
   ✓ orders roll up by customer              0.8ms
   ✗ balance never goes negative             2.1ms

   1 failed, 2 passed, 44 cached (0.08s)

   balance never goes negative
     assertion failed: expected 0, found -5
       at src/ledger.ply:88:5
     suspects: apply_debit, Ledger.balance
```
